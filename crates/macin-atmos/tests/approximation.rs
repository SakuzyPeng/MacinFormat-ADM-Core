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
            "macin-atmos-approximation-{}-{}",
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
        objects: vec![ObjectSpec::new(TrackId(7), "Object")],
        ..Default::default()
    }
}

fn update(sample: u64, patch: ObjectPatch) -> ObjectUpdate {
    ObjectUpdate {
        track: TrackId(7),
        sample,
        ramp_samples: None,
        patch,
    }
}

fn write(path: &Path, spec: MasterSpec, updates: &[ObjectUpdate]) -> WriteReport {
    let mut writer = MasterWriter::create(path, spec).unwrap();
    writer.schedule_updates(updates).unwrap();
    writer
        .write_block(PcmBlock::F32Interleaved(&vec![0.125; 4800 * 11]), &[])
        .unwrap();
    writer.finish().unwrap()
}

fn snapshot(path: &Path) -> macin_adm::Snapshot {
    let mut reader = macin_bwf::ChunkReader::new(fs::File::open(path).unwrap()).unwrap();
    let xml = String::from_utf8(reader.metadata(*b"axml", 1 << 20).unwrap().unwrap()).unwrap();
    let chna =
        macin_bwf::Chna::decode(&reader.metadata(*b"chna", 1 << 20).unwrap().unwrap()).unwrap();
    let tracks: Vec<_> = chna
        .entries
        .iter()
        .map(|e| (String::from_utf8(e.uid.to_vec()).unwrap(), e.track_index))
        .collect();
    macin_adm::Snapshot::extract(&macin_adm::Document::parse(&xml).unwrap(), 48_000, &tracks)
        .unwrap()
}

#[test]
fn explicit_diffuse_mapping_writes_updates_and_returns_to_default() {
    let dir = Temp::new();
    for (index, profile) in [Profile::Standard, Profile::Logic].into_iter().enumerate() {
        let mut spec = spec(profile);
        spec.approximate_mappings.diffuse_decorrelation = true;
        spec.objects[0].initial.decorrelation = 0.25;
        let path = dir.path(&format!("{index}.wav"));
        let report = write(
            &path,
            spec,
            &[
                update(
                    1200,
                    ObjectPatch {
                        decorrelation: Some(0.75),
                        ..Default::default()
                    },
                ),
                update(
                    2400,
                    ObjectPatch {
                        decorrelation: Some(0.),
                        ..Default::default()
                    },
                ),
            ],
        );
        let scene = snapshot(&path);
        let values: Vec<_> = scene
            .object_blocks
            .iter()
            .map(|b| (b.start, b.diffuse))
            .collect();
        assert_eq!(values, [(0, 0.25), (1200, 0.75), (2400, 0.)]);
        assert_eq!(report.diagnostics.len(), 1);
        let diagnostic = &report.diagnostics[0];
        assert_eq!(diagnostic.kind, MappingDiagnosticKind::Approximation);
        assert_eq!(
            (diagnostic.track, diagnostic.sample, diagnostic.field),
            (Some(TrackId(7)), Some(0), "decorrelation")
        );
        assert!(diagnostic.message.contains("0.25 -> diffuse 0.25"));
    }
}

#[test]
fn choosing_one_approximation_does_not_authorize_other_fields() {
    let dir = Temp::new();
    for (index, approximation, field) in [
        (0, Approximation::DiffuseDecorrelation, "screen_factor"),
        (1, Approximation::ScreenRefFactor, "decorrelation"),
    ] {
        let mut spec = spec(Profile::Standard);
        spec.approximate_mappings.enable(approximation);
        spec.objects[0].initial.decorrelation = 0.25;
        spec.objects[0].initial.screen_factor = 0.5;
        let path = dir.path(&format!("rejected-{index}.wav"));
        let error = MasterWriter::create(&path, spec).err().unwrap();
        assert_eq!(error.field, Some(field));
        assert!(!path.exists());
    }
    let mut spec = spec(Profile::Standard);
    spec.loss_policy = LossPolicy::Allow;
    spec.objects[0].initial.decorrelation = 0.25;
    spec.objects[0].initial.screen_factor = 0.5;
    let path = dir.path("loss-only.wav");
    let report = write(&path, spec, &[]);
    let block = snapshot(&path).object_blocks[0];
    assert_eq!((block.diffuse, block.screen_ref), (0., 0));
    assert_eq!(report.diagnostics.len(), 2);
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.kind == MappingDiagnosticKind::Loss)
    );
}

#[test]
fn screen_factor_updates_can_turn_screen_ref_off_again() {
    let dir = Temp::new();
    let mut spec = spec(Profile::Standard);
    spec.approximate_mappings.screen_ref_factor = true;
    spec.objects[0].initial.screen_factor = 0.25;
    let path = dir.path("screen.wav");
    let report = write(
        &path,
        spec,
        &[
            update(
                1200,
                ObjectPatch {
                    screen_factor: Some(0.),
                    ..Default::default()
                },
            ),
            update(
                2400,
                ObjectPatch {
                    screen_factor: Some(0.75),
                    ..Default::default()
                },
            ),
        ],
    );
    let values: Vec<_> = snapshot(&path)
        .object_blocks
        .iter()
        .map(|b| (b.start, b.screen_ref))
        .collect();
    assert_eq!(values, [(0, 1), (1200, 0), (2400, 1)]);
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(
        report.diagnostics[0].kind,
        MappingDiagnosticKind::Approximation
    );
}

#[test]
fn conflicting_native_fields_reject_every_target_before_commit() {
    let dir = Temp::new();
    let mut spec = spec(Profile::Standard);
    spec.approximate_mappings.screen_ref_factor = true;
    let targets = [
        Target {
            path: dir.path("out.wav"),
            profile: Profile::Standard,
        },
        Target {
            path: dir.path("home"),
            profile: Profile::DamfHome,
        },
    ];
    let mut writer = MultiWriter::create(&spec, &targets).unwrap();
    let error = writer
        .schedule_updates(&[update(
            1200,
            ObjectPatch {
                screen_ref: Some(true),
                screen_factor: Some(0.4),
                ..Default::default()
            },
        )])
        .unwrap_err();
    assert_eq!(error.field, Some("screen_ref"));
    writer
        .schedule_updates(&[update(
            1200,
            ObjectPatch {
                screen_ref: Some(true),
                ..Default::default()
            },
        )])
        .unwrap();
    writer
        .write_block(PcmBlock::F32Interleaved(&vec![0.125; 4800 * 11]), &[])
        .unwrap();
    let reports = writer.finish().unwrap();
    assert!(
        reports[0].diagnostics.is_empty(),
        "failed preparations must not leave approximation diagnostics"
    );
    assert_eq!(reports[1].diagnostics.len(), 1);
    assert_eq!(reports[1].diagnostics[0].sample, Some(1200));
    let text = fs::read_to_string(targets[1].path.join("master.atmos.metadata")).unwrap();
    assert!(
        text.contains("samplePos: 1200\n    screenFactor: 1"),
        "{text}"
    );
    assert!(!text.contains("screenFactor: 0.4"));
}

#[test]
fn conflicting_screen_fields_keep_native_values_only_when_loss_is_allowed() {
    let dir = Temp::new();
    let mut damf = spec(Profile::DamfHome);
    damf.approximate_mappings.screen_ref_factor = true;
    damf.objects[0].initial.screen_ref = true;
    damf.objects[0].initial.screen_factor = 0.4;
    let error = MasterWriter::create(dir.path("reject-home"), damf.clone())
        .err()
        .unwrap();
    assert_eq!(error.field, Some("screen_ref"));
    damf.loss_policy = LossPolicy::Allow;
    let report = write(&dir.path("home"), damf, &[]);
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].kind, MappingDiagnosticKind::Loss);
    let metadata = fs::read_to_string(dir.path("home/master.atmos.metadata")).unwrap();
    assert!(metadata.contains("screenFactor: 0.4"), "{metadata}");

    let mut adm = spec(Profile::Standard);
    adm.approximate_mappings.screen_ref_factor = true;
    adm.objects[0].initial.screen_ref = true;
    let conflicting = update(
        1200,
        ObjectPatch {
            screen_ref: Some(false),
            screen_factor: Some(0.4),
            ..Default::default()
        },
    );
    let mut strict = MasterWriter::create(dir.path("reject.wav"), adm.clone()).unwrap();
    let error = strict.schedule_updates(&[conflicting]).unwrap_err();
    assert_eq!(error.field, Some("screen_factor"));
    drop(strict);
    adm.loss_policy = LossPolicy::Allow;
    let target = dir.path("allow.wav");
    let report = write(&target, adm, &[conflicting]);
    let values: Vec<_> = snapshot(&target)
        .object_blocks
        .iter()
        .map(|b| (b.start, b.screen_ref))
        .collect();
    assert_eq!(values, [(0, 1), (1200, 0)]);
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].kind, MappingDiagnosticKind::Loss);
    assert_eq!(report.diagnostics[0].field, "screen_factor");
}

#[cfg(feature = "read")]
#[test]
fn screen_ref_changes_survive_both_approximate_directions() {
    use macin_atmos::{adm::AdmReader, damf::DamfReader};
    let dir = Temp::new();
    let source = dir.path("source.wav");
    write(
        &source,
        spec(Profile::Standard),
        &[
            update(
                1200,
                ObjectPatch {
                    screen_ref: Some(true),
                    ..Default::default()
                },
            ),
            update(
                2400,
                ObjectPatch {
                    screen_ref: Some(false),
                    ..Default::default()
                },
            ),
        ],
    );
    let options = MappingOptions {
        approximate_mappings: ApproximateMappings {
            screen_ref_factor: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let reader = AdmReader::open_with_options(source, options).unwrap();
    let target = Target {
        path: dir.path("home"),
        profile: Profile::DamfHome,
    };
    let report = reader
        .convert(reader.audio().unwrap(), std::slice::from_ref(&target))
        .unwrap();
    assert!(report.read.is_empty());
    assert_eq!(
        report.outputs[0].diagnostics[0].kind,
        MappingDiagnosticKind::Approximation
    );
    let damf = DamfReader::open_with_options(target.path.join("master.atmos"), options).unwrap();
    assert_eq!(damf.spec().objects[0].initial.screen_factor, 0.);
    let updates: Vec<_> = damf
        .updates()
        .unwrap()
        .map(|u| {
            let u = u.unwrap();
            (u.sample, u.patch.screen_factor, u.ramp_samples)
        })
        .collect();
    assert_eq!(updates, [(1200, Some(1.), None), (2400, Some(0.), None)]);
    let back = Target {
        path: dir.path("back.wav"),
        profile: Profile::Standard,
    };
    damf.convert(damf.audio().unwrap(), std::slice::from_ref(&back))
        .unwrap();
    let values: Vec<_> = snapshot(&back.path)
        .object_blocks
        .iter()
        .map(|b| (b.start, b.screen_ref))
        .collect();
    assert_eq!(values, [(0, 0), (1200, 1), (2400, 0)]);
}
