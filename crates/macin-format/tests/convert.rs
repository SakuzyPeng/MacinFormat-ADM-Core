use macin_atmos::{
    BinauralMode, MasterSpec, MasterWriter, ObjectPatch, ObjectSpec, ObjectUpdate, PcmBlock,
    Profile, TrackId,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "macin-format-convert-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A DAMF home master whose one object switches to near-field headphone rendering, which ADM
/// cannot express.
fn damf(dir: &Path) -> PathBuf {
    let spec = MasterSpec {
        profile: Profile::DamfHome,
        objects: vec![ObjectSpec::new(TrackId(1), "Voice")],
        ..Default::default()
    };
    let mut writer = MasterWriter::create(dir.join("src"), spec).unwrap();
    let near = ObjectUpdate {
        track: TrackId(1),
        sample: 100,
        ramp_samples: None,
        patch: ObjectPatch {
            binaural_mode: Some(BinauralMode::Near),
            ..Default::default()
        },
    };
    writer
        .write_block(PcmBlock::F32Interleaved(&vec![0.25; 4800 * 11]), &[near])
        .unwrap();
    writer.finish().unwrap();
    dir.join("src/master.atmos")
}

fn mfadm(args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mfadm"))
        .args(args)
        .output()
        .unwrap()
}
fn to(profile: &str, path: &Path) -> std::ffi::OsString {
    let mut arg = std::ffi::OsString::from("--to=");
    arg.push(profile);
    arg.push("=");
    arg.push(path);
    arg
}

#[test]
fn convert_writes_every_target_and_reports_losses() {
    let dir = Scratch::new();
    let manifest = damf(&dir.0);
    let (adm, damf3) = (dir.path("out.wav"), dir.path("out-3dof"));
    let output = mfadm(&[
        "convert".as_ref(),
        manifest.as_os_str(),
        &to("standard", &adm),
        &to("3dof", &damf3),
        "--allow-loss".as_ref(),
        "--json".as_ref(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["source_profile"], "home");
    assert_eq!(report["objects"], 1);
    assert_eq!(report["updates"], 1);
    let outputs = report["outputs"].as_array().unwrap();
    assert_eq!(outputs[0]["profile"], "standard");
    assert_eq!(outputs[0]["frames"], 4800);
    assert_eq!(outputs[0]["losses"][0]["field"], "binaural_mode");
    assert_eq!(
        outputs[0]["losses"][0]["track"], 10,
        "a DAMF ID is the channel index"
    );
    assert!(outputs[1]["losses"].as_array().unwrap().is_empty());
    assert!(damf3.join("master.atmos").is_file());

    // The ADM output is a valid master the inspection commands can read.
    let summary = macin_format::adm(&adm, 1 << 20).unwrap();
    assert_eq!(summary.objects.len(), 2);
    assert_eq!(summary.objects[1].name, "Voice");
}

#[test]
fn convert_refuses_losses_and_existing_outputs_by_default() {
    let dir = Scratch::new();
    let manifest = damf(&dir.0);
    let adm = dir.path("strict.wav");
    let output = mfadm(&["convert".as_ref(), manifest.as_os_str(), &to("logic", &adm)]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("track 10, sample 100, field binaural_mode"),
        "{stderr}"
    );
    assert!(!adm.exists(), "a refused conversion leaves nothing behind");

    std::fs::write(&adm, b"keep me").unwrap();
    let output = mfadm(&[
        "convert".as_ref(),
        manifest.as_os_str(),
        &to("logic", &adm),
        "--allow-loss".as_ref(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("strict.wav") && stderr.contains("already exists"),
        "{stderr}"
    );
    assert_eq!(std::fs::read(&adm).unwrap(), b"keep me");
}

#[test]
fn convert_takes_a_wav_in_place_of_the_damf_audio() {
    let dir = Scratch::new();
    let manifest = damf(&dir.0);
    let from_caf = dir.path("caf.wav");
    assert!(
        mfadm(&[
            "convert".as_ref(),
            manifest.as_os_str(),
            &to("standard", &from_caf),
            "--allow-loss".as_ref(),
        ])
        .status
        .success()
    );
    // Strip the ADM chunks: the same PCM bytes in a plain RIFF is the "WAV" for the second run.
    let wav = dir.path("plain.wav");
    let mut chunks = macin_bwf::ChunkReader::new(std::fs::File::open(&from_caf).unwrap()).unwrap();
    let data = chunks
        .chunks()
        .iter()
        .find(|c| &c.id == b"data")
        .unwrap()
        .size;
    let mut pcm = vec![0; usize::try_from(data).unwrap()];
    assert!(chunks.read_chunk(*b"data", &mut pcm).unwrap());
    let format = macin_bwf::Reader::new(std::fs::File::open(&from_caf).unwrap())
        .unwrap()
        .info()
        .format;
    let mut writer = macin_bwf::Writer::new(
        std::fs::File::create(&wav).unwrap(),
        format,
        macin_bwf::WriterOptions::default(),
    )
    .unwrap();
    writer.write_pcm_bytes(&pcm).unwrap();
    writer.finish().unwrap();

    let from_wav = dir.path("wav.wav");
    let output = mfadm(&[
        "convert".as_ref(),
        manifest.as_os_str(),
        &to("standard", &from_wav),
        "--audio".as_ref(),
        wav.as_os_str(),
        "--allow-loss".as_ref(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("audio from"));
    assert_eq!(
        std::fs::read(&from_wav).unwrap(),
        std::fs::read(&from_caf).unwrap()
    );
}

#[test]
fn convert_arguments_are_checked_by_clap() {
    for args in [
        vec!["convert", "x.atmos"],
        vec!["convert", "x.atmos", "--to", "standard"],
        vec!["convert", "x.atmos", "--to", "mp3=out.mp3"],
        vec!["convert", "x.atmos", "--to", "standard="],
    ] {
        let args: Vec<&std::ffi::OsStr> = args.iter().map(|a| a.as_ref()).collect();
        assert_eq!(mfadm(&args).status.code(), Some(2), "{args:?}");
    }
}

fn dbmd_identity(path: &Path) -> (String, String) {
    let mut wav = macin_bwf::Reader::new(std::fs::File::open(path).unwrap()).unwrap();
    let data = wav.metadata(*b"dbmd", 1 << 20).unwrap().unwrap();
    let mut at = 4;
    while data[at] != 0 {
        let id = data[at];
        let size = u16::from_le_bytes(data[at + 1..at + 3].try_into().unwrap());
        let payload = &data[at + 3..at + 3 + usize::from(size)];
        let checksum = data[at + 3 + usize::from(size)];
        assert_eq!(
            payload
                .iter()
                .fold(size as u8, |sum, byte| sum.wrapping_add(*byte))
                .wrapping_add(checksum),
            0
        );
        if id == 9 {
            let text = |bytes: &[u8]| {
                String::from_utf8(bytes.to_vec())
                    .unwrap()
                    .trim_end_matches('\0')
                    .to_owned()
            };
            return (text(&payload[..32]), text(&payload[32..96]));
        }
        at += usize::from(size) + 4;
    }
    panic!("DBMD segment 9 missing");
}

#[test]
fn convert_customizes_dbmd_from_adm_and_damf() {
    let dir = Scratch::new();
    let variants = [
        (None, None),
        (Some("Created using Dolby equipment".to_owned()), None),
        (None, Some("Studio Export".to_owned())),
        (Some("A".repeat(32)), Some("B".repeat(64))),
        (Some(String::new()), Some(String::new())),
    ];
    for (source_index, source) in [adm(&dir.0), damf(&dir.0)].iter().enumerate() {
        for (index, (originator, product)) in variants.iter().enumerate() {
            let logic = dir.path(&format!("logic-{source_index}-{index}.wav"));
            let standard = dir.path(&format!("standard-{source_index}-{index}.wav"));
            let home = dir.path(&format!("home-{source_index}-{index}"));
            let mut args = vec![
                "convert".into(),
                source.as_os_str().to_owned(),
                to("logic", &logic),
                to("standard", &standard),
                to("home", &home),
                "--allow-loss".into(),
            ];
            for (flag, value) in [
                ("--dbmd-originator", originator),
                ("--dbmd-product", product),
            ] {
                if let Some(value) = value {
                    args.push(flag.into());
                    args.push(value.into());
                }
            }
            let args: Vec<_> = args.iter().map(|s| s.as_os_str()).collect();
            let output = mfadm(&args);
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                dbmd_identity(&logic),
                (
                    originator
                        .as_deref()
                        .unwrap_or("Created by MacinFormat")
                        .to_owned(),
                    product
                        .as_deref()
                        .unwrap_or("MacinFormat ADM Core")
                        .to_owned(),
                )
            );
            assert_eq!(
                std::fs::read(&standard).unwrap(),
                std::fs::read(dir.path(&format!("standard-{source_index}-0.wav"))).unwrap()
            );
            assert_eq!(
                std::fs::read(home.join("master.atmos")).unwrap(),
                std::fs::read(dir.path(&format!("home-{source_index}-0/master.atmos"))).unwrap()
            );
        }
    }
}

#[test]
fn dbmd_creator_arguments_validate_ascii_width_and_control_characters() {
    for (flag, value) in [
        ("--dbmd-originator", "A".repeat(33)),
        ("--dbmd-originator", "创建工具".to_owned()),
        ("--dbmd-originator", "first\nsecond".to_owned()),
        ("--dbmd-product", "B".repeat(65)),
        ("--dbmd-product", "产品".to_owned()),
        ("--dbmd-product", "first\tsecond".to_owned()),
    ] {
        let output = mfadm(&[
            "convert".as_ref(),
            "missing.atmos".as_ref(),
            "--to=logic=unused.wav".as_ref(),
            flag.as_ref(),
            value.as_ref(),
        ]);
        assert_eq!(output.status.code(), Some(2), "{flag} {value:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("ASCII"));
    }
}

#[test]
fn convert_help_explains_the_conversion_tool_originator_option() {
    for help in ["--help", "-h"] {
        let output = mfadm(&["convert".as_ref(), help.as_ref()]);
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        for expected in [
            "--dbmd-originator",
            "--dbmd-product",
            "Created using Dolby equipment",
            "Conversion Tool 2.1.2",
            "input file is not Dolby Atmos",
        ] {
            assert!(text.contains(expected), "{expected:?} missing from {text}");
        }
    }
}

#[test]
fn convert_reports_invalid_caf_layout_without_panicking() {
    let dir = Scratch::new();
    let manifest = damf(&dir.0);
    let audio = dir.path("src/master.atmos.audio");
    let mut caf = std::fs::read(&audio).unwrap();
    caf[36..40].copy_from_slice(&u32::MAX.wrapping_mul(3).to_be_bytes());
    caf[44..48].copy_from_slice(&u32::MAX.to_be_bytes());
    caf[56..64].copy_from_slice(&4i64.to_be_bytes());
    caf.truncate(68);
    std::fs::write(&audio, caf).unwrap();
    let out = dir.path("out.wav");
    let output = mfadm(&[
        "convert".as_ref(),
        manifest.as_os_str(),
        &to("standard", &out),
        "--allow-loss".as_ref(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid CAF channel layout"), "{stderr}");
    assert!(!out.exists());
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
}

#[test]
fn convert_rejects_duplicate_metadata_keys_even_when_allowing_loss() {
    let dir = Scratch::new();
    let manifest = damf(&dir.0);
    let metadata = dir.path("src/master.atmos.metadata");
    let mut text = std::fs::read_to_string(&metadata).unwrap();
    text.push_str("sampleRate: 96000\n");
    std::fs::write(&metadata, text).unwrap();
    let out = dir.path("out.wav");
    let output = mfadm(&[
        "convert".as_ref(),
        manifest.as_os_str(),
        &to("standard", &out),
        "--allow-loss".as_ref(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("duplicate key \"sampleRate\""), "{stderr}");
    assert!(!out.exists());
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
}

/// An ADM Standard master of one object that moves once.
fn adm(dir: &Path) -> PathBuf {
    let path = dir.join("src.wav");
    let spec = MasterSpec {
        objects: vec![ObjectSpec::new(TrackId(1), "Voice")],
        ..Default::default()
    };
    let mut writer = MasterWriter::create(&path, spec).unwrap();
    let moves = ObjectUpdate {
        track: TrackId(1),
        sample: 1000,
        ramp_samples: Some(480),
        patch: ObjectPatch {
            position: Some([0.5, 1., 0.]),
            ..Default::default()
        },
    };
    writer
        .write_block(PcmBlock::F32Interleaved(&vec![0.25; 4800 * 11]), &[moves])
        .unwrap();
    writer.finish().unwrap();
    path
}

#[test]
fn convert_reads_adm_as_well_as_damf() {
    let dir = Scratch::new();
    let source = adm(&dir.0);
    let (damf, logic) = (dir.path("damf"), dir.path("logic.wav"));
    let output = mfadm(&[
        "convert".as_ref(),
        source.as_os_str(),
        &to("home", &damf),
        &to("logic", &logic),
        "--fps".as_ref(),
        "25".as_ref(),
        "--json".as_ref(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["source_profile"], "standard");
    assert_eq!(report["objects"], 1);
    assert_eq!(report["updates"], 1);
    let manifest = std::fs::read_to_string(damf.join("master.atmos")).unwrap();
    assert!(manifest.contains("fps: 25"), "{manifest}");
    let metadata = std::fs::read_to_string(damf.join("master.atmos.metadata")).unwrap();
    assert!(
        metadata.contains("samplePos: 1000") && metadata.contains("rampLength: 480"),
        "{metadata}"
    );
    assert_eq!(macin_format::chunks(&logic).unwrap().container, "RF64");

    // The DAMF converts back; --fps belongs to ADM sources only.
    let back = dir.path("back.wav");
    let output = mfadm(&[
        "convert".as_ref(),
        damf.join("master.atmos").as_os_str(),
        &to("standard", &back),
        "--fps".as_ref(),
        "25".as_ref(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--fps applies to ADM input"));
    let output = mfadm(&[
        "convert".as_ref(),
        damf.join("master.atmos").as_os_str(),
        &to("standard", &back),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read(&back).unwrap(),
        std::fs::read(&source).unwrap()
    );
    assert_eq!(
        mfadm(&[
            "convert".as_ref(),
            source.as_os_str(),
            &to("home", &dir.path("x")),
            "--fps=60".as_ref()
        ])
        .status
        .code(),
        Some(2)
    );
}

#[test]
fn convert_requires_each_approximation_and_reports_it_separately() {
    let dir = Scratch::new();
    let original = adm(&dir.0);
    let source = dir.path("fields.wav");
    let mut reader = macin_bwf::ChunkReader::new(std::fs::File::open(&original).unwrap()).unwrap();
    let mut xml = String::from_utf8(reader.metadata(*b"axml", 1 << 20).unwrap().unwrap()).unwrap();
    let channel = xml
        .find("<audioChannelFormat audioChannelFormatID=\"AC_00031001\"")
        .unwrap();
    let end = channel + xml[channel..].find("</audioBlockFormat>").unwrap();
    xml.insert_str(end, "<diffuse>0.5</diffuse><screenRef>1</screenRef>");
    macin_bwf::replace_chunk(
        reader.into_inner(),
        std::fs::File::create(&source).unwrap(),
        *b"axml",
        xml.as_bytes(),
    )
    .unwrap();
    for (index, choice, missing) in [
        (0, "diffuse-decorrelation", "screen_ref"),
        (1, "screen-ref-factor", "diffuse"),
    ] {
        let target = dir.path(&format!("rejected-{index}"));
        let result = mfadm(&[
            "convert".as_ref(),
            source.as_os_str(),
            &to("home", &target),
            "--approximate".as_ref(),
            choice.as_ref(),
        ]);
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr).contains(missing));
        assert!(!target.exists());
    }
    let target = dir.path("home");
    let result = mfadm(&[
        "convert".as_ref(),
        source.as_os_str(),
        &to("home", &target),
        "--approximate".as_ref(),
        "diffuse-decorrelation".as_ref(),
        "--approximate".as_ref(),
        "screen-ref-factor".as_ref(),
        "--json".as_ref(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["read_losses"], serde_json::json!([]));
    assert_eq!(report["outputs"][0]["losses"][0]["field"], "diffuse");
    assert_eq!(report["outputs"][0]["losses"][0]["kind"], "approximation");
    assert_eq!(report["outputs"][0]["losses"][1]["field"], "screen_ref");
    let reader = macin_atmos::damf::DamfReader::open(
        target.join("master.atmos"),
        macin_atmos::LossPolicy::Reject,
    )
    .unwrap();
    assert_eq!(reader.spec().objects[0].initial.decorrelation, 0.5);
    assert_eq!(reader.spec().objects[0].initial.screen_factor, 1.);
    let target = dir.path("text");
    let result = mfadm(&[
        "convert".as_ref(),
        source.as_os_str(),
        &to("home", &target),
        "--approximate".as_ref(),
        "diffuse-decorrelation".as_ref(),
        "--approximate".as_ref(),
        "screen-ref-factor".as_ref(),
    ]);
    assert!(result.status.success());
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.contains("approximate diffuse"), "{text}");
    assert!(text.contains("approximate screen_ref"), "{text}");
    assert!(!text.contains("lost "), "{text}");
}

#[test]
fn unknown_approximation_is_a_usage_error() {
    let result = mfadm(&[
        "convert".as_ref(),
        "unused.wav".as_ref(),
        "--to=home=unused".as_ref(),
        "--approximate".as_ref(),
        "all".as_ref(),
    ]);
    assert_eq!(result.status.code(), Some(2));
    let message = String::from_utf8_lossy(&result.stderr);
    assert!(message.contains("diffuse-decorrelation"), "{message}");
    assert!(message.contains("screen-ref-factor"), "{message}");
}

#[test]
fn convert_reports_and_preserves_bed_updates() {
    let dir = Scratch::new();
    let source = dir.path("bed-source");
    let spec = MasterSpec {
        profile: Profile::DamfHome,
        objects: vec![ObjectSpec::new(TrackId(1), "Voice")],
        ..Default::default()
    };
    let mut writer = MasterWriter::create(&source, spec).unwrap();
    writer
        .schedule_bed_updates(&[macin_atmos::BedUpdate {
            channel: 0,
            sample: 1200,
            ramp_samples: Some(600),
            patch: macin_atmos::BedPatch {
                gain: Some(macin_atmos::Gain::Decibels(-3.)),
                ..Default::default()
            },
        }])
        .unwrap();
    writer
        .write_block(PcmBlock::F32Interleaved(&vec![0.125; 4800 * 11]), &[])
        .unwrap();
    writer.finish().unwrap();
    let target = dir.path("bed-copy");
    let output = mfadm(&[
        "convert".as_ref(),
        source.join("master.atmos").as_os_str(),
        &to("home", &target),
        "--json".as_ref(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["updates"], 0);
    assert_eq!(report["bed_updates"], 1);
    assert_eq!(
        std::fs::read(source.join("master.atmos.metadata")).unwrap(),
        std::fs::read(target.join("master.atmos.metadata")).unwrap()
    );
}

#[test]
fn adm_replacement_audio_retains_object_end_and_reports_bed_extension() {
    let dir = Scratch::new();
    let source = adm(&dir.0);
    let audio = dir.path("longer.wav");
    let mut writer = macin_bwf::Writer::new(
        std::fs::File::create(&audio).unwrap(),
        macin_bwf::Format {
            channels: 11,
            sample_rate: 48_000,
            sample_format: macin_bwf::SampleFormat::Pcm24,
            channel_mask: None,
        },
        macin_bwf::WriterOptions::default(),
    )
    .unwrap();
    writer.write_pcm_bytes(&vec![0; 9600 * 11 * 3]).unwrap();
    writer.finish().unwrap();
    let damf_path = dir.path("damf");
    let adm_path = dir.path("longer-adm.wav");
    let args = [
        "convert".as_ref(),
        source.as_os_str(),
        "--audio".as_ref(),
        audio.as_os_str(),
        &to("home", &damf_path),
        &to("standard", &adm_path),
        "--json".as_ref(),
    ];
    let output = mfadm(&args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["updates"], 2);
    assert_eq!(report["read_losses"], serde_json::json!([]));
    assert_eq!(report["outputs"][0]["frames"], 9600);
    let damf = macin_atmos::damf::DamfReader::open(
        damf_path.join("master.atmos"),
        macin_atmos::LossPolicy::Reject,
    )
    .unwrap();
    let bed_events = damf
        .bed_updates()
        .unwrap()
        .collect::<macin_atmos::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(bed_events.len(), 10);
    assert!(
        bed_events
            .iter()
            .all(|e| e.sample == 4800 && e.patch.active == Some(false))
    );
    let last = damf.updates().unwrap().last().unwrap().unwrap();
    assert_eq!((last.sample, last.patch.active), (4800, Some(false)));
    let adm = macin_atmos::adm::AdmReader::open(adm_path, macin_atmos::LossPolicy::Reject).unwrap();
    assert!(
        adm.updates()
            .unwrap()
            .map(Result::unwrap)
            .any(|u| u.sample == 4800 && u.patch.gain == Some(macin_atmos::Gain::Linear(0.)))
    );
}

#[test]
fn literal_preservation_is_per_field_and_reports_the_nonportable_value() {
    let dir = Scratch::new();
    let mut s = MasterSpec {
        objects: vec![ObjectSpec::new(TrackId(1), "Voice")],
        ..Default::default()
    };
    s.objects[0].initial.divergence = Some(macin_atmos::ObjectDivergence {
        value: 0.4,
        position_range: None,
        azimuth_range: Some(30.),
    });
    s.preserve_literal.cartesian_azimuth_range = true;
    let legacy = dir.path("old.wav");
    let mut w = MasterWriter::create(&legacy, s).unwrap();
    w.write_block(PcmBlock::S24Le(&[0; 33 * 4]), &[]).unwrap();
    w.finish().unwrap();
    let dst = dir.path("copy.wav");
    let to_adm = to("standard", &dst);
    let fail = mfadm(&[
        "convert".as_ref(),
        legacy.as_os_str(),
        &to_adm,
        "--preserve-literal".as_ref(),
        "damf-mid".as_ref(),
    ]);
    assert_eq!(fail.status.code(), Some(1));
    assert!(!dst.exists());
    let saved = mfadm(&[
        "convert".as_ref(),
        legacy.as_os_str(),
        &to_adm,
        "--preserve-literal".as_ref(),
        "cartesian-azimuth-range".as_ref(),
        "--json".as_ref(),
    ]);
    assert!(saved.status.success());
    let report: serde_json::Value = serde_json::from_slice(&saved.stdout).unwrap();
    assert_eq!(report["read_losses"], serde_json::json!([]));
    assert_eq!(
        report["outputs"][0]["losses"][0]["kind"],
        "preserved_literal"
    );
    assert_eq!(
        macin_atmos::adm::AdmReader::open(dst, macin_atmos::LossPolicy::Reject)
            .unwrap()
            .spec()
            .objects[0]
            .initial
            .divergence
            .unwrap()
            .azimuth_range,
        Some(30.)
    );
    let manifest = damf(&dir.0);
    let meta = manifest.with_extension("atmos.metadata");
    let yaml = std::fs::read_to_string(&meta)
        .unwrap()
        .replace("binauralRenderMode: near", "binauralRenderMode: mid");
    std::fs::write(meta, yaml).unwrap();
    let out = dir.path("home");
    let dest = to("home", &out);
    let saved = mfadm(&[
        "convert".as_ref(),
        manifest.as_os_str(),
        &dest,
        "--preserve-literal".as_ref(),
        "damf-mid".as_ref(),
    ]);
    assert!(saved.status.success());
    assert!(String::from_utf8_lossy(&saved.stdout).contains("preserved binaural_mode"));
    assert!(
        std::fs::read_to_string(out.join("master.atmos.metadata"))
            .unwrap()
            .contains("binauralRenderMode: mid")
    );
}
