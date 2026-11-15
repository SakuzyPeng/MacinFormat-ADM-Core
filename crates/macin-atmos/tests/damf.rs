#![cfg(feature = "read")]
use macin_atmos::damf::{CafReader, DamfReader};
use macin_atmos::*;
use macin_bwf::{Format, Reader, SampleFormat, Writer, WriterOptions};
use std::{
    fs,
    io::{BufReader, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "macin-atmos-damf-{}-{}",
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

fn spec(profile: Profile, rate: u32) -> MasterSpec {
    let mut spec = MasterSpec {
        profile,
        sample_rate: rate,
        loss_policy: LossPolicy::Allow,
        frame_rate: FrameRate::Fps25,
        stem: "film".into(),
        objects: vec![
            ObjectSpec::new(TrackId(10), "Dialog"),
            ObjectSpec::new(TrackId(11), "Fly-by"),
            ObjectSpec::new(TrackId(12), "Wide pad"),
        ],
        ..Default::default()
    };
    let o = &mut spec.objects;
    o[0].initial.dialog = 1;
    o[0].initial.position = [0., 1., 0.];
    o[1].initial.position = [-1., 0.5, 0.25];
    o[1].initial.gain = Gain::Decibels(-6.);
    o[1].initial.head_tracking = HeadTracking::HeadRelative;
    o[2].initial.extent = Some(Extent::Cartesian([0.5, 0.25, 0.125]));
    o[2].initial.zone = Zone::NoBack;
    o[2].initial.decorrelation = 0.5;
    o[2].initial.binaural_mode = BinauralMode::Far;
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
            11,
            1000,
            Some(2400),
            ObjectPatch {
                position: Some([1., 0.5, 0.75]),
                ..Default::default()
            },
        ),
        patch(
            12,
            1500,
            None,
            ObjectPatch {
                binaural_mode: Some(BinauralMode::Near),
                trim_bypass: Some(true),
                ..Default::default()
            },
        ),
        patch(
            11,
            2000,
            Some(500),
            ObjectPatch {
                gain: Some(Gain::Silence),
                ..Default::default()
            },
        ),
        patch(
            10,
            4000,
            Some(0),
            ObjectPatch {
                position: Some([0.25, 1., 0.]),
                extent: Some(Some(Extent::Uniform(0.2))),
                ..Default::default()
            },
        ),
        patch(
            12,
            6000,
            Some(1200),
            ObjectPatch {
                gain: Some(Gain::Decibels(-3.5)),
                extent: Some(None),
                ..Default::default()
            },
        ),
        patch(
            10,
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
/// Write `spec` directly, in uneven blocks, with `updates` scheduled up front.
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
fn targets(dir: &Temp, profiles: &[Profile]) -> Vec<Target> {
    profiles
        .iter()
        .map(|&profile| Target {
            path: dir.path(&format!("out-{profile:?}")),
            profile,
        })
        .collect()
}

#[test]
fn damf_reads_back_into_the_model_it_was_written_from() {
    for (profile, rate) in [(Profile::DamfHome, 48_000), (Profile::DamfThreeDof, 96_000)] {
        let dir = Temp::new();
        write(&dir.path("src"), spec(profile, rate), &updates());
        let reader = DamfReader::open(dir.path("src/film.atmos"), LossPolicy::Reject).unwrap();
        let read = reader.spec();
        let expected = spec(profile, rate);
        assert_eq!(read.profile, profile);
        assert_eq!(read.sample_rate, rate);
        assert_eq!(read.frame_rate, FrameRate::Fps25);
        assert_eq!(read.stem, "film");
        assert_eq!(read.objects.len(), 3);
        for (a, b) in read.objects.iter().zip(&expected.objects) {
            assert_eq!((a.id, &a.name), (b.id, &b.name));
            assert_eq!(a.initial.position, b.initial.position, "{}", a.name);
            assert_eq!(a.initial.extent, b.initial.extent, "{}", a.name);
            assert_eq!(
                a.initial.gain.linear(),
                b.initial.gain.linear(),
                "{}",
                a.name
            );
            assert_eq!(
                (
                    a.initial.zone,
                    a.initial.dialog,
                    a.initial.head_tracking,
                    a.initial.binaural_mode
                ),
                (
                    b.initial.zone,
                    b.initial.dialog,
                    b.initial.head_tracking,
                    b.initial.binaural_mode
                )
            );
        }
        assert!(reader.diagnostics().is_empty());
        assert_eq!(reader.creation_tool(), Some("MacinFormat-ADM-Core"));
        assert_eq!(reader.last_event_sample(), Some(7000));
        let read: Vec<_> = reader.updates().unwrap().map(Result::unwrap).collect();
        let mut written = updates();
        // The file is per object, so the order is per object too.
        written.sort_by_key(|u| u.track);
        assert_eq!(read.len(), written.len());
        assert_eq!(reader.update_count(), written.len() as u64);
        for (a, b) in read.iter().zip(&written) {
            assert_eq!(
                (a.track, a.sample, a.ramp_samples),
                (b.track, b.sample, b.ramp_samples)
            );
            let gain = |p: &ObjectPatch| p.gain.map(Gain::linear);
            assert_eq!(gain(&a.patch), gain(&b.patch));
            assert_eq!(
                ObjectPatch {
                    gain: None,
                    ..a.patch
                },
                ObjectPatch {
                    gain: None,
                    ..b.patch
                }
            );
        }
    }
}

#[test]
fn damf_to_damf_reproduces_every_member_byte_for_byte() {
    for profile in [Profile::DamfHome, Profile::DamfThreeDof] {
        let dir = Temp::new();
        write(&dir.path("src"), spec(profile, 48_000), &updates());
        let reader = DamfReader::open(dir.path("src/film.atmos"), LossPolicy::Reject).unwrap();
        let out = targets(&dir, &[profile]);
        let conversion = reader.convert(reader.audio().unwrap(), &out).unwrap();
        assert!(conversion.read.is_empty());
        assert_eq!(conversion.outputs[0].frames, FRAMES as u64);
        assert_eq!(
            contents(&out[0].path),
            contents(&dir.path("src")),
            "{profile:?}"
        );
    }
}

#[test]
fn damf_to_adm_matches_writing_the_model_as_adm_directly() {
    let dir = Temp::new();
    write(
        &dir.path("src"),
        spec(Profile::DamfHome, 48_000),
        &updates(),
    );
    let reader = DamfReader::open(dir.path("src/film.atmos"), LossPolicy::Allow).unwrap();
    let out = targets(
        &dir,
        &[Profile::Standard, Profile::Logic, Profile::DamfThreeDof],
    );
    let conversion = reader.convert(reader.audio().unwrap(), &out).unwrap();
    for (target, report) in out.iter().zip(&conversion.outputs) {
        let direct = dir.path(&format!("direct-{:?}", target.profile));
        let expected = write(
            &direct,
            MasterSpec {
                profile: target.profile,
                ..spec(Profile::DamfHome, 48_000)
            },
            &updates(),
        );
        assert_eq!(
            contents(&target.path),
            contents(&direct),
            "{:?}",
            target.profile
        );
        assert_eq!(
            report.diagnostics, expected.diagnostics,
            "{:?}",
            target.profile
        );
    }
    // Headphone policy, zones, trim and dialog have no ADM mapping; under Allow they are reported.
    let fields: Vec<_> = conversion.outputs[0]
        .diagnostics
        .iter()
        .map(|d| d.field)
        .collect();
    for field in [
        "binaural_mode",
        "head_tracking",
        "zone",
        "trim_bypass",
        "dialog",
        "decorrelation",
    ] {
        assert!(fields.contains(&field), "{field} in {fields:?}");
    }
    let error = DamfReader::open(dir.path("src/film.atmos"), LossPolicy::Reject)
        .unwrap()
        .convert(
            reader.audio().unwrap(),
            &[Target {
                path: dir.path("strict.wav"),
                profile: Profile::Standard,
            }],
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unsupported);
    assert!(!dir.path("strict.wav").exists());
}

#[test]
fn a_wav_can_stand_in_for_the_damf_audio() {
    let dir = Temp::new();
    write(
        &dir.path("src"),
        spec(Profile::DamfHome, 48_000),
        &updates(),
    );
    let reader = DamfReader::open(dir.path("src/film.atmos"), LossPolicy::Allow).unwrap();
    // The same 13 channels as 24-bit PCM: the WAV path must give identical bytes.
    let wav = dir.path("audio.wav");
    let mut writer = Writer::new(
        fs::File::create(&wav).unwrap(),
        Format {
            channels: 13,
            sample_rate: 48_000,
            sample_format: SampleFormat::Pcm24,
            channel_mask: None,
        },
        WriterOptions::default(),
    )
    .unwrap();
    let mut source = reader.audio().unwrap();
    let mut block = Vec::new();
    while source.read_s24(1000, &mut block).unwrap() > 0 {
        writer.write_pcm_bytes(&block).unwrap();
    }
    writer.finish().unwrap().flush().unwrap();
    let from_caf = targets(&dir, &[Profile::Standard]);
    reader.convert(reader.audio().unwrap(), &from_caf).unwrap();
    let from_wav = dir.path("from-wav.wav");
    reader
        .convert(
            reader.wave_audio(&wav).unwrap(),
            &[Target {
                path: from_wav.clone(),
                profile: Profile::Standard,
            }],
        )
        .unwrap();
    assert_eq!(
        fs::read(&from_wav).unwrap(),
        fs::read(&from_caf[0].path).unwrap()
    );

    let short = dir.path("short.wav");
    let mut writer = Writer::new(
        fs::File::create(&short).unwrap(),
        Format {
            channels: 12,
            sample_rate: 48_000,
            sample_format: SampleFormat::Pcm16,
            channel_mask: None,
        },
        WriterOptions::default(),
    )
    .unwrap();
    writer.write_pcm_bytes(&[0; 12 * 2 * 10]).unwrap();
    writer.finish().unwrap().flush().unwrap();
    let error = reader.wave_audio(&short).unwrap_err();
    assert!(error.message.contains("ID 12"), "{error}");
}

/// A CAF whose channel `c` holds the 24-bit value `c + 1` in every frame.
fn tagged_caf(path: &Path, channels: usize, frames: usize) {
    let mut out = Vec::new();
    containers::write_caf_header(&mut out, Some(frames as u64), channels, 48_000).unwrap();
    for _ in 0..frames {
        for c in 0..channels {
            out.extend_from_slice(&(c as i32 + 1).to_le_bytes()[..3]);
        }
    }
    fs::write(path, out).unwrap();
}
fn channel_values(path: &Path) -> Vec<f32> {
    let mut reader = Reader::new(BufReader::new(fs::File::open(path).unwrap())).unwrap();
    let channels = usize::from(reader.info().format.channels);
    let mut frame = vec![0.; channels];
    reader.read_frames(&mut frame).unwrap();
    frame
}

const THIRD_PARTY_MANIFEST: &str = r#"# Written by some other tool.
version: "0.5"
presentations:
- type: home
  audio: 'mix.atmos.audio'
  metadata: "mix.atmos.metadata"
  fps: 24
  offset: 0.0
  objects:
  - {ID: 13, description: "Second object"}
  - {ID: 0, description: 'First object'}
  bedInstances:
  - description: Main bed
    channels:
    - {channel: Rts, ID: 1}
    - {channel: Lts, ID: 2}
    - {channel: Rrs, ID: 3}
    - {channel: Lrs, ID: 4}
    - {channel: Rss, ID: 5}
    - {channel: Lss, ID: 6}
    - {channel: LFE, ID: 7}
    - {channel: C, ID: 8}
    - {channel: R, ID: 9}
    - {channel: L, ID: 10}
"#;

fn third_party(dir: &Temp, metadata: &str) -> PathBuf {
    fs::write(dir.path("mix.atmos"), THIRD_PARTY_MANIFEST).unwrap();
    fs::write(dir.path("mix.atmos.metadata"), metadata).unwrap();
    tagged_caf(&dir.path("mix.atmos.audio"), 14, 4800);
    dir.path("mix.atmos")
}
const THIRD_PARTY_METADATA: &str = "events:
  # Events interleaved in time across objects, as a renderer writes them.
  - {ID: 13, samplePos: 0, pos: [0.5, -0.5, 0], gain: -.inf}
  - ID: 0
    samplePos: 0
    pos: [ -1, 1, 0 ]
    size3D: [0.1, 0.2, 0.3]
  - {ID: 13, samplePos: 1200, gain: 0, pos: [0.5, 0.5, 0], rampLength: 480}
  - {ID: 0, samplePos: 2400, pos: [1, 1, 0]}
  - {ID: 7, samplePos: 0, binauralRenderMode: 'off', gain: 0}
sampleRate: 48000
";

#[test]
fn third_party_yaml_layouts_and_channel_order_are_read() {
    let dir = Temp::new();
    let manifest = third_party(&dir, THIRD_PARTY_METADATA);
    let reader = DamfReader::open(&manifest, LossPolicy::Reject).unwrap();
    let spec = reader.spec();
    assert_eq!(spec.stem, "mix");
    assert_eq!(spec.bed_name, "Main bed");
    let names: Vec<_> = spec
        .objects
        .iter()
        .map(|o| (o.id.0, o.name.as_str()))
        .collect();
    assert_eq!(names, [(0, "First object"), (13, "Second object")]);
    assert_eq!(
        spec.objects[0].initial.extent,
        Some(Extent::Cartesian([0.1, 0.2, 0.3]))
    );
    assert_eq!(spec.objects[1].initial.gain, Gain::Silence);
    let updates: Vec<_> = reader.updates().unwrap().map(Result::unwrap).collect();
    assert_eq!(
        updates
            .iter()
            .map(|u| (u.track.0, u.sample, u.ramp_samples))
            .collect::<Vec<_>>(),
        [(13, 1200, Some(480)), (0, 2400, Some(0))],
        "a continuous event without rampLength ramps over zero samples"
    );
    let out = dir.path("out.wav");
    reader
        .convert(
            reader.audio().unwrap(),
            &[Target {
                path: out.clone(),
                profile: Profile::Standard,
            }],
        )
        .unwrap();
    // Output order is L R C LFE Lss Rss Lrs Rrs Lts Rts, then objects by ID; the CAF holds
    // channel c as the value c + 1.
    let ids = [10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 13];
    let expected: Vec<f32> = ids.iter().map(|id| (id + 1) as f32 / 8_388_608.).collect();
    assert_eq!(channel_values(&out), expected);
}

#[test]
fn an_objects_only_master_gets_a_silent_bed() {
    let dir = Temp::new();
    fs::write(
        dir.path("o.atmos"),
        "version: 0.5.1\npresentations:\n  - type: home\n    metadata: o.atmos.metadata\n    audio: o.atmos.audio\n    objects:\n      - {ID: 0, description: Solo}\n",
    )
    .unwrap();
    fs::write(
        dir.path("o.atmos.metadata"),
        "sampleRate: 48000\nevents:\n  - {ID: 0, samplePos: 0, pos: [0, 1, 0]}\n",
    )
    .unwrap();
    tagged_caf(&dir.path("o.atmos.audio"), 1, 480);
    let reader = DamfReader::open(dir.path("o.atmos"), LossPolicy::Reject).unwrap();
    let out = dir.path("o.wav");
    reader
        .convert(
            reader.audio().unwrap(),
            &[Target {
                path: out.clone(),
                profile: Profile::Standard,
            }],
        )
        .unwrap();
    let mut expected = vec![0.; 10];
    expected.push(1. / 8_388_608.);
    assert_eq!(channel_values(&out), expected);
}

fn open_with(metadata: &str, policy: LossPolicy) -> Result<DamfReader> {
    let dir = Temp::new();
    let manifest = third_party(&dir, metadata);
    DamfReader::open(manifest, policy)
}
fn rejected(metadata: &str) -> Error {
    open_with(metadata, LossPolicy::Reject).unwrap_err()
}

#[test]
fn what_the_model_cannot_hold_follows_the_loss_policy() {
    for (event, field) in [
        (
            "{ID: 7, samplePos: 0, binauralRenderMode: near}",
            "bed_binaural_mode",
        ),
        ("{ID: 10, samplePos: 0, gain: -3}", "bed_gain"),
        (
            "{ID: 13, samplePos: 0, objectClass: dialog}",
            "unknown_field",
        ),
        (
            "{ID: 13, samplePos: 0, pos: [0, 0, 0]}\n  - {ID: 13, samplePos: 10, snap: true, rampLength: 5}",
            "ramp",
        ),
        ("{ID: 13, samplePos: 100, pos: [0, 0, 0]}", "initial_state"),
    ] {
        let metadata =
            format!("sampleRate: 48000\nevents:\n  - {{ID: 0, samplePos: 0}}\n  - {event}\n");
        let error = rejected(&metadata);
        assert_eq!(
            (error.kind, error.field),
            (ErrorKind::Unsupported, Some(field)),
            "{event}: {error}"
        );
        let reader = open_with(&metadata, LossPolicy::Allow).unwrap();
        let fields: Vec<_> = reader.diagnostics().iter().map(|d| d.field).collect();
        assert!(fields.contains(&field), "{event}: {fields:?}");
    }
    // An object with no events at all.
    let error = rejected("sampleRate: 48000\nevents:\n  - {ID: 0, samplePos: 0}\n");
    assert_eq!(error.field, Some("initial_state"));
}

#[test]
fn malformed_or_unsupported_input_is_refused() {
    let base =
        "sampleRate: 48000\nevents:\n  - {ID: 0, samplePos: 0}\n  - {ID: 13, samplePos: 0}\n";
    for (metadata, kind, text) in [
        (
            format!("{base}  - {{ID: 13, samplePos: 50}}\n  - {{ID: 13, samplePos: 40}}\n"),
            ErrorKind::InvalidInput,
            "backwards",
        ),
        (
            format!("{base}  - {{ID: 99, samplePos: 0}}\n"),
            ErrorKind::InvalidInput,
            "does not declare",
        ),
        (
            format!("{base}  - {{ID: 13, samplePos: 9, pos: [2, 0, 0]}}\n"),
            ErrorKind::InvalidInput,
            "within -1..=1",
        ),
        (
            format!("{base}  - &a {{ID: 13, samplePos: 9}}\n"),
            ErrorKind::Unsupported,
            "anchors",
        ),
        (
            format!("{base}  - {{ID: 13, samplePos: 9, gain: '-3'}}\n"),
            ErrorKind::InvalidInput,
            "plain scalar",
        ),
        (
            format!("{base}  - {{ID: 13, ID: 13, samplePos: 9}}\n"),
            ErrorKind::InvalidInput,
            "duplicate",
        ),
        (
            format!("{base}---\nevents: []\n"),
            ErrorKind::Unsupported,
            "single YAML document",
        ),
        (
            "events: []\n".to_owned(),
            ErrorKind::InvalidInput,
            "sampleRate",
        ),
        (
            base.replace("48000", "44100"),
            ErrorKind::InvalidInput,
            "48000 or 96000",
        ),
    ] {
        let error = rejected(&metadata);
        assert_eq!(error.kind, kind, "{metadata}: {error}");
        assert!(error.message.contains(text), "{metadata}: {error}");
    }

    let dir = Temp::new();
    third_party(&dir, base);
    for (from, to, text) in [
        (
            "presentations:\n",
            "presentations:\n- {type: home, metadata: x, audio: y}\n",
            "single-presentation",
        ),
        (
            "  bedInstances:\n",
            "  bedInstances:\n  - {description: Extra, channels: []}\n",
            "multi-bed",
        ),
        (
            "- {channel: L, ID: 10}",
            "- {channel: Ls, ID: 10}",
            "not part of a 7.1.2 bed",
        ),
        ("version: \"0.5\"", "version: 0.7.0", "not supported"),
    ] {
        fs::write(
            dir.path("mix.atmos"),
            THIRD_PARTY_MANIFEST.replacen(from, to, 1),
        )
        .unwrap();
        let error = DamfReader::open(dir.path("mix.atmos"), LossPolicy::Allow).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Unsupported, "{to}: {error}");
        assert!(error.message.contains(text), "{to}: {error}");
    }

    fs::write(dir.path("mix.atmos"), THIRD_PARTY_MANIFEST).unwrap();
    let late = format!("{base}  - {{ID: 13, samplePos: 4800}}\n");
    fs::write(dir.path("mix.atmos.metadata"), late).unwrap();
    let reader = DamfReader::open(dir.path("mix.atmos"), LossPolicy::Reject).unwrap();
    let error = reader
        .convert(
            reader.audio().unwrap(),
            &[Target {
                path: dir.path("late.wav"),
                profile: Profile::Standard,
            }],
        )
        .unwrap_err();
    assert!(error.message.contains("4800 frames"), "{error}");

    let mut float = fs::read(dir.path("mix.atmos.audio")).unwrap();
    float[32..36].copy_from_slice(&3u32.to_be_bytes());
    fs::write(dir.path("mix.atmos.audio"), float).unwrap();
    let error = reader.audio().unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unsupported, "{error}");
}

fn copy_caf_to_wave(caf: &Path, wave: &Path) {
    let mut source = CafReader::open(caf).unwrap();
    let mut writer = Writer::new(
        fs::File::create(wave).unwrap(),
        Format {
            channels: u16::try_from(source.channels()).unwrap(),
            sample_rate: source.sample_rate(),
            sample_format: SampleFormat::Pcm24,
            channel_mask: None,
        },
        WriterOptions::default(),
    )
    .unwrap();
    let mut pcm = Vec::new();
    while source.read(1000, &mut pcm).unwrap() > 0 {
        writer.write_pcm_bytes(&pcm).unwrap();
    }
    writer.finish().unwrap().flush().unwrap();
}

#[test]
fn conversion_rewinds_previewed_caf_and_wave_audio() {
    let dir = Temp::new();
    let manifest = third_party(&dir, THIRD_PARTY_METADATA);
    let reader = DamfReader::open(manifest, LossPolicy::Reject).unwrap();
    let wave = dir.path("source.wav");
    copy_caf_to_wave(reader.audio_path(), &wave);
    let direct = targets(&dir, &[Profile::Standard]);
    reader.convert(reader.audio().unwrap(), &direct).unwrap();
    let expected = fs::read(&direct[0].path).unwrap();

    for preview in [0, 100, 4800] {
        let sources = [reader.audio().unwrap(), reader.wave_audio(&wave).unwrap()];
        for (index, mut audio) in sources.into_iter().enumerate() {
            assert_eq!(audio.read_s24(preview, &mut Vec::new()).unwrap(), preview);
            let target = Target {
                path: dir.path(&format!("preview-{preview}-{index}.wav")),
                profile: Profile::Standard,
            };
            let conversion = reader
                .convert(audio, std::slice::from_ref(&target))
                .unwrap();
            assert_eq!(conversion.outputs[0].frames, 4800);
            assert_eq!(fs::read(&target.path).unwrap(), expected);
        }
    }
}

#[test]
fn conversion_checks_a_foreign_audio_sources_sample_rate() {
    let dir = Temp::new();
    let manifest = third_party(&dir, THIRD_PARTY_METADATA);
    let reader = DamfReader::open(manifest, LossPolicy::Reject).unwrap();
    let other = Temp::new();
    let manifest = third_party(&other, &THIRD_PARTY_METADATA.replace("48000", "96000"));
    let mut caf = fs::read(other.path("mix.atmos.audio")).unwrap();
    caf[20..28].copy_from_slice(&96_000f64.to_be_bytes());
    fs::write(other.path("mix.atmos.audio"), caf).unwrap();
    let foreign = DamfReader::open(manifest, LossPolicy::Reject).unwrap();
    let wave = other.path("source.wav");
    copy_caf_to_wave(foreign.audio_path(), &wave);

    for audio in [foreign.audio().unwrap(), foreign.wave_audio(&wave).unwrap()] {
        let out = targets(&dir, &[Profile::Standard]);
        let error = reader.convert(audio, &out).unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidInput);
        assert!(
            error.message.contains("96000 Hz") && error.message.contains("48000 Hz"),
            "{error}"
        );
        assert!(!out[0].path.exists());
    }
}

#[test]
fn conversion_maps_foreign_audio_to_the_destination_readers_channels() {
    let dir = Temp::new();
    let manifest = third_party(&dir, THIRD_PARTY_METADATA);
    let reader = DamfReader::open(manifest, LossPolicy::Reject).unwrap();
    let direct = targets(&dir, &[Profile::Standard]);
    reader.convert(reader.audio().unwrap(), &direct).unwrap();

    let other = Temp::new();
    let manifest = third_party(&other, THIRD_PARTY_METADATA);
    fs::write(
        &manifest,
        THIRD_PARTY_MANIFEST
            .replace("{channel: L, ID: 10}", "{channel: L, ID: 9}")
            .replace("{channel: R, ID: 9}", "{channel: R, ID: 10}"),
    )
    .unwrap();
    let foreign = DamfReader::open(manifest, LossPolicy::Reject).unwrap();
    let wave = other.path("source.wav");
    copy_caf_to_wave(foreign.audio_path(), &wave);
    let sources = [foreign.audio().unwrap(), foreign.wave_audio(&wave).unwrap()];
    for (index, audio) in sources.into_iter().enumerate() {
        let target = Target {
            path: dir.path(&format!("remapped-{index}.wav")),
            profile: Profile::Standard,
        };
        reader
            .convert(audio, std::slice::from_ref(&target))
            .unwrap();
        assert_eq!(
            fs::read(&target.path).unwrap(),
            fs::read(&direct[0].path).unwrap()
        );
    }
}

#[test]
fn malformed_caf_channel_counts_return_structured_errors() {
    let dir = Temp::new();
    let path = dir.path("bad.caf");
    let mut header = Vec::new();
    containers::write_caf_header(&mut header, Some(0), 1, 48_000).unwrap();
    for (channels, bytes_per_packet) in [
        (0u32, 0u32),
        (14, 41),
        (u32::MAX, u32::MAX.wrapping_mul(3)),
        (u32::MAX / 3 + 1, 2),
    ] {
        header[36..40].copy_from_slice(&bytes_per_packet.to_be_bytes());
        header[44..48].copy_from_slice(&channels.to_be_bytes());
        fs::write(&path, &header).unwrap();
        let error = CafReader::open(&path).unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidInput);
        assert!(
            error.message.contains("invalid CAF channel layout"),
            "{error}"
        );
    }
}

#[test]
fn unknown_bed_channel_fields_follow_the_loss_policy() {
    let dir = Temp::new();
    let manifest = third_party(&dir, THIRD_PARTY_METADATA);
    fs::write(
        &manifest,
        THIRD_PARTY_MANIFEST.replace(
            "{channel: L, ID: 10}",
            "{channel: L, ID: 10, customGain: -6}",
        ),
    )
    .unwrap();
    let error = DamfReader::open(&manifest, LossPolicy::Reject).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unsupported);
    assert_eq!(error.field, Some("unknown_field"));
    assert_eq!(error.track, Some(TrackId(10)));
    assert_eq!(error.sample, None);
    assert!(error.message.contains("customGain"), "{error}");

    let reader = DamfReader::open(&manifest, LossPolicy::Allow).unwrap();
    assert_eq!(reader.diagnostics().len(), 1);
    let diagnostic = &reader.diagnostics()[0];
    assert_eq!(diagnostic.field, "unknown_field");
    assert_eq!(diagnostic.track, Some(TrackId(10)));
    assert_eq!(diagnostic.sample, None);
    assert_eq!(diagnostic.message, error.message);
    let conversion = reader
        .convert(
            reader.audio().unwrap(),
            &targets(&dir, &[Profile::DamfHome]),
        )
        .unwrap();
    assert_eq!(conversion.read, reader.diagnostics());
}

#[test]
fn duplicate_metadata_top_level_keys_are_rejected() {
    let dir = Temp::new();
    for (metadata, key) in [
        (
            format!("sampleRate: 96000\n{THIRD_PARTY_METADATA}"),
            "sampleRate",
        ),
        (
            format!("{THIRD_PARTY_METADATA}\"sampleRate\": 48000\n"),
            "sampleRate",
        ),
        (format!("{THIRD_PARTY_METADATA}events: []\n"), "events"),
        (
            format!("{THIRD_PARTY_METADATA}extension: true\nextension: false\n"),
            "extension",
        ),
    ] {
        let manifest = third_party(&dir, &metadata);
        for policy in [LossPolicy::Reject, LossPolicy::Allow] {
            let error = DamfReader::open(&manifest, policy).unwrap_err();
            assert_eq!(error.kind, ErrorKind::InvalidInput);
            assert!(
                error.message.contains(&format!("duplicate key {key:?}")),
                "{error}"
            );
        }
    }
}
