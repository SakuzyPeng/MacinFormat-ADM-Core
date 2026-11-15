use macin_adm::{Document, Snapshot};

const XML: &str = r#"<?xml version="1.0"?><ebuCoreMain><coreMetadata><format><audioFormatExtended>
<audioProgramme audioProgrammeID="APR_1001" audioProgrammeName="P"><audioContentIDRef>ACO_1001</audioContentIDRef></audioProgramme>
<audioContent audioContentID="ACO_1001" audioContentName="C"><audioObjectIDRef>AO_1001</audioObjectIDRef></audioContent>
<audioObject audioObjectID="AO_1001" audioObjectName="O"><audioPackFormatIDRef>AP_00031001</audioPackFormatIDRef><audioTrackUIDRef>ATU_00000001</audioTrackUIDRef></audioObject>
<audioPackFormat audioPackFormatID="AP_00031001" audioPackFormatName="Pack name" typeDefinition="Objects"><audioChannelFormatIDRef>AC_00031001</audioChannelFormatIDRef></audioPackFormat>
<audioChannelFormat audioChannelFormatID="AC_00031001" audioChannelFormatName="Channel" typeDefinition="Objects">
<audioBlockFormat audioBlockFormatID="AB_00031001_00000001" rtime="00:00:00.000000000" duration="00:00:01.000000000">
<position coordinate="X">0.100000000001</position><position coordinate="Y">-0.5</position><position coordinate="Z">0</position>
<cartesian>1</cartesian><importance>7</importance><zoneExclusion/></audioBlockFormat></audioChannelFormat>
<audioStreamFormat audioStreamFormatID="AS_00031001" audioStreamFormatName="S" formatDefinition="PCM"><audioChannelFormatIDRef>AC_00031001</audioChannelFormatIDRef><audioTrackFormatIDRef>AT_00031001_01</audioTrackFormatIDRef></audioStreamFormat>
<audioTrackFormat audioTrackFormatID="AT_00031001_01" audioTrackFormatName="T" formatDefinition="PCM"><audioStreamFormatIDRef>AS_00031001</audioStreamFormatIDRef></audioTrackFormat>
<audioTrackUID UID="ATU_00000001"><audioTrackFormatIDRef>AT_00031001_01</audioTrackFormatIDRef><audioPackFormatIDRef>AP_00031001</audioPackFormatIDRef></audioTrackUID>
</audioFormatExtended></format></coreMetadata></ebuCoreMain>"#;

#[test]
fn snapshot_nodes_lead_back_to_the_literal_elements() {
    let doc = Document::parse(XML).unwrap();
    let scene = Snapshot::extract(&doc, 48000, &[("ATU_00000001".into(), 1)]).unwrap();
    let block = scene.object_blocks[0].node;
    assert_eq!(doc.element_name(block), Some("audioBlockFormat"));
    assert_eq!(
        doc.element_attribute(block, "duration"),
        Some("00:00:01.000000000")
    );
    let children: Vec<_> = doc
        .element_children(block)
        .into_iter()
        .map(|c| (doc.element_name(c).unwrap(), doc.element_text(c).unwrap()))
        .collect();
    assert_eq!(children[0], ("position", "0.100000000001"));
    assert!(children.contains(&("importance", "7")));
    assert!(children.contains(&("zoneExclusion", "")));
    // The projection holds f32; the literal keeps every digit.
    assert_ne!(f64::from(scene.object_blocks[0].position.x), 0.100000000001);

    let pack = doc.element_by_id("ap_00031001").unwrap();
    assert_eq!(
        doc.element_attribute(pack, "audioPackFormatName"),
        Some("Pack name")
    );
    // Common definitions are reachable too.
    let common = doc.element_by_id("AP_00010002").unwrap();
    assert_eq!(doc.element_name(common), Some("audioPackFormat"));
    assert_eq!(
        doc.track_channels("atu_00000001"),
        vec![doc.element_by_id("AC_00031001").unwrap()]
    );
    assert!(doc.track_channels("ATU_ffffffff").is_empty());
    assert!(doc.track_channels("AO_1001").is_empty());
}

#[test]
fn unknown_node_numbers_are_none_not_panics() {
    let doc = Document::parse(XML).unwrap();
    for node in [u32::MAX >> 1, (1 << 31) | 0x7fff_fff0] {
        assert_eq!(doc.element_name(node), None);
        assert_eq!(doc.element_attribute(node, "x"), None);
        assert_eq!(doc.element_text(node), None);
        assert!(doc.element_children(node).is_empty());
    }
    assert_eq!(doc.element_by_id("AO_9999"), None);
}
