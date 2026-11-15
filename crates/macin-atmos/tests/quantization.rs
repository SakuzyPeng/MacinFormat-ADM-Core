//! `macin_bwf::Writer::write_frames_nearest` documents the master writer's quantisation; hold the two
//! to the same bytes.
use macin_atmos::*;
use macin_bwf::{ChunkReader, Format, SampleFormat, Writer, WriterOptions};
use std::io::Cursor;

#[test]
fn masters_and_write_frames_nearest_quantise_identically() {
    let path = std::env::temp_dir().join(format!(
        "macin-atmos-quantization-{}.wav",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let frames = 4096;
    let mut samples: Vec<f32> = (0..frames * 11)
        .map(|i| ((i * 2_654_435_761_usize) % 2_000_001) as f32 / 1_000_000. - 1.)
        .collect();
    samples[..6].copy_from_slice(&[
        -1.0,
        1.0,
        0.5 / 8388608.0,
        -0.5 / 8388608.0,
        1.5 / 8388608.0,
        0.0,
    ]);
    let spec = MasterSpec {
        objects: vec![ObjectSpec::new(TrackId(1), "Object")],
        ..Default::default()
    };
    let mut master = MasterWriter::create(&path, spec).unwrap();
    master
        .write_block(PcmBlock::F32Interleaved(&samples), &[])
        .unwrap();
    master.finish().unwrap();
    let mut chunks = ChunkReader::new(std::fs::File::open(&path).unwrap()).unwrap();
    let size = chunks
        .chunks()
        .iter()
        .find(|c| &c.id == b"data")
        .unwrap()
        .size;
    let mut from_master = vec![0; usize::try_from(size).unwrap()];
    assert!(chunks.read_chunk(*b"data", &mut from_master).unwrap());
    std::fs::remove_file(&path).unwrap();

    let mut writer = Writer::new(
        Cursor::new(Vec::new()),
        Format {
            channels: 11,
            sample_rate: 48_000,
            sample_format: SampleFormat::Pcm24,
            channel_mask: None,
        },
        WriterOptions::default(),
    )
    .unwrap();
    writer.write_frames_nearest(&samples).unwrap();
    let wave = writer.finish().unwrap().into_inner();
    let mut chunks = ChunkReader::new(Cursor::new(wave)).unwrap();
    let mut from_wave = vec![0; from_master.len()];
    assert!(chunks.read_chunk(*b"data", &mut from_wave).unwrap());
    assert_eq!(from_wave, from_master);
}
