use macin_bwf::*;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

fn format() -> Format {
    Format {
        channels: 1,
        sample_rate: 96_000,
        sample_format: SampleFormat::Pcm24,
        channel_mask: None,
    }
}
fn options() -> WriterOptions {
    WriterOptions {
        container: Some(Container::Bw64),
        ..Default::default()
    }
}

// A virtual file counts bytes and retains just the header. Writing >4 GiB costs <1 MiB RAM,
// without creating a large file, copying the PCM, or changing the writer's private state.
struct Virtual {
    pos: u64,
    len: u64,
    head: [u8; 128],
}
impl Write for Virtual {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self.pos < self.head.len() as u64 {
            let start = self.pos as usize;
            let count = b.len().min(self.head.len() - start);
            self.head[start..start + count].copy_from_slice(&b[..count]);
        }
        self.pos += b.len() as u64;
        self.len = self.len.max(self.pos);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Seek for Virtual {
    fn seek(&mut self, at: SeekFrom) -> io::Result<u64> {
        self.pos = match at {
            SeekFrom::Start(v) => v,
            SeekFrom::Current(v) => self.pos.checked_add_signed(v).unwrap(),
            SeekFrom::End(v) => self.len.checked_add_signed(v).unwrap(),
        };
        Ok(self.pos)
    }
}
#[test]
fn a_large_pcm_chunk_and_trailing_xml_have_distinct_sizes() {
    let mut writer = Writer::new_deferred(
        Virtual {
            pos: 0,
            len: 0,
            head: [0; 128],
        },
        format(),
        options(),
    )
    .unwrap();
    let pcm = vec![0; 3 * 262_144];
    let mut size = 0;
    while size <= u64::from(u32::MAX) {
        writer.write_pcm_bytes(&pcm).unwrap();
        size += pcm.len() as u64;
    }
    let mut xml = &b"<independent/>"[..];
    let xml_len = xml.len() as u64;
    let out = writer
        .finish_with_chunks(&mut [TrailingChunk {
            id: *b"axml",
            size: xml_len,
            source: &mut xml,
        }])
        .unwrap();
    assert_eq!(&out.head[..4], b"BW64");
    assert_eq!(
        u64::from_le_bytes(out.head[20..28].try_into().unwrap()),
        out.len - 8
    );
    assert_eq!(
        u64::from_le_bytes(out.head[28..36].try_into().unwrap()),
        size
    );
    assert_eq!(u64::from_le_bytes(out.head[36..44].try_into().unwrap()), 0);
    assert_eq!(&out.head[84..88], b"data");
    assert_eq!(&out.head[88..92], &u32::MAX.to_le_bytes());
    assert_eq!(
        out.len,
        92 + size + (size & 1) + 8 + xml_len + (xml_len & 1)
    );
}
#[test]
fn truncated_metadata_overflow_and_reserved_chunks_are_rejected() {
    for (id, size) in [(*b"axml", 30), (*b"axml", u64::MAX), (*b"data", 1)] {
        let mut writer =
            Writer::new_deferred(Cursor::new(Vec::new()), format(), options()).unwrap();
        writer.write_pcm_bytes(&[1, 2, 3]).unwrap();
        let mut input = &b"x"[..];
        assert!(
            writer
                .finish_with_chunks(&mut [TrailingChunk {
                    id,
                    size,
                    source: &mut input
                }])
                .is_err()
        );
    }
}
#[test]
fn trailing_xml_is_readable_without_counting_it_as_audio() {
    let mut writer = Writer::new_deferred(Cursor::new(Vec::new()), format(), options()).unwrap();
    writer.write_pcm_bytes(&[0, 0, 64]).unwrap();
    let mut input = &b"<x/>trailer"[..];
    let result = writer
        .finish_with_chunks(&mut [TrailingChunk {
            id: *b"axml",
            size: 4,
            source: &mut input,
        }])
        .unwrap();
    let mut remaining = String::new();
    input.read_to_string(&mut remaining).unwrap();
    assert_eq!(remaining, "trailer");
    let mut reader = Reader::new(Cursor::new(result.into_inner())).unwrap();
    assert_eq!(reader.info().frames, 1);
    assert_eq!(reader.metadata(*b"axml", 100).unwrap().unwrap(), b"<x/>");
    let mut pcm = [0.; 4];
    assert_eq!(reader.read_frames(&mut pcm).unwrap(), 1);
    assert_eq!(pcm[0], 0.5);
}
struct Failure {
    cursor: Cursor<Vec<u8>>,
    remaining: usize,
}
impl Write for Failure {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Err(io::Error::other("injected short write"));
        }
        let n = self.remaining.min(b.len());
        self.remaining -= n;
        self.cursor.write(&b[..n])
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Seek for Failure {
    fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
        self.cursor.seek(p)
    }
}
#[test]
fn a_partial_audio_write_permanently_invalidates_the_writer() {
    let sink = Failure {
        cursor: Cursor::new(Vec::new()),
        remaining: 93,
    };
    let mut writer = Writer::new_deferred(sink, format(), options()).unwrap();
    assert!(writer.write_pcm_bytes(&[0; 6]).is_err());
    assert!(writer.write_pcm_bytes(&[]).is_err());
    assert!(writer.finish().is_err());
}
