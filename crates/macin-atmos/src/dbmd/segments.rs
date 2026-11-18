use crate::{Error, ErrorKind, Result, ancillary::Origin};

/// 检查过的原生 DBMD。保留段的顺序、节目 ID 及 EBU 要求透传的保留字节。
#[derive(Clone, Debug)]
pub struct DbmdSource {
    pub(super) version: [u8; 4],
    pub(super) parts: std::sync::Arc<[Segment]>,
    pub(super) padding: Option<u8>,
    pub(crate) extra_encoding: bool,
    pub(crate) origin: Option<Origin>,
}

#[derive(Clone, Debug)]
pub(super) struct Segment {
    pub id: u8,
    pub data: Vec<u8>,
    pub size_code: u16,
}

impl DbmdSource {
    pub fn version(&self) -> u32 {
        u32::from_le_bytes(self.version)
    }

    pub fn segments(&self) -> impl Iterator<Item = (u8, &[u8])> {
        self.parts
            .iter()
            .map(|part| (part.id, part.data.as_slice()))
    }

    pub(crate) fn requires_dbmd(&self) -> bool {
        self.extra_encoding
            || self
                .segment(9)
                .is_some_and(|p| p[99..111].iter().any(|&v| v != 0))
    }

    pub(super) fn segment(&self, id: u8) -> Option<&[u8]> {
        self.segments().find(|p| p.0 == id).map(|p| p.1)
    }

    pub(super) fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > super::DBMD_LIMIT {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "DBMD exceeds the 1 MiB metadata budget",
            ));
        }
        let (version, mut body) = bytes
            .split_first_chunk::<4>()
            .ok_or_else(|| Error::invalid("Truncated DBMD version"))?;
        if !matches!(version, [5..=8, 0, 0, 1]) {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "Unsupported DBMD version",
            ));
        }
        let mut parts: Vec<Segment> = Vec::new();
        let mut singleton = [false; 3];
        let mut extra_encoding = false;
        loop {
            let (&id, rest) = body
                .split_first()
                .ok_or_else(|| Error::invalid("DBMD has no terminator"))?;
            if id == 0 {
                let padding = match rest {
                    [] => None,
                    [v] if bytes.len().is_multiple_of(2) => Some(*v),
                    _ => return Err(Error::invalid("Data after DBMD terminator")),
                };
                return Ok(Self {
                    version: *version,
                    parts: parts.into(),
                    padding,
                    extra_encoding,
                    origin: None,
                });
            }
            let (header, tail) = rest
                .split_first_chunk::<2>()
                .ok_or_else(|| Error::invalid("Truncated DBMD segment header"))?;
            let size_code = u16::from_le_bytes(*header);
            let size = usize::from(if size_code == 0 { u16::MAX } else { size_code });
            let (data, tail) = tail
                .split_at_checked(size)
                .ok_or_else(|| Error::invalid("Truncated DBMD segment"))?;
            let (&check, tail) = tail
                .split_first()
                .ok_or_else(|| Error::invalid("Missing DBMD checksum"))?;
            if checksum(size_code, data) != check {
                return Err(Error::invalid("Invalid DBMD checksum"));
            }
            if let Some(index) = [1, 9, 10].iter().position(|v| *v == id)
                && std::mem::replace(&mut singleton[index], true)
            {
                return Err(Error::invalid("Duplicate DBMD segment"));
            }
            if let Some(expected) = match id {
                1 => Some(118),
                3 => Some(69),
                7 => Some(96),
                8 => Some(70),
                9 => Some(248),
                _ => None,
            } && size != expected
            {
                return Err(Error::invalid("Invalid DBMD segment length"));
            }
            extra_encoding |= !matches!(id, 7 | 9 | 10);
            parts.push(Segment {
                id,
                size_code,
                data: data.to_vec(),
            });
            body = tail;
        }
    }
}

fn checksum(size_code: u16, payload: &[u8]) -> u8 {
    0u8.wrapping_sub(
        payload
            .iter()
            .fold(size_code as u8, |sum, b| sum.wrapping_add(*b)),
    )
}

pub(super) fn append(out: &mut Vec<u8>, id: u8, size_code: u16, payload: &[u8]) {
    out.push(id);
    out.extend_from_slice(&size_code.to_le_bytes());
    out.extend_from_slice(payload);
    out.push(checksum(size_code, payload));
}
