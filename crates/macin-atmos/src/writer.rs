use crate::{metadata, *};
use macin_bwf::{Container, Format, SampleFormat, TrailingChunk, Writer, WriterOptions};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy)]
struct Ramp {
    start: u64,
    end: u64,
    from: [f64; 3],
    to: [f64; 3],
}
impl Ramp {
    fn fixed(v: [f64; 3]) -> Self {
        Self {
            start: 0,
            end: 0,
            from: v,
            to: v,
        }
    }
    fn at(self, t: u64) -> [f64; 3] {
        if t >= self.end || self.end == self.start {
            return self.to;
        }
        let fraction = t.saturating_sub(self.start) as f64 / (self.end - self.start) as f64;
        std::array::from_fn(|i| self.from[i] + (self.to[i] - self.from[i]) * fraction)
    }
    fn update(self, sample: u64, length: u64, to: [f64; 3]) -> Self {
        Self {
            start: sample,
            end: sample + length,
            from: self.at(sample),
            to,
        }
    }
}
#[derive(Debug, Clone)]
struct Timeline {
    state: ObjectState,
    position: Ramp,
    gain: Ramp,
    extent: Ramp,
    cursor: u64,
    last_event: u64,
    block_number: u32,
}
impl Timeline {
    fn new(state: ObjectState) -> Self {
        Self {
            state,
            position: Ramp::fixed(state.position),
            gain: Ramp::fixed([state.gain.linear(); 3]),
            extent: Ramp::fixed(state.extent.map_or([0.; 3], Extent::xyz)),
            cursor: 0,
            last_event: 0,
            block_number: 0,
        }
    }
    fn update(&mut self, event: ObjectUpdate, state: ObjectState) {
        let length = event.ramp_samples.unwrap_or(0);
        if event.patch.position.is_some() {
            self.position = self.position.update(event.sample, length, state.position);
        }
        if event.patch.gain.is_some() {
            self.gain = self
                .gain
                .update(event.sample, length, [state.gain.linear(); 3]);
        }
        if event.patch.extent.is_some() {
            self.extent = self.extent.update(
                event.sample,
                length,
                state.extent.map_or([0.; 3], Extent::xyz),
            );
        }
        self.state = state;
        self.last_event = event.sample;
    }
    fn write_until(
        &mut self,
        end: u64,
        index: usize,
        spec: &MasterSpec,
        out: &mut dyn Write,
    ) -> Result<()> {
        while self.cursor < end {
            let ramps = [self.position, self.gain, self.extent];
            let next = ramps
                .iter()
                .map(|r| r.end)
                .filter(|v| *v > self.cursor)
                .fold(end, u64::min);
            let mut state = self.state;
            state.position = self.position.at(next);
            state.gain = Gain::Linear(self.gain.at(next)[0]);
            if state.extent.is_some() {
                state.extent = Some(Extent::Cartesian(self.extent.at(next)));
            }
            self.block_number = self
                .block_number
                .checked_add(1)
                .ok_or_else(|| Error::invalid("Too many ADM blocks"))?;
            metadata::write_adm_block(
                out,
                index,
                self.block_number,
                metadata::AdmBlock {
                    sample: self.cursor,
                    duration: next - self.cursor,
                    ramp_samples: if ramps.iter().any(|r| r.end > self.cursor) {
                        next - self.cursor
                    } else {
                        0
                    },
                    state,
                },
                spec.sample_rate,
                spec.profile,
            )?;
            self.cursor = next;
        }
        Ok(())
    }
}
struct Track {
    timeline: Timeline,
    spool: BufWriter<File>,
}
enum Audio {
    Bwf(Writer<BufWriter<File>>),
    Caf(BufWriter<File>),
}
impl Audio {
    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        match self {
            Self::Bwf(writer) => {
                writer.write_pcm_bytes(bytes)?;
            }
            Self::Caf(writer) => writer.write_all(bytes)?,
        }
        Ok(())
    }
}

/// An atomic file/fileset author. Buffers supplied to write_block are borrowed only for that call.
/// Only current per-track states, bounded audio scratch and deduplicated diagnostics stay in RAM.
/// Metadata fragments are stored alongside the output, never in an independent audio cache.
pub struct MasterWriter {
    spec: MasterSpec,
    output: PathBuf,
    temp: PathBuf,
    audio: Option<Audio>,
    tracks: Vec<Track>,
    frames: u64,
    failed: bool,
    diagnostics: Vec<MappingDiagnostic>,
}
impl std::fmt::Debug for MasterWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MasterWriter")
            .field("output", &self.output)
            .field("frames", &self.frames)
            .field("profile", &self.spec.profile)
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}
impl Drop for MasterWriter {
    fn drop(&mut self) {
        self.audio.take();
        self.tracks.clear();
        let _ = fs::remove_dir_all(&self.temp);
    }
}
impl MasterWriter {
    pub fn create(output: impl AsRef<Path>, mut spec: MasterSpec) -> Result<Self> {
        spec.validate()?;
        let output = output.as_ref().to_owned();
        if fs::symlink_metadata(&output).is_ok() {
            return Err(Error::new(ErrorKind::OutputExists, "Output already exists"));
        }
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if !parent.is_dir() {
            return Err(Error::invalid("Output parent must exist"));
        }
        let mut diagnostics = Vec::new();
        for object in &mut spec.objects {
            object.initial = mapped(
                spec.profile,
                spec.loss_policy,
                object.id,
                0,
                object.initial,
                &mut diagnostics,
            )?;
        }
        if spec.profile.is_adm() {
            for (field, lost) in [
                ("screen_size_ratio", spec.screen_size_ratio != 1.),
                ("bed_distribution", spec.bed_distribution),
                ("warp_mode", spec.warp_mode != WarpMode::LoRo),
            ] {
                if lost {
                    loss(
                        spec.loss_policy,
                        None,
                        None,
                        field,
                        "Presentation property has no verified ADM mapping",
                        &mut diagnostics,
                    )?;
                }
            }
        }
        // Validate fixed-width fields before creating any files.
        if spec.profile == Profile::Logic {
            dbmd::build_logic_dbmd(
                spec.channels(),
                spec.frame_rate,
                &spec.dbmd_originator,
                &spec.dbmd_product,
            )
            .map_err(Error::invalid)?;
        }
        let mut temp = None;
        for _ in 0..100 {
            let path = parent.join(format!(
                ".macin-atmos-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    temp = Some(path);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        let temp = temp
            .ok_or_else(|| Error::new(ErrorKind::Io, "Cannot allocate output staging directory"))?;
        let mut this = Self {
            spec,
            output,
            temp,
            audio: None,
            tracks: Vec::new(),
            frames: 0,
            failed: false,
            diagnostics,
        };
        this.initialize()?;
        Ok(this)
    }
    fn audio_path(&self) -> PathBuf {
        self.temp.join(if self.spec.profile.is_adm() {
            "master.wav".into()
        } else {
            format!("{}.atmos.audio", self.spec.stem)
        })
    }
    fn initialize(&mut self) -> Result<()> {
        let file = BufWriter::new(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(self.audio_path())?,
        );
        self.audio = Some(if self.spec.profile.is_adm() {
            Audio::Bwf(Writer::new_deferred(
                file,
                Format {
                    channels: self.spec.channels() as u16,
                    sample_rate: self.spec.sample_rate,
                    sample_format: SampleFormat::Pcm24,
                    channel_mask: None,
                },
                WriterOptions {
                    container: Some(if self.spec.profile == Profile::Logic {
                        Container::Rf64
                    } else {
                        Container::Bw64
                    }),
                    axml: None,
                    chna: Some(metadata::chna(&self.spec)?),
                },
            )?)
        } else {
            let mut file = file;
            crate::containers::write_caf_header(
                &mut file,
                None,
                self.spec.channels(),
                self.spec.sample_rate,
            )?;
            Audio::Caf(file)
        });
        for (i, object) in self.spec.objects.iter().enumerate() {
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(self.temp.join(format!("track-{i}.tmp")))?;
            let mut spool = BufWriter::new(file);
            if !self.spec.profile.is_adm() {
                spool.write_all(
                    metadata::damf_event(
                        (i + 10) as u32,
                        0,
                        ObjectPatch::full(object.initial),
                        Some(0),
                    )?
                    .as_bytes(),
                )?;
            }
            self.tracks.push(Track {
                timeline: Timeline::new(object.initial),
                spool,
            });
        }
        Ok(())
    }
    pub fn frames_written(&self) -> u64 {
        self.frames
    }
    pub fn spec(&self) -> &MasterSpec {
        &self.spec
    }
    fn ready(&self) -> Result<()> {
        if self.failed {
            Err(Error::new(ErrorKind::Failed, "Writer failed; discard it"))
        } else {
            Ok(())
        }
    }
    /// Schedule known future events without retaining the full timeline. Per-track sample order
    /// must be nondecreasing, and events cannot precede already submitted audio.
    pub fn schedule_updates(&mut self, updates: &[ObjectUpdate]) -> Result<()> {
        self.ready()?;
        let mut states: Vec<_> = self.tracks.iter().map(|t| t.timeline.clone()).collect();
        let mut diagnostics = self.diagnostics.clone();
        let mut prepared = Vec::with_capacity(updates.len());
        for event in updates {
            let index = self
                .spec
                .objects
                .iter()
                .position(|o| o.id == event.track)
                .ok_or_else(|| {
                    Error::invalid("Unknown TrackId").context(event.track, event.sample, "track")
                })?;
            let previous = &mut states[index];
            if event.sample < self.frames || event.sample < previous.last_event {
                return Err(
                    Error::invalid("Metadata samples must not go backwards").context(
                        event.track,
                        event.sample,
                        "sample",
                    ),
                );
            }
            if event.patch.has_continuous() && event.ramp_samples.is_none() {
                return Err(
                    Error::invalid("Continuous updates require an explicit ramp length").context(
                        event.track,
                        event.sample,
                        "ramp",
                    ),
                );
            }
            if !event.patch.has_continuous() && event.ramp_samples.is_some() {
                return Err(
                    Error::invalid("Discrete-only updates must not reset a ramp").context(
                        event.track,
                        event.sample,
                        "ramp",
                    ),
                );
            }
            event
                .sample
                .checked_add(event.ramp_samples.unwrap_or(0))
                .ok_or_else(|| Error::invalid("Ramp end overflow"))?;
            metadata::sample_time(event.sample, self.spec.sample_rate, self.spec.profile)?;
            let candidate = event.patch.apply(previous.state);
            candidate
                .validate()
                .map_err(|e| e.context(event.track, event.sample, "state"))?;
            let state = mapped(
                self.spec.profile,
                self.spec.loss_policy,
                event.track,
                event.sample,
                candidate,
                &mut diagnostics,
            )?;
            let mut mapped_event = *event;
            if !self.spec.profile.is_adm()
                && mapped_event.patch.binaural_mode == Some(BinauralMode::Mid)
            {
                mapped_event.patch.binaural_mode = Some(state.binaural_mode);
            }
            if !self.spec.profile.is_adm() && mapped_event.patch.screen_ref.is_some() {
                mapped_event.patch.screen_ref = None;
            }
            previous.update(mapped_event, state);
            prepared.push((index, mapped_event, state));
        }
        // No validation errors after this point; an I/O failure poisons the whole transaction.
        for (index, event, state) in prepared {
            let track = &mut self.tracks[index];
            let result = if self.spec.profile.is_adm() {
                track
                    .timeline
                    .write_until(event.sample, index, &self.spec, &mut track.spool)
            } else {
                metadata::damf_event(
                    (index + 10) as u32,
                    event.sample,
                    event.patch,
                    event.ramp_samples,
                )
                .and_then(|text| track.spool.write_all(text.as_bytes()).map_err(Into::into))
            };
            if let Err(error) = result {
                self.failed = true;
                return Err(error);
            }
            track.timeline.update(event, state);
        }
        self.diagnostics = diagnostics;
        Ok(())
    }

    pub fn write_block(&mut self, pcm: PcmBlock<'_>, updates: &[ObjectUpdate]) -> Result<()> {
        self.ready()?;
        let channels = self.spec.channels();
        let frames = validate_pcm(pcm, channels)?;
        if frames == 0 && !updates.is_empty() {
            return Err(Error::invalid("Metadata requires a nonempty audio block"));
        }
        let end = self
            .frames
            .checked_add(frames as u64)
            .ok_or_else(|| Error::invalid("PCM timeline overflow"))?;
        end.checked_mul(channels as u64)
            .and_then(|v| v.checked_mul(3))
            .ok_or_else(|| Error::invalid("PCM size overflow"))?;
        metadata::sample_time(end, self.spec.sample_rate, self.spec.profile)?;
        if updates
            .iter()
            .any(|e| e.sample < self.frames || e.sample >= end)
        {
            return Err(Error::invalid(
                "Block updates must be within the audio block",
            ));
        }
        self.schedule_updates(updates)?;
        let result = self.write_audio(pcm, frames);
        if let Err(error) = result {
            self.failed = true;
            return Err(error);
        }
        self.frames = end;
        Ok(())
    }
    fn write_audio(&mut self, pcm: PcmBlock<'_>, frames: usize) -> Result<()> {
        let audio = self
            .audio
            .as_mut()
            .ok_or_else(|| Error::new(ErrorKind::Failed, "Audio writer closed"))?;
        if let PcmBlock::S24Le(data) = pcm {
            return audio.write(data);
        }
        let channels = self.spec.channels();
        let mut scratch = Vec::with_capacity(channels * 3 * 1024);
        for first in (0..frames).step_by(1024) {
            scratch.clear();
            for frame in first..frames.min(first + 1024) {
                for channel in 0..channels {
                    let value = match pcm {
                        PcmBlock::F32Interleaved(v) => v[frame * channels + channel],
                        PcmBlock::F32Planar(v) => v[channel][frame],
                        PcmBlock::S24Le(_) => unreachable!(),
                    };
                    // Signed 24-bit full-scale, nearest with ties away from zero. +1 saturates to
                    // the positive endpoint; -1 uses the exact negative endpoint. No dither.
                    let value = (f64::from(value) * 8_388_608.)
                        .round()
                        .clamp(-8_388_608., 8_388_607.) as i32;
                    scratch.extend_from_slice(&value.to_le_bytes()[..3]);
                }
            }
            audio.write(&scratch)?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<WriteReport> {
        self.ready()?;
        if self.frames == 0 {
            return Err(Error::invalid("Empty master"));
        }
        if self
            .tracks
            .iter()
            .any(|t| t.timeline.last_event >= self.frames)
        {
            return Err(Error::invalid(
                "Scheduled metadata lies beyond the PCM duration",
            ));
        }
        for (i, track) in self.tracks.iter_mut().enumerate() {
            if self.spec.profile.is_adm() {
                track
                    .timeline
                    .write_until(self.frames, i, &self.spec, &mut track.spool)?;
            }
            track.spool.flush()?;
        }
        let mut axml_bytes = 0;
        let mut dbmd_bytes = 0;
        match self
            .audio
            .take()
            .ok_or_else(|| Error::new(ErrorKind::Failed, "Audio writer closed"))?
        {
            Audio::Bwf(writer) => {
                let axml_path = self.temp.join("axml.tmp");
                let mut axml = BufWriter::new(File::create(&axml_path)?);
                metadata::write_axml(&mut axml, &self.spec, self.frames, |i, out| {
                    std::io::copy(
                        &mut BufReader::new(File::open(self.temp.join(format!("track-{i}.tmp")))?),
                        out,
                    )?;
                    Ok(())
                })?;
                axml.flush()?;
                axml_bytes = axml.stream_position()?;
                drop(axml);
                let dbmd = if self.spec.profile == Profile::Logic {
                    dbmd::build_logic_dbmd(
                        self.spec.channels(),
                        self.spec.frame_rate,
                        &self.spec.dbmd_originator,
                        &self.spec.dbmd_product,
                    )
                    .map_err(Error::invalid)?
                } else {
                    Vec::new()
                };
                dbmd_bytes = dbmd.len();
                let mut source = BufReader::new(File::open(&axml_path)?);
                let mut dbmd_source = dbmd.as_slice();
                let mut chunks = vec![TrailingChunk {
                    id: *b"axml",
                    size: axml_bytes,
                    source: &mut source,
                }];
                if !dbmd.is_empty() {
                    chunks.push(TrailingChunk {
                        id: *b"dbmd",
                        size: dbmd.len() as u64,
                        source: &mut dbmd_source,
                    });
                }
                let mut file = writer.finish_with_chunks(&mut chunks)?;
                file.flush()?;
                file.get_ref().sync_all()?;
                drop(file);
            }
            Audio::Caf(mut file) => {
                let bytes = self
                    .frames
                    .checked_mul(self.spec.channels() as u64)
                    .and_then(|v| v.checked_mul(3))
                    .ok_or_else(|| Error::invalid("CAF size overflow"))?;
                let size = i64::try_from(
                    bytes
                        .checked_add(4)
                        .ok_or_else(|| Error::invalid("CAF size overflow"))?,
                )
                .map_err(|_| Error::invalid("CAF size exceeds i64"))?;
                file.seek(SeekFrom::Start(56))?;
                file.write_all(&size.to_be_bytes())?;
                file.flush()?;
                file.get_ref().sync_all()?;
                drop(file);
                let mut metadata_file = BufWriter::new(File::create(
                    self.temp.join(format!("{}.atmos.metadata", self.spec.stem)),
                )?);
                writeln!(
                    metadata_file,
                    "sampleRate: {}\nevents:",
                    self.spec.sample_rate
                )?;
                metadata_file.write_all(metadata::damf_bed_events().as_bytes())?;
                for i in 0..self.tracks.len() {
                    std::io::copy(
                        &mut BufReader::new(File::open(self.temp.join(format!("track-{i}.tmp")))?),
                        &mut metadata_file,
                    )?;
                }
                metadata_file.flush()?;
                metadata_file.get_ref().sync_all()?;
                drop(metadata_file);
                let mut manifest =
                    File::create(self.temp.join(format!("{}.atmos", self.spec.stem)))?;
                manifest.write_all(metadata::damf_manifest(&self.spec)?.as_bytes())?;
                manifest.sync_all()?;
            }
        }
        self.tracks.clear();
        for entry in fs::read_dir(&self.temp)? {
            let path = entry?.path();
            if path.extension().is_some_and(|x| x == "tmp") {
                fs::remove_file(path)?;
            }
        }
        let source = if self.spec.profile.is_adm() {
            self.audio_path()
        } else {
            self.temp.clone()
        };
        renamore::rename_exclusive(&source, &self.output)?;
        let objects = self
            .spec
            .objects
            .iter()
            .enumerate()
            .map(|(i, object)| TrackMapping {
                id: object.id,
                track_index: (11 + i) as u16,
                audio_object_id: metadata::object_id((11 + i) as u16),
            })
            .collect();
        Ok(WriteReport {
            frames: self.frames,
            channels: self.spec.channels() as u16,
            sample_rate: self.spec.sample_rate,
            pcm_bytes: self.frames * self.spec.channels() as u64 * 3,
            axml_bytes,
            dbmd_bytes,
            objects,
            diagnostics: std::mem::take(&mut self.diagnostics),
        })
    }
}

fn validate_pcm(pcm: PcmBlock<'_>, channels: usize) -> Result<usize> {
    let valid = |v: f32| v.is_finite() && (-1.0..=1.0).contains(&v);
    match pcm {
        PcmBlock::S24Le(v) => {
            if !v.len().is_multiple_of(channels * 3) {
                return Err(Error::invalid("S24LE block is not whole frames"));
            }
            Ok(v.len() / (channels * 3))
        }
        PcmBlock::F32Interleaved(v) => {
            if !v.len().is_multiple_of(channels) || v.iter().any(|v| !valid(*v)) {
                return Err(Error::invalid("Invalid normalized interleaved PCM"));
            }
            Ok(v.len() / channels)
        }
        PcmBlock::F32Planar(v) => {
            if v.len() != channels {
                return Err(Error::invalid("Planar channel count mismatch"));
            }
            let frames = v[0].len();
            if v.iter()
                .any(|v| v.len() != frames || v.iter().any(|v| !valid(*v)))
            {
                return Err(Error::invalid("Invalid normalized planar PCM"));
            }
            Ok(frames)
        }
    }
}
fn loss(
    policy: LossPolicy,
    track: Option<TrackId>,
    sample: Option<u64>,
    field: &'static str,
    message: &'static str,
    diagnostics: &mut Vec<MappingDiagnostic>,
) -> Result<()> {
    if policy == LossPolicy::Reject {
        let mut error = Error::new(ErrorKind::Unsupported, message);
        error.track = track;
        error.sample = sample;
        error.field = Some(field);
        return Err(error);
    }
    if !diagnostics
        .iter()
        .any(|d| d.track == track && d.field == field)
    {
        diagnostics.push(MappingDiagnostic {
            track,
            sample,
            field,
            message: message.into(),
        });
    }
    Ok(())
}
fn mapped(
    profile: Profile,
    policy: LossPolicy,
    id: TrackId,
    sample: u64,
    mut state: ObjectState,
    diagnostics: &mut Vec<MappingDiagnostic>,
) -> Result<ObjectState> {
    state.validate()?;
    if profile.is_adm() {
        for (field, lost) in [
            ("elevation", !state.elevation),
            ("zone", state.zone != Zone::All),
            ("trim_bypass", state.trim_bypass),
            ("screen_factor", state.screen_factor != 0.),
            ("depth_factor", state.depth_factor != 0.),
            ("decorrelation", state.decorrelation != 0.),
            ("dialog", state.dialog != -1),
            ("music", state.music != -1),
            (
                "head_tracking",
                state.head_tracking != HeadTracking::SceneRelative,
            ),
            (
                "binaural_mode",
                state.binaural_mode != BinauralMode::Undefined,
            ),
            (
                "importance",
                (state.importance * 10.).round() / 10. != state.importance,
            ),
        ] {
            if lost {
                loss(
                    policy,
                    Some(id),
                    Some(sample),
                    field,
                    "Property has no exact mapping in this ADM profile",
                    diagnostics,
                )?;
            }
        }
        state.importance = (state.importance * 10.).round() / 10.;
        state.screen_ref |= state.screen_factor != 0.;
        state.elevation = true;
        state.zone = Zone::All;
        state.trim_bypass = false;
        state.screen_factor = 0.;
        state.depth_factor = 0.;
        state.decorrelation = 0.;
        state.dialog = -1;
        state.music = -1;
        state.head_tracking = HeadTracking::SceneRelative;
        state.binaural_mode = BinauralMode::Undefined;
    } else {
        if state.binaural_mode == BinauralMode::Mid {
            loss(
                policy,
                Some(id),
                Some(sample),
                "binaural_mode",
                "Mid is not supported by the verified DAMF profile; using undefined",
                diagnostics,
            )?;
            state.binaural_mode = BinauralMode::Undefined;
        }
        if state.screen_ref {
            loss(
                policy,
                Some(id),
                Some(sample),
                "screen_ref",
                "ADM screenRef has no exact DAMF screenFactor mapping",
                diagnostics,
            )?;
            state.screen_ref = false;
        }
    }
    Ok(state)
}
