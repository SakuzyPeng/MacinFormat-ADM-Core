use super::*;

#[test]
fn dbmd_ebu_segments_keep_programmes_checksum_padding_and_wire_lengths() {
    // EBU 的零长度码代表 65535 字节，校验初值取线上的长度低字节。
    let mut db = vec![7, 0, 0, 1];
    let mut ddplus = vec![0; 96];
    ddplus[2] = 32;
    ddplus[5] = 0x7a;
    ddplus[14..16].copy_from_slice(&[3, 5]);
    let mut second = ddplus.clone();
    second[0] = 1;
    second[14..16].copy_from_slice(&[1, 4]);
    for (tag, data) in [
        (7, ddplus.clone()),
        (1, vec![0; 118]),
        (3, vec![0; 69]),
        (3, {
            let mut v = vec![0; 69];
            v[0] = 1;
            v
        }),
        (8, vec![0; 70]),
        (7, second.clone()),
    ] {
        dbmd::append_dbmd_segment(&mut db, tag, &data).unwrap();
    }
    db.extend_from_slice(&[42, 0, 0]);
    let sentinel = db.len();
    db.resize(sentinel + 65535, 0);
    db[sentinel] = 9;
    db.extend_from_slice(&[0u8.wrapping_sub(9), 0]);
    if db.len() % 2 == 1 {
        db.push(0xa5);
    }
    let parsed = dbmd::read(&db, 10).unwrap();
    assert_eq!(parsed.source.version(), 0x01000007);
    assert_eq!(
        parsed.source.segments().map(|p| p.0).collect::<Vec<_>>(),
        [7, 1, 3, 3, 8, 7, 42]
    );
    let dd = parsed.ddplus_compression;
    assert_eq!((dd.compr1, dd.dynrng1), (3, 5));
    let mut spec = MasterSpec {
        profile: Profile::Logic,
        dbmd_source: Some(parsed.source),
        ddplus_compression: dd,
        warp_mode: parsed.warp_mode,
        downmix_mode: parsed.downmix_mode,
        surround_phase_shift_present: false,
        bed_distribution_present: false,
        ..Default::default()
    };
    let channels: Vec<_> = spec.bed.iter().copied().map(Into::into).collect();
    assert_eq!(dbmd::build_from_spec(&spec, &channels).unwrap(), db);
    spec.ddplus_compression.compr1 = 4;
    let edited = dbmd::read(&dbmd::build_from_spec(&spec, &channels).unwrap(), 10).unwrap();
    let dd: Vec<_> = edited
        .source
        .segments()
        .filter_map(|(i, v)| (i == 7).then_some(v))
        .collect();
    ddplus[14] = 4;
    assert_eq!(dd, [&ddplus[..], &second[..]]);
    let mut corrupt = db;
    corrupt[sentinel + 65535] ^= 1;
    assert_eq!(
        dbmd::read(&corrupt, 10).unwrap_err().kind,
        ErrorKind::InvalidInput
    );
}

#[test]
fn dbmd_trim_balance_uses_sign_and_magnitude_on_all_nine_layouts() {
    let mut spec = spec(Profile::Logic);
    let channels = vec![StaticChannelMetadata::default(); spec.channels()];
    for (value, byte) in [
        (-1., 127),
        (-0.75, 95),
        (-0.5, 64),
        (-0.25, 32),
        (-1. / 127., 1),
        (0., 128),
        (0.5, 192),
        (1., 255),
    ] {
        spec.trim_mode.fill(Some(TrimValues {
            center: -3.,
            front_back_overhead_floor: value,
            front_back_listener: value,
            ..Default::default()
        }));
        let bytes = dbmd::build_from_spec(&spec, &channels).unwrap();
        let raw = payload(&bytes, 10);
        for i in 0..9 {
            let at = 7 + i * 15;
            assert_eq!(raw[at], 0);
            assert_eq!(&raw[at + 13..at + 15], &[byte, byte]);
        }
        let read = dbmd::read(&bytes, channels.len()).unwrap();
        assert!(read.unsupported.is_empty());
        let expected = (value * 127.).round() / 127.;
        for trim in read.trim_mode.unwrap() {
            let trim = trim.unwrap();
            assert_eq!(
                (
                    trim.center,
                    trim.front_back_listener,
                    trim.front_back_overhead_floor
                ),
                (-3., expected, expected)
            );
        }
    }
}

#[cfg(feature = "read")]
#[test]
fn old_logic_header_ffoa_and_not_indicated_settings_round_trip_through_damf() {
    use macin_atmos::{adm::AdmReader, damf::DamfReader};
    let dir = Temp::new();
    let mut s = spec(Profile::Logic);
    s.warp_mode = WarpMode::NotIndicated;
    s.downmix_mode = DownmixMode::DirectRender;
    s.set_surround_phase_shift(None);
    s.first_action_ns = Some(50_000_000);
    s.objects[0].channel_names.stream_pack_ref = true;
    s.trim_mode.fill(Some(TrimValues {
        front_back_listener: -1.,
        ..Default::default()
    }));
    let source = dir.path("original.wav");
    write(&source, s, &[], &[]);
    let bytes = chunk(&source, *b"dbmd");
    let mut db = vec![7, 0, 0, 1];
    for id in [7, 9, 10] {
        let mut value = payload(&bytes, id).to_vec();
        if id == 9 {
            value[111] = 0x13;
            assert_eq!(value[139], 0x43);
            assert_eq!(value[152] & 7, 4);
        }
        dbmd::append_dbmd_segment(&mut db, id, &value).unwrap();
    }
    db.extend_from_slice(&[0, 0xa5]);
    let older = dir.path("older.wav");
    macin_bwf::replace_chunk(
        fs::File::open(&source).unwrap(),
        fs::File::create(&older).unwrap(),
        *b"dbmd",
        &db,
    )
    .unwrap();
    let reader = AdmReader::open(&older, LossPolicy::Reject).unwrap();
    assert_eq!(reader.spec().frame_rate, FrameRate::Fps25);
    assert_eq!(reader.spec().first_action_ns, Some(50_000_000));
    let native = Target {
        profile: Profile::Logic,
        path: dir.path("rewrite.wav"),
    };
    let home = Target {
        profile: Profile::DamfHome,
        path: dir.path("home"),
    };
    let report = reader
        .convert(reader.audio().unwrap(), &[native.clone(), home.clone()])
        .unwrap();
    assert!(report.read.is_empty() && report.outputs.iter().all(|r| r.diagnostics.is_empty()));
    assert_eq!(chunk(&native.path, *b"dbmd"), db);
    let stream = String::from_utf8(chunk(&native.path, *b"axml")).unwrap();
    let section = stream
        .split("<audioStreamFormat ")
        .last()
        .unwrap()
        .split("</audioStreamFormat>")
        .next()
        .unwrap();
    assert!(section.contains("<audioPackFormatIDRef>AP_00031001</audioPackFormatIDRef>"));
    let manifest = home.path.join("master.atmos");
    let text = fs::read_to_string(&manifest).unwrap();
    assert!(text.contains("ffoa: 0.05") && text.contains("downmixType_5to2: directRender"));
    assert!(!text.contains("warpMode:") && !text.contains("51-to-20_LsRs90degPhaseShift:"));
    let damf = DamfReader::open(manifest, LossPolicy::Reject).unwrap();
    assert_eq!(damf.spec().first_action_ns, Some(50_000_000));
    assert!(
        damf.spec()
            .trim_mode
            .iter()
            .all(|v| v.unwrap().front_back_listener == -1.)
    );
    let back = Target {
        profile: Profile::Logic,
        path: dir.path("from-damf.wav"),
    };
    damf.convert(damf.audio().unwrap(), std::slice::from_ref(&back))
        .unwrap();
    let bytes = chunk(&back.path, *b"dbmd");
    let s9 = payload(&bytes, 9);
    assert_eq!((s9[111], s9[139], s9[152] & 7), (0x23, 0x43, 4));
    assert_eq!(chunk(&back.path, *b"data"), chunk(&source, *b"data"));
}

#[cfg(feature = "read")]
#[test]
fn dbmd_not_indicated_trim_bypass_survives_native_updates_and_reports_portable_loss() {
    let dir = Temp::new();
    let mut s = spec(Profile::Logic);
    s.objects[0].initial.trim_bypass_unspecified = true;
    let path = dir.path("source.wav");
    write(&path, s, &[], &[]);
    assert_eq!(payload(&chunk(&path, *b"dbmd"), 10)[142 + 10], 2);
    let reader = adm::AdmReader::open(&path, LossPolicy::Reject).unwrap();
    assert!(reader.spec().objects[0].initial.trim_bypass_unspecified);
    let yes = ObjectPatch {
        trim_bypass: Some(true),
        ..Default::default()
    }
    .apply(reader.spec().objects[0].initial);
    assert!(yes.trim_bypass && !yes.trim_bypass_unspecified);
    let home = Target {
        profile: Profile::DamfHome,
        path: dir.path("portable"),
    };
    assert_eq!(
        reader
            .convert(reader.audio().unwrap(), std::slice::from_ref(&home))
            .unwrap_err()
            .field,
        Some("trim_bypass")
    );
    let allowed = adm::AdmReader::open(path, LossPolicy::Allow).unwrap();
    let output = allowed
        .convert(allowed.audio().unwrap(), std::slice::from_ref(&home))
        .unwrap();
    assert!(
        output.outputs[0]
            .diagnostics
            .iter()
            .any(|d| d.field == "trim_bypass")
    );
    assert!(
        !damf::DamfReader::open(home.path.join("master.atmos"), LossPolicy::Reject)
            .unwrap()
            .spec()
            .objects[0]
            .initial
            .trim_bypass
    );
}

#[cfg(feature = "read")]
#[test]
fn damf_dbmd_sidecar_round_trips_text_and_maps_ddplus_compression() {
    let dir = Temp::new();
    let source = dir.path("source");
    write(&source, spec(Profile::DamfHome), &[], &[]);
    let manifest = source.join("master.atmos");
    let path = source.join("master.atmos.dbmd");
    let text = "# 保留原有注释\nddplus:\n  compr1: music_standard\n  dynrng1: speech\n";
    fs::write(&path, text).unwrap();
    let reader = damf::DamfReader::open(&manifest, LossPolicy::Reject).unwrap();
    let native = Target {
        profile: Profile::Logic,
        path: dir.path("logic.wav"),
    };
    let home = Target {
        profile: Profile::DamfHome,
        path: dir.path("copy"),
    };
    reader
        .convert(reader.audio().unwrap(), &[native.clone(), home.clone()])
        .unwrap();
    assert_eq!(
        fs::read_to_string(home.path.join("master.atmos.dbmd")).unwrap(),
        text
    );
    assert_eq!(&payload(&chunk(&native.path, *b"dbmd"), 7)[14..16], &[3, 5]);
    let wave = adm::AdmReader::open(&native.path, LossPolicy::Reject).unwrap();
    let from_wave = Target {
        profile: Profile::DamfHome,
        path: dir.path("decoded"),
    };
    wave.convert(wave.audio().unwrap(), std::slice::from_ref(&from_wave))
        .unwrap();
    let back =
        damf::DamfReader::open(from_wave.path.join("master.atmos"), LossPolicy::Reject).unwrap();
    assert_eq!(
        back.spec().ddplus_compression,
        reader.spec().ddplus_compression
    );
    let extended = format!("{text}  dialnorm: -26\n");
    fs::write(&path, &extended).unwrap();
    let reader = damf::DamfReader::open(&manifest, LossPolicy::Reject).unwrap();
    let copy = Target {
        profile: Profile::DamfHome,
        path: dir.path("opaque"),
    };
    reader
        .convert(reader.audio().unwrap(), std::slice::from_ref(&copy))
        .unwrap();
    assert_eq!(
        fs::read_to_string(copy.path.join("master.atmos.dbmd")).unwrap(),
        extended
    );
    let unsupported = Target {
        profile: Profile::Logic,
        path: dir.path("partial.wav"),
    };
    assert_eq!(
        reader
            .convert(reader.audio().unwrap(), std::slice::from_ref(&unsupported))
            .unwrap_err()
            .field,
        Some("dbmd_sidecar")
    );
    let reader = damf::DamfReader::open(manifest, LossPolicy::Allow).unwrap();
    let r = reader
        .convert(reader.audio().unwrap(), std::slice::from_ref(&unsupported))
        .unwrap();
    assert!(
        r.outputs[0]
            .diagnostics
            .iter()
            .any(|d| d.field == "dbmd_sidecar")
    );
    assert_eq!(
        &payload(&chunk(&unsupported.path, *b"dbmd"), 7)[14..16],
        &[3, 5]
    );
}

#[test]
fn dbmd_first_action_distinguishes_zero_unset_and_unrepresentable_times() {
    let dir = Temp::new();
    let mut s = spec(Profile::Logic);
    let channels: Vec<_> = s
        .bed
        .iter()
        .copied()
        .map(Into::into)
        .chain(s.objects.iter().map(|o| o.initial.into()))
        .collect();
    for (ns, raw) in [
        (None, [0xff, 0, 0, 0, 0, 0]),
        (Some(0), [0, 0, 0, 0, 0, 0]),
        (Some(59_999_995_000), [0, 1, 0, 0, 0, 0]),
        (Some(3_600_500_000_000), [1, 0, 0x50, 0xc3, 0, 0]),
    ] {
        s.first_action_ns = ns;
        let data = dbmd::build_from_spec(&s, &channels).unwrap();
        assert_eq!(&payload(&data, 9)[112..118], &raw);
        let read = dbmd::read(&data, channels.len()).unwrap();
        assert_eq!(
            read.first_action_ns,
            ns.map(|v| ((v + 5_000) / 10_000) * 10_000)
        );
    }
    for (i, ns) in [-500_000_000, 128 * 3600 * 1_000_000_000]
        .into_iter()
        .enumerate()
    {
        s.first_action_ns = Some(ns);
        s.loss_policy = LossPolicy::Reject;
        let path = dir.path(&format!("clock-{i}.wav"));
        assert_eq!(
            MasterWriter::create(&path, s.clone()).unwrap_err().field,
            Some("ffoa")
        );
        s.loss_policy = LossPolicy::Allow;
        let result = write(&path, s.clone(), &[], &[]);
        assert!(result.diagnostics.iter().any(|d| d.field == "ffoa"));
        assert_eq!(payload(&chunk(&path, *b"dbmd"), 9)[112], 0xff);
    }
}

#[cfg(feature = "read")]
#[test]
fn dbmd_reserved_source_data_is_tied_to_the_original_pcm_and_layout() {
    let tmp = Temp::new();
    let wave = tmp.path("base.wav");
    write(&wave, spec(Profile::Logic), &[], &[]);
    let baseline = chunk(&wave, *b"dbmd");
    let mut db = dbmd::DBMD_VERSION.to_vec();
    for id in [7, 9, 10] {
        let mut p = payload(&baseline, id).to_vec();
        if id == 7 {
            p[2] = 0x35;
            p[14] = 5;
        }
        dbmd::append_dbmd_segment(&mut db, id, &p).unwrap();
    }
    db.push(0);
    let input = tmp.path("input.wav");
    macin_bwf::replace_chunk(
        fs::File::open(&wave).unwrap(),
        fs::File::create(&input).unwrap(),
        *b"dbmd",
        &db,
    )
    .unwrap();
    let target = Target {
        profile: Profile::Logic,
        path: tmp.path("changed.wav"),
    };
    let strict = adm::AdmReader::open(&input, LossPolicy::Reject).unwrap();
    assert_eq!(
        strict
            .convert(
                strict.wave_audio(&wave).unwrap(),
                std::slice::from_ref(&target)
            )
            .unwrap_err()
            .field,
        Some("dbmd_encoding")
    );
    assert!(!target.path.exists());
    let allowed = adm::AdmReader::open(input, LossPolicy::Allow).unwrap();
    let report = allowed
        .convert(
            allowed.wave_audio(wave).unwrap(),
            std::slice::from_ref(&target),
        )
        .unwrap();
    assert!(report.read.iter().any(|d| d.field == "dbmd_encoding"));
    let out = chunk(&target.path, *b"dbmd");
    assert_eq!((payload(&out, 7)[2], payload(&out, 7)[14]), (0, 5));
}
