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

// 独立构造文件，避免写入器只生成紧凑 CHNA 而掩盖导入兼容性问题。
fn stereo_xml(uids: &str) -> String {
    format!(
        r#"<ebuCoreMain><coreMetadata><format><audioFormatExtended>
<audioProgramme audioProgrammeID="APR_1001" audioProgrammeName="Programme"><audioContentIDRef>ACO_1001</audioContentIDRef></audioProgramme>
<audioContent audioContentID="ACO_1001" audioContentName="Content"><audioObjectIDRef>AO_1001</audioObjectIDRef></audioContent>
<audioObject audioObjectID="AO_1001" audioObjectName="Stereo"><audioPackFormatIDRef>AP_00010002</audioPackFormatIDRef><audioTrackUIDRef>ATU_00000001</audioTrackUIDRef><audioTrackUIDRef>ATU_00000002</audioTrackUIDRef></audioObject>
{uids}</audioFormatExtended></format></coreMetadata></ebuCoreMain>"#
    )
}

fn stereo_chna() -> Vec<u8> {
    let mut chna = vec![2, 0, 2, 0];
    for index in 1u16..=2 {
        chna.extend_from_slice(&index.to_le_bytes());
        chna.extend_from_slice(
            format!("ATU_{index:08x}AT_0001000{index}_01AP_00010002\0").as_bytes(),
        );
    }
    chna
}

fn independent_wave(dir: &Path, xml: &str, chna: &[u8]) -> PathBuf {
    let mut body = b"WAVE".to_vec();
    let fmt = [1, 0, 2, 0, 0x80, 0xbb, 0, 0, 0, 0xee, 2, 0, 4, 0, 16, 0];
    for (id, data) in [
        (b"fmt ", fmt.as_slice()),
        (b"chna", chna),
        (b"data", &[0; 4][..]),
        (b"axml", xml.as_bytes()),
    ] {
        body.extend_from_slice(id);
        body.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
        body.extend_from_slice(data);
        if data.len() % 2 != 0 {
            body.push(0);
        }
    }
    let mut wave = b"RIFF".to_vec();
    wave.extend_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
    wave.extend_from_slice(&body);
    let path = dir.join("independent.wav");
    std::fs::write(&path, wave).unwrap();
    path
}

fn json_report(command: &str, path: &Path) -> serde_json::Value {
    let output = mfadm(&[command.as_ref(), path.as_os_str(), "--json".as_ref()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn reserved_chna_records_are_ignored_by_info_and_adm() {
    let dir = Scratch::new();
    let mut chna = stereo_chna();
    chna.resize(4 + 32 * 40, 0);
    let path = independent_wave(
        &dir.0,
        &stereo_xml(
            r#"<audioTrackUID UID="ATU_00000001"><audioTrackFormatIDRef>AT_00010001_01</audioTrackFormatIDRef><audioPackFormatIDRef>AP_00010002</audioPackFormatIDRef></audioTrackUID>
<audioTrackUID UID="ATU_00000002"><audioTrackFormatIDRef>AT_00010002_01</audioTrackFormatIDRef><audioPackFormatIDRef>AP_00010002</audioPackFormatIDRef></audioTrackUID>"#,
        ),
        &chna,
    );
    assert_eq!(
        json_report("info", &path)["chna"].as_array().unwrap().len(),
        2
    );
    let adm = json_report("adm", &path);
    assert_eq!(adm["direct_speaker_blocks"], 2);
    assert_eq!(adm["objects"][0]["kind"], "DirectSpeakers");
}

#[test]
fn chna_provides_missing_track_uid_definitions_and_format_references() {
    let dir = Scratch::new();
    for uids in [
        "",
        r#"<audioTrackUID UID="ATU_00000001"/><audioTrackUID UID="ATU_00000002"/>"#,
    ] {
        for direct in [false, true] {
            let mut chna = stereo_chna();
            if direct {
                for (index, record) in chna[4..].as_chunks_mut::<40>().0.iter_mut().enumerate() {
                    record[14..28]
                        .copy_from_slice(format!("AC_0001000{}_00", index + 1).as_bytes());
                }
            }
            let xml = stereo_xml(uids);
            let path = independent_wave(&dir.0, &xml, &chna);
            let adm = json_report("adm", &path);
            assert_eq!(adm["objects"][0]["kind"], "DirectSpeakers");
            assert_eq!(adm["direct_speaker_blocks"], 2);
            assert_eq!(adm["objects"][0]["tracks"][0]["track_index"], 1);
            assert_eq!(adm["objects"][0]["tracks"][1]["track_index"], 2);
            assert_eq!(adm["warnings"], serde_json::json!([]));
            let output = mfadm(&["axml".as_ref(), path.as_os_str()]);
            assert!(output.status.success());
            assert_eq!(output.stdout, xml.as_bytes());
        }
    }
}

#[test]
fn inconsistent_chna_and_axml_references_are_errors() {
    let dir = Scratch::new();
    for reference in [
        "<audioTrackFormatIDRef>AT_00010002_01</audioTrackFormatIDRef>",
        "<audioChannelFormatIDRef>AC_00010001</audioChannelFormatIDRef>",
        "<audioPackFormatIDRef>AP_00010003</audioPackFormatIDRef>",
    ] {
        let xml = stereo_xml(&format!(
            r#"<audioTrackUID UID="ATU_00000001">{reference}</audioTrackUID><audioTrackUID UID="ATU_00000002"/>"#
        ));
        let path = independent_wave(&dir.0, &xml, &stereo_chna());
        let output = mfadm(&["adm".as_ref(), path.as_os_str(), "--json".as_ref()]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("CHNA") && error.contains("AXML"), "{error}");
    }
}

#[test]
fn importing_reserved_chna_retains_track_validation_and_metadata_budget() {
    let dir = Scratch::new();
    let mut chna = stereo_chna();
    chna.resize(4 + 32 * 40, 0);
    let path = independent_wave(&dir.0, &stereo_xml(""), &chna);
    let error = macin_format::adm(&path, 1024).unwrap_err();
    assert!(error.message.contains("预算"), "{error}");
    for (offset, value) in [(0, 1u16), (4, 3u16)] {
        let mut invalid = chna.clone();
        invalid[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        let path = independent_wave(&dir.0, &stereo_xml(""), &invalid);
        for command in ["info", "adm"] {
            let output = mfadm(&[command.as_ref(), path.as_os_str()]);
            assert_eq!(output.status.code(), Some(1));
        }
    }
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
