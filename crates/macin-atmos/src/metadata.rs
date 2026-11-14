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
            write!(self.out, " {key}=\"{}\"", quick_xml::escape::escape(*value))?;
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
    if object_index >= MAX_OBJECTS
        || block_number == 0
        || block.duration == 0
        || block.ramp_samples > block.duration
    {
        return Err(Error::invalid("Invalid ADM block bounds"));
    }
    block.state.validate()?;
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
    for (axis, v) in ["X", "Y", "Z"].iter().zip(s.position) {
        xml.element("position", &[("coordinate", axis)], &number(v, 12))?;
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
    if s.snap {
        xml.element("channelLock", &[], "1")?;
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
    xml.element(
        "importance",
        &[],
        &(s.importance * 10.).round().clamp(0., 10.).to_string(),
    )?;
    xml.end("audioBlockFormat")
}

/// Emit the graph once; object block fragments may be copied from bounded scratch files.
pub fn write_axml(
    out: &mut dyn Write,
    spec: &MasterSpec,
    frames: u64,
    mut blocks: impl FnMut(usize, &mut dyn Write) -> Result<()>,
) -> Result<()> {
    spec.validate()?;
    let start = sample_time(0, spec.sample_rate, spec.profile)?;
    let duration = sample_time(frames, spec.sample_rate, spec.profile)?;
    out.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")?;
    let mut xml = Xml { out, depth: 0 };
    xml.start(
        "ebuCoreMain",
        &[("xmlns", "urn:ebu:metadata-schema:ebuCore_2017")],
    )?;
    xml.start("coreMetadata", &[])?;
    xml.start("format", &[])?;
    xml.start("audioFormatExtended", &[("version", "ITU-R_BS.2076-2")])?;
    xml.start(
        "audioProgramme",
        &[
            ("audioProgrammeID", "APR_1001"),
            ("audioProgrammeName", &spec.name),
            ("start", &start),
            ("end", &duration),
        ],
    )?;
    xml.element("audioContentIDRef", &[], "ACO_1001")?;
    xml.end("audioProgramme")?;
    xml.start(
        "audioContent",
        &[
            ("audioContentID", "ACO_1001"),
            ("audioContentName", &format!("{} Content", spec.name)),
        ],
    )?;
    xml.element("audioObjectIDRef", &[], "AO_1001")?;
    for i in 0..spec.objects.len() {
        xml.element("audioObjectIDRef", &[], &object_id((11 + i) as u16))?;
    }
    xml.end("audioContent")?;
    xml.start(
        "audioObject",
        &[
            ("audioObjectID", "AO_1001"),
            ("audioObjectName", &spec.bed_name),
            ("start", &start),
            ("duration", &duration),
        ],
    )?;
    xml.element("audioPackFormatIDRef", &[], "AP_00011001")?;
    for i in 1..=10 {
        xml.element("audioTrackUIDRef", &[], &track_uid(i))?;
    }
    xml.end("audioObject")?;
    for (i, object) in spec.objects.iter().enumerate() {
        xml.start(
            "audioObject",
            &[
                ("audioObjectID", &object_id((11 + i) as u16)),
                ("audioObjectName", &object.name),
                ("start", &start),
                ("duration", &duration),
            ],
        )?;
        xml.element("audioPackFormatIDRef", &[], &object_pack(i))?;
        xml.element("audioTrackUIDRef", &[], &track_uid((11 + i) as u16))?;
        xml.end("audioObject")?;
    }
    xml.start(
        "audioPackFormat",
        &[
            ("audioPackFormatID", "AP_00011001"),
            ("audioPackFormatName", &spec.bed_pack_name),
            ("typeLabel", "0001"),
            ("typeDefinition", "DirectSpeakers"),
        ],
    )?;
    for i in 0..10 {
        xml.element("audioChannelFormatIDRef", &[], &bed_channel(i))?;
    }
    xml.end("audioPackFormat")?;
    for (i, object) in spec.objects.iter().enumerate() {
        xml.start(
            "audioPackFormat",
            &[
                ("audioPackFormatID", &object_pack(i)),
                ("audioPackFormatName", &object.name),
                ("typeLabel", "0003"),
                ("typeDefinition", "Objects"),
            ],
        )?;
        xml.element("audioChannelFormatIDRef", &[], &object_channel(i))?;
        xml.end("audioPackFormat")?;
    }
    for (i, (name, label, position)) in BED.iter().enumerate() {
        let id = bed_channel(i);
        xml.start(
            "audioChannelFormat",
            &[
                ("audioChannelFormatID", &id),
                ("audioChannelFormatName", name),
                ("typeLabel", "0001"),
                ("typeDefinition", "DirectSpeakers"),
            ],
        )?;
        xml.start(
            "audioBlockFormat",
            &[(
                "audioBlockFormatID",
                &format!("AB_{}_00000001", id.trim_start_matches("AC_")),
            )],
        )?;
        xml.element("speakerLabel", &[], label)?;
        for (axis, v) in ["X", "Y", "Z"].iter().zip(position) {
            xml.element("position", &[("coordinate", axis)], &number(*v, 12))?;
        }
        xml.element("cartesian", &[], "1")?;
        xml.end("audioBlockFormat")?;
        xml.end("audioChannelFormat")?;
    }
    for (i, object) in spec.objects.iter().enumerate() {
        xml.start(
            "audioChannelFormat",
            &[
                ("audioChannelFormatID", &object_channel(i)),
                ("audioChannelFormatName", &object.name),
                ("typeLabel", "0003"),
                ("typeDefinition", "Objects"),
            ],
        )?;
        blocks(i, xml.out)?;
        xml.end("audioChannelFormat")?;
    }
    let channels = BED
        .iter()
        .enumerate()
        .map(|(i, b)| (bed_channel(i), b.0))
        .chain(
            spec.objects
                .iter()
                .enumerate()
                .map(|(i, o)| (object_channel(i), o.name.as_str())),
        );
    for (id, name) in channels {
        let stream = id.replacen("AC_", "AS_", 1);
        let track = format!("{}_01", id.replacen("AC_", "AT_", 1));
        let pcm = format!("PCM_{name}");
        xml.start(
            "audioStreamFormat",
            &[
                ("audioStreamFormatID", &stream),
                ("audioStreamFormatName", &pcm),
                ("formatLabel", "0001"),
                ("formatDefinition", "PCM"),
            ],
        )?;
        xml.element("audioChannelFormatIDRef", &[], &id)?;
        xml.element("audioTrackFormatIDRef", &[], &track)?;
        xml.end("audioStreamFormat")?;
        xml.start(
            "audioTrackFormat",
            &[
                ("audioTrackFormatID", &track),
                ("audioTrackFormatName", &pcm),
                ("formatLabel", "0001"),
                ("formatDefinition", "PCM"),
            ],
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
        xml.start(
            "audioTrackUID",
            &[
                ("UID", &track_uid((i + 1) as u16)),
                ("bitDepth", "24"),
                ("sampleRate", &spec.sample_rate.to_string()),
            ],
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
            write_adm_block(
                out,
                i,
                u32::try_from(j + 1).map_err(|_| Error::invalid("Too many ADM blocks"))?,
                block,
                spec.sample_rate,
                spec.profile,
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
        "    offset: 0".into(),
        format!("    fps: {}", spec.frame_rate.as_str()),
        "    scBedConfiguration: [3]".into(),
        format!(
            "    creationTool: {}",
            yaml_plain_or_quote(&spec.creation_tool)
        ),
        format!(
            "    creationToolVersion: {}",
            yaml_plain_or_quote(&spec.creation_tool_version)
        ),
        "    downmixType_5to2: LoRo_Stereo".into(),
        "    51-to-20_LsRs90degPhaseShift: false".into(),
        format!("    warpMode: {}", spec.warp_mode.as_str()),
        format!("    screenSizeRatio: {}", number(spec.screen_size_ratio, 9)),
        format!("    bedDistribution: {}", spec.bed_distribution),
        "    trimMode:".into(),
    ];
    for name in [
        "SomeSurroundsNoHeights",
        "SomeSurroundsSomeHeights",
        "SomeSurroundsManyHeights",
        "ManySurroundsNoHeights",
    ] {
        lines.push(format!("      {name}:"));
        lines.push("        {}".into());
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
    for (id, label) in BED_LABELS.iter().enumerate() {
        use std::fmt::Write;
        writeln!(out, "  - ID: {id}\n    samplePos: 0\n    active: true\n    importance: 1\n    gain: 0\n    rampLength: 0\n    trimBypass: false\n    headTrackMode: scene relative\n    binauralRenderMode: {}",
            if *label == "LFE" { "off" } else { "undefined" }).unwrap();
    }
    out
}
pub fn head_tracking_name(mode: HeadTracking) -> &'static str {
    match mode {
        HeadTracking::SceneRelative => "scene relative",
        HeadTracking::HeadRelative => "head relative",
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
