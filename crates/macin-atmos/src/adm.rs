//! ADM BWF reading into the master model (feature `read`).
//!
//! [`AdmReader`] reads the Atmos subset this crate writes, and the same shape from other tools,
//! into the [`MasterSpec`] and [`ObjectUpdate`]s that `MasterWriter` takes, so ADM converts to
//! DAMF (or to the other ADM profile) through ordinary writing. The subset is one programme, at
//! most one 7.1.2 `DirectSpeakers` bed with `RC_` labels, and single-track Cartesian `Objects`.
//! What lies outside it is refused, and what the model cannot hold inside it is a loss under the
//! [`LossPolicy`], exactly as for [`DamfReader`](crate::damf::DamfReader).
//!
//! Numbers are parsed from the document text as `f64`, not taken from the `f32` projection, so a
//! master written by this crate reads back to the values it was written from.
//!
//! 母版读取逐块扫描 AXML，只保留引用结构和声道范围；有序时间线不累计全部 block 或更新。
//! 乱序声道保留紧凑偏移索引，以维持稳定的时间排序。更新迭代期间源文件须保持可读且不变。
use crate::damf::{AudioSource, Conversion, Input, write_conversion};
use crate::*;
use macin_adm::records::{NONE, Span};
use macin_adm::{Document, Snapshot, TrackUidDefinition, parse_boolean};
use macin_bwf::{Chna, Container, Reader};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

mod stream;
use stream::{Channel, Index, ProjectedBlocks};

/// 接受的最大 `axml` chunk；按流扫描，不整体载入内存。
pub const AXML_LIMIT: u64 = 1 << 30;
const CHNA_LIMIT: u64 = 4 + 40 * u16::MAX as u64;

/// An ADM BWF master opened for reading. See the module documentation.
#[derive(Debug, Clone)]
pub struct AdmReader {
    spec: MasterSpec,
    path: PathBuf,
    /// The source channel (0-based) feeding each output channel: ten bed slots, then objects.
    channels: Vec<Option<u32>>,
    updates: u64,
    timelines: Vec<ObjectTimeline>,
    last_change: Option<u64>,
    /// 有限结束边界独立于原音频长度保存，用于转换时提交停用事件。
    object_ends: Vec<(TrackId, u64)>,
    bed_end: Option<u64>,
    last_sample: Option<u64>,
    frames: u64,
    diagnostics: Vec<MappingDiagnostic>,
}

fn located(path: &Path, mut error: Error) -> Error {
    if error.track.is_none() {
        error.message = format!("{}: {}", path.display(), error.message);
    }
    error
}
fn unsupported(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::Unsupported, message)
}
fn adm_error(error: macin_adm::Error) -> Error {
    Error::new(
        if error.code == 2 {
            ErrorKind::Unsupported
        } else {
            ErrorKind::InvalidInput
        },
        error.message,
    )
}

struct Losses<'a> {
    policy: LossPolicy,
    diagnostics: &'a mut Vec<MappingDiagnostic>,
}
impl Losses<'_> {
    fn report(
        &mut self,
        track: Option<TrackId>,
        sample: Option<u64>,
        field: &'static str,
        message: String,
    ) -> Result<()> {
        if self.policy == LossPolicy::Reject {
            let mut error = Error::new(ErrorKind::Unsupported, message);
            error.track = track;
            error.sample = sample;
            error.field = Some(field);
            return Err(error);
        }
        if !self
            .diagnostics
            .iter()
            .any(|d| d.track == track && d.field == field && d.message == message)
        {
            self.diagnostics.push(MappingDiagnostic {
                track,
                sample,
                field,
                message,
            });
        }
        Ok(())
    }
}

/// Child elements of an object block the model reads, or checks for their default.
const BLOCK_ELEMENTS: [&str; 13] = [
    "position",
    "width",
    "height",
    "depth",
    "cartesian",
    "gain",
    "jumpPosition",
    "channelLock",
    "screenRef",
    "importance",
    "diffuse",
    "objectDivergence",
    "headLocked",
];
const OBJECT_ELEMENTS: [&str; 7] = [
    "audioPackFormatIDRef",
    "audioTrackUIDRef",
    "audioObjectIDRef",
    "gain",
    "mute",
    "positionOffset",
    "headLocked",
];

fn number(text: &str, what: &str) -> Result<f64> {
    let value: f64 = text
        .trim()
        .parse()
        .map_err(|_| Error::invalid(format!("{what} must be a number")))?;
    if !value.is_finite() {
        return Err(Error::invalid(format!("{what} must be finite")));
    }
    Ok(value)
}

/// The state one object block describes, parsed from its literal text.
fn block_state(
    doc: &Document,
    node: u32,
    base: ObjectState,
    gain_factor: Option<f64>,
    losses: &mut Losses<'_>,
    track: TrackId,
    sample: u64,
) -> Result<ObjectState> {
    // Each block is complete on its own: an element it leaves out takes its default, not the
    // previous block's value. Only what ADM does not spell per block carries over from `base`.
    let mut state = ObjectState {
        snap: false,
        screen_ref: false,
        ..base
    };
    let mut position = [None; 3];
    let mut size = [0.; 3];
    let mut gain = Gain::Linear(1.);
    let mut importance = 1.;
    let at = |e: Error| e.context(track, sample, "block");
    for child in doc.element_children(node) {
        let name = doc.element_name(child).unwrap_or_default();
        let text = doc.element_text(child).unwrap_or_default();
        match name {
            "position" => {
                let axis = match doc.element_attribute(child, "coordinate") {
                    Some("X") => 0,
                    Some("Y") => 1,
                    Some("Z") => 2,
                    _ => {
                        return Err(at(unsupported(
                            "polar object positions are not supported; the Atmos subset is Cartesian",
                        )));
                    }
                };
                position[axis] = Some(number(text, "position").map_err(at)?);
            }
            "width" => size[0] = number(text, "width").map_err(at)?,
            "depth" => size[1] = number(text, "depth").map_err(at)?,
            "height" => size[2] = number(text, "height").map_err(at)?,
            "cartesian" if parse_boolean(text).map_err(adm_error).map_err(at)? == 0 => {
                return Err(at(unsupported("object blocks must be Cartesian")));
            }
            "gain" => {
                let value = number(text, "gain").map_err(at)?;
                gain = match doc.element_attribute(child, "gainUnit") {
                    None | Some("linear") => Gain::Linear(value),
                    Some("dB") => Gain::Decibels(value),
                    Some(other) => {
                        return Err(at(Error::invalid(format!("unknown gainUnit {other:?}"))));
                    }
                };
            }
            "channelLock" => {
                state.snap = parse_boolean(text).map_err(adm_error).map_err(at)? != 0;
                if doc.element_attribute(child, "maxDistance").is_some() {
                    losses.report(
                        Some(track),
                        Some(sample),
                        "channel_lock_distance",
                        "channelLock maxDistance has no mapping".into(),
                    )?;
                }
            }
            "screenRef" => {
                state.screen_ref = parse_boolean(text).map_err(adm_error).map_err(at)? != 0;
            }
            "importance" => {
                let value: u8 = text
                    .trim()
                    .parse()
                    .ok()
                    .filter(|v| *v <= 10)
                    .ok_or_else(|| at(Error::invalid("importance must be 0..=10")))?;
                importance = f64::from(value) / 10.;
            }
            "diffuse" if number(text, "diffuse").map_err(at)? != 0. => {
                losses.report(
                    Some(track),
                    Some(sample),
                    "diffuse",
                    "diffuse has no mapping".into(),
                )?;
            }
            "objectDivergence" if number(text, "objectDivergence").map_err(at)? != 0. => {
                losses.report(
                    Some(track),
                    Some(sample),
                    "divergence",
                    "objectDivergence has no mapping".into(),
                )?;
            }
            "headLocked" if parse_boolean(text).map_err(adm_error).map_err(at)? != 0 => {
                losses.report(
                    Some(track),
                    Some(sample),
                    "head_tracking",
                    "headLocked has no verified mapping".into(),
                )?;
            }
            _ if BLOCK_ELEMENTS.contains(&name) => {}
            _ => losses.report(
                Some(track),
                Some(sample),
                "unknown_field",
                format!("Block element {name:?} has no mapping"),
            )?,
        }
    }
    let [Some(x), Some(y), z] = position else {
        return Err(at(Error::invalid("object block needs X and Y positions")));
    };
    state.position = [x, y, z.unwrap_or(0.)];
    state.extent = match size {
        [0., 0., 0.] => None,
        [w, d, h] if w == d && d == h => Some(Extent::Uniform(w)),
        v => Some(Extent::Cartesian(v)),
    };
    state.gain = match gain_factor {
        None => Gain::Silence,
        Some(1.) => gain,
        Some(f) => Gain::Linear(gain.linear() * f),
    };
    state.importance = importance;
    state.validate().map_err(at)?;
    Ok(state)
}

/// The fields of `next` that differ from `current`, as a patch.
fn difference(current: &ObjectState, next: &ObjectState) -> ObjectPatch {
    let changed = |a: bool| a.then_some(());
    ObjectPatch {
        active: changed(current.active != next.active).map(|()| next.active),
        position: changed(current.position != next.position).map(|()| next.position),
        gain: changed(current.gain != next.gain).map(|()| next.gain),
        extent: changed(current.extent != next.extent).map(|()| next.extent),
        importance: changed(current.importance != next.importance).map(|()| next.importance),
        snap: changed(current.snap != next.snap).map(|()| next.snap),
        screen_ref: changed(current.screen_ref != next.screen_ref).map(|()| next.screen_ref),
        ..ObjectPatch::default()
    }
}

fn channel_end(channels: &[Channel], start: u64) -> u64 {
    channels
        .iter()
        .enumerate()
        .max_by_key(|(i, c)| (c.last.0, *i))
        .map_or(u64::MAX, |(_, c)| start.saturating_add(c.last.1))
}

#[derive(Debug, Clone)]
struct ObjectTimeline {
    id: TrackId,
    channels: Vec<Channel>,
    start: u64,
    locked: u32,
    factor: Option<f64>,
    end: u64,
}

struct ObjectEvents {
    blocks: ProjectedBlocks,
    timeline: ObjectTimeline,
    state: ObjectState,
    initial: ObjectState,
    previous_end: u64,
    activation: Option<ObjectUpdate>,
    source_frames: u64,
    frames: u64,
    done: bool,
}

impl ObjectEvents {
    fn new(
        path: &Path,
        timeline: ObjectTimeline,
        rate: u32,
        source_frames: u64,
        frames: u64,
        losses: &mut Losses<'_>,
    ) -> Result<Self> {
        let mut blocks = ProjectedBlocks::new(
            &timeline.channels,
            path,
            rate,
            timeline.start,
            timeline.locked,
        )?;
        let first = blocks
            .next()?
            .ok_or_else(|| Error::invalid("对象声道没有 block"))?;
        let block = first
            .scene
            .object_blocks
            .first()
            .ok_or_else(|| unsupported("对象需要 Objects block"))?;
        let state = block_state(
            &first.doc,
            block.node,
            ObjectState::default(),
            timeline.factor,
            losses,
            timeline.id,
            block.start,
        )?;
        let begin = block.start.max(timeline.start);
        let mut initial = state;
        initial.active = begin == 0;
        let activation =
            (begin > 0 && begin < source_frames.min(timeline.end)).then_some(ObjectUpdate {
                track: timeline.id,
                sample: begin,
                ramp_samples: None,
                patch: ObjectPatch {
                    active: Some(true),
                    ..ObjectPatch::default()
                },
            });
        Ok(Self {
            blocks,
            timeline,
            state,
            initial,
            previous_end: block.end,
            activation,
            source_frames,
            frames,
            done: false,
        })
    }

    fn next_event(&mut self, losses: &mut Losses<'_>) -> Result<Option<ObjectUpdate>> {
        if let Some(update) = self.activation.take() {
            return Ok(Some(update));
        }
        if self.done {
            return Ok(None);
        }
        while let Some(item) = self.blocks.next()? {
            let block = &item.scene.object_blocks[0];
            if self.previous_end != block.start {
                return Err(
                    unsupported("the blocks do not form a contiguous timeline").context(
                        self.timeline.id,
                        block.start,
                        "block",
                    ),
                );
            }
            self.previous_end = block.end;
            if block.start >= self.source_frames.min(self.timeline.end) {
                continue;
            }
            let next = block_state(
                &item.doc,
                block.node,
                self.state,
                self.timeline.factor,
                losses,
                self.timeline.id,
                block.start,
            )?;
            let patch = difference(&self.state, &next);
            self.state = next;
            if patch == ObjectPatch::default() {
                continue;
            }
            let duration = block.end.saturating_sub(block.start);
            let ramp = if block.jump == 0 {
                if block.end == u64::MAX { 0 } else { duration }
            } else {
                block.interpolation.min(duration)
            };
            return Ok(Some(ObjectUpdate {
                track: self.timeline.id,
                sample: block.start,
                ramp_samples: patch.has_continuous().then_some(ramp),
                patch,
            }));
        }
        self.done = true;
        Ok((self.timeline.end < self.frames).then_some(ObjectUpdate {
            track: self.timeline.id,
            sample: self.timeline.end,
            ramp_samples: None,
            patch: ObjectPatch {
                active: Some(false),
                ..ObjectPatch::default()
            },
        }))
    }
}

/// 逐对象、逐 block 读取的 ADM 更新流；遇到错误后终止。
pub struct Updates {
    path: PathBuf,
    timelines: std::vec::IntoIter<ObjectTimeline>,
    current: Option<ObjectEvents>,
    rate: u32,
    source_frames: u64,
    frames: u64,
    policy: LossPolicy,
    failed: bool,
}

impl std::fmt::Debug for Updates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Updates")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Updates {
    fn advance(&mut self) -> Result<()> {
        let mut diagnostics = Vec::new();
        let mut losses = Losses {
            policy: self.policy,
            diagnostics: &mut diagnostics,
        };
        self.current = self
            .timelines
            .next()
            .map(|timeline| {
                ObjectEvents::new(
                    &self.path,
                    timeline,
                    self.rate,
                    self.source_frames,
                    self.frames,
                    &mut losses,
                )
            })
            .transpose()
            .map_err(|e| located(&self.path, e))?;
        Ok(())
    }
    fn next_update(&mut self) -> Result<Option<ObjectUpdate>> {
        let mut diagnostics = Vec::new();
        let mut losses = Losses {
            policy: self.policy,
            diagnostics: &mut diagnostics,
        };
        while let Some(events) = &mut self.current {
            if let Some(update) = events.next_event(&mut losses)? {
                return Ok(Some(update));
            }
            self.advance()?;
        }
        Ok(None)
    }
}

impl Iterator for Updates {
    type Item = Result<ObjectUpdate>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        match self.next_update() {
            Ok(Some(update)) => Some(Ok(update)),
            Ok(None) => None,
            Err(error) => {
                self.failed = true;
                self.current = None;
                Some(Err(located(&self.path, error)))
            }
        }
    }
}

impl std::iter::FusedIterator for Updates {}

struct Scene<'a> {
    doc: &'a Document,
    scene: &'a Snapshot,
    index: &'a Index,
}
impl Scene<'_> {
    fn channels(&self, uid: u32, rate: u32) -> Result<Vec<Channel>> {
        self.doc
            .track_channels(self.text(uid))
            .into_iter()
            .filter(|&node| {
                self.doc
                    .element_children(node)
                    .into_iter()
                    .any(|child| self.doc.element_name(child) == Some("audioBlockFormat"))
            })
            .map(|node| {
                let id = self
                    .doc
                    .element_attribute(node, "audioChannelFormatID")
                    .ok_or_else(|| Error::invalid("声道缺少 ID"))?;
                self.index
                    .channels
                    .get(&id.to_ascii_uppercase())
                    .cloned()
                    .map_or_else(
                        || {
                            Channel::inline(
                                self.doc
                                    .element_xml(node)
                                    .ok_or_else(|| Error::invalid("声道缺少原文"))?,
                                rate,
                            )
                        },
                        Ok,
                    )
            })
            .collect()
    }
    fn text(&self, index: u32) -> &str {
        if index == NONE {
            ""
        } else {
            &self.scene.strings[index as usize]
        }
    }
    fn list(&self, span: Span) -> impl Iterator<Item = &str> {
        self.scene.indices[span.offset..span.offset + span.len]
            .iter()
            .map(|&i| self.text(i))
    }
    fn channel_losses(
        &self,
        uid: u32,
        track: Option<TrackId>,
        losses: &mut Losses<'_>,
    ) -> Result<()> {
        for channel in self.doc.track_channels(self.text(uid)) {
            for child in self.doc.element_children(channel) {
                let name = self.doc.element_name(child).unwrap_or_default();
                if name != "audioBlockFormat" {
                    let field = match name {
                        "frequency" | "channelFrequency" => "channel_frequency",
                        _ => "unknown_field",
                    };
                    losses.report(
                        track,
                        None,
                        field,
                        format!("Channel {} element {name:?} has no mapping", self.text(uid)),
                    )?;
                }
            }
        }
        Ok(())
    }
}

impl AdmReader {
    /// Read and validate an ADM BWF master. `policy` governs both what reading may drop and,
    /// through the spec, what writing may drop.
    pub fn open(path: impl AsRef<Path>, policy: LossPolicy) -> Result<Self> {
        let path = path.as_ref();
        Self::read(path, policy).map_err(|e| located(path, e))
    }

    fn read(path: &Path, policy: LossPolicy) -> Result<Self> {
        let mut wave = Reader::new(BufReader::new(File::open(path)?))?;
        let info = wave.info();
        let rate = info.format.sample_rate;
        let axml = wave
            .chunks()
            .iter()
            .find(|chunk| chunk.id == *b"axml")
            .copied()
            .ok_or_else(|| unsupported("not an ADM BWF: there is no axml chunk"))?;
        if axml.size > AXML_LIMIT {
            return Err(unsupported("WAVE 元数据超过调用方预算"));
        }
        let chna_bytes = wave
            .metadata(*b"chna", CHNA_LIMIT)?
            .ok_or_else(|| unsupported("not an ADM BWF: there is no chna chunk"))?;
        let chna = Chna::decode_import(&chna_bytes)?;
        if chna.tracks != info.format.channels {
            return Err(Error::invalid("chna track count differs from fmt"));
        }
        let text = |b: &[u8]| {
            String::from_utf8_lossy(b)
                .trim_end_matches([' ', '\0'])
                .to_owned()
        };
        let mut tracks = Vec::new();
        let mut definitions = Vec::new();
        for entry in chna.entries.iter().filter(|e| e.track_index != 0) {
            if entry.track_index > chna.tracks {
                return Err(Error::invalid("chna track index exceeds the track count"));
            }
            let uid = text(&entry.uid);
            let mut format_id = text(&entry.track_format);
            if format_id.to_ascii_uppercase().starts_with("AC_") && format_id.ends_with("_00") {
                format_id.truncate(format_id.len() - 3);
            }
            let pack = text(&entry.pack_format);
            tracks.push((uid.clone(), entry.track_index));
            definitions.push(TrackUidDefinition {
                uid,
                format_id,
                pack_id: (!pack.is_empty()).then_some(pack),
            });
        }
        let index = Index::read(path, axml, rate)?;
        let doc = Document::parse_with_track_uids(&index.xml, &definitions).map_err(adm_error)?;
        let scene = Snapshot::extract(&doc, rate, &tracks).map_err(adm_error)?;
        let profile = if info.container == Container::Rf64
            && wave.chunks().iter().any(|c| &c.id == b"dbmd")
        {
            Profile::Logic
        } else {
            Profile::Standard
        };
        let mut diagnostics = Vec::new();
        let mut reader = Self {
            spec: MasterSpec {
                sample_rate: rate,
                profile,
                loss_policy: policy,
                ..MasterSpec::default()
            },
            path: path.to_owned(),
            channels: vec![None; BED_CHANNELS],
            updates: 0,
            timelines: Vec::new(),
            last_change: None,
            object_ends: Vec::new(),
            bed_end: None,
            last_sample: None,
            frames: info.frames,
            diagnostics: Vec::new(),
        };
        reader.project(
            &Scene {
                doc: &doc,
                scene: &scene,
                index: &index,
            },
            &mut Losses {
                policy,
                diagnostics: &mut diagnostics,
            },
        )?;
        reader.diagnostics = diagnostics;
        reader.spec.validate()?;
        Ok(reader)
    }

    fn project(&mut self, s: &Scene<'_>, losses: &mut Losses<'_>) -> Result<()> {
        let scene = s.scene;
        if let Some(&warning) = scene.warnings.first() {
            return Err(unsupported(s.text(warning).to_owned()));
        }
        let [programme] = scene.programmes.as_slice() else {
            return Err(unsupported(format!(
                "only single-programme ADM is supported (found {})",
                scene.programmes.len()
            )));
        };
        self.spec.name = s.text(programme.name).to_owned();
        for (lost, field, what) in [
            (
                programme.loudness.present != 0,
                "loudness",
                "Programme loudness metadata",
            ),
            (
                programme.reference_screen != 0,
                "reference_screen",
                "Programme reference screen",
            ),
            (programme.language != NONE, "language", "Programme language"),
            (programme.labels.len != 0, "labels", "Programme labels"),
            (
                programme.start != 0,
                "offset",
                "A programme start other than zero",
            ),
        ] {
            if lost {
                losses.report(None, None, field, format!("{what} has no mapping"))?;
            }
        }
        let mut objects = Vec::new();
        for content_id in s.list(programme.content_ids) {
            let content = scene
                .contents
                .iter()
                .find(|c| s.text(c.id) == content_id)
                .ok_or_else(|| {
                    Error::invalid(format!("programme references unknown {content_id}"))
                })?;
            for (lost, field, what) in [
                (
                    content.loudness.present != 0,
                    "loudness",
                    "Content loudness metadata",
                ),
                (content.language != NONE, "language", "Content language"),
                (content.labels.len != 0, "labels", "Content labels"),
                (
                    content.dialogue_kind != NONE,
                    "dialog",
                    "Content dialogue classification",
                ),
            ] {
                if lost {
                    losses.report(None, None, field, format!("{what} has no mapping"))?;
                }
            }
            for object_id in s.list(content.object_ids) {
                let index = scene
                    .objects
                    .iter()
                    .position(|o| s.text(o.id) == object_id)
                    .ok_or_else(|| {
                        Error::invalid(format!("content references unknown {object_id}"))
                    })?;
                if !objects.contains(&index) {
                    objects.push(index);
                }
            }
        }
        let mut bed = None;
        let mut singles = BTreeMap::new();
        for index in objects {
            let object = &scene.objects[index];
            let name = s.text(object.name);
            if object.child_objects.len != 0 || object.has_parent != 0 {
                return Err(unsupported(format!(
                    "nested audioObjects are not supported ({name})"
                )));
            }
            if scene
                .hoa
                .iter()
                .any(|h| s.text(h.object_id) == s.text(object.id))
            {
                return Err(unsupported(format!(
                    "HOA is not part of the Atmos subset ({name})"
                )));
            }
            let tracks =
                &scene.tracks[object.tracks.offset..object.tracks.offset + object.tracks.len];
            if !tracks.is_empty()
                && tracks
                    .iter()
                    .all(|t| t.ds_blocks.len > 0 && t.blocks.len == 0)
            {
                if bed.replace(index).is_some() {
                    return Err(unsupported("multi-bed ADM is not supported"));
                }
            } else if let [track] = tracks
                && track.blocks.len > 0
                && track.ds_blocks.len == 0
            {
                let number = u16::try_from(track.channel).map_err(|_| {
                    Error::invalid(format!("{} of {name} is not in chna", s.text(track.uid)))
                })?;
                if singles.insert(number, index).is_some() {
                    return Err(Error::invalid(format!(
                        "track {number} carries two objects"
                    )));
                }
            } else {
                return Err(unsupported(format!(
                    "{name} is not a 7.1.2 DirectSpeakers bed or a single-track Objects object"
                )));
            }
        }
        if let Some(index) = bed {
            self.bed(s, index, losses)?;
        }
        for (&number, &index) in &singles {
            self.object(s, index, number, losses)?;
        }
        Ok(())
    }

    /// Object-level attributes: what folds into the blocks (gain, mute) and what is a loss.
    fn object_level(
        s: &Scene<'_>,
        index: usize,
        track: Option<TrackId>,
        // The loss fields for positionOffset, importance, dialogue and headLocked.
        fields: &'static [&'static str; 4],
        losses: &mut Losses<'_>,
    ) -> Result<Option<f64>> {
        let object = &s.scene.objects[index];
        let node = object.node;
        for attribute in ["interact", "disableDucking"] {
            if s.doc.element_attribute(node, attribute).is_some() {
                losses.report(
                    track,
                    None,
                    "unknown_field",
                    format!("audioObject {attribute} has no mapping"),
                )?;
            }
        }
        for child in s.doc.element_children(node) {
            let name = s.doc.element_name(child).unwrap_or_default();
            if !OBJECT_ELEMENTS.contains(&name) && name != "audioObjectLabel" {
                losses.report(
                    track,
                    None,
                    "unknown_field",
                    format!("audioObject element {name:?} has no mapping"),
                )?;
            }
        }
        for (lost, field, what) in [
            (object.offset_present != 0, fields[0], "positionOffset"),
            (
                object.importance_present != 0 && object.importance != 10,
                fields[1],
                "Object-level importance",
            ),
            (
                object.dialogue_present != 0,
                fields[2],
                "Object dialogue classification",
            ),
            (object.head_locked != 0, fields[3], "Object headLocked"),
            (object.labels.len != 0, "labels", "Object labels"),
        ] {
            if lost {
                losses.report(track, None, field, format!("{what} has no mapping"))?;
            }
        }
        if object.mute_present != 0 && object.mute != 0 {
            return Ok(None);
        }
        Ok(Some(if object.source_gain.present == 0 {
            1.
        } else if object.source_gain.decibels != 0 {
            libm::pow(10., object.source_gain.value / 20.)
        } else {
            object.source_gain.value
        }))
    }

    fn bed(&mut self, s: &Scene<'_>, index: usize, losses: &mut Losses<'_>) -> Result<()> {
        let scene = s.scene;
        let object = &scene.objects[index];
        let name = s.text(object.name);
        self.spec.bed_name = name.to_owned();
        let factor = Self::object_level(
            s,
            index,
            None,
            &[
                "bed_position",
                "bed_importance",
                "bed_dialog",
                "bed_head_tracking",
            ],
            losses,
        )?;
        if factor != Some(1.) {
            losses.report(
                None,
                None,
                "bed_gain",
                "Bed object gain or mute has no mapping".into(),
            )?;
        }
        let ends_early = object.duration_present != 0 && object.end < self.frames;
        if object.absolute_start != 0 || ends_early {
            losses.report(
                None,
                None,
                "bed_timing",
                "A bed that does not span the programme has no mapping".into(),
            )?;
        }
        let tracks = &scene.tracks[object.tracks.offset..object.tracks.offset + object.tracks.len];
        self.bed_end = Some(object.end);
        for track in tracks {
            s.channel_losses(track.uid, None, losses)?;
            let channels = s.channels(track.uid, self.spec.sample_rate)?;
            let mut blocks = ProjectedBlocks::new(
                &channels,
                &self.path,
                self.spec.sample_rate,
                object.absolute_start,
                object.head_locked,
            )?;
            let first = blocks
                .next()?
                .ok_or_else(|| Error::invalid("bed 声道没有 block"))?;
            let first_block = &first.scene.direct_blocks[0];
            let first_start = first_block.start;
            let label = first.scene.indices
                [first_block.labels.offset..first_block.labels.offset + first_block.labels.len]
                .first()
                .map(|&i| first.scene.strings[i as usize].as_str())
                .unwrap_or_default()
                .to_owned();
            let short = label.strip_prefix("RC_").unwrap_or(&label).to_owned();
            let slot = BED_LABELS.iter().position(|l| *l == short).ok_or_else(|| {
                unsupported(format!("bed channel {label:?} is not part of a 7.1.2 bed"))
            })?;
            let source_channel = u32::try_from(track.channel)
                .ok()
                .and_then(|n| n.checked_sub(1))
                .ok_or_else(|| {
                    Error::invalid(format!("bed track {} is not in chna", s.text(track.uid)))
                })?;
            if self.channels[slot].replace(source_channel).is_some() {
                return Err(Error::invalid(format!("bed channel {label} appears twice")));
            }
            let end = channel_end(&channels, object.absolute_start).min(object.end);
            self.bed_end = self.bed_end.map(|previous| previous.min(end));
            if first_start != 0 || end < self.frames {
                losses.report(
                    None,
                    Some(first_start),
                    "bed_timing",
                    format!("Bed channel {label} does not continuously span the programme"),
                )?;
            }
            let pack_id = scene.direct_blocks[track.ds_blocks.offset].pack_id;
            if pack_id != NONE
                && let Some(pack) = s.doc.element_by_id(s.text(pack_id))
                && let Some(pack_name) = s.doc.element_attribute(pack, "audioPackFormatName")
            {
                pack_name.clone_into(&mut self.spec.bed_pack_name);
            }
            let mut current = Some(first);
            let mut previous_end = None;
            while let Some(item) = current.take() {
                current = blocks.next()?;
                let block = &item.scene.direct_blocks[0];
                if previous_end.is_some_and(|end| end != block.start) {
                    losses.report(
                        None,
                        Some(first_start),
                        "bed_timing",
                        format!("Bed channel {label} does not continuously span the programme"),
                    )?;
                }
                previous_end = Some(block.end);
                let s = Scene {
                    doc: &item.doc,
                    scene: &item.scene,
                    index: s.index,
                };
                let gain = block.source.gain;
                let unity = if gain.decibels != 0 { 0. } else { 1. };
                if gain.value != unity {
                    losses.report(
                        None,
                        Some(block.start),
                        "bed_gain",
                        format!("Bed channel {label} gain has no mapping"),
                    )?;
                }
                if block.head_locked != 0 {
                    losses.report(
                        None,
                        Some(block.start),
                        "bed_head_tracking",
                        format!("Bed channel {label} headLocked has no mapping"),
                    )?;
                }
                if !s
                    .list(block.labels)
                    .map(|l| l.strip_prefix("RC_").unwrap_or(l))
                    .eq(std::iter::once(short.as_str()))
                {
                    losses.report(
                        None,
                        Some(block.start),
                        "bed_position",
                        format!("Bed channel {label} changes its speaker labels"),
                    )?;
                }
                for child in s.doc.element_children(block.node) {
                    let field = s.doc.element_name(child).unwrap_or_default();
                    match field {
                        "speakerLabel" | "cartesian" | "gain" | "headLocked" => {}
                        "position" => {
                            if ["bound", "screenEdgeLock"]
                                .iter()
                                .any(|a| s.doc.element_attribute(child, a).is_some())
                            {
                                losses.report(
                                    None,
                                    Some(block.start),
                                    "bed_position",
                                    format!(
                                        "Bed channel {label} position attributes have no mapping"
                                    ),
                                )?;
                            }
                        }
                        "importance" => {
                            let text = s.doc.element_text(child).unwrap_or_default();
                            if number(text, "importance")? != 10. {
                                losses.report(
                                    None,
                                    Some(block.start),
                                    "bed_importance",
                                    format!("Bed channel {label} importance has no mapping"),
                                )?;
                            }
                        }
                        _ => losses.report(
                            None,
                            Some(block.start),
                            "unknown_field",
                            format!("Bed channel {label} block element {field:?} has no mapping"),
                        )?,
                    }
                }
            }
        }
        if self.channels.iter().any(Option::is_none) {
            return Err(unsupported("only a complete 7.1.2 bed is supported"));
        }
        Ok(())
    }

    fn object(
        &mut self,
        s: &Scene<'_>,
        index: usize,
        number: u16,
        losses: &mut Losses<'_>,
    ) -> Result<()> {
        let scene = s.scene;
        let object = &scene.objects[index];
        let id = TrackId(u64::from(number));
        let name = s.text(object.name).to_owned();
        let factor = Self::object_level(
            s,
            index,
            Some(id),
            &["position_offset", "importance", "dialog", "head_tracking"],
            losses,
        )?;
        let track = &scene.tracks[object.tracks.offset];
        s.channel_losses(track.uid, Some(id), losses)?;
        let channels = s.channels(track.uid, self.spec.sample_rate)?;
        let end = channel_end(&channels, object.absolute_start).min(object.end);
        if end != u64::MAX {
            self.object_ends.push((id, end));
        }
        let timeline = ObjectTimeline {
            id,
            channels,
            start: object.absolute_start,
            locked: object.head_locked,
            factor,
            end,
        };
        let mut events = ObjectEvents::new(
            &self.path,
            timeline.clone(),
            self.spec.sample_rate,
            self.frames,
            self.frames,
            losses,
        )?;
        let initial = events.initial;
        while let Some(update) = events.next_event(losses)? {
            self.updates += 1;
            self.last_sample = self.last_sample.max(Some(update.sample));
            if update.patch.active != Some(false) {
                self.last_change = self.last_change.max(Some(update.sample));
            }
        }
        self.timelines.push(timeline);
        let mut spec = ObjectSpec::new(id, name);
        spec.initial = initial;
        self.spec.objects.push(spec);
        self.channels.push(Some(u32::from(number) - 1));
        Ok(())
    }

    /// Set the frame rate the outputs declare. ADM carries none, and a Logic source's DBMD frame
    /// rate is not read back; the default is the spec's (24 fps).
    #[must_use]
    pub fn with_frame_rate(mut self, frame_rate: FrameRate) -> Self {
        self.spec.frame_rate = frame_rate;
        self
    }
    /// 设置转换输出中 Logic DBMD 的创建来源与产品字段。
    /// 创建来源最多 32 字节，产品最多 64 字节，均为不含控制字符的 ASCII 文本。
    #[must_use]
    pub fn with_dbmd_identity(
        mut self,
        originator: impl Into<String>,
        product: impl Into<String>,
    ) -> Self {
        self.spec.dbmd_originator = originator.into();
        self.spec.dbmd_product = product.into();
        self
    }
    /// The model: bed and objects in output order (objects by track), initial states, and the
    /// source profile (`Logic` for RF64 with DBMD, otherwise `Standard`).
    pub fn spec(&self) -> &MasterSpec {
        &self.spec
    }
    /// What reading dropped under `LossPolicy::Allow`.
    pub fn diagnostics(&self) -> &[MappingDiagnostic] {
        &self.diagnostics
    }
    /// 原音频长度下的更新，按对象、时间顺序流式读取；I/O 和解析错误通过迭代项返回。
    /// 源文件在迭代完成前必须保持可读且内容不变。
    pub fn updates(&self) -> Result<Updates> {
        self.updates_for_frames(self.frames)
    }
    /// 返回指定音频长度下的更新，保留有限的对象结束边界。
    /// 非停用事件仍按原音频范围读取，由转换流程核对替换音频是否覆盖它们。
    pub fn updates_for_frames(&self, frames: u64) -> Result<Updates> {
        let mut updates = Updates {
            path: self.path.clone(),
            timelines: self.timelines.clone().into_iter(),
            current: None,
            rate: self.spec.sample_rate,
            source_frames: self.frames,
            frames,
            policy: self.spec.loss_policy,
            failed: false,
        };
        updates.advance()?;
        Ok(updates)
    }
    /// 原文件长度下的更新数；已在打开时统计，无需再扫描元数据。
    pub fn update_count(&self) -> u64 {
        self.updates
    }
    /// 指定音频长度下的更新数，包括相应的有限停用边界。
    pub fn update_count_for_frames(&self, frames: u64) -> u64 {
        self.updates
            - self
                .object_ends
                .iter()
                .filter(|(_, end)| *end < self.frames && *end >= frames)
                .count() as u64
            + self
                .object_ends
                .iter()
                .filter(|(_, end)| *end >= self.frames && *end < frames)
                .count() as u64
    }
    pub fn last_event_sample(&self) -> Option<u64> {
        self.last_sample
    }
    /// The file's own audio.
    pub fn audio(&self) -> Result<AudioSource> {
        self.wave_audio(&self.path)
    }
    /// A WAV in place of the file's audio, with channel `n` carrying ADM track `n + 1`.
    pub fn wave_audio(&self, path: impl AsRef<Path>) -> Result<AudioSource> {
        let path = path.as_ref();
        let wave = File::open(path)
            .map_err(Error::from)
            .and_then(|f| Reader::new(BufReader::new(f)).map_err(Error::from))
            .map_err(|e| located(path, e))?;
        AudioSource::new(
            self.spec.sample_rate,
            &self.channels,
            "ADM track index",
            Input::Wave(wave),
        )
    }
    /// Write the master to every target in one pass through [`MultiWriter`].
    pub fn convert(&self, audio: AudioSource, targets: &[Target]) -> Result<Conversion> {
        let audio = AudioSource::new(
            self.spec.sample_rate,
            &self.channels,
            "ADM track index",
            audio.input,
        )?;
        let frames = audio.frames();
        let mut diagnostics = self.diagnostics.clone();
        if self
            .bed_end
            .is_some_and(|end| end >= self.frames && end < frames)
        {
            Losses {
                policy: self.spec.loss_policy,
                diagnostics: &mut diagnostics,
            }
            .report(
                None,
                None,
                "bed_timing",
                "The bed ends before the replacement audio and cannot be extended exactly".into(),
            )?;
        }
        let updates = self.updates_for_frames(frames)?;
        let last_sample = self.last_change.max(
            self.object_ends
                .iter()
                .filter(|(_, end)| *end < frames)
                .map(|(_, end)| *end)
                .max(),
        );
        write_conversion(
            &self.spec,
            updates,
            audio,
            last_sample,
            &diagnostics,
            targets,
        )
    }
}
