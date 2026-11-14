//! Inspection of RIFF/RF64/BW64 containers and their ADM metadata, and DAMF conversion, behind
//! `mfadm`.
//!
//! Every report is a plain serialisable value built from the public `macin-bwf` / `macin-adm` /
//! `macin-atmos` API; the binary only chooses between text and JSON. Inspection never writes;
//! conversion only creates new outputs and never touches its inputs.
#![forbid(unsafe_code)]

pub mod convert;

use macin_adm::records::{NONE, Span};
use macin_adm::{Document, Snapshot, TrackUidDefinition};
use macin_bwf::{Chna, ChunkReader, Container, Reader, SampleFormat};
use serde::Serialize;
use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Default budget for a single metadata chunk read into memory.
pub const DEFAULT_METADATA_BUDGET: u64 = 256 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: String,
    /// Set when the failure came from the operating system, so a caller can tell a closed
    /// output pipe from a malformed file.
    pub io_kind: Option<std::io::ErrorKind>,
}
impl Error {
    pub fn msg(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            io_kind: None,
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self {
            message: error.to_string(),
            io_kind: Some(error.kind()),
        }
    }
}
impl From<macin_bwf::Error> for Error {
    fn from(error: macin_bwf::Error) -> Self {
        Self::msg(error.message)
    }
}
impl From<macin_adm::Error> for Error {
    fn from(error: macin_adm::Error) -> Self {
        Self::msg(error.message)
    }
}
pub type Result<T> = std::result::Result<T, Error>;

fn open(path: &Path) -> Result<BufReader<File>> {
    File::open(path).map(BufReader::new).map_err(|e| Error {
        message: format!("{}: {e}", path.display()),
        io_kind: Some(e.kind()),
    })
}

fn container_name(container: Container) -> &'static str {
    match container {
        Container::Riff => "RIFF",
        Container::Rf64 => "RF64",
        Container::Bw64 => "BW64",
    }
}

/// A four-character code or fixed-width identifier, shown as text with NUL padding removed and
/// any non-printable byte escaped, so a malformed ID is visible rather than silently mangled.
fn fourcc(bytes: &[u8]) -> String {
    let trimmed = bytes
        .iter()
        .rposition(|&b| b != 0)
        .map_or(&[][..], |end| &bytes[..=end]);
    trimmed
        .iter()
        .flat_map(|&b| std::ascii::escape_default(b))
        .map(char::from)
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct ChunkEntry {
    pub id: String,
    /// Payload offset from the start of the file (the chunk header sits 8 bytes earlier).
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChunksReport {
    pub container: &'static str,
    /// Container length as declared, including the 8-byte top-level header.
    pub end: u64,
    pub chunks: Vec<ChunkEntry>,
}

/// List the chunk table without decoding `fmt ` or the audio.
pub fn chunks(path: &Path) -> Result<ChunksReport> {
    let reader = ChunkReader::new(open(path)?)?;
    Ok(ChunksReport {
        container: container_name(reader.container()),
        end: reader.end(),
        chunks: reader
            .chunks()
            .iter()
            .map(|c| ChunkEntry {
                id: fourcc(&c.id),
                offset: c.offset,
                size: c.size,
            })
            .collect(),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct ChnaRow {
    pub track_index: u16,
    pub uid: String,
    pub track_format: String,
    pub pack_format: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InfoReport {
    pub container: &'static str,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: &'static str,
    pub bits: u16,
    pub valid_bits: u16,
    pub channel_mask: Option<u32>,
    pub frames: u64,
    pub seconds: f64,
    pub data_bytes: u64,
    /// `None` when the file has no `chna` chunk.
    pub chna: Option<Vec<ChnaRow>>,
}

fn sample_format_name(format: SampleFormat) -> &'static str {
    match format {
        SampleFormat::Pcm16 | SampleFormat::Pcm24 | SampleFormat::Pcm32 => "pcm",
        SampleFormat::Float32 => "float",
    }
}

#[allow(
    clippy::cast_precision_loss,
    reason = "a duration for display; exact frame counts are reported alongside"
)]
fn seconds(samples: u64, rate: u32) -> f64 {
    samples as f64 / f64::from(rate)
}

fn read_chna(reader: &mut Reader<BufReader<File>>, budget: u64) -> Result<Option<Chna>> {
    let Some(data) = reader.metadata(*b"chna", budget)? else {
        return Ok(None);
    };
    let chna = Chna::decode_import(&data)?;
    if !(data.len() - 4).is_multiple_of(40) {
        return Err(Error::msg("CHNA 记录被截断"));
    }
    if chna.tracks != reader.info().format.channels {
        return Err(Error::msg("CHNA 轨道数与 fmt 不一致"));
    }
    if chna.entries.iter().any(|e| e.track_index > chna.tracks) {
        return Err(Error::msg("CHNA track index 超出轨道数"));
    }
    Ok(Some(chna))
}

/// Audio format, length and the `chna` track table.
pub fn info(path: &Path) -> Result<InfoReport> {
    let mut reader = Reader::new(open(path)?)?;
    let info = reader.info();
    let chna = read_chna(&mut reader, DEFAULT_METADATA_BUDGET)?.map(|chna| {
        chna.entries
            .iter()
            .map(|e| ChnaRow {
                track_index: e.track_index,
                uid: fourcc(&e.uid),
                track_format: fourcc(&e.track_format),
                pack_format: fourcc(&e.pack_format),
            })
            .collect()
    });
    Ok(InfoReport {
        container: container_name(info.container),
        sample_rate: info.format.sample_rate,
        channels: info.format.channels,
        sample_format: sample_format_name(info.format.sample_format),
        bits: info.format.sample_format.bits(),
        valid_bits: info.valid_bits,
        channel_mask: info.format.channel_mask,
        frames: info.frames,
        seconds: seconds(info.frames, info.format.sample_rate),
        data_bytes: info.data_bytes,
        chna,
    })
}

/// The raw `axml` payload, byte for byte.
pub fn axml(path: &Path, budget: u64) -> Result<Vec<u8>> {
    let mut reader = ChunkReader::new(open(path)?)?;
    reader
        .metadata(*b"axml", budget)?
        .ok_or_else(|| Error::msg("no axml chunk"))
}

#[derive(Debug, Clone, Serialize)]
pub struct ProgrammeRow {
    pub id: String,
    pub name: String,
    pub language: Option<String>,
    pub start: u64,
    pub end: Option<u64>,
    pub contents: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContentRow {
    pub id: String,
    pub name: String,
    pub language: Option<String>,
    pub objects: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrackRow {
    pub uid: String,
    /// The `chna` track index this UID is carried on; `None` when `chna` does not map it.
    pub track_index: Option<u16>,
    pub object_blocks: usize,
    pub direct_speaker_blocks: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ObjectRow {
    pub id: String,
    pub name: String,
    /// `Objects`, `DirectSpeakers`, `HOA`, a `+`-joined mix, or `None` for a pure container object.
    pub kind: Option<String>,
    /// Absolute start in samples, after the parent chain.
    pub start: u64,
    pub duration: Option<u64>,
    pub gain: f32,
    pub mute: bool,
    pub tracks: Vec<TrackRow>,
    pub children: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HoaRow {
    pub object: String,
    pub pack: String,
    pub channels: usize,
    pub max_order: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdmReport {
    pub sample_rate: u32,
    pub programmes: Vec<ProgrammeRow>,
    pub contents: Vec<ContentRow>,
    pub objects: Vec<ObjectRow>,
    pub hoa: Vec<HoaRow>,
    pub object_blocks: usize,
    pub direct_speaker_blocks: usize,
    pub warnings: Vec<String>,
}

struct Strings<'a>(&'a Snapshot);
impl Strings<'_> {
    fn get(&self, index: u32) -> String {
        self.0.strings[index as usize].clone()
    }
    fn optional(&self, index: u32) -> Option<String> {
        (index != NONE).then(|| self.get(index))
    }
    fn list(&self, span: Span) -> Vec<String> {
        self.0.indices[span.offset..span.offset + span.len]
            .iter()
            .map(|&i| self.get(i))
            .collect()
    }
}

/// Summarise the ADM metadata of a file, resolved against its own `chna` and sample rate.
pub fn adm(path: &Path, budget: u64) -> Result<AdmReport> {
    let mut reader = Reader::new(open(path)?)?;
    let rate = reader.info().format.sample_rate;
    let xml = reader
        .metadata(*b"axml", budget)?
        .ok_or_else(|| Error::msg("no axml chunk"))?;
    let chna = read_chna(&mut reader, budget)?;
    let mut channels = Vec::new();
    let mut definitions = Vec::new();
    for entry in chna.iter().flat_map(|c| &c.entries) {
        if entry.track_index == 0 {
            continue;
        }
        let uid = chna_identifier(&entry.uid)?;
        let mut format_id = chna_identifier(&entry.track_format)?;
        if format_id.to_ascii_uppercase().starts_with("AC_")
            && let Some(id) = format_id.strip_suffix("_00")
        {
            format_id = id.to_owned();
        }
        let pack = chna_identifier(&entry.pack_format)?;
        channels.push((uid.clone(), entry.track_index));
        definitions.push(TrackUidDefinition {
            uid,
            format_id,
            pack_id: (!pack.is_empty()).then_some(pack),
        });
    }
    let xml =
        std::str::from_utf8(&xml).map_err(|e| Error::msg(format!("axml is not UTF-8: {e}")))?;
    let doc = Document::parse_with_track_uids(xml, &definitions)?;
    let scene = Snapshot::extract(&doc, rate, &channels)?;
    Ok(summarise(&scene, rate))
}

fn chna_identifier(bytes: &[u8]) -> Result<String> {
    std::str::from_utf8(bytes)
        .map(|s| s.trim_end_matches([' ', '\0']).to_owned())
        .map_err(|_| Error::msg("CHNA 标识符必须为 UTF-8"))
}

/// The same summary from an AXML document already in memory.
pub fn adm_from_xml(xml: &str, rate: u32, chna: &[(String, u16)]) -> Result<AdmReport> {
    let doc = Document::parse(xml)?;
    let scene = Snapshot::extract(&doc, rate, chna)?;
    Ok(summarise(&scene, rate))
}

fn summarise(scene: &Snapshot, rate: u32) -> AdmReport {
    let s = Strings(scene);
    let hoa: Vec<HoaRow> = scene
        .hoa
        .iter()
        .map(|h| {
            let channels =
                &scene.hoa_channels[h.channels.offset..h.channels.offset + h.channels.len];
            HoaRow {
                object: s.get(h.object_id),
                pack: s.get(h.pack_id),
                channels: channels.len(),
                max_order: channels.iter().map(|c| c.order).max(),
            }
        })
        .collect();
    let objects = scene
        .objects
        .iter()
        .map(|o| {
            let id = s.get(o.id);
            let tracks: Vec<TrackRow> = scene.tracks
                [o.tracks.offset..o.tracks.offset + o.tracks.len]
                .iter()
                .map(|t| TrackRow {
                    uid: s.get(t.uid),
                    track_index: u16::try_from(t.channel).ok(),
                    object_blocks: t.blocks.len,
                    direct_speaker_blocks: t.ds_blocks.len,
                })
                .collect();
            let mut kinds = Vec::new();
            if tracks.iter().any(|t| t.object_blocks > 0) {
                kinds.push("Objects");
            }
            if tracks.iter().any(|t| t.direct_speaker_blocks > 0) {
                kinds.push("DirectSpeakers");
            }
            if hoa.iter().any(|h| h.object == id) {
                kinds.push("HOA");
            }
            ObjectRow {
                name: s.get(o.name),
                kind: (!kinds.is_empty()).then(|| kinds.join("+")),
                start: o.absolute_start,
                duration: (o.duration_present != 0).then_some(o.duration),
                gain: o.gain,
                mute: o.mute != 0,
                tracks,
                children: s.list(o.child_objects),
                id,
            }
        })
        .collect();
    AdmReport {
        sample_rate: rate,
        programmes: scene
            .programmes
            .iter()
            .map(|p| ProgrammeRow {
                id: s.get(p.id),
                name: s.get(p.name),
                language: s.optional(p.language),
                start: p.start,
                end: (p.end_present != 0).then_some(p.end),
                contents: s.list(p.content_ids),
            })
            .collect(),
        contents: scene
            .contents
            .iter()
            .map(|c| ContentRow {
                id: s.get(c.id),
                name: s.get(c.name),
                language: s.optional(c.language),
                objects: s.list(c.object_ids),
            })
            .collect(),
        objects,
        hoa,
        object_blocks: scene.object_blocks.len(),
        direct_speaker_blocks: scene.direct_blocks.len(),
        warnings: scene.warnings.iter().map(|&w| s.get(w)).collect(),
    }
}

/// Sample count as `h:mm:ss.mmm` at the file's rate.
#[must_use]
pub fn timecode(samples: u64, rate: u32) -> String {
    let rate = u64::from(rate);
    let millis = samples / rate * 1000 + samples % rate * 1000 / rate;
    format!(
        "{}:{:02}:{:02}.{:03}",
        millis / 3_600_000,
        millis / 60_000 % 60,
        millis / 1000 % 60,
        millis % 1000
    )
}
