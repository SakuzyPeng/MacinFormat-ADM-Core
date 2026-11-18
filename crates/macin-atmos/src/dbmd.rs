//! Logic 兼容 dbmd 生成。

use crate::{
    BinauralMode, DownmixMode, Error, FrameRate, HeadTracking, MasterSpec, StaticChannelMetadata,
    TrimConfiguration, TrimValues, WarpMode,
};
use std::collections::BTreeMap;
pub const DBMD_VERSION: [u8; 4] = [0x06, 0x00, 0x00, 0x01];
const DBMD_ATMOS_SUPPLEMENTAL_SYNC: [u8; 4] = [0xbd, 0x6f, 0x72, 0xf8];
const CONFIG_BITS: [u16; 9] = [1, 2, 4, 8, 16, 32, 64, 128, 1024];

/// 已验证的 DBMD 参数。创建者信息不继承到新成品。
#[derive(Debug, Clone)]
pub struct ReadMetadata {
    pub frame_rate: Option<FrameRate>,
    pub sc_bed_configuration: Vec<u8>,
    pub downmix_mode: DownmixMode,
    pub surround_phase_shift: bool,
    pub bed_distribution: bool,
    pub bed_distribution_present: bool,
    pub warp_mode: WarpMode,
    pub trim_mode: Option<[Option<TrimValues>; 4]>,
    pub channels: Option<Vec<StaticChannelMetadata>>,
    pub unsupported: Vec<(&'static str, String)>,
}

fn segments(data: &[u8]) -> crate::Result<BTreeMap<u8, &[u8]>> {
    if data.get(..4) != Some(DBMD_VERSION.as_slice()) {
        return Err(Error::new(
            crate::ErrorKind::Unsupported,
            "Unsupported DBMD version",
        ));
    }
    let mut result = BTreeMap::new();
    let mut at = 4;
    loop {
        let id = *data
            .get(at)
            .ok_or_else(|| Error::invalid("DBMD has no terminator"))?;
        if id == 0 {
            if data[at..].iter().any(|v| *v != 0) {
                return Err(Error::invalid("Data after DBMD terminator"));
            }
            return Ok(result);
        }
        let header = data
            .get(at + 1..at + 3)
            .ok_or_else(|| Error::invalid("Truncated DBMD segment header"))?;
        let size = usize::from(u16::from_le_bytes([header[0], header[1]]));
        let payload = data
            .get(at + 3..at + 3 + size)
            .ok_or_else(|| Error::invalid("Truncated DBMD segment"))?;
        let checksum = *data
            .get(at + 3 + size)
            .ok_or_else(|| Error::invalid("Missing DBMD checksum"))?;
        let sum = payload
            .iter()
            .fold(header[0].wrapping_add(checksum), |sum, v| {
                sum.wrapping_add(*v)
            });
        if sum != 0 {
            return Err(Error::invalid("Invalid DBMD checksum"));
        }
        if result.insert(id, payload).is_some() {
            return Err(Error::invalid("Duplicate DBMD segment"));
        }
        at += size + 4;
    }
}

fn frame_rate(code: u8) -> Option<FrameRate> {
    Some(match code {
        0x21 => FrameRate::Fps23976,
        0x22 => FrameRate::Fps24,
        0x23 => FrameRate::Fps25,
        0x24 => FrameRate::Fps2997Drop,
        0x25 => FrameRate::Fps2997,
        0x26 => FrameRate::Fps30,
        _ => return None,
    })
}

pub fn read(data: &[u8], channel_count: usize) -> crate::Result<ReadMetadata> {
    if channel_count > 128 {
        return Err(Error::invalid("Too many DBMD channels"));
    }
    let parts = segments(data)?;
    let baseline =
        build_logic_dbmd(channel_count, FrameRate::Fps24, "", "").map_err(Error::invalid)?;
    let defaults = segments(&baseline)?;
    let mut result = ReadMetadata {
        frame_rate: None,
        sc_bed_configuration: vec![3],
        downmix_mode: DownmixMode::LoRo,
        surround_phase_shift: false,
        bed_distribution: false,
        bed_distribution_present: false,
        warp_mode: WarpMode::LoRo,
        trim_mode: None,
        channels: None,
        unsupported: Vec::new(),
    };
    for (&id, payload) in &parts {
        match id {
            7 if payload.len() == 96 => {
                if *payload != defaults[&7] {
                    result.unsupported.push((
                        "dbmd_encoding",
                        "DBMD encoding parameters are not represented by this model".into(),
                    ));
                }
            }
            9 if payload.len() == 248 => {
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
                result.downmix_mode = match (payload[139] >> 4) & 3 {
                    1 => DownmixMode::LoRo,
                    2 => DownmixMode::LtRtProLogic,
                    3 => DownmixMode::LtRtPlII,
                    _ => {
                        result
                            .unsupported
                            .push(("downmix_type", "Unknown DBMD downmix mode".into()));
                        DownmixMode::LoRo
                    }
                };
                result.surround_phase_shift = payload[139] & 1 != 0;
                result.bed_distribution_present = payload[152] & 0x80 == 0;
                result.bed_distribution =
                    result.bed_distribution_present && payload[152] & 0x40 != 0;
                result.warp_mode = match payload[152] & 3 {
                    0 => WarpMode::Normal,
                    1 => WarpMode::Warping,
                    2 => WarpMode::ProLogicIIx,
                    _ => WarpMode::LoRo,
                };
                let unknown = payload[139] & !0x31 != 0
                    || payload[152] & !0xc3 != 0
                    || payload
                        .iter()
                        .zip(defaults[&9])
                        .enumerate()
                        .any(|(i, (a, b))| {
                            i >= 111 && ![111, 135, 136, 139, 152].contains(&i) && a != b
                        });
                if unknown {
                    result.unsupported.push((
                        "dbmd_encoding",
                        "Unmapped DBMD Atmos encoding parameters".into(),
                    ));
                }
            }
            10 => {
                if payload.len() < 142 || payload[..4] != DBMD_ATMOS_SUPPLEMENTAL_SYNC {
                    return Err(Error::invalid("Invalid DBMD supplemental header"));
                }
                let count = usize::from(u16::from_le_bytes([payload[4], payload[5]]));
                if count != channel_count || payload.len() != 142 + count * 2 {
                    return Err(Error::invalid(
                        "DBMD channel count or supplemental length differs from audio",
                    ));
                }
                if payload[6] != 0 {
                    result
                        .unsupported
                        .push(("dbmd_encoding", "Unknown supplemental flags".into()));
                }
                let mut trims = [None; 4];
                for index in 0..9 {
                    let at = 7 + 15 * index;
                    if payload[at] > 1 {
                        return Err(Error::invalid("Invalid DBMD automatic-trim flag"));
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
                    if payload[at] == 1 {
                        continue;
                    }
                    let Some(configuration) = TrimConfiguration::ALL
                        .into_iter()
                        .find(|c| c.dbmd_index() == index)
                    else {
                        result.unsupported.push((
                            "trim_mode",
                            format!("Unmapped manual trim configuration {index}"),
                        ));
                        continue;
                    };
                    if payload[at + 13] == 0 || payload[at + 14] == 0 {
                        return Err(Error::invalid("Invalid DBMD manual balance"));
                    }
                    trims[configuration.index()] = Some(TrimValues {
                        center: values[0],
                        surround: values[1],
                        height: values[2],
                        front_back_overhead_floor: (f64::from(payload[at + 13]) - 128.) / 127.,
                        front_back_listener: (f64::from(payload[at + 14]) - 128.) / 127.,
                    });
                }
                result.trim_mode = Some(trims);
                let mut channels = Vec::with_capacity(count);
                for i in 0..count {
                    let trim = payload[142 + i];
                    if trim > 1 {
                        return Err(Error::invalid("Invalid DBMD trimBypass"));
                    }
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
                        4 => BinauralMode::Undefined,
                        _ => {
                            result.unsupported.push((
                                "binaural_mode",
                                format!("Unknown DBMD binaural mode on channel {}", i + 1),
                            ));
                            BinauralMode::Undefined
                        }
                    };
                    if value & 0x38 != 0 {
                        result.unsupported.push((
                            "dbmd_encoding",
                            format!("Unknown DBMD flags on channel {}", i + 1),
                        ));
                    }
                    channels.push(StaticChannelMetadata {
                        head_tracking,
                        binaural_mode,
                        trim_bypass: trim != 0,
                    });
                }
                result.channels = Some(channels);
            }
            7 | 9 => return Err(Error::invalid("Invalid DBMD segment length")),
            _ => result
                .unsupported
                .push(("dbmd_encoding", format!("Unmapped DBMD segment {id}"))),
        }
    }
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
    let baseline = build_logic_dbmd(
        spec.channels(),
        spec.frame_rate,
        &spec.dbmd_originator,
        &spec.dbmd_product,
    )
    .map_err(Error::invalid)?;
    let parts = segments(&baseline)?;
    let mut atmos = parts[&9].to_vec();
    let mask = spec
        .sc_bed_configuration
        .iter()
        .fold(0, |mask, c| mask | CONFIG_BITS[usize::from(*c)]);
    atmos[135..137].copy_from_slice(&u16::to_le_bytes(mask));
    atmos[139] = match spec.downmix_mode {
        DownmixMode::LoRo => 0x10,
        DownmixMode::LtRtProLogic => 0x20,
        DownmixMode::LtRtPlII => 0x30,
    } | u8::from(spec.surround_phase_shift);
    atmos[152] = if !spec.bed_distribution_present {
        0x80
    } else if spec.bed_distribution {
        0x40
    } else {
        0
    } | match spec.warp_mode {
        WarpMode::Normal => 0,
        WarpMode::Warping => 1,
        WarpMode::ProLogicIIx => 2,
        WarpMode::LoRo => 3,
    };
    let mut supplemental = parts[&10].to_vec();
    for config in TrimConfiguration::ALL {
        let at = 7 + config.dbmd_index() * 15;
        supplemental[at..at + 15].fill(0);
        if let Some(values) = spec.trim_mode[config.index()] {
            values.validate()?;
            for (i, value) in [values.center, values.surround, values.height]
                .into_iter()
                .enumerate()
            {
                supplemental[at + 1 + i * 4..at + 5 + i * 4]
                    .copy_from_slice(&(value as f32).to_le_bytes());
            }
            supplemental[at + 13] =
                (128. + (values.front_back_overhead_floor * 127.).round()) as u8;
            supplemental[at + 14] = (128. + (values.front_back_listener * 127.).round()) as u8;
        } else {
            supplemental[at] = 1;
        }
    }
    for (i, state) in channels.iter().enumerate() {
        supplemental[142 + i] = u8::from(state.trim_bypass);
        supplemental[142 + channels.len() + i] = match state.head_tracking {
            HeadTracking::HeadRelative => 0,
            HeadTracking::SceneRelative => 0x40,
            HeadTracking::Undefined => 0x80,
        } | match state.binaural_mode {
            BinauralMode::Off => 0,
            BinauralMode::Near => 1,
            BinauralMode::Far => 2,
            BinauralMode::Undefined => 4,
            BinauralMode::Mid => {
                return Err(Error::new(
                    crate::ErrorKind::Unsupported,
                    "Mid has no verified DBMD encoding",
                ));
            }
        };
    }
    let mut out = DBMD_VERSION.to_vec();
    for (id, payload) in [
        (7, parts[&7]),
        (9, atmos.as_slice()),
        (10, supplemental.as_slice()),
    ] {
        append_dbmd_segment(&mut out, id, payload).map_err(Error::invalid)?;
    }
    out.push(0);
    if out.len() & 1 != 0 {
        out.push(0);
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

    // EBU Tech 3285 Supplement 6 定义 segment 头、终止标记和校验和；7/9/10
    // 是 Logic 互操作配置所需的私有负载。这里只写项目夹具与结构判据已覆盖的
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
    for (offset, value) in [
        (97usize, 0x01u8), // content creation tool 0.1.0
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
    out.push(id);
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(payload);
    let size_low = size.to_le_bytes().first().copied().unwrap_or(0);
    let sum = payload
        .iter()
        .fold(size_low, |sum, byte| sum.wrapping_add(*byte));
    out.push(0u8.wrapping_sub(sum));
    Ok(())
}
