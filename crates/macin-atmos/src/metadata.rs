//! 来源无关的母版序列化；低层入口允许已经完成映射的调用方保留既有导出契约。
use crate::*;
use macin_bwf::{Chna, ChnaEntry};
use std::io::Write;

const BED: [(&str, &str, [f64; 3]); 10] = [
    ("RoomCentricLeft", "RC_L", [-1., 1., 0.]),
    ("RoomCentricRight", "RC_R", [1., 1., 0.]),
    ("RoomCentricCenter", "RC_C", [0., 1., 0.]),
    ("RoomCentricLFE", "RC_LFE", [-1., 1., -1.]),
    ("RoomCentricLeftSideSurround", "RC_Lss", [-1., 0., 0.]),
    ("RoomCentricRightSideSurround", "RC_Rss", [1., 0., 0.]),
    ("RoomCentricLeftRearSurround", "RC_Lrs", [-1., -1., 0.]),
    ("RoomCentricRightRearSurround", "RC_Rrs", [1., -1., 0.]),
    ("RoomCentricLeftTopSurround", "RC_Lts", [-1., 0., 1.]),
    ("RoomCentricRightTopSurround", "RC_Rts", [1., 0., 1.]),
];
#[cfg(feature = "read")]
pub(crate) fn default_bed_channel_name(index: usize) -> &'static str {
    BED[index].0
}
pub(crate) fn bed_position(index: usize) -> [f64; 3] {
    BED[index].2
}

pub fn object_id(track: u16) -> String {
    format!("AO_{:04x}", 0x1000u32 + u32::from(track))
}
pub fn track_uid(track: u16) -> String {
    format!("ATU_{track:08x}")
}
fn bed_channel(index: usize) -> String {
    format!("AC_0001{:04x}", 0x1001 + index)
}
fn object_channel(index: usize) -> String {
    format!("AC_0003{:04x}", 0x1001 + index)
}
fn object_pack(index: usize) -> String {
    format!("AP_0003{:04x}", 0x1001 + index)
}
pub fn number(v: f64, precision: usize) -> String {
    if v == 0. {
        return "0".into();
    }
    let mut text = format!("{v:.precision$}");
    if text.parse::<f64>() == Ok(0.0) {
        return v.to_string();
    }
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}
fn units(samples: u64, rate: u32, scale: u64) -> Result<u64> {
    if rate == 0 {
        return Err(Error::invalid("Zero sample rate"));
    }
    u64::try_from(
        (u128::from(samples) * u128::from(scale) + u128::from(rate / 2)) / u128::from(rate),
    )
    .map_err(|_| Error::invalid("ADM clock overflow"))
}
fn clock(v: u64, profile: Profile) -> Result<String> {
    let scale = profile.clock_units_per_second();
    let seconds = v / scale;
    if seconds / 3600 > 99 {
        return Err(Error::invalid("ADM duration must be less than 100 hours"));
    }
    Ok(format!(
        "{:02}:{:02}:{:02}.{:0digits$}",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60,
        v % scale,
        digits = profile.clock_fraction_digits()
    ))
}
pub fn sample_time(samples: u64, rate: u32, profile: Profile) -> Result<String> {
    clock(
        units(samples, rate, profile.clock_units_per_second())?,
        profile,
    )
}
pub fn sample_span(start: u64, end: u64, rate: u32, profile: Profile) -> Result<String> {
    let scale = profile.clock_units_per_second();
    clock(
        units(end, rate, scale)?
            .checked_sub(units(start, rate, scale)?)
            .ok_or_else(|| Error::invalid("Negative time span"))?,
        profile,
    )
}
pub fn seconds_span(start: u64, end: u64, rate: u32) -> Result<String> {
    let ns = units(end, rate, 1_000_000_000)?
        .checked_sub(units(start, rate, 1_000_000_000)?)
        .ok_or_else(|| Error::invalid("Negative interpolation span"))?;
    let mut out = format!("{}.{:09}", ns / 1_000_000_000, ns % 1_000_000_000);
    while out.ends_with('0') {
        out.pop();
    }
    if out.ends_with('.') {
        out.push('0');
    }
    Ok(out)
}

pub(crate) fn sample_seconds(samples: u64, rate: u32) -> Result<String> {
    if samples == 0 {
        Ok("0".into())
    } else {
        seconds_span(0, samples, rate)
    }
}

#[cfg(feature = "read")]
pub(crate) fn parse_action_seconds(text: &str) -> Result<i64> {
    let invalid = || Error::invalid("Invalid or out-of-range FFOA in seconds");
    let negative = text.starts_with('-');
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    if (!whole.is_empty() || !fraction.is_empty())
        && whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        let seconds = if whole.is_empty() {
            0
        } else {
            whole.parse::<i128>().map_err(|_| invalid())?
        };
        let mut ns = 0i128;
        for i in 0..9 {
            ns = 10 * ns + i128::from(fraction.as_bytes().get(i).copied().unwrap_or(b'0') - b'0');
        }
        ns += i128::from(fraction.as_bytes().get(9).is_some_and(|&x| x >= b'5'));
        let absolute = seconds
            .checked_mul(1_000_000_000)
            .and_then(|s| s.checked_add(ns))
            .ok_or_else(invalid)?;
        return i64::try_from(if negative { -absolute } else { absolute }).map_err(|_| invalid());
    }
    let ns = (text.parse::<f64>().map_err(|_| invalid())? * 1e9).round();
    let min = i64::MIN as f64;
    if !(min..-min).contains(&ns) {
        return Err(invalid());
    }
    Ok(ns as i64)
}

pub(crate) fn trim_fields(values: TrimValues) -> [(&'static str, f64); 5] {
    [
        ("centerTrim", values.center),
        ("surroundTrim", values.surround),
        ("heightTrim", values.height),
        (
            "frontBackBalanceOverheadFloor",
            values.front_back_overhead_floor,
        ),
        ("frontBackBalanceListener", values.front_back_listener),
    ]
}

struct Xml<'a> {
    out: &'a mut dyn Write,
    depth: usize,
}
impl Xml<'_> {
    fn indent(&mut self) -> Result<()> {
        for _ in 0..self.depth {
            self.out.write_all(b"  ")?;
        }
        Ok(())
    }
    fn tag(&mut self, name: &str, attrs: &[(&str, &str)]) -> Result<()> {
        self.indent()?;
        write!(self.out, "<{name}")?;
        for (key, value) in attrs {
            let escaped = quick_xml::escape::escape(*value);
            if value.contains(['\n', '\r', '\t']) {
                write!(
                    self.out,
                    " {key}=\"{}\"",
                    escaped
                        .replace('\t', "&#9;")
                        .replace('\n', "&#10;")
                        .replace('\r', "&#13;")
                )?;
            } else {
                write!(self.out, " {key}=\"{escaped}\"")?;
            }
        }
        self.out.write_all(b">")?;
        Ok(())
    }
    fn start(&mut self, name: &str, attrs: &[(&str, &str)]) -> Result<()> {
        self.tag(name, attrs)?;
        self.out.write_all(b"\n")?;
        self.depth += 1;
        Ok(())
    }
    fn native_element(&mut self, value: &AdmElement) -> Result<()> {
        let name = if value.name == "channelFrequency" {
            "frequency"
        } else {
            &value.name
        };
        let mut attrs: Vec<_> = value
            .attributes
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let screen = name == "audioProgrammeReferenceScreen";
        if screen
            && !attrs.iter().any(|(k, _)| *k == "aspectRatio")
            && let Some(ratio) = value.children.iter().find(|e| e.name == "aspectRatio")
        {
            attrs.push(("aspectRatio", ratio.text.trim()));
        }
        if value.children.is_empty() {
            return self.element(name, &attrs, &value.text);
        }
        self.start(name, &attrs)?;
        for child in &value.children {
            if !(screen && child.name == "aspectRatio") {
                self.native_element(child)?;
            }
        }
        self.end(name)
    }
    fn start_extended(
        &mut self,
        name: &str,
        attrs: &[(&str, &str)],
        vendor: &[VendorAttribute],
    ) -> Result<()> {
        if vendor.is_empty() {
            return self.start(name, attrs);
        }
        let mut bindings = std::collections::BTreeSet::new();
        let mut extra = Vec::new();
        for a in vendor {
            for (prefix, uri) in std::iter::once((&a.prefix, &a.namespace))
                .chain(a.value_prefixes.iter().map(|(p, u)| (p, u)))
            {
                if prefix != "xml" && bindings.insert(prefix) {
                    extra.push((format!("xmlns:{prefix}"), uri.as_str()));
                }
            }
            extra.push((format!("{}:{}", a.prefix, a.name), a.value.as_str()));
        }
        let mut combined = attrs.to_vec();
        combined.extend(extra.iter().map(|(k, v)| (k.as_str(), *v)));
        self.start(name, &combined)
    }
    fn start_native(
        &mut self,
        name: &str,
        attrs: &[(&str, &str)],
        metadata: &AdmMetadata,
    ) -> Result<()> {
        let mut attrs = attrs.to_vec();
        attrs.extend(
            metadata
                .attributes
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str())),
        );
        self.start(name, &attrs)?;
        for element in &metadata.elements {
            self.native_element(element)?;
        }
        Ok(())
    }
    fn end(&mut self, name: &str) -> Result<()> {
        self.depth = self
            .depth
            .checked_sub(1)
            .ok_or_else(|| Error::invalid("Unbalanced XML"))?;
        self.indent()?;
        writeln!(self.out, "</{name}>")?;
        Ok(())
    }
    fn element(&mut self, name: &str, attrs: &[(&str, &str)], value: &str) -> Result<()> {
        self.tag(name, attrs)?;
        writeln!(self.out, "{}</{name}>", quick_xml::escape::escape(value))?;
        Ok(())
    }
}

pub fn chna(spec: &MasterSpec) -> Result<Chna> {
    spec.validate()?;
    let mut entries = Vec::with_capacity(spec.channels());
    for index in 0..spec.channels() {
        let (channel, pack) = if index < BED_CHANNELS {
            (bed_channel(index), "AP_00011001".into())
        } else {
            (
                object_channel(index - BED_CHANNELS),
                object_pack(index - BED_CHANNELS),
            )
        };
        let track = (index + 1) as u16;
        entries.push(ChnaEntry {
            track_index: track,
            uid: ascii(&track_uid(track))?,
            track_format: ascii(&format!("{}_01", channel.replacen("AC_", "AT_", 1)))?,
            pack_format: ascii(&pack)?,
            pad: 0,
        });
    }
    Ok(Chna {
        tracks: spec.channels() as u16,
        entries,
    })
}
fn ascii<const N: usize>(s: &str) -> Result<[u8; N]> {
    if !s.is_ascii() || s.len() > N {
        return Err(Error::invalid("ADM identifier exceeds fixed width"));
    }
    let mut data = [0; N];
    data[..s.len()].copy_from_slice(s.as_bytes());
    Ok(data)
}

#[derive(Debug, Clone, Copy)]
pub struct AdmBlock {
    pub sample: u64,
    pub duration: u64,
    pub ramp_samples: u64,
    pub state: ObjectState,
}

pub fn write_adm_block(
    out: &mut dyn Write,
    object_index: usize,
    block_number: u32,
    block: AdmBlock,
    rate: u32,
    profile: Profile,
) -> Result<()> {
    write_adm_block_with_approximations(
        out,
        object_index,
        block_number,
        block,
        rate,
        profile,
        ApproximateMappings::default(),
    )
}

/// 序列化已完成映射的 block；损失检查与近似诊断仍由调用方负责。
pub fn write_adm_block_with_approximations(
    out: &mut dyn Write,
    object_index: usize,
    block_number: u32,
    block: AdmBlock,
    rate: u32,
    profile: Profile,
    approximations: ApproximateMappings,
) -> Result<()> {
    write_native_object_block(
        out,
        object_index,
        block_number,
        block,
        rate,
        profile,
        approximations,
        LiteralPreservation::default(),
        &[],
    )
}
#[allow(clippy::too_many_arguments)]
pub fn write_native_object_block(
    out: &mut dyn Write,
    object_index: usize,
    block_number: u32,
    block: AdmBlock,
    rate: u32,
    profile: Profile,
    approximations: ApproximateMappings,
    literal: LiteralPreservation,
    zones: &[ExclusionZone],
) -> Result<()> {
    for z in zones {
        z.validate()?;
    }
    if !zones.is_empty() && (block.state.zone != Zone::All || !block.state.elevation) {
        return Err(Error::invalid(
            "A custom zone list conflicts with a zone preset",
        ));
    }
    if object_index >= MAX_OBJECTS
        || block_number == 0
        || block.duration == 0
        || block.ramp_samples > block.duration
    {
        return Err(Error::invalid("Invalid ADM block bounds"));
    }
    block.state.validate()?;
    if block
        .state
        .divergence
        .is_some_and(|v| v.azimuth_range.is_some())
        && !literal.cartesian_azimuth_range
    {
        return Err(Error::invalid(
            "Cartesian ADM cannot write divergence azimuthRange",
        ));
    }
    let end = block
        .sample
        .checked_add(block.duration)
        .ok_or_else(|| Error::invalid("Block end overflow"))?;
    let mut xml = Xml { out, depth: 5 };
    let id = format!(
        "AB_{}_{block_number:08x}",
        object_channel(object_index).trim_start_matches("AC_")
    );
    xml.start(
        "audioBlockFormat",
        &[
            ("audioBlockFormatID", &id),
            ("rtime", &sample_time(block.sample, rate, profile)?),
            ("duration", &sample_span(block.sample, end, rate, profile)?),
        ],
    )?;
    let s = block.state;
    let diffuse = if s.diffuse != 0. {
        s.diffuse
    } else if approximations.diffuse_decorrelation {
        s.decorrelation
    } else {
        0.
    };
    if diffuse != 0. {
        xml.element("diffuse", &[], &diffuse.to_string())?;
    }
    if let Some(value) = s.divergence {
        let position = value.position_range.map(|v| v.to_string());
        let azimuth = value.azimuth_range.map(|v| v.to_string());
        let attrs: Vec<_> = position
            .as_deref()
            .map(|v| ("positionRange", v))
            .into_iter()
            .chain(azimuth.as_deref().map(|v| ("azimuthRange", v)))
            .collect();
        xml.element("objectDivergence", &attrs, &value.value.to_string())?;
    }
    for (i, (axis, v)) in ["X", "Y", "Z"].iter().zip(s.position).enumerate() {
        let mut attrs = vec![("coordinate", *axis)];
        if let Some(edge) = s.screen_edge_lock[i] {
            attrs.push(("screenEdgeLock", edge.as_str()));
        }
        xml.element("position", &attrs, &number(v, 12))?;
    }
    if let Some(extent) = s.extent {
        let [x, y, z] = extent.xyz();
        for (name, v) in [("width", x), ("height", z), ("depth", y)] {
            xml.element(name, &[], &number(v, 12))?;
        }
    }
    xml.element("cartesian", &[], "1")?;
    xml.element(
        "gain",
        &[],
        &number(if s.active { s.gain.linear() } else { 0. }, 12),
    )?;
    if s.snap || s.channel_lock_distance.is_some() {
        let distance = s.channel_lock_distance.map(|v| v.to_string());
        let attrs: Vec<_> = distance
            .as_deref()
            .map(|v| ("maxDistance", v))
            .into_iter()
            .collect();
        xml.element("channelLock", &attrs, if s.snap { "1" } else { "0" })?;
    }
    let ramp_end = block
        .sample
        .checked_add(block.ramp_samples)
        .ok_or_else(|| Error::invalid("Ramp end overflow"))?;
    xml.element(
        "jumpPosition",
        &[(
            "interpolationLength",
            &seconds_span(block.sample, ramp_end, rate)?,
        )],
        "1",
    )?;
    if s.screen_ref {
        xml.element("screenRef", &[], "1")?;
    }
    if let Some(h) = s.headphone_virtualise {
        write_headphone(&mut xml, h)?;
    }
    if !zones.is_empty() {
        xml.start("zoneExclusion", &[])?;
        for zone in zones {
            let (keys, values): (&[&str], &[f64]) = match &zone.bounds {
                ZoneBounds::Cartesian(v) => (&crate::zones::ATTRIBUTES, v),
                ZoneBounds::Polar(v) => (&crate::block::POLAR_ZONE_ATTRS, v),
            };
            let text: Vec<_> = values.iter().map(ToString::to_string).collect();
            let attrs: Vec<_> = keys
                .iter()
                .zip(&text)
                .map(|(k, v)| (*k, v.as_str()))
                .collect();
            xml.element("zone", &attrs, &zone.name)?;
        }
        xml.end("zoneExclusion")?;
    }
    if s.zone != Zone::All || !s.elevation {
        xml.start("zoneExclusion", &[])?;
        for mask in crate::zones::masks(s.zone, s.elevation) {
            let values = mask.bounds.map(|v| number(v, 6));
            let attributes: Vec<_> = crate::zones::ATTRIBUTES
                .iter()
                .zip(&values)
                .map(|(k, v)| (*k, v.as_str()))
                .collect();
            xml.element("zone", &attributes, mask.name)?;
        }
        xml.end("zoneExclusion")?;
    }
    if profile == Profile::Standard && s.head_tracking == HeadTracking::HeadRelative {
        xml.element("headLocked", &[], "1")?;
    }
    xml.element(
        "importance",
        &[],
        &(s.importance * 10.).round().clamp(0., 10.).to_string(),
    )?;
    xml.end("audioBlockFormat")
}

fn write_headphone(xml: &mut Xml<'_>, value: HeadphoneVirtualise) -> Result<()> {
    let mut attrs = Vec::new();
    let drr = value.drr.map(|v| v.to_string());
    if let Some(b) = value.bypass {
        attrs.push(("bypass", if b { "1" } else { "0" }));
    }
    if let Some(v) = &drr {
        attrs.push(("DRR", v.as_str()));
    }
    xml.element("headphoneVirtualise", &attrs, "")
}
pub(crate) fn bed_state_block(
    spec: &MasterSpec,
    channel: usize,
    state: BedState,
    timing: AdmTiming,
) -> AdmBedBlock {
    let gain = if state.active {
        state.gain
    } else {
        Gain::Silence
    };
    AdmBedBlock {
        channel,
        timing,
        labels: vec![BED[channel].1.into()],
        geometry: spec.bed_geometry[channel].clone(),
        gain: (gain.linear() != 1.).then_some(gain),
        importance: (state.importance != 1.).then_some((state.importance * 10.).round() as u8),
        head_tracking: state.head_tracking,
        headphone_virtualise: None,
    }
}
/// 写一个独立的 DirectSpeakers block，保留各自的相对采样时段。
pub fn write_adm_bed_block(
    out: &mut dyn Write,
    block: &AdmBedBlock,
    sequence: u32,
    rate: u32,
    profile: Profile,
) -> Result<()> {
    block.validate()?;
    if sequence == 0 || !profile.is_adm() {
        return Err(Error::invalid("Invalid ADM bed block target"));
    }
    let id = format!(
        "AB_{}_{sequence:08x}",
        bed_channel(block.channel).trim_start_matches("AC_")
    );
    let start = block
        .timing
        .start
        .map(|v| sample_time(v, rate, profile))
        .transpose()?;
    let duration = block
        .timing
        .end()
        .map(|end| sample_span(block.timing.start.unwrap_or(0), end, rate, profile))
        .transpose()?;
    let mut attrs = vec![("audioBlockFormatID", id.as_str())];
    if let Some(v) = &start {
        attrs.push(("rtime", v));
    }
    if let Some(v) = &duration {
        attrs.push(("duration", v));
    }
    let mut xml = Xml { out, depth: 5 };
    xml.start("audioBlockFormat", &attrs)?;
    for label in &block.labels {
        xml.element("speakerLabel", &[], label)?;
    }
    if block.geometry.is_empty() {
        for (axis, v) in ["X", "Y", "Z"].iter().zip(bed_position(block.channel)) {
            xml.element("position", &[("coordinate", axis)], &number(v, 12))?;
        }
        xml.element("cartesian", &[], "1")?;
    } else {
        for node in &block.geometry.elements {
            xml.native_element(node)?;
        }
    }
    if let Some(gain) = block.gain {
        match gain {
            Gain::Decibels(v) => xml.element("gain", &[("gainUnit", "dB")], &v.to_string())?,
            g => xml.element("gain", &[], &g.linear().to_string())?,
        }
    }
    if let Some(v) = block.importance {
        xml.element("importance", &[], &v.to_string())?;
    }
    if profile == Profile::Standard && block.head_tracking == HeadTracking::HeadRelative {
        xml.element("headLocked", &[], "1")?;
    }
    if let Some(v) = block.headphone_virtualise {
        write_headphone(&mut xml, v)?;
    }
    xml.end("audioBlockFormat")
}
/// Emit the graph once; object block fragments may be copied from bounded scratch files.
pub fn write_axml(
    out: &mut dyn Write,
    spec: &MasterSpec,
    frames: u64,
    blocks: impl FnMut(usize, &mut dyn Write) -> Result<()>,
) -> Result<()> {
    write_axml_with_beds(out, spec, frames, |_, _| Ok(false), blocks)
}
pub fn write_axml_with_beds(
    out: &mut dyn Write,
    spec: &MasterSpec,
    frames: u64,
    mut bed_blocks: impl FnMut(usize, &mut dyn Write) -> Result<bool>,
    mut blocks: impl FnMut(usize, &mut dyn Write) -> Result<()>,
) -> Result<()> {
    spec.validate()?;
    let start = sample_time(0, spec.sample_rate, spec.profile)?;
    let duration = sample_time(frames, spec.sample_rate, spec.profile)?;
    let programme_start = sample_time(spec.start_sample, spec.sample_rate, spec.profile)?;
    let programme_end = sample_time(
        spec.start_sample
            .checked_add(frames)
            .ok_or_else(|| Error::invalid("Programme end overflow"))?,
        spec.sample_rate,
        spec.profile,
    )?;
    out.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")?;
    let mut xml = Xml { out, depth: 0 };
    xml.start(
        "ebuCoreMain",
        &[("xmlns", "urn:ebu:metadata-schema:ebuCore_2017")],
    )?;
    xml.start("coreMetadata", &[])?;
    xml.start("format", &[])?;
    xml.start("audioFormatExtended", &[("version", "ITU-R_BS.2076-2")])?;
    xml.start_native(
        "audioProgramme",
        &[
            ("audioProgrammeID", "APR_1001"),
            ("audioProgrammeName", &spec.name),
            ("start", &programme_start),
            ("end", &programme_end),
        ],
        &spec.adm,
    )?;
    let default_content;
    let contents = if spec.contents.is_empty() {
        default_content = vec![ContentSpec {
            name: format!("{} Content", spec.name),
            include_bed: true,
            objects: spec.objects.iter().map(|o| o.id).collect(),
            dialogue: None,
            adm: AdmMetadata::default(),
        }];
        &default_content
    } else {
        &spec.contents
    };
    for i in 0..contents.len() {
        xml.element("audioContentIDRef", &[], &format!("ACO_{:04x}", 0x1001 + i))?;
    }
    xml.end("audioProgramme")?;
    for (i, content) in contents.iter().enumerate() {
        xml.start_native(
            "audioContent",
            &[
                ("audioContentID", &format!("ACO_{:04x}", 0x1001 + i)),
                ("audioContentName", &content.name),
            ],
            &content.adm,
        )?;
        if content.include_bed {
            xml.element("audioObjectIDRef", &[], "AO_1001")?;
        }
        for id in &content.objects {
            let index = spec
                .objects
                .iter()
                .position(|o| o.id == *id)
                .ok_or_else(|| Error::invalid("Unknown content object"))?;
            xml.element("audioObjectIDRef", &[], &object_id((11 + index) as u16))?;
        }
        if let Some(dialogue) = content.dialogue {
            let (kind, attribute) = match dialogue.kind {
                ContentKind::NonDialogue => ("0", "nonDialogueContentKind"),
                ContentKind::Dialogue => ("1", "dialogueContentKind"),
                ContentKind::Mixed => ("2", "mixedContentKind"),
            };
            xml.element(
                "dialogue",
                &[(attribute, &dialogue.subtype.to_string())],
                kind,
            )?;
        }
        xml.end("audioContent")?;
    }
    let timing = spec.bed_timing.unwrap_or(AdmTiming {
        start: Some(0),
        duration: Some(frames),
    });
    let bed_start = timing
        .start
        .map(|v| sample_time(v, spec.sample_rate, spec.profile))
        .transpose()?;
    let bed_duration = timing
        .end()
        .map(|end| {
            sample_span(
                timing.start.unwrap_or(0),
                end,
                spec.sample_rate,
                spec.profile,
            )
        })
        .transpose()?;
    let mut attrs = vec![
        ("audioObjectID", "AO_1001"),
        ("audioObjectName", spec.bed_name.as_str()),
    ];
    if let Some(v) = &bed_start {
        attrs.push(("start", v));
    }
    if let Some(v) = &bed_duration {
        attrs.push(("duration", v));
    }
    xml.start_native("audioObject", &attrs, &spec.bed_adm)?;
    xml.element("audioPackFormatIDRef", &[], "AP_00011001")?;
    for i in 1..=10 {
        xml.element("audioTrackUIDRef", &[], &track_uid(i))?;
    }
    xml.end("audioObject")?;
    for (i, object) in spec.objects.iter().enumerate() {
        xml.start_native(
            "audioObject",
            &[
                ("audioObjectID", &object_id((11 + i) as u16)),
                ("audioObjectName", &object.name),
                ("start", &start),
                ("duration", &duration),
            ],
            &object.adm,
        )?;
        xml.element("audioPackFormatIDRef", &[], &object_pack(i))?;
        xml.element("audioTrackUIDRef", &[], &track_uid((11 + i) as u16))?;
        xml.end("audioObject")?;
    }
    xml.start_native(
        "audioPackFormat",
        &[
            ("audioPackFormatID", "AP_00011001"),
            ("audioPackFormatName", &spec.bed_pack_name),
            ("typeLabel", "0001"),
            ("typeDefinition", "DirectSpeakers"),
        ],
        &spec.bed_pack_adm,
    )?;
    for i in 0..10 {
        xml.element("audioChannelFormatIDRef", &[], &bed_channel(i))?;
    }
    xml.end("audioPackFormat")?;
    for (i, object) in spec.objects.iter().enumerate() {
        xml.start_native(
            "audioPackFormat",
            &[
                ("audioPackFormatID", &object_pack(i)),
                (
                    "audioPackFormatName",
                    object.pack_name.as_deref().unwrap_or(&object.name),
                ),
                ("typeLabel", "0003"),
                ("typeDefinition", "Objects"),
            ],
            &object.pack_adm,
        )?;
        xml.element("audioChannelFormatIDRef", &[], &object_channel(i))?;
        xml.end("audioPackFormat")?;
    }
    for (i, (name, _, _)) in BED.iter().enumerate() {
        let id = bed_channel(i);
        xml.start_native(
            "audioChannelFormat",
            &[
                ("audioChannelFormatID", &id),
                (
                    "audioChannelFormatName",
                    spec.bed_channel_names[i].channel.as_deref().unwrap_or(name),
                ),
                ("typeLabel", "0001"),
                ("typeDefinition", "DirectSpeakers"),
            ],
            &spec.bed_channel_adm[i],
        )?;
        if !bed_blocks(i, xml.out)? {
            let generated;
            let first = match &spec.bed_blocks[i] {
                Some(b) => b,
                None => {
                    generated = bed_state_block(spec, i, spec.bed[i], AdmTiming::default());
                    &generated
                }
            };
            write_adm_bed_block(xml.out, first, 1, spec.sample_rate, spec.profile)?;
        }
        xml.end("audioChannelFormat")?;
    }
    for (i, object) in spec.objects.iter().enumerate() {
        xml.start_native(
            "audioChannelFormat",
            &[
                ("audioChannelFormatID", &object_channel(i)),
                (
                    "audioChannelFormatName",
                    object
                        .channel_names
                        .channel
                        .as_deref()
                        .unwrap_or(&object.name),
                ),
                ("typeLabel", "0003"),
                ("typeDefinition", "Objects"),
            ],
            &object.channel_adm,
        )?;
        blocks(i, xml.out)?;
        xml.end("audioChannelFormat")?;
    }
    let channels = BED
        .iter()
        .enumerate()
        .map(|(i, b)| {
            (
                bed_channel(i),
                "AP_00011001".to_owned(),
                spec.bed_channel_names[i].channel.as_deref().unwrap_or(b.0),
                &spec.bed_channel_names[i],
            )
        })
        .chain(spec.objects.iter().enumerate().map(|(i, o)| {
            (
                object_channel(i),
                object_pack(i),
                o.channel_names.channel.as_deref().unwrap_or(&o.name),
                &o.channel_names,
            )
        }));
    for (id, pack, name, names) in channels {
        let stream = id.replacen("AC_", "AS_", 1);
        let track = format!("{}_01", id.replacen("AC_", "AT_", 1));
        let pcm = format!("PCM_{name}");
        xml.start_extended(
            "audioStreamFormat",
            &[
                ("audioStreamFormatID", &stream),
                (
                    "audioStreamFormatName",
                    names.stream.as_deref().unwrap_or(&pcm),
                ),
                ("formatLabel", "0001"),
                ("formatDefinition", "PCM"),
            ],
            &names.extensions.stream,
        )?;
        xml.element("audioChannelFormatIDRef", &[], &id)?;
        if names.stream_pack_ref {
            xml.element("audioPackFormatIDRef", &[], &pack)?;
        }
        xml.element("audioTrackFormatIDRef", &[], &track)?;
        xml.end("audioStreamFormat")?;
        xml.start_extended(
            "audioTrackFormat",
            &[
                ("audioTrackFormatID", &track),
                (
                    "audioTrackFormatName",
                    names.track.as_deref().unwrap_or(&pcm),
                ),
                ("formatLabel", "0001"),
                ("formatDefinition", "PCM"),
            ],
            &names.extensions.track,
        )?;
        xml.element("audioStreamFormatIDRef", &[], &stream)?;
        xml.end("audioTrackFormat")?;
    }
    for i in 0..spec.channels() {
        let (id, pack) = if i < 10 {
            (bed_channel(i), "AP_00011001".into())
        } else {
            (object_channel(i - 10), object_pack(i - 10))
        };
        xml.start_extended(
            "audioTrackUID",
            &[
                ("UID", &track_uid((i + 1) as u16)),
                ("bitDepth", "24"),
                ("sampleRate", &spec.sample_rate.to_string()),
            ],
            if i < BED_CHANNELS {
                &spec.bed_channel_names[i].extensions.uid
            } else {
                &spec.objects[i - BED_CHANNELS].channel_names.extensions.uid
            },
        )?;
        xml.element(
            "audioTrackFormatIDRef",
            &[],
            &format!("{}_01", id.replacen("AC_", "AT_", 1)),
        )?;
        xml.element("audioPackFormatIDRef", &[], &pack)?;
        xml.end("audioTrackUID")?;
    }
    for name in [
        "audioFormatExtended",
        "format",
        "coreMetadata",
        "ebuCoreMain",
    ] {
        xml.end(name)?;
    }
    Ok(())
}

pub fn build_axml(spec: &MasterSpec, frames: u64, blocks: &[Vec<AdmBlock>]) -> Result<String> {
    if blocks.len() != spec.objects.len() {
        return Err(Error::invalid("Object timeline count mismatch"));
    }
    let mut data = Vec::new();
    write_axml(&mut data, spec, frames, |i, out| {
        let mut end = 0;
        for (j, block) in blocks[i].iter().copied().enumerate() {
            if block.sample != end {
                return Err(Error::invalid(
                    "ADM blocks must cover a contiguous timeline",
                ));
            }
            end = block
                .sample
                .checked_add(block.duration)
                .ok_or_else(|| Error::invalid("Block end overflow"))?;
            write_native_object_block(
                out,
                i,
                u32::try_from(j + 1).map_err(|_| Error::invalid("Too many ADM blocks"))?,
                block,
                spec.sample_rate,
                spec.profile,
                spec.approximate_mappings,
                spec.preserve_literal,
                &spec.objects[i].custom_zones,
            )?;
        }
        if end != frames {
            return Err(Error::invalid("ADM timeline does not match PCM duration"));
        }
        Ok(())
    })?;
    String::from_utf8(data).map_err(|_| Error::invalid("Invalid XML encoding"))
}

pub fn yaml_quote(value: &str) -> String {
    let mut s = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c if c.is_control() => {
                use std::fmt::Write;
                write!(s, "\\u{:04x}", u32::from(c)).unwrap();
            }
            c => s.push(c),
        }
    }
    s.push('"');
    s
}

pub fn damf_manifest(spec: &MasterSpec) -> Result<String> {
    spec.validate()?;
    if spec.sc_bed_configuration.is_empty() {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "An unspecified DBMD configuration must be mapped before writing DAMF",
        ));
    }
    let mut lines = vec![
        format!("version: {}", spec.profile.damf_version()),
        "presentations:".into(),
        format!("  - type: {}", spec.profile.damf_type()),
        "    simplified: false".into(),
        format!(
            "    metadata: {}",
            yaml_quote(&format!("{}.atmos.metadata", spec.stem))
        ),
        format!(
            "    audio: {}",
            yaml_quote(&format!("{}.atmos.audio", spec.stem))
        ),
        format!(
            "    offset: {}",
            sample_seconds(spec.start_sample, spec.sample_rate)?
        ),
        format!("    fps: {}", spec.frame_rate.as_str()),
        format!(
            "    scBedConfiguration: [{}]",
            spec.sc_bed_configuration
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        format!(
            "    creationTool: {}",
            yaml_plain_or_quote(&spec.creation_tool)
        ),
        format!(
            "    creationToolVersion: {}",
            yaml_plain_or_quote(&spec.creation_tool_version)
        ),
    ];
    if let Some(ns) = spec.first_action_ns {
        let sign = if ns < 0 { "-" } else { "" };
        lines.push(format!(
            "    ffoa: {sign}{}",
            sample_seconds(ns.unsigned_abs(), 1_000_000_000)?
        ));
    }
    if spec.downmix_mode != DownmixMode::NotIndicated {
        lines.push(format!(
            "    downmixType_5to2: {}",
            spec.downmix_mode.as_str()
        ));
    }
    if spec.surround_phase_shift_present {
        lines.push(format!(
            "    51-to-20_LsRs90degPhaseShift: {}",
            spec.surround_phase_shift
        ));
    }
    if spec.warp_mode != WarpMode::NotIndicated {
        lines.push(format!("    warpMode: {}", spec.warp_mode.as_str()));
    }
    lines.push(format!(
        "    screenSizeRatio: {}",
        number(spec.screen_size_ratio, 9)
    ));
    if spec.bed_distribution_present {
        lines.push(format!("    bedDistribution: {}", spec.bed_distribution));
    }
    lines.push("    trimMode:".into());
    for config in TrimConfiguration::ALL {
        if let Some(values) = spec.trim_mode[config.index()] {
            lines.push(format!("      {}:", config.as_str()));
            let mut any = false;
            for (name, value) in trim_fields(values) {
                if value != 0. {
                    lines.push(format!("        {name}: {}", number(value, 9)));
                    any = true;
                }
            }
            if !any {
                lines.push("        {}".into());
            }
        }
    }
    if spec.trim_mode.iter().all(Option::is_none) {
        lines.push("      {}".into());
    }
    lines.extend([
        "    bedInstances:".into(),
        format!("      - description: {}", yaml_quote(&spec.bed_name)),
        "        channels:".into(),
    ]);
    for (i, channel) in BED_LABELS.iter().enumerate() {
        lines.extend([
            format!("          - channel: {channel}"),
            format!("            ID: {i}"),
        ]);
    }
    lines.push(if spec.objects.is_empty() {
        "    objects: []".into()
    } else {
        "    objects:".into()
    });
    for (i, object) in spec.objects.iter().enumerate() {
        lines.extend([
            format!("      - description: {}", yaml_quote(&object.name)),
            format!("        ID: {}", i + 10),
        ]);
    }
    Ok(lines.join("\n") + "\n")
}

fn yaml_plain_or_quote(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.trim() == value
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '-' | '_'))
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "true"
                | "false"
                | "null"
                | "yes"
                | "no"
                | "on"
                | "off"
                | "y"
                | "n"
                | ".nan"
                | ".inf"
                | "-.inf"
        )
        && value.parse::<f64>().is_err();
    if plain {
        value.into()
    } else {
        yaml_quote(value)
    }
}
pub fn damf_bed_events() -> String {
    let mut out = String::new();
    for id in 0..BED_CHANNELS {
        out.push_str(
            &damf_bed_event(
                id as u32,
                0,
                BedPatch::full(BedState::for_channel(id)),
                Some(0),
            )
            .expect("default bed is valid"),
        );
    }
    out
}
pub fn damf_bed_event(id: u32, sample: u64, patch: BedPatch, ramp: Option<u64>) -> Result<String> {
    damf_event(id, sample, patch.object_patch(), ramp)
}
pub fn head_tracking_name(mode: HeadTracking) -> &'static str {
    match mode {
        HeadTracking::SceneRelative => "scene relative",
        HeadTracking::HeadRelative => "head relative",
        HeadTracking::Undefined => "undefined",
    }
}
pub fn binaural_name(mode: BinauralMode) -> &'static str {
    match mode {
        BinauralMode::Undefined => "undefined",
        BinauralMode::Off => "off",
        BinauralMode::Near => "near",
        BinauralMode::Mid => "mid",
        BinauralMode::Far => "far",
    }
}

/// Serialize a patch, preserving field inheritance and leaving continuous ramps untouched when
/// only discrete properties change. The caller has already checked the selected backend's losses.
pub fn damf_event(id: u32, sample: u64, patch: ObjectPatch, ramp: Option<u64>) -> Result<String> {
    patch.apply(ObjectState::default()).validate()?;
    if patch.trim_bypass_unspecified {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "DAMF cannot encode an unspecified DBMD trim bypass",
        ));
    }
    use std::fmt::Write;
    let mut out = format!("  - ID: {id}\n    samplePos: {sample}\n");
    macro_rules! field {
        ($f:ident, $name:literal) => {
            if let Some(v) = patch.$f {
                writeln!(out, concat!("    ", $name, ": {}"), v).unwrap();
            }
        };
    }
    field!(active, "active");
    if let Some([x, y, z]) = patch.position {
        writeln!(
            out,
            "    pos: [{}, {}, {}]",
            number(x, 9),
            number(y, 9),
            number(z, 9)
        )
        .unwrap();
    }
    field!(snap, "snap");
    field!(elevation, "elevation");
    if let Some(v) = patch.zone {
        writeln!(out, "    zones: {}", v.as_str()).unwrap();
    }
    if let Some(extent) = patch.extent {
        match extent {
            None => out.push_str("    size: 0\n"),
            Some(Extent::Uniform(v)) => writeln!(out, "    size: {}", number(v, 9)).unwrap(),
            Some(Extent::Cartesian([x, y, z])) => writeln!(
                out,
                "    size3D: [{}, {}, {}]",
                number(x, 9),
                number(y, 9),
                number(z, 9)
            )
            .unwrap(),
        }
    }
    for (name, value) in [
        ("decorr", patch.decorrelation),
        ("importance", patch.importance),
    ] {
        if let Some(v) = value {
            writeln!(out, "    {name}: {}", number(v, 9)).unwrap();
        }
    }
    if let Some(gain) = patch.gain {
        writeln!(
            out,
            "    gain: {}",
            gain.decibels()
                .map_or_else(|| "-.inf".into(), |v| number(v, 9))
        )
        .unwrap();
    }
    if let Some(v) = ramp {
        writeln!(out, "    rampLength: {v}").unwrap();
    }
    field!(trim_bypass, "trimBypass");
    field!(dialog, "dialog");
    field!(music, "music");
    for (name, value) in [
        ("screenFactor", patch.screen_factor),
        ("depthFactor", patch.depth_factor),
    ] {
        if let Some(v) = value {
            writeln!(out, "    {name}: {}", number(v, 9)).unwrap();
        }
    }
    if let Some(v) = patch.head_tracking {
        writeln!(out, "    headTrackMode: {}", head_tracking_name(v)).unwrap();
    }
    if let Some(v) = patch.binaural_mode {
        writeln!(out, "    binauralRenderMode: {}", binaural_name(v)).unwrap();
    }
    Ok(out)
}
