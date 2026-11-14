use macin_adm::{Document, Patch, Snapshot, TrackUidDefinition};

const OBJECTS: &str = include_str!("fixtures/objects-stream-only.xml");
const HOA: &str = include_str!("fixtures/hoa-pack-attributes.xml");

fn definitions(kind: u16, channels: u16) -> Vec<TrackUidDefinition> {
    (1..=channels)
        .map(|channel| TrackUidDefinition {
            uid: format!("ATU_{channel:08x}"),
            format_id: format!("AT_{kind:04x}{:04x}_01", 0x1000 + channel),
            pack_id: Some(format!("AP_{kind:04x}1001")),
        })
        .collect()
}

fn without_uids(xml: &str) -> String {
    let mut output = xml.to_owned();
    while let Some(start) = output.find("<audioTrackUID ") {
        let end = start + output[start..].find("</audioTrackUID>").unwrap();
        output.replace_range(start..end + "</audioTrackUID>".len(), "");
    }
    output
}

#[test]
fn chna_only_track_definitions_resolve_objects_and_hoa() {
    for (source, kind, count) in [(OBJECTS, 3, 2), (HOA, 4, 4)] {
        let xml = without_uids(source);
        let mut definitions = definitions(kind, count);
        // CHNA 固定宽度字段允许填充，ADM ID 匹配忽略十六进制字母的大小写。
        definitions[0].uid = "atu_00000001 \0".to_owned();
        for direct in [false, true] {
            let mut entries = definitions.clone();
            if direct {
                for entry in &mut entries {
                    entry.format_id = entry.format_id.replace("AT_", "AC_");
                    entry.format_id.truncate(11);
                }
            }
            let doc = Document::parse_with_track_uids(&xml, &entries).unwrap();
            let channels = definitions
                .iter()
                .enumerate()
                .map(|(i, d)| (d.uid.clone(), u16::try_from(i + 1).unwrap()))
                .collect::<Vec<_>>();
            let scene = Snapshot::extract(&doc, 48000, &channels).unwrap();
            assert_eq!(scene.tracks.len(), usize::from(count));
            assert_eq!(scene.tracks[0].channel, 1);
            if kind == 3 {
                assert_eq!(scene.object_blocks.len(), 2);
                assert_eq!(scene.object_blocks[0].position.azimuth, 30.0);
                assert_eq!(scene.object_blocks[1].position.azimuth, -30.0);
            } else {
                assert_eq!(scene.hoa_channels.len(), 4);
                assert_eq!(scene.hoa.len(), 1);
                assert_eq!(scene.hoa[0].channels.len, 4);
                assert_eq!(scene.hoa_channels[3].degree, 1);
            }
        }
    }
}

#[test]
fn chna_overlay_preserves_source_and_semantic_patches() {
    let xml = without_uids(OBJECTS);
    let definitions = definitions(3, 2);
    let doc = Document::parse_with_track_uids(&xml, &definitions).unwrap();
    assert_eq!(doc.apply_patches(&[], 48000).unwrap(), xml);
    let scene = Snapshot::extract(&doc, 48000, &[]).unwrap();
    let output = doc
        .apply_patches(
            &[Patch {
                node: scene.objects[0].node,
                field: 1,
                present: 1,
                value: 0.5,
                ..Patch::default()
            }],
            48000,
        )
        .unwrap();
    assert_eq!(
        output,
        xml.replacen(
            "</audioObject>",
            "<gain gainUnit=\"linear\">0.5</gain></audioObject>",
            1
        )
    );
    let updated = Document::parse_with_track_uids(&output, &definitions).unwrap();
    assert_eq!(
        Snapshot::extract(&updated, 48000, &[]).unwrap().objects[0].gain,
        0.5
    );
}

#[test]
fn chna_definitions_validate_targets_and_preserve_optional_pack_references() {
    let mut entries = definitions(3, 2);
    for entry in &mut entries {
        entry.pack_id = None;
    }
    let doc = Document::parse_with_track_uids(OBJECTS, &entries).unwrap();
    assert_eq!(
        Snapshot::extract(&doc, 48000, &[])
            .unwrap()
            .object_blocks
            .len(),
        2
    );
    for (uid, format, pack) in [
        ("ATU_00000000", "AT_00031001_01", "AP_00031001"),
        ("ATU_invalid", "AT_00031001_01", "AP_00031001"),
        ("ATU_00000001", "AT_0003ffff_01", "AP_00031001"),
        ("ATU_00000001", "AS_00031001", "AP_00031001"),
        ("ATU_00000001", "AT_00031001_01", "AP_0003ffff"),
    ] {
        let invalid = TrackUidDefinition {
            uid: uid.to_owned(),
            format_id: format.to_owned(),
            pack_id: Some(pack.to_owned()),
        };
        assert!(Document::parse_with_track_uids(OBJECTS, &[invalid]).is_err());
    }
    let mut entries = definitions(3, 2);
    entries.push(entries[0].clone());
    assert!(Document::parse_with_track_uids(OBJECTS, &entries).is_ok());
    entries.last_mut().unwrap().format_id = entries[1].format_id.clone();
    assert!(Document::parse_with_track_uids(&without_uids(OBJECTS), &entries).is_err());
}
