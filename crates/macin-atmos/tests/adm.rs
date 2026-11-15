#![cfg(feature = "read")]
use macin_atmos::adm::AdmReader;
use macin_atmos::damf::DamfReader;
use macin_atmos::*;
use macin_bwf::{Chna, ChnaEntry, Container, Format, Reader, SampleFormat, Writer, WriterOptions};
use std::{
    fs,
    io::BufReader,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "macin-atmos-adm-{}-{}",
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

const FRAMES: usize = 9000;

/// A model ADM can hold exactly: no zones, headphone policy, dialogue or decorrelation.
fn spec(profile: Profile) -> MasterSpec {
    let mut spec = MasterSpec {
        profile,
        name: "Feature mix".into(),
        bed_name: "Main bed".into(),
        objects: vec![
            ObjectSpec::new(TrackId(5), "Dialog"),
            ObjectSpec::new(TrackId(6), "Fly-by"),
            ObjectSpec::new(TrackId(7), "Wide pad"),
        ],
        ..Default::default()
    };
    let o = &mut spec.objects;
    o[0].initial.position = [0., 1., 0.];
    o[0].initial.screen_ref = true;
    o[1].initial.position = [-1., 0.5, 0.25];
    o[1].initial.importance = 0.7;
    o[2].initial.extent = Some(Extent::Cartesian([0.5, 0.25, 0.125]));
    o[2].initial.snap = true;
    spec
}
/// The same model with what DAMF does not carry left at its default: screenRef has no DAMF
/// mapping, and DAMF has no programme name, so the name comes back as the default.
fn damf_safe(profile: Profile) -> MasterSpec {
    let mut spec = spec(profile);
    spec.objects[0].initial.screen_ref = false;
    spec.name = MasterSpec::default().name;
    spec
}
fn patch(track: u64, sample: u64, ramp: Option<u64>, patch: ObjectPatch) -> ObjectUpdate {
    ObjectUpdate {
        track: TrackId(track),
        sample,
        ramp_samples: ramp,
        patch,
    }
}
fn updates() -> Vec<ObjectUpdate> {
    vec![
        patch(
            6,
            1000,
            Some(2400),
            ObjectPatch {
                position: Some([1., 0.5, 0.75]),
                ..Default::default()
            },
        ),
        patch(
            7,
            1500,
            None,
            ObjectPatch {
                snap: Some(false),
                importance: Some(0.3),
                ..Default::default()
            },
        ),
        patch(
            6,
            4000,
            Some(500),
            ObjectPatch {
                gain: Some(Gain::Silence),
                ..Default::default()
            },
        ),
        patch(
            5,
            4000,
            Some(0),
            ObjectPatch {
                position: Some([0.25, 1., 0.]),
                extent: Some(Some(Extent::Uniform(0.2))),
                ..Default::default()
            },
        ),
        patch(
            7,
            6000,
            Some(1200),
            ObjectPatch {
                gain: Some(Gain::Decibels(-3.5)),
                extent: Some(None),
                ..Default::default()
            },
        ),
        patch(
            5,
            7000,
            None,
            ObjectPatch {
                active: Some(false),
                ..Default::default()
            },
        ),
    ]
}
fn pcm(first: usize, frames: usize, channels: usize) -> Vec<f32> {
    (first * channels..(first + frames) * channels)
        .map(|i| ((i * 104_729) % 4001) as f32 / 2000. - 1.)
        .collect()
}
fn write(path: &Path, spec: MasterSpec, updates: &[ObjectUpdate]) -> WriteReport {
    let channels = spec.channels();
    let mut writer = MasterWriter::create(path, spec).unwrap();
    writer.schedule_updates(updates).unwrap();
    let mut at = 0;
    for size in [777, 3001, FRAMES - 3778] {
        writer
            .write_block(PcmBlock::F32Interleaved(&pcm(at, size, channels)), &[])
            .unwrap();
        at += size;
    }
    writer.finish().unwrap()
}
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
fn to(path: PathBuf, profile: Profile) -> [Target; 1] {
    [Target { path, profile }]
}

#[test]
fn adm_reads_back_into_the_model_it_was_written_from() {
    let dir = Temp::new();
    write(&dir.path("src.wav"), spec(Profile::Standard), &updates());
    let reader = AdmReader::open(dir.path("src.wav"), LossPolicy::Reject).unwrap();
    assert!(reader.diagnostics().is_empty());
    let read = reader.spec();
    let expected = spec(Profile::Standard);
    assert_eq!(read.profile, Profile::Standard);
    assert_eq!(
        (read.name.as_str(), read.bed_name.as_str()),
        ("Feature mix", "Main bed")
    );
    // TrackIds are the file's tracks: the bed holds 1..=10, the objects follow.
    let ids: Vec<_> = read
        .objects
        .iter()
        .map(|o| (o.id.0, o.name.as_str()))
        .collect();
    assert_eq!(ids, [(11, "Dialog"), (12, "Fly-by"), (13, "Wide pad")]);
    for (a, b) in read.objects.iter().zip(&expected.objects) {
        assert_eq!(a.initial, b.initial, "{}", a.name);
    }
    let updates = reader.updates();
    let fly_by: Vec<_> = updates
        .iter()
        .filter(|u| u.track == TrackId(12))
        .map(|u| (u.sample, u.ramp_samples))
        .collect();
    // The ramp's block, then the silent gain's block; the hold after the ramp changes nothing.
    assert_eq!(fly_by, [(1000, Some(2400)), (4000, Some(500))]);
    let last = updates
        .iter()
        .find(|u| u.track == TrackId(11) && u.sample == 7000)
        .unwrap();
    // ADM has no active flag: the writer spells inactive as gain 0, which reads back as silence.
    assert_eq!(last.patch.gain, Some(Gain::Linear(0.)));
}

#[test]
fn adm_to_adm_reproduces_the_file_byte_for_byte() {
    for profile in [Profile::Standard, Profile::Logic] {
        let dir = Temp::new();
        write(&dir.path("src.wav"), spec(profile), &updates());
        let reader = AdmReader::open(dir.path("src.wav"), LossPolicy::Reject).unwrap();
        assert_eq!(reader.spec().profile, profile);
        let out = dir.path("out.wav");
        let conversion = reader
            .convert(reader.audio().unwrap(), &to(out.clone(), profile))
            .unwrap();
        assert!(conversion.outputs[0].diagnostics.is_empty());
        assert_eq!(
            fs::read(&out).unwrap(),
            fs::read(dir.path("src.wav")).unwrap(),
            "{profile:?}"
        );
    }
}

#[test]
fn adm_to_damf_and_back_returns_to_the_same_adm() {
    // Values DAMF can spell exactly: positions within nine decimals, gains of unity or silence.
    let exact: Vec<_> = updates()
        .into_iter()
        .map(|mut u| {
            if let Some(Gain::Decibels(_)) = u.patch.gain {
                u.patch.gain = Some(Gain::Silence);
            }
            u
        })
        .collect();
    for profile in [Profile::Standard, Profile::Logic] {
        let dir = Temp::new();
        write(&dir.path("src.wav"), damf_safe(profile), &exact);
        let reader = AdmReader::open(dir.path("src.wav"), LossPolicy::Reject).unwrap();
        let both = [
            Target {
                path: dir.path("home"),
                profile: Profile::DamfHome,
            },
            Target {
                path: dir.path("3dof"),
                profile: Profile::DamfThreeDof,
            },
        ];
        let conversion = reader.convert(reader.audio().unwrap(), &both).unwrap();
        assert!(conversion.outputs.iter().all(|o| o.diagnostics.is_empty()));
        let damf = DamfReader::open(dir.path("home/master.atmos"), LossPolicy::Reject).unwrap();
        let back = dir.path("back.wav");
        damf.convert(damf.audio().unwrap(), &to(back.clone(), profile))
            .unwrap();
        assert_eq!(
            fs::read(&back).unwrap(),
            fs::read(dir.path("src.wav")).unwrap(),
            "{profile:?}"
        );
        assert_eq!(
            contents(&dir.path("home"))[1..],
            contents(&dir.path("3dof"))[1..],
            "the two DAMF profiles share audio and metadata"
        );
    }
}

#[test]
fn gains_survive_the_trip_through_damf_within_its_precision() {
    let dir = Temp::new();
    write(
        &dir.path("src.wav"),
        damf_safe(Profile::Standard),
        &updates(),
    );
    let adm = AdmReader::open(dir.path("src.wav"), LossPolicy::Reject).unwrap();
    adm.convert(
        adm.audio().unwrap(),
        &to(dir.path("damf"), Profile::DamfHome),
    )
    .unwrap();
    let damf = DamfReader::open(dir.path("damf/master.atmos"), LossPolicy::Reject).unwrap();
    let from_damf: Vec<_> = damf.updates().unwrap().map(Result::unwrap).collect();
    assert_eq!(from_damf.len(), adm.updates().len());
    for (a, b) in adm.updates().iter().zip(&from_damf) {
        assert_eq!(
            (a.track.0 - 1, a.sample, a.ramp_samples),
            (b.track.0, b.sample, b.ramp_samples)
        );
        if let (Some(x), Some(y)) = (a.patch.gain, b.patch.gain) {
            assert!((x.linear() - y.linear()).abs() < 1e-9, "{x:?} vs {y:?}");
        }
    }
}

/// One object of a hand-written ADM: its track, extra attributes and elements, and blocks.
struct Object<'a> {
    track: u16,
    name: &'a str,
    attributes: &'a str,
    elements: &'a str,
    blocks: &'a [&'a str],
}
const RC_BED: [&str; 10] = [
    "RC_Rts", "RC_Lts", "RC_Rrs", "RC_Lrs", "RC_Rss", "RC_Lss", "RC_LFE", "RC_C", "RC_R", "RC_L",
];

/// A chna-only ADM BWF in the shape other tools write: one content per object, track UIDs
/// defined only in chna, the bed on tracks 1..=10 in reverse label order. Track `t` carries the
/// 24-bit value `t` in every frame.
fn third_party(path: &Path, programme: &str, objects: &[Object<'_>]) {
    let mut xml = format!(
        "<?xml version=\"1.0\"?><ebuCoreMain><coreMetadata><format><audioFormatExtended>\n\
         <audioProgramme audioProgrammeID=\"APR_1001\" audioProgrammeName=\"Other tool\"{programme}>\
         <audioContentIDRef>ACO_1001</audioContentIDRef>"
    );
    for i in 0..objects.len() {
        xml += &format!(
            "<audioContentIDRef>ACO_{:04x}</audioContentIDRef>",
            0x1002 + i
        );
    }
    xml += "</audioProgramme>\n<audioContent audioContentID=\"ACO_1001\" audioContentName=\"Bed\">\
            <audioObjectIDRef>AO_1001</audioObjectIDRef></audioContent>\n";
    for i in 0..objects.len() {
        xml += &format!(
            "<audioContent audioContentID=\"ACO_{0:04x}\" audioContentName=\"C\">\
             <audioObjectIDRef>AO_{0:04x}</audioObjectIDRef></audioContent>\n",
            0x1002 + i
        );
    }
    xml += "<audioObject audioObjectID=\"AO_1001\" audioObjectName=\"Bed\">\
            <audioPackFormatIDRef>AP_00011001</audioPackFormatIDRef>";
    for t in 1..=10 {
        xml += &format!("<audioTrackUIDRef>ATU_{t:08x}</audioTrackUIDRef>");
    }
    xml += "</audioObject>\n<audioPackFormat audioPackFormatID=\"AP_00011001\" \
            audioPackFormatName=\"Bed pack\" typeDefinition=\"DirectSpeakers\">";
    for c in 1..=10 {
        xml += &format!(
            "<audioChannelFormatIDRef>AC_0001{:04x}</audioChannelFormatIDRef>",
            0x1000 + c
        );
    }
    xml += "</audioPackFormat>\n";
    for (c, label) in RC_BED.iter().enumerate() {
        xml += &format!(
            "<audioChannelFormat audioChannelFormatID=\"AC_0001{:04x}\" audioChannelFormatName=\"{label}\" \
             typeDefinition=\"DirectSpeakers\"><audioBlockFormat audioBlockFormatID=\"AB_0001{:04x}_00000001\">\
             <speakerLabel>{label}</speakerLabel><position coordinate=\"azimuth\">0</position>\
             <position coordinate=\"elevation\">0</position></audioBlockFormat></audioChannelFormat>\n",
            0x1001 + c,
            0x1001 + c
        );
    }
    for (i, o) in objects.iter().enumerate() {
        let id = 0x1001 + i;
        xml += &format!(
            "<audioObject audioObjectID=\"AO_{:04x}\" audioObjectName=\"{}\"{}>\
             <audioPackFormatIDRef>AP_0003{id:04x}</audioPackFormatIDRef>\
             <audioTrackUIDRef>ATU_{:08x}</audioTrackUIDRef>{}</audioObject>\n\
             <audioPackFormat audioPackFormatID=\"AP_0003{id:04x}\" audioPackFormatName=\"P\" typeDefinition=\"Objects\">\
             <audioChannelFormatIDRef>AC_0003{id:04x}</audioChannelFormatIDRef></audioPackFormat>\n\
             <audioChannelFormat audioChannelFormatID=\"AC_0003{id:04x}\" audioChannelFormatName=\"{}\" typeDefinition=\"Objects\">",
            id + 1,
            o.name,
            o.attributes,
            o.track,
            o.elements,
            o.name
        );
        for (b, block) in o.blocks.iter().enumerate() {
            xml += &format!(
                "<audioBlockFormat audioBlockFormatID=\"AB_0003{id:04x}_{:08x}\" {block}</audioBlockFormat>",
                b + 1
            );
        }
        xml += "</audioChannelFormat>\n";
    }
    xml += "</audioFormatExtended></format></coreMetadata></ebuCoreMain>\n";
    let tracks = 10 + objects.len() as u16;
    let entry = |track: u16, format: String, pack: String| ChnaEntry {
        track_index: track,
        uid: format!("ATU_{track:08x}").as_bytes().try_into().unwrap(),
        track_format: format.as_bytes().try_into().unwrap(),
        pack_format: pack.as_bytes().try_into().unwrap(),
        pad: 0,
    };
    let mut entries: Vec<_> = (1..=10)
        .map(|t| {
            entry(
                t,
                format!("AC_0001{:04x}_00", 0x1000 + t),
                "AP_00011001".into(),
            )
        })
        .collect();
    for (i, o) in objects.iter().enumerate() {
        let id = 0x1001 + i;
        entries.push(entry(
            o.track,
            format!("AC_0003{id:04x}_00"),
            format!("AP_0003{id:04x}"),
        ));
    }
    let mut writer = Writer::new(
        fs::File::create(path).unwrap(),
        Format {
            channels: tracks,
            sample_rate: 48_000,
            sample_format: SampleFormat::Pcm24,
            channel_mask: None,
        },
        WriterOptions {
            container: Some(Container::Bw64),
            axml: Some(xml.into_bytes()),
            chna: Some(Chna { tracks, entries }),
        },
    )
    .unwrap();
    let mut frame = Vec::new();
    for t in 1..=i32::from(tracks) {
        frame.extend_from_slice(&t.to_le_bytes()[..3]);
    }
    writer.write_pcm_bytes(&frame.repeat(4800)).unwrap();
    writer.finish().unwrap();
}
fn channel_values(path: &Path) -> Vec<i32> {
    let mut reader = Reader::new(BufReader::new(fs::File::open(path).unwrap())).unwrap();
    let mut frame = vec![0.; usize::from(reader.info().format.channels)];
    reader.read_frames(&mut frame).unwrap();
    frame
        .iter()
        .map(|v| (f64::from(*v) * 8_388_608.).round() as i32)
        .collect()
}
const STILL: &str = r#"rtime="00:00:00.00000" duration="00:00:00.10000"><position coordinate="X">0</position><position coordinate="Y">1</position><position coordinate="Z">0</position>"#;

#[test]
fn third_party_adm_timing_gain_and_channel_order_are_read() {
    let dir = Temp::new();
    let src = dir.path("other.wav");
    third_party(
        &src,
        "",
        &[
            Object {
                track: 12,
                name: "Late, short and quiet",
                // Starts 1200 samples in and lasts 2400; its gain folds into every block.
                attributes: r#" start="00:00:00.02500" duration="00:00:00.05000""#,
                elements: r#"<gain gainUnit="dB">-6</gain>"#,
                blocks: &[
                    r#"rtime="00:00:00.00000" duration="00:00:00.01000"><position coordinate="X">-1</position><position coordinate="Y">1</position><position coordinate="Z">0</position><gain>0.5</gain>"#,
                    // No jumpPosition: interpolate across the whole 960-sample block.
                    r#"rtime="00:00:00.01000" duration="00:00:00.02000"><position coordinate="X">1</position><position coordinate="Y">1</position><position coordinate="Z">0</position><gain>0.5</gain><width>0.3</width><depth>0.3</depth><height>0.3</height>"#,
                    r#"rtime="00:00:00.03000" duration="00:00:00.02000"><position coordinate="X">1</position><position coordinate="Y">1</position><position coordinate="Z">0</position><gain gainUnit="dB">-12</gain><jumpPosition interpolationLength="0.005">1</jumpPosition><importance>4</importance>"#,
                ],
            },
            Object {
                track: 11,
                name: "Muted",
                attributes: "",
                elements: "<mute>1</mute>",
                blocks: &[STILL],
            },
        ],
    );
    let reader = AdmReader::open(&src, LossPolicy::Reject).unwrap();
    let spec = reader.spec();
    assert_eq!(
        (
            spec.name.as_str(),
            spec.bed_name.as_str(),
            spec.bed_pack_name.as_str()
        ),
        ("Other tool", "Bed", "Bed pack")
    );
    let objects: Vec<_> = spec
        .objects
        .iter()
        .map(|o| (o.id.0, o.name.as_str()))
        .collect();
    assert_eq!(
        objects,
        [(11, "Muted"), (12, "Late, short and quiet")],
        "objects follow their tracks"
    );
    assert_eq!(spec.objects[0].initial.gain, Gain::Silence);
    let late = &spec.objects[1].initial;
    assert!(!late.active, "inactive before its start");
    assert!((late.gain.linear() - 0.5 * libm::pow(10., -6. / 20.)).abs() < 1e-12);
    let timeline: Vec<_> = reader
        .updates()
        .iter()
        .map(|u| (u.track.0, u.sample, u.ramp_samples, u.patch.active))
        .collect();
    assert_eq!(
        timeline,
        [
            (12, 1200, None, Some(true)),
            (12, 1680, Some(960), None),
            (12, 2640, Some(240), None),
            (12, 3600, None, Some(false)),
        ]
    );
    let ramp = &reader.updates()[1].patch;
    assert_eq!(ramp.position, Some([1., 1., 0.]));
    assert_eq!(ramp.extent, Some(Some(Extent::Uniform(0.3))));
    let jump = &reader.updates()[2].patch;
    assert!((jump.gain.unwrap().linear() - libm::pow(10., -18. / 20.)).abs() < 1e-12);
    assert_eq!(jump.importance, Some(0.4));

    let out = dir.path("out.wav");
    reader
        .convert(reader.audio().unwrap(), &to(out.clone(), Profile::Standard))
        .unwrap();
    // L R C LFE Lss Rss Lrs Rrs Lts Rts from the reversed bed, then tracks 11 and 12.
    assert_eq!(
        channel_values(&out),
        [10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 11, 12]
    );
}

#[test]
fn what_the_model_cannot_hold_follows_the_loss_policy() {
    let block = |extra: &str| format!("{STILL}{extra}");
    for (programme, attributes, elements, block, field) in [
        (
            "",
            "",
            "",
            block("<zoneExclusion><zone>Left</zone></zoneExclusion>"),
            "unknown_field",
        ),
        ("", "", "", block("<diffuse>0.5</diffuse>"), "diffuse"),
        (
            "",
            "",
            "",
            block("<headLocked>1</headLocked>"),
            "head_tracking",
        ),
        (
            "",
            "",
            "",
            block("<objectDivergence>0.2</objectDivergence>"),
            "divergence",
        ),
        (
            "",
            "",
            "",
            block(r#"<channelLock maxDistance="0.5">1</channelLock>"#),
            "channel_lock_distance",
        ),
        ("", r#" dialogue="1""#, "", block(""), "dialog"),
        (
            "",
            "",
            r#"<positionOffset coordinate="X">0.1</positionOffset>"#,
            block(""),
            "position_offset",
        ),
        ("", r#" interact="1""#, "", block(""), "unknown_field"),
        (
            r#" audioProgrammeLanguage="en""#,
            "",
            "",
            block(""),
            "language",
        ),
    ] {
        let dir = Temp::new();
        let blocks = [block.as_str()];
        third_party(
            &dir.path("x.wav"),
            programme,
            &[Object {
                track: 11,
                name: "O",
                attributes,
                elements,
                blocks: &blocks,
            }],
        );
        let error = AdmReader::open(dir.path("x.wav"), LossPolicy::Reject).unwrap_err();
        assert_eq!(
            (error.kind, error.field),
            (ErrorKind::Unsupported, Some(field)),
            "{block}: {error}"
        );
        let reader = AdmReader::open(dir.path("x.wav"), LossPolicy::Allow).unwrap();
        let fields: Vec<_> = reader.diagnostics().iter().map(|d| d.field).collect();
        assert!(fields.contains(&field), "{block}: {fields:?}");
    }
}

#[test]
fn structures_outside_the_atmos_subset_are_refused() {
    let polar = r#"rtime="00:00:00.00000" duration="00:00:00.10000"><position coordinate="azimuth">30</position><position coordinate="elevation">0</position>"#;
    let first = r#"rtime="00:00:00.00000" duration="00:00:00.04000"><position coordinate="X">0</position><position coordinate="Y">1</position><position coordinate="Z">0</position>"#;
    let gap = r#"rtime="00:00:00.05000" duration="00:00:00.05000"><position coordinate="X">0</position><position coordinate="Y">1</position><position coordinate="Z">0</position>"#;
    for (blocks, text) in [(vec![polar], "polar"), (vec![first, gap], "contiguous")] {
        let dir = Temp::new();
        third_party(
            &dir.path("x.wav"),
            "",
            &[Object {
                track: 11,
                name: "O",
                attributes: "",
                elements: "",
                blocks: &blocks,
            }],
        );
        let error = AdmReader::open(dir.path("x.wav"), LossPolicy::Allow).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Unsupported, "{error}");
        assert!(error.message.contains(text), "{error}");
    }
    // Our own writer's file, with the graph changed in ways the subset does not cover.
    let dir = Temp::new();
    write(&dir.path("src.wav"), spec(Profile::Standard), &updates());
    let mut chunks =
        macin_bwf::ChunkReader::new(fs::File::open(dir.path("src.wav")).unwrap()).unwrap();
    let xml = String::from_utf8(chunks.metadata(*b"axml", 1 << 20).unwrap().unwrap()).unwrap();
    let chna = Chna::decode(&chunks.metadata(*b"chna", 1 << 20).unwrap().unwrap()).unwrap();
    let mut data = vec![
        0;
        usize::try_from(
            chunks
                .chunks()
                .iter()
                .find(|c| &c.id == b"data")
                .unwrap()
                .size
        )
        .unwrap()
    ];
    chunks.read_chunk(*b"data", &mut data).unwrap();
    let rewrite = |name: &str, xml: String| -> PathBuf {
        let path = dir.path(name);
        let mut writer = Writer::new(
            fs::File::create(&path).unwrap(),
            Format {
                channels: 13,
                sample_rate: 48_000,
                sample_format: SampleFormat::Pcm24,
                channel_mask: None,
            },
            WriterOptions {
                container: Some(Container::Bw64),
                axml: Some(xml.into_bytes()),
                chna: Some(chna.clone()),
            },
        )
        .unwrap();
        writer.write_pcm_bytes(&data).unwrap();
        writer.finish().unwrap();
        path
    };
    let second_programme = xml.replacen(
        "<audioContent ",
        "<audioProgramme audioProgrammeID=\"APR_1002\" audioProgrammeName=\"Alt\"><audioContentIDRef>ACO_1001</audioContentIDRef></audioProgramme>\n<audioContent ",
        1,
    );
    let nested = xml.replacen(
        "<audioPackFormatIDRef>AP_00031001</audioPackFormatIDRef>",
        "<audioPackFormatIDRef>AP_00031001</audioPackFormatIDRef><audioObjectIDRef>AO_100d</audioObjectIDRef>",
        1,
    );
    let five_one = xml.replacen(
        "<speakerLabel>RC_Lts</speakerLabel>",
        "<speakerLabel>RC_Lfh</speakerLabel>",
        1,
    );
    for (name, xml, text) in [
        ("programmes.wav", second_programme, "single-programme"),
        ("nested.wav", nested, "nested"),
        ("bed.wav", five_one, "7.1.2"),
    ] {
        let error = AdmReader::open(rewrite(name, xml), LossPolicy::Allow).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Unsupported, "{name}: {error}");
        assert!(error.message.contains(text), "{name}: {error}");
    }
    let plain = dir.path("plain.wav");
    let mut writer = Writer::new(
        fs::File::create(&plain).unwrap(),
        Format {
            channels: 2,
            sample_rate: 48_000,
            sample_format: SampleFormat::Pcm24,
            channel_mask: None,
        },
        WriterOptions::default(),
    )
    .unwrap();
    writer.write_pcm_bytes(&[0; 6]).unwrap();
    writer.finish().unwrap();
    let error = AdmReader::open(&plain, LossPolicy::Allow).unwrap_err();
    assert!(error.message.contains("no axml"), "{error}");
}

#[test]
fn conversion_rewinds_and_remaps_audio_it_did_not_open() {
    let dir = Temp::new();
    write(
        &dir.path("src.wav"),
        damf_safe(Profile::Standard),
        &updates(),
    );
    let reader = AdmReader::open(dir.path("src.wav"), LossPolicy::Reject).unwrap();
    let fresh = dir.path("fresh.wav");
    reader
        .convert(
            reader.audio().unwrap(),
            &to(fresh.clone(), Profile::Standard),
        )
        .unwrap();

    let mut previewed = reader.audio().unwrap();
    let mut block = Vec::new();
    previewed.read_s24(1234, &mut block).unwrap();
    let after_preview = dir.path("previewed.wav");
    reader
        .convert(previewed, &to(after_preview.clone(), Profile::Standard))
        .unwrap();
    assert_eq!(fs::read(&after_preview).unwrap(), fs::read(&fresh).unwrap());

    // Audio opened by a DAMF reader of the same master is remapped to this reader's channels.
    reader
        .convert(
            reader.audio().unwrap(),
            &to(dir.path("damf"), Profile::DamfHome),
        )
        .unwrap();
    let damf = DamfReader::open(dir.path("damf/master.atmos"), LossPolicy::Reject).unwrap();
    let foreign = dir.path("foreign.wav");
    reader
        .convert(
            damf.audio().unwrap(),
            &to(foreign.clone(), Profile::Standard),
        )
        .unwrap();
    assert_eq!(fs::read(&foreign).unwrap(), fs::read(&fresh).unwrap());
}
