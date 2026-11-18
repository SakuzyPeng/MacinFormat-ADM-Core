//! 可逐块提交的 ADM 数据，变长区域和扬声器标签只占用当前 block 的内存。
use crate::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HeadphoneVirtualise {
    pub bypass: Option<bool>,
    pub drr: Option<f64>,
}
impl HeadphoneVirtualise {
    pub(crate) fn validate(self) -> Result<()> {
        if let Some(v) = self.drr {
            crate::native::range(v, -130., 130.)?;
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZoneBounds {
    /// minX, maxX, minY, maxY, minZ, maxZ。
    Cartesian([f64; 6]),
    /// minAzimuth, maxAzimuth, minElevation, maxElevation。方位区间可以跨越 ±180°。
    Polar([f64; 4]),
}
pub(crate) const POLAR_ZONE_ATTRS: [&str; 4] =
    ["minAzimuth", "maxAzimuth", "minElevation", "maxElevation"];
#[derive(Debug, Clone, PartialEq)]
pub struct ExclusionZone {
    pub name: String,
    pub bounds: ZoneBounds,
}
impl ExclusionZone {
    pub(crate) fn validate(&self) -> Result<()> {
        crate::model::valid_text(&self.name)?;
        match self.bounds {
            ZoneBounds::Cartesian(values) => {
                for pair in values.as_chunks::<2>().0 {
                    crate::native::range(pair[0], -1., 1.)?;
                    crate::native::range(pair[1], pair[0], 1.)?;
                }
            }
            ZoneBounds::Polar([a, b, c, d]) => {
                crate::native::range(a, -180., 180.)?;
                crate::native::range(b, -180., 180.)?;
                crate::native::range(c, -90., 90.)?;
                crate::native::range(d, c, 90.)?;
            }
        }
        Ok(())
    }
}
/// None 继承自定义区域，Some(空列表) 清除。普通 ObjectUpdate 仍为 Copy。
#[derive(Debug, Clone, PartialEq)]
pub struct NativeObjectUpdate {
    pub event: ObjectUpdate,
    pub zones: Option<Vec<ExclusionZone>>,
}
impl From<ObjectUpdate> for NativeObjectUpdate {
    fn from(event: ObjectUpdate) -> Self {
        Self { event, zones: None }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdmTiming {
    pub start: Option<u64>,
    pub duration: Option<u64>,
}
impl AdmTiming {
    pub(crate) fn validate(self) -> Result<()> {
        if self
            .duration
            .is_some_and(|d| self.start.unwrap_or(0).checked_add(d).is_none())
        {
            return Err(Error::invalid("ADM time span overflow"));
        }
        Ok(())
    }
    pub(crate) fn end(self) -> Option<u64> {
        self.duration
            .and_then(|d| self.start.unwrap_or(0).checked_add(d))
    }
}
/// 每个 block 的 rtime 相对于 bed 对象起点。输入的缺省时长、间隙和标签归属均可保留。
#[derive(Debug, Clone, PartialEq)]
pub struct AdmBedBlock {
    pub channel: usize,
    pub timing: AdmTiming,
    pub labels: Vec<String>,
    pub geometry: AdmMetadata,
    pub gain: Option<Gain>,
    pub importance: Option<u8>,
    pub head_tracking: HeadTracking,
    pub headphone_virtualise: Option<HeadphoneVirtualise>,
}
impl AdmBedBlock {
    #[cfg(feature = "read")]
    /// 原节目外的块没有有效采样。节目内的块仍需由替换音频覆盖其起点。
    pub(crate) fn clip_to_audio(
        &mut self,
        origin: u64,
        programme_end: Option<u64>,
        frames: u64,
    ) -> Result<bool> {
        let start = origin
            .checked_add(self.timing.start.unwrap_or(0))
            .ok_or_else(|| Error::invalid("Bed block start overflow"))?;
        if programme_end.is_some_and(|end| start >= end) {
            return Ok(false);
        }
        let remaining = frames
            .min(programme_end.unwrap_or(frames))
            .checked_sub(start)
            .filter(|&n| n != 0)
            .ok_or_else(|| Error::invalid("Replacement audio ends before a bed block starts"))?;
        if let Some(duration) = &mut self.timing.duration {
            *duration = (*duration).min(remaining);
        }
        Ok(true)
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if self.channel >= BED_CHANNELS {
            return Err(Error::invalid("ADM bed block channel index out of range"));
        }
        for label in &self.labels {
            crate::model::valid_text(label)?;
        }
        self.geometry.validate(crate::native::Level::BedGeometry)?;
        self.timing.validate()?;
        if let Some(v) = self.headphone_virtualise {
            v.validate()?;
        }
        if self.importance.is_some_and(|i| i > 10) {
            return Err(Error::invalid("Bed block importance must be 0..=10"));
        }
        ObjectState {
            gain: self.gain.unwrap_or_default(),
            ..Default::default()
        }
        .validate()
    }
}

/// 扩展属性按所在元素保存前缀、名称、命名空间 URI 和文本值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorAttribute {
    pub prefix: String,
    pub name: String,
    pub namespace: String,
    pub value: String,
    pub value_prefixes: Vec<(String, String)>,
}
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrackExtensions {
    pub stream: Vec<VendorAttribute>,
    pub track: Vec<VendorAttribute>,
    pub uid: Vec<VendorAttribute>,
}
impl TrackExtensions {
    pub(crate) fn validate(&self) -> Result<()> {
        for attrs in [&self.stream, &self.track, &self.uid] {
            let mut bindings = BTreeMap::new();
            let mut names = BTreeSet::new();
            for a in attrs {
                if !ncname(&a.name) || !names.insert((&a.namespace, &a.name)) {
                    return Err(Error::invalid(
                        "Invalid or duplicate namespaced track attribute",
                    ));
                }
                crate::model::valid_text(&a.value)?;
                for (prefix, uri) in std::iter::once((&a.prefix, &a.namespace))
                    .chain(a.value_prefixes.iter().map(|(p, u)| (p, u)))
                {
                    crate::model::valid_text(uri)?;
                    if !ncname(prefix)
                        || prefix == "xmlns"
                        || uri.is_empty()
                        || uri == "http://www.w3.org/2000/xmlns/"
                        || (prefix == "xml") != (uri == "http://www.w3.org/XML/1998/namespace")
                        || bindings.insert(prefix, uri).is_some_and(|old| old != uri)
                    {
                        return Err(Error::invalid(
                            "Invalid or conflicting XML namespace binding",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
    pub fn is_empty(&self) -> bool {
        self.stream.is_empty() && self.track.is_empty() && self.uid.is_empty()
    }
}
fn ncname(s: &str) -> bool {
    fn start(c: char) -> bool {
        matches!(c,'A'..='Z'|'a'..='z'|'_'|'\u{c0}'..='\u{d6}'|'\u{d8}'..='\u{f6}'|'\u{f8}'..='\u{2ff}'|'\u{370}'..='\u{37d}'|'\u{37f}'..='\u{1fff}'|'\u{200c}'..='\u{200d}'|'\u{2070}'..='\u{218f}'|'\u{2c00}'..='\u{2fef}'|'\u{3001}'..='\u{d7ff}'|'\u{f900}'..='\u{fdcf}'|'\u{fdf0}'..='\u{fffd}'|'\u{10000}'..='\u{effff}')
    }
    let mut chars = s.chars();
    chars.next().is_some_and(start) && chars.all(|c| {
        start(c)
            || matches!(c,'-'|'.'|'0'..='9'|'\u{b7}'|'\u{300}'..='\u{36f}'|'\u{203f}'..='\u{2040}')
    })
}
