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
