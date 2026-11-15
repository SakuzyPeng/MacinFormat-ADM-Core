//! DAMF reading and conversion (feature `read`).
//!
//! A DAMF master is read into the same model `MasterWriter` takes, a [`MasterSpec`] plus a
//! stream of [`ObjectUpdate`]s, so converting it is ordinary writing: every profile's loss
//! checks apply unchanged. What the model cannot hold is reported while reading, under the same
//! [`LossPolicy`]: `Reject` fails with the field named, `Allow` drops it with a
//! [`MappingDiagnostic`].
//!
//! The metadata file is scanned twice and never held in memory. The first pass validates the
//! whole timeline and folds each object's leading sample-zero events into its initial state; the
//! second, [`DamfReader::updates`], yields the remaining events in file order.
use crate::yaml::{self, Node};
use crate::*;
use macin_bwf::Reader;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const MANIFEST_LIMIT: u64 = 16 << 20;
const UPDATE_BATCH: usize = 4096;
const AUDIO_BLOCK: usize = 4800;

/// A DAMF master opened for reading. See the module documentation.
#[derive(Debug, Clone)]
pub struct DamfReader {
    spec: MasterSpec,
    metadata: PathBuf,
    audio: PathBuf,
    /// The DAMF ID feeding each output channel: ten bed slots, then the objects. A bed slot is
    /// `None` when the master has no bed, and is then written as silence.
    channels: Vec<Option<u32>>,
    beds: BTreeMap<u32, usize>,
    /// Per object ID, how many leading events were folded into the initial state.
    folded: BTreeMap<u32, usize>,
    updates: u64,
    last_sample: Option<u64>,
    creation_tool: Option<String>,
    diagnostics: Vec<MappingDiagnostic>,
}

/// The result of [`DamfReader::convert`]: what reading dropped, then one report per target.
#[derive(Debug, Clone)]
pub struct Conversion {
    pub read: Vec<MappingDiagnostic>,
    pub outputs: Vec<WriteReport>,
}

fn located(path: &Path, error: Error) -> Error {
    let mut error = error;
    error.message = format!("{}: {}", path.display(), error.message);
    error
}
fn unsupported(line: usize, message: impl std::fmt::Display) -> Error {
    Error::new(ErrorKind::Unsupported, format!("line {line}: {message}"))
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

fn get<'n>(entries: &'n [(String, Node)], key: &str) -> Option<&'n Node> {
    entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}
fn require<'n>(entries: &'n [(String, Node)], key: &str, line: usize) -> Result<&'n Node> {
    get(entries, key).ok_or_else(|| yaml::invalid(line, format!("missing {key}")))
}
fn unknown<'n>(entries: &'n [(String, Node)], known: &[&str]) -> impl Iterator<Item = &'n str> {
    entries
        .iter()
        .map(|(k, _)| k.as_str())
        .filter(move |k| !known.contains(k))
}
fn id(node: &Node, what: &str) -> Result<u32> {
    u32::try_from(node.as_u64(what)?)
        .map_err(|_| yaml::invalid(node.line(), format!("{what} is too large")))
}

/// Does a manifest field differ from the fixed value the writer emits?
type Differs = fn(&Node) -> Result<bool>;

/// One parsed `.atmos.metadata` event.
struct RawEvent {
    line: usize,
    id: u32,
    sample: u64,
    patch: ObjectPatch,
    ramp: Option<u64>,
    unknown: Vec<String>,
}

const EVENT_FIELDS: [&str; 20] = [
    "ID",
    "samplePos",
    "active",
    "pos",
    "snap",
    "elevation",
    "zones",
    "size",
    "size3D",
    "decorr",
    "importance",
    "gain",
    "rampLength",
    "trimBypass",
    "dialog",
    "music",
    "screenFactor",
    "depthFactor",
    "headTrackMode",
    "binauralRenderMode",
];

fn classification(node: &Node, what: &str) -> Result<i8> {
    match node.as_i64(what)? {
        v @ -1..=1 => Ok(v as i8),
        _ => Err(yaml::invalid(
            node.line(),
            format!("{what} must be -1, 0 or 1"),
        )),
    }
}

impl RawEvent {
    fn parse(node: &Node, previous_id: Option<u32>) -> Result<Self> {
        let entries = node.as_mapping("event")?;
        let line = node.line();
        let mut patch = ObjectPatch::default();
        for (key, value) in entries {
            match key.as_str() {
                "active" => patch.active = Some(value.as_bool(key)?),
                "pos" => patch.position = Some(value.as_vector(key)?),
                "snap" => patch.snap = Some(value.as_bool(key)?),
                "elevation" => patch.elevation = Some(value.as_bool(key)?),
                "zones" => {
                    patch.zone = Some(match value.as_text(key)? {
                        "all" => Zone::All,
                        "no back" => Zone::NoBack,
                        "no sides" => Zone::NoSides,
                        "center back" => Zone::CenterBack,
                        "screen only" => Zone::ScreenOnly,
                        "surround only" => Zone::SurroundOnly,
                        other => {
                            return Err(yaml::invalid(
                                value.line(),
                                format!("unknown zones {other:?}"),
                            ));
                        }
                    });
                }
                "size" => {
                    if get(entries, "size3D").is_some() {
                        return Err(yaml::invalid(line, "an event has either size or size3D"));
                    }
                    let v = value.as_f64(key)?;
                    // The writer spells "no extent" as size 0, so 0 reads back as no extent.
                    patch.extent = Some((v != 0.).then_some(Extent::Uniform(v)));
                }
                "size3D" => patch.extent = Some(Some(Extent::Cartesian(value.as_vector(key)?))),
                "decorr" => patch.decorrelation = Some(value.as_f64(key)?),
                "importance" => patch.importance = Some(value.as_f64(key)?),
                "gain" => {
                    let v = value.as_f64(key)?;
                    patch.gain = Some(if v == f64::NEG_INFINITY {
                        Gain::Silence
                    } else {
                        Gain::Decibels(v)
                    });
                }
                "trimBypass" => patch.trim_bypass = Some(value.as_bool(key)?),
                "dialog" => patch.dialog = Some(classification(value, key)?),
                "music" => patch.music = Some(classification(value, key)?),
                "screenFactor" => patch.screen_factor = Some(value.as_f64(key)?),
                "depthFactor" => patch.depth_factor = Some(value.as_f64(key)?),
                "headTrackMode" => {
                    patch.head_tracking = Some(match value.as_text(key)? {
                        "scene relative" => HeadTracking::SceneRelative,
                        "head relative" => HeadTracking::HeadRelative,
                        other => {
                            return Err(yaml::invalid(
                                value.line(),
                                format!("unknown headTrackMode {other:?}"),
                            ));
                        }
                    });
                }
                "binauralRenderMode" => {
                    patch.binaural_mode = Some(match value.as_text(key)? {
                        "undefined" => BinauralMode::Undefined,
                        "off" => BinauralMode::Off,
                        "near" => BinauralMode::Near,
                        "mid" => BinauralMode::Mid,
                        "far" => BinauralMode::Far,
                        other => {
                            return Err(yaml::invalid(
                                value.line(),
                                format!("unknown binauralRenderMode {other:?}"),
                            ));
                        }
                    });
                }
                _ => {}
            }
        }
        Ok(Self {
            line,
            id: match get(entries, "ID") {
                Some(value) => id(value, "ID")?,
                None => previous_id.ok_or_else(|| yaml::invalid(line, "missing ID"))?,
            },
            sample: require(entries, "samplePos", line)?.as_u64("samplePos")?,
            ramp: get(entries, "rampLength")
                .map(|n| n.as_u64("rampLength"))
                .transpose()?,
            patch,
            unknown: unknown(entries, &EVENT_FIELDS).map(str::to_owned).collect(),
        })
    }
    /// The model's ramp: continuous updates always carry one (DAMF's default is 0), discrete
    /// ones never do. A nonzero ramp on a discrete-only event has no mapping.
    fn ramp(&self) -> (Option<u64>, bool) {
        if self.patch.has_continuous() {
            (Some(self.ramp.unwrap_or(0)), false)
        } else {
            (None, self.ramp.is_some_and(|r| r != 0))
        }
    }
}

/// Fields a bed channel may carry, and the only values the fixed bed can reproduce.
fn bed_losses(patch: &ObjectPatch, lfe: bool) -> [(&'static str, bool); 16] {
    let expected_mode = if lfe {
        BinauralMode::Off
    } else {
        BinauralMode::Undefined
    };
    [
        ("bed_active", patch.active == Some(false)),
        (
            "bed_gain",
            patch.gain.is_some_and(|g| g.decibels() != Some(0.)),
        ),
        ("bed_importance", patch.importance.is_some_and(|v| v != 1.)),
        ("bed_trim_bypass", patch.trim_bypass == Some(true)),
        (
            "bed_head_tracking",
            patch
                .head_tracking
                .is_some_and(|m| m != HeadTracking::SceneRelative),
        ),
        (
            "bed_binaural_mode",
            patch.binaural_mode.is_some_and(|m| m != expected_mode),
        ),
        ("bed_position", patch.position.is_some()),
        ("bed_extent", patch.extent.is_some_and(|e| e.is_some())),
        ("bed_zone", patch.zone.is_some_and(|z| z != Zone::All)),
        ("bed_snap", patch.snap == Some(true)),
        ("bed_elevation", patch.elevation == Some(false)),
        (
            "bed_decorrelation",
            patch.decorrelation.is_some_and(|v| v != 0.),
        ),
        ("bed_dialog", patch.dialog.is_some_and(|v| v != -1)),
        ("bed_music", patch.music.is_some_and(|v| v != -1)),
        (
            "bed_screen_factor",
            patch.screen_factor.is_some_and(|v| v != 0.),
        ),
        (
            "bed_depth_factor",
            patch.depth_factor.is_some_and(|v| v != 0.),
        ),
    ]
}

/// Walks `.atmos.metadata`: `sampleRate` and the `events` sequence, one event at a time.
struct MetadataScan<R: std::io::BufRead + 'static> {
    stream: yaml::FileStream<R>,
    path: PathBuf,
    state: ScanState,
    keys: BTreeSet<String>,
    sample_rate: Option<u64>,
    previous_id: Option<u32>,
    unknown: Vec<String>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScanState {
    Keys,
    Events,
    Done,
}
fn open_metadata(path: &Path) -> Result<MetadataScan<BufReader<File>>> {
    let file = File::open(path).map_err(|e| located(path, e.into()))?;
    let mut scan = MetadataScan {
        stream: yaml::from_reader(BufReader::with_capacity(1 << 16, file)),
        path: path.to_owned(),
        state: ScanState::Keys,
        keys: BTreeSet::new(),
        sample_rate: None,
        previous_id: None,
        unknown: Vec::new(),
    };
    scan.start().map_err(|e| located(path, e))?;
    Ok(scan)
}
impl<R: std::io::BufRead + 'static> MetadataScan<R> {
    fn start(&mut self) -> Result<()> {
        self.stream.begin()?;
        let (event, span) = self.stream.next()?;
        if !matches!(event, saphyr_parser::Event::MappingStart(0, None)) {
            return Err(yaml::invalid(
                span.start.line(),
                "metadata must be a mapping",
            ));
        }
        Ok(())
    }
    /// The next event, resolving an omitted ID from the preceding event in either pass.
    fn next_event(&mut self) -> Result<Option<RawEvent>> {
        self.step().map_err(|e| located(&self.path, e))
    }
    fn step(&mut self) -> Result<Option<RawEvent>> {
        loop {
            match self.state {
                ScanState::Done => return Ok(None),
                ScanState::Events => {
                    let (event, span) = self.stream.next()?;
                    if matches!(event, saphyr_parser::Event::SequenceEnd) {
                        self.state = ScanState::Keys;
                        continue;
                    }
                    let node = self.stream.node(event, span, 1)?;
                    let event = RawEvent::parse(&node, self.previous_id)?;
                    self.previous_id = Some(event.id);
                    return Ok(Some(event));
                }
                ScanState::Keys => {
                    let Some((key, line)) = self.stream.key()? else {
                        self.stream.end()?;
                        self.state = ScanState::Done;
                        return Ok(None);
                    };
                    if !self.keys.insert(key.clone()) {
                        return Err(yaml::invalid(line, format!("duplicate key {key:?}")));
                    }
                    let (event, span) = self.stream.next()?;
                    match key.as_str() {
                        "events"
                            if matches!(event, saphyr_parser::Event::SequenceStart(0, None)) =>
                        {
                            self.state = ScanState::Events;
                        }
                        "events" => return Err(yaml::invalid(line, "events must be a sequence")),
                        "sampleRate" => {
                            self.sample_rate =
                                Some(self.stream.node(event, span, 1)?.as_u64("sampleRate")?);
                        }
                        _ => {
                            self.stream.node(event, span, 1)?;
                            self.unknown.push(key);
                        }
                    }
                }
            }
        }
    }
}

/// Per-object state during the validating pass.
struct Pass {
    state: ObjectState,
    /// The state after the folded leading events: the object's initial state.
    initial: ObjectState,
    folding: bool,
    folded: usize,
    last: u64,
}

impl DamfReader {
    /// Read and validate a DAMF master from its `.atmos` manifest. `policy` governs both what
    /// reading may drop and, through the spec, what writing may drop.
    pub fn open(manifest: impl AsRef<Path>, policy: LossPolicy) -> Result<Self> {
        let manifest = manifest.as_ref();
        let text = File::open(manifest)
            .map_err(Error::from)
            .and_then(|f| yaml::read_text(f, MANIFEST_LIMIT, "DAMF manifest"))
            .map_err(|e| located(manifest, e))?;
        let mut diagnostics = Vec::new();
        let mut reader =
            Self::from_manifest(manifest, &text, policy, &mut diagnostics).map_err(|e| {
                if e.track.is_some() {
                    e
                } else {
                    located(manifest, e)
                }
            })?;
        reader.scan(&mut diagnostics)?;
        reader.diagnostics = diagnostics;
        Ok(reader)
    }

    fn from_manifest(
        manifest: &Path,
        text: &str,
        policy: LossPolicy,
        diagnostics: &mut Vec<MappingDiagnostic>,
    ) -> Result<Self> {
        let mut losses = Losses {
            policy,
            diagnostics,
        };
        let root = yaml::parse_document(text)?;
        let top = root.as_mapping("manifest")?;
        for key in unknown(top, &["version", "presentations"]) {
            losses.report(
                None,
                None,
                "unknown_field",
                format!("Unknown manifest field {key:?}"),
            )?;
        }
        let version = require(top, "version", root.line())?;
        let version_text = version.as_text("version")?;
        let mut parts = version_text.split('.');
        match (parts.next(), parts.next()) {
            (Some("0"), Some("5" | "6")) => {}
            _ => {
                return Err(unsupported(
                    version.line(),
                    format!("DAMF version {version_text} is not supported (0.5 or 0.6)"),
                ));
            }
        }
        let presentations = require(top, "presentations", root.line())?;
        let [presentation] = presentations.as_sequence("presentations")? else {
            return Err(unsupported(
                presentations.line(),
                "only single-presentation DAMF is supported",
            ));
        };
        let line = presentation.line();
        let p = presentation.as_mapping("presentation")?;
        let kind = require(p, "type", line)?;
        let profile = match kind.as_text("type")? {
            "home" => Profile::DamfHome,
            "3dof" => Profile::DamfThreeDof,
            other => {
                return Err(unsupported(
                    kind.line(),
                    format!("presentation type {other:?} is not supported"),
                ));
            }
        };
        const KNOWN: [&str; 17] = [
            "type",
            "simplified",
            "metadata",
            "audio",
            "offset",
            "fps",
            "scBedConfiguration",
            "creationTool",
            "creationToolVersion",
            "downmixType_5to2",
            "51-to-20_LsRs90degPhaseShift",
            "warpMode",
            "screenSizeRatio",
            "bedDistribution",
            "trimMode",
            "bedInstances",
            "objects",
        ];
        for key in unknown(p, &KNOWN) {
            losses.report(
                None,
                None,
                "unknown_field",
                format!("Unknown presentation field {key:?}"),
            )?;
        }
        let mut spec = MasterSpec {
            profile,
            loss_policy: policy,
            ..MasterSpec::default()
        };
        if let Some(stem) = manifest
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.strip_suffix(".atmos").unwrap_or(n))
            .filter(|stem| !stem.is_empty())
        {
            spec.stem = stem.to_owned();
        }
        let fixed: [(&str, &'static str, Differs); 5] = [
            ("simplified", "simplified", |n| n.as_bool("simplified")),
            ("offset", "offset", |n| Ok(n.as_f64("offset")? != 0.)),
            ("scBedConfiguration", "sc_bed_configuration", |n| {
                Ok(
                    !matches!(n.as_sequence("scBedConfiguration")?, [v] if v.as_u64("scBedConfiguration")? == 3),
                )
            }),
            ("downmixType_5to2", "downmix_type", |n| {
                Ok(n.as_text("downmixType_5to2")? != "LoRo_Stereo")
            }),
            ("51-to-20_LsRs90degPhaseShift", "ls_rs_phase_shift", |n| {
                n.as_bool("51-to-20_LsRs90degPhaseShift")
            }),
        ];
        for (key, field, differs) in fixed {
            if let Some(node) = get(p, key)
                && differs(node)?
            {
                losses.report(
                    None,
                    None,
                    field,
                    format!("{key} differs from the fixed value the writer emits"),
                )?;
            }
        }
        if let Some(node) = get(p, "trimMode") {
            let custom = node
                .as_mapping("trimMode")?
                .iter()
                .any(|(_, v)| !matches!(v, Node::Mapping(e, _) if e.is_empty()));
            if custom {
                losses.report(
                    None,
                    None,
                    "trim_mode",
                    "Custom trimMode settings have no mapping".into(),
                )?;
            }
        }
        if let Some(node) = get(p, "fps") {
            spec.frame_rate = match node.as_text("fps")? {
                "23.976" => FrameRate::Fps23976,
                "24" => FrameRate::Fps24,
                "25" => FrameRate::Fps25,
                "29.97" => FrameRate::Fps2997,
                "29.97df" => FrameRate::Fps2997Drop,
                "30" => FrameRate::Fps30,
                other => {
                    return Err(unsupported(
                        node.line(),
                        format!("fps {other:?} is not supported"),
                    ));
                }
            };
        }
        if let Some(node) = get(p, "warpMode") {
            spec.warp_mode = match node.as_text("warpMode")? {
                "None" => WarpMode::None,
                "LoRo" => WarpMode::LoRo,
                other => {
                    return Err(yaml::invalid(
                        node.line(),
                        format!("unknown warpMode {other:?}"),
                    ));
                }
            };
        }
        if let Some(node) = get(p, "screenSizeRatio") {
            spec.screen_size_ratio = node.as_f64("screenSizeRatio")?;
        }
        if let Some(node) = get(p, "bedDistribution") {
            spec.bed_distribution = node.as_bool("bedDistribution")?;
        }
        let creation_tool = get(p, "creationTool")
            .map(|n| n.as_text("creationTool").map(str::to_owned))
            .transpose()?;

        let mut ids = BTreeMap::new();
        let mut claim = |node: &Node, what: &'static str| -> Result<u32> {
            let value = id(node, "ID")?;
            if let Some(previous) = ids.insert(value, what) {
                return Err(yaml::invalid(
                    node.line(),
                    format!("ID {value} is both a {previous} and a {what}"),
                ));
            }
            Ok(value)
        };
        let mut channels: Vec<Option<u32>> = vec![None; BED_CHANNELS];
        let mut beds = BTreeMap::new();
        let instances = match get(p, "bedInstances") {
            Some(node) => node.as_sequence("bedInstances")?,
            None => &[],
        };
        match instances {
            [] => {}
            [bed] => {
                let b = bed.as_mapping("bed instance")?;
                for key in unknown(b, &["description", "channels"]) {
                    losses.report(
                        None,
                        None,
                        "unknown_field",
                        format!("Unknown bed field {key:?}"),
                    )?;
                }
                if let Some(name) = get(b, "description") {
                    spec.bed_name = name.as_text("description")?.to_owned();
                }
                let list = require(b, "channels", bed.line())?;
                for item in list.as_sequence("channels")? {
                    let c = item.as_mapping("bed channel")?;
                    let label = require(c, "channel", item.line())?.as_text("channel")?;
                    let slot = BED_LABELS.iter().position(|l| *l == label).ok_or_else(|| {
                        unsupported(
                            item.line(),
                            format!("bed channel {label:?} is not part of a 7.1.2 bed"),
                        )
                    })?;
                    if channels[slot].is_some() {
                        return Err(yaml::invalid(
                            item.line(),
                            format!("bed channel {label} appears twice"),
                        ));
                    }
                    let value = claim(require(c, "ID", item.line())?, "bed channel")?;
                    for key in unknown(c, &["channel", "ID"]) {
                        losses.report(
                            Some(TrackId(u64::from(value))),
                            None,
                            "unknown_field",
                            format!("Unknown bed channel {label} field {key:?}"),
                        )?;
                    }
                    channels[slot] = Some(value);
                    beds.insert(value, slot);
                }
                if channels.iter().any(Option::is_none) {
                    return Err(unsupported(
                        list.line(),
                        "only a complete 7.1.2 bed is supported",
                    ));
                }
            }
            _ => return Err(unsupported(line, "multi-bed DAMF is not supported")),
        }
        let objects = match get(p, "objects") {
            Some(node) => node.as_sequence("objects")?,
            None => &[],
        };
        let mut declared = Vec::new();
        for item in objects {
            let o = item.as_mapping("object")?;
            for key in unknown(o, &["description", "ID"]) {
                losses.report(
                    None,
                    None,
                    "unknown_field",
                    format!("Unknown object field {key:?}"),
                )?;
            }
            let value = claim(require(o, "ID", item.line())?, "object")?;
            let name = get(o, "description")
                .map(|n| n.as_text("description"))
                .transpose()?
                .unwrap_or_default();
            declared.push((value, name.to_owned()));
        }
        // Output order is by ID, which is also the order of the source's audio channels.
        declared.sort_by_key(|(value, _)| *value);
        for (value, name) in declared {
            channels.push(Some(value));
            spec.objects
                .push(ObjectSpec::new(TrackId(u64::from(value)), name));
        }
        let base = manifest.parent().unwrap_or(Path::new(""));
        let member =
            |key: &str| -> Result<PathBuf> { Ok(base.join(require(p, key, line)?.as_text(key)?)) };
        let metadata = member("metadata")?;
        let audio = member("audio")?;
        Ok(Self {
            spec,
            metadata,
            audio,
            channels,
            beds,
            folded: BTreeMap::new(),
            updates: 0,
            last_sample: None,
            creation_tool,
            diagnostics: Vec::new(),
        })
    }

    /// The validating pass over the metadata.
    fn scan(&mut self, diagnostics: &mut Vec<MappingDiagnostic>) -> Result<()> {
        let mut losses = Losses {
            policy: self.spec.loss_policy,
            diagnostics,
        };
        let mut passes: BTreeMap<u32, Pass> = self
            .spec
            .objects
            .iter()
            .map(|o| {
                (
                    o.id.0 as u32,
                    Pass {
                        state: ObjectState::default(),
                        initial: ObjectState::default(),
                        folding: true,
                        folded: 0,
                        last: 0,
                    },
                )
            })
            .collect();
        let mut bed_last: BTreeMap<u32, u64> = BTreeMap::new();
        let mut scan = open_metadata(&self.metadata)?;
        let path = self.metadata.clone();
        let at = |e: Error| {
            if e.track.is_some() {
                e
            } else {
                located(&path, e)
            }
        };
        while let Some(event) = scan.next_event()? {
            let track = TrackId(u64::from(event.id));
            for key in &event.unknown {
                losses.report(
                    Some(track),
                    Some(event.sample),
                    "unknown_field",
                    format!("Unknown event field {key:?}"),
                )?;
            }
            self.last_sample = self.last_sample.max(Some(event.sample));
            if let Some(&slot) = self.beds.get(&event.id) {
                let last = bed_last.entry(event.id).or_default();
                if event.sample < *last {
                    return Err(at(yaml::invalid(
                        event.line,
                        format!("events for ID {} go backwards", event.id),
                    )));
                }
                *last = event.sample;
                for (field, lost) in bed_losses(&event.patch, BED_LABELS[slot] == "LFE") {
                    if lost {
                        losses.report(None, Some(event.sample), field, format!("Bed channel {} carries {field} that the fixed bed cannot reproduce", BED_LABELS[slot]))?;
                    }
                }
                continue;
            }
            let pass = passes.get_mut(&event.id).ok_or_else(|| {
                at(yaml::invalid(
                    event.line,
                    format!(
                        "event for ID {} which the manifest does not declare",
                        event.id
                    ),
                ))
            })?;
            if event.sample < pass.last {
                return Err(at(yaml::invalid(
                    event.line,
                    format!("events for ID {} go backwards", event.id),
                )));
            }
            pass.last = event.sample;
            let (ramp, ramp_lost) = event.ramp();
            if ramp_lost {
                losses.report(
                    Some(track),
                    Some(event.sample),
                    "ramp",
                    "rampLength on an event without position, gain or size has no mapping".into(),
                )?;
            }
            pass.state = event.patch.apply(pass.state);
            pass.state
                .validate()
                .map_err(|e| e.context(track, event.sample, "state"))?;
            if pass.folding && event.sample == 0 && ramp.unwrap_or(0) == 0 {
                pass.folded += 1;
                pass.initial = pass.state;
                continue;
            }
            if pass.folding {
                pass.folding = false;
                if pass.folded == 0 {
                    losses.report(Some(track), Some(event.sample), "initial_state", "Object has no event at sample 0; the default state is assumed before its first event".into())?;
                }
            }
            self.updates += 1;
        }
        let rate = scan
            .sample_rate
            .ok_or_else(|| located(&self.metadata, Error::invalid("metadata has no sampleRate")))?;
        self.spec.sample_rate = u32::try_from(rate)
            .map_err(|_| located(&self.metadata, Error::invalid("sampleRate is too large")))?;
        for key in std::mem::take(&mut scan.unknown) {
            losses.report(
                None,
                None,
                "unknown_field",
                format!("Unknown metadata field {key:?}"),
            )?;
        }
        for object in &mut self.spec.objects {
            let pass = &passes[&(object.id.0 as u32)];
            if pass.folded == 0 && pass.folding {
                losses.report(
                    Some(object.id),
                    None,
                    "initial_state",
                    "Object has no events; the default state is assumed".into(),
                )?;
            }
            object.initial = pass.initial;
            self.folded.insert(object.id.0 as u32, pass.folded);
        }
        self.spec.validate()?;
        Ok(())
    }

    /// The model: bed and objects in output order, initial states, and the presentation fields
    /// the profiles carry. `profile` is the source's (home or 3dof).
    pub fn spec(&self) -> &MasterSpec {
        &self.spec
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
    /// What reading dropped under `LossPolicy::Allow`.
    pub fn diagnostics(&self) -> &[MappingDiagnostic] {
        &self.diagnostics
    }
    /// The source's `creationTool`, if any. Conversion writes its own creator, not this one.
    pub fn creation_tool(&self) -> Option<&str> {
        self.creation_tool.as_deref()
    }
    /// How many updates [`Self::updates`] yields.
    pub fn update_count(&self) -> u64 {
        self.updates
    }
    /// The latest `samplePos` in the metadata; the audio must be longer than this.
    pub fn last_event_sample(&self) -> Option<u64> {
        self.last_sample
    }
    pub fn metadata_path(&self) -> &Path {
        &self.metadata
    }
    pub fn audio_path(&self) -> &Path {
        &self.audio
    }

    /// The updates after the initial states, in file order (per object, nondecreasing time).
    pub fn updates(&self) -> Result<Updates> {
        Ok(Updates {
            scan: open_metadata(&self.metadata)?,
            skip: self.folded.clone(),
            beds: self.beds.keys().copied().collect(),
        })
    }

    /// The source's own `.atmos.audio`.
    pub fn audio(&self) -> Result<AudioSource> {
        let caf = CafReader::open(&self.audio)?;
        AudioSource::new(
            self.spec.sample_rate,
            &self.channels,
            "DAMF ID",
            Input::Caf(caf),
        )
    }
    /// A WAV file in place of the source's audio, with channel `n` carrying DAMF ID `n`.
    /// 16- and 24-bit PCM convert exactly; 32-bit PCM and float are quantised to 24 bits, and
    /// float samples outside ±1 are an error.
    pub fn wave_audio(&self, path: impl AsRef<Path>) -> Result<AudioSource> {
        let path = path.as_ref();
        let file = File::open(path).map_err(|e| located(path, e.into()))?;
        let wave = Reader::new(BufReader::new(file)).map_err(|e| located(path, e.into()))?;
        AudioSource::new(
            self.spec.sample_rate,
            &self.channels,
            "DAMF ID",
            Input::Wave(wave),
        )
    }

    /// 通过 [`MultiWriter`] 一次写出所有目标，DAMF 目标保留来源的 stem。
    /// 转换从音频起点重新读取，并按当前 reader 核对采样率、重建声道映射。
    pub fn convert(&self, audio: AudioSource, targets: &[Target]) -> Result<Conversion> {
        let audio = AudioSource::new(
            self.spec.sample_rate,
            &self.channels,
            "DAMF ID",
            audio.input,
        )?;
        write_conversion(
            &self.spec,
            self.updates()?,
            audio,
            self.last_sample,
            &self.diagnostics,
            targets,
        )
    }
}

/// The conversion both readers share: check the audio covers the metadata, rewind it, schedule
/// every update, then stream the audio through one [`MultiWriter`].
pub(crate) fn write_conversion(
    spec: &MasterSpec,
    updates: impl IntoIterator<Item = Result<ObjectUpdate>>,
    mut audio: AudioSource,
    last_sample: Option<u64>,
    read: &[MappingDiagnostic],
    targets: &[Target],
) -> Result<Conversion> {
    if audio.frames == 0 {
        return Err(Error::invalid("The audio is empty"));
    }
    if let Some(last) = last_sample.filter(|s| *s >= audio.frames) {
        return Err(Error::invalid(format!(
            "Metadata reaches sample {last} but the audio has {} frames",
            audio.frames
        )));
    }
    audio.rewind()?;
    let mut writer = MultiWriter::create(spec, targets)?;
    let mut batch = Vec::with_capacity(UPDATE_BATCH);
    for update in updates {
        batch.push(update?);
        if batch.len() == UPDATE_BATCH {
            writer.schedule_updates(&batch)?;
            batch.clear();
        }
    }
    writer.schedule_updates(&batch)?;
    let mut pcm = Vec::new();
    while audio.read_s24(AUDIO_BLOCK, &mut pcm)? > 0 {
        writer.write_block(PcmBlock::S24Le(&pcm), &[])?;
    }
    Ok(Conversion {
        read: read.to_vec(),
        outputs: writer.finish()?,
    })
}

/// The second pass. Each item is one [`ObjectUpdate`], ready for `schedule_updates`.
pub struct Updates {
    scan: MetadataScan<BufReader<File>>,
    skip: BTreeMap<u32, usize>,
    beds: Vec<u32>,
}
impl std::fmt::Debug for Updates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Updates")
            .field("path", &self.scan.path)
            .finish_non_exhaustive()
    }
}
impl Iterator for Updates {
    type Item = Result<ObjectUpdate>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let event = match self.scan.next_event() {
                Ok(Some(event)) => event,
                Ok(None) => return None,
                Err(e) => return Some(Err(e)),
            };
            if self.beds.contains(&event.id) {
                continue;
            }
            if let Some(left) = self.skip.get_mut(&event.id).filter(|n| **n > 0) {
                *left -= 1;
                continue;
            }
            return Some(Ok(ObjectUpdate {
                track: TrackId(u64::from(event.id)),
                sample: event.sample,
                ramp_samples: event.ramp().0,
                patch: event.patch,
            }));
        }
    }
}

/// Signed packed S24LE CAF, as DAMF stores its audio. Other chunks are skipped.
#[derive(Debug)]
pub struct CafReader {
    source: BufReader<File>,
    data_start: u64,
    channels: usize,
    sample_rate: u32,
    frames: u64,
    position: u64,
}
fn be<const N: usize>(source: &mut impl Read) -> Result<[u8; N]> {
    let mut bytes = [0; N];
    source.read_exact(&mut bytes)?;
    Ok(bytes)
}
impl CafReader {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        Self::read_header(path).map_err(|e| located(path, e))
    }
    fn read_header(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let length = file.metadata()?.len();
        let mut source = BufReader::new(file);
        let unsupported = |m: &str| Error::new(ErrorKind::Unsupported, m);
        if &be::<4>(&mut source)? != b"caff" || be::<4>(&mut source)? != [0, 1, 0, 0] {
            return Err(unsupported("not a version 1 CAF file"));
        }
        let mut at = 8u64;
        let mut format = None;
        loop {
            if at == length {
                return Err(Error::invalid("CAF has no data chunk"));
            }
            let kind = be::<4>(&mut source)?;
            let size = i64::from_be_bytes(be(&mut source)?);
            at += 12;
            if &kind == b"desc" {
                if size != 32 || format.is_some() {
                    return Err(Error::invalid("invalid CAF desc chunk"));
                }
                let rate = f64::from_be_bytes(be(&mut source)?);
                let id = be::<4>(&mut source)?;
                let fields: [u32; 5] = [
                    u32::from_be_bytes(be(&mut source)?),
                    u32::from_be_bytes(be(&mut source)?),
                    u32::from_be_bytes(be(&mut source)?),
                    u32::from_be_bytes(be(&mut source)?),
                    u32::from_be_bytes(be(&mut source)?),
                ];
                let [flags, bytes_per_packet, frames_per_packet, channels, bits] = fields;
                // Integer (not float), little endian, 24 bits packed: what DAMF stores.
                if &id != b"lpcm" || flags != 2 || bits != 24 || frames_per_packet != 1 {
                    return Err(unsupported(
                        "DAMF audio must be signed 24-bit little-endian PCM",
                    ));
                }
                if channels == 0 || channels.checked_mul(3) != Some(bytes_per_packet) {
                    return Err(Error::invalid("invalid CAF channel layout"));
                }
                if rate.fract() != 0. || !(1.0..=f64::from(u32::MAX)).contains(&rate) {
                    return Err(unsupported("CAF sample rate must be a whole number"));
                }
                format = Some((rate as u32, channels as usize));
                at += 32;
            } else if &kind == b"data" {
                let (sample_rate, channels) =
                    format.ok_or_else(|| Error::invalid("CAF data precedes desc"))?;
                let size = if size == -1 {
                    length
                        .checked_sub(at)
                        .ok_or_else(|| Error::invalid("CAF data is truncated"))?
                } else {
                    u64::try_from(size).map_err(|_| Error::invalid("invalid CAF data size"))?
                };
                if size < 4 || at + size > length {
                    return Err(Error::invalid("CAF data is truncated"));
                }
                source.seek_relative(4)?;
                let bytes = size - 4;
                let align = channels as u64 * 3;
                if bytes % align != 0 {
                    return Err(Error::invalid("CAF data is not whole frames"));
                }
                return Ok(Self {
                    source,
                    data_start: at + 4,
                    channels,
                    sample_rate,
                    frames: bytes / align,
                    position: 0,
                });
            } else {
                let size =
                    u64::try_from(size).map_err(|_| Error::invalid("invalid CAF chunk size"))?;
                if at + size > length {
                    return Err(Error::invalid("CAF chunk extends past the file"));
                }
                source.seek(SeekFrom::Current(i64::try_from(size).unwrap_or(i64::MAX)))?;
                at += size;
            }
        }
    }
    pub fn channels(&self) -> usize {
        self.channels
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn frames(&self) -> u64 {
        self.frames
    }
    /// Read up to `frames` frames of interleaved S24LE into `out`; returns the frames read.
    pub fn read(&mut self, frames: usize, out: &mut Vec<u8>) -> Result<usize> {
        let frames = frames.min(usize::try_from(self.frames - self.position).unwrap_or(usize::MAX));
        out.resize(frames * self.channels * 3, 0);
        self.source.read_exact(out)?;
        self.position += frames as u64;
        Ok(frames)
    }
}

pub(crate) enum Input {
    Caf(CafReader),
    Wave(Reader<BufReader<File>>),
}
impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Caf(caf) => f.debug_tuple("Caf").field(caf).finish(),
            Self::Wave(wave) => f.debug_tuple("Wave").field(&wave.info()).finish(),
        }
    }
}

/// Audio for conversion, delivered in the reader's output channel order as S24LE.
#[derive(Debug)]
pub struct AudioSource {
    pub(crate) input: Input,
    /// Source channel per output channel; `None` is a silent bed slot.
    map: Vec<Option<usize>>,
    source_channels: usize,
    frames: u64,
    raw: Vec<u8>,
    float: Vec<f32>,
    mapped: Vec<f32>,
}
impl AudioSource {
    /// `map` gives the source channel (0-based) feeding each output channel; `None` is silence.
    /// `what` names a source channel in the error for one that is missing ("DAMF ID", say).
    pub(crate) fn new(
        sample_rate: u32,
        map: &[Option<u32>],
        what: &str,
        input: Input,
    ) -> Result<Self> {
        let (rate, channels, frames) = match &input {
            Input::Caf(caf) => (caf.sample_rate, caf.channels, caf.frames),
            Input::Wave(wave) => {
                let info = wave.info();
                (
                    info.format.sample_rate,
                    usize::from(info.format.channels),
                    info.frames,
                )
            }
        };
        if rate != sample_rate {
            return Err(Error::invalid(format!(
                "Audio is {rate} Hz but the metadata is {sample_rate} Hz"
            )));
        }
        let map: Vec<Option<usize>> = map.iter().map(|c| c.map(|id| id as usize)).collect();
        if let Some(highest) = map.iter().flatten().max()
            && *highest >= channels
        {
            return Err(Error::invalid(format!(
                "{what} {highest} needs audio channel {highest}, but the audio has {channels} channels"
            )));
        }
        Ok(Self {
            input,
            map,
            source_channels: channels,
            frames,
            raw: Vec::new(),
            float: Vec::new(),
            mapped: Vec::new(),
        })
    }
    pub fn frames(&self) -> u64 {
        self.frames
    }
    fn rewind(&mut self) -> Result<()> {
        match &mut self.input {
            Input::Caf(caf) => {
                caf.source.seek(SeekFrom::Start(caf.data_start))?;
                caf.position = 0;
            }
            Input::Wave(wave) => {
                wave.seek_frame(0)?;
            }
        }
        Ok(())
    }
    /// Read up to `frames` frames into `out`; returns how many were read (0 at the end).
    pub fn read_s24(&mut self, frames: usize, out: &mut Vec<u8>) -> Result<usize> {
        out.clear();
        let width = self.map.len();
        match &mut self.input {
            Input::Caf(caf) => {
                let read = caf.read(frames, &mut self.raw)?;
                out.reserve(read * width * 3);
                for frame in self.raw.chunks_exact(self.source_channels * 3) {
                    for source in &self.map {
                        match source {
                            Some(c) => out.extend_from_slice(&frame[c * 3..c * 3 + 3]),
                            None => out.extend_from_slice(&[0; 3]),
                        }
                    }
                }
                Ok(read)
            }
            Input::Wave(wave) => {
                self.float.resize(frames * self.source_channels, 0.);
                let read = usize::try_from(wave.read_frames(&mut self.float)?).unwrap_or(0);
                self.mapped.clear();
                for frame in
                    self.float[..read * self.source_channels].chunks_exact(self.source_channels)
                {
                    self.mapped.extend(
                        self.map
                            .iter()
                            .map(|source| source.map_or(0., |c| frame[c])),
                    );
                }
                let pcm = PcmBlock::F32Interleaved(&self.mapped);
                crate::writer::validate_pcm(pcm, width)
                    .map_err(|_| Error::invalid("WAV samples must be finite and within ±1"))?;
                crate::writer::quantize(pcm, width, 0..read, out);
                Ok(read)
            }
        }
    }
}
