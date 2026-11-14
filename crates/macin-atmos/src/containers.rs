//! 已知元数据的批量容器入口与流式 CAF 头。
use crate::{Error, Profile, Result};
use std::fs::File;
use std::io::{BufWriter, Seek, Write};
const BYTES_PER_SAMPLE: u64 = 3;
fn adm_io_error(error: std::io::Error) -> String {
    format!("Failed to write ADM BWF: {error}")
}
pub struct AdmWaveSpec<'a> {
    pub frames: u64,
    pub channels: usize,
    pub sample_rate: u32,
    pub axml: &'a [u8],
    pub chna: &'a [u8],
    pub dbmd: Option<&'a [u8]>,
    pub compatibility: Profile,
}

pub fn write_adm_wave_payload<F>(
    file: File,
    spec: AdmWaveSpec<'_>,
    write_pcm_payload: F,
) -> std::result::Result<(), String>
where
    F: FnOnce(&mut BufWriter<File>) -> std::result::Result<(), String>,
{
    let AdmWaveSpec {
        frames,
        channels,
        sample_rate,
        axml,
        chna,
        dbmd,
        compatibility,
    } = spec;
    let channels_u16 = u16::try_from(channels).map_err(|_| "ADM BWF channel count exceeds u16")?;
    let block_align_u64 = u64::from(channels_u16)
        .checked_mul(BYTES_PER_SAMPLE)
        .ok_or("ADM BWF blockAlign overflow")?;
    let block_align =
        u16::try_from(block_align_u64).map_err(|_| "ADM BWF blockAlign exceeds u16")?;
    let bytes_per_second = sample_rate
        .checked_mul(u32::from(block_align))
        .ok_or("ADM BWF bytesPerSecond overflow")?;
    let data_size = frames
        .checked_mul(block_align_u64)
        .ok_or("ADM BWF data-size overflow")?;
    let axml_size = u64::try_from(axml.len()).map_err(|_| "AXML size exceeds u64")?;
    let chna_size = u64::try_from(chna.len()).map_err(|_| "CHNA size exceeds u64")?;
    let dbmd_size = dbmd
        .map(|value| u64::try_from(value.len()).map_err(|_| "DBMD size exceeds u64"))
        .transpose()?;
    let dbmd_total = dbmd_size.map(chunk_total).transpose()?.unwrap_or(0);
    let axml_uses_ds64 = axml_size >= u64::from(u32::MAX);
    let ds64_payload_size = 28u64
        .checked_add(if axml_uses_ds64 { 12 } else { 0 })
        .ok_or("ds64 size overflow")?;
    let total_size = 12u64
        .checked_add(chunk_total(ds64_payload_size)?)
        .and_then(|value| value.checked_add(24))
        .and_then(|value| value.checked_add(chunk_total(chna_size).ok()?))
        .and_then(|value| value.checked_add(chunk_total(axml_size).ok()?))
        .and_then(|value| value.checked_add(dbmd_total))
        .and_then(|value| value.checked_add(chunk_total(data_size).ok()?))
        .ok_or("ADM BWF file-size overflow")?;
    let riff_size = total_size
        .checked_sub(8)
        .ok_or("ADM BWF outer-size underflow")?;

    let mut writer = BufWriter::new(file);
    writer
        .write_all(compatibility.container_id())
        .map_err(adm_io_error)?;
    writer
        .write_all(&u32::MAX.to_le_bytes())
        .map_err(adm_io_error)?;
    writer.write_all(b"WAVE").map_err(adm_io_error)?;

    writer.write_all(b"ds64").map_err(adm_io_error)?;
    writer
        .write_all(
            &u32::try_from(ds64_payload_size)
                .map_err(|_| "ds64 payload exceeds u32")?
                .to_le_bytes(),
        )
        .map_err(adm_io_error)?;
    writer
        .write_all(&riff_size.to_le_bytes())
        .map_err(adm_io_error)?;
    writer
        .write_all(&data_size.to_le_bytes())
        .map_err(adm_io_error)?;
    let sample_count = match compatibility {
        Profile::Standard => 0,
        _ => frames,
    };
    writer
        .write_all(&sample_count.to_le_bytes())
        .map_err(adm_io_error)?;
    writer
        .write_all(&u32::from(axml_uses_ds64).to_le_bytes())
        .map_err(adm_io_error)?;
    if axml_uses_ds64 {
        writer.write_all(b"axml").map_err(adm_io_error)?;
        writer
            .write_all(&axml_size.to_le_bytes())
            .map_err(adm_io_error)?;
    }

    writer.write_all(b"fmt ").map_err(adm_io_error)?;
    writer
        .write_all(&16u32.to_le_bytes())
        .map_err(adm_io_error)?;
    writer
        .write_all(&1u16.to_le_bytes())
        .map_err(adm_io_error)?;
    writer
        .write_all(&channels_u16.to_le_bytes())
        .map_err(adm_io_error)?;
    writer
        .write_all(&sample_rate.to_le_bytes())
        .map_err(adm_io_error)?;
    writer
        .write_all(&bytes_per_second.to_le_bytes())
        .map_err(adm_io_error)?;
    writer
        .write_all(&block_align.to_le_bytes())
        .map_err(adm_io_error)?;
    writer
        .write_all(&24u16.to_le_bytes())
        .map_err(adm_io_error)?;

    write_chunk(&mut writer, b"chna", chna, false)?;
    write_chunk(&mut writer, b"axml", axml, axml_uses_ds64)?;
    if let Some(dbmd) = dbmd {
        write_chunk(&mut writer, b"dbmd", dbmd, false)?;
    }
    writer.write_all(b"data").map_err(adm_io_error)?;
    writer
        .write_all(&u32::MAX.to_le_bytes())
        .map_err(adm_io_error)?;
    let before = writer.stream_position().map_err(adm_io_error)?;
    write_pcm_payload(&mut writer)?;
    let actual = writer
        .stream_position()
        .map_err(adm_io_error)?
        .checked_sub(before)
        .ok_or("PCM position moved backwards")?;
    if actual != data_size {
        return Err(format!(
            "PCM byte count {actual} does not match the declared {data_size}"
        ));
    }
    if data_size & 1 != 0 {
        writer.write_all(&[0]).map_err(adm_io_error)?;
    }
    writer.flush().map_err(adm_io_error)?;
    let file = writer
        .into_inner()
        .map_err(|error| format!("Failed to finish writing ADM BWF: {}", error.error()))?;
    file.sync_all()
        .map_err(|error| format!("Failed to sync ADM BWF: {error}"))
}

pub fn chunk_total(payload: u64) -> std::result::Result<u64, String> {
    8u64.checked_add(payload)
        .and_then(|value| value.checked_add(payload & 1))
        .ok_or("RIFF chunk-size overflow".to_owned())
}

pub fn write_chunk<W: Write>(
    writer: &mut W,
    id: &[u8; 4],
    data: &[u8],
    uses_ds64: bool,
) -> std::result::Result<(), String> {
    writer.write_all(id).map_err(adm_io_error)?;
    let size = if uses_ds64 {
        u32::MAX
    } else {
        u32::try_from(data.len()).map_err(|_| "RIFF chunk payload exceeds u32")?
    };
    writer
        .write_all(&size.to_le_bytes())
        .map_err(adm_io_error)?;
    writer.write_all(data).map_err(adm_io_error)?;
    if data.len() & 1 != 0 {
        writer.write_all(&[0]).map_err(adm_io_error)?;
    }
    Ok(())
}

/// Signed packed S24LE CAF. None reserves unknown duration for final backpatching.
pub fn write_caf_header(
    out: &mut impl Write,
    frames: Option<u64>,
    channels: usize,
    rate: u32,
) -> Result<()> {
    if channels == 0 || channels > 128 || !matches!(rate, 48_000 | 96_000) {
        return Err(Error::invalid("Invalid Atmos CAF format"));
    }
    let align = (channels as u64) * 3;
    let size = match frames {
        None => -1,
        Some(frames) => i64::try_from(
            frames
                .checked_mul(align)
                .and_then(|v| v.checked_add(4))
                .ok_or_else(|| Error::invalid("CAF size overflow"))?,
        )
        .map_err(|_| Error::invalid("CAF size exceeds i64"))?,
    };
    out.write_all(b"caff")?;
    out.write_all(&1u16.to_be_bytes())?;
    out.write_all(&0u16.to_be_bytes())?;
    out.write_all(b"desc")?;
    out.write_all(&32i64.to_be_bytes())?;
    out.write_all(&f64::from(rate).to_bits().to_be_bytes())?;
    out.write_all(b"lpcm")?;
    for value in [2, align as u32, 1, channels as u32, 24] {
        out.write_all(&value.to_be_bytes())?;
    }
    out.write_all(b"data")?;
    out.write_all(&size.to_be_bytes())?;
    out.write_all(&0u32.to_be_bytes())?;
    Ok(())
}
