use macin_atmos::*;
#[path = "native/bed_timing.rs"]
mod bed_timing;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "macin-native-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn path(&self, p: &str) -> PathBuf {
        self.0.join(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn spec() -> MasterSpec {
    MasterSpec {
        objects: vec![ObjectSpec::new(TrackId(1), "Object")],
        ..Default::default()
    }
}
fn write(path: &Path, spec: MasterSpec, updates: &[ObjectUpdate]) -> WriteReport {
    let n = spec.channels();
    let mut writer = MasterWriter::create(path, spec).unwrap();
    writer.schedule_updates(updates).unwrap();
    writer
        .write_block(PcmBlock::S24Le(&vec![17; n * 3 * 4800]), &[])
        .unwrap();
    writer.finish().unwrap()
}
fn chunk(path: &Path, id: [u8; 4]) -> Vec<u8> {
    macin_bwf::ChunkReader::new(fs::File::open(path).unwrap())
        .unwrap()
        .metadata(id, 1 << 24)
        .unwrap()
        .unwrap()
}
fn element(name: &str, attrs: &[(&str, &str)], text: &str) -> AdmElement {
    AdmElement {
        name: name.into(),
        attributes: attrs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        text: text.into(),
        children: vec![],
    }
}

#[test]
fn batch_axml_preserves_the_same_zones_and_literal_ranges_as_the_streaming_writer() {
    let dir = Temp::new();
    let mut s = spec();
    s.preserve_literal.cartesian_azimuth_range = true;
    s.objects[0].custom_zones = vec![ExclusionZone {
        name: "left region".into(),
        bounds: ZoneBounds::Cartesian([-1., 0., -1., 1., -1., 1.]),
    }];
    s.objects[0].initial.divergence = Some(ObjectDivergence {
        value: 0.5,
        azimuth_range: Some(45.),
        position_range: None,
    });
    let blocks = [vec![metadata::AdmBlock {
        sample: 0,
        duration: 4800,
        ramp_samples: 0,
        state: s.objects[0].initial,
    }]];
    let batch = metadata::build_axml(&s, 4800, &blocks).unwrap();
    assert!(batch.contains("left region") && batch.contains(r#"azimuthRange="45""#));
    let path = dir.path("stream.wav");
    write(&path, s, &[]);
    assert_eq!(batch.as_bytes(), chunk(&path, *b"axml"));
}
fn metadata(attrs: &[(&str, &str)], elements: Vec<AdmElement>) -> AdmMetadata {
    AdmMetadata {
        attributes: attrs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        elements,
    }
}
fn rich_spec() -> MasterSpec {
    let mut s = spec();
    let loudness = AdmElement {
        children: vec![element("integratedLoudness", &[], "-23.123456789012345")],
        ..element(
            "loudnessMetadata",
            &[
                ("loudnessMethod", "ITU-R BS.1770"),
                ("loudnessRecType", "EBU R128"),
            ],
            "",
        )
    };
    let screen = AdmElement {
        children: vec![
            element(
                "screenCentrePosition",
                &[("X", "0"), ("Y", "1"), ("Z", "0.2")],
                "",
            ),
            element("screenWidth", &[("X", "0.3")], ""),
        ],
        ..element(
            "audioProgrammeReferenceScreen",
            &[("aspectRatio", "1.78")],
            "",
        )
    };
    s.adm = metadata(
        &[("audioProgrammeLanguage", "zh"), ("maxDuckingDepth", "-12")],
        vec![
            element(
                "audioProgrammeLabel",
                &[("language", "zh")],
                " 节目 & <标题> ",
            ),
            loudness.clone(),
            screen,
        ],
    );
    s.contents = vec![ContentSpec {
        name: "Voice".into(),
        include_bed: true,
        objects: vec![TrackId(1)],
        dialogue: None,
        adm: metadata(
            &[("audioContentLanguage", "en")],
            vec![
                element("audioContentLabel", &[("language", "en")], "Content"),
                loudness,
            ],
        ),
    }];
    s.objects[0].adm = metadata(
        &[
            ("importance", "3"),
            ("dialogue", "1"),
            ("interact", "1"),
            ("disableDucking", "1"),
        ],
        vec![
            element("audioObjectLabel", &[("language", "en")], "Object label"),
            element(
                "positionOffset",
                &[("coordinate", "X")],
                "0.123456789012345",
            ),
            AdmElement {
                children: vec![
                    element(
                        "gainInteractionRange",
                        &[("bound", "min"), ("gainUnit", "dB")],
                        "-12",
                    ),
                    element(
                        "gainInteractionRange",
                        &[("bound", "max"), ("gainUnit", "dB")],
                        "6",
                    ),
                ],
                ..element(
                    "audioObjectInteraction",
                    &[("onOffInteract", "1"), ("gainInteract", "1")],
                    "",
                )
            },
        ],
    );
    s.objects[0].pack_adm = metadata(&[("importance", "6")], vec![]);
    s.objects[0].channel_adm = metadata(
        &[],
        vec![element(
            "frequency",
            &[("typeDefinition", "highPass")],
            "80",
        )],
    );
    s.bed_adm = metadata(
        &[("importance", "7")],
        vec![element("positionOffset", &[("coordinate", "X")], "0.1")],
    );
    s.bed_pack_adm = metadata(&[("importance", "8")], vec![]);
    s.bed_channel_adm[3] = metadata(
        &[],
        vec![element(
            "frequency",
            &[("typeDefinition", "lowPass")],
            "120",
        )],
    );
    s.objects[0].initial = ObjectState {
        importance: 0.9,
        diffuse: 0.25,
        divergence: Some(ObjectDivergence {
            value: 0.4,
            azimuth_range: None,
            position_range: Some(0.2),
        }),
        snap: true,
        channel_lock_distance: Some(0.3),
        screen_edge_lock: [Some(ScreenEdge::Left), None, Some(ScreenEdge::Top)],
        ..Default::default()
    };
    s
}
#[test]
fn native_writer_keeps_parameters_and_owners_without_approximation() {
    let dir = Temp::new();
    let path = dir.path("native.wav");
    let report = write(&path, rich_spec(), &[]);
    assert!(report.diagnostics.is_empty());
    let xml = String::from_utf8(chunk(&path, *b"axml")).unwrap();
    assert!(xml.contains("<diffuse>0.25</diffuse>"));
    assert!(xml.contains("<objectDivergence positionRange=\"0.2\">0.4</objectDivergence>"));
    assert!(xml.contains("<channelLock maxDistance=\"0.3\">1</channelLock>"));
    assert!(xml.contains("coordinate=\"X\" screenEdgeLock=\"left\""));
    assert!(xml.contains("<importance>9</importance>"));
    assert!(xml.contains("importance=\"3\" dialogue=\"1\""));
    assert!(xml.contains("typeDefinition=\"Objects\" importance=\"6\""));
    assert!(xml.contains("0.123456789012345"));
    assert!(xml.contains(" 节目 &amp; &lt;标题&gt; "));
    let precise_path = dir.path("precise.wav");
    let mut precise = spec();
    precise.objects[0].initial.diffuse = 0.12345678901234567;
    precise.objects[0].initial.divergence = Some(ObjectDivergence {
        value: 0.9876543210123456,
        azimuth_range: None,
        position_range: Some(0.23456789012345678),
    });
    precise.objects[0].initial.channel_lock_distance = Some(3.1234567890123457);
    write(&precise_path, precise.clone(), &[]);
    let precise_xml = String::from_utf8(chunk(&precise_path, *b"axml")).unwrap();
    assert!(precise_xml.contains(&format!(
        "<diffuse>{}</diffuse>",
        precise.objects[0].initial.diffuse
    )));
    #[cfg(feature = "read")]
    {
        let read = macin_atmos::adm::AdmReader::open(precise_path, LossPolicy::Reject).unwrap();
        assert_eq!(read.spec().objects[0].initial, precise.objects[0].initial);
    }
    let mut defaults = spec();
    defaults.profile = Profile::DamfHome;
    defaults.objects[0].adm = metadata(&[("importance", "10")], vec![]);
    defaults.objects[0].pack_adm = metadata(&[("importance", "10")], vec![]);
    defaults.bed_adm = metadata(&[("importance", "10")], vec![]);
    defaults.bed_pack_adm = metadata(&[("importance", "10")], vec![]);
    assert!(
        write(&dir.path("default-priority"), defaults, &[])
            .diagnostics
            .is_empty()
    );
}
#[test]
fn native_validation_rejects_bad_ranges_and_unowned_xml() {
    for initial in [
        ObjectState {
            diffuse: 1.1,
            ..Default::default()
        },
        ObjectState {
            channel_lock_distance: Some(-0.1),
            ..Default::default()
        },
        ObjectState {
            divergence: Some(ObjectDivergence {
                value: 0.1,
                azimuth_range: None,
                position_range: Some(2.),
            }),
            ..Default::default()
        },
        ObjectState {
            screen_edge_lock: [None, Some(ScreenEdge::Left), None],
            ..Default::default()
        },
    ] {
        let mut s = spec();
        s.objects[0].initial = initial;
        assert_eq!(s.validate().unwrap_err().kind, ErrorKind::InvalidInput);
    }
    for invalid in [
        element("audioObjectIDRef", &[], "AO_1001"),
        element("audioProgrammeLabel", &[("auditFlag", "1")], "text"),
        element("loudnessMetadata", &[], "not a number"),
    ] {
        let mut s = spec();
        s.adm.elements.push(invalid);
        assert_eq!(s.validate().unwrap_err().kind, ErrorKind::InvalidInput);
    }
    let screen = AdmElement {
        children: vec![
            element("screenCentrePosition", &[("azimuth", "0")], ""),
            element("screenWidth", &[("X", "1")], ""),
        ],
        ..element("audioProgrammeReferenceScreen", &[], "")
    };
    let mut s = spec();
    s.adm.elements.push(screen);
    assert!(s.validate().is_err());
    let mut s = spec();
    s.objects[0].adm.elements = vec![
        element("positionOffset", &[("coordinate", "X")], "0.1"),
        element("positionOffset", &[("coordinate", "X")], "0.2"),
    ];
    assert!(s.validate().is_err());
    let mut s = spec();
    s.objects[0].adm.attributes = vec![
        ("importance".into(), "3".into()),
        ("importance".into(), "5".into()),
    ];
    assert!(s.validate().is_err());
    for chunks in [
        vec![MetadataChunk {
            id: *b"data",
            data: vec![],
        }],
        vec![MetadataChunk {
            id: *b"LIST",
            data: b"adtl".to_vec(),
        }],
        vec![MetadataChunk {
            id: *b"iXML",
            data: vec![0; CONTAINER_METADATA_LIMIT + 1],
        }],
        vec![
            MetadataChunk {
                id: *b"iXML",
                data: vec![],
            },
            MetadataChunk {
                id: *b"iXML",
                data: vec![],
            },
        ],
    ] {
        assert_eq!(
            ContainerMetadata::new(MetadataContainer::Wave, chunks)
                .unwrap_err()
                .kind,
            ErrorKind::InvalidInput
        );
    }
}
#[test]
fn diffuse_approximation_conflicts_respect_native_value_and_are_atomic() {
    let dir = Temp::new();
    for (index, profile, field) in [
        (0, Profile::Standard, "decorrelation"),
        (1, Profile::DamfHome, "diffuse"),
    ] {
        let mut s = spec();
        s.profile = profile;
        s.approximate_mappings.diffuse_decorrelation = true;
        s.objects[0].initial.diffuse = 0.2;
        s.objects[0].initial.decorrelation = 0.8;
        let path = dir.path(&format!("reject-{index}"));
        assert_eq!(
            MasterWriter::create(&path, s.clone()).unwrap_err().field,
            Some(field)
        );
        assert!(!path.exists());
        s.loss_policy = LossPolicy::Allow;
        let mut w = MasterWriter::create(&path, s).unwrap();
        let state = w.spec().objects[0].initial;
        assert_eq!(
            (state.diffuse, state.decorrelation),
            if profile.is_adm() {
                (0.2, 0.)
            } else {
                (0., 0.8)
            }
        );
        w.write_block(PcmBlock::S24Le(&[0; 33]), &[]).unwrap();
        let report = w.finish().unwrap();
        assert_eq!(report.diagnostics[0].kind, MappingDiagnosticKind::Loss);
    }
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
    let mut w = MultiWriter::create(&spec(), &targets).unwrap();
    let event = ObjectUpdate {
        track: TrackId(1),
        sample: 10,
        ramp_samples: Some(0),
        patch: ObjectPatch {
            diffuse: Some(0.5),
            ..Default::default()
        },
    };
    assert_eq!(
        w.schedule_updates(&[event]).unwrap_err().field,
        Some("diffuse")
    );
    w.write_block(PcmBlock::S24Le(&[0; 33 * 20]), &[]).unwrap();
    assert!(w.finish().unwrap().iter().all(|r| r.diagnostics.is_empty()));
    assert!(
        !String::from_utf8(chunk(&targets[0].path, *b"axml"))
            .unwrap()
            .contains("<diffuse>")
    );
}

#[cfg(feature = "read")]
mod reading {
    use super::*;
    use macin_atmos::{adm::AdmReader, damf::DamfReader};
    fn target(path: PathBuf, profile: Profile) -> [Target; 1] {
        [Target { path, profile }]
    }
    #[test]
    fn standard_keeps_the_object_pack_when_the_track_omits_its_pack_reference() {
        let dir = Temp::new();
        let source = dir.path("source.wav");
        let mut s = spec();
        s.objects[0].pack_name = Some("Pack only on object".into());
        s.objects[0].pack_adm = metadata(&[("importance", "7")], vec![]);
        write(&source, s, &[]);
        let xml = String::from_utf8(chunk(&source, *b"axml")).unwrap();
        let (before, track) = xml.rsplit_once("<audioTrackUID ").unwrap();
        let clean = format!(
            "{before}<audioTrackUID {}",
            track.replace(
                "<audioPackFormatIDRef>AP_00031001</audioPackFormatIDRef>",
                ""
            )
        );
        assert_ne!(xml, clean);
        let step = dir.path("step.wav");
        macin_bwf::replace_chunk(
            fs::File::open(&source).unwrap(),
            fs::File::create(&step).unwrap(),
            *b"axml",
            clean.as_bytes(),
        )
        .unwrap();
        let mut tracks = macin_bwf::Chna::decode(&chunk(&step, *b"chna")).unwrap();
        tracks.entries[10].pack_format = [0; 11];
        let input = dir.path("object-pack.wav");
        macin_bwf::replace_chunk(
            fs::File::open(&step).unwrap(),
            fs::File::create(&input).unwrap(),
            *b"chna",
            &tracks.encode().unwrap(),
        )
        .unwrap();
        let read = AdmReader::open(input, LossPolicy::Reject).unwrap();
        assert_eq!(
            read.spec().objects[0].pack_name.as_deref(),
            Some("Pack only on object")
        );
        assert_eq!(
            read.spec().objects[0].pack_adm.attributes,
            [("importance".into(), "7".into())]
        );
    }

    #[test]
    fn wave_list_subtypes_are_read_by_occurrence() {
        use std::io::Write;
        let dir = Temp::new();
        let base = dir.path("base.wav");
        write(&base, spec(), &[]);
        let input = dir.path("two-lists.wav");
        let mut bytes = b"RIFF\0\0\0\0WAVE".to_vec();
        let info = b"INFOINAM\x04\0\0\0mix\0".to_vec();
        for (id, body) in [
            (*b"fmt ", chunk(&base, *b"fmt ")),
            (*b"chna", chunk(&base, *b"chna")),
            (*b"LIST", b"adtl".to_vec()),
            (*b"data", chunk(&base, *b"data")),
            (*b"LIST", info.clone()),
            (*b"axml", chunk(&base, *b"axml")),
        ] {
            bytes.write_all(&id).unwrap();
            bytes.write_all(&(body.len() as u32).to_le_bytes()).unwrap();
            bytes.write_all(&body).unwrap();
            if body.len() % 2 != 0 {
                bytes.push(0);
            }
        }
        let size = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&size.to_le_bytes());
        fs::write(&input, bytes).unwrap();
        assert_eq!(
            AdmReader::open(&input, LossPolicy::Reject)
                .unwrap_err()
                .field,
            Some("container_metadata")
        );
        let read = AdmReader::open(input, LossPolicy::Allow).unwrap();
        let out = dir.path("copy.wav");
        let report = read
            .convert(
                read.audio().unwrap(),
                &target(out.clone(), Profile::Standard),
            )
            .unwrap();
        assert_eq!(report.read.len(), 1);
        assert_eq!(chunk(&out, *b"LIST"), info);
    }

    #[test]
    fn fixed_bed_keeps_screen_edges_with_their_fallback_coordinates() {
        let dir = Temp::new();
        let mut original = spec();
        original.bed_geometry[0] = metadata(
            &[],
            vec![
                element(
                    "position",
                    &[("coordinate", "azimuth"), ("screenEdgeLock", "left")],
                    "29.75",
                ),
                element(
                    "position",
                    &[("coordinate", "azimuth"), ("bound", "min")],
                    "28.5",
                ),
                element(
                    "position",
                    &[("coordinate", "elevation"), ("screenEdgeLock", "top")],
                    "12",
                ),
            ],
        );
        original.bed_geometry[1] = metadata(
            &[],
            vec![
                element("cartesian", &[], "1"),
                element(
                    "position",
                    &[("coordinate", "X"), ("screenEdgeLock", "right")],
                    "0.875",
                ),
                element("position", &[("coordinate", "Y")], "0.925"),
            ],
        );
        let path = dir.path("source.wav");
        write(&path, original.clone(), &[]);
        let read = AdmReader::open(&path, LossPolicy::Reject).unwrap();
        assert_eq!(read.spec().bed_geometry, original.bed_geometry);
        let logic = dir.path("logic.wav");
        let report = read
            .convert(
                read.audio().unwrap(),
                &target(logic.clone(), Profile::Logic),
            )
            .unwrap();
        assert!(report.outputs[0].diagnostics.is_empty());
        assert_eq!(
            AdmReader::open(&logic, LossPolicy::Reject)
                .unwrap()
                .spec()
                .bed_geometry,
            original.bed_geometry
        );
        assert_eq!(chunk(&logic, *b"data"), chunk(&path, *b"data"));
        let home = target(dir.path("home"), Profile::DamfHome);
        assert_eq!(
            read.convert(read.audio().unwrap(), &home)
                .unwrap_err()
                .field,
            Some("bed_position")
        );
        let read = AdmReader::open(path, LossPolicy::Allow).unwrap();
        let report = read.convert(read.audio().unwrap(), &home).unwrap();
        assert_eq!(
            report.outputs[0]
                .diagnostics
                .iter()
                .filter(|d| d.field == "bed_position")
                .count(),
            2
        );
    }

    #[test]
    fn changing_bed_geometry_keeps_its_sample_and_fallback_coordinates() {
        let dir = Temp::new();
        let source = dir.path("source.wav");
        let mut s = spec();
        s.bed_geometry[0] = metadata(
            &[],
            vec![
                element(
                    "position",
                    &[("coordinate", "azimuth"), ("screenEdgeLock", "left")],
                    "34",
                ),
                element("position", &[("coordinate", "elevation")], "2"),
            ],
        );
        write(&source, s, &[]);
        let base = String::from_utf8(chunk(&source, *b"axml")).unwrap();
        let start = base.find("<audioBlockFormat ").unwrap();
        let body = start + base[start..].find('>').unwrap() + 1;
        let end = body + base[body..].find("</audioBlockFormat>").unwrap();
        let first = &base[body..end];
        for (label, value) in [("equivalent", "34.0"), ("changing", "30")] {
            let second = first.replace(
                r#"coordinate="azimuth" screenEdgeLock="left">34"#,
                &format!(r#"screenEdgeLock="left" coordinate="azimuth">{value}"#),
            );
            let blocks = format!(
                r#"<audioBlockFormat audioBlockFormatID="AB_00011001_00000001" rtime="00:00:00.00000" duration="00:00:00.05000">{first}</audioBlockFormat><audioBlockFormat audioBlockFormatID="AB_00011001_00000002" rtime="00:00:00.05000" duration="00:00:00.05000">{second}</audioBlockFormat>"#
            );
            let mut xml = base.clone();
            xml.replace_range(start..end + "</audioBlockFormat>".len(), &blocks);
            let input = dir.path(&format!("{label}.wav"));
            macin_bwf::replace_chunk(
                fs::File::open(&source).unwrap(),
                fs::File::create(&input).unwrap(),
                *b"axml",
                xml.as_bytes(),
            )
            .unwrap();
            let read = AdmReader::open(input, LossPolicy::Reject).unwrap();
            assert!(read.diagnostics().is_empty());
            let block = read.bed_blocks().next().unwrap().unwrap();
            assert_eq!(block.timing.start, Some(2400));
            assert_eq!(block.geometry.elements[0].text, value);
        }
    }
    #[test]
    fn legacy_screen_and_frequency_forms_write_standard_adm_spelling() {
        let dir = Temp::new();
        let source = dir.path("source.wav");
        write(&source, rich_spec(), &[]);
        let input = dir.path("legacy.wav");
        let xml = String::from_utf8(chunk(&source, *b"axml"))
            .unwrap()
            .replace(
                r#"<audioProgrammeReferenceScreen aspectRatio="1.78">"#,
                "<audioProgrammeReferenceScreen><aspectRatio>2.35</aspectRatio>",
            )
            .replace("<frequency ", "<channelFrequency ")
            .replace("</frequency>", "</channelFrequency>");
        assert!(xml.contains("<aspectRatio>2.35</aspectRatio>"));
        macin_bwf::replace_chunk(
            fs::File::open(source).unwrap(),
            fs::File::create(&input).unwrap(),
            *b"axml",
            xml.as_bytes(),
        )
        .unwrap();
        let read = AdmReader::open(input, LossPolicy::Reject).unwrap();
        let out = dir.path("copy.wav");
        read.convert(
            read.audio().unwrap(),
            &target(out.clone(), Profile::Standard),
        )
        .unwrap();
        let xml = String::from_utf8(chunk(&out, *b"axml")).unwrap();
        assert!(xml.contains(r#"<audioProgrammeReferenceScreen aspectRatio="2.35">"#));
        assert!(!xml.contains("<aspectRatio>"));
        assert!(!xml.contains("<channelFrequency"));
        assert!(xml.contains(r#"<frequency typeDefinition="highPass">80"#));
    }

    #[test]
    fn replacing_a_damf_caf_reports_discarded_info_and_permits_an_absent_original() {
        let dir = Temp::new();
        let mut original = spec();
        original.profile = Profile::DamfHome;
        let info = b"\0\0\0\x01artist\0Original\0";
        original.container_metadata = Some(
            ContainerMetadata::new(
                MetadataContainer::Caf,
                vec![MetadataChunk {
                    id: *b"info",
                    data: info.to_vec(),
                }],
            )
            .unwrap(),
        );
        let source = dir.path("source");
        write(&source, original, &[]);
        let manifest = source.join("master.atmos");
        let wav = dir.path("replacement.wav");
        write(&wav, spec(), &[]);
        for policy in [LossPolicy::Reject, LossPolicy::Allow] {
            let read = DamfReader::open(&manifest, policy).unwrap();
            let out = dir.path("output");
            let result = read.convert(
                read.wave_audio(&wav).unwrap(),
                &target(out.clone(), Profile::DamfHome),
            );
            if policy == LossPolicy::Reject {
                assert_eq!(result.unwrap_err().field, Some("container_metadata"));
                assert!(!out.exists());
            } else {
                assert!(
                    result
                        .unwrap()
                        .read
                        .iter()
                        .any(|d| d.field == "container_metadata")
                );
                let output = fs::read(out.join("master.atmos.audio")).unwrap();
                assert!(!output.windows(info.len()).any(|v| v == info));
            }
        }
        fs::remove_file(source.join("master.atmos.audio")).unwrap();
        let read = DamfReader::open(manifest, LossPolicy::Reject).unwrap();
        let converted = read
            .convert(
                read.wave_audio(&wav).unwrap(),
                &target(dir.path("recovered"), Profile::DamfHome),
            )
            .unwrap();
        assert!(converted.read.is_empty());
    }

    #[test]
    fn approximate_decorrelation_on_a_motion_event_copies_the_value_at_its_timestamp() {
        let dir = Temp::new();
        let mut original = spec();
        original.approximate_mappings.diffuse_decorrelation = true;
        let output = dir.path("motion.wav");
        write(
            &output,
            original,
            &[
                ObjectUpdate {
                    track: TrackId(1),
                    sample: 1200,
                    ramp_samples: Some(2400),
                    patch: ObjectPatch {
                        position: Some([1., 0., 0.]),
                        decorrelation: Some(0.8),
                        ..Default::default()
                    },
                },
                ObjectUpdate {
                    track: TrackId(1),
                    sample: 2400,
                    ramp_samples: None,
                    patch: ObjectPatch {
                        snap: Some(true),
                        ..Default::default()
                    },
                },
            ],
        );
        let read = AdmReader::open(output, LossPolicy::Reject).unwrap();
        let updates = read.updates().unwrap().collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(
            (updates[0].sample, updates[0].patch.diffuse),
            (1200, Some(0.8))
        );
        assert_eq!(updates[0].patch.position, Some([0.5, 0., 0.]));
        assert_eq!(updates[1].patch.diffuse, None);
    }

    #[test]
    fn cartesian_divergence_keeps_legacy_range_for_inspection_and_reports_it_per_target() {
        let dir = Temp::new();
        let mut original = spec();
        let value = ObjectDivergence {
            value: 0.6,
            position_range: Some(0.2),
            azimuth_range: None,
        };
        original.objects[0].initial.divergence = Some(value);
        let source = dir.path("source.wav");
        write(&source, original.clone(), &[]);
        let xml = String::from_utf8(chunk(&source, *b"axml")).unwrap();
        let legacy = dir.path("legacy.wav");
        let legacy_xml = xml.replace(r#"positionRange="0.2""#, r#"azimuthRange="30""#);
        macin_bwf::replace_chunk(
            fs::File::open(&source).unwrap(),
            fs::File::create(&legacy).unwrap(),
            *b"axml",
            legacy_xml.as_bytes(),
        )
        .unwrap();
        let strict = AdmReader::open(&legacy, LossPolicy::Reject).unwrap();
        assert_eq!(
            strict.spec().objects[0]
                .initial
                .divergence
                .unwrap()
                .azimuth_range,
            Some(30.)
        );
        let out = target(dir.path("copy.wav"), Profile::Standard);
        assert_eq!(
            strict
                .convert(strict.audio().unwrap(), &out)
                .unwrap_err()
                .field,
            Some("divergence_azimuth_range")
        );
        let allow = AdmReader::open(legacy, LossPolicy::Allow).unwrap();
        let report = allow.convert(allow.audio().unwrap(), &out).unwrap();
        assert_eq!(
            report.outputs[0].diagnostics[0].field,
            "divergence_azimuth_range"
        );
        let read = AdmReader::open(&out[0].path, LossPolicy::Reject).unwrap();
        assert_eq!(
            read.spec().objects[0].initial.divergence,
            Some(ObjectDivergence {
                position_range: None,
                ..value
            })
        );
        original.objects[0]
            .initial
            .divergence
            .as_mut()
            .unwrap()
            .azimuth_range = Some(30.);
        assert_eq!(
            original.validate().unwrap_err().kind,
            ErrorKind::InvalidInput
        );
    }
    #[test]
    fn native_adm_roundtrip_retains_metadata_levels_precision_and_pcm() {
        let dir = Temp::new();
        let source = dir.path("source.wav");
        let s = rich_spec();
        write(
            &source,
            s.clone(),
            &[
                ObjectUpdate {
                    track: TrackId(1),
                    sample: 1200,
                    ramp_samples: Some(600),
                    patch: ObjectPatch {
                        diffuse: Some(0.75),
                        divergence: Some(Some(ObjectDivergence {
                            value: 0.8,
                            azimuth_range: None,
                            position_range: Some(0.5),
                        })),
                        ..Default::default()
                    },
                },
                ObjectUpdate {
                    track: TrackId(1),
                    sample: 2400,
                    ramp_samples: Some(0),
                    patch: ObjectPatch {
                        diffuse: Some(0.),
                        divergence: Some(None),
                        channel_lock_distance: Some(None),
                        screen_edge_lock: Some([None; 3]),
                        ..Default::default()
                    },
                },
            ],
        );
        let reader = AdmReader::open(&source, LossPolicy::Reject).unwrap();
        assert!(reader.diagnostics().is_empty());
        assert_eq!(reader.spec().adm, s.adm);
        assert_eq!(reader.spec().contents[0].adm, s.contents[0].adm);
        assert_eq!(reader.spec().bed_adm, s.bed_adm);
        assert_eq!(reader.spec().bed_pack_adm, s.bed_pack_adm);
        assert_eq!(reader.spec().bed_channel_adm, s.bed_channel_adm);
        let object = &reader.spec().objects[0];
        assert_eq!(object.adm, s.objects[0].adm);
        assert_eq!(object.pack_adm, s.objects[0].pack_adm);
        assert_eq!(object.channel_adm, s.objects[0].channel_adm);
        assert_eq!(object.initial, s.objects[0].initial);
        let events: Vec<_> = reader.updates().unwrap().collect::<Result<_>>().unwrap();
        assert_eq!(events[0].sample, 1200);
        assert_eq!(events[0].ramp_samples, Some(600));
        assert_eq!(events[0].patch.diffuse, Some(0.75));
        assert_eq!(events[1].patch.diffuse, Some(0.));
        assert_eq!(events[1].patch.divergence, Some(None));
        let copy = dir.path("copy.wav");
        let report = reader
            .convert(
                reader.audio().unwrap(),
                &target(copy.clone(), Profile::Standard),
            )
            .unwrap();
        assert!(report.read.is_empty() && report.outputs[0].diagnostics.is_empty());
        assert_eq!(chunk(&copy, *b"data"), chunk(&source, *b"data"));
        assert_eq!(chunk(&copy, *b"axml"), chunk(&source, *b"axml"));
    }
    #[test]
    fn clearing_divergence_mid_ramp_keeps_the_intermediate_value() {
        let dir = Temp::new();
        let path = dir.path("ramp.wav");
        let mut s = spec();
        s.objects[0].initial.divergence = Some(ObjectDivergence {
            value: 0.8,
            azimuth_range: None,
            position_range: Some(0.5),
        });
        write(
            &path,
            s,
            &[
                ObjectUpdate {
                    track: TrackId(1),
                    sample: 1200,
                    ramp_samples: Some(2400),
                    patch: ObjectPatch {
                        divergence: Some(None),
                        ..Default::default()
                    },
                },
                ObjectUpdate {
                    track: TrackId(1),
                    sample: 2400,
                    ramp_samples: None,
                    patch: ObjectPatch {
                        snap: Some(true),
                        ..Default::default()
                    },
                },
            ],
        );
        let reader = AdmReader::open(path, LossPolicy::Reject).unwrap();
        let updates = reader
            .updates()
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert_eq!(updates[0].sample, 1200);
        assert_eq!(updates[0].ramp_samples, Some(1200));
        assert_eq!(
            updates[0].patch.divergence,
            Some(Some(ObjectDivergence {
                value: 0.4,
                azimuth_range: None,
                position_range: Some(0.25)
            }))
        );
        assert_eq!(updates[1].sample, 2400);
        assert_eq!(updates[1].patch.divergence, Some(None));
        assert_eq!(updates[1].ramp_samples, Some(1200));
        assert_eq!(updates[1].patch.snap, Some(true));
    }

    #[test]
    fn reordering_channels_rejects_ancillary_passthrough_and_unknown_nested_fields_are_diagnosed() {
        let dir = Temp::new();
        let source = dir.path("source.wav");
        let mut s = rich_spec();
        s.container_metadata = Some(
            ContainerMetadata::new(
                MetadataContainer::Wave,
                vec![MetadataChunk {
                    id: *b"iXML",
                    data: b"<BWFXML/>".to_vec(),
                }],
            )
            .unwrap(),
        );
        write(&source, s, &[]);
        let changed = dir.path("reordered.wav");
        let mut chna = macin_bwf::Chna::decode(&chunk(&source, *b"chna")).unwrap();
        chna.entries[0].track_index = 2;
        chna.entries[1].track_index = 1;
        macin_bwf::replace_chunk(
            fs::File::open(&source).unwrap(),
            fs::File::create(&changed).unwrap(),
            *b"chna",
            &chna.encode().unwrap(),
        )
        .unwrap();
        let reader = AdmReader::open(&changed, LossPolicy::Reject).unwrap();
        assert_eq!(
            reader
                .convert(
                    reader.audio().unwrap(),
                    &target(dir.path("rejected.wav"), Profile::Standard)
                )
                .unwrap_err()
                .field,
            Some("container_metadata")
        );
        let changed = dir.path("extension.wav");
        let xml = String::from_utf8(chunk(&source, *b"axml"))
            .unwrap()
            .replacen(
                "<loudnessMetadata ",
                "<loudnessMetadata auditFlag=\"1\" ",
                1,
            );
        macin_bwf::replace_chunk(
            fs::File::open(&source).unwrap(),
            fs::File::create(&changed).unwrap(),
            *b"axml",
            xml.as_bytes(),
        )
        .unwrap();
        assert_eq!(
            AdmReader::open(&changed, LossPolicy::Reject)
                .unwrap_err()
                .field,
            Some("unknown_field")
        );
        let allowed = AdmReader::open(changed, LossPolicy::Allow).unwrap();
        assert_eq!(allowed.diagnostics()[0].field, "unknown_field");
        assert!(
            allowed
                .spec()
                .adm
                .elements
                .iter()
                .any(|e| e.name == "loudnessMetadata")
        );
        let foreign = dir.path("foreign-extension.wav");
        let xml = String::from_utf8(chunk(&source, *b"axml"))
            .unwrap()
            .replacen(
                "</loudnessMetadata>",
                r#"<v:calibration xmlns:v="urn:test">custom</v:calibration></loudnessMetadata>"#,
                1,
            );
        macin_bwf::replace_chunk(
            fs::File::open(&source).unwrap(),
            fs::File::create(&foreign).unwrap(),
            *b"axml",
            xml.as_bytes(),
        )
        .unwrap();
        assert_eq!(
            AdmReader::open(&foreign, LossPolicy::Reject)
                .unwrap_err()
                .field,
            Some("unknown_field")
        );
        let allowed = AdmReader::open(foreign, LossPolicy::Allow).unwrap();
        assert!(allowed.diagnostics()[0].message.contains("v:calibration"));
        let duplicate = dir.path("duplicate-block.wav");
        let mut xml = String::from_utf8(chunk(&source, *b"axml")).unwrap();
        let object_channel = xml
            .find("<audioChannelFormat audioChannelFormatID=\"AC_00031001\"")
            .unwrap();
        let at = object_channel + xml[object_channel..].find("</audioBlockFormat>").unwrap();
        xml.insert_str(at, "<diffuse>0.5</diffuse>");
        macin_bwf::replace_chunk(
            fs::File::open(&source).unwrap(),
            fs::File::create(&duplicate).unwrap(),
            *b"axml",
            xml.as_bytes(),
        )
        .unwrap();
        assert_eq!(
            AdmReader::open(duplicate, LossPolicy::Allow)
                .unwrap_err()
                .kind,
            ErrorKind::InvalidInput
        );
        let oversized = dir.path("over-budget.wav");
        macin_bwf::replace_chunk(
            fs::File::open(&source).unwrap(),
            fs::File::create(&oversized).unwrap(),
            *b"iXML",
            &vec![0; CONTAINER_METADATA_LIMIT + 1],
        )
        .unwrap();
        assert_eq!(
            AdmReader::open(&oversized, LossPolicy::Reject)
                .unwrap_err()
                .field,
            Some("container_metadata")
        );
        let reader = AdmReader::open(oversized, LossPolicy::Allow).unwrap();
        assert!(reader.spec().container_metadata.is_none());
        assert_eq!(reader.diagnostics()[0].field, "container_metadata");
    }

    #[test]
    fn native_metadata_losses_are_target_specific_and_approximation_does_not_enable_them() {
        let dir = Temp::new();
        let source = dir.path("source.wav");
        write(&source, rich_spec(), &[]);
        let reader = AdmReader::open_with_options(
            &source,
            MappingOptions {
                approximate_mappings: ApproximateMappings {
                    diffuse_decorrelation: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        assert!(reader.diagnostics().is_empty());
        assert_eq!(
            reader
                .convert(
                    reader.audio().unwrap(),
                    &target(dir.path("home"), Profile::DamfHome)
                )
                .unwrap_err()
                .field,
            Some("divergence")
        );
        let reader = AdmReader::open_with_options(
            &source,
            MappingOptions {
                preserve_literal: Default::default(),
                loss_policy: LossPolicy::Allow,
                approximate_mappings: ApproximateMappings {
                    diffuse_decorrelation: true,
                    ..Default::default()
                },
            },
        )
        .unwrap();
        let report = reader
            .convert(
                reader.audio().unwrap(),
                &[
                    Target {
                        path: dir.path("copy.wav"),
                        profile: Profile::Standard,
                    },
                    Target {
                        path: dir.path("home"),
                        profile: Profile::DamfHome,
                    },
                ],
            )
            .unwrap();
        assert!(report.read.is_empty() && report.outputs[0].diagnostics.is_empty());
        let fields: Vec<_> = report.outputs[1]
            .diagnostics
            .iter()
            .map(|d| d.field)
            .collect();
        for field in [
            "diffuse",
            "divergence",
            "channel_lock_distance",
            "screen_edge_lock",
            "language",
            "labels",
            "loudness",
            "reference_screen",
            "importance",
            "pack_importance",
            "position_offset",
            "frequency",
            "interaction",
            "ducking",
        ] {
            assert!(fields.contains(&field), "{field}: {fields:?}");
        }
        let damf = DamfReader::open(dir.path("home/master.atmos"), LossPolicy::Reject).unwrap();
        assert_eq!(damf.spec().objects[0].initial.decorrelation, 0.25);
    }
    #[test]
    fn replacing_audio_invalidates_loudness_and_allow_does_not_copy_stale_values() {
        let dir = Temp::new();
        let source = dir.path("source.wav");
        write(&source, rich_spec(), &[]);
        let replacement = dir.path("replacement.wav");
        write(&replacement, spec(), &[]);
        let reader = AdmReader::open(&source, LossPolicy::Reject).unwrap();
        assert_eq!(
            reader
                .convert(
                    reader.wave_audio(&replacement).unwrap(),
                    &target(dir.path("reject.wav"), Profile::Standard)
                )
                .unwrap_err()
                .field,
            Some("loudness")
        );
        let reader = AdmReader::open(source, LossPolicy::Allow).unwrap();
        let output = dir.path("copy.wav");
        let report = reader
            .convert(
                reader.wave_audio(replacement).unwrap(),
                &target(output.clone(), Profile::Standard),
            )
            .unwrap();
        assert!(report.read.iter().any(|d| d.field == "loudness"));
        assert!(
            !String::from_utf8(chunk(&output, *b"axml"))
                .unwrap()
                .contains("<loudnessMetadata")
        );
    }
    #[test]
    fn wave_chunks_survive_safe_conversion_and_reject_changed_layout_duration_or_family() {
        let dir = Temp::new();
        let source = dir.path("source.wav");
        let mut s = spec();
        s.container_metadata = Some(
            ContainerMetadata::new(
                MetadataContainer::Wave,
                vec![
                    MetadataChunk {
                        id: *b"bext",
                        data: vec![0; 602],
                    },
                    MetadataChunk {
                        id: *b"iXML",
                        data: b"<BWFXML><NOTE>synthetic</NOTE></BWFXML>".to_vec(),
                    },
                    MetadataChunk {
                        id: *b"LIST",
                        data: b"INFOodd".to_vec(),
                    },
                ],
            )
            .unwrap(),
        );
        write(&source, s, &[]);
        let reader = AdmReader::open(&source, LossPolicy::Reject).unwrap();
        let copy = dir.path("copy.wav");
        reader
            .convert(
                reader.audio().unwrap(),
                &target(copy.clone(), Profile::Logic),
            )
            .unwrap();
        for id in [*b"bext", *b"iXML", *b"LIST", *b"data"] {
            assert_eq!(chunk(&copy, id), chunk(&source, id));
        }
        assert_eq!(
            reader
                .convert(
                    reader.audio().unwrap(),
                    &target(dir.path("home"), Profile::DamfHome)
                )
                .unwrap_err()
                .field,
            Some("container_metadata")
        );
        for change in 0..3 {
            let mut s = reader.spec().clone();
            match change {
                0 => s.start_sample += 1,
                1 => {
                    s.objects.push(ObjectSpec::new(TrackId(99), "extra"));
                    s.contents[0].objects.push(TrackId(99));
                }
                _ => s.sample_rate = 96_000,
            };
            assert_eq!(
                MasterWriter::create(dir.path(&format!("changed-{change}")), s)
                    .unwrap_err()
                    .field,
                Some("container_metadata")
            );
        }
        let mut w = MasterWriter::create(dir.path("short.wav"), reader.spec().clone()).unwrap();
        w.write_block(PcmBlock::S24Le(&[0; 33]), &[]).unwrap();
        assert_eq!(w.finish().unwrap_err().field, Some("container_metadata"));
        assert!(!dir.path("short.wav").exists());
        let allowed = AdmReader::open(&source, LossPolicy::Allow).unwrap();
        let report = allowed
            .convert(
                allowed.audio().unwrap(),
                &target(dir.path("home"), Profile::DamfHome),
            )
            .unwrap();
        assert_eq!(report.outputs[0].diagnostics[0].field, "container_metadata");
    }
}

#[test]
fn custom_zones_cannot_overwrite_a_preset_or_bypass_other_targets() {
    let dir = Temp::new();
    let targets = [
        Target {
            path: dir.path("a.wav"),
            profile: Profile::Standard,
        },
        Target {
            path: dir.path("home"),
            profile: Profile::DamfHome,
        },
    ];
    let s = spec();
    let mut w = MultiWriter::create(&s, &targets).unwrap();
    let z = ExclusionZone {
        name: "custom".into(),
        bounds: ZoneBounds::Cartesian([-0.9, -0.3, -1., 0., -1., 1.]),
    };
    let event = ObjectUpdate {
        track: TrackId(1),
        sample: 50,
        ramp_samples: None,
        patch: Default::default(),
    };
    assert_eq!(
        w.schedule_native_updates(&[NativeObjectUpdate {
            event,
            zones: Some(vec![z.clone()])
        }])
        .unwrap_err()
        .field,
        Some("zone")
    );
    let mut bad = z.clone();
    bad.bounds = ZoneBounds::Polar([-90., 90., 95., 0.]);
    assert_eq!(
        w.schedule_native_updates(&[NativeObjectUpdate {
            event,
            zones: Some(vec![bad])
        }])
        .unwrap_err()
        .kind,
        ErrorKind::InvalidInput
    );
    let mut corrected = event;
    corrected.patch.snap = Some(true);
    w.schedule_updates(&[corrected]).unwrap();
    w.write_block(PcmBlock::S24Le(&vec![0; 3300]), &[]).unwrap();
    assert!(w.finish().unwrap().iter().all(|r| r.diagnostics.is_empty()));
    assert!(
        !String::from_utf8(chunk(&targets[0].path, *b"axml"))
            .unwrap()
            .contains("zoneExclusion")
    );
    let mut s = spec();
    s.objects[0].custom_zones = vec![z];
    s.objects[0].initial.zone = Zone::NoBack;
    assert_eq!(
        MasterWriter::create(dir.path("bad.wav"), s)
            .unwrap_err()
            .kind,
        ErrorKind::InvalidInput
    );
}

#[cfg(feature = "read")]
mod remaining {
    use super::*;
    use macin_atmos::{adm::AdmReader, damf::DamfReader};
    fn target(path: PathBuf, profile: Profile) -> [Target; 1] {
        [Target { path, profile }]
    }
    #[test]
    fn vendor_attributes_retain_namespaces_values_whitespace_and_scope() {
        let dir = Temp::new();
        let source = dir.path("source.wav");
        write(&source, spec(), &[]);
        let xml = String::from_utf8(chunk(&source, *b"axml")).unwrap();
        let mut xml = xml.replacen(
            "<ebuCoreMain ",
            r#"<ebuCoreMain xmlns:v="urn:vendor:outer" xmlns:u="urn:units" "#,
            1,
        );
        for (node, value) in [
            ("audioStreamFormat", r#"v:描述="u:gain&#9;&amp;&#10;end""#),
            (
                "audioTrackFormat",
                r#"xmlns:v="urn:vendor:inner" v:note="inner""#,
            ),
            ("audioTrackUID", r#"v:note="uid""#),
        ] {
            let start = xml.find(&format!("<{node} ")).unwrap() + node.len() + 1;
            xml.insert_str(start, &format!(" {value}"));
        }
        let input = dir.path("in.wav");
        macin_bwf::replace_chunk(
            fs::File::open(source).unwrap(),
            fs::File::create(&input).unwrap(),
            *b"axml",
            xml.as_bytes(),
        )
        .unwrap();
        let r = AdmReader::open(&input, LossPolicy::Reject).unwrap();
        let attrs = &r.spec().bed_channel_names[0].extensions;
        assert_eq!(
            (
                &*attrs.stream[0].namespace,
                &*attrs.stream[0].name,
                &*attrs.stream[0].value
            ),
            ("urn:vendor:outer", "描述", "u:gain\t&\nend")
        );
        assert_eq!(
            attrs.stream[0].value_prefixes,
            [("u".into(), "urn:units".into())]
        );
        assert_eq!(attrs.track[0].namespace, "urn:vendor:inner");
        assert_eq!(attrs.uid[0].value, "uid");
        for profile in [Profile::Standard, Profile::Logic] {
            let out = dir.path(&format!("{profile:?}.wav"));
            r.convert(r.audio().unwrap(), &target(out.clone(), profile))
                .unwrap();
            let back = AdmReader::open(out, LossPolicy::Reject).unwrap();
            assert_eq!(back.spec().bed_channel_names[0].extensions, *attrs);
        }
        assert_eq!(
            r.convert(
                r.audio().unwrap(),
                &target(dir.path("home"), Profile::DamfHome)
            )
            .unwrap_err()
            .field,
            Some("track_extensions")
        );
    }
    #[test]
    fn custom_object_blocks_preserve_all_bounds_headphones_and_resets() {
        let dir = Temp::new();
        let z = ExclusionZone {
            name: " Custom <L> & R ".into(),
            bounds: ZoneBounds::Cartesian([-0.9123456789012345, -0.3, -0.9, 0.8, -0.75, 0.7]),
        };
        let polar = ExclusionZone {
            name: "rear".into(),
            bounds: ZoneBounds::Polar([160., -160., -25., 45.]),
        };
        let mut s = spec();
        s.objects[0].custom_zones = vec![z.clone()];
        s.objects[0].initial.headphone_virtualise = Some(HeadphoneVirtualise {
            bypass: Some(true),
            drr: Some(-12.5),
        });
        let path = dir.path("source.wav");
        let mut w = MasterWriter::create(&path, s).unwrap();
        let update = |sample, zones, h| NativeObjectUpdate {
            event: ObjectUpdate {
                track: TrackId(1),
                sample,
                ramp_samples: None,
                patch: ObjectPatch {
                    headphone_virtualise: Some(h),
                    ..Default::default()
                },
            },
            zones: Some(zones),
        };
        w.schedule_native_updates(&[
            update(
                100,
                vec![polar.clone(), z.clone()],
                Some(HeadphoneVirtualise {
                    bypass: Some(false),
                    drr: Some(9.),
                }),
            ),
            update(200, vec![], None),
        ])
        .unwrap();
        w.write_block(PcmBlock::S24Le(&vec![7; 330 * 33]), &[])
            .unwrap();
        w.finish().unwrap();
        let r = AdmReader::open(&path, LossPolicy::Reject).unwrap();
        let expected = r
            .native_updates()
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            r.spec().objects[0].custom_zones.as_slice(),
            std::slice::from_ref(&z)
        );
        assert_eq!(expected.len(), 2);
        assert_eq!(expected[0].zones, Some(vec![polar, z]));
        assert_eq!(expected[1].zones, Some(vec![]));
        assert_eq!(expected[1].event.patch.headphone_virtualise, Some(None));
        for profile in [Profile::Standard, Profile::Logic] {
            let out = dir.path(&format!("{profile:?}.wav"));
            r.convert(r.audio().unwrap(), &target(out.clone(), profile))
                .unwrap();
            let back = AdmReader::open(&out, LossPolicy::Reject).unwrap();
            assert_eq!(r.spec().objects[0].initial, back.spec().objects[0].initial);
            assert_eq!(
                r.spec().objects[0].custom_zones,
                back.spec().objects[0].custom_zones
            );
            assert_eq!(
                expected,
                back.native_updates()
                    .unwrap()
                    .collect::<Result<Vec<_>>>()
                    .unwrap()
            );
            assert_eq!(chunk(&path, *b"data"), chunk(&out, *b"data"));
        }
        let allowed = AdmReader::open(path, LossPolicy::Allow).unwrap();
        let report = allowed
            .convert(
                allowed.audio().unwrap(),
                &target(dir.path("home"), Profile::DamfHome),
            )
            .unwrap();
        assert!(report.read.is_empty());
        for f in ["zone", "headphone_virtualise"] {
            assert!(report.outputs[0].diagnostics.iter().any(|d| d.field == f));
        }
    }
    #[test]
    fn native_bed_gaps_and_object_gain_apply_once_in_damf() {
        let dir = Temp::new();
        let mut s = spec();
        s.bed_timing = Some(AdmTiming {
            start: Some(100),
            duration: Some(600),
        });
        s.bed_adm.elements.push(element("gain", &[], "0.5"));
        s.objects[0].adm.elements.push(element("gain", &[], "0.25"));
        s.objects[0].initial.gain = Gain::Linear(0.5);
        let first = AdmBedBlock {
            channel: 0,
            timing: AdmTiming {
                start: Some(50),
                duration: Some(100),
            },
            labels: vec!["RC_L".into()],
            geometry: Default::default(),
            gain: Some(Gain::Linear(0.75)),
            importance: Some(3),
            head_tracking: HeadTracking::SceneRelative,
            headphone_virtualise: None,
        };
        s.bed_blocks[0] = Some(first.clone());
        let mut next = first.clone();
        next.timing = AdmTiming {
            start: Some(250),
            duration: None,
        };
        next.gain = Some(Gain::Linear(0.5));
        next.head_tracking = HeadTracking::HeadRelative;
        let src = dir.path("native.wav");
        let mut w = MasterWriter::create(&src, s).unwrap();
        w.schedule_adm_bed_blocks(std::slice::from_ref(&next))
            .unwrap();
        w.schedule_updates(&[ObjectUpdate {
            track: TrackId(1),
            sample: 300,
            ramp_samples: Some(0),
            patch: ObjectPatch {
                gain: Some(Gain::Linear(0.75)),
                ..Default::default()
            },
        }])
        .unwrap();
        w.write_block(PcmBlock::S24Le(&vec![1; 1000 * 33]), &[])
            .unwrap();
        w.finish().unwrap();
        let r = AdmReader::open(&src, LossPolicy::Reject).unwrap();
        assert_eq!(r.spec().bed_blocks[0], Some(first));
        assert_eq!(r.bed_blocks().collect::<Result<Vec<_>>>().unwrap(), [next]);
        let out = dir.path("home");
        r.convert(r.audio().unwrap(), &target(out.clone(), Profile::DamfHome))
            .unwrap();
        let home = DamfReader::open(out.join("master.atmos"), LossPolicy::Reject).unwrap();
        assert!(!home.spec().bed[0].active);
        assert!((home.spec().bed[0].gain.linear() - 0.375).abs() < 1e-8);
        assert!((home.spec().objects[0].initial.gain.linear() - 0.125).abs() < 1e-8);
        let bs = home
            .bed_updates()
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let bs: Vec<_> = bs.iter().filter(|b| b.channel == 0).collect();
        assert_eq!(
            bs.iter()
                .map(|b| (b.sample, b.patch.active))
                .collect::<Vec<_>>(),
            [
                (150, Some(true)),
                (250, Some(false)),
                (350, Some(true)),
                (700, Some(false))
            ]
        );
        assert_eq!(bs[2].patch.head_tracking, Some(HeadTracking::HeadRelative));
        assert!((bs[2].patch.gain.unwrap().linear() - 0.25).abs() < 1e-8);
        assert!(
            (home
                .updates()
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .patch
                .gain
                .unwrap()
                .linear()
                - 0.1875)
                .abs()
                < 1e-8
        );
    }
    #[test]
    fn native_bed_batch_rejects_overlaps_for_damf_without_touching_the_adm_spool() {
        let dir = Temp::new();
        let mut s = spec();
        let first = AdmBedBlock {
            channel: 0,
            timing: AdmTiming {
                start: Some(0),
                duration: Some(400),
            },
            labels: vec!["RC_L".into()],
            geometry: Default::default(),
            gain: Some(Gain::Linear(0.8)),
            importance: None,
            head_tracking: HeadTracking::SceneRelative,
            headphone_virtualise: None,
        };
        s.bed_blocks[0] = Some(first.clone());
        let targets = [
            Target {
                profile: Profile::Standard,
                path: dir.path("a.wav"),
            },
            Target {
                profile: Profile::DamfHome,
                path: dir.path("home"),
            },
        ];
        let mut writer = MultiWriter::create(&s, &targets).unwrap();
        let mut second = first;
        second.gain = Some(Gain::Linear(0.5));
        second.timing = AdmTiming {
            start: Some(200),
            duration: Some(150),
        };
        let error = writer
            .schedule_adm_bed_blocks(std::slice::from_ref(&second))
            .unwrap_err();
        assert_eq!((error.field, error.sample), (Some("bed_timing"), Some(200)));
        second.timing.start = Some(400);
        writer
            .schedule_adm_bed_blocks(std::slice::from_ref(&second))
            .unwrap();
        writer
            .write_block(PcmBlock::S24Le(&vec![0; 33 * 600]), &[])
            .unwrap();
        assert!(
            writer
                .finish()
                .unwrap()
                .iter()
                .all(|r| r.diagnostics.is_empty())
        );
        let r = AdmReader::open(&targets[0].path, LossPolicy::Reject).unwrap();
        assert_eq!(r.bed_update_count(), 1);
        assert_eq!(r.bed_blocks().next().unwrap().unwrap(), second);
        let home =
            DamfReader::open(targets[1].path.join("master.atmos"), LossPolicy::Reject).unwrap();
        let events = home
            .bed_updates()
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.channel == 0)
                .map(|e| e.sample)
                .collect::<Vec<_>>(),
            [400, 550]
        );
    }
    #[test]
    fn damf_bed_rendering_changes_and_mid_literal_roundtrip_independently() {
        let dir = Temp::new();
        let mut s = spec();
        s.profile = Profile::DamfHome;
        s.objects[0].initial.binaural_mode = BinauralMode::Mid;
        s.bed[2].binaural_mode = BinauralMode::Mid;
        s.preserve_literal.damf_mid = true;
        let p = BedRendering {
            position: Some([0.5, -0.25, 0.]),
            extent: Some(Some(Extent::Uniform(0.4))),
            snap: Some(true),
            zone: Some(Zone::NoSides),
            elevation: Some(false),
            decorrelation: Some(0.75),
            dialog: Some(1),
            music: Some(0),
            screen_factor: Some(0.2),
            depth_factor: Some(1.2),
        };
        s.bed[0].rendering = p;
        let path = dir.path("in");
        let mut w = MasterWriter::create(&path, s).unwrap();
        let patch = BedRendering {
            position: Some([0.; 3]),
            extent: Some(None),
            snap: Some(false),
            screen_factor: Some(0.),
            ..Default::default()
        };
        w.schedule_bed_updates(&[BedUpdate {
            channel: 0,
            sample: 500,
            ramp_samples: Some(30),
            patch: BedPatch {
                rendering: patch,
                ..Default::default()
            },
        }])
        .unwrap();
        w.write_block(PcmBlock::S24Le(&vec![0; 1000 * 33]), &[])
            .unwrap();
        assert!(
            w.finish()
                .unwrap()
                .diagnostics
                .iter()
                .all(|d| d.kind == MappingDiagnosticKind::PreservedLiteral)
        );
        let options = MappingOptions {
            preserve_literal: LiteralPreservation {
                damf_mid: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let r = DamfReader::open_with_options(path.join("master.atmos"), options).unwrap();
        assert_eq!(r.spec().bed[0].rendering, p);
        assert_eq!(r.spec().objects[0].initial.binaural_mode, BinauralMode::Mid);
        let updates = r
            .bed_updates()
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert_eq!(updates[0].patch.rendering, patch);
        assert_eq!(updates[0].ramp_samples, Some(30));
        let copy = dir.path("copy");
        r.convert(r.audio().unwrap(), &target(copy.clone(), Profile::DamfHome))
            .unwrap();
        for f in ["master.atmos.audio", "master.atmos.metadata"] {
            assert_eq!(
                fs::read(path.join(f)).unwrap(),
                fs::read(copy.join(f)).unwrap()
            );
        }
        // 静态 Mid 可写入 Logic DBMD，bed 的 DAMF 位置仍有独立的能力检查。
        assert_eq!(
            r.convert(
                r.audio().unwrap(),
                &target(dir.path("logic.wav"), Profile::Logic)
            )
            .unwrap_err()
            .field,
            Some("bed_position")
        );
    }
}
