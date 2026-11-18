//! Logic 兼容 dbmd 生成。

use crate::{
    BinauralMode, DownmixMode, Error, FrameRate, HeadTracking, MasterSpec, StaticChannelMetadata,
    TrimConfiguration, TrimValues, WarpMode,
};
mod segments;
mod sidecar;
pub use segments::DbmdSource;
pub use sidecar::{DbmdSidecar, DdPlusCompression};
pub const DBMD_VERSION: [u8; 4] = [0x06, 0x00, 0x00, 0x01];
pub const DBMD_LIMIT: usize = 1 << 20;
const DBMD_ATMOS_SUPPLEMENTAL_SYNC: [u8; 4] = [0xbd, 0x6f, 0x72, 0xf8];
const CONFIG_BITS: [u16; 9] = [1, 2, 4, 8, 16, 32, 64, 128, 1024];

/// 已验证的 DBMD 参数。创建者信息不继承到新成品。
#[derive(Debug, Clone)]
pub struct ReadMetadata {
    pub frame_rate: Option<FrameRate>,
    pub first_action_ns: Option<i64>,
    pub ddplus_compression: DdPlusCompression,
    pub source: DbmdSource,
    pub sc_bed_configuration: Vec<u8>,
    pub downmix_mode: DownmixMode,
    pub surround_phase_shift: bool,
    pub surround_phase_shift_present: bool,
    pub bed_distribution: bool,
    pub bed_distribution_present: bool,
    pub warp_mode: WarpMode,
    pub trim_mode: Option<[Option<TrimValues>; 9]>,
    pub channels: Option<Vec<StaticChannelMetadata>>,
    pub unsupported: Vec<(&'static str, String)>,
}

fn first_action(bytes: &[u8]) -> crate::Result<Option<i64>> {
    if bytes[0] & 0x80 != 0 {
        return Ok(None);
    }
    let hours = i64::from(bytes[0]);
    let minutes = i64::from(bytes[1]);
    let fraction = i64::from(u32::from_le_bytes(bytes[2..6].try_into().unwrap()));
    if minutes >= 60 || fraction >= 6_000_000 {
        return Err(Error::invalid("Invalid DBMD first frame of action time"));
    }
    Ok(Some(
        ((hours * 60 + minutes) * 6_000_000 + fraction) * 10_000,
    ))
}

pub(crate) fn action_ticks(ns: i64) -> Option<u64> {
    let ticks = (u64::try_from(ns).ok()? + 5_000) / 10_000;
    (ticks < 128 * 360_000_000).then_some(ticks)
}

fn write_first_action(ns: Option<i64>, bytes: &mut [u8]) -> crate::Result<()> {
    bytes.fill(0);
    let Some(ns) = ns else {
        bytes[0] = 0xff;
        return Ok(());
    };
    let ticks = action_ticks(ns).ok_or_else(|| {
        Error::new(
            crate::ErrorKind::Unsupported,
            "First frame of action is outside the DBMD clock",
        )
    })?;
    bytes[0] = (ticks / 360_000_000) as u8;
    bytes[1] = ((ticks / 6_000_000) % 60) as u8;
    bytes[2..6].copy_from_slice(&((ticks % 6_000_000) as u32).to_le_bytes());
    Ok(())
}

fn balance(byte: u8) -> f64 {
    let magnitude = f64::from(byte & 0x7f) / 127.;
    if byte & 0x80 == 0 {
        -magnitude
    } else {
        magnitude
    }
}

fn balance_code(value: f64) -> u8 {
    let magnitude = (value.abs() * 127.).round() as u8;
    if value < 0. && magnitude != 0 {
        magnitude
    } else {
        0x80 | magnitude
    }
}

fn frame_rate(code: u8) -> Option<FrameRate> {
    Some(match code & 0x0f {
        1 => FrameRate::Fps23976,
        2 => FrameRate::Fps24,
        3 => FrameRate::Fps25,
        4 => FrameRate::Fps2997Drop,
        5 => FrameRate::Fps2997,
        6 => FrameRate::Fps30,
        _ => return None,
    })
}

pub fn read(data: &[u8], channel_count: usize) -> crate::Result<ReadMetadata> {
    if channel_count > 128 {
        return Err(Error::invalid("Too many DBMD channels"));
    }
    let source = DbmdSource::parse(data)?;
    let baseline =
        build_logic_dbmd(channel_count, FrameRate::Fps24, "", "").map_err(Error::invalid)?;
    let defaults = DbmdSource::parse(&baseline)?;
    let mut extra_encoding = source.extra_encoding;
    let mut have_program_zero = false;
    let mut result = ReadMetadata {
        frame_rate: None,
        first_action_ns: None,
        ddplus_compression: DdPlusCompression::default(),
        source,
        sc_bed_configuration: vec![3],
        downmix_mode: DownmixMode::NotIndicated,
        surround_phase_shift: false,
        surround_phase_shift_present: false,
        bed_distribution: false,
        bed_distribution_present: false,
        warp_mode: WarpMode::NotIndicated,
        trim_mode: None,
        channels: None,
        unsupported: Vec::new(),
    };
    for (id, payload) in result.source.segments() {
        match id {
            7 => {
                let baseline = defaults.segment(7).unwrap();
                if payload[0] == 0 && !have_program_zero {
                    result.ddplus_compression = DdPlusCompression {
                        compr1: payload[14],
                        dynrng1: payload[15],
                    };
                    have_program_zero = true;
                }
                extra_encoding |=
                    payload[..14] != baseline[..14] || payload[16..] != baseline[16..];
            }
            9 => {
                result.first_action_ns = first_action(&payload[112..118])?;
                result.frame_rate = frame_rate(payload[111]);
                if result.frame_rate.is_none() {
                    result.unsupported.push((
                        "frame_rate",
                        format!("Unknown DBMD frame-rate code {}", payload[111]),
                    ));
                }
                let mask = u16::from_le_bytes([payload[135], payload[136]]);
                if mask & !0x04ff != 0 {
                    result.unsupported.push((
                        "sc_bed_configuration",
                        "Unknown DBMD bed configuration bits".into(),
                    ));
                }
                result.sc_bed_configuration = CONFIG_BITS
                    .iter()
                    .enumerate()
                    .filter_map(|(i, bit)| (mask & bit != 0).then_some(i as u8))
                    .collect();
                result.downmix_mode = match (payload[139] >> 4) & 7 {
                    0 => DownmixMode::NotIndicated,
                    1 => DownmixMode::LoRo,
                    2 => DownmixMode::LtRtProLogic,
                    3 => DownmixMode::LtRtPlII,
                    4 => DownmixMode::DirectRender,
                    _ => {
                        result
                            .unsupported
                            .push(("downmix_type", "Unknown DBMD downmix mode".into()));
                        DownmixMode::LoRo
                    }
                };
                let phase = payload[139] & 3;
                result.surround_phase_shift = phase == 1;
                result.surround_phase_shift_present = phase < 2;
                if phase == 2 {
                    result.unsupported.push((
                        "ls_rs_phase_shift",
                        "Invalid DBMD phase shift, using not indicated".into(),
                    ));
                }
                result.bed_distribution_present = payload[152] & 0x80 == 0;
                result.bed_distribution =
                    result.bed_distribution_present && payload[152] & 0x40 != 0;
                result.warp_mode = match payload[152] & 7 {
                    0 => WarpMode::Normal,
                    1 => WarpMode::Warping,
                    2 => WarpMode::ProLogicIIx,
                    3 => WarpMode::LoRo,
                    4 => WarpMode::NotIndicated,
                    _ => {
                        result
                            .unsupported
                            .push(("warp_mode", "Unknown DBMD warp mode".into()));
                        WarpMode::NotIndicated
                    }
                };
                extra_encoding |= payload[139] & !0x73 != 0
                    || payload[152] & !0xc7 != 0
                    || payload
                        .iter()
                        .zip(defaults.segment(9).unwrap())
                        .enumerate()
                        .any(|(i, (a, b))| {
                            i >= 118 && ![135, 136, 139, 152].contains(&i) && a != b
                        });
            }
            10 => {
                if payload.len() < 142 || payload[..4] != DBMD_ATMOS_SUPPLEMENTAL_SYNC {
                    return Err(Error::invalid("Invalid DBMD supplemental header"));
                }
                let count = usize::from(u16::from_le_bytes([payload[4], payload[5]]));
                if (count != 0 && count != channel_count) || payload.len() < 142 + count * 2 {
                    return Err(Error::invalid(
                        "DBMD channel count or supplemental length differs from audio",
                    ));
                }
                extra_encoding |= payload[6] != 0 || payload.len() > 142 + count * 2;
                let mut trims = [None; 9];
                for config in TrimConfiguration::ALL {
                    let at = 7 + 15 * config.dbmd_index();
                    extra_encoding |= payload[at] & !1 != 0;
                    if payload[at] & 1 != 0 {
                        continue;
                    }
                    let mut values = [0.; 3];
                    for (i, value) in values.iter_mut().enumerate() {
                        let start = at + 1 + i * 4;
                        *value = f64::from(f32::from_le_bytes(
                            payload[start..start + 4].try_into().unwrap(),
                        ));
                        if !value.is_finite() {
                            return Err(Error::invalid("Non-finite DBMD trim"));
                        }
                    }
                    trims[config.index()] = Some(TrimValues {
                        center: values[0],
                        surround: values[1],
                        height: values[2],
                        front_back_overhead_floor: balance(payload[at + 13]),
                        front_back_listener: balance(payload[at + 14]),
                    });
                }
                result.trim_mode = Some(trims);
                let mut channels = Vec::with_capacity(count);
                for i in 0..count {
                    let trim = payload[142 + i];
                    if trim & 3 == 3 {
                        return Err(Error::invalid("Invalid DBMD trimBypass"));
                    }
                    extra_encoding |= trim & !3 != 0;
                    let value = payload[142 + count + i];
                    let head_tracking = match value & 0xc0 {
                        0 => HeadTracking::HeadRelative,
                        0x40 => HeadTracking::SceneRelative,
                        0x80 => HeadTracking::Undefined,
                        _ => {
                            result.unsupported.push((
                                "head_tracking",
                                format!("Unknown DBMD head mode on channel {}", i + 1),
                            ));
                            HeadTracking::Undefined
                        }
                    };
                    let binaural_mode = match value & 7 {
                        0 => BinauralMode::Off,
                        1 => BinauralMode::Near,
                        2 => BinauralMode::Far,
                        3 => BinauralMode::Mid,
                        4 => BinauralMode::Undefined,
                        _ => {
                            result.unsupported.push((
                                "binaural_mode",
                                format!("Unknown DBMD binaural mode on channel {}", i + 1),
                            ));
                            BinauralMode::Undefined
                        }
                    };
                    extra_encoding |= value & 0x38 != 0;
                    channels.push(StaticChannelMetadata {
                        head_tracking,
                        binaural_mode,
                        trim_bypass: trim & 3 == 1,
                        trim_bypass_unspecified: trim & 3 == 2,
                    });
                }
                result.channels = Some(channels);
            }
            _ => {}
        }
    }
    if result.source.segments().filter(|p| p.0 == 7).count() > 1 {
        extra_encoding = true;
    }
    result.source.extra_encoding = extra_encoding;
    Ok(result)
}

pub fn build_from_spec(
    spec: &MasterSpec,
    channels: &[StaticChannelMetadata],
) -> crate::Result<Vec<u8>> {
    spec.validate()?;
    if channels.len() != spec.channels() {
        return Err(Error::invalid(
            "DBMD channel metadata count differs from audio",
        ));
    }
    if channels
        .iter()
        .any(|s| s.trim_bypass && s.trim_bypass_unspecified)
    {
        return Err(Error::invalid("Conflicting DBMD trim bypass values"));
    }
    if spec.dbmd_source.as_ref().is_some_and(|s| {
        s.extra_encoding && s.origin.as_ref().is_some_and(|o| !o.matches(spec, None))
    }) {
        return Err(Error::new(
            crate::ErrorKind::Unsupported,
            "DBMD encoding data does not match the source layout",
        ));
    }
    let baseline = build_logic_dbmd(
        spec.channels(),
        spec.frame_rate,
        &spec.dbmd_originator,
        &spec.dbmd_product,
    )
    .map_err(Error::invalid)?;
    let defaults = DbmdSource::parse(&baseline)?;
    let source = spec.dbmd_source.as_ref().unwrap_or(&defaults);
    let default_atmos = defaults.segment(9).unwrap();
    let original_atmos = source.segment(9);
    let mut atmos = original_atmos.unwrap_or(default_atmos).to_vec();
    atmos[..99].copy_from_slice(&default_atmos[..99]);
    atmos[111] = (atmos[111] & 0xf0) | (spec.frame_rate.dbmd_code() & 15);
    if first_action(&atmos[112..118])? != spec.first_action_ns {
        write_first_action(spec.first_action_ns, &mut atmos[112..118])?;
    }
    let mask = spec
        .sc_bed_configuration
        .iter()
        .fold(0, |mask, c| mask | CONFIG_BITS[usize::from(*c)]);
    atmos[135..137].copy_from_slice(&u16::to_le_bytes(mask));
    atmos[139] = (atmos[139] & !0x73)
        | match spec.downmix_mode {
            DownmixMode::NotIndicated => 0,
            DownmixMode::DirectRender => 0x40,
            DownmixMode::LoRo => 0x10,
            DownmixMode::LtRtProLogic => 0x20,
            DownmixMode::LtRtPlII => 0x30,
        }
        | if spec.surround_phase_shift_present {
            u8::from(spec.surround_phase_shift)
        } else {
            3
        };
    atmos[152] = (atmos[152] & 0x38)
        | if !spec.bed_distribution_present {
            0x80 | (atmos[152] & 0x40)
        } else if spec.bed_distribution {
            0x40
        } else {
            0
        }
        | match spec.warp_mode {
            WarpMode::Normal => 0,
            WarpMode::Warping => 1,
            WarpMode::ProLogicIIx => 2,
            WarpMode::LoRo => 3,
            WarpMode::NotIndicated => 4,
        };
    let mut supplemental = defaults.segment(10).unwrap().to_vec();
    let original_sup = source.segment(10);
    let original_count = original_sup.map_or(0, |s| usize::from(u16::from_le_bytes([s[4], s[5]])));
    let count = if original_sup.is_some()
        && original_count == 0
        && channels
            .iter()
            .enumerate()
            .all(|(i, v)| *v == crate::BedState::for_channel(i).into())
    {
        0
    } else {
        channels.len()
    };
    supplemental.truncate(142 + 2 * count);
    supplemental[4..6].copy_from_slice(&(count as u16).to_le_bytes());
    if let Some(s) = original_sup {
        supplemental[6..142].copy_from_slice(&s[6..142]);
        supplemental.extend_from_slice(&s[142 + original_count * 2..]);
    }
    for config in TrimConfiguration::ALL {
        let at = 7 + config.dbmd_index() * 15;
        let was_auto = supplemental[at] & 1 != 0;
        if let Some(values) = spec.trim_mode[config.index()] {
            supplemental[at] &= !1;
            values.validate()?;
            for (i, value) in [values.center, values.surround, values.height]
                .into_iter()
                .enumerate()
            {
                supplemental[at + 1 + i * 4..at + 5 + i * 4]
                    .copy_from_slice(&(value as f32).to_le_bytes());
            }
            for (offset, value) in [
                (13, values.front_back_overhead_floor),
                (14, values.front_back_listener),
            ] {
                if was_auto || balance(supplemental[at + offset]) != value {
                    supplemental[at + offset] = balance_code(value);
                }
            }
        } else {
            supplemental[at] |= 1;
            if !was_auto {
                supplemental[at + 1..at + 15].fill(0);
            }
        }
    }
    for (i, state) in channels.iter().take(count).enumerate() {
        let (trim_flags, head_flags) = original_sup
            .filter(|_| original_count == channels.len())
            .map_or((0, 0), |s| {
                (s[142 + i] & !3, s[142 + original_count + i] & 0x38)
            });
        supplemental[142 + i] = trim_flags
            | if state.trim_bypass_unspecified {
                2
            } else {
                u8::from(state.trim_bypass)
            };
        supplemental[142 + channels.len() + i] = head_flags
            | match state.head_tracking {
                HeadTracking::HeadRelative => 0,
                HeadTracking::SceneRelative => 0x40,
                HeadTracking::Undefined => 0x80,
            }
            | match state.binaural_mode {
                BinauralMode::Off => 0,
                BinauralMode::Near => 1,
                BinauralMode::Far => 2,
                BinauralMode::Mid => 3,
                BinauralMode::Undefined => 4,
            };
    }
    let mut out = source.version.to_vec();
    let mut have_ddplus = false;
    for part in source.parts.iter() {
        let mut ddplus;
        let data = match part.id {
            7 if part.data[0] == 0 && !have_ddplus => {
                have_ddplus = true;
                ddplus = part.data.clone();
                ddplus[14] = spec.ddplus_compression.compr1;
                ddplus[15] = spec.ddplus_compression.dynrng1;
                &ddplus
            }
            9 => &atmos,
            10 => &supplemental,
            _ => &part.data,
        };
        let size = if part.size_code == 0 && data.len() == usize::from(u16::MAX) {
            0
        } else {
            u16::try_from(data.len()).map_err(|_| Error::invalid("DBMD segment exceeds u16"))?
        };
        segments::append(&mut out, part.id, size, data);
    }
    if !have_ddplus && spec.ddplus_compression != DdPlusCompression::default() {
        let mut ddplus = defaults.segment(7).unwrap().to_vec();
        ddplus[14..16].copy_from_slice(&[
            spec.ddplus_compression.compr1,
            spec.ddplus_compression.dynrng1,
        ]);
        append_dbmd_segment(&mut out, 7, &ddplus).map_err(Error::invalid)?;
    }
    if original_atmos.is_none()
        && (spec.frame_rate != FrameRate::Fps24
            || spec.dbmd_originator != crate::DEFAULT_DBMD_ORIGINATOR
            || spec.dbmd_product != crate::DEFAULT_DBMD_PRODUCT
            || spec.first_action_ns.is_some()
            || spec.sc_bed_configuration != [3]
            || spec.downmix_mode != DownmixMode::NotIndicated
            || spec.surround_phase_shift_present
            || spec.bed_distribution_present
            || spec.warp_mode != WarpMode::NotIndicated)
    {
        append_dbmd_segment(&mut out, 9, &atmos).map_err(Error::invalid)?;
    }
    if original_sup.is_none() && supplemental != defaults.segment(10).unwrap() {
        append_dbmd_segment(&mut out, 10, &supplemental).map_err(Error::invalid)?;
    }
    out.push(0);
    if out.len() & 1 != 0 {
        out.push(source.padding.unwrap_or(0));
    }
    if out.len() > DBMD_LIMIT {
        return Err(Error::invalid(
            "Output DBMD exceeds the 1 MiB metadata budget",
        ));
    }
    Ok(out)
}

pub fn build_logic_dbmd(
    channel_count: usize,
    frame_rate: FrameRate,
    originator: &str,
    product: &str,
) -> Result<Vec<u8>, String> {
    let channel_count =
        u16::try_from(channel_count).map_err(|_| "DBMD channel count exceeds u16")?;
    if channel_count > 128 {
        return Err("DBMD Atmos supplemental data supports at most 128 channels".to_owned());
    }

    let mut out = Vec::new();
    out.extend_from_slice(&DBMD_VERSION);

    // EBU Tech 3285 Supplement 6 定义 segment 头、终止标记和校验和；第 7 段是公开的 DD+ 元数据，
    // 第 9、10 段为 Atmos 扩展。这里只写项目夹具与结构判据已覆盖的
    // 字段。创建来源与产品使用调用方提供的固定宽度 ASCII 文本。
    let mut ddplus = [0u8; 96];
    for (offset, value) in [
        (1usize, 0x47u8),
        (5, 0x60),
        (8, 0x24),
        (9, 0x24),
        (14, 0x02),
        (15, 0x02),
    ] {
        set_dbmd_byte(&mut ddplus, offset, value)?;
    }
    append_dbmd_segment(&mut out, 7, &ddplus)?;

    let mut atmos = [0u8; 248];
    copy_dbmd_ascii(&mut atmos, 0, 32, originator)?;
    copy_dbmd_ascii(&mut atmos, 32, 64, product)?;
    for (byte, component) in atmos[96..99].iter_mut().zip([
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        env!("CARGO_PKG_VERSION_PATCH"),
    ]) {
        *byte = component
            .parse()
            .map_err(|_| "DBMD creator version exceeds its byte width")?;
    }
    for (offset, value) in [
        (111, frame_rate.dbmd_code()),
        (112, 0xff),
        (118, 0x03),
        (134, 0xf0),
        (135, 0x08),
        (139, 0x10),
        (152, 0x83), // 旧低层入口：未指定 bedDistribution，Lo/Ro warp。
        (168, 0xf0),
        (170, 0x08),
        (184, 0xf0),
        (186, 0x08),
        (200, 0xf0),
        (202, 0x08),
    ] {
        set_dbmd_byte(&mut atmos, offset, value)?;
    }
    append_dbmd_segment(&mut out, 9, &atmos)?;

    let channel_count_usize = usize::from(channel_count);
    let mut supplemental =
        Vec::with_capacity(142usize.saturating_add(channel_count_usize.saturating_mul(2)));
    supplemental.extend_from_slice(&DBMD_ATMOS_SUPPLEMENTAL_SYNC);
    supplemental.extend_from_slice(&channel_count.to_le_bytes());
    supplemental.push(0); // reserved
    for config in 0..9 {
        let auto_trim = matches!(config, 0 | 3 | 5 | 6 | 8);
        supplemental.push(u8::from(auto_trim));
        supplemental.extend_from_slice(&[0; 12]);
        if auto_trim {
            supplemental.extend_from_slice(&[0; 2]);
        } else {
            supplemental.extend_from_slice(&[0x80, 0x80]);
        }
    }
    supplemental.resize(supplemental.len().saturating_add(channel_count_usize), 0); // trim bypass
    for channel in 0..channel_count_usize {
        // lower三位是 binauralRenderMode；0x40 表示 scene-relative。
        supplemental.push(if channel == 3 { 0x40 } else { 0x44 });
    }
    append_dbmd_segment(&mut out, 10, &supplemental)?;

    out.push(0); // segment terminator
    if out.len() & 1 != 0 {
        out.push(0); // RIFF payload alignment
    }
    Ok(out)
}

pub fn set_dbmd_byte<const N: usize>(
    target: &mut [u8; N],
    offset: usize,
    value: u8,
) -> Result<(), String> {
    let byte = target
        .get_mut(offset)
        .ok_or("Internal DBMD field offset is out of bounds")?;
    *byte = value;
    Ok(())
}

pub fn copy_dbmd_ascii<const N: usize>(
    target: &mut [u8; N],
    offset: usize,
    width: usize,
    value: &str,
) -> Result<(), String> {
    if !value.is_ascii() || value.len() > width {
        return Err("Internal DBMD ASCII field is invalid".to_owned());
    }
    let end = offset
        .checked_add(value.len())
        .ok_or("Internal DBMD ASCII field-offset overflow")?;
    let field = target
        .get_mut(offset..end)
        .ok_or("Internal DBMD ASCII field is out of bounds")?;
    field.copy_from_slice(value.as_bytes());
    Ok(())
}

pub fn append_dbmd_segment(out: &mut Vec<u8>, id: u8, payload: &[u8]) -> Result<(), String> {
    let size = u16::try_from(payload.len()).map_err(|_| "DBMD segment exceeds u16")?;
    if id == 0 || payload.is_empty() {
        return Err("DBMD data segments require a nonzero ID and a nonempty payload".into());
    }
    segments::append(out, id, size, payload);
    Ok(())
}
