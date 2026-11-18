//! 有预算的附加容器元数据。来源布局改变时拒绝透传，不猜测私有载荷内的时间或声道引用。
use crate::{Error, MasterSpec, Result};

pub const CONTAINER_METADATA_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataContainer {
    Wave,
    Caf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataChunk {
    pub id: [u8; 4],
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerMetadata {
    pub container: MetadataContainer,
    pub chunks: Vec<MetadataChunk>,
    pub(crate) origin: Option<Origin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Origin {
    pub frames: u64,
    pub channels: usize,
    pub sample_rate: u32,
    pub start_sample: u64,
    pub object_tracks: Vec<crate::TrackId>,
    pub unchanged: bool,
}
impl MetadataContainer {
    pub(crate) fn accepts(self, id: [u8; 4]) -> bool {
        match self {
            Self::Wave => [*b"bext", *b"iXML", *b"LIST"].contains(&id),
            Self::Caf => id == *b"info",
        }
    }
}

/// ADM 来源和替换 WAVE 共用同一套识别与预算规则。按索引读取，以区分不同 LIST 子类型。
#[cfg(feature = "read")]
pub(crate) fn read_wave_chunks<R: std::io::Read + std::io::Seek>(
    wave: &mut macin_bwf::Reader<R>,
    mut report: impl FnMut(String) -> Result<()>,
) -> Result<Vec<MetadataChunk>> {
    let mut chunks = Vec::new();
    let mut remaining = CONTAINER_METADATA_LIMIT as u64;
    let mut seen = std::collections::BTreeSet::new();
    for index in 0..wave.chunks().len() {
        let chunk = wave.chunks()[index];
        if matches!(
            &chunk.id,
            b"fmt " | b"data" | b"ds64" | b"axml" | b"chna" | b"dbmd" | b"JUNK" | b"PAD " | b"fact"
        ) {
            continue;
        }
        if !MetadataContainer::Wave.accepts(chunk.id) {
            report(format!(
                "WAVE chunk {:?} is not retained by master conversion",
                String::from_utf8_lossy(&chunk.id)
            ))?;
            continue;
        }
        if chunk.id != *b"LIST" && !seen.insert(chunk.id) {
            return Err(Error::invalid("Duplicate preserved WAVE chunk"));
        }
        if chunk.size > remaining {
            report("WAVE ancillary metadata exceeds 4 MiB preservation budget".into())?;
            continue;
        }
        let data = wave.metadata_at(index, remaining)?;
        if chunk.id == *b"LIST" {
            if !data.starts_with(b"INFO") {
                report("Only WAVE LIST/INFO metadata can be retained".into())?;
                continue;
            }
            if !seen.insert(chunk.id) {
                return Err(Error::invalid("Duplicate WAVE LIST/INFO chunk"));
            }
        }
        remaining -= chunk.size;
        chunks.push(MetadataChunk { id: chunk.id, data });
    }
    Ok(chunks)
}
impl ContainerMetadata {
    /// 调用方自行提供的载荷；调用方负责保证它与所写音频及布局一致。
    pub fn new(container: MetadataContainer, chunks: Vec<MetadataChunk>) -> Result<Self> {
        let metadata = Self {
            container,
            chunks,
            origin: None,
        };
        metadata.validate()?;
        Ok(metadata)
    }
    pub(crate) fn validate(&self) -> Result<()> {
        let mut size = 0usize;
        let mut ids = std::collections::BTreeSet::new();
        for chunk in &self.chunks {
            if !self.container.accepts(chunk.id) || !ids.insert(chunk.id) {
                return Err(Error::invalid("Unsupported or duplicate ancillary chunk"));
            }
            if chunk.id == *b"LIST" && !chunk.data.starts_with(b"INFO") {
                return Err(Error::invalid(
                    "Only LIST/INFO ancillary metadata is supported",
                ));
            }
            size = size
                .checked_add(chunk.data.len())
                .ok_or_else(|| Error::invalid("Ancillary metadata size overflow"))?;
        }
        if size > CONTAINER_METADATA_LIMIT {
            return Err(Error::invalid("Ancillary metadata exceeds 4 MiB budget"));
        }
        Ok(())
    }
    pub(crate) fn matches(&self, spec: &MasterSpec, frames: Option<u64>) -> bool {
        self.chunks.is_empty()
            || (self.container == MetadataContainer::Wave) == spec.profile.is_adm()
                && self.origin.as_ref().is_none_or(|o| {
                    o.unchanged
                        && o.channels == spec.channels()
                        && o.sample_rate == spec.sample_rate
                        && o.start_sample == spec.start_sample
                        && o.object_tracks
                            .iter()
                            .copied()
                            .eq(spec.objects.iter().map(|o| o.id))
                        && frames.is_none_or(|f| f == o.frames)
                })
    }
}
