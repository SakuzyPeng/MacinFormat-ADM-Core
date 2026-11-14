use macin_atmos::*;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "macin-atmos-multi-{}-{}",
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

const PROFILES: [Profile; 4] = [
    Profile::Standard,
    Profile::Logic,
    Profile::DamfHome,
    Profile::DamfThreeDof,
];

fn spec() -> MasterSpec {
    let mut spec = MasterSpec {
        sample_rate: 48_000,
        objects: vec![
            ObjectSpec::new(TrackId(5), "Moving"),
            ObjectSpec::new(TrackId(9), "Still"),
        ],
        ..Default::default()
    };
    spec.objects[0].initial.position = [-0.5, 0.25, 0.];
    spec
}
fn pcm(first: usize, frames: usize) -> Vec<f32> {
    (first * 12..(first + frames) * 12)
        .map(|i| ((i * 7919) % 2001) as f32 / 1000. - 1.)
        .collect()
}
fn moves(sample: u64) -> ObjectUpdate {
    ObjectUpdate {
        track: TrackId(5),
        sample,
        ramp_samples: Some(600),
        patch: ObjectPatch {
            position: Some([0.5, 0.25, 0.5]),
            ..Default::default()
        },
    }
}
fn headphone(sample: u64) -> ObjectUpdate {
    ObjectUpdate {
        track: TrackId(9),
        sample,
        ramp_samples: None,
        patch: ObjectPatch {
            binaural_mode: Some(BinauralMode::Near),
            ..Default::default()
        },
    }
}

/// Every file of an output, keyed by name, so a DAMF directory compares as a whole.
fn contents(path: &Path) -> Vec<(String, Vec<u8>)> {
    if path.is_file() {
        return vec![(String::new(), fs::read(path).unwrap())];
    }
    let mut files: Vec<_> = fs::read_dir(path)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().into_string().unwrap(),
                fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    files.sort();
    files
}

fn single(path: &Path, profile: Profile, policy: LossPolicy, headphones: bool) -> WriteReport {
    let mut writer = MasterWriter::create(
        path,
        MasterSpec {
            profile,
            loss_policy: policy,
            ..spec()
        },
    )
    .unwrap();
    writer
        .write_block(PcmBlock::F32Interleaved(&pcm(0, 1000)), &[moves(100)])
        .unwrap();
    let mut late = vec![moves(1500)];
    if headphones {
        late.insert(0, headphone(1200));
    }
    writer.schedule_updates(&late).unwrap();
    writer
        .write_block(PcmBlock::F32Interleaved(&pcm(1000, 1400)), &[])
        .unwrap();
    writer.finish().unwrap()
}

#[test]
fn one_pass_matches_four_single_writers_byte_for_byte() {
    let dir = Temp::new();
    let targets: Vec<_> = PROFILES
        .iter()
        .enumerate()
        .map(|(i, &profile)| Target {
            path: dir.path(&format!("multi-{i}")),
            profile,
        })
        .collect();
    let spec = MasterSpec {
        loss_policy: LossPolicy::Allow,
        ..spec()
    };
    let mut multi = MultiWriter::create(&spec, &targets).unwrap();
    multi
        .write_block(PcmBlock::F32Interleaved(&pcm(0, 1000)), &[moves(100)])
        .unwrap();
    multi
        .schedule_updates(&[headphone(1200), moves(1500)])
        .unwrap();
    multi
        .write_block(PcmBlock::F32Interleaved(&pcm(1000, 1400)), &[])
        .unwrap();
    assert_eq!(multi.frames_written(), 2400);
    let reports = multi.finish().unwrap();
    assert_eq!(reports.len(), 4);
    for (i, (target, report)) in targets.iter().zip(&reports).enumerate() {
        let alone = dir.path(&format!("single-{i}"));
        let expected = single(&alone, target.profile, LossPolicy::Allow, true);
        assert_eq!(
            contents(&target.path),
            contents(&alone),
            "{:?}",
            target.profile
        );
        assert_eq!(
            report.diagnostics, expected.diagnostics,
            "{:?}",
            target.profile
        );
        assert_eq!(report.frames, 2400);
    }
    // Headphone policy is DAMF only, so exactly the ADM targets report it as lost.
    let lost: Vec<_> = reports.iter().map(|r| !r.diagnostics.is_empty()).collect();
    assert_eq!(lost, [true, true, false, false]);
}

#[test]
fn a_rejection_on_one_target_leaves_every_target_unchanged() {
    let dir = Temp::new();
    let targets = [
        Target {
            path: dir.path("adm.wav"),
            profile: Profile::Standard,
        },
        Target {
            path: dir.path("damf"),
            profile: Profile::DamfHome,
        },
    ];
    let mut multi = MultiWriter::create(&spec(), &targets).unwrap();
    multi
        .write_block(PcmBlock::F32Interleaved(&pcm(0, 1000)), &[moves(100)])
        .unwrap();
    // Near has no ADM mapping, so under Reject the ADM target refuses it and DAMF must not keep it.
    let error = multi
        .schedule_updates(&[headphone(1200), moves(1500)])
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unsupported);
    assert_eq!(error.field, Some("binaural_mode"));
    let error = multi
        .write_block(
            PcmBlock::F32Interleaved(&pcm(1000, 1400)),
            &[headphone(1200)],
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unsupported);
    assert_eq!(
        multi.frames_written(),
        1000,
        "a rejected block writes no audio"
    );
    multi.schedule_updates(&[moves(1500)]).unwrap();
    multi
        .write_block(PcmBlock::F32Interleaved(&pcm(1000, 1400)), &[])
        .unwrap();
    multi.finish().unwrap();
    for target in &targets {
        let alone = dir.path(&format!("alone-{:?}", target.profile));
        single(&alone, target.profile, LossPolicy::Reject, false);
        assert_eq!(
            contents(&target.path),
            contents(&alone),
            "{:?}",
            target.profile
        );
    }
}

#[test]
fn a_failed_publish_rolls_back_the_outputs_already_published() {
    let dir = Temp::new();
    let targets = [
        Target {
            path: dir.path("first.wav"),
            profile: Profile::Logic,
        },
        Target {
            path: dir.path("second"),
            profile: Profile::DamfThreeDof,
        },
        Target {
            path: dir.path("third.wav"),
            profile: Profile::Standard,
        },
    ];
    let mut multi = MultiWriter::create(&spec(), &targets).unwrap();
    multi
        .write_block(PcmBlock::F32Interleaved(&pcm(0, 480)), &[])
        .unwrap();
    fs::write(dir.path("third.wav"), b"someone else's file").unwrap();
    let error = multi.finish().unwrap_err();
    assert_eq!(error.kind, ErrorKind::OutputExists, "{error}");
    assert!(error.message.contains("third.wav"), "{error}");
    let mut left: Vec<_> = fs::read_dir(&dir.0)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(
        left,
        ["third.wav"],
        "published outputs and staging are gone"
    );
    assert_eq!(
        fs::read(dir.path("third.wav")).unwrap(),
        b"someone else's file"
    );
}

#[test]
fn targets_are_checked_before_anything_is_created() {
    let dir = Temp::new();
    assert!(MultiWriter::create(&spec(), &[]).is_err());
    let same = Target {
        path: dir.path("same.wav"),
        profile: Profile::Standard,
    };
    let error = MultiWriter::create(&spec(), &[same.clone(), same]).unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidInput);
    fs::write(dir.path("taken"), b"").unwrap();
    let error = MultiWriter::create(
        &spec(),
        &[
            Target {
                path: dir.path("free.wav"),
                profile: Profile::Standard,
            },
            Target {
                path: dir.path("taken"),
                profile: Profile::DamfHome,
            },
        ],
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::OutputExists);
    assert_eq!(
        fs::read_dir(&dir.0).unwrap().count(),
        1,
        "no staging is left behind"
    );
}
