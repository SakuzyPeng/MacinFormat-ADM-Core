use crate::{Error, Result};
use std::collections::BTreeSet;

pub const BED_CHANNELS: usize = 10;
pub const MAX_OBJECTS: usize = 118;
pub const BED_LABELS: [&str; 10] = [
    "L", "R", "C", "LFE", "Lss", "Rss", "Lrs", "Rrs", "Lts", "Rts",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrackId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Profile {
    #[default]
    Standard,
    Logic,
    DamfHome,
    DamfThreeDof,
}
impl Profile {
    pub fn is_adm(self) -> bool {
        matches!(self, Self::Standard | Self::Logic)
    }
    pub fn container_id(self) -> &'static [u8; 4] {
        if self == Self::Logic {
            b"RF64"
        } else {
            b"BW64"
        }
    }
    pub fn clock_units_per_second(self) -> u64 {
        if self == Self::Logic {
            100_000
        } else {
            1_000_000_000
        }
    }
    pub fn clock_fraction_digits(self) -> usize {
        if self == Self::Logic { 5 } else { 9 }
    }
    pub fn damf_version(self) -> &'static str {
        if self == Self::DamfThreeDof {
            "0.6.0"
        } else {
            "0.5.1"
        }
    }
    pub fn damf_type(self) -> &'static str {
        if self == Self::DamfThreeDof {
            "3dof"
        } else {
            "home"
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrameRate {
    Fps23976,
    #[default]
    Fps24,
    Fps25,
    Fps2997,
    Fps2997Drop,
    Fps30,
}
impl FrameRate {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fps23976 => "23.976",
            Self::Fps24 => "24",
            Self::Fps25 => "25",
            Self::Fps2997 => "29.97",
            Self::Fps2997Drop => "29.97df",
            Self::Fps30 => "30",
        }
    }
    pub fn dbmd_code(self) -> u8 {
        match self {
            Self::Fps23976 => 0x21,
            Self::Fps24 => 0x22,
            Self::Fps25 => 0x23,
            Self::Fps2997 => 0x25,
            Self::Fps2997Drop => 0x24,
            Self::Fps30 => 0x26,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LossPolicy {
    #[default]
    Reject,
    Allow,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gain {
    Linear(f64),
    Decibels(f64),
    Silence,
}
impl Default for Gain {
    fn default() -> Self {
        Self::Linear(1.0)
    }
}
impl Gain {
    pub fn linear(self) -> f64 {
        match self {
            Self::Linear(v) => v,
            Self::Decibels(v) => libm::pow(10.0, v / 20.0),
            Self::Silence => 0.0,
        }
    }
    pub fn decibels(self) -> Option<f64> {
        match self {
            Self::Silence | Self::Linear(0.0) => None,
            Self::Decibels(v) => Some(v),
            Self::Linear(v) => Some(20.0 * libm::log10(v)),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Extent {
    Uniform(f64),
    Cartesian([f64; 3]),
}
impl Extent {
    pub fn xyz(self) -> [f64; 3] {
        match self {
            Self::Uniform(v) => [v; 3],
            Self::Cartesian(v) => v,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeadTracking {
    #[default]
    SceneRelative,
    HeadRelative,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BinauralMode {
    #[default]
    Undefined,
    Off,
    Near,
    Mid,
    Far,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Zone {
    #[default]
    All,
    NoBack,
    NoSides,
    CenterBack,
    ScreenOnly,
    SurroundOnly,
}
impl Zone {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::NoBack => "no back",
            Self::NoSides => "no sides",
            Self::CenterBack => "center back",
            Self::ScreenOnly => "screen only",
            Self::SurroundOnly => "surround only",
        }
    }
}

/// Cartesian room coordinates: X right, Y front, Z up, each in -1..=1.
/// Gains are metadata, and are not applied to the PCM submitted to the writer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectState {
    pub active: bool,
    pub position: [f64; 3],
    pub gain: Gain,
    pub extent: Option<Extent>,
    /// Normalized priority 0..=1; ADM represents its discrete tenths.
    pub importance: f64,
    pub snap: bool,
    pub elevation: bool,
    pub zone: Zone,
    pub screen_ref: bool,
    pub screen_factor: f64,
    pub depth_factor: f64,
    pub decorrelation: f64,
    pub trim_bypass: bool,
    /// DAMF classification: -1 unspecified, 0 false, 1 true.
    pub dialog: i8,
    pub music: i8,
    pub head_tracking: HeadTracking,
    pub binaural_mode: BinauralMode,
}
impl Default for ObjectState {
    fn default() -> Self {
        Self {
            active: true,
            position: [0.; 3],
            gain: Gain::default(),
            extent: None,
            importance: 1.,
            snap: false,
            elevation: true,
            zone: Zone::All,
            screen_ref: false,
            screen_factor: 0.,
            depth_factor: 0.,
            decorrelation: 0.,
            trim_bypass: false,
            dialog: -1,
            music: -1,
            head_tracking: HeadTracking::SceneRelative,
            binaural_mode: BinauralMode::Undefined,
        }
    }
}
impl ObjectState {
    pub fn validate(&self) -> Result<()> {
        if self
            .position
            .iter()
            .any(|v| !v.is_finite() || !(-1.0..=1.0).contains(v))
        {
            return Err(Error::invalid(
                "Object coordinates must be finite and within -1..=1",
            ));
        }
        if !self.gain.linear().is_finite() || self.gain.linear() < 0.0 {
            return Err(Error::invalid("Object gain must be finite and nonnegative"));
        }
        if let Gain::Decibels(v) = self.gain
            && !v.is_finite()
        {
            return Err(Error::invalid(
                "Use Gain::Silence instead of an infinite decibel value",
            ));
        }
        if self.extent.is_some_and(|v| {
            v.xyz()
                .iter()
                .any(|x| !x.is_finite() || !(0.0..=1.0).contains(x))
        }) {
            return Err(Error::invalid(
                "Object extent must be finite and within 0..=1",
            ));
        }
        if [self.importance, self.screen_factor, self.decorrelation]
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            || !self.depth_factor.is_finite()
            || !(0.0..=2.0).contains(&self.depth_factor)
            || !(-1..=1).contains(&self.dialog)
            || !(-1..=1).contains(&self.music)
        {
            return Err(Error::invalid("Invalid object rendering property"));
        }
        Ok(())
    }
}

/// Absent fields inherit their values. A headphone-only patch does not restart a motion ramp.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ObjectPatch {
    pub active: Option<bool>,
    pub position: Option<[f64; 3]>,
    pub gain: Option<Gain>,
    pub extent: Option<Option<Extent>>,
    pub importance: Option<f64>,
    pub snap: Option<bool>,
    pub elevation: Option<bool>,
    pub zone: Option<Zone>,
    pub screen_ref: Option<bool>,
    pub screen_factor: Option<f64>,
    pub depth_factor: Option<f64>,
    pub decorrelation: Option<f64>,
    pub trim_bypass: Option<bool>,
    pub dialog: Option<i8>,
    pub music: Option<i8>,
    pub head_tracking: Option<HeadTracking>,
    pub binaural_mode: Option<BinauralMode>,
}
impl ObjectPatch {
    pub fn full(s: ObjectState) -> Self {
        Self {
            active: Some(s.active),
            position: Some(s.position),
            gain: Some(s.gain),
            extent: Some(s.extent),
            importance: Some(s.importance),
            snap: Some(s.snap),
            elevation: Some(s.elevation),
            zone: Some(s.zone),
            screen_ref: Some(s.screen_ref),
            screen_factor: Some(s.screen_factor),
            depth_factor: Some(s.depth_factor),
            decorrelation: Some(s.decorrelation),
            trim_bypass: Some(s.trim_bypass),
            dialog: Some(s.dialog),
            music: Some(s.music),
            head_tracking: Some(s.head_tracking),
            binaural_mode: Some(s.binaural_mode),
        }
    }
    pub fn apply(self, mut s: ObjectState) -> ObjectState {
        macro_rules! fields { ($($field:ident),*) => { $(if let Some(value) = self.$field { s.$field = value; })* }; }
        fields!(
            active,
            position,
            gain,
            extent,
            importance,
            snap,
            elevation,
            zone,
            screen_ref,
            screen_factor,
            depth_factor,
            decorrelation,
            trim_bypass,
            dialog,
            music,
            head_tracking,
            binaural_mode
        );
        s
    }
    pub fn has_continuous(self) -> bool {
        self.position.is_some() || self.gain.is_some() || self.extent.is_some()
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectUpdate {
    pub track: TrackId,
    pub sample: u64,
    /// Some starts a new continuous-property ramp. None preserves the current ramp.
    pub ramp_samples: Option<u64>,
    pub patch: ObjectPatch,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectSpec {
    pub id: TrackId,
    pub name: String,
    pub initial: ObjectState,
}
impl ObjectSpec {
    pub fn new(id: TrackId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            initial: ObjectState::default(),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WarpMode {
    None,
    #[default]
    LoRo,
}
impl WarpMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::LoRo => "LoRo",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MasterSpec {
    pub name: String,
    pub bed_name: String,
    pub bed_pack_name: String,
    pub sample_rate: u32,
    pub profile: Profile,
    pub frame_rate: FrameRate,
    pub loss_policy: LossPolicy,
    pub objects: Vec<ObjectSpec>,
    pub creation_tool: String,
    pub creation_tool_version: String,
    /// Logic DBMD's fixed-width ASCII originator (32) and product (64) fields.
    pub dbmd_originator: String,
    pub dbmd_product: String,
    pub screen_size_ratio: f64,
    pub bed_distribution: bool,
    pub warp_mode: WarpMode,
    /// Safe single filename component used for all three DAMF members.
    pub stem: String,
}
impl Default for MasterSpec {
    fn default() -> Self {
        Self {
            name: "Atmos_Master".into(),
            bed_name: "7.1.2 bed".into(),
            bed_pack_name: "7.1.2 bed".into(),
            sample_rate: 48_000,
            profile: Profile::Standard,
            frame_rate: FrameRate::Fps24,
            loss_policy: LossPolicy::Reject,
            objects: Vec::new(),
            creation_tool: "MacinFormat-ADM-Core".into(),
            creation_tool_version: env!("CARGO_PKG_VERSION").into(),
            dbmd_originator: "Created by MacinFormat".into(),
            dbmd_product: "MacinFormat ADM Core".into(),
            screen_size_ratio: 1.,
            bed_distribution: false,
            warp_mode: WarpMode::LoRo,
            stem: "master".into(),
        }
    }
}
impl MasterSpec {
    pub fn channels(&self) -> usize {
        BED_CHANNELS + self.objects.len()
    }
    pub fn validate(&self) -> Result<()> {
        if !matches!(self.sample_rate, 48_000 | 96_000) {
            return Err(Error::invalid("Atmos sample rate must be 48000 or 96000"));
        }
        if self.objects.len() > MAX_OBJECTS {
            return Err(Error::invalid("Atmos master exceeds 118 objects"));
        }
        let mut ids = BTreeSet::new();
        for object in &self.objects {
            if !ids.insert(object.id) {
                return Err(Error::invalid("Duplicate TrackId"));
            }
            valid_text(&object.name)?;
            object.initial.validate()?;
        }
        for text in [
            &self.name,
            &self.bed_name,
            &self.bed_pack_name,
            &self.creation_tool,
            &self.creation_tool_version,
        ] {
            valid_text(text)?;
        }
        // DAMF members append extensions, so a stem may end in a dot or space.
        if self.stem.is_empty()
            || matches!(self.stem.as_str(), "." | "..")
            || self.stem.contains(['/', '\\', ':'])
            || self.stem.chars().any(char::is_control)
        {
            return Err(Error::invalid(
                "DAMF stem must be a safe single filename component",
            ));
        }
        if !self.screen_size_ratio.is_finite() || self.screen_size_ratio <= 0. {
            return Err(Error::invalid(
                "Screen size ratio must be finite and positive",
            ));
        }
        if self.profile == Profile::Logic {
            for (s, len) in [(&self.dbmd_originator, 32), (&self.dbmd_product, 64)] {
                if !s.is_ascii() || s.len() > len || s.chars().any(char::is_control) {
                    return Err(Error::invalid(
                        "Logic DBMD creator field exceeds its ASCII width",
                    ));
                }
            }
        }
        Ok(())
    }
}
fn valid_text(value: &str) -> Result<()> {
    if value.chars().any(|c| {
        (c < '\u{20}' && !matches!(c, '\t' | '\n' | '\r')) || matches!(c, '\u{fffe}' | '\u{ffff}')
    }) {
        return Err(Error::invalid("Text contains an XML-invalid character"));
    }
    Ok(())
}

/// Audio in physical file order: ten bed channels followed by the declared objects.
#[derive(Debug, Clone, Copy)]
pub enum PcmBlock<'a> {
    F32Interleaved(&'a [f32]),
    F32Planar(&'a [&'a [f32]]),
    S24Le(&'a [u8]),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingDiagnostic {
    pub track: Option<TrackId>,
    pub sample: Option<u64>,
    pub field: &'static str,
    pub message: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackMapping {
    pub id: TrackId,
    pub track_index: u16,
    pub audio_object_id: String,
}
#[derive(Debug, Clone)]
pub struct WriteReport {
    pub frames: u64,
    pub channels: u16,
    pub sample_rate: u32,
    pub pcm_bytes: u64,
    pub axml_bytes: u64,
    pub dbmd_bytes: usize,
    pub objects: Vec<TrackMapping>,
    pub diagnostics: Vec<MappingDiagnostic>,
}
