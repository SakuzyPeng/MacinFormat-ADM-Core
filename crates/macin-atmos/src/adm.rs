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
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

mod bed;
mod stream;
pub use bed::BedBlocks;
use bed::BedTimeline;
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
    beds: Vec<BedTimeline>,
    last_sample: Option<u64>,
    frames: u64,
    programme_duration: Option<u64>,
    diagnostics: Vec<MappingDiagnostic>,
    dbmd_channels: Option<Vec<StaticChannelMetadata>>,
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
                kind: MappingDiagnosticKind::Loss,
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
const BLOCK_ELEMENTS: [&str; 15] = [
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
    "zoneExclusion",
    "headphoneVirtualise",
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
#[derive(Debug, Clone, Copy)]
struct BlockContext {
    base: ObjectState,
    dbmd_head: Option<HeadTracking>,
}

fn resolve_head(
    value: HeadTracking,
    dbmd: Option<HeadTracking>,
    losses: &mut Losses<'_>,
    track: Option<TrackId>,
    sample: Option<u64>,
    field: &'static str,
) -> Result<HeadTracking> {
    if let Some(dbmd) = dbmd {
        if value != dbmd {
            losses.report(
                track,
                sample,
                field,
                "XML headLocked conflicts with DBMD; keeping DBMD".into(),
            )?;
        }
        Ok(dbmd)
    } else {
        Ok(value)
    }
}

fn head(value: u32) -> HeadTracking {
    if value == 0 {
        HeadTracking::SceneRelative
    } else {
        HeadTracking::HeadRelative
    }
}

fn headphone(doc: &Document, node: u32) -> Result<HeadphoneVirtualise> {
    let value = HeadphoneVirtualise {
        bypass: doc
            .element_attribute(node, "bypass")
            .map(parse_boolean)
            .transpose()
            .map_err(adm_error)?
            .map(|v| v != 0),
        drr: doc
            .element_attribute(node, "DRR")
            .map(|v| number(v, "DRR"))
            .transpose()?,
    };
    value.validate()?;
    Ok(value)
}
fn zone_list(doc: &Document, root: u32) -> Result<Option<Vec<ExclusionZone>>> {
    let plain = |n| {
        doc.element_attributes(n)
            .into_iter()
            .filter(|(k, _)| *k != "xmlns" && !k.starts_with("xmlns:"))
            .collect::<Vec<_>>()
    };
    if !plain(root).is_empty() || !doc.foreign_child_names(root).is_empty() {
        return Ok(None);
    }
    let mut zones = Vec::new();
    for n in doc.element_children(root) {
        if doc.element_name(n) != Some("zone")
            || !doc.element_children(n).is_empty()
            || !doc.foreign_child_names(n).is_empty()
        {
            return Ok(None);
        }
        let attrs = plain(n);
        let read = |keys: &[&str]| -> Result<Option<Vec<f64>>> {
            if keys.len() != attrs.len()
                || !keys.iter().all(|k| doc.element_attribute(n, k).is_some())
            {
                return Ok(None);
            }
            keys.iter()
                .map(|k| number(doc.element_attribute(n, k).unwrap(), k))
                .collect::<Result<_>>()
                .map(Some)
        };
        let bounds = if let Some(v) = read(&crate::zones::ATTRIBUTES)? {
            ZoneBounds::Cartesian(v.try_into().unwrap())
        } else if let Some(v) = read(&crate::block::POLAR_ZONE_ATTRS)? {
            ZoneBounds::Polar(v.try_into().unwrap())
        } else {
            return Ok(None);
        };
        let z = ExclusionZone {
            name: doc.element_text(n).unwrap_or_default().to_owned(),
            bounds,
        };
        z.validate()?;
        zones.push(z);
    }
    Ok((!zones.is_empty()).then_some(zones))
}
fn block_state(
    doc: &Document,
    node: u32,
    context: BlockContext,
    losses: &mut Losses<'_>,
    track: TrackId,
    sample: u64,
) -> Result<(ObjectState, Vec<ExclusionZone>)> {
    // Each block is complete on its own: an element it leaves out takes its default, not the
    // previous block's value. Only what ADM does not spell per block carries over from `base`.
    let mut state = ObjectState {
        snap: false,
        screen_ref: false,
        headphone_virtualise: None,
        decorrelation: 0.,
        diffuse: 0.,
        divergence: None,
        channel_lock_distance: None,
        screen_edge_lock: [None; 3],
        zone: Zone::All,
        elevation: true,
        ..context.base
    };
    let mut custom = Vec::new();
    let mut position = [None; 3];
    let mut size = [0.; 3];
    let mut gain = Gain::Linear(1.);
    let mut importance = 1.;
    let at = |e: Error| e.context(track, sample, "block");
    for name in doc.foreign_child_names(node) {
        losses.report(
            Some(track),
            Some(sample),
            "unknown_field",
            format!("Block foreign child {name:?} has no mapping"),
        )?;
    }
    let mut singletons = BTreeSet::new();
    for (name, _) in doc.element_attributes(node) {
        if !["audioBlockFormatID", "rtime", "duration"].contains(&name)
            && name != "xmlns"
            && !name.starts_with("xmlns:")
        {
            losses.report(
                Some(track),
                Some(sample),
                "unknown_field",
                format!("Block attribute {name:?} has no mapping"),
            )?;
        }
    }
    for child in doc.element_children(node) {
        let name = doc.element_name(child).unwrap_or_default();
        let text = doc.element_text(child).unwrap_or_default();
        if name != "position" && BLOCK_ELEMENTS.contains(&name) && !singletons.insert(name) {
            return Err(at(Error::invalid(format!("Duplicate block {name}"))));
        }
        if name != "zoneExclusion" && BLOCK_ELEMENTS.contains(&name) {
            let allowed: &[&str] = match name {
                "position" => &["coordinate", "bound", "screenEdgeLock"],
                "gain" => &["gainUnit"],
                "jumpPosition" => &["interpolationLength"],
                "channelLock" => &["maxDistance"],
                "headphoneVirtualise" => &["bypass", "DRR"],
                "objectDivergence" => &["azimuthRange", "positionRange"],
                _ => &[],
            };
            for (attribute, _) in doc.element_attributes(child) {
                if !allowed.contains(&attribute)
                    && attribute != "xmlns"
                    && !attribute.starts_with("xmlns:")
                {
                    losses.report(
                        Some(track),
                        Some(sample),
                        "unknown_field",
                        format!("Block {name} attribute {attribute:?} has no mapping"),
                    )?;
                }
            }
            if !doc.element_children(child).is_empty() || !doc.foreign_child_names(child).is_empty()
            {
                losses.report(
                    Some(track),
                    Some(sample),
                    "unknown_field",
                    format!("Block {name} contains unmapped child elements"),
                )?;
            }
        }
        match name {
            "position" => {
                if doc.element_attribute(child, "bound").is_some() {
                    losses.report(
                        Some(track),
                        Some(sample),
                        "position_bound",
                        "Object position bounds have no mapping".into(),
                    )?;
                    continue;
                }
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
                if let Some(edge) = doc.element_attribute(child, "screenEdgeLock") {
                    state.screen_edge_lock[axis] = Some(match edge {
                        "left" => ScreenEdge::Left,
                        "right" => ScreenEdge::Right,
                        "top" => ScreenEdge::Top,
                        "bottom" => ScreenEdge::Bottom,
                        _ => return Err(at(Error::invalid("Invalid screenEdgeLock"))),
                    });
                }
                if position[axis]
                    .replace(number(text, "position").map_err(at)?)
                    .is_some()
                {
                    return Err(at(Error::invalid("Duplicate object coordinate")));
                }
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
                state.channel_lock_distance = doc
                    .element_attribute(child, "maxDistance")
                    .map(|v| number(v, "maxDistance"))
                    .transpose()
                    .map_err(at)?;
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
            "diffuse" => state.diffuse = number(text, "diffuse").map_err(at)?,
            "objectDivergence" => {
                state.divergence = Some(ObjectDivergence {
                    value: number(text, "objectDivergence").map_err(at)?,
                    azimuth_range: doc
                        .element_attribute(child, "azimuthRange")
                        .map(|v| number(v, "azimuthRange"))
                        .transpose()
                        .map_err(at)?,
                    position_range: doc
                        .element_attribute(child, "positionRange")
                        .map(|v| number(v, "positionRange"))
                        .transpose()
                        .map_err(at)?,
                });
            }
            "headLocked" => {
                state.head_tracking = resolve_head(
                    head(parse_boolean(text).map_err(adm_error).map_err(at)?),
                    context.dbmd_head,
                    losses,
                    Some(track),
                    Some(sample),
                    "head_tracking",
                )?;
            }
            "headphoneVirtualise" => {
                state.headphone_virtualise = Some(headphone(doc, child).map_err(at)?)
            }
            "zoneExclusion" => {
                if let Some(zones) = zone_list(doc, child).map_err(at)? {
                    let rectangles = zones
                        .iter()
                        .map(|z| match z.bounds {
                            ZoneBounds::Cartesian(bounds) => Some((z.name.as_str(), bounds)),
                            _ => None,
                        })
                        .collect::<Option<Vec<_>>>();
                    if let Some((zone, elevation)) = rectangles.and_then(crate::zones::decode) {
                        state.zone = zone;
                        state.elevation = elevation;
                    } else {
                        custom = zones;
                    }
                } else {
                    losses.report(
                        Some(track),
                        Some(sample),
                        "zone",
                        "Incomplete or unsupported zoneExclusion geometry".into(),
                    )?;
                }
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
    state.gain = gain;
    state.importance = importance;
    state.validate().map_err(at)?;
    Ok((state, custom))
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
        decorrelation: changed(current.decorrelation != next.decorrelation)
            .map(|()| next.decorrelation),
        diffuse: changed(current.diffuse != next.diffuse).map(|()| next.diffuse),
        divergence: changed(current.divergence != next.divergence).map(|()| next.divergence),
        channel_lock_distance: changed(current.channel_lock_distance != next.channel_lock_distance)
            .map(|()| next.channel_lock_distance),
        screen_edge_lock: changed(current.screen_edge_lock != next.screen_edge_lock)
            .map(|()| next.screen_edge_lock),
        headphone_virtualise: changed(current.headphone_virtualise != next.headphone_virtualise)
            .map(|()| next.headphone_virtualise),
        zone: changed(current.zone != next.zone).map(|()| next.zone),
        elevation: changed(current.elevation != next.elevation).map(|()| next.elevation),
        head_tracking: changed(current.head_tracking != next.head_tracking)
            .map(|()| next.head_tracking),
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
    base: ObjectState,
    dbmd_head: Option<HeadTracking>,
    end: u64,
}
impl ObjectTimeline {
    fn context(&self) -> BlockContext {
        BlockContext {
            base: self.base,
            dbmd_head: self.dbmd_head,
        }
    }
}

struct ObjectEvents {
    blocks: ProjectedBlocks,
    timeline: ObjectTimeline,
    state: ObjectState,
    initial: ObjectState,
    zones: Vec<ExclusionZone>,
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
        let (state, zones) = block_state(
            &first.doc,
            block.node,
            timeline.context(),
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
            zones,
            previous_end: block.end,
            activation,
            source_frames,
            frames,
            done: false,
        })
    }

    fn next_event(&mut self, losses: &mut Losses<'_>) -> Result<Option<NativeObjectUpdate>> {
        if let Some(update) = self.activation.take() {
            return Ok(Some(update.into()));
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
            let (next, zones) = block_state(
                &item.doc,
                block.node,
                self.timeline.context(),
                losses,
                self.timeline.id,
                block.start,
            )?;
            let patch = difference(&self.state, &next);
            self.state = next;
            let zones = (zones != self.zones).then(|| {
                self.zones = zones;
                self.zones.clone()
            });
            if zones.is_none() && patch == ObjectPatch::default() {
                continue;
            }
            let duration = block.end.saturating_sub(block.start);
            let ramp = if block.jump == 0 {
                if block.end == u64::MAX { 0 } else { duration }
            } else {
                block.interpolation.min(duration)
            };
            return Ok(Some(NativeObjectUpdate {
                zones,
                event: ObjectUpdate {
                    track: self.timeline.id,
                    sample: block.start,
                    ramp_samples: patch.has_continuous().then_some(ramp),
                    patch,
                },
            }));
        }
        self.done = true;
        Ok((self.timeline.end < self.frames)
            .then_some(ObjectUpdate {
                track: self.timeline.id,
                sample: self.timeline.end,
                ramp_samples: None,
                patch: ObjectPatch {
                    active: Some(false),
                    ..ObjectPatch::default()
                },
            })
            .map(Into::into))
    }
}

/// 逐对象、逐 block 读取的 ADM 更新流；遇到错误后终止。
pub struct NativeUpdates {
    path: PathBuf,
    timelines: std::vec::IntoIter<ObjectTimeline>,
    current: Option<ObjectEvents>,
    rate: u32,
    source_frames: u64,
    frames: u64,
    policy: LossPolicy,
    failed: bool,
}

impl std::fmt::Debug for NativeUpdates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Updates")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl NativeUpdates {
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
    fn next_update(&mut self) -> Result<Option<NativeObjectUpdate>> {
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

impl Iterator for NativeUpdates {
    type Item = Result<NativeObjectUpdate>;
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

impl std::iter::FusedIterator for NativeUpdates {}
/// 数值更新投影。需要自定义区域时使用 native_updates_for_frames。
#[derive(Debug)]
pub struct Updates(NativeUpdates);
impl Iterator for Updates {
    type Item = Result<ObjectUpdate>;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|r| r.map(|v| v.event))
    }
}
impl std::iter::FusedIterator for Updates {}

struct Scene<'a> {
    doc: &'a Document,
    scene: &'a Snapshot,
    index: Option<&'a Index>,
}
impl Scene<'_> {
    fn metadata(
        &self,
        node: u32,
        level: crate::native::Level,
        track: Option<TrackId>,
        losses: &mut Losses<'_>,
    ) -> Result<AdmMetadata> {
        let (attrs, elements) = crate::native::schema(level.root());
        let metadata = AdmMetadata {
            attributes: self
                .doc
                .element_attributes(node)
                .into_iter()
                .filter(|(k, _)| attrs.contains(k))
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            elements: self
                .doc
                .element_children(node)
                .into_iter()
                .filter(|&n| elements.contains(&self.doc.element_name(n).unwrap_or_default()))
                .map(|n| self.metadata_element(n, track, losses, 0))
                .collect::<Result<_>>()?,
        };
        metadata.validate(level)?;
        Ok(metadata)
    }
    fn metadata_element(
        &self,
        node: u32,
        track: Option<TrackId>,
        losses: &mut Losses<'_>,
        depth: usize,
    ) -> Result<AdmElement> {
        if depth > 4 {
            return Err(Error::invalid("ADM metadata nesting exceeds its schema"));
        }
        let name = self.doc.element_name(node).unwrap_or_default();
        let (attrs, elements) = crate::native::schema(name);
        self.attributes(node, attrs, track, losses)?;
        self.children(node, elements, track, losses)?;
        Ok(AdmElement {
            name: name.to_owned(),
            attributes: self
                .doc
                .element_attributes(node)
                .into_iter()
                .filter(|(k, _)| attrs.contains(k))
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            text: if elements.is_empty() {
                self.doc.element_text(node).unwrap_or_default().to_owned()
            } else {
                self.doc
                    .element_text(node)
                    .unwrap_or_default()
                    .trim()
                    .to_owned()
            },
            children: self
                .doc
                .element_children(node)
                .into_iter()
                .filter(|&n| elements.contains(&self.doc.element_name(n).unwrap_or_default()))
                .map(|n| self.metadata_element(n, track, losses, depth + 1))
                .collect::<Result<_>>()?,
        })
    }
    fn channel_metadata(
        &self,
        uid: u32,
        track: Option<TrackId>,
        losses: &mut Losses<'_>,
    ) -> Result<AdmMetadata> {
        let mut metadata = None;
        for node in self.doc.track_channels(self.text(uid)) {
            let next = self.metadata(node, crate::native::Level::Channel, track, losses)?;
            if metadata.as_ref().is_some_and(|previous| *previous != next) {
                losses.report(
                    track,
                    None,
                    "channel_frequency",
                    "Merged channels have different frequency metadata".into(),
                )?;
            } else {
                metadata = Some(next);
            }
        }
        Ok(metadata.unwrap_or_default())
    }
    fn attributes(
        &self,
        node: u32,
        allowed: &[&str],
        track: Option<TrackId>,
        losses: &mut Losses<'_>,
    ) -> Result<()> {
        for (name, _) in self.doc.element_attributes(node) {
            if !allowed.contains(&name)
                && !crate::native::schema(self.doc.element_name(node).unwrap_or_default())
                    .0
                    .contains(&name)
                && name != "xmlns"
                && !name.starts_with("xmlns:")
            {
                losses.report(
                    track,
                    None,
                    "unknown_field",
                    format!(
                        "{} attribute {name:?} has no mapping",
                        self.doc.element_name(node).unwrap_or("element")
                    ),
                )?;
            }
        }
        Ok(())
    }
    fn children(
        &self,
        node: u32,
        allowed: &[&str],
        track: Option<TrackId>,
        losses: &mut Losses<'_>,
    ) -> Result<()> {
        for name in self.doc.foreign_child_names(node) {
            losses.report(
                track,
                None,
                "unknown_field",
                format!(
                    "{} foreign child {name:?} has no mapping",
                    self.doc.element_name(node).unwrap_or("element")
                ),
            )?;
        }
        for child in self.doc.element_children(node) {
            let name = self.doc.element_name(child).unwrap_or_default();
            if !allowed.contains(&name)
                && !crate::native::schema(self.doc.element_name(node).unwrap_or_default())
                    .1
                    .contains(&name)
            {
                losses.report(
                    track,
                    None,
                    "unknown_field",
                    format!(
                        "{} child {name:?} has no mapping",
                        self.doc.element_name(node).unwrap_or("element")
                    ),
                )?;
            }
        }
        Ok(())
    }
    fn pack_losses(
        &self,
        object: u32,
        track: Option<TrackId>,
        losses: &mut Losses<'_>,
    ) -> Result<()> {
        let mut pending: Vec<_> = self
            .doc
            .element_children(object)
            .into_iter()
            .filter(|&n| self.doc.element_name(n) == Some("audioPackFormatIDRef"))
            .collect();
        if pending.len() > 1 {
            losses.report(
                track,
                None,
                "pack_structure",
                "Multiple object pack references are flattened".into(),
            )?;
        }
        let mut seen = BTreeSet::new();
        while let Some(reference) = pending.pop() {
            let Some(node) = self
                .doc
                .element_text(reference)
                .and_then(|id| self.doc.element_by_id(id.trim()))
            else {
                return Err(Error::invalid("Unknown pack reference"));
            };
            if !seen.insert(node) {
                continue;
            }
            self.attributes(
                node,
                &[
                    "audioPackFormatID",
                    "audioPackFormatName",
                    "typeLabel",
                    "typeDefinition",
                    "importance",
                ],
                track,
                losses,
            )?;
            self.children(
                node,
                &["audioChannelFormatIDRef", "audioPackFormatIDRef"],
                track,
                losses,
            )?;
            if self
                .doc
                .element_children(node)
                .iter()
                .any(|&n| self.doc.element_name(n) == Some("audioPackFormatIDRef"))
            {
                losses.report(
                    track,
                    None,
                    "pack_structure",
                    "Nested pack metadata cannot be flattened exactly".into(),
                )?;
            }
            pending.extend(
                self.doc
                    .element_children(node)
                    .into_iter()
                    .filter(|&n| self.doc.element_name(n) == Some("audioPackFormatIDRef")),
            );
        }
        Ok(())
    }
    fn track_pack(&self, object: u32, uid: u32) -> Option<u32> {
        self.doc
            .pack_for_track(object, self.doc.element_by_id(self.text(uid))?)
    }
    fn channel_names(
        &self,
        object: u32,
        uid: u32,
        default: &str,
        track: Option<TrackId>,
        losses: &mut Losses<'_>,
    ) -> Result<ChannelNames> {
        let channel = self.doc.track_channels(self.text(uid)).first().copied();
        let name = channel
            .and_then(|n| self.doc.element_attribute(n, "audioChannelFormatName"))
            .filter(|name| *name != default)
            .map(str::to_owned);
        let pcm = format!("PCM_{}", name.as_deref().unwrap_or(default));
        let formats = self.doc.track_format_elements(self.text(uid));
        let uid_node = self.doc.element_by_id(self.text(uid));
        let mut stream_pack_ref = false;
        if let Some((_, stream)) = formats {
            let assigned = self.track_pack(object, uid);
            for child in self.doc.element_children(stream) {
                if self.doc.element_name(child) == Some("audioPackFormatIDRef") {
                    self.attributes(child, &[], track, losses)?;
                    self.children(child, &[], track, losses)?;
                    let target = self
                        .doc
                        .element_text(child)
                        .and_then(|id| self.doc.element_by_id(id.trim()));
                    if !stream_pack_ref && assigned.is_some() && target == assigned {
                        stream_pack_ref = true;
                    } else {
                        losses.report(
                            track,
                            None,
                            "pack_structure",
                            "Stream pack reference is repeated or differs from its assigned pack"
                                .into(),
                        )?;
                    }
                }
            }
        }
        for (node, attrs, children) in [
            (
                uid_node,
                &["UID", "sampleRate", "bitDepth"][..],
                &[
                    "audioTrackFormatIDRef",
                    "audioChannelFormatIDRef",
                    "audioPackFormatIDRef",
                ][..],
            ),
            (
                formats.map(|(t, _)| t),
                &[
                    "audioTrackFormatID",
                    "audioTrackFormatName",
                    "formatLabel",
                    "formatDefinition",
                ],
                &["audioStreamFormatIDRef"],
            ),
            (
                formats.map(|(_, s)| s),
                &[
                    "audioStreamFormatID",
                    "audioStreamFormatName",
                    "formatLabel",
                    "formatDefinition",
                ],
                &[
                    "audioChannelFormatIDRef",
                    "audioPackFormatIDRef",
                    "audioTrackFormatIDRef",
                ],
            ),
        ] {
            if let Some(node) = node {
                let qualified = self.doc.qualified_attributes(node);
                let mut allowed = attrs.to_vec();
                allowed.extend(qualified.iter().map(|a| a.name));
                self.attributes(node, &allowed, track, losses)?;
                self.children(node, children, track, losses)?;
            }
        }
        let qualified = |node: Option<u32>| {
            node.into_iter()
                .flat_map(|node| self.doc.qualified_attributes(node))
                .filter_map(|a| {
                    let (prefix, local) = a.name.split_once(':')?;
                    Some(VendorAttribute {
                        prefix: prefix.into(),
                        name: local.into(),
                        namespace: a.namespace.into(),
                        value: a.value.into(),
                        value_prefixes: a.value_prefixes.to_vec(),
                    })
                })
                .collect()
        };
        Ok(ChannelNames {
            stream_pack_ref,
            extensions: TrackExtensions {
                track: qualified(formats.map(|(t, _)| t)),
                stream: qualified(formats.map(|(_, s)| s)),
                uid: qualified(self.doc.element_by_id(self.text(uid))),
            },
            channel: name,
            stream: formats
                .and_then(|(_, n)| self.doc.element_attribute(n, "audioStreamFormatName"))
                .filter(|name| *name != pcm)
                .map(str::to_owned),
            track: formats
                .and_then(|(n, _)| self.doc.element_attribute(n, "audioTrackFormatName"))
                .filter(|name| *name != pcm)
                .map(str::to_owned),
        })
    }
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
                    .and_then(|index| index.channels.get(&id.to_ascii_uppercase()))
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
            self.attributes(
                channel,
                &[
                    "audioChannelFormatID",
                    "audioChannelFormatName",
                    "typeLabel",
                    "typeDefinition",
                ],
                track,
                losses,
            )?;
            for child in self.doc.element_children(channel) {
                let name = self.doc.element_name(child).unwrap_or_default();
                if !matches!(name, "audioBlockFormat" | "frequency" | "channelFrequency") {
                    let field = "unknown_field";
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
        Self::open_with_options(
            path,
            MappingOptions {
                loss_policy: policy,
                ..Default::default()
            },
        )
    }

    /// 读取前选择允许的近似；选项同时传递给转换输出和更新迭代。
    pub fn open_with_options(path: impl AsRef<Path>, options: MappingOptions) -> Result<Self> {
        let path = path.as_ref();
        Self::read(path, options).map_err(|e| located(path, e))
    }

    fn read(path: &Path, options: MappingOptions) -> Result<Self> {
        let mut wave = Reader::new(BufReader::new(File::open(path)?))?;
        let info = wave.info();
        let rate = info.format.sample_rate;
        let mut dbmd_chunks = wave
            .chunks()
            .iter()
            .enumerate()
            .filter(|(_, c)| c.id == *b"dbmd");
        let dbmd_index = dbmd_chunks.next().map(|(index, _)| index);
        if dbmd_chunks.next().is_some() {
            let mut error = Error::invalid("Duplicate WAVE dbmd chunk");
            error.field = Some("dbmd");
            return Err(error);
        }
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
        let profile = if info.container == Container::Rf64 && dbmd_index.is_some() {
            Profile::Logic
        } else {
            Profile::Standard
        };
        let mut diagnostics = Vec::new();
        let ancillary = crate::ancillary::read_wave_chunks(&mut wave, |message| {
            Losses {
                policy: options.loss_policy,
                diagnostics: &mut diagnostics,
            }
            .report(None, None, "container_metadata", message)
        })?;
        let mut reader = Self {
            spec: MasterSpec {
                sample_rate: rate,
                profile,
                downmix_mode: DownmixMode::NotIndicated,
                warp_mode: WarpMode::NotIndicated,
                surround_phase_shift_present: false,
                loss_policy: options.loss_policy,
                approximate_mappings: options.approximate_mappings,
                preserve_literal: options.preserve_literal,
                ..MasterSpec::default()
            },
            path: path.to_owned(),
            channels: vec![None; BED_CHANNELS],
            updates: 0,
            timelines: Vec::new(),
            last_change: None,
            object_ends: Vec::new(),
            beds: Vec::new(),
            last_sample: None,
            frames: info.frames,
            programme_duration: None,
            diagnostics: Vec::new(),
            dbmd_channels: None,
        };
        if let Some(index) = dbmd_index {
            let bytes = wave.metadata_at(index, dbmd::DBMD_LIMIT as u64)?;
            let mut losses = Losses {
                policy: options.loss_policy,
                diagnostics: &mut diagnostics,
            };
            match crate::dbmd::read(&bytes, usize::from(info.format.channels)) {
                Ok(metadata) => {
                    for (field, message) in metadata.unsupported {
                        losses.report(None, None, field, message)?;
                    }
                    if let Some(rate) = metadata.frame_rate {
                        reader.spec.frame_rate = rate;
                    }
                    reader.spec.sc_bed_configuration = metadata.sc_bed_configuration;
                    reader.spec.downmix_mode = metadata.downmix_mode;
                    reader.spec.first_action_ns = metadata.first_action_ns;
                    reader.spec.ddplus_compression = metadata.ddplus_compression;
                    reader.spec.dbmd_source = Some(metadata.source);
                    reader.spec.surround_phase_shift = metadata.surround_phase_shift;
                    reader.spec.surround_phase_shift_present =
                        metadata.surround_phase_shift_present;
                    reader.spec.bed_distribution = metadata.bed_distribution;
                    reader.spec.bed_distribution_present = metadata.bed_distribution_present;
                    reader.spec.warp_mode = metadata.warp_mode;
                    if let Some(values) = metadata.trim_mode {
                        reader.spec.trim_mode = values;
                    }
                    reader.dbmd_channels = metadata.channels;
                }
                Err(error) if error.kind == ErrorKind::Unsupported => {
                    losses.report(None, None, "dbmd", error.message)?
                }
                Err(mut error) => {
                    error.field = Some("dbmd");
                    return Err(error);
                }
            }
        }
        reader.project(
            &Scene {
                doc: &doc,
                scene: &scene,
                index: Some(&index),
            },
            &mut Losses {
                policy: options.loss_policy,
                diagnostics: &mut diagnostics,
            },
        )?;
        if !ancillary.is_empty() || reader.spec.dbmd_source.is_some() {
            let origin = crate::ancillary::Origin {
                frames: info.frames,
                channels: usize::from(info.format.channels),
                sample_rate: rate,
                start_sample: reader.spec.start_sample,
                object_tracks: reader.spec.objects.iter().map(|o| o.id).collect(),
                unchanged: info.format.sample_format == macin_bwf::SampleFormat::Pcm24
                    && reader.channels.len() == usize::from(info.format.channels)
                    && reader
                        .channels
                        .iter()
                        .enumerate()
                        .all(|(i, c)| *c == Some(i as u32)),
            };
            if let Some(source) = &mut reader.spec.dbmd_source {
                source.origin = Some(origin.clone());
            }
            if !ancillary.is_empty() {
                let mut metadata = ContainerMetadata::new(MetadataContainer::Wave, ancillary)?;
                metadata.origin = Some(origin);
                reader.spec.container_metadata = Some(metadata);
            }
        }
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
        if let Some(node) = s.doc.element_by_id(s.text(programme.id)) {
            s.attributes(
                node,
                &[
                    "audioProgrammeID",
                    "audioProgrammeName",
                    "audioProgrammeLanguage",
                    "start",
                    "end",
                ],
                None,
                losses,
            )?;
            s.children(
                node,
                &[
                    "audioContentIDRef",
                    "loudnessMetadata",
                    "audioProgrammeReferenceScreen",
                    "audioProgrammeLabel",
                ],
                None,
                losses,
            )?;
        }
        self.spec.adm = s.metadata(
            s.doc.element_by_id(s.text(programme.id)).unwrap(),
            crate::native::Level::Programme,
            None,
            losses,
        )?;
        self.spec.start_sample = programme.start;
        if programme.end_present != 0 {
            let duration = programme
                .end
                .checked_sub(programme.start)
                .filter(|v| *v != 0)
                .ok_or_else(|| Error::invalid("Programme end must follow its start"))?;
            if duration > self.frames {
                return Err(Error::invalid("Programme duration exceeds the audio"));
            }
            self.frames = duration;
            self.programme_duration = Some(duration);
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
            if let Some(node) = s.doc.element_by_id(content_id) {
                s.attributes(
                    node,
                    &["audioContentID", "audioContentName", "audioContentLanguage"],
                    None,
                    losses,
                )?;
                s.children(
                    node,
                    &[
                        "audioObjectIDRef",
                        "dialogue",
                        "loudnessMetadata",
                        "audioContentLabel",
                    ],
                    None,
                    losses,
                )?;
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
        for content_id in s.list(programme.content_ids) {
            let content = scene
                .contents
                .iter()
                .find(|c| s.text(c.id) == content_id)
                .unwrap();
            let refs: Vec<_> = s.list(content.object_ids).collect();
            let mut members = Vec::new();
            for reference in &refs {
                if let Some((&number, _)) = singles
                    .iter()
                    .find(|(_, index)| s.text(scene.objects[**index].id) == *reference)
                {
                    let id = TrackId(u64::from(number));
                    if !members.contains(&id) {
                        members.push(id);
                    }
                }
            }
            let mut dialogue = None;
            if let Some(node) = s.doc.element_by_id(content_id) {
                for child in s.doc.element_children(node) {
                    if s.doc.element_name(child) != Some("dialogue") {
                        continue;
                    }
                    if dialogue.is_some() {
                        return Err(Error::invalid(
                            "Content has multiple dialogue classifications",
                        ));
                    }
                    let (kind, attribute) =
                        match s.doc.element_text(child).unwrap_or_default().trim() {
                            "0" => (ContentKind::NonDialogue, "nonDialogueContentKind"),
                            "1" => (ContentKind::Dialogue, "dialogueContentKind"),
                            "2" => (ContentKind::Mixed, "mixedContentKind"),
                            _ => return Err(Error::invalid("Invalid content dialogue")),
                        };
                    s.attributes(child, &[attribute], None, losses)?;
                    s.children(child, &[], None, losses)?;
                    let subtype = s
                        .doc
                        .element_attribute(child, attribute)
                        .unwrap_or("0")
                        .parse()
                        .map_err(|_| Error::invalid("Invalid content dialogue subtype"))?;
                    dialogue = Some(ContentDialogue { kind, subtype });
                }
            }
            self.spec.contents.push(ContentSpec {
                name: s.text(content.name).to_owned(),
                include_bed: bed
                    .is_some_and(|index| refs.contains(&s.text(scene.objects[index].id))),
                objects: members,
                dialogue,
                adm: s.metadata(
                    s.doc.element_by_id(content_id).unwrap(),
                    crate::native::Level::Content,
                    None,
                    losses,
                )?,
            });
        }
        if bed.is_none()
            && let Some(first) = self.spec.contents.first_mut()
        {
            // 输入无 bed 时，十个补充声道来自静音；只把这个已声明的静音 bed 放入首组。
            first.include_bed = true;
        }
        Ok(())
    }

    /// 校验对象和 pack。对象增益和静音留在对象层。
    fn object_level(
        s: &Scene<'_>,
        index: usize,
        track: Option<TrackId>,
        losses: &mut Losses<'_>,
    ) -> Result<()> {
        let object = &s.scene.objects[index];
        let node = object.node;
        s.attributes(
            node,
            &[
                "audioObjectID",
                "audioObjectName",
                "start",
                "duration",
                "importance",
                "dialogue",
                "interact",
                "disableDucking",
            ],
            track,
            losses,
        )?;
        s.pack_losses(node, track, losses)?;
        s.children(node, &OBJECT_ELEMENTS, track, losses)?;
        Ok(())
    }

    fn bed(&mut self, s: &Scene<'_>, index: usize, losses: &mut Losses<'_>) -> Result<()> {
        let scene = s.scene;
        let object = &scene.objects[index];
        let name = s.text(object.name);
        self.spec.bed_name = name.to_owned();
        self.spec.bed_adm = s.metadata(object.node, crate::native::Level::Object, None, losses)?;
        Self::object_level(s, index, None, losses)?;
        self.spec.bed_timing = Some(AdmTiming {
            start: (object.start_present != 0).then_some(object.absolute_start),
            duration: (object.duration_present != 0).then_some(object.duration),
        });
        let tracks = &scene.tracks[object.tracks.offset..object.tracks.offset + object.tracks.len];
        let object_head_present = s
            .doc
            .element_children(object.node)
            .iter()
            .any(|&node| s.doc.element_name(node) == Some("headLocked"));
        let mut bed_pack_seen = false;
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
            let labels = &first.scene.indices
                [first_block.labels.offset..first_block.labels.offset + first_block.labels.len];
            let slot = labels
                .iter()
                .map(|&i| first.scene.strings[i as usize].trim())
                .find_map(|label| {
                    BED_LABELS
                        .iter()
                        .position(|v| *v == label.strip_prefix("RC_").unwrap_or(label))
                })
                .ok_or_else(|| unsupported("bed has no recognised 7.1.2 speaker label"))?;
            let label = BED_LABELS[slot];
            let source_channel = u32::try_from(track.channel)
                .ok()
                .and_then(|n| n.checked_sub(1))
                .ok_or_else(|| {
                    Error::invalid(format!("bed track {} is not in chna", s.text(track.uid)))
                })?;
            if self.channels[slot].replace(source_channel).is_some() {
                return Err(Error::invalid(format!("bed channel {label} appears twice")));
            }
            self.spec.bed_channel_names[slot] = s.channel_names(
                object.node,
                track.uid,
                metadata::default_bed_channel_name(slot),
                None,
                losses,
            )?;
            self.spec.bed_channel_adm[slot] = s.channel_metadata(track.uid, None, losses)?;
            if let Some(pack) = s.track_pack(object.node, track.uid) {
                let metadata = s.metadata(pack, crate::native::Level::Pack, None, losses)?;
                if bed_pack_seen && self.spec.bed_pack_adm != metadata {
                    losses.report(
                        None,
                        None,
                        "pack_structure",
                        "Bed tracks use different pack metadata".into(),
                    )?;
                } else {
                    self.spec.bed_pack_adm = metadata;
                    bed_pack_seen = true;
                }
            }
            let private = self
                .dbmd_channels
                .as_ref()
                .and_then(|v| v.get(source_channel as usize))
                .copied();
            let timeline = BedTimeline {
                slot,
                channels,
                start: object.absolute_start,
                locked: object.head_locked,
                object_head: object_head_present,
                private,
                updates: 0,
            };
            let first = timeline.project(&first, losses)?;
            self.spec.bed[slot] = BedPatch {
                gain: first.gain,
                importance: first.importance.map(|i| f64::from(i) / 10.),
                head_tracking: Some(first.head_tracking),
                binaural_mode: private.map(|v| v.binaural_mode),
                trim_bypass: private.map(|v| v.trim_bypass),
                trim_bypass_unspecified: private.is_some_and(|v| v.trim_bypass_unspecified),
                ..Default::default()
            }
            .apply(self.spec.bed[slot]);
            self.spec.bed_geometry[slot] = first.geometry.clone();
            self.spec.bed_blocks[slot] = Some(first);
            if let Some(name) = s
                .track_pack(object.node, track.uid)
                .and_then(|p| s.doc.element_attribute(p, "audioPackFormatName"))
            {
                name.clone_into(&mut self.spec.bed_pack_name);
            }
            let mut timeline = timeline;
            while let Some(item) = blocks.next()? {
                timeline.project(&item, losses)?;
                timeline.updates += 1;
            }
            self.beds.push(timeline);
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
        Self::object_level(s, index, Some(id), losses)?;
        let track = &scene.tracks[object.tracks.offset];
        s.channel_losses(track.uid, Some(id), losses)?;
        let channels = s.channels(track.uid, self.spec.sample_rate)?;
        let end = channel_end(&channels, object.absolute_start)
            .min(object.end)
            .min(self.programme_duration.unwrap_or(u64::MAX));
        if end != u64::MAX {
            self.object_ends.push((id, end));
        }
        let source_channel = number
            .checked_sub(1)
            .ok_or_else(|| Error::invalid("Object track index must be positive"))?;
        let private = self
            .dbmd_channels
            .as_ref()
            .and_then(|v| v.get(usize::from(source_channel)))
            .copied();
        let mut base =
            private.map_or_else(ObjectState::default, |v| v.apply(ObjectState::default()));
        if s.doc
            .element_children(object.node)
            .iter()
            .any(|&node| s.doc.element_name(node) == Some("headLocked"))
        {
            base.head_tracking = resolve_head(
                head(object.head_locked),
                private.map(|v| v.head_tracking),
                losses,
                Some(id),
                None,
                "head_tracking",
            )?;
        }
        let timeline = ObjectTimeline {
            id,
            channels,
            start: object.absolute_start,
            locked: object.head_locked,
            base,
            dbmd_head: private.map(|v| v.head_tracking),
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
        let initial_zones = events.zones.clone();
        while let Some(NativeObjectUpdate { event: update, .. }) = events.next_event(losses)? {
            self.updates += 1;
            self.last_sample = self.last_sample.max(Some(update.sample));
            if update.patch.active != Some(false) {
                self.last_change = self.last_change.max(Some(update.sample));
            }
        }
        self.timelines.push(timeline);
        let mut spec = ObjectSpec::new(id, name);
        spec.pack_name = s
            .track_pack(object.node, track.uid)
            .and_then(|node| s.doc.element_attribute(node, "audioPackFormatName"))
            .filter(|name| *name != spec.name)
            .map(str::to_owned);
        spec.channel_names =
            s.channel_names(object.node, track.uid, &spec.name, Some(id), losses)?;
        spec.adm = s.metadata(object.node, crate::native::Level::Object, Some(id), losses)?;
        if let Some(pack) = s.track_pack(object.node, track.uid) {
            spec.pack_adm = s.metadata(pack, crate::native::Level::Pack, Some(id), losses)?;
        }
        spec.channel_adm = s.channel_metadata(track.uid, Some(id), losses)?;
        spec.initial = initial;
        spec.custom_zones = initial_zones;
        self.spec.objects.push(spec);
        self.channels.push(Some(u32::from(number) - 1));
        Ok(())
    }

    /// 显式覆盖输出帧率；有可识别 DBMD 时默认使用其帧率，否则取 24 fps。
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
    /// 读取阶段无法保留的字段；原生字段的跨格式近似与损失在目标写出报告中。
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
        Ok(Updates(self.native_updates_for_frames(frames)?))
    }
    pub fn native_updates(&self) -> Result<NativeUpdates> {
        self.native_updates_for_frames(self.frames)
    }
    pub fn native_updates_for_frames(&self, frames: u64) -> Result<NativeUpdates> {
        let mut updates = NativeUpdates {
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
    pub fn bed_update_count(&self) -> u64 {
        self.beds.iter().map(|b| b.updates).sum()
    }
    /// 每路首块位于 spec.bed_blocks，迭代器交付后续所有原生块。
    pub fn bed_blocks(&self) -> BedBlocks {
        BedBlocks {
            path: self.path.clone(),
            tracks: self.beds.clone().into_iter(),
            rate: self.spec.sample_rate,
            policy: self.spec.loss_policy,
            current: None,
            failed: false,
        }
    }
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
        Ok(self.wave_audio(&self.path)?.with_frame_limit(self.frames))
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
        .map(|audio| audio.with_origin(path))
    }
    /// Write the master to every target in one pass through [`MultiWriter`].
    pub fn convert(&self, audio: AudioSource, targets: &[Target]) -> Result<Conversion> {
        let original_audio = audio.originates_at(&self.path);
        let mut audio = audio.remap(self.spec.sample_rate, &self.channels, "ADM track index")?;
        let frames = audio.frames();
        let mut diagnostics = self.diagnostics.clone();
        let mut spec = self.spec.clone();
        if !original_audio {
            let mut losses = Losses {
                policy: spec.loss_policy,
                diagnostics: &mut diagnostics,
            };
            let replacement = audio.ancillary_metadata(&spec, |message| {
                losses.report(None, None, "container_metadata", message)
            })?;
            if spec.container_metadata.take().is_some() || replacement.is_some() {
                losses.report(
                    None,
                    None,
                    "container_metadata",
                    "Replacement audio invalidates the source and replacement container metadata"
                        .into(),
                )?;
            }
        }
        if !original_audio || frames != self.frames {
            if spec.dbmd_source.as_ref().is_some_and(|s| s.extra_encoding) {
                Losses {
                    policy: spec.loss_policy,
                    diagnostics: &mut diagnostics,
                }
                .report(
                    None,
                    None,
                    "dbmd_encoding",
                    "Replacing or trimming audio invalidates the original DBMD encoding data"
                        .into(),
                )?;
                spec.dbmd_source = None;
            }
            let has_loudness =
                spec.adm.has_loudness() || spec.contents.iter().any(|c| c.adm.has_loudness());
            if has_loudness {
                Losses {
                    policy: spec.loss_policy,
                    diagnostics: &mut diagnostics,
                }
                .report(
                    None,
                    None,
                    "loudness",
                    "Replacing or trimming audio invalidates source loudness metadata".into(),
                )?;
                spec.adm.clear_loudness();
                for content in &mut spec.contents {
                    content.adm.clear_loudness();
                }
            }
        }
        let bed_origin = spec.bed_timing.and_then(|t| t.start).unwrap_or(0);
        bed::clip_spec(&mut spec, bed_origin, self.programme_duration, frames)?;
        let bed_blocks = self.bed_blocks().filter_map(|block| {
            block
                .and_then(|mut b| {
                    Ok(
                        b.clip_to_audio(bed_origin, self.programme_duration, frames)?
                            .then_some(b),
                    )
                })
                .transpose()
        });
        let updates = self.native_updates_for_frames(frames)?;
        let last_sample = self.last_change.max(
            self.object_ends
                .iter()
                .filter(|(_, end)| *end < frames)
                .map(|(_, end)| *end)
                .max(),
        );
        write_conversion(
            &spec,
            updates,
            std::iter::empty::<Result<BedUpdate>>(),
            bed_blocks,
            audio,
            last_sample,
            &diagnostics,
            targets,
        )
    }
}
