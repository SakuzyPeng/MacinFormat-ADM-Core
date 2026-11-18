use crate::{metadata, *};
use macin_bwf::{Container, Format, SampleFormat, TrailingChunk, Writer, WriterOptions};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

mod bed;

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
    custom_zones: std::sync::Arc<[ExclusionZone]>,
    initial_static: StaticChannelMetadata,
    position: Ramp,
    gain: Ramp,
    extent: Ramp,
    diffuse: Ramp,
    divergence: Ramp,
    cursor: u64,
    last_event: u64,
    block_number: u32,
}
impl Timeline {
    fn new(state: ObjectState, zones: &[ExclusionZone]) -> Self {
        Self {
            state,
            custom_zones: std::sync::Arc::from(zones),
            initial_static: state.into(),
            position: Ramp::fixed(state.position),
            gain: Ramp::fixed([state.gain.linear(); 3]),
            extent: Ramp::fixed(state.extent.map_or([0.; 3], Extent::xyz)),
            diffuse: Ramp::fixed([state.diffuse; 3]),
            divergence: Ramp::fixed(divergence_values(state.divergence)),
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
        if event.patch.diffuse.is_some() {
            self.diffuse = self
                .diffuse
                .update(event.sample, length, [state.diffuse; 3]);
        } else if event.patch.decorrelation.is_some() {
            // decorr 的近似按事件取值，即使同一事件含有单独的运动 ramp。
            self.diffuse = Ramp::fixed([state.diffuse; 3]);
        }
        if event.patch.divergence.is_some() {
            self.divergence =
                self.divergence
                    .update(event.sample, length, divergence_values(state.divergence));
        }
        self.state = state;
        if event.sample == 0 {
            self.initial_static = state.into();
        }
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
            let ramps = [
                self.position,
                self.gain,
                self.extent,
                self.diffuse,
                self.divergence,
            ];
            let next = ramps
                .iter()
                .map(|r| r.end)
                .filter(|v| *v > self.cursor)
                .fold(end, u64::min);
            let mut state = self.state;
            state.position = self.position.at(next);
            state.diffuse = self.diffuse.at(next)[0];
            let values = self.divergence.at(next);
            if state.divergence.is_some() || self.divergence.end > next && values != [0.; 3] {
                let target = state.divergence;
                state.divergence = Some(ObjectDivergence {
                    value: values[0],
                    azimuth_range: (!target.is_some_and(|v| v.position_range.is_some())
                        && (target.is_some_and(|v| v.azimuth_range.is_some()) || values[1] != 0.))
                        .then_some(values[1]),
                    position_range: (!target.is_some_and(|v| v.azimuth_range.is_some())
                        && (target.is_some_and(|v| v.position_range.is_some()) || values[2] != 0.))
                        .then_some(values[2]),
                });
            }
            state.gain = Gain::Linear(self.gain.at(next)[0]);
            if state.extent.is_some() {
                state.extent = Some(Extent::Cartesian(self.extent.at(next)));
            }
            self.block_number = self
                .block_number
                .checked_add(1)
                .ok_or_else(|| Error::invalid("Too many ADM blocks"))?;
            metadata::write_native_object_block(
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
                spec.approximate_mappings,
                spec.preserve_literal,
                &self.custom_zones,
            )?;
            self.cursor = next;
        }
        Ok(())
    }
}
fn divergence_values(value: Option<ObjectDivergence>) -> [f64; 3] {
    value.map_or([0.; 3], |v| {
        [
            v.value,
            v.azimuth_range.unwrap_or(0.),
            v.position_range.unwrap_or(0.),
        ]
    })
}
struct Track {
    timeline: Timeline,
    spool: BufWriter<File>,
}
struct BedTrack {
    state: BedState,
    initial_static: StaticChannelMetadata,
    last_event: u64,
    spool: BufWriter<File>,
    number: u32,
    end: Option<u64>,
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
    beds: Vec<BedTrack>,
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
        self.beds.clear();
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
        if spec
            .container_metadata
            .as_ref()
            .is_some_and(|m| !m.matches(&spec, None))
        {
            loss(
                spec.loss_policy,
                None,
                None,
                "container_metadata",
                "Ancillary metadata requires the original container family, audio, channel order, rate and programme start",
                &mut diagnostics,
            )?;
            spec.container_metadata = None;
        }
        for object in &mut spec.objects {
            if !object.custom_zones.is_empty() && !spec.profile.is_adm() {
                loss(
                    spec.loss_policy,
                    Some(object.id),
                    Some(0),
                    "zone",
                    "Custom ADM zones have no DAMF preset",
                    &mut diagnostics,
                )?;
                object.custom_zones.clear();
            }
            object.initial = mapped(
                spec.profile,
                MappingOptions {
                    loss_policy: spec.loss_policy,
                    approximate_mappings: spec.approximate_mappings,
                    preserve_literal: spec.preserve_literal,
                },
                object.id,
                0,
                object.initial,
                None,
                &mut diagnostics,
            )?;
        }
        for channel in 0..BED_CHANNELS {
            let raw_gain = spec.bed_blocks[channel]
                .as_ref()
                .map_or(spec.bed[channel].gain, |b| b.gain.unwrap_or_default());
            spec.bed[channel] = if let Some(b) = &spec.bed_blocks[channel] {
                let sample = bed::interval(&spec, b)?.0;
                bed::from_block(&spec, b, spec.bed[channel], sample, &mut diagnostics)?
            } else {
                let state = spec.bed[channel];
                mapped_bed(
                    spec.profile,
                    spec.loss_policy,
                    spec.preserve_literal,
                    channel,
                    0,
                    state,
                    &mut diagnostics,
                )?
            };
            spec.bed[channel].gain = raw_gain;
        }
        if !spec.profile.is_adm() {
            for (channel, geometry) in spec.bed_geometry.iter().enumerate() {
                if spec.bed_blocks[channel].is_none()
                    && !geometry.is_canonical_bed(metadata::bed_position(channel))?
                {
                    loss(
                        spec.loss_policy,
                        None,
                        None,
                        "bed_position",
                        &format!(
                            "DAMF cannot retain ADM speaker geometry for bed channel {}",
                            BED_LABELS[channel]
                        ),
                        &mut diagnostics,
                    )?;
                }
            }
            for (track, names) in spec
                .objects
                .iter()
                .map(|o| (Some(o.id), &o.channel_names))
                .chain(spec.bed_channel_names.iter().map(|n| (None, n)))
            {
                if !names.extensions.is_empty() {
                    loss(
                        spec.loss_policy,
                        track,
                        None,
                        "track_extensions",
                        "DAMF cannot retain XML track extension attributes",
                        &mut diagnostics,
                    )?;
                }
            }
            let mut metadata = vec![
                (None, "programme", &spec.adm),
                (None, "bed object", &spec.bed_adm),
                (None, "bed pack", &spec.bed_pack_adm),
            ];
            metadata.extend(spec.contents.iter().map(|c| (None, "content", &c.adm)));
            metadata.extend(
                spec.bed_channel_adm
                    .iter()
                    .map(|m| (None, "bed channel", m)),
            );
            for object in &spec.objects {
                metadata.extend([
                    (Some(object.id), "object", &object.adm),
                    (Some(object.id), "pack", &object.pack_adm),
                    (Some(object.id), "channel", &object.channel_adm),
                ]);
            }
            for (track, owner, metadata) in metadata {
                for field in metadata.fields() {
                    let field = if field == "importance" && owner.ends_with("pack") {
                        "pack_importance"
                    } else {
                        field
                    };
                    loss(
                        spec.loss_policy,
                        track,
                        None,
                        field,
                        &format!("Native ADM {owner} metadata has no DAMF mapping"),
                        &mut diagnostics,
                    )?;
                }
            }
        }
        // DAMF has no programme name. The name is descriptive, not rendering semantics, so a DAMF
        // master drops it without reporting a loss, deliberately.
        if spec.profile.is_adm() {
            for (field, lost) in [
                ("screen_size_ratio", spec.screen_size_ratio != 1.),
                (
                    "bed_distribution",
                    spec.profile == Profile::Standard
                        && (spec.bed_distribution || !spec.bed_distribution_present),
                ),
                (
                    "warp_mode",
                    spec.profile == Profile::Standard && spec.warp_mode != WarpMode::LoRo,
                ),
                (
                    "sc_bed_configuration",
                    spec.profile == Profile::Standard && spec.sc_bed_configuration != [3],
                ),
                (
                    "downmix_type",
                    spec.profile == Profile::Standard && spec.downmix_mode != DownmixMode::LoRo,
                ),
                (
                    "ls_rs_phase_shift",
                    spec.profile == Profile::Standard && spec.surround_phase_shift,
                ),
                (
                    "trim_mode",
                    spec.profile == Profile::Standard
                        && spec.trim_mode != [Some(TrimValues::default()); 4],
                ),
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
        } else {
            if spec.sc_bed_configuration.is_empty() {
                loss(
                    spec.loss_policy,
                    None,
                    None,
                    "sc_bed_configuration",
                    "An unspecified DBMD configuration has no exact DAMF form; using [3]",
                    &mut diagnostics,
                )?;
                spec.sc_bed_configuration.push(3);
            }
            if spec.contents.len() > 1 {
                loss(
                    spec.loss_policy,
                    None,
                    None,
                    "content_structure",
                    "DAMF cannot retain multiple ADM content groups",
                    &mut diagnostics,
                )?;
            }
            if spec.contents.iter().any(|c| {
                c.dialogue.is_some_and(|d| {
                    d != (ContentDialogue {
                        kind: ContentKind::Mixed,
                        subtype: 0,
                    })
                })
            }) {
                loss(
                    spec.loss_policy,
                    None,
                    None,
                    "dialog",
                    "ADM content dialogue classification has no exact DAMF mapping",
                    &mut diagnostics,
                )?;
            }
        }
        // Validate fixed-width fields before creating any files.
        if spec.profile == Profile::Logic {
            let channels: Vec<_> = spec
                .bed
                .iter()
                .map(|b| b.object_state().into())
                .chain(spec.objects.iter().map(|o| o.initial.into()))
                .collect();
            dbmd::build_from_spec(&spec, &channels)?;
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
            beds: Vec::new(),
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
        for (channel, mut state) in self.spec.bed.iter().copied().enumerate() {
            if !self.spec.profile.is_adm() {
                state.gain = self.spec.bed_adm.effective_gain(state.gain)?;
                state.validate()?;
            }
            let mut spool = BufWriter::new(
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(self.temp.join(format!("bed-{channel}.tmp")))?,
            );
            let first = self.spec.bed_blocks[channel].as_ref();
            let (sample, end) = first
                .map(|b| bed::interval(&self.spec, b))
                .transpose()?
                .unwrap_or((
                    self.spec.bed_timing.and_then(|t| t.start).unwrap_or(0),
                    self.spec.bed_timing.and_then(AdmTiming::end),
                ));
            if self.spec.profile.is_adm() {
                if let Some(b) = first {
                    metadata::write_adm_bed_block(
                        &mut spool,
                        b,
                        1,
                        self.spec.sample_rate,
                        self.spec.profile,
                    )?;
                }
            } else {
                let active = state.active && end.is_none_or(|end| sample < end);
                let mut initial = state;
                initial.active = active && sample == 0;
                spool.write_all(
                    metadata::damf_bed_event(channel as u32, 0, BedPatch::full(initial), Some(0))?
                        .as_bytes(),
                )?;
                if sample != 0 && active {
                    spool.write_all(
                        metadata::damf_bed_event(
                            channel as u32,
                            sample,
                            BedPatch {
                                active: Some(true),
                                ..Default::default()
                            },
                            None,
                        )?
                        .as_bytes(),
                    )?;
                }
            }
            self.beds.push(BedTrack {
                state,
                initial_static: state.into(),
                last_event: sample,
                number: u32::from(first.is_some()),
                end,
                spool,
            });
        }
        for (i, object) in self.spec.objects.iter().enumerate() {
            let mut initial = object.initial;
            if !self.spec.profile.is_adm() {
                initial.gain = object.adm.effective_gain(initial.gain)?;
                initial.validate()?;
            }
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(self.temp.join(format!("track-{i}.tmp")))?;
            let mut spool = BufWriter::new(file);
            if !self.spec.profile.is_adm() {
                spool.write_all(
                    metadata::damf_event((i + 10) as u32, 0, ObjectPatch::full(initial), Some(0))?
                        .as_bytes(),
                )?;
            }
            self.tracks.push(Track {
                timeline: Timeline::new(initial, &object.custom_zones),
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
        let prepared = self.prepare(updates)?;
        self.commit(prepared)
    }

    /// 调度规范 bed 声道的更新，不借用对象 TrackId 命名空间。
    pub fn schedule_bed_updates(&mut self, updates: &[BedUpdate]) -> Result<()> {
        self.ready()?;
        let prepared = self.prepare_beds(updates)?;
        self.commit_beds(prepared)
    }

    fn prepare_beds(&self, updates: &[BedUpdate]) -> Result<PreparedBeds> {
        let mut states: Vec<_> = self
            .beds
            .iter()
            .map(|b| (b.state, b.last_event, b.initial_static))
            .collect();
        let mut diagnostics = self.diagnostics.clone();
        let mut events = Vec::with_capacity(updates.len());
        for event in updates {
            let previous = states
                .get_mut(event.channel)
                .ok_or_else(|| Error::invalid("Bed channel must be in 0..10"))?;
            if event.sample < self.frames || event.sample < previous.1 {
                return Err(Error::invalid("Bed metadata samples must not go backwards"));
            }
            if self.spec.bed_blocks[event.channel].is_some() {
                return Err(Error::invalid(
                    "Use native bed blocks consistently on this channel",
                ));
            }
            if event.patch.has_continuous() != event.ramp_samples.is_some() {
                return Err(Error::invalid(
                    "Continuous bed properties require an explicit ramp; discrete updates must not reset it",
                ));
            }
            event
                .sample
                .checked_add(event.ramp_samples.unwrap_or(0))
                .ok_or_else(|| Error::invalid("Bed ramp end overflow"))?;
            if self.spec.profile.is_adm() && event.ramp_samples.is_some_and(|r| r != 0) {
                loss(
                    self.spec.loss_policy,
                    None,
                    Some(event.sample),
                    "bed_ramp",
                    "ADM DirectSpeakers has no independent DAMF rampLength; writing the target value",
                    &mut diagnostics,
                )?;
            }
            let mut patch = event.patch;
            if !self.spec.profile.is_adm()
                && let Some(g) = patch.gain
            {
                patch.gain = Some(self.spec.bed_adm.effective_gain(g)?);
            }
            let mut state = mapped_bed(
                self.spec.profile,
                self.spec.loss_policy,
                self.spec.preserve_literal,
                event.channel,
                event.sample,
                patch.apply(previous.0),
                &mut diagnostics,
            )?;
            if self.spec.profile.is_adm() && event.sample != 0 {
                let source = state.object_state();
                for (field, changed) in [
                    (
                        "bed_head_tracking",
                        self.spec.profile == Profile::Logic
                            && source.head_tracking != previous.2.head_tracking,
                    ),
                    (
                        "bed_binaural_mode",
                        source.binaural_mode != previous.2.binaural_mode,
                    ),
                    (
                        "bed_trim_bypass",
                        source.trim_bypass != previous.2.trim_bypass,
                    ),
                ] {
                    if changed {
                        loss(
                            self.spec.loss_policy,
                            None,
                            Some(event.sample),
                            field,
                            &format!(
                                "Bed {} has dynamic metadata this ADM profile cannot retain; keeping its initial value",
                                BED_LABELS[event.channel]
                            ),
                            &mut diagnostics,
                        )?;
                    }
                }
                if self.spec.profile == Profile::Logic {
                    state.head_tracking = previous.2.head_tracking;
                }
                state.binaural_mode = previous.2.binaural_mode;
                state.trim_bypass = previous.2.trim_bypass;
            }
            let mut mapped_event = *event;
            mapped_event.patch = patch;
            if mapped_event.patch.binaural_mode == Some(BinauralMode::Mid) {
                mapped_event.patch.binaural_mode = Some(state.binaural_mode);
            }
            previous.0 = state;
            previous.1 = event.sample;
            if event.sample == 0 {
                previous.2 = state.object_state().into();
            }
            events.push((mapped_event, state, event.patch.gain));
        }
        Ok(PreparedBeds {
            events,
            diagnostics,
        })
    }

    fn commit_beds(&mut self, prepared: PreparedBeds) -> Result<()> {
        for (event, state, raw_gain) in prepared.events {
            let track = &mut self.beds[event.channel];
            let result = if self.spec.profile.is_adm() {
                if event.sample > track.last_event {
                    let origin = self.spec.bed_timing.and_then(|t| t.start).unwrap_or(0);
                    let b = metadata::bed_state_block(
                        &self.spec,
                        event.channel,
                        track.state,
                        AdmTiming {
                            start: Some(track.last_event - origin),
                            duration: Some(event.sample - track.last_event),
                        },
                    );
                    track.number = track
                        .number
                        .checked_add(1)
                        .ok_or_else(|| Error::invalid("Too many bed blocks"))?;
                    metadata::write_adm_bed_block(
                        &mut track.spool,
                        &b,
                        track.number,
                        self.spec.sample_rate,
                        self.spec.profile,
                    )
                } else {
                    Ok(())
                }
            } else {
                metadata::damf_bed_event(
                    event.channel as u32,
                    event.sample,
                    event.patch,
                    event.ramp_samples,
                )
                .and_then(|s| track.spool.write_all(s.as_bytes()).map_err(Into::into))
            };
            if let Err(error) = result {
                self.failed = true;
                return Err(error);
            }
            track.state = state;
            track.last_event = event.sample;
            if event.sample == 0 {
                track.initial_static = state.object_state().into();
                self.spec.bed[event.channel] = BedState {
                    gain: raw_gain.unwrap_or(self.spec.bed[event.channel].gain),
                    ..state
                };
            }
        }
        self.diagnostics = prepared.diagnostics;
        Ok(())
    }
    /// Validate and map updates without touching any spool, so a rejection leaves the writer
    /// exactly as it was. `MultiWriter` prepares on every target before committing to any.
    pub fn schedule_native_updates(&mut self, updates: &[NativeObjectUpdate]) -> Result<()> {
        self.ready()?;
        let prepared = self.prepare_native(updates)?;
        self.commit(prepared)
    }
    fn prepare_native(&self, updates: &[NativeObjectUpdate]) -> Result<Prepared> {
        self.prepare_objects(updates.iter().map(|u| (&u.event, u.zones.as_deref())))
    }
    fn prepare(&self, updates: &[ObjectUpdate]) -> Result<Prepared> {
        self.prepare_objects(updates.iter().map(|u| (u, None)))
    }
    fn prepare_objects<'a>(
        &self,
        updates: impl ExactSizeIterator<Item = (&'a ObjectUpdate, Option<&'a [ExclusionZone]>)>,
    ) -> Result<Prepared> {
        let mut states: Vec<_> = self.tracks.iter().map(|t| t.timeline.clone()).collect();
        let mut diagnostics = self.diagnostics.clone();
        let mut prepared = Vec::with_capacity(updates.len());
        for (event, zones) in updates {
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
            let mut event = *event;
            if !self.spec.profile.is_adm()
                && let Some(g) = event.patch.gain
            {
                event.patch.gain = Some(self.spec.objects[index].adm.effective_gain(g)?);
            }
            let event = &event;
            let candidate = event.patch.apply(previous.state);
            candidate
                .validate()
                .map_err(|e| e.context(event.track, event.sample, "state"))?;
            let mut state = mapped(
                self.spec.profile,
                MappingOptions {
                    loss_policy: self.spec.loss_policy,
                    approximate_mappings: self.spec.approximate_mappings,
                    preserve_literal: self.spec.preserve_literal,
                },
                event.track,
                event.sample,
                candidate,
                Some(event.patch),
                &mut diagnostics,
            )?;
            if self.spec.profile == Profile::Logic && event.sample != 0 {
                for (field, changed) in [
                    (
                        "head_tracking",
                        state.head_tracking != previous.initial_static.head_tracking,
                    ),
                    (
                        "binaural_mode",
                        state.binaural_mode != previous.initial_static.binaural_mode,
                    ),
                    (
                        "trim_bypass",
                        state.trim_bypass != previous.initial_static.trim_bypass,
                    ),
                ] {
                    if changed {
                        loss(
                            self.spec.loss_policy,
                            Some(event.track),
                            Some(event.sample),
                            field,
                            "Logic DBMD cannot retain dynamic channel metadata; keeping its initial value",
                            &mut diagnostics,
                        )?;
                    }
                }
                state = previous.initial_static.apply(state);
            }
            let mut mapped_event = *event;
            if !self.spec.profile.is_adm()
                && mapped_event.patch.binaural_mode == Some(BinauralMode::Mid)
            {
                mapped_event.patch.binaural_mode = Some(state.binaural_mode);
            }
            if !self.spec.profile.is_adm() && mapped_event.patch.screen_ref.is_some() {
                if self.spec.approximate_mappings.screen_ref_factor {
                    mapped_event.patch.screen_factor = Some(state.screen_factor);
                }
                mapped_event.patch.screen_ref = None;
            }
            if self.spec.profile.is_adm() && !self.spec.approximate_mappings.diffuse_decorrelation {
                mapped_event.patch.decorrelation = None;
            }
            if !self.spec.profile.is_adm() {
                if mapped_event.patch.diffuse.is_some()
                    && self.spec.approximate_mappings.diffuse_decorrelation
                {
                    mapped_event.patch.decorrelation = Some(state.decorrelation);
                }
                mapped_event.patch.diffuse = None;
                mapped_event.patch.divergence = None;
                mapped_event.patch.channel_lock_distance = None;
                mapped_event.patch.screen_edge_lock = None;
            }
            mapped_event.ramp_samples = mapped_event
                .patch
                .has_continuous()
                .then_some(event.ramp_samples.unwrap_or(0));
            let mut next_zones = None;
            if let Some(values) = zones {
                for z in values {
                    z.validate()?;
                }
                if !values.is_empty() && !self.spec.profile.is_adm() {
                    loss(
                        self.spec.loss_policy,
                        Some(event.track),
                        Some(event.sample),
                        "zone",
                        "Custom ADM zones have no DAMF preset",
                        &mut diagnostics,
                    )?;
                } else {
                    next_zones = Some(std::sync::Arc::from(values));
                }
            }
            if let Some(z) = &next_zones {
                previous.custom_zones = std::sync::Arc::clone(z);
            }
            if !previous.custom_zones.is_empty() && (state.zone != Zone::All || !state.elevation) {
                return Err(Error::invalid(
                    "Custom zone list conflicts with a zone preset",
                ));
            }
            previous.update(mapped_event, state);
            prepared.push(PreparedObject {
                index,
                event: mapped_event,
                state,
                zones: next_zones,
            });
        }
        Ok(Prepared {
            events: prepared,
            diagnostics,
        })
    }
    /// No validation errors after this point; an I/O failure poisons the whole transaction.
    fn commit(&mut self, prepared: Prepared) -> Result<()> {
        for PreparedObject {
            index,
            event,
            state,
            zones,
        } in prepared.events
        {
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
            if let Some(zones) = zones {
                track.timeline.custom_zones = zones;
            }
            track.timeline.update(event, state);
        }
        self.diagnostics = prepared.diagnostics;
        Ok(())
    }

    pub fn write_block(&mut self, pcm: PcmBlock<'_>, updates: &[ObjectUpdate]) -> Result<()> {
        self.ready()?;
        let frames = validate_pcm(pcm, self.spec.channels())?;
        let end = self.block_end(frames, updates)?;
        let prepared = self.prepare(updates)?;
        self.commit(prepared)?;
        let result = self.write_audio(pcm, frames);
        if let Err(error) = result {
            self.failed = true;
            return Err(error);
        }
        self.frames = end;
        Ok(())
    }
    /// Timeline checks for a block of `frames` already validated PCM frames.
    fn block_end(&self, frames: usize, updates: &[ObjectUpdate]) -> Result<u64> {
        let channels = self.spec.channels();
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
        Ok(end)
    }
    fn write_audio(&mut self, pcm: PcmBlock<'_>, frames: usize) -> Result<()> {
        let audio = self
            .audio
            .as_mut()
            .ok_or_else(|| Error::new(ErrorKind::Failed, "Audio writer closed"))?;
        if let PcmBlock::S24Le(data) = pcm {
            return audio.write(data);
        }
        let mut scratch = Vec::new();
        for first in (0..frames).step_by(QUANTUM) {
            quantize(
                pcm,
                self.spec.channels(),
                first..frames.min(first + QUANTUM),
                &mut scratch,
            );
            audio.write(&scratch)?;
        }
        Ok(())
    }

    pub fn finish(self) -> Result<WriteReport> {
        self.stage()?.publish()
    }
    /// Complete every member in the staging directory; nothing is visible at the output yet.
    fn stage(mut self) -> Result<Staged> {
        self.ready()?;
        if self.frames == 0 {
            return Err(Error::invalid("Empty master"));
        }
        if self
            .tracks
            .iter()
            .any(|t| t.timeline.last_event >= self.frames)
            || self.beds.iter().any(|b| b.last_event >= self.frames)
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
        for (channel, bed) in self.beds.iter_mut().enumerate() {
            if self.spec.profile.is_adm()
                && bed.number != 0
                && self.spec.bed_blocks[channel].is_none()
            {
                let object = self.spec.bed_timing.unwrap_or_default();
                let end = self.frames.min(object.end().unwrap_or(u64::MAX));
                if end <= bed.last_event {
                    return Err(Error::invalid("Bed event lies outside its object duration"));
                }
                let b = metadata::bed_state_block(
                    &self.spec,
                    channel,
                    bed.state,
                    AdmTiming {
                        start: Some(bed.last_event - object.start.unwrap_or(0)),
                        duration: Some(end - bed.last_event),
                    },
                );
                bed.number = bed
                    .number
                    .checked_add(1)
                    .ok_or_else(|| Error::invalid("Too many bed blocks"))?;
                metadata::write_adm_bed_block(
                    &mut bed.spool,
                    &b,
                    bed.number,
                    self.spec.sample_rate,
                    self.spec.profile,
                )?;
            } else if !self.spec.profile.is_adm()
                && let Some(stop) = bed.end
                && stop < self.frames
                && stop >= bed.last_event
            {
                bed.spool.write_all(
                    metadata::damf_bed_event(
                        channel as u32,
                        stop,
                        BedPatch {
                            active: Some(false),
                            ..Default::default()
                        },
                        None,
                    )?
                    .as_bytes(),
                )?;
            }
            bed.spool.flush()?;
        }
        if self
            .spec
            .container_metadata
            .as_ref()
            .is_some_and(|m| !m.matches(&self.spec, Some(self.frames)))
        {
            loss(
                self.spec.loss_policy,
                None,
                None,
                "container_metadata",
                "Ancillary metadata cannot be retained after changing the audio duration",
                &mut self.diagnostics,
            )?;
            self.spec.container_metadata = None;
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
                metadata::write_axml_with_beds(
                    &mut axml,
                    &self.spec,
                    self.frames,
                    |i, out| {
                        if self.beds[i].number == 0 {
                            return Ok(false);
                        }
                        std::io::copy(
                            &mut BufReader::new(File::open(
                                self.temp.join(format!("bed-{i}.tmp")),
                            )?),
                            out,
                        )?;
                        Ok(true)
                    },
                    |i, out| {
                        std::io::copy(
                            &mut BufReader::new(File::open(
                                self.temp.join(format!("track-{i}.tmp")),
                            )?),
                            out,
                        )?;
                        Ok(())
                    },
                )?;
                axml.flush()?;
                axml_bytes = axml.stream_position()?;
                drop(axml);
                let dbmd = if self.spec.profile == Profile::Logic {
                    let channels: Vec<_> = self
                        .beds
                        .iter()
                        .map(|b| b.initial_static)
                        .chain(self.tracks.iter().map(|t| t.timeline.initial_static))
                        .collect();
                    dbmd::build_from_spec(&self.spec, &channels)?
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
                let metadata = self.spec.container_metadata.as_ref();
                let mut sources: Vec<_> = metadata
                    .into_iter()
                    .flat_map(|m| &m.chunks)
                    .map(|c| std::io::Cursor::new(&c.data))
                    .collect();
                for (chunk, source) in metadata
                    .into_iter()
                    .flat_map(|m| &m.chunks)
                    .zip(&mut sources)
                {
                    chunks.push(TrailingChunk {
                        id: chunk.id,
                        size: chunk.data.len() as u64,
                        source,
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
                if let Some(metadata) = &self.spec.container_metadata {
                    file.seek(SeekFrom::End(0))?;
                    for chunk in &metadata.chunks {
                        file.write_all(&chunk.id)?;
                        file.write_all(&(chunk.data.len() as i64).to_be_bytes())?;
                        file.write_all(&chunk.data)?;
                    }
                }
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
                for i in 0..BED_CHANNELS {
                    std::io::copy(
                        &mut BufReader::new(File::open(self.temp.join(format!("bed-{i}.tmp")))?),
                        &mut metadata_file,
                    )?;
                }
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
        self.beds.clear();
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
        let report = WriteReport {
            frames: self.frames,
            channels: self.spec.channels() as u16,
            sample_rate: self.spec.sample_rate,
            pcm_bytes: self.frames * self.spec.channels() as u64 * 3,
            axml_bytes,
            dbmd_bytes,
            objects,
            diagnostics: std::mem::take(&mut self.diagnostics),
        };
        Ok(Staged {
            source,
            report,
            writer: self,
        })
    }
}

/// A finished master still inside its staging directory. Dropping it discards the master.
struct Staged {
    writer: MasterWriter,
    source: PathBuf,
    report: WriteReport,
}
impl Staged {
    fn publish(self) -> Result<WriteReport> {
        renamore::rename_exclusive(&self.source, &self.writer.output)?;
        Ok(self.report)
    }
    /// Move a published output back into staging so dropping the writer removes it.
    fn unpublish(&self) -> std::io::Result<()> {
        fs::rename(&self.writer.output, &self.source)
    }
}

struct PreparedObject {
    index: usize,
    event: ObjectUpdate,
    state: ObjectState,
    zones: Option<std::sync::Arc<[ExclusionZone]>>,
}
struct Prepared {
    events: Vec<PreparedObject>,
    diagnostics: Vec<MappingDiagnostic>,
}
struct PreparedBeds {
    events: Vec<(BedUpdate, BedState, Option<Gain>)>,
    diagnostics: Vec<MappingDiagnostic>,
}

const QUANTUM: usize = 1024;

/// Signed 24-bit full-scale, nearest with ties away from zero. +1 saturates to the positive
/// endpoint; -1 uses the exact negative endpoint. No dither. `pcm` must already be validated.
pub(crate) fn quantize(
    pcm: PcmBlock<'_>,
    channels: usize,
    frames: std::ops::Range<usize>,
    out: &mut Vec<u8>,
) {
    out.clear();
    for frame in frames {
        for channel in 0..channels {
            let value = match pcm {
                PcmBlock::F32Interleaved(v) => v[frame * channels + channel],
                PcmBlock::F32Planar(v) => v[channel][frame],
                PcmBlock::S24Le(_) => unreachable!("S24LE is written as is"),
            };
            let value = (f64::from(value) * 8_388_608.)
                .round()
                .clamp(-8_388_608., 8_388_607.) as i32;
            out.extend_from_slice(&value.to_le_bytes()[..3]);
        }
    }
}

/// Where one `MultiWriter` output goes and which profile it takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    pub profile: Profile,
}

/// Writes one model to several outputs in a single pass: ADM and DAMF side by side, for example.
///
/// Every target gets the same spec except for its profile, so the PCM is quantised once and the
/// outputs carry identical audio. Updates are validated and mapped on every target before any
/// target commits them: a rejection (an ADM loss under `LossPolicy::Reject`, say) leaves all
/// targets unchanged, as it would a single writer. `finish` publishes all outputs or, if one
/// publish fails, moves the already published ones back and returns the error. That rollback
/// does not survive a crash between renames: a process killed there can leave part of the set.
#[derive(Debug)]
pub struct MultiWriter {
    writers: Vec<MasterWriter>,
    scratch: Vec<u8>,
}
impl MultiWriter {
    pub fn create(spec: &MasterSpec, targets: &[Target]) -> Result<Self> {
        if targets.is_empty() {
            return Err(Error::invalid("MultiWriter needs at least one target"));
        }
        for (i, target) in targets.iter().enumerate() {
            if targets[..i].iter().any(|t| t.path == target.path) {
                return Err(Error::invalid("MultiWriter targets must not share a path"));
            }
        }
        let writers = targets
            .iter()
            .map(|target| {
                MasterWriter::create(
                    &target.path,
                    MasterSpec {
                        profile: target.profile,
                        ..spec.clone()
                    },
                )
                .map_err(|mut e| {
                    e.message = format!("{}: {}", target.path.display(), e.message);
                    e
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            writers,
            scratch: Vec::new(),
        })
    }
    pub fn frames_written(&self) -> u64 {
        self.writers[0].frames
    }
    /// The shared spec, with the first target's profile.
    pub fn spec(&self) -> &MasterSpec {
        &self.writers[0].spec
    }
    fn ready(&self) -> Result<()> {
        self.writers.iter().try_for_each(MasterWriter::ready)
    }
    fn poison(&mut self) {
        for writer in &mut self.writers {
            writer.failed = true;
        }
    }
    fn prepare(&self, updates: &[ObjectUpdate]) -> Result<Vec<Prepared>> {
        self.writers.iter().map(|w| w.prepare(updates)).collect()
    }
    fn commit(&mut self, prepared: Vec<Prepared>) -> Result<()> {
        for (writer, prepared) in self.writers.iter_mut().zip(prepared) {
            if let Err(error) = writer.commit(prepared) {
                self.poison();
                return Err(error);
            }
        }
        Ok(())
    }
    pub fn schedule_updates(&mut self, updates: &[ObjectUpdate]) -> Result<()> {
        self.ready()?;
        let prepared = self.prepare(updates)?;
        self.commit(prepared)
    }
    pub fn schedule_native_updates(&mut self, updates: &[NativeObjectUpdate]) -> Result<()> {
        self.ready()?;
        let prepared = self
            .writers
            .iter()
            .map(|w| w.prepare_native(updates))
            .collect::<Result<_>>()?;
        self.commit(prepared)
    }
    pub fn schedule_bed_updates(&mut self, updates: &[BedUpdate]) -> Result<()> {
        self.ready()?;
        let prepared = self
            .writers
            .iter()
            .map(|w| w.prepare_beds(updates))
            .collect::<Result<Vec<_>>>()?;
        for (writer, events) in self.writers.iter_mut().zip(prepared) {
            if let Err(error) = writer.commit_beds(events) {
                self.poison();
                return Err(error);
            }
        }
        Ok(())
    }
    pub fn write_block(&mut self, pcm: PcmBlock<'_>, updates: &[ObjectUpdate]) -> Result<()> {
        self.ready()?;
        let channels = self.writers[0].spec.channels();
        let frames = validate_pcm(pcm, channels)?;
        let ends = self
            .writers
            .iter()
            .map(|w| w.block_end(frames, updates))
            .collect::<Result<Vec<_>>>()?;
        let prepared = self.prepare(updates)?;
        self.commit(prepared)?;
        if let Err(error) = self.write_audio(pcm, frames, channels) {
            self.poison();
            return Err(error);
        }
        for (writer, end) in self.writers.iter_mut().zip(ends) {
            writer.frames = end;
        }
        Ok(())
    }
    fn write_audio(&mut self, pcm: PcmBlock<'_>, frames: usize, channels: usize) -> Result<()> {
        let write = |bytes: &[u8], writers: &mut [MasterWriter]| -> Result<()> {
            for writer in writers {
                writer
                    .audio
                    .as_mut()
                    .ok_or_else(|| Error::new(ErrorKind::Failed, "Audio writer closed"))?
                    .write(bytes)?;
            }
            Ok(())
        };
        if let PcmBlock::S24Le(data) = pcm {
            return write(data, &mut self.writers);
        }
        for first in (0..frames).step_by(QUANTUM) {
            quantize(
                pcm,
                channels,
                first..frames.min(first + QUANTUM),
                &mut self.scratch,
            );
            write(&self.scratch, &mut self.writers)?;
        }
        Ok(())
    }
    /// Reports in target order. On error no output of this writer remains published.
    pub fn finish(self) -> Result<Vec<WriteReport>> {
        let staged = self
            .writers
            .into_iter()
            .map(MasterWriter::stage)
            .collect::<Result<Vec<_>>>()?;
        let mut published = 0;
        let mut failure = None;
        for (i, item) in staged.iter().enumerate() {
            if let Err(error) = renamore::rename_exclusive(&item.source, &item.writer.output) {
                failure = Some((i, error));
                break;
            }
            published += 1;
        }
        let Some((index, error)) = failure else {
            return Ok(staged.into_iter().map(|s| s.report).collect());
        };
        let mut error = Error::from(error);
        for item in &staged[..published] {
            if let Err(rollback) = item.unpublish() {
                error.message = format!(
                    "{}; rollback of {} also failed: {rollback}",
                    error.message,
                    item.writer.output.display()
                );
            }
        }
        error.message = format!(
            "Publishing {} failed: {}",
            staged[index].writer.output.display(),
            error.message
        );
        Err(error)
    }
}

pub(crate) fn validate_pcm(pcm: PcmBlock<'_>, channels: usize) -> Result<usize> {
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
    message: &str,
    diagnostics: &mut Vec<MappingDiagnostic>,
) -> Result<()> {
    if policy == LossPolicy::Reject {
        let mut error = Error::new(ErrorKind::Unsupported, message);
        error.track = track;
        error.sample = sample;
        error.field = Some(field);
        return Err(error);
    }
    if !diagnostics.iter().any(|d| {
        d.kind == MappingDiagnosticKind::Loss
            && d.track == track
            && d.field == field
            && d.message == message
    }) {
        diagnostics.push(MappingDiagnostic {
            kind: MappingDiagnosticKind::Loss,
            track,
            sample,
            field,
            message: message.into(),
        });
    }
    Ok(())
}
fn literal(
    diagnostics: &mut Vec<MappingDiagnostic>,
    track: Option<TrackId>,
    sample: u64,
    field: &'static str,
    message: &str,
) {
    if !diagnostics.iter().any(|d| {
        d.kind == MappingDiagnosticKind::PreservedLiteral
            && d.track == track
            && d.field == field
            && d.message == message
    }) {
        diagnostics.push(MappingDiagnostic {
            kind: MappingDiagnosticKind::PreservedLiteral,
            track,
            sample: Some(sample),
            field,
            message: message.into(),
        });
    }
}
fn mapped(
    profile: Profile,
    options: MappingOptions,
    id: TrackId,
    sample: u64,
    mut state: ObjectState,
    patch: Option<ObjectPatch>,
    diagnostics: &mut Vec<MappingDiagnostic>,
) -> Result<ObjectState> {
    state.validate()?;
    let policy = options.loss_policy;
    let approximations = options.approximate_mappings;
    if !profile.is_adm()
        && options.preserve_literal.damf_mid
        && state.binaural_mode == BinauralMode::Mid
    {
        literal(
            diagnostics,
            Some(id),
            sample,
            "binaural_mode",
            "Writing literal DAMF mid; Dolby Conversion Tool 2.1.2 rejects this value",
        );
    } else if profile != Profile::Standard && state.binaural_mode == BinauralMode::Mid {
        loss(
            policy,
            Some(id),
            Some(sample),
            "binaural_mode",
            "Mid has no verified encoding; using undefined",
            diagnostics,
        )?;
        state.binaural_mode = BinauralMode::Undefined;
    }
    if profile.is_adm() {
        if let Some(divergence) = &mut state.divergence
            && divergence.azimuth_range.is_some()
        {
            if options.preserve_literal.cartesian_azimuth_range {
                literal(
                    diagnostics,
                    Some(id),
                    sample,
                    "divergence_azimuth_range",
                    "Writing legacy azimuthRange verbatim on a Cartesian block; BS.2076-2 specifies positionRange here",
                );
            } else {
                divergence.azimuth_range = None;
                loss(
                    policy,
                    Some(id),
                    Some(sample),
                    "divergence_azimuth_range",
                    "azimuthRange has no Cartesian ADM mapping; keeping the divergence value",
                    diagnostics,
                )?;
            }
        }
        if approximations.diffuse_decorrelation
            && let Some(value) = patch
                .and_then(|p| p.decorrelation)
                .or((state.decorrelation != 0.).then_some(state.decorrelation))
        {
            let native = patch.map_or((state.diffuse != 0.).then_some(state.diffuse), |p| {
                p.diffuse
            });
            if native.is_some_and(|v| v != value) {
                loss(
                    policy,
                    Some(id),
                    Some(sample),
                    "decorrelation",
                    "Approximate decorr conflicts with explicit diffuse; keeping diffuse",
                    diagnostics,
                )?;
            } else {
                state.diffuse = value;
                record_approximation(
                    diagnostics,
                    id,
                    sample,
                    "decorrelation",
                    format!(
                        "Approximate decorr {value} -> diffuse {value}: numeric copy as discrete updates; rendering equivalence is not guaranteed"
                    ),
                );
            }
            state.decorrelation = 0.;
        }
        if approximations.screen_ref_factor
            && let Some(value) = patch
                .and_then(|p| p.screen_factor)
                .or((state.screen_factor != 0.).then_some(state.screen_factor))
        {
            let screen_ref = value != 0.;
            if patch
                .and_then(|p| p.screen_ref)
                .is_some_and(|native| native != screen_ref)
            {
                loss(
                    policy,
                    Some(id),
                    Some(sample),
                    "screen_factor",
                    "Approximate screenFactor conflicts with explicit screenRef; keeping screenRef",
                    diagnostics,
                )?;
            } else {
                state.screen_ref = screen_ref;
                record_approximation(
                    diagnostics,
                    id,
                    sample,
                    "screen_factor",
                    format!(
                        "Approximate screenFactor {value} -> screenRef {screen_ref}: nonzero becomes true; continuous factor is not retained"
                    ),
                );
            }
            state.screen_factor = 0.;
        }
        for (field, lost) in [
            (
                "trim_bypass",
                profile == Profile::Standard && state.trim_bypass,
            ),
            ("screen_factor", state.screen_factor != 0.),
            ("depth_factor", state.depth_factor != 0.),
            (
                "decorrelation",
                state.decorrelation != 0. && !approximations.diffuse_decorrelation,
            ),
            ("dialog", state.dialog != -1),
            ("music", state.music != -1),
            (
                "head_tracking",
                profile == Profile::Standard && state.head_tracking == HeadTracking::Undefined,
            ),
            (
                "binaural_mode",
                profile == Profile::Standard && state.binaural_mode != BinauralMode::Undefined,
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
        state.screen_factor = 0.;
        state.depth_factor = 0.;
        state.decorrelation = 0.;
        state.dialog = -1;
        state.music = -1;
        if profile == Profile::Standard {
            state.trim_bypass = false;
            if state.head_tracking == HeadTracking::Undefined {
                state.head_tracking = HeadTracking::SceneRelative;
            }
            state.binaural_mode = BinauralMode::Undefined;
        }
    } else {
        if let Some(value) = patch
            .and_then(|p| p.diffuse)
            .or((state.diffuse != 0.).then_some(state.diffuse))
        {
            if approximations.diffuse_decorrelation {
                let native = patch.map_or(
                    (state.decorrelation != 0.).then_some(state.decorrelation),
                    |p| p.decorrelation,
                );
                if native.is_some_and(|v| v != value) {
                    loss(
                        policy,
                        Some(id),
                        Some(sample),
                        "diffuse",
                        "Approximate diffuse conflicts with explicit decorr; keeping decorr",
                        diagnostics,
                    )?;
                } else {
                    state.decorrelation = value;
                    record_approximation(
                        diagnostics,
                        id,
                        sample,
                        "diffuse",
                        format!(
                            "Approximate diffuse {value} -> decorr {value}: numeric copy as discrete updates; rendering equivalence is not guaranteed"
                        ),
                    );
                }
            } else if value != 0. {
                loss(
                    policy,
                    Some(id),
                    Some(sample),
                    "diffuse",
                    "Native diffuse has no DAMF mapping",
                    diagnostics,
                )?;
            }
        }
        for (field, lost) in [
            ("divergence", state.divergence.is_some()),
            (
                "channel_lock_distance",
                state.channel_lock_distance.is_some(),
            ),
            (
                "screen_edge_lock",
                state.screen_edge_lock.iter().any(Option::is_some),
            ),
        ] {
            if lost {
                loss(
                    policy,
                    Some(id),
                    Some(sample),
                    field,
                    "Native ADM block property has no DAMF mapping",
                    diagnostics,
                )?;
            }
        }
        if state.headphone_virtualise.is_some() {
            loss(
                policy,
                Some(id),
                Some(sample),
                "headphone_virtualise",
                "ADM headphoneVirtualise has no DAMF mapping",
                diagnostics,
            )?;
            state.headphone_virtualise = None;
        }
        state.diffuse = 0.;
        state.divergence = None;
        state.channel_lock_distance = None;
        state.screen_edge_lock = [None; 3];
        if let Some(value) = patch
            .and_then(|p| p.screen_ref)
            .or(state.screen_ref.then_some(true))
        {
            if approximations.screen_ref_factor {
                let factor = if value { 1. } else { 0. };
                let native = match patch {
                    Some(p) => p.screen_factor,
                    None => (state.screen_factor != 0.).then_some(state.screen_factor),
                };
                if native.is_some_and(|native| native != factor) {
                    loss(
                        policy,
                        Some(id),
                        Some(sample),
                        "screen_ref",
                        "Approximate screenRef conflicts with explicit screenFactor; keeping screenFactor",
                        diagnostics,
                    )?;
                } else {
                    state.screen_factor = factor;
                    record_approximation(
                        diagnostics,
                        id,
                        sample,
                        "screen_ref",
                        format!(
                            "Approximate screenRef {value} -> screenFactor {factor}: boolean becomes 0 or 1; rendering equivalence is not guaranteed"
                        ),
                    );
                }
            } else if value {
                loss(
                    policy,
                    Some(id),
                    Some(sample),
                    "screen_ref",
                    "ADM screenRef has no exact DAMF screenFactor mapping",
                    diagnostics,
                )?;
            }
            state.screen_ref = false;
        }
    }
    Ok(state)
}

fn mapped_bed(
    profile: Profile,
    policy: LossPolicy,
    preserve: LiteralPreservation,
    channel: usize,
    sample: u64,
    mut state: BedState,
    diagnostics: &mut Vec<MappingDiagnostic>,
) -> Result<BedState> {
    state.validate()?;
    let default = BedState::for_channel(channel);
    if !profile.is_adm() && preserve.damf_mid && state.binaural_mode == BinauralMode::Mid {
        literal(
            diagnostics,
            None,
            sample,
            "bed_binaural_mode",
            &format!(
                "Writing literal mid on bed {}; Dolby Conversion Tool 2.1.2 rejects mid",
                BED_LABELS[channel]
            ),
        );
    } else if state.binaural_mode == BinauralMode::Mid {
        loss(
            policy,
            None,
            Some(sample),
            "bed_binaural_mode",
            &format!(
                "Bed {} Mid mode has no verified encoding; using the default",
                BED_LABELS[channel]
            ),
            diagnostics,
        )?;
        state.binaural_mode = default.binaural_mode;
    }
    if profile.is_adm() {
        for (field, changed) in [
            ("bed_active", !state.active),
            (
                "bed_importance",
                (state.importance * 10.).round() / 10. != state.importance,
            ),
            ("bed_position", state.rendering.position.is_some()),
            ("bed_extent", state.rendering.extent.flatten().is_some()),
            (
                "bed_zone",
                state.rendering.zone.is_some_and(|v| v != Zone::All),
            ),
            ("bed_snap", state.rendering.snap == Some(true)),
            ("bed_elevation", state.rendering.elevation == Some(false)),
            (
                "bed_decorrelation",
                state.rendering.decorrelation.is_some_and(|v| v != 0.),
            ),
            (
                "bed_dialog",
                state.rendering.dialog.is_some_and(|v| v != -1),
            ),
            ("bed_music", state.rendering.music.is_some_and(|v| v != -1)),
            (
                "bed_screen_factor",
                state.rendering.screen_factor.is_some_and(|v| v != 0.),
            ),
            (
                "bed_depth_factor",
                state.rendering.depth_factor.is_some_and(|v| v != 0.),
            ),
            (
                "bed_trim_bypass",
                profile == Profile::Standard && state.trim_bypass,
            ),
            (
                "bed_binaural_mode",
                profile == Profile::Standard && state.binaural_mode != default.binaural_mode,
            ),
            (
                "bed_head_tracking",
                profile == Profile::Standard && state.head_tracking == HeadTracking::Undefined,
            ),
        ] {
            if changed {
                loss(
                    policy,
                    None,
                    Some(sample),
                    field,
                    &format!(
                        "Bed {} property has no mapping in this ADM profile",
                        BED_LABELS[channel]
                    ),
                    diagnostics,
                )?;
            }
        }
        state.rendering = BedRendering::default();
        state.importance = (state.importance * 10.).round() / 10.;
        if profile == Profile::Standard {
            state.trim_bypass = false;
            state.binaural_mode = default.binaural_mode;
            if state.head_tracking == HeadTracking::Undefined {
                state.head_tracking = HeadTracking::SceneRelative;
            }
        }
    }
    Ok(state)
}
