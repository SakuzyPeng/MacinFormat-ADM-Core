use crate::{
    AdmBedBlock, AdmMetadata, AdmTiming, ContainerMetadata, Error, ExclusionZone,
    HeadphoneVirtualise, ObjectDivergence, Result, ScreenEdge, TrackExtensions,
};
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

/// 调用方逐项授权的近似映射；不表示两种参数的渲染语义等价。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approximation {
    DiffuseDecorrelation,
    ScreenRefFactor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ApproximateMappings {
    /// diffuse 与 decorr 数值直传，时间点保留为离散更新。
    pub diffuse_decorrelation: bool,
    /// screenRef 映射为 0/1；非零 screenFactor 映射为 true。
    pub screen_ref_factor: bool,
}
impl ApproximateMappings {
    pub fn enable(&mut self, approximation: Approximation) {
        match approximation {
            Approximation::DiffuseDecorrelation => self.diffuse_decorrelation = true,
            Approximation::ScreenRefFactor => self.screen_ref_factor = true,
        }
    }
}

/// 以原文取值写出存在互操作限制的字段，和近似映射互相独立。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LiteralPreservation {
    pub cartesian_azimuth_range: bool,
    pub damf_mid: bool,
}
/// 已选择的近似可在 Reject 下执行；其余损失仍遵循 loss_policy。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MappingOptions {
    pub loss_policy: LossPolicy,
    pub approximate_mappings: ApproximateMappings,
    pub preserve_literal: LiteralPreservation,
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
    Undefined,
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
    /// 原生 ADM 扩散度；与 DAMF decorrelation 分开保存。
    pub diffuse: f64,
    pub divergence: Option<ObjectDivergence>,
    pub headphone_virtualise: Option<HeadphoneVirtualise>,
    pub channel_lock_distance: Option<f64>,
    /// X/Z 轴分别接受 left/right 与 top/bottom；Y 不允许锁定。
    pub screen_edge_lock: [Option<ScreenEdge>; 3],
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
            diffuse: 0.,
            divergence: None,
            headphone_virtualise: None,
            channel_lock_distance: None,
            screen_edge_lock: [None; 3],
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
        if [
            self.importance,
            self.screen_factor,
            self.decorrelation,
            self.diffuse,
        ]
        .iter()
        .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            || !self.depth_factor.is_finite()
            || !(0.0..=2.0).contains(&self.depth_factor)
            || !(-1..=1).contains(&self.dialog)
            || !(-1..=1).contains(&self.music)
        {
            return Err(Error::invalid("Invalid object rendering property"));
        }
        if let Some(value) = self.headphone_virtualise {
            value.validate()?;
        }
        if let Some(value) = self.divergence {
            value.validate()?;
        }
        if let Some(value) = self.channel_lock_distance {
            crate::native::range(value, 0., 2. * libm::sqrt(3.))?;
        }
        for (axis, edge) in self.screen_edge_lock.iter().enumerate() {
            if let Some(edge) = edge
                && !matches!(
                    (axis, edge),
                    (0, ScreenEdge::Left | ScreenEdge::Right)
                        | (2, ScreenEdge::Top | ScreenEdge::Bottom)
                )
            {
                return Err(Error::invalid(
                    "screenEdgeLock does not match its coordinate",
                ));
            }
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
    pub diffuse: Option<f64>,
    pub divergence: Option<Option<ObjectDivergence>>,
    pub headphone_virtualise: Option<Option<HeadphoneVirtualise>>,
    pub channel_lock_distance: Option<Option<f64>>,
    pub screen_edge_lock: Option<[Option<ScreenEdge>; 3]>,
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
            diffuse: Some(s.diffuse),
            divergence: Some(s.divergence),
            headphone_virtualise: Some(s.headphone_virtualise),
            channel_lock_distance: Some(s.channel_lock_distance),
            screen_edge_lock: Some(s.screen_edge_lock),
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
            diffuse,
            divergence,
            headphone_virtualise,
            channel_lock_distance,
            screen_edge_lock,
            trim_bypass,
            dialog,
            music,
            head_tracking,
            binaural_mode
        );
        s
    }
    pub fn has_continuous(self) -> bool {
        self.position.is_some()
            || self.gain.is_some()
            || self.extent.is_some()
            || self.diffuse.is_some()
            || self.divergence.is_some()
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
    pub pack_name: Option<String>,
    pub channel_names: ChannelNames,
    pub adm: AdmMetadata,
    pub pack_adm: AdmMetadata,
    pub channel_adm: AdmMetadata,
    pub custom_zones: Vec<ExclusionZone>,
}
impl ObjectSpec {
    pub fn new(id: TrackId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            initial: ObjectState::default(),
            pack_name: None,
            channel_names: ChannelNames::default(),
            adm: AdmMetadata::default(),
            pack_adm: AdmMetadata::default(),
            channel_adm: AdmMetadata::default(),
            custom_zones: Vec::new(),
        }
    }
}
/// 未指定时从对象名或固定 bed 名称生成；用于 ADM 同格式保留独立名称。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChannelNames {
    pub channel: Option<String>,
    pub stream: Option<String>,
    pub track: Option<String>,
    pub extensions: TrackExtensions,
}
impl ChannelNames {
    fn validate(&self) -> Result<()> {
        self.extensions.validate()?;
        for text in [&self.channel, &self.stream, &self.track]
            .into_iter()
            .flatten()
        {
            valid_text(text)?;
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WarpMode {
    #[default]
    LoRo,
    ProLogicIIx,
    Warping,
    Normal,
}
impl WarpMode {
    /// 旧 API 的无 warp 名称；输出使用已验证的 normal 拼写。
    #[allow(non_upper_case_globals)]
    pub const None: Self = Self::Normal;

    pub fn as_str(self) -> &'static str {
        match self {
            Self::LoRo => "LoRo",
            Self::ProLogicIIx => "ProLogicIIx",
            Self::Warping => "warping",
            Self::Normal => "normal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DownmixMode {
    #[default]
    LoRo,
    LtRtProLogic,
    LtRtPlII,
}
impl DownmixMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LoRo => "LoRo_Stereo",
            Self::LtRtProLogic => "LtRt_ProLogic",
            Self::LtRtPlII => "LtRt_PLII",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimConfiguration {
    SomeSurroundsNoHeights,
    SomeSurroundsSomeHeights,
    SomeSurroundsManyHeights,
    ManySurroundsNoHeights,
}
impl TrimConfiguration {
    pub const ALL: [Self; 4] = [
        Self::SomeSurroundsNoHeights,
        Self::SomeSurroundsSomeHeights,
        Self::SomeSurroundsManyHeights,
        Self::ManySurroundsNoHeights,
    ];
    pub fn index(self) -> usize {
        match self {
            Self::SomeSurroundsNoHeights => 0,
            Self::SomeSurroundsSomeHeights => 1,
            Self::SomeSurroundsManyHeights => 2,
            Self::ManySurroundsNoHeights => 3,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SomeSurroundsNoHeights => "SomeSurroundsNoHeights",
            Self::SomeSurroundsSomeHeights => "SomeSurroundsSomeHeights",
            Self::SomeSurroundsManyHeights => "SomeSurroundsManyHeights",
            Self::ManySurroundsNoHeights => "ManySurroundsNoHeights",
        }
    }
    pub(crate) fn dbmd_index(self) -> usize {
        [1, 4, 7, 2][self.index()]
    }
}

/// 手动 trim；None 配置表示自动模式，Some(default) 表示显式空的手动配置。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TrimValues {
    pub center: f64,
    pub surround: f64,
    pub height: f64,
    pub front_back_overhead_floor: f64,
    pub front_back_listener: f64,
}
impl TrimValues {
    pub fn validate(self) -> Result<()> {
        if [self.center, self.surround, self.height]
            .iter()
            .any(|v| !v.is_finite() || !(*v as f32).is_finite())
            || [self.front_back_overhead_floor, self.front_back_listener]
                .iter()
                .any(|v| !v.is_finite() || !(-1. ..=1.).contains(v))
        {
            return Err(Error::invalid("Invalid trim values"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StaticChannelMetadata {
    pub head_tracking: HeadTracking,
    pub binaural_mode: BinauralMode,
    pub trim_bypass: bool,
}
impl From<ObjectState> for StaticChannelMetadata {
    fn from(state: ObjectState) -> Self {
        Self {
            head_tracking: state.head_tracking,
            binaural_mode: state.binaural_mode,
            trim_bypass: state.trim_bypass,
        }
    }
}
impl StaticChannelMetadata {
    pub(crate) fn apply(self, mut state: ObjectState) -> ObjectState {
        state.head_tracking = self.head_tracking;
        state.binaural_mode = self.binaural_mode;
        state.trim_bypass = self.trim_bypass;
        state
    }
}

/// DAMF bed 上显式出现的渲染字段。None 保持缺省或继承，不人为给其他声道写入这些字段。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BedRendering {
    pub position: Option<[f64; 3]>,
    pub extent: Option<Option<Extent>>,
    pub snap: Option<bool>,
    pub zone: Option<Zone>,
    pub elevation: Option<bool>,
    pub decorrelation: Option<f64>,
    pub dialog: Option<i8>,
    pub music: Option<i8>,
    pub screen_factor: Option<f64>,
    pub depth_factor: Option<f64>,
}
impl BedRendering {
    #[cfg(feature = "read")]
    pub(crate) fn from_patch(p: ObjectPatch) -> Self {
        Self {
            position: p.position,
            extent: p.extent,
            snap: p.snap,
            zone: p.zone,
            elevation: p.elevation,
            decorrelation: p.decorrelation,
            dialog: p.dialog,
            music: p.music,
            screen_factor: p.screen_factor,
            depth_factor: p.depth_factor,
        }
    }
    fn merged(mut self, changes: Self) -> Self {
        if changes.position.is_some() {
            self.position = changes.position;
        }
        if changes.extent.is_some() {
            self.extent = changes.extent;
        }
        if changes.snap.is_some() {
            self.snap = changes.snap;
        }
        if changes.zone.is_some() {
            self.zone = changes.zone;
        }
        if changes.elevation.is_some() {
            self.elevation = changes.elevation;
        }
        if changes.decorrelation.is_some() {
            self.decorrelation = changes.decorrelation;
        }
        if changes.dialog.is_some() {
            self.dialog = changes.dialog;
        }
        if changes.music.is_some() {
            self.music = changes.music;
        }
        if changes.screen_factor.is_some() {
            self.screen_factor = changes.screen_factor;
        }
        if changes.depth_factor.is_some() {
            self.depth_factor = changes.depth_factor;
        }
        self
    }
    fn properties(self) -> ObjectPatch {
        ObjectPatch {
            position: self.position,
            extent: self.extent,
            snap: self.snap,
            zone: self.zone,
            elevation: self.elevation,
            decorrelation: self.decorrelation,
            dialog: self.dialog,
            music: self.music,
            screen_factor: self.screen_factor,
            depth_factor: self.depth_factor,
            ..Default::default()
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BedState {
    pub active: bool,
    pub rendering: BedRendering,
    pub gain: Gain,
    pub importance: f64,
    pub head_tracking: HeadTracking,
    pub binaural_mode: BinauralMode,
    pub trim_bypass: bool,
}
impl Default for BedState {
    fn default() -> Self {
        Self {
            active: true,
            rendering: Default::default(),
            gain: Gain::default(),
            importance: 1.,
            head_tracking: HeadTracking::SceneRelative,
            binaural_mode: BinauralMode::Undefined,
            trim_bypass: false,
        }
    }
}
impl BedState {
    pub fn for_channel(channel: usize) -> Self {
        Self {
            binaural_mode: if channel == 3 {
                BinauralMode::Off
            } else {
                BinauralMode::Undefined
            },
            ..Self::default()
        }
    }
    pub fn validate(self) -> Result<()> {
        self.object_state().validate()
    }
    pub(crate) fn object_state(self) -> ObjectState {
        self.rendering.properties().apply(ObjectState {
            active: self.active,
            gain: self.gain,
            importance: self.importance,
            head_tracking: self.head_tracking,
            binaural_mode: self.binaural_mode,
            trim_bypass: self.trim_bypass,
            ..ObjectState::default()
        })
    }
}
impl From<BedState> for StaticChannelMetadata {
    fn from(state: BedState) -> Self {
        Self {
            head_tracking: state.head_tracking,
            binaural_mode: state.binaural_mode,
            trim_bypass: state.trim_bypass,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BedPatch {
    pub active: Option<bool>,
    pub rendering: BedRendering,
    pub gain: Option<Gain>,
    pub importance: Option<f64>,
    pub head_tracking: Option<HeadTracking>,
    pub binaural_mode: Option<BinauralMode>,
    pub trim_bypass: Option<bool>,
}
impl BedPatch {
    pub fn full(state: BedState) -> Self {
        Self {
            active: Some(state.active),
            rendering: state.rendering,
            gain: Some(state.gain),
            importance: Some(state.importance),
            head_tracking: Some(state.head_tracking),
            binaural_mode: Some(state.binaural_mode),
            trim_bypass: Some(state.trim_bypass),
        }
    }
    pub fn apply(self, mut state: BedState) -> BedState {
        macro_rules! fields { ($($field:ident),*) => { $(if let Some(value) = self.$field { state.$field = value; })* }; }
        fields!(
            active,
            gain,
            importance,
            head_tracking,
            binaural_mode,
            trim_bypass
        );
        state.rendering = state.rendering.merged(self.rendering);
        state
    }
    pub fn has_continuous(self) -> bool {
        self.gain.is_some() || self.rendering.position.is_some() || self.rendering.extent.is_some()
    }
    pub(crate) fn object_patch(self) -> ObjectPatch {
        ObjectPatch {
            active: self.active,
            gain: self.gain,
            importance: self.importance,
            head_tracking: self.head_tracking,
            binaural_mode: self.binaural_mode,
            trim_bypass: self.trim_bypass,
            ..self.rendering.properties()
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BedUpdate {
    /// BED_LABELS 中从 0 开始的声道位置。
    pub channel: usize,
    pub sample: u64,
    pub ramp_samples: Option<u64>,
    pub patch: BedPatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    NonDialogue,
    Dialogue,
    Mixed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentDialogue {
    pub kind: ContentKind,
    pub subtype: u8,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentSpec {
    pub name: String,
    pub include_bed: bool,
    pub objects: Vec<TrackId>,
    pub dialogue: Option<ContentDialogue>,
    pub adm: AdmMetadata,
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
    pub approximate_mappings: ApproximateMappings,
    pub preserve_literal: LiteralPreservation,
    pub objects: Vec<ObjectSpec>,
    pub bed: [BedState; BED_CHANNELS],
    pub bed_channel_names: [ChannelNames; BED_CHANNELS],
    pub adm: AdmMetadata,
    pub bed_adm: AdmMetadata,
    pub bed_pack_adm: AdmMetadata,
    pub bed_channel_adm: [AdmMetadata; BED_CHANNELS],
    /// 固定 bed 的静态位置覆盖，保留屏幕边缘或边界所依附的原始坐标。空值使用规范位置。
    pub bed_geometry: [AdmMetadata; BED_CHANNELS],
    pub bed_timing: Option<AdmTiming>,
    /// 每路首个原生 ADM block，后续 block 通过 schedule_adm_bed_blocks 提交。
    pub bed_blocks: [Option<AdmBedBlock>; BED_CHANNELS],
    pub container_metadata: Option<ContainerMetadata>,
    /// 为空时生成默认的单一 content；非空时保留分组及名称。
    pub contents: Vec<ContentSpec>,
    /// 节目在外部时间线中的起点；事件仍相对于音频第 0 个采样。
    pub start_sample: u64,
    /// 空集合表示 DBMD 未指定；DAMF 的缺省或空输入列表表示 [3]。
    pub sc_bed_configuration: Vec<u8>,
    pub downmix_mode: DownmixMode,
    pub surround_phase_shift: bool,
    pub trim_mode: [Option<TrimValues>; 4],
    pub creation_tool: String,
    pub creation_tool_version: String,
    /// Logic DBMD's fixed-width ASCII originator (32) and product (64) fields.
    pub dbmd_originator: String,
    pub dbmd_product: String,
    pub screen_size_ratio: f64,
    pub bed_distribution: bool,
    /// false 表示原生元数据未指定；此时 bed_distribution 必须为 false。
    pub bed_distribution_present: bool,
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
            approximate_mappings: ApproximateMappings::default(),
            preserve_literal: LiteralPreservation::default(),
            objects: Vec::new(),
            bed: std::array::from_fn(BedState::for_channel),
            bed_channel_names: std::array::from_fn(|_| ChannelNames::default()),
            adm: AdmMetadata::default(),
            bed_adm: AdmMetadata::default(),
            bed_pack_adm: AdmMetadata::default(),
            bed_channel_adm: std::array::from_fn(|_| AdmMetadata::default()),
            bed_geometry: std::array::from_fn(|_| AdmMetadata::default()),
            bed_timing: None,
            bed_blocks: std::array::from_fn(|_| None),
            container_metadata: None,
            contents: Vec::new(),
            start_sample: 0,
            sc_bed_configuration: vec![3],
            downmix_mode: DownmixMode::LoRo,
            surround_phase_shift: false,
            trim_mode: [Some(TrimValues::default()); 4],
            creation_tool: "MacinFormat-ADM-Core".into(),
            creation_tool_version: env!("CARGO_PKG_VERSION").into(),
            dbmd_originator: "Created by MacinFormat".into(),
            dbmd_product: "MacinFormat ADM Core".into(),
            screen_size_ratio: 1.,
            bed_distribution: false,
            bed_distribution_present: true,
            warp_mode: WarpMode::LoRo,
            stem: "master".into(),
        }
    }
}
impl MasterSpec {
    pub fn set_bed_distribution(&mut self, value: Option<bool>) {
        self.bed_distribution = value.unwrap_or(false);
        self.bed_distribution_present = value.is_some();
    }
    pub fn channels(&self) -> usize {
        BED_CHANNELS + self.objects.len()
    }
    pub fn validate(&self) -> Result<()> {
        if let Some(metadata) = &self.container_metadata {
            metadata.validate()?;
        }
        if !self.bed_distribution_present && self.bed_distribution {
            return Err(Error::invalid(
                "An unspecified bedDistribution cannot have a true value",
            ));
        }
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
            if let Some(name) = &object.pack_name {
                valid_text(name)?;
            }
            object.channel_names.validate()?;
            object.adm.validate(crate::native::Level::Object)?;
            object.pack_adm.validate(crate::native::Level::Pack)?;
            object.channel_adm.validate(crate::native::Level::Channel)?;
            if !object.custom_zones.is_empty()
                && (object.initial.zone != Zone::All || !object.initial.elevation)
            {
                return Err(Error::invalid("Custom zones conflict with a zone preset"));
            }
            for zone in &object.custom_zones {
                zone.validate()?;
            }
            object.initial.validate()?;
        }
        self.adm.validate(crate::native::Level::Programme)?;
        self.bed_adm.validate(crate::native::Level::Object)?;
        self.bed_pack_adm.validate(crate::native::Level::Pack)?;
        for metadata in &self.bed_channel_adm {
            metadata.validate(crate::native::Level::Channel)?;
        }
        for geometry in &self.bed_geometry {
            geometry.validate(crate::native::Level::BedGeometry)?;
        }
        if let Some(t) = self.bed_timing {
            t.validate()?;
        }
        for (i, b) in self.bed_blocks.iter().enumerate() {
            if let Some(b) = b {
                b.validate()?;
                let first = b.labels.iter().map(|label| label.trim()).find_map(|label| {
                    BED_LABELS
                        .iter()
                        .position(|v| *v == label.strip_prefix("RC_").unwrap_or(label))
                });
                if b.channel != i || first != Some(i) {
                    return Err(Error::invalid("Initial ADM bed block has wrong channel"));
                }
            }
        }
        for bed in self.bed {
            bed.validate()?;
        }
        for names in &self.bed_channel_names {
            names.validate()?;
        }
        if self.contents.len() > 0xefff {
            return Err(Error::invalid("Too many ADM content groups"));
        }
        let mut referenced = BTreeSet::new();
        for content in &self.contents {
            valid_text(&content.name)?;
            content.adm.validate(crate::native::Level::Content)?;
            let mut members = BTreeSet::new();
            for id in &content.objects {
                if !ids.contains(id) || !members.insert(*id) {
                    return Err(Error::invalid(
                        "Content references an unknown or duplicate object",
                    ));
                }
                referenced.insert(*id);
            }
            if let Some(dialogue) = content.dialogue {
                let maximum = match dialogue.kind {
                    ContentKind::NonDialogue => 2,
                    ContentKind::Dialogue => 6,
                    ContentKind::Mixed => 3,
                };
                if dialogue.subtype > maximum {
                    return Err(Error::invalid("Invalid content dialogue subtype"));
                }
            }
        }
        if !self.contents.is_empty() && referenced != ids {
            return Err(Error::invalid("Every object must belong to a content"));
        }
        if !self.contents.is_empty() && !self.contents.iter().any(|c| c.include_bed) {
            return Err(Error::invalid(
                "The content graph must reference the fixed bed",
            ));
        }
        let mut configurations = BTreeSet::new();
        if self
            .sc_bed_configuration
            .iter()
            .any(|&v| v > 8 || !configurations.insert(v))
        {
            return Err(Error::invalid(
                "scBedConfiguration requires unique codes in 0..=8",
            ));
        }
        for value in self.trim_mode.into_iter().flatten() {
            value.validate()?;
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
pub(crate) fn valid_text(value: &str) -> Result<()> {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingDiagnosticKind {
    Loss,
    Approximation,
    PreservedLiteral,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingDiagnostic {
    pub kind: MappingDiagnosticKind,
    pub track: Option<TrackId>,
    pub sample: Option<u64>,
    pub field: &'static str,
    pub message: String,
}

pub(crate) fn record_approximation(
    diagnostics: &mut Vec<MappingDiagnostic>,
    track: TrackId,
    sample: u64,
    field: &'static str,
    message: String,
) {
    // 每条轨道、每个字段仅记录第一次，避免随时间线增长累计数值诊断。
    if !diagnostics.iter().any(|d| {
        d.kind == MappingDiagnosticKind::Approximation && d.track == Some(track) && d.field == field
    }) {
        diagnostics.push(MappingDiagnostic {
            kind: MappingDiagnosticKind::Approximation,
            track: Some(track),
            sample: Some(sample),
            field,
            message,
        });
    }
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
