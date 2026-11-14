use macin_atmos::*;
use macin_bwf::{ChunkReader, Reader};
use std::{
    fs,
    io::Cursor,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "macin-atmos-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn spec(profile: Profile, rate: u32) -> MasterSpec {
    MasterSpec {
        profile,
        sample_rate: rate,
        objects: vec![ObjectSpec::new(TrackId(700), "Object <&\"")],
        ..Default::default()
    }
}
fn pcm(frames: usize) -> Vec<f32> {
    (0..frames * 11)
        .map(|i| {
            if i % 11 == 3 {
                -0.25
            } else if i % 11 == 10 {
                (i % 17) as f32 / 32.
            } else {
                0.
            }
        })
        .collect()
}
fn metadata(data: &[u8], id: [u8; 4]) -> Vec<u8> {
    ChunkReader::new(Cursor::new(data))
        .unwrap()
        .metadata(id, 1_000_000)
        .unwrap()
        .unwrap()
}
fn snapshot(data: &[u8]) -> macin_adm::Snapshot {
    let xml = String::from_utf8(metadata(data, *b"axml")).unwrap();
    let chna = macin_bwf::Chna::decode(&metadata(data, *b"chna")).unwrap();
    let mapping = chna
        .entries
        .iter()
        .map(|e| {
            (
                String::from_utf8(e.uid.to_vec()).unwrap(),
                e.track_index - 1,
            )
        })
        .collect::<Vec<_>>();
    let rate = Reader::new(Cursor::new(data))
        .unwrap()
        .info()
        .format
        .sample_rate;
    macin_adm::Snapshot::extract(&macin_adm::Document::parse(&xml).unwrap(), rate, &mapping)
        .unwrap()
}
fn update(sample: u64, x: f64, ramp: u64) -> ObjectUpdate {
    ObjectUpdate {
        track: TrackId(700),
        sample,
        ramp_samples: Some(ramp),
        patch: ObjectPatch {
            position: Some([x, 0., 0.]),
            ..Default::default()
        },
    }
}

#[test]
fn four_profiles_two_rates_and_audio_are_consistent() {
    let tmp = Temp::new();
    for rate in [48_000, 96_000] {
        let mut expected = None;
        for (index, profile) in [
            Profile::Standard,
            Profile::Logic,
            Profile::DamfHome,
            Profile::DamfThreeDof,
        ]
        .into_iter()
        .enumerate()
        {
            let path = tmp.path(&format!("{rate}-{index}"));
            let mut writer = MasterWriter::create(&path, spec(profile, rate)).unwrap();
            let samples = pcm(129);
            writer
                .write_block(
                    PcmBlock::F32Interleaved(&samples),
                    &[update(0, -1., 0), update(17, 1., 80)],
                )
                .unwrap();
            let report = writer.finish().unwrap();
            assert_eq!(
                (report.frames, report.channels, report.sample_rate),
                (129, 11, rate)
            );
            assert_eq!(report.objects[0].id, TrackId(700));
            assert_eq!(report.objects[0].track_index, 11);
            let raw = if profile.is_adm() {
                let data = fs::read(&path).unwrap();
                assert_eq!(&data[..4], profile.container_id());
                assert_eq!(&data[12..16], b"ds64");
                let chunks = ChunkReader::new(Cursor::new(&data)).unwrap();
                let position = |id| chunks.chunks().iter().position(|c| c.id == id).unwrap();
                assert!(position(*b"axml") > position(*b"data"));
                let snap = snapshot(&data);
                assert_eq!(snap.tracks.len(), 11);
                assert!(snap.strings.iter().any(|s| s == "Object <&\""));
                let ds64 = metadata(&data, *b"ds64");
                assert_eq!(
                    u64::from_le_bytes(ds64[16..24].try_into().unwrap()),
                    if profile == Profile::Standard { 0 } else { 129 }
                );
                if profile == Profile::Logic {
                    assert!(!metadata(&data, *b"dbmd").is_empty());
                }
                metadata(&data, *b"data")
            } else {
                let audio = fs::read(path.join("master.atmos.audio")).unwrap();
                assert_eq!(&audio[..4], b"caff");
                assert_eq!(
                    f64::from_bits(u64::from_be_bytes(audio[20..28].try_into().unwrap())),
                    f64::from(rate)
                );
                assert_eq!(
                    i64::from_be_bytes(audio[56..64].try_into().unwrap()),
                    129 * 11 * 3 + 4
                );
                let text = fs::read_to_string(path.join("master.atmos.metadata")).unwrap();
                assert!(text.starts_with(&format!("sampleRate: {rate}\nevents:\n")));
                assert!(
                    fs::read_to_string(path.join("master.atmos"))
                        .unwrap()
                        .contains(profile.damf_version())
                );
                assert_eq!(fs::read_dir(&path).unwrap().count(), 3);
                audio[68..].to_vec()
            };
            if let Some(ref expected) = expected {
                assert_eq!(&raw, expected);
            } else {
                expected = Some(raw);
            }
        }
    }
}

#[test]
fn chunk_boundaries_do_not_change_a_master() {
    let tmp = Temp::new();
    for profile in [
        Profile::Standard,
        Profile::Logic,
        Profile::DamfHome,
        Profile::DamfThreeDof,
    ] {
        let samples = pcm(213);
        let updates = [
            update(0, -1., 0),
            update(23, 1., 109),
            ObjectUpdate {
                track: TrackId(700),
                sample: 71,
                ramp_samples: None,
                patch: ObjectPatch {
                    importance: Some(0.4),
                    ..Default::default()
                },
            },
            update(150, 0., 20),
        ];
        let mut outputs = Vec::new();
        for size in [1, 17, 213] {
            let path = tmp.path(&format!("{profile:?}-{size}"));
            let mut writer = MasterWriter::create(&path, spec(profile, 96_000)).unwrap();
            for start in (0..213).step_by(size) {
                let end = 213.min(start + size);
                let events: Vec<_> = updates
                    .iter()
                    .copied()
                    .filter(|e| (start as u64..end as u64).contains(&e.sample))
                    .collect();
                writer
                    .write_block(
                        PcmBlock::F32Interleaved(&samples[start * 11..end * 11]),
                        &events,
                    )
                    .unwrap();
            }
            writer.finish().unwrap();
            let parts = if profile.is_adm() {
                vec![fs::read(path).unwrap()]
            } else {
                [
                    "master.atmos",
                    "master.atmos.metadata",
                    "master.atmos.audio",
                ]
                .iter()
                .map(|s| fs::read(path.join(s)).unwrap())
                .collect()
            };
            outputs.push(parts);
        }
        assert_eq!(outputs[0], outputs[1]);
        assert_eq!(outputs[1], outputs[2]);
    }
}

#[test]
fn partial_updates_preserve_independent_motion_endpoints() {
    let tmp = Temp::new();
    let path = tmp.path("ramp.wav");
    let mut spec = spec(Profile::Standard, 48_000);
    spec.objects[0].initial.position[0] = -1.;
    let mut writer = MasterWriter::create(&path, spec).unwrap();
    writer
        .write_block(
            PcmBlock::F32Interleaved(&pcm(600)),
            &[
                update(100, 1., 400),
                ObjectUpdate {
                    track: TrackId(700),
                    sample: 200,
                    ramp_samples: None,
                    patch: ObjectPatch {
                        importance: Some(0.5),
                        ..Default::default()
                    },
                },
                ObjectUpdate {
                    track: TrackId(700),
                    sample: 300,
                    ramp_samples: Some(100),
                    patch: ObjectPatch {
                        gain: Some(Gain::Linear(0.5)),
                        ..Default::default()
                    },
                },
            ],
        )
        .unwrap();
    writer.finish().unwrap();
    let snap = snapshot(&fs::read(path).unwrap());
    let blocks = &snap.object_blocks;
    for (sample, x) in [(100, -0.5), (200, 0.), (300, 0.5), (400, 1.)] {
        let block = blocks.iter().find(|b| b.start == sample).unwrap();
        assert!((block.position.x - x).abs() < 1e-6, "{block:?}");
    }
    assert_eq!(blocks.last().unwrap().end, 600);
}

#[test]
fn damf_headphone_patch_does_not_write_ramp_or_position() {
    let tmp = Temp::new();
    let path = tmp.path("damf");
    let mut writer = MasterWriter::create(&path, spec(Profile::DamfHome, 48_000)).unwrap();
    writer
        .write_block(
            PcmBlock::F32Interleaved(&pcm(20)),
            &[
                update(0, -1., 0),
                update(2, 1., 15),
                ObjectUpdate {
                    track: TrackId(700),
                    sample: 5,
                    ramp_samples: None,
                    patch: ObjectPatch {
                        head_tracking: Some(HeadTracking::HeadRelative),
                        binaural_mode: Some(BinauralMode::Far),
                        ..Default::default()
                    },
                },
            ],
        )
        .unwrap();
    writer.finish().unwrap();
    let text = fs::read_to_string(path.join("master.atmos.metadata")).unwrap();
    let event = text.split("  - ID: 10").last().unwrap();
    assert!(event.contains("samplePos: 5"));
    assert!(event.contains("headTrackMode: head relative"));
    assert!(!event.contains("rampLength"));
    assert!(!event.contains("pos:"));
}

#[test]
fn raw_s24_and_float_endpoints_are_explicit() {
    let tmp = Temp::new();
    let mut float = MasterWriter::create(tmp.path("float"), MasterSpec::default()).unwrap();
    let mut samples = [0.; 10];
    samples[0] = -1.;
    samples[1] = 1.;
    samples[2] = 0.5;
    float
        .write_block(PcmBlock::F32Interleaved(&samples), &[])
        .unwrap();
    float.finish().unwrap();
    let bytes = metadata(&fs::read(tmp.path("float")).unwrap(), *b"data");
    assert_eq!(&bytes[..9], &[0, 0, 128, 255, 255, 127, 0, 0, 64]);
    let mut raw = MasterWriter::create(tmp.path("raw"), MasterSpec::default()).unwrap();
    raw.write_block(PcmBlock::S24Le(&bytes), &[]).unwrap();
    raw.finish().unwrap();
    assert_eq!(
        fs::read(tmp.path("float")).unwrap(),
        fs::read(tmp.path("raw")).unwrap()
    );
}

#[test]
fn strict_mapping_and_explicit_downgrade_report_source() {
    let tmp = Temp::new();
    let mut spec = spec(Profile::Standard, 48_000);
    spec.objects[0].initial.zone = Zone::NoBack;
    let error = MasterWriter::create(tmp.path("reject"), spec.clone()).unwrap_err();
    assert_eq!(
        (error.kind, error.track, error.field),
        (ErrorKind::Unsupported, Some(TrackId(700)), Some("zone"))
    );
    assert_eq!(fs::read_dir(&tmp.0).unwrap().count(), 0);
    spec.loss_policy = LossPolicy::Allow;
    let mut writer = MasterWriter::create(tmp.path("allow"), spec).unwrap();
    writer
        .write_block(PcmBlock::F32Interleaved(&pcm(1)), &[])
        .unwrap();
    let report = writer.finish().unwrap();
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].sample, Some(0));
}

#[test]
fn rejected_block_can_be_retried_without_changing_output() {
    let tmp = Temp::new();
    let mut writer =
        MasterWriter::create(tmp.path("master"), spec(Profile::Standard, 48_000)).unwrap();
    for value in [f32::NAN, f32::INFINITY, 1.1] {
        assert!(
            writer
                .write_block(PcmBlock::F32Interleaved(&[value; 11]), &[])
                .is_err()
        );
        assert_eq!(writer.frames_written(), 0);
    }
    assert!(writer.write_block(PcmBlock::S24Le(&[0; 32]), &[]).is_err());
    assert!(
        writer
            .write_block(
                PcmBlock::F32Interleaved(&[0.; 11]),
                &[ObjectUpdate {
                    track: TrackId(99),
                    sample: 0,
                    ramp_samples: None,
                    patch: ObjectPatch::default()
                }]
            )
            .is_err()
    );
    assert!(
        writer
            .write_block(PcmBlock::F32Interleaved(&[0.; 11]), &[update(1, 1., 0)])
            .is_err()
    );
    writer
        .write_block(PcmBlock::F32Interleaved(&[0.; 11]), &[update(0, 1., 0)])
        .unwrap();
    assert!(writer.schedule_updates(&[update(0, 0., 0)]).is_err());
    writer.finish().unwrap();
}

#[test]
fn drop_failed_finish_and_publish_race_leave_no_partial_outputs() {
    let tmp = Temp::new();
    drop(MasterWriter::create(tmp.path("drop"), MasterSpec::default()).unwrap());
    assert_eq!(fs::read_dir(&tmp.0).unwrap().count(), 0);
    let mut writer =
        MasterWriter::create(tmp.path("future"), spec(Profile::Standard, 48_000)).unwrap();
    writer.schedule_updates(&[update(10, 1., 0)]).unwrap();
    writer
        .write_block(PcmBlock::F32Interleaved(&pcm(1)), &[])
        .unwrap();
    assert!(writer.finish().is_err());
    assert_eq!(fs::read_dir(&tmp.0).unwrap().count(), 0);
    for (name, profile) in [
        ("race.wav", Profile::Standard),
        ("race.damf", Profile::DamfHome),
    ] {
        let target = tmp.path(name);
        let mut writer = MasterWriter::create(
            &target,
            MasterSpec {
                profile,
                ..Default::default()
            },
        )
        .unwrap();
        writer.write_block(PcmBlock::S24Le(&[0; 30]), &[]).unwrap();
        fs::write(&target, b"other owner").unwrap();
        assert_eq!(writer.finish().unwrap_err().kind, ErrorKind::OutputExists);
        assert_eq!(fs::read(&target).unwrap(), b"other owner");
    }
    assert_eq!(fs::read_dir(&tmp.0).unwrap().count(), 2);
}

#[test]
fn topology_bounds_and_maximum_track_ids() {
    let tmp = Temp::new();
    let mut s = MasterSpec {
        objects: (0..118)
            .map(|i| ObjectSpec::new(TrackId(u64::MAX - i), i.to_string()))
            .collect(),
        ..Default::default()
    };
    let mut writer = MasterWriter::create(tmp.path("max"), s.clone()).unwrap();
    writer.write_block(PcmBlock::S24Le(&[0; 384]), &[]).unwrap();
    assert_eq!(
        writer.finish().unwrap().objects.last().unwrap().track_index,
        128
    );
    s.objects.push(ObjectSpec::new(TrackId(1), "too many"));
    assert!(s.validate().is_err());
    s.objects.truncate(2);
    s.objects[1].id = s.objects[0].id;
    assert!(s.validate().is_err());
    s.objects.clear();
    s.sample_rate = 44_100;
    assert!(s.validate().is_err());
}

#[test]
fn tiny_gain_is_not_silently_muted_and_tiny_priority_loss_is_reported() {
    let tmp = Temp::new();
    let mut s = spec(Profile::Standard, 48_000);
    s.objects[0].initial.gain = Gain::Linear(1e-20);
    let path = tmp.path("quiet.wav");
    let mut writer = MasterWriter::create(&path, s.clone()).unwrap();
    writer
        .write_block(PcmBlock::F32Interleaved(&pcm(1)), &[])
        .unwrap();
    writer.finish().unwrap();
    let parsed = snapshot(&fs::read(path).unwrap());
    assert!(parsed.object_blocks[0].gain > 0.);
    s.objects[0].initial.importance = 1e-20;
    assert_eq!(
        MasterWriter::create(tmp.path("priority.wav"), s)
            .unwrap_err()
            .field,
        Some("importance")
    );
}

#[test]
fn damf_identity_fields_cannot_be_yaml_indicators_or_implicit_booleans() {
    let s = MasterSpec {
        profile: Profile::DamfHome,
        creation_tool: "-".into(),
        creation_tool_version: "yes".into(),
        ..Default::default()
    };
    let manifest = macin_atmos::metadata::damf_manifest(&s).unwrap();
    assert!(manifest.contains("    creationTool: \"-\"\n"));
    assert!(manifest.contains("    creationToolVersion: \"yes\"\n"));
}

#[test]
fn damf_stems_preserve_trailing_dots_and_spaces() {
    let tmp = Temp::new();
    for profile in [Profile::DamfHome, Profile::DamfThreeDof] {
        for (index, stem) in ["master.", "master "].into_iter().enumerate() {
            let path = tmp.path(&format!("{profile:?}-{index}"));
            let mut spec = spec(profile, 48_000);
            spec.stem = stem.into();
            let mut writer = MasterWriter::create(&path, spec).unwrap();
            writer.write_block(PcmBlock::S24Le(&[0; 33]), &[]).unwrap();
            assert_eq!(writer.finish().unwrap().frames, 1);

            let manifest = fs::read_to_string(path.join(format!("{stem}.atmos"))).unwrap();
            for suffix in ["metadata", "audio"] {
                let name = format!("{stem}.atmos.{suffix}");
                assert!(path.join(&name).is_file());
                assert!(manifest.contains(&format!("    {suffix}: \"{name}\"\n")));
            }
            assert_eq!(fs::read_dir(path).unwrap().count(), 3);
        }
    }
}

#[test]
fn damf_stems_reject_empty_paths_and_control_characters() {
    let tmp = Temp::new();
    for stem in ["", ".", "..", "a/b", "a\\b", "a:b", "a\n", "a\0"] {
        let mut spec = spec(Profile::DamfHome, 48_000);
        spec.stem = stem.into();
        let error = MasterWriter::create(tmp.path("invalid"), spec).unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidInput);
    }
    assert_eq!(fs::read_dir(&tmp.0).unwrap().count(), 0);
}
