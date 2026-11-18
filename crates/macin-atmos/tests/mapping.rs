use macin_atmos::*;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "macin-mapping-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
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
fn spec(profile: Profile) -> MasterSpec {
    MasterSpec {
        profile,
        objects: vec![ObjectSpec::new(TrackId(77), "Voice")],
        ..Default::default()
    }
}
fn chunk(path: &Path, id: [u8; 4]) -> Vec<u8> {
    macin_bwf::ChunkReader::new(fs::File::open(path).unwrap())
        .unwrap()
        .metadata(id, 1 << 20)
        .unwrap()
        .unwrap()
}
fn payload(data: &[u8], id: u8) -> &[u8] {
    let mut at = 4;
    while data[at] != 0 {
        let length = usize::from(u16::from_le_bytes([data[at + 1], data[at + 2]]));
        if data[at] == id {
            return &data[at + 3..at + 3 + length];
        }
        at += length + 4;
    }
    panic!("missing segment {id}")
}
fn write(
    path: &Path,
    spec: MasterSpec,
    objects: &[ObjectUpdate],
    beds: &[BedUpdate],
) -> WriteReport {
    let channels = spec.channels();
    let mut writer = MasterWriter::create(path, spec).unwrap();
    writer.schedule_updates(objects).unwrap();
    writer.schedule_bed_updates(beds).unwrap();
    writer
        .write_block(PcmBlock::F32Interleaved(&vec![0.125; 4800 * channels]), &[])
        .unwrap();
    writer.finish().unwrap()
}
fn object(sample: u64, patch: ObjectPatch) -> ObjectUpdate {
    ObjectUpdate {
        track: TrackId(77),
        sample,
        ramp_samples: patch.has_continuous().then_some(0),
        patch,
    }
}

#[test]
fn logic_preserves_explicit_mid_and_unspecified_on_separate_channels() {
    let dir = Temp::new();
    let mut spec = spec(Profile::Logic);
    spec.objects
        .push(ObjectSpec::new(TrackId(78), "Default distance"));
    spec.bed[0].binaural_mode = BinauralMode::Mid;
    spec.objects[0].initial.head_tracking = HeadTracking::HeadRelative;
    spec.objects[0].initial.binaural_mode = BinauralMode::Mid;
    let path = dir.path("mid.wav");
    let report = write(&path, spec, &[], &[]);
    assert!(report.diagnostics.is_empty());

    // 低三位：3 是显式 Mid，4 是未指定。高两位仍属于头追设置。
    let bytes = chunk(&path, *b"dbmd");
    let modes = &payload(&bytes, 10)[142 + 12..];
    assert_eq!(
        [modes[0], modes[1], modes[3], modes[10], modes[11]],
        [0x43, 0x44, 0x40, 0x03, 0x44]
    );
    let decoded = dbmd::read(&bytes, 12).unwrap();
    assert!(decoded.unsupported.is_empty());
    let channels = decoded.channels.unwrap();
    assert_eq!(channels[0].binaural_mode, BinauralMode::Mid);
    assert_eq!(channels[10].binaural_mode, BinauralMode::Mid);
    assert_eq!(channels[11].binaural_mode, BinauralMode::Undefined);

    #[cfg(feature = "read")]
    {
        let reader = adm::AdmReader::open(&path, LossPolicy::Reject).unwrap();
        let output = dir.path("copy.wav");
        let result = reader
            .convert(
                reader.audio().unwrap(),
                &[Target {
                    path: output.clone(),
                    profile: Profile::Logic,
                }],
            )
            .unwrap();
        assert!(result.read.is_empty() && result.outputs[0].diagnostics.is_empty());
        assert_eq!(chunk(&output, *b"dbmd"), bytes);

        let home_path = dir.path("home");
        let targets = [Target {
            path: home_path.clone(),
            profile: Profile::DamfHome,
        }];
        let rejected = reader
            .convert(reader.audio().unwrap(), &targets)
            .unwrap_err();
        assert_eq!(rejected.field, Some("binaural_mode"));
        let allowed = adm::AdmReader::open(path, LossPolicy::Allow).unwrap();
        let report = allowed.convert(allowed.audio().unwrap(), &targets).unwrap();
        assert!(report.read.is_empty());
        let losses = &report.outputs[0].diagnostics;
        assert_eq!(losses.len(), 2);
        for field in ["binaural_mode", "bed_binaural_mode"] {
            assert!(
                losses
                    .iter()
                    .any(|d| d.kind == MappingDiagnosticKind::Loss && d.field == field)
            );
        }
        let home =
            damf::DamfReader::open(home_path.join("master.atmos"), LossPolicy::Reject).unwrap();
        assert_eq!(
            home.spec().objects[0].initial.binaural_mode,
            BinauralMode::Undefined
        );
        assert_eq!(home.spec().bed[0].binaural_mode, BinauralMode::Undefined);
        assert_eq!(home.spec().bed[3].binaural_mode, BinauralMode::Off);
    }
}

#[test]
fn dbmd_bytes_match_verified_static_fields_and_validate_container_integrity() {
    let mut spec = spec(Profile::Logic);
    spec.frame_rate = FrameRate::Fps25;
    spec.sc_bed_configuration = vec![0, 2, 8];
    spec.downmix_mode = DownmixMode::LtRtPlII;
    spec.surround_phase_shift = true;
    spec.bed_distribution = true;
    spec.warp_mode = WarpMode::Warping;
    spec.trim_mode = [None; 4];
    spec.trim_mode[TrimConfiguration::SomeSurroundsNoHeights.index()] = Some(TrimValues {
        center: -3.,
        surround: 0.25,
        height: 0.5,
        front_back_overhead_floor: 1.,
        front_back_listener: -0.5,
    });
    spec.objects[0].initial.head_tracking = HeadTracking::Undefined;
    spec.objects[0].initial.binaural_mode = BinauralMode::Near;
    spec.objects[0].initial.trim_bypass = true;
    let channels: Vec<StaticChannelMetadata> = spec
        .bed
        .iter()
        .copied()
        .map(Into::into)
        .chain(spec.objects.iter().map(|o| o.initial.into()))
        .collect();
    let data = dbmd::build_from_spec(&spec, &channels).unwrap();
    let atmos = payload(&data, 9);
    assert_eq!(
        (atmos[111], atmos[135], atmos[136], atmos[139], atmos[152]),
        (0x23, 5, 4, 0x31, 0x41)
    );
    let supplemental = payload(&data, 10);
    assert_eq!(supplemental[22], 0);
    assert_eq!(&supplemental[23..27], &(-3_f32).to_le_bytes());
    assert_eq!((supplemental[35], supplemental[36]), (255, 64));
    assert_eq!(supplemental[142 + 10], 1);
    assert_eq!(supplemental[142 + 11 + 10], 0x81);
    let parsed = dbmd::read(&data, 11).unwrap();
    assert!(parsed.unsupported.is_empty());
    assert_eq!(parsed.frame_rate, Some(FrameRate::Fps25));
    assert_eq!(parsed.sc_bed_configuration, [0, 2, 8]);
    assert_eq!(parsed.downmix_mode, DownmixMode::LtRtPlII);
    assert_eq!(parsed.warp_mode, WarpMode::Warping);
    assert!(parsed.surround_phase_shift && parsed.bed_distribution);
    assert!(parsed.bed_distribution_present);
    assert_eq!(parsed.channels.unwrap(), channels);
    let trim = parsed.trim_mode.unwrap()[0].unwrap();
    assert_eq!(trim.center, -3.);
    assert_eq!(trim.front_back_listener, -64. / 127.);
    assert!(dbmd::read(&data, 12).is_err());
    let mut corrupt = data.clone();
    corrupt[10] ^= 1;
    assert_eq!(
        dbmd::read(&corrupt, 11).unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    assert!(dbmd::read(&data[..data.len() - 3], 11).is_err());
    spec.set_bed_distribution(None);
    spec.sc_bed_configuration.clear();
    let unspecified = dbmd::build_from_spec(&spec, &channels).unwrap();
    assert_eq!(payload(&unspecified, 9)[152], 0x81);
    assert!(
        dbmd::read(&unspecified, 11)
            .unwrap()
            .sc_bed_configuration
            .is_empty()
    );
    assert!(
        !dbmd::read(&unspecified, 11)
            .unwrap()
            .bed_distribution_present
    );
}

#[test]
fn logic_static_metadata_uses_sample_zero_and_dynamic_loss_keeps_that_state() {
    let dir = Temp::new();
    let mut spec = spec(Profile::Logic);
    let initial = object(
        0,
        ObjectPatch {
            head_tracking: Some(HeadTracking::HeadRelative),
            binaural_mode: Some(BinauralMode::Near),
            trim_bypass: Some(true),
            ..Default::default()
        },
    );
    let later = object(
        1200,
        ObjectPatch {
            head_tracking: Some(HeadTracking::SceneRelative),
            binaural_mode: Some(BinauralMode::Far),
            trim_bypass: Some(false),
            ..Default::default()
        },
    );
    let mut rejected = MasterWriter::create(dir.path("reject.wav"), spec.clone()).unwrap();
    let error = rejected.schedule_updates(&[initial, later]).unwrap_err();
    assert_eq!(error.field, Some("head_tracking"));
    drop(rejected);
    spec.loss_policy = LossPolicy::Allow;
    let target = dir.path("allow.wav");
    let report = write(&target, spec, &[initial, later], &[]);
    assert_eq!(report.diagnostics.len(), 3);
    let parsed = dbmd::read(&chunk(&target, *b"dbmd"), 11).unwrap();
    let channel = parsed.channels.unwrap()[10];
    assert_eq!(channel.head_tracking, HeadTracking::HeadRelative);
    assert_eq!(channel.binaural_mode, BinauralMode::Near);
    assert!(channel.trim_bypass);
    assert!(
        !String::from_utf8(chunk(&target, *b"axml"))
            .unwrap()
            .contains("headLocked")
    );
}

#[test]
fn bed_preflight_is_atomic_across_profiles() {
    let dir = Temp::new();
    let targets = [
        Target {
            path: dir.path("logic.wav"),
            profile: Profile::Logic,
        },
        Target {
            path: dir.path("home"),
            profile: Profile::DamfHome,
        },
    ];
    let mut writer = MultiWriter::create(&spec(Profile::Logic), &targets).unwrap();
    let initial = BedUpdate {
        channel: 0,
        sample: 0,
        ramp_samples: None,
        patch: BedPatch {
            head_tracking: Some(HeadTracking::HeadRelative),
            ..Default::default()
        },
    };
    let gain = BedUpdate {
        channel: 0,
        sample: 1200,
        ramp_samples: Some(100),
        patch: BedPatch {
            gain: Some(Gain::Decibels(-3.)),
            ..Default::default()
        },
    };
    assert_eq!(
        writer
            .schedule_bed_updates(&[initial, gain])
            .unwrap_err()
            .field,
        Some("bed_ramp")
    );
    assert_eq!(
        writer.spec().bed[0].head_tracking,
        HeadTracking::SceneRelative
    );
    writer.schedule_bed_updates(&[initial]).unwrap();
    writer
        .write_block(PcmBlock::F32Interleaved(&vec![0.125; 4800 * 11]), &[])
        .unwrap();
    let reports = writer.finish().unwrap();
    assert!(reports.iter().all(|r| r.diagnostics.is_empty()));
    assert_eq!(
        dbmd::read(&chunk(&targets[0].path, *b"dbmd"), 11)
            .unwrap()
            .channels
            .unwrap()[0]
            .head_tracking,
        HeadTracking::HeadRelative
    );
    let metadata = fs::read_to_string(targets[1].path.join("master.atmos.metadata")).unwrap();
    assert!(!metadata.contains("samplePos: 1200"));
}

#[test]
fn custom_content_graph_must_reference_declared_audio() {
    let mut spec = spec(Profile::Standard);
    spec.contents = vec![ContentSpec {
        adm: AdmMetadata::default(),
        name: "Voice".into(),
        include_bed: false,
        objects: vec![TrackId(77)],
        dialogue: None,
    }];
    assert!(spec.validate().is_err());
    spec.contents[0].include_bed = true;
    spec.validate().unwrap();
    spec.contents[0].objects.push(TrackId(999));
    assert!(spec.validate().is_err());
    spec.objects.clear();
    spec.contents = vec![
        ContentSpec {
            adm: AdmMetadata::default(),
            name: String::new(),
            include_bed: true,
            objects: Vec::new(),
            dialogue: None
        };
        0xf000
    ];
    assert!(spec.validate().unwrap_err().message.contains("Too many"));
    spec.contents.pop();
    spec.validate().unwrap();
}

#[test]
fn standard_does_not_silently_normalize_unspecified_presentation_settings() {
    let dir = Temp::new();
    let mut spec = spec(Profile::Standard);
    spec.set_bed_distribution(None);
    assert_eq!(
        MasterWriter::create(dir.path("distribution.wav"), spec.clone())
            .unwrap_err()
            .field,
        Some("bed_distribution")
    );
    spec.set_bed_distribution(Some(false));
    spec.trim_mode = [None; 4];
    assert_eq!(
        MasterWriter::create(dir.path("trim.wav"), spec.clone())
            .unwrap_err()
            .field,
        Some("trim_mode")
    );
    spec.loss_policy = LossPolicy::Allow;
    let report = write(&dir.path("allowed.wav"), spec.clone(), &[], &[]);
    assert_eq!(report.diagnostics[0].field, "trim_mode");
    spec.profile = Profile::DamfHome;
    spec.loss_policy = LossPolicy::Reject;
    spec.sc_bed_configuration.clear();
    assert_eq!(
        MasterWriter::create(dir.path("unset-home"), spec.clone())
            .unwrap_err()
            .field,
        Some("sc_bed_configuration")
    );
    assert_eq!(
        metadata::damf_manifest(&spec).unwrap_err().kind,
        ErrorKind::Unsupported
    );
    spec.loss_policy = LossPolicy::Allow;
    let report = write(&dir.path("fallback-home"), spec, &[], &[]);
    assert_eq!(report.diagnostics[0].field, "sc_bed_configuration");
    assert!(
        fs::read_to_string(dir.path("fallback-home/master.atmos"))
            .unwrap()
            .contains("scBedConfiguration: [3]")
    );
}

#[cfg(feature = "read")]
fn rewrite_xml(source: &Path, target: &Path, change: impl FnOnce(String) -> String) {
    let xml = String::from_utf8(chunk(source, *b"axml")).unwrap();
    macin_bwf::replace_chunk(
        fs::File::open(source).unwrap(),
        fs::File::create(target).unwrap(),
        *b"axml",
        change(xml).as_bytes(),
    )
    .unwrap();
}

#[cfg(feature = "read")]
#[test]
fn dbmd_and_presentation_fields_survive_logic_damf_logic_without_external_overrides() {
    use macin_atmos::{adm::AdmReader, damf::DamfReader};
    let dir = Temp::new();
    let source = dir.path("source.wav");
    let mut original = spec(Profile::Logic);
    original.frame_rate = FrameRate::Fps25;
    original.start_sample = 4800;
    original.sc_bed_configuration = vec![2, 8];
    original.downmix_mode = DownmixMode::LtRtProLogic;
    original.surround_phase_shift = true;
    original.bed_distribution = true;
    original.warp_mode = WarpMode::ProLogicIIx;
    original.trim_mode = [None; 4];
    original.trim_mode[1] = Some(TrimValues {
        center: -3.,
        front_back_listener: 1.,
        ..Default::default()
    });
    original.objects[0].initial.head_tracking = HeadTracking::Undefined;
    original.objects[0].initial.binaural_mode = BinauralMode::Far;
    original.objects[0].initial.trim_bypass = true;
    original.bed[1].head_tracking = HeadTracking::HeadRelative;
    original.bed[1].binaural_mode = BinauralMode::Near;
    write(&source, original.clone(), &[], &[]);
    let read = AdmReader::open(&source, LossPolicy::Reject).unwrap();
    assert!(read.diagnostics().is_empty());
    assert_eq!(read.spec().frame_rate, FrameRate::Fps25);
    assert_eq!(read.spec().start_sample, 4800);
    assert_eq!(
        read.spec().objects[0].initial.head_tracking,
        HeadTracking::Undefined
    );
    assert_eq!(read.spec().bed[1].head_tracking, HeadTracking::HeadRelative);
    let home = dir.path("home");
    read.convert(
        read.audio().unwrap(),
        &[Target {
            path: home.clone(),
            profile: Profile::DamfHome,
        }],
    )
    .unwrap();
    let damf = DamfReader::open(home.join("master.atmos"), LossPolicy::Reject).unwrap();
    assert_eq!(
        damf.spec().sc_bed_configuration,
        original.sc_bed_configuration
    );
    assert_eq!(damf.spec().downmix_mode, original.downmix_mode);
    assert_eq!(damf.spec().warp_mode, original.warp_mode);
    assert_eq!(damf.spec().trim_mode, original.trim_mode);
    assert_eq!(damf.spec().start_sample, 4800);
    let back = dir.path("back.wav");
    damf.convert(
        damf.audio().unwrap(),
        &[Target {
            path: back.clone(),
            profile: Profile::Logic,
        }],
    )
    .unwrap();
    assert_eq!(chunk(&source, *b"dbmd"), chunk(&back, *b"dbmd"));
    assert_eq!(chunk(&source, *b"data"), chunk(&back, *b"data"));
}

#[cfg(feature = "read")]
#[test]
fn every_zone_and_elevation_preset_is_exact_and_dynamic_changes_are_retained() {
    use macin_atmos::adm::AdmReader;
    let dir = Temp::new();
    for (index, zone) in [
        Zone::All,
        Zone::NoBack,
        Zone::NoSides,
        Zone::CenterBack,
        Zone::ScreenOnly,
        Zone::SurroundOnly,
    ]
    .into_iter()
    .enumerate()
    {
        for elevation in [false, true] {
            let path = dir.path(&format!("{index}-{elevation}.wav"));
            let mut spec = spec(Profile::Standard);
            spec.objects[0].initial.zone = zone;
            spec.objects[0].initial.elevation = elevation;
            write(
                &path,
                spec,
                &[object(
                    1200,
                    ObjectPatch {
                        zone: Some(Zone::SurroundOnly),
                        elevation: Some(!elevation),
                        ..Default::default()
                    },
                )],
                &[],
            );
            let read = AdmReader::open(path, LossPolicy::Reject).unwrap();
            assert_eq!(
                (
                    read.spec().objects[0].initial.zone,
                    read.spec().objects[0].initial.elevation
                ),
                (zone, elevation)
            );
            let updates: Vec<_> = read.updates().unwrap().map(Result::unwrap).collect();
            assert_eq!(updates.len(), 1);
            let final_state = updates[0].patch.apply(read.spec().objects[0].initial);
            assert_eq!(
                (final_state.zone, final_state.elevation),
                (Zone::SurroundOnly, !elevation)
            );
        }
    }
    let source = dir.path("known.wav");
    let mut spec = spec(Profile::Standard);
    spec.objects[0].initial.zone = Zone::NoBack;
    write(&source, spec, &[], &[]);
    let changed = dir.path("changed.wav");
    rewrite_xml(&source, &changed, |s| s.replace("-0.41934", "-0.41935"));
    let read = AdmReader::open(changed, LossPolicy::Reject).unwrap();
    assert_eq!(read.spec().objects[0].initial.zone, Zone::All);
    assert!(!read.spec().objects[0].custom_zones.is_empty());
    assert!(read.diagnostics().is_empty());
    let out = dir.path("literal-zones.wav");
    read.convert(
        read.audio().unwrap(),
        &[Target {
            path: out.clone(),
            profile: Profile::Standard,
        }],
    )
    .unwrap();
    assert!(
        String::from_utf8(chunk(&out, *b"axml"))
            .unwrap()
            .contains("-0.41935")
    );
}

#[cfg(feature = "read")]
#[test]
fn native_adm_head_lock_inherits_objects_and_resets_missing_blocks() {
    use macin_atmos::adm::AdmReader;
    let dir = Temp::new();
    let source = dir.path("source.wav");
    let mut spec = spec(Profile::Standard);
    spec.objects[0].initial.head_tracking = HeadTracking::HeadRelative;
    write(
        &source,
        spec,
        &[
            object(
                1200,
                ObjectPatch {
                    head_tracking: Some(HeadTracking::SceneRelative),
                    ..Default::default()
                },
            ),
            object(
                2400,
                ObjectPatch {
                    head_tracking: Some(HeadTracking::HeadRelative),
                    ..Default::default()
                },
            ),
        ],
        &[],
    );
    let reader = AdmReader::open(&source, LossPolicy::Reject).unwrap();
    assert_eq!(
        reader.spec().objects[0].initial.head_tracking,
        HeadTracking::HeadRelative
    );
    let updates: Vec<_> = reader
        .updates()
        .unwrap()
        .map(|u| {
            let u = u.unwrap();
            (u.sample, u.patch.head_tracking)
        })
        .collect();
    assert_eq!(
        updates,
        [
            (1200, Some(HeadTracking::SceneRelative)),
            (2400, Some(HeadTracking::HeadRelative))
        ]
    );
    let inherited = dir.path("inherited.wav");
    rewrite_xml(&source, &inherited, |s| {
        s.replacen(
            "</audioObject>",
            "<headLocked>1</headLocked></audioObject>",
            1,
        )
    });
    let reader = AdmReader::open(inherited, LossPolicy::Reject).unwrap();
    assert!(
        reader
            .spec()
            .bed
            .iter()
            .all(|b| b.head_tracking == HeadTracking::HeadRelative)
    );
}

#[cfg(feature = "read")]
#[test]
fn bed_updates_are_streamed_and_do_not_share_object_ids() {
    use macin_atmos::damf::DamfReader;
    let dir = Temp::new();
    let source = dir.path("source");
    let mut spec = spec(Profile::DamfHome);
    spec.objects[0].id = TrackId(1);
    spec.bed[1].gain = Gain::Decibels(-3.);
    spec.bed[1].head_tracking = HeadTracking::HeadRelative;
    spec.bed[1].trim_bypass = true;
    let updates = [
        BedUpdate {
            channel: 1,
            sample: 1200,
            ramp_samples: Some(600),
            patch: BedPatch {
                gain: Some(Gain::Decibels(-6.)),
                ..Default::default()
            },
        },
        BedUpdate {
            channel: 1,
            sample: 2400,
            ramp_samples: None,
            patch: BedPatch {
                active: Some(false),
                head_tracking: Some(HeadTracking::Undefined),
                ..Default::default()
            },
        },
    ];
    write(&source, spec, &[], &updates);
    let reader = DamfReader::open(source.join("master.atmos"), LossPolicy::Reject).unwrap();
    assert_eq!(reader.update_count(), 0);
    assert_eq!(reader.bed_update_count(), 2);
    assert_eq!(reader.spec().bed[1].gain, Gain::Decibels(-3.));
    for _ in 0..2 {
        assert_eq!(
            reader
                .bed_updates()
                .unwrap()
                .map(Result::unwrap)
                .collect::<Vec<_>>(),
            updates
        );
    }
    let target = dir.path("copy");
    reader
        .convert(
            reader.audio().unwrap(),
            &[Target {
                path: target.clone(),
                profile: Profile::DamfHome,
            }],
        )
        .unwrap();
    for ext in ["atmos", "atmos.audio", "atmos.metadata"] {
        assert_eq!(
            fs::read(source.join(format!("master.{ext}"))).unwrap(),
            fs::read(target.join(format!("master.{ext}"))).unwrap()
        );
    }
    let failure = reader
        .convert(
            reader.audio().unwrap(),
            &[Target {
                path: dir.path("bad.wav"),
                profile: Profile::Logic,
            }],
        )
        .unwrap_err();
    assert_eq!(failure.field, Some("bed_ramp"));
}

#[cfg(feature = "read")]
#[test]
fn content_groups_and_programme_span_survive_adm_conversion() {
    use macin_atmos::adm::AdmReader;
    let dir = Temp::new();
    let source = dir.path("source.wav");
    let mut spec = spec(Profile::Standard);
    spec.start_sample = 4800;
    spec.contents = vec![
        ContentSpec {
            adm: AdmMetadata::default(),
            name: "Bed group".into(),
            include_bed: true,
            objects: vec![],
            dialogue: None,
        },
        ContentSpec {
            adm: AdmMetadata::default(),
            name: "Voice group".into(),
            include_bed: false,
            objects: vec![TrackId(77)],
            dialogue: Some(ContentDialogue {
                kind: ContentKind::Dialogue,
                subtype: 2,
            }),
        },
    ];
    write(&source, spec, &[], &[]);
    let reader = AdmReader::open(&source, LossPolicy::Reject).unwrap();
    assert_eq!(reader.spec().start_sample, 4800);
    assert_eq!(reader.spec().contents.len(), 2);
    assert_eq!(reader.spec().contents[1].name, "Voice group");
    assert_eq!(reader.spec().contents[1].objects, [TrackId(11)]);
    let copy = dir.path("copy.wav");
    reader
        .convert(
            reader.audio().unwrap(),
            &[Target {
                path: copy.clone(),
                profile: Profile::Standard,
            }],
        )
        .unwrap();
    assert_eq!(fs::read(&source).unwrap(), fs::read(&copy).unwrap());
    assert_eq!(
        reader
            .convert(
                reader.audio().unwrap(),
                &[Target {
                    path: dir.path("home"),
                    profile: Profile::DamfHome
                }]
            )
            .unwrap_err()
            .field,
        Some("content_structure")
    );
    let short = dir.path("short.wav");
    rewrite_xml(&source, &short, |s| {
        s.replace("end=\"00:00:00.200000000\"", "end=\"00:00:00.150000000\"")
    });
    let reader = AdmReader::open(short, LossPolicy::Reject).unwrap();
    let mut audio = reader.audio().unwrap();
    assert_eq!(audio.frames(), 2400);
    assert_eq!(audio.read_s24(123, &mut Vec::new()).unwrap(), 123);
    let target = dir.path("clipped.wav");
    let report = reader
        .convert(
            audio,
            &[Target {
                path: target.clone(),
                profile: Profile::Standard,
            }],
        )
        .unwrap();
    assert_eq!(report.outputs[0].frames, 2400);
    assert_eq!(
        chunk(&target, *b"data"),
        chunk(&source, *b"data")[..2400 * 11 * 3]
    );
}

#[cfg(feature = "read")]
#[test]
fn independent_adm_names_are_retained_without_changing_references() {
    use macin_atmos::adm::AdmReader;
    let dir = Temp::new();
    for (index, profile) in [Profile::Standard, Profile::Logic].into_iter().enumerate() {
        let mut original = spec(profile);
        original.objects[0].pack_name = Some("Voice pack".into());
        original.objects[0].channel_names = ChannelNames {
            extensions: Default::default(),
            channel: Some("对象<&>".into()),
            stream: Some("Voice stream".into()),
            track: Some("Voice track".into()),
        };
        original.bed_channel_names[0] = ChannelNames {
            extensions: Default::default(),
            channel: Some("Front left".into()),
            stream: Some("L stream".into()),
            track: Some("L track".into()),
        };
        let source = dir.path(&format!("source-{index}.wav"));
        write(&source, original.clone(), &[], &[]);
        let reader = AdmReader::open(&source, LossPolicy::Reject).unwrap();
        assert_eq!(
            reader.spec().objects[0].pack_name,
            original.objects[0].pack_name
        );
        assert_eq!(
            reader.spec().objects[0].channel_names,
            original.objects[0].channel_names
        );
        assert_eq!(
            reader.spec().bed_channel_names[0],
            original.bed_channel_names[0]
        );
        let output = dir.path(&format!("output-{index}.wav"));
        reader
            .convert(
                reader.audio().unwrap(),
                &[Target {
                    path: output.clone(),
                    profile,
                }],
            )
            .unwrap();
        assert_eq!(fs::read(source).unwrap(), fs::read(output).unwrap());
    }
}

#[cfg(feature = "read")]
#[test]
fn unsupported_attributes_extensions_and_container_chunks_are_reported() {
    use macin_atmos::adm::AdmReader;
    let dir = Temp::new();
    let source = dir.path("source.wav");
    write(&source, spec(Profile::Standard), &[], &[]);
    for (index, before, after, field) in [
        (
            0,
            "<audioProgramme ",
            "<audioProgramme auditFlag=\"1\" ",
            "unknown_field",
        ),
        (
            1,
            "</audioProgramme>",
            "<auditExtension>value</auditExtension></audioProgramme>",
            "unknown_field",
        ),
        (
            2,
            "<audioContent ",
            "<audioContent auditFlag=\"1\" ",
            "unknown_field",
        ),
        (
            3,
            "</audioContent>",
            "<auditExtension>value</auditExtension></audioContent>",
            "unknown_field",
        ),
        (
            4,
            "<audioPackFormat ",
            "<audioPackFormat auditFlag=\"3\" ",
            "unknown_field",
        ),
    ] {
        let target = dir.path(&format!("extension-{index}.wav"));
        rewrite_xml(&source, &target, |s| s.replacen(before, after, 1));
        assert_eq!(
            AdmReader::open(&target, LossPolicy::Reject)
                .unwrap_err()
                .field,
            Some(field)
        );
        let reader = AdmReader::open(target, LossPolicy::Allow).unwrap();
        assert!(reader.diagnostics().iter().any(|d| d.field == field));
    }
    {
        let (name, addition, field) = ("attribute", " auditFlag=\"1\"", "unknown_field");
        let target = dir.path(&format!("position-{name}.wav"));
        rewrite_xml(&source, &target, |mut s| {
            let channel = s
                .find("<audioChannelFormat audioChannelFormatID=\"AC_00031001\"")
                .unwrap();
            let at = channel + s[channel..].find("<position").unwrap() + "<position".len();
            s.insert_str(at, addition);
            s
        });
        assert_eq!(
            AdmReader::open(&target, LossPolicy::Reject)
                .unwrap_err()
                .field,
            Some(field)
        );
        assert_eq!(
            AdmReader::open(target, LossPolicy::Allow)
                .unwrap()
                .diagnostics()[0]
                .field,
            field
        );
    }
    let bounded = dir.path("bounded.wav");
    rewrite_xml(&source, &bounded, |mut s| {
        let channel = s
            .find("<audioChannelFormat audioChannelFormatID=\"AC_00031001\"")
            .unwrap();
        let end = channel + s[channel..].find("</audioBlockFormat>").unwrap();
        s.insert_str(
            end,
            "<position coordinate=\"X\" bound=\"min\">-1</position>",
        );
        s
    });
    assert_eq!(
        AdmReader::open(&bounded, LossPolicy::Reject)
            .unwrap_err()
            .field,
        Some("position_bound")
    );
    assert_eq!(
        AdmReader::open(bounded, LossPolicy::Allow)
            .unwrap()
            .spec()
            .objects[0]
            .initial
            .position[0],
        0.
    );

    let mut two = spec(Profile::Standard);
    two.objects.push(ObjectSpec::new(TrackId(88), "Other"));
    let two_source = dir.path("two.wav");
    write(&two_source, two, &[], &[]);
    let packs = dir.path("packs.wav");
    rewrite_xml(&two_source, &packs, |s| {
        s.replacen("<audioPackFormatIDRef>AP_00031001</audioPackFormatIDRef>", "<audioPackFormatIDRef>AP_00031001</audioPackFormatIDRef><audioPackFormatIDRef>AP_00031002</audioPackFormatIDRef>", 1)
    });
    assert_eq!(
        AdmReader::open(&packs, LossPolicy::Reject)
            .unwrap_err()
            .field,
        Some("pack_structure")
    );
    assert!(
        AdmReader::open(packs, LossPolicy::Allow)
            .unwrap()
            .diagnostics()
            .iter()
            .any(|d| d.field == "pack_structure")
    );
    let extra = dir.path("bext.wav");
    fs::copy(&source, &extra).unwrap();
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&extra)
        .unwrap();
    let bext = macin_bwf::bext_payload(&macin_bwf::BextFields {
        description: b"synthetic",
        originator: b"test",
        originator_reference: b"test",
        date_utc: b"2020-01-01T00:00:00Z",
        loudness: None,
        true_peak: None,
    });
    macin_bwf::append_bext(&mut file, &bext, None).unwrap();
    drop(file);
    let reader = AdmReader::open(&extra, LossPolicy::Reject).unwrap();
    assert!(reader.diagnostics().is_empty());
    let target = dir.path("preserved-bext.wav");
    reader
        .convert(
            reader.audio().unwrap(),
            &[Target {
                path: target.clone(),
                profile: Profile::Standard,
            }],
        )
        .unwrap();
    assert_eq!(chunk(&target, *b"bext"), bext);

    let logic = dir.path("logic.wav");
    write(&logic, spec(Profile::Logic), &[], &[]);
    let mut bytes = chunk(&logic, *b"dbmd");
    bytes[10] ^= 1;
    let corrupt = dir.path("corrupt.wav");
    macin_bwf::replace_chunk(
        fs::File::open(logic).unwrap(),
        fs::File::create(&corrupt).unwrap(),
        *b"dbmd",
        &bytes,
    )
    .unwrap();
    for policy in [LossPolicy::Reject, LossPolicy::Allow] {
        let error = AdmReader::open(&corrupt, policy).unwrap_err();
        assert_eq!(
            (error.kind, error.field),
            (ErrorKind::InvalidInput, Some("dbmd"))
        );
    }
}

#[cfg(feature = "read")]
#[test]
fn unknown_dbmd_segments_can_be_dropped_but_known_values_are_retained() {
    use macin_atmos::adm::AdmReader;
    let dir = Temp::new();
    let mut spec = spec(Profile::Logic);
    spec.frame_rate = FrameRate::Fps25;
    let source = dir.path("source.wav");
    write(&source, spec, &[], &[]);
    let original = chunk(&source, *b"dbmd");
    let mut extra = dbmd::DBMD_VERSION.to_vec();
    for id in [7, 9, 10] {
        dbmd::append_dbmd_segment(&mut extra, id, payload(&original, id)).unwrap();
    }
    dbmd::append_dbmd_segment(&mut extra, 42, &[1, 2, 3]).unwrap();
    extra.push(0);
    let target = dir.path("extra.wav");
    macin_bwf::replace_chunk(
        fs::File::open(&source).unwrap(),
        fs::File::create(&target).unwrap(),
        *b"dbmd",
        &extra,
    )
    .unwrap();
    assert_eq!(
        AdmReader::open(&target, LossPolicy::Reject)
            .unwrap_err()
            .field,
        Some("dbmd_encoding")
    );
    let reader = AdmReader::open(target, LossPolicy::Allow).unwrap();
    assert_eq!(reader.spec().frame_rate, FrameRate::Fps25);
    assert_eq!(reader.diagnostics()[0].field, "dbmd_encoding");

    let mut version = original;
    version[0] = 7;
    let target = dir.path("version.wav");
    macin_bwf::replace_chunk(
        fs::File::open(&source).unwrap(),
        fs::File::create(&target).unwrap(),
        *b"dbmd",
        &version,
    )
    .unwrap();
    assert_eq!(
        AdmReader::open(&target, LossPolicy::Reject)
            .unwrap_err()
            .field,
        Some("dbmd")
    );
    assert_eq!(
        AdmReader::open(target, LossPolicy::Allow)
            .unwrap()
            .diagnostics()[0]
            .field,
        "dbmd"
    );
    for (name, field) in [
        ("frame", "frame_rate"),
        ("head", "head_tracking"),
        ("reserved-binaural", "binaural_mode"),
    ] {
        let mut edited = dbmd::DBMD_VERSION.to_vec();
        for id in [7, 9, 10] {
            let mut value = payload(&chunk(&source, *b"dbmd"), id).to_vec();
            if name == "frame" && id == 9 {
                value[111] = 0x99;
            }
            if name == "head" && id == 10 {
                value[142 + 11 + 10] = 0xc4;
            }
            if name == "reserved-binaural" && id == 10 {
                value[142 + 11 + 10] = 0x45;
            }
            dbmd::append_dbmd_segment(&mut edited, id, &value).unwrap();
        }
        edited.push(0);
        let target = dir.path(&format!("unknown-{name}.wav"));
        macin_bwf::replace_chunk(
            fs::File::open(&source).unwrap(),
            fs::File::create(&target).unwrap(),
            *b"dbmd",
            &edited,
        )
        .unwrap();
        assert_eq!(
            AdmReader::open(&target, LossPolicy::Reject)
                .unwrap_err()
                .field,
            Some(field)
        );
        let read = AdmReader::open(target, LossPolicy::Allow).unwrap();
        assert_eq!(read.diagnostics()[0].field, field);
        if name == "frame" {
            assert_eq!(read.spec().frame_rate, FrameRate::Fps24);
        } else if name == "head" {
            assert_eq!(
                read.spec().objects[0].initial.head_tracking,
                HeadTracking::Undefined
            );
        } else {
            let state = read.spec().objects[0].initial;
            assert_eq!(state.binaural_mode, BinauralMode::Undefined);
            assert_eq!(state.head_tracking, HeadTracking::SceneRelative);
        }
    }
}

#[cfg(feature = "read")]
#[test]
fn caf_metadata_before_and_after_audio_requires_explicit_loss_permission() {
    use macin_atmos::damf::{CafReader, DamfReader};
    let dir = Temp::new();
    for before in [true, false] {
        let source = dir.path(&format!("source-{before}"));
        write(&source, spec(Profile::DamfHome), &[], &[]);
        let audio = source.join("master.atmos.audio");
        let original = fs::read(&audio).unwrap();
        let mut info = Vec::from(&b"info"[..]);
        let body = b"\0\0\0\x01artist\0test\0";
        info.extend_from_slice(&(body.len() as i64).to_be_bytes());
        info.extend_from_slice(body);
        let mut changed = original.clone();
        let at = if before { 52 } else { changed.len() };
        changed.splice(at..at, info);
        fs::write(&audio, changed).unwrap();
        let reader = DamfReader::open(source.join("master.atmos"), LossPolicy::Reject).unwrap();
        let target = dir.path(&format!("copy-{before}"));
        let report = reader
            .convert(
                reader.audio().unwrap(),
                &[Target {
                    path: target.clone(),
                    profile: Profile::DamfHome,
                }],
            )
            .unwrap();
        assert!(report.read.is_empty());
        assert!(report.outputs[0].diagnostics.is_empty());
        let output = fs::read(target.join("master.atmos.audio")).unwrap();
        assert_eq!(&output[..original.len()], &original);
        assert_eq!(&output[original.len() + 12..], body);
        let mut duplicate = original;
        duplicate.extend_from_slice(b"data");
        duplicate.extend_from_slice(&4_i64.to_be_bytes());
        duplicate.extend_from_slice(&[0; 4]);
        fs::write(&audio, duplicate).unwrap();
        assert_eq!(
            CafReader::open(audio).unwrap_err().kind,
            ErrorKind::InvalidInput
        );
    }
}
