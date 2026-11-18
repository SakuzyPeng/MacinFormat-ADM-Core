use super::*;

fn block(channel: usize, start: u64, duration: Option<u64>) -> AdmBedBlock {
    AdmBedBlock {
        channel,
        labels: vec![format!("RC_{}", BED_LABELS[channel])],
        timing: AdmTiming {
            start: Some(start),
            duration,
        },
        geometry: AdmMetadata::default(),
        gain: None,
        importance: None,
        head_tracking: HeadTracking::SceneRelative,
        headphone_virtualise: None,
    }
}

fn gain_update(sample: u64, gain: f64) -> BedUpdate {
    BedUpdate {
        channel: 0,
        sample,
        ramp_samples: Some(0),
        patch: BedPatch {
            gain: Some(Gain::Linear(gain)),
            ..Default::default()
        },
    }
}

#[test]
fn invalid_bed_clock_leaves_scheduled_metadata_and_pcm_retryable() {
    let dir = Temp::new();
    let path = dir.path("retry.wav");
    let mut writer = MasterWriter::create(&path, spec()).unwrap();
    writer.write_block(PcmBlock::S24Le(&[3; 33]), &[]).unwrap();
    let too_late = 101 * 3600 * 48_000;
    let error = writer
        .schedule_bed_updates(&[gain_update(2, 0.5), gain_update(too_late, 1.)])
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidInput);
    writer
        .schedule_bed_updates(&[gain_update(1, 0.25)])
        .unwrap();
    writer
        .write_block(PcmBlock::S24Le(&[4; 33 * 2]), &[])
        .unwrap();
    assert_eq!(writer.finish().unwrap().frames, 3);
    let xml = String::from_utf8(chunk(&path, *b"axml")).unwrap();
    assert!(xml.contains("<gain>0.25</gain>"));
    assert!(!xml.contains("<gain>0.5</gain>"));
}

#[test]
fn unbounded_bed_overlaps_the_following_block_with_either_object_duration() {
    let dir = Temp::new();
    for duration in [None, Some(100)] {
        for policy in [LossPolicy::Reject, LossPolicy::Allow] {
            let mut s = spec();
            s.profile = Profile::DamfHome;
            s.loss_policy = policy;
            s.bed_timing = Some(AdmTiming {
                start: None,
                duration,
            });
            s.bed_blocks[0] = Some(block(0, 0, None));
            let mut writer =
                MasterWriter::create(dir.path(&format!("{duration:?}-{policy:?}")), s).unwrap();
            let result = writer.schedule_adm_bed_blocks(&[block(0, 25, Some(75))]);
            if policy == LossPolicy::Reject {
                let error = result.unwrap_err();
                assert_eq!((error.field, error.sample), (Some("bed_timing"), Some(25)));
            } else {
                result.unwrap();
                writer
                    .write_block(PcmBlock::S24Le(&[1; 33 * 100]), &[])
                    .unwrap();
                let diagnostics = writer.finish().unwrap().diagnostics;
                assert!(
                    diagnostics
                        .iter()
                        .any(|d| d.field == "bed_timing" && d.sample == Some(25))
                );
            }
        }
    }
}

#[cfg(feature = "read")]
mod reading {
    use super::*;
    use macin_atmos::{adm::AdmReader, damf::DamfReader};

    #[test]
    fn bed_origin_gain_and_end_survive_multi_target_retry() {
        let dir = Temp::new();
        let mut s = spec();
        s.bed_timing = Some(AdmTiming {
            start: Some(12),
            duration: Some(13),
        });
        let targets = [
            Target {
                path: dir.path("std.wav"),
                profile: Profile::Standard,
            },
            Target {
                path: dir.path("logic.wav"),
                profile: Profile::Logic,
            },
            Target {
                path: dir.path("home"),
                profile: Profile::DamfHome,
            },
        ];
        let mut writer = MultiWriter::create(&s, &targets).unwrap();
        for outside in [25, 50] {
            let result =
                writer.schedule_bed_updates(&[gain_update(15, 0.5), gain_update(outside, 1.)]);
            assert_eq!(result.unwrap_err().kind, ErrorKind::InvalidInput);
        }
        writer
            .schedule_bed_updates(&[gain_update(12, 0.25)])
            .unwrap();
        writer
            .write_block(PcmBlock::S24Le(&[7; 33 * 100]), &[])
            .unwrap();
        writer.finish().unwrap();
        for t in &targets[..2] {
            let r = AdmReader::open(&t.path, LossPolicy::Reject).unwrap();
            assert_eq!(r.spec().bed[0].gain, Gain::Linear(0.25));
            assert_eq!(r.spec().bed_timing, s.bed_timing);
        }
        let r = DamfReader::open(targets[2].path.join("master.atmos"), LossPolicy::Reject).unwrap();
        let updates: Vec<_> = r
            .bed_updates()
            .unwrap()
            .map(Result::unwrap)
            .filter(|b| b.channel == 0)
            .collect();
        assert!(updates.iter().any(|u| {
            u.sample == 12
                && u.patch
                    .gain
                    .is_some_and(|g| (g.linear() - 0.25).abs() < 1e-8)
        }));
        assert!(
            updates
                .iter()
                .any(|u| u.sample == 25 && u.patch.active == Some(false))
        );
    }

    #[test]
    fn logic_96k_preserves_adjacent_bed_sample_boundaries() {
        let dir = Temp::new();
        let mut s = spec();
        s.sample_rate = 96_000;
        s.bed_blocks[0] = Some(block(0, 0, Some(4000)));
        let source = dir.path("standard.wav");
        let mut w = MasterWriter::create(&source, s).unwrap();
        let subsequent = [block(0, 4000, Some(4000)), block(0, 8000, Some(4000))];
        w.schedule_adm_bed_blocks(&subsequent).unwrap();
        w.write_block(PcmBlock::S24Le(&vec![7; 33 * 12_000]), &[])
            .unwrap();
        w.finish().unwrap();
        let logic = Target {
            path: dir.path("logic.wav"),
            profile: Profile::Logic,
        };
        let r = AdmReader::open(&source, LossPolicy::Reject).unwrap();
        r.convert(r.audio().unwrap(), std::slice::from_ref(&logic))
            .unwrap();
        let r = AdmReader::open(&logic.path, LossPolicy::Reject).unwrap();
        assert_eq!(
            r.bed_blocks().collect::<Result<Vec<_>>>().unwrap(),
            subsequent
        );
        let home = Target {
            path: dir.path("home"),
            profile: Profile::DamfHome,
        };
        r.convert(r.audio().unwrap(), std::slice::from_ref(&home))
            .unwrap();
        let r = DamfReader::open(home.path.join("master.atmos"), LossPolicy::Reject).unwrap();
        let events: Vec<_> = r
            .bed_updates()
            .unwrap()
            .map(Result::unwrap)
            .filter(|b| b.channel == 0)
            .map(|b| (b.sample, b.patch.active))
            .collect();
        assert_eq!(events, [(4000, Some(true)), (8000, Some(true))]);
    }

    #[test]
    fn programme_end_clips_bed_objects_and_blocks_before_scheduling() {
        let dir = Temp::new();
        let source = dir.path("full.wav");
        let mut s = spec();
        s.bed_blocks[0] = Some(block(0, 24, Some(48)));
        s.bed_blocks[1] = Some(block(1, 72, Some(24)));
        let mut w = MasterWriter::create(&source, s).unwrap();
        w.schedule_adm_bed_blocks(&[block(0, 72, Some(24))])
            .unwrap();
        w.write_block(PcmBlock::S24Le(&[3; 96 * 33]), &[]).unwrap();
        w.finish().unwrap();
        let xml = String::from_utf8(chunk(&source, *b"axml")).unwrap();
        let shortened = xml.replace(r#"end="00:00:00.002000000""#, r#"end="00:00:00.001000000""#);
        assert_ne!(shortened, xml);
        let input = dir.path("programme.wav");
        macin_bwf::replace_chunk(
            fs::File::open(source).unwrap(),
            fs::File::create(&input).unwrap(),
            *b"axml",
            shortened.as_bytes(),
        )
        .unwrap();
        let r = AdmReader::open(&input, LossPolicy::Reject).unwrap();
        let target = Target {
            path: dir.path("clipped.wav"),
            profile: Profile::Standard,
        };
        r.convert(r.audio().unwrap(), std::slice::from_ref(&target))
            .unwrap();
        let copy = AdmReader::open(target.path, LossPolicy::Reject).unwrap();
        assert_eq!(copy.audio().unwrap().frames(), 48);
        assert_eq!(copy.spec().bed_timing.unwrap().duration, Some(48));
        assert_eq!(
            copy.spec().bed_blocks[0].as_ref().unwrap(),
            &block(0, 24, Some(24))
        );
        let silent = copy.spec().bed_blocks[1].as_ref().unwrap();
        assert_eq!(silent.timing.duration, Some(0));
        assert_eq!(silent.gain.unwrap().linear(), 0.);
        assert_eq!(copy.bed_update_count(), 0);
        let home = Target {
            path: dir.path("long-home"),
            profile: Profile::DamfHome,
        };
        r.convert(r.wave_audio(&input).unwrap(), std::slice::from_ref(&home))
            .unwrap();
        let output = DamfReader::open(home.path.join("master.atmos"), LossPolicy::Reject).unwrap();
        assert_eq!(output.audio().unwrap().frames(), 96);
        assert!(!output.spec().bed[1].active);
        let updates: Vec<_> = output
            .bed_updates()
            .unwrap()
            .map(Result::unwrap)
            .filter(|u| u.channel == 0)
            .map(|u| (u.sample, u.patch.active))
            .collect();
        assert_eq!(updates, [(24, Some(true)), (48, Some(false))]);
    }
}
