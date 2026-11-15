use crate::{Chna, Container, Error, Format, Result, SampleFormat, add, write_chunk};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

#[derive(Debug, Clone, Default)]
pub struct WriterOptions {
    /// None starts as RIFF and promotes integer PCM to BW64, float32 to RF64.
    pub container: Option<Container>,
    pub axml: Option<Vec<u8>>,
    pub chna: Option<Chna>,
}

/// A bounded metadata source appended after the PCM. The reader is consumed for exactly `size`
/// bytes; short input is an error. It need not be seekable or resident in memory.
pub struct TrailingChunk<'a> {
    pub id: [u8; 4],
    pub size: u64,
    pub source: &'a mut dyn Read,
}

/// Drop never finalizes a writer. Call finish and observe its result before installing output.
pub struct Writer<W> {
    sink: W,
    format: Format,
    container: Option<Container>,
    data_start: u64,
    data_bytes: u64,
    fact: Option<u64>,
    scratch: Vec<u8>,
    failed: bool,
    deferred: bool,
}
impl<W: Write + Seek> Writer<W> {
    pub fn new(sink: W, format: Format, options: WriterOptions) -> Result<Self> {
        Self::new_inner(sink, format, options, false)
    }
    /// Reserve space for a large trailing AXML chunk without moving the audio. Only this entry
    /// changes the historical writer layout. The extra 12-byte JUNK becomes a ds64 table entry
    /// when AXML needs 64-bit addressing (ITU-R BS.2088-2, Annex 1 §4).
    pub fn new_deferred(sink: W, format: Format, options: WriterOptions) -> Result<Self> {
        if !matches!(options.container, Some(Container::Bw64 | Container::Rf64)) {
            return Err(Error::invalid("Deferred metadata requires BW64 or RF64"));
        }
        Self::new_inner(sink, format, options, true)
    }
    fn new_inner(
        mut sink: W,
        format: Format,
        options: WriterOptions,
        deferred: bool,
    ) -> Result<Self> {
        let align = format.block_align()?;
        let byte_rate = format.byte_rate()?;
        if options.container == Some(Container::Bw64) && !format.sample_format.is_integer() {
            return Err(Error::unsupported("BW64 输出仅支持整数 PCM"));
        }
        if options
            .chna
            .as_ref()
            .is_some_and(|chna| chna.tracks != format.channels)
        {
            return Err(Error::invalid("CHNA 轨道数与 fmt 不一致"));
        }
        let chna = options.chna.as_ref().map(Chna::encode).transpose()?;
        if options
            .axml
            .as_ref()
            .is_some_and(|data| data.len() >= u32::MAX as usize)
        {
            return Err(Error::unsupported("AXML chunk 超出 uint32"));
        }
        if sink.seek(SeekFrom::End(0))? != 0 {
            return Err(Error::invalid("WAVE writer 需要空的输出流"));
        }
        sink.seek(SeekFrom::Start(0))?;
        sink.write_all(b"RIFF\0\0\0\0WAVE")?;
        write_chunk(&mut sink, *b"JUNK", &[0; 28])?;
        if deferred {
            write_chunk(&mut sink, *b"JUNK", &[0; 4])?;
        }
        let tag = if format.sample_format.is_integer() {
            1u16
        } else {
            3u16
        };
        let mut fmt = Vec::with_capacity(40);
        fmt.extend_from_slice(
            &if format.channel_mask.is_some() {
                0xfffeu16
            } else {
                tag
            }
            .to_le_bytes(),
        );
        fmt.extend_from_slice(&format.channels.to_le_bytes());
        fmt.extend_from_slice(&format.sample_rate.to_le_bytes());
        fmt.extend_from_slice(&byte_rate.to_le_bytes());
        fmt.extend_from_slice(&align.to_le_bytes());
        fmt.extend_from_slice(&format.sample_format.bits().to_le_bytes());
        if let Some(mask) = format.channel_mask {
            fmt.extend_from_slice(&22u16.to_le_bytes());
            fmt.extend_from_slice(&format.sample_format.bits().to_le_bytes());
            fmt.extend_from_slice(&mask.to_le_bytes());
            fmt.extend_from_slice(&tag.to_le_bytes());
            fmt.extend_from_slice(&[0, 0, 0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113]);
        }
        write_chunk(&mut sink, *b"fmt ", &fmt)?;
        let fact = if format.sample_format == SampleFormat::Float32 {
            let offset = add(sink.stream_position()?, 8)?;
            write_chunk(&mut sink, *b"fact", &[0; 4])?;
            Some(offset)
        } else {
            None
        };
        if let Some(data) = chna {
            write_chunk(&mut sink, *b"chna", &data)?;
        }
        if let Some(data) = options.axml {
            write_chunk(&mut sink, *b"axml", &data)?;
        }
        sink.write_all(b"data\0\0\0\0")?;
        let data_start = sink.stream_position()?;
        Ok(Self {
            sink,
            format,
            container: options.container,
            data_start,
            data_bytes: 0,
            fact,
            scratch: Vec::new(),
            failed: false,
            deferred,
        })
    }
    pub fn format(&self) -> Format {
        self.format
    }
    /// Encode normalized samples, preserving libbw64 0.10.0 for integer PCM: samples are clipped to
    /// ±1, scaled by `2^(bits-1) - 1` and truncated toward zero, so `-1.0` becomes `-(2^(bits-1) - 1)`.
    /// That is not the inverse of [`Reader::read_frames`](crate::Reader::read_frames), which divides by
    /// `2^(bits-1)`; use [`Self::write_frames_nearest`] when samples read back must survive the trip.
    pub fn write_frames(&mut self, samples: &[f32]) -> Result<u64> {
        self.write_samples(samples, Quantization::Libbw64)
    }
    /// Encode normalized samples the way `macin-atmos` masters are quantised and `read_frames`
    /// decodes: integer PCM is clipped to ±1, scaled by `2^(bits-1)`, rounded to nearest with ties
    /// away from zero, and saturated, so `-1.0` is the exact negative endpoint and `+1.0` the
    /// largest positive code. Samples `read_frames` returned from 16- and 24-bit PCM are written
    /// back unchanged. Float32 output is identical to [`Self::write_frames`].
    pub fn write_frames_nearest(&mut self, samples: &[f32]) -> Result<u64> {
        self.write_samples(samples, Quantization::Nearest)
    }
    fn write_samples(&mut self, samples: &[f32], quantization: Quantization) -> Result<u64> {
        if self.failed {
            return Err(Error::io("WAVE writer 已因 I/O 错误失效"));
        }
        if !samples
            .len()
            .is_multiple_of(usize::from(self.format.channels))
        {
            return Err(Error::invalid("音频缓冲区不是完整帧"));
        }
        if self.format.sample_format.is_integer() && samples.iter().any(|sample| sample.is_nan()) {
            return Err(Error::invalid("NaN 无法编码为整数 PCM"));
        }
        let width = usize::from(self.format.sample_format.bits() / 8);
        let size = samples
            .len()
            .checked_mul(width)
            .ok_or_else(|| Error::invalid("PCM 缓冲区长度溢出"))?;
        let next = add(self.data_bytes, size as u64)?;
        let end = add(self.data_start, add(next, next & 1)?)?;
        if self.container == Some(Container::Riff) && end - 8 >= u64::from(u32::MAX) {
            return Err(Error::unsupported("固定 RIFF 输出超过大小上限"));
        }
        if size > self.scratch.len() {
            self.scratch
                .try_reserve(size - self.scratch.len())
                .map_err(|_| Error::io("无法分配 PCM 缓冲区"))?;
        }
        self.scratch.resize(size, 0);
        for (sample, raw) in samples.iter().zip(self.scratch.chunks_exact_mut(width)) {
            if self.format.sample_format == SampleFormat::Float32 {
                raw.copy_from_slice(&sample.to_le_bytes());
            } else {
                let full = f64::from(1u32 << (self.format.sample_format.bits() - 1));
                let clipped = f64::from(sample.clamp(-1.0, 1.0));
                let value = match quantization {
                    // Preserve libbw64 0.10.0: f32 clipping, f64 scale, truncate toward zero.
                    Quantization::Libbw64 => (clipped * (full - 1.0)) as i32,
                    Quantization::Nearest => {
                        (clipped * full).round().clamp(-full, full - 1.0) as i32
                    }
                };
                raw.copy_from_slice(&value.to_le_bytes()[..width]);
            }
        }
        if let Err(error) = self.sink.write_all(&self.scratch) {
            self.failed = true;
            return Err(error.into());
        }
        self.data_bytes = next;
        Ok((samples.len() / usize::from(self.format.channels)) as u64)
    }
    /// Returns the finalized stream. Failure consumes the writer without claiming a usable file.
    pub fn finish(self) -> Result<W> {
        self.finish_with_chunks(&mut [])
    }
    /// Append whole frames already encoded in the declared on-disk sample format. No scaling,
    /// clipping, endian conversion or quantization is performed.
    pub fn write_pcm_bytes(&mut self, data: &[u8]) -> Result<u64> {
        if self.failed {
            return Err(Error::io("WAVE writer 已因 I/O 错误失效"));
        }
        let align = usize::from(self.format.block_align()?);
        if !data.len().is_multiple_of(align) {
            return Err(Error::invalid("PCM bytes must contain whole frames"));
        }
        let next = add(self.data_bytes, data.len() as u64)?;
        let end = add(self.data_start, add(next, next & 1)?)?;
        if self.container == Some(Container::Riff) && end - 8 >= u64::from(u32::MAX) {
            return Err(Error::unsupported("固定 RIFF 输出超过大小上限"));
        }
        if let Err(error) = self.sink.write_all(data) {
            self.failed = true;
            return Err(error.into());
        }
        self.data_bytes = next;
        Ok((data.len() / align) as u64)
    }
    pub fn finish_with_chunks(mut self, chunks: &mut [TrailingChunk<'_>]) -> Result<W> {
        if self.failed {
            return Err(Error::io("不能完成已失败的 WAVE writer"));
        }
        if self.data_bytes & 1 != 0 {
            self.sink.write_all(&[0])?;
        }
        let mut end = add(self.data_start, add(self.data_bytes, self.data_bytes & 1)?)?;
        let mut large_axml = None;
        let mut ids = std::collections::BTreeSet::new();
        for chunk in chunks.iter() {
            if matches!(
                &chunk.id,
                b"data" | b"fmt " | b"ds64" | b"RIFF" | b"RF64" | b"BW64"
            ) || !ids.insert(chunk.id)
            {
                return Err(Error::invalid("Duplicate or structural trailing chunk"));
            }
            if chunk.size >= u64::from(u32::MAX) {
                if !self.deferred || chunk.id != *b"axml" {
                    return Err(Error::unsupported(
                        "Large trailing chunk requires reserved AXML entry",
                    ));
                }
                large_axml = Some(chunk.size);
            }
            end = add(end, add(8, add(chunk.size, chunk.size & 1)?)?)?;
        }
        let riff_size = end - 8;
        let frames = self.data_bytes / u64::from(self.format.block_align()?);
        let container = self.container.unwrap_or_else(|| {
            if riff_size < u64::from(u32::MAX) {
                Container::Riff
            } else if self.format.sample_format.is_integer() {
                Container::Bw64
            } else {
                Container::Rf64
            }
        });
        if container == Container::Riff && riff_size >= u64::from(u32::MAX) {
            return Err(Error::unsupported("固定 RIFF 输出超过大小上限"));
        }
        for chunk in chunks {
            self.sink.write_all(&chunk.id)?;
            self.sink
                .write_all(&(chunk.size.min(u64::from(u32::MAX)) as u32).to_le_bytes())?;
            let copied = std::io::copy(&mut chunk.source.take(chunk.size), &mut self.sink)?;
            if copied != chunk.size {
                return Err(Error::io("Trailing metadata source is truncated"));
            }
            if chunk.size & 1 != 0 {
                self.sink.write_all(&[0])?;
            }
        }
        self.sink.seek(SeekFrom::Start(0))?;
        self.sink.write_all(&container.id())?;
        self.sink.write_all(
            &if container == Container::Riff {
                riff_size as u32
            } else {
                u32::MAX
            }
            .to_le_bytes(),
        )?;
        if container != Container::Riff {
            self.sink.seek(SeekFrom::Start(12))?;
            let mut ds64 = Vec::with_capacity(28);
            ds64.extend_from_slice(&riff_size.to_le_bytes());
            ds64.extend_from_slice(&self.data_bytes.to_le_bytes());
            // Preserve the legacy writer; the new Atmos BW64 path uses the mandated dummy zero.
            let count = if self.deferred && container == Container::Bw64 {
                0
            } else {
                frames
            };
            ds64.extend_from_slice(&count.to_le_bytes());
            ds64.extend_from_slice(&u32::from(large_axml.is_some()).to_le_bytes());
            if let Some(size) = large_axml {
                ds64.extend_from_slice(b"axml");
                ds64.extend_from_slice(&size.to_le_bytes());
            }
            write_chunk(&mut self.sink, *b"ds64", &ds64)?;
        }
        self.sink.seek(SeekFrom::Start(self.data_start - 4))?;
        self.sink.write_all(
            &if container == Container::Riff {
                self.data_bytes as u32
            } else {
                u32::MAX
            }
            .to_le_bytes(),
        )?;
        if let Some(fact) = self.fact {
            self.sink.seek(SeekFrom::Start(fact))?;
            self.sink
                .write_all(&(frames.min(u64::from(u32::MAX)) as u32).to_le_bytes())?;
        }
        self.sink.seek(SeekFrom::Start(end))?;
        self.sink.flush()?;
        Ok(self.sink)
    }
}

/// Creates a file writer: float output is always RF64 (32-bit only) because a streaming writer
/// cannot know the final size; integer PCM is 16/24/32-bit and starts as RIFF. `exclusive` fails on
/// an existing file instead of replacing it. A file created here is removed if setup fails.
pub fn create_file_writer(
    path: &Path,
    channels: u32,
    rate: u32,
    bits: u16,
    float: bool,
    exclusive: bool,
) -> Result<Writer<File>> {
    let (sample_format, container) = match (float, bits) {
        (true, 32) => (SampleFormat::Float32, Some(Container::Rf64)),
        (true, _) => return Err(Error::unsupported("浮点 WAVE 位深必须为 32")),
        (false, 16) => (SampleFormat::Pcm16, None),
        (false, 24) => (SampleFormat::Pcm24, None),
        (false, 32) => (SampleFormat::Pcm32, None),
        (false, _) => return Err(Error::unsupported("整数 WAVE 位深必须为 16/24/32")),
    };
    let format = Format {
        channels: u16::try_from(channels).map_err(|_| Error::invalid("WAVE 声道数过大"))?,
        sample_rate: rate,
        sample_format,
        channel_mask: None,
    };
    format.block_align()?;
    let mut options = OpenOptions::new();
    options.write(true);
    if exclusive {
        options.create_new(true);
    } else {
        options.create(true).truncate(true);
    }
    let file = options.open(path)?;
    Writer::new(
        file,
        format,
        WriterOptions {
            container,
            ..WriterOptions::default()
        },
    )
    .inspect_err(|_| {
        let _ = std::fs::remove_file(path);
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quantization {
    Libbw64,
    Nearest,
}
