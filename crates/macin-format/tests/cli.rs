use macin_atmos::{MasterSpec, MasterWriter, ObjectSpec, PcmBlock, Profile, TrackId};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "macin-format-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A 7.1.2 bed plus two objects, 4800 frames.
fn master(dir: &Path, profile: Profile, rate: u32) -> PathBuf {
    let path = dir.join("master.wav");
    let spec = MasterSpec {
        sample_rate: rate,
        profile,
        objects: vec![
            ObjectSpec::new(TrackId(7), "Left object"),
            ObjectSpec::new(TrackId(8), "Right object"),
        ],
        ..Default::default()
    };
    let mut writer = MasterWriter::create(&path, spec).unwrap();
    let pcm = vec![0.0f32; 4800 * 12];
    writer
        .write_block(PcmBlock::F32Interleaved(&pcm), &[])
        .unwrap();
    writer.finish().unwrap();
    path
}

fn mfadm(args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mfadm"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn chunks_follow_the_adm_layout() {
    let dir = Scratch::new();
    let report = macin_format::chunks(&master(&dir.0, Profile::Standard, 48000)).unwrap();
    assert_eq!(report.container, "BW64");
    let ids: Vec<_> = report.chunks.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids.first(), Some(&"ds64"));
    let at = |id| ids.iter().position(|c| *c == id).unwrap();
    assert!(at("chna") < at("data") && at("data") < at("axml"));
    let data = &report.chunks[at("data")];
    assert_eq!(data.size, 4800 * 12 * 3);
}

#[test]
fn info_reports_format_and_the_chna_table() {
    let dir = Scratch::new();
    let report = macin_format::info(&master(&dir.0, Profile::Logic, 96000)).unwrap();
    assert_eq!(report.container, "RF64");
    assert_eq!(
        (report.sample_rate, report.channels, report.bits),
        (96000, 12, 24)
    );
    assert_eq!(report.frames, 4800);
    assert!((report.seconds - 0.05).abs() < 1e-12);
    let chna = report.chna.unwrap();
    assert_eq!(chna.len(), 12);
    assert_eq!(chna[0].track_index, 1);
    assert!(chna[0].uid.starts_with("ATU_"));
    assert!(chna.iter().all(|e| !e.uid.contains('\\')));
}

#[test]
fn adm_summary_resolves_tracks_against_chna() {
    let dir = Scratch::new();
    let report = macin_format::adm(&master(&dir.0, Profile::Standard, 48000), 1 << 20).unwrap();
    assert_eq!(report.sample_rate, 48000);
    assert_eq!(report.programmes.len(), 1);
    assert_eq!(report.programmes[0].contents.len(), 1);
    let kinds: Vec<_> = report.objects.iter().map(|o| o.kind.as_deref()).collect();
    assert_eq!(
        kinds,
        [Some("DirectSpeakers"), Some("Objects"), Some("Objects")]
    );
    assert_eq!(report.objects[0].tracks.len(), 10);
    let names: Vec<_> = report.objects[1..]
        .iter()
        .map(|o| o.name.as_str())
        .collect();
    assert_eq!(names, ["Left object", "Right object"]);
    let tracks: Vec<_> = report
        .objects
        .iter()
        .flat_map(|o| &o.tracks)
        .map(|t| t.track_index)
        .collect();
    assert_eq!(tracks, (1..=12).map(Some).collect::<Vec<_>>());
    assert!(report.warnings.is_empty());
}

#[test]
fn adm_respects_the_metadata_budget() {
    let dir = Scratch::new();
    let error = macin_format::adm(&master(&dir.0, Profile::Standard, 48000), 64).unwrap_err();
    assert!(error.io_kind.is_none(), "{error}");
}

#[test]
fn axml_is_the_chunk_byte_for_byte() {
    let dir = Scratch::new();
    let path = master(&dir.0, Profile::Standard, 48000);
    let xml = macin_format::axml(&path, 1 << 20).unwrap();
    let report = macin_format::chunks(&path).unwrap();
    let chunk = report.chunks.iter().find(|c| c.id == "axml").unwrap();
    let file = std::fs::read(&path).unwrap();
    let start = usize::try_from(chunk.offset).unwrap();
    let end = start + usize::try_from(chunk.size).unwrap();
    assert_eq!(xml, &file[start..end]);
    let output = mfadm(&["axml".as_ref(), path.as_os_str()]);
    assert!(output.status.success());
    assert_eq!(output.stdout, xml);
}

#[test]
fn json_output_parses_and_matches_the_library() {
    let dir = Scratch::new();
    let path = master(&dir.0, Profile::Standard, 48000);
    for command in ["chunks", "info", "adm"] {
        let output = mfadm(&[command.as_ref(), path.as_os_str(), "--json".as_ref()]);
        assert!(output.status.success(), "{command}");
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(value.is_object(), "{command}");
    }
    let output = mfadm(&["adm".as_ref(), path.as_os_str(), "--json".as_ref()]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["objects"].as_array().unwrap().len(), 3);
}

#[test]
fn text_output_and_failures() {
    let dir = Scratch::new();
    let path = master(&dir.0, Profile::Standard, 48000);
    let output = mfadm(&["info".as_ref(), path.as_os_str()]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("48000 Hz, 12 ch, 24-bit pcm"), "{text}");

    let missing = dir.0.join("missing.wav");
    let output = mfadm(&["chunks".as_ref(), missing.as_os_str()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("mfadm: "));

    let output = mfadm(&["adm".as_ref()]);
    assert_eq!(output.status.code(), Some(2), "usage errors are clap's");
}
