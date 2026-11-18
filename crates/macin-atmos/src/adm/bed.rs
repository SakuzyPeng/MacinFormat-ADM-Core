//! 和对象相同，bed 按文件范围逐块投影。首块保存在 spec，后续块可重放。
use super::*;

/// 保留节目内的 bed 窗口。某声道完全在节目外时用零时长的静音首块维持固定声道布局。
pub(super) fn clip_spec(
    spec: &mut MasterSpec,
    origin: u64,
    programme_end: Option<u64>,
    frames: u64,
) -> Result<()> {
    let end = frames.min(programme_end.unwrap_or(frames));
    if let Some(timing) = &mut spec.bed_timing {
        if programme_end.is_some_and(|p| origin >= p) {
            *timing = AdmTiming {
                start: Some(0),
                duration: Some(0),
            };
        } else if timing.end().is_some_and(|s| s > end)
            || (timing.duration.is_none() && end < frames)
        {
            timing.duration = Some(end.saturating_sub(origin));
        }
    }
    for channel in 0..BED_CHANNELS {
        if let Some(first) = &mut spec.bed_blocks[channel]
            && !first.clip_to_audio(origin, programme_end, frames)?
        {
            spec.bed_geometry[channel] = AdmMetadata::default();
            spec.bed_blocks[channel] = Some(metadata::bed_state_block(
                spec,
                channel,
                BedState {
                    gain: Gain::Silence,
                    importance: 1.,
                    ..spec.bed[channel]
                },
                AdmTiming {
                    start: None,
                    duration: Some(0),
                },
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(super) struct BedTimeline {
    pub slot: usize,
    pub channels: Vec<Channel>,
    pub start: u64,
    pub locked: u32,
    pub object_head: bool,
    pub private: Option<StaticChannelMetadata>,
    pub updates: u64,
}
impl BedTimeline {
    pub(super) fn project(
        &self,
        item: &stream::Block,
        losses: &mut Losses<'_>,
    ) -> Result<AdmBedBlock> {
        let scene = Scene {
            doc: &item.doc,
            scene: &item.scene,
            index: None,
        };
        let block = &item.scene.direct_blocks[0];
        let node = block.node;
        let doc = &item.doc;
        scene.attributes(
            node,
            &["audioBlockFormatID", "rtime", "duration"],
            None,
            losses,
        )?;
        scene.children(
            node,
            &[
                "speakerLabel",
                "position",
                "cartesian",
                "headLocked",
                "gain",
                "importance",
                "headphoneVirtualise",
            ],
            None,
            losses,
        )?;
        let mut output = AdmBedBlock {
            channel: self.slot,
            timing: AdmTiming {
                start: (block.source.rtime_present != 0).then_some(block.source.rtime_samples),
                duration: (block.source.duration_present != 0)
                    .then_some(block.source.duration_samples),
            },
            labels: scene.list(block.labels).map(str::to_owned).collect(),
            geometry: AdmMetadata::default(),
            gain: None,
            importance: None,
            head_tracking: HeadTracking::SceneRelative,
            headphone_virtualise: None,
        };
        let mut has_head = self.object_head;
        let children = doc.element_children(node);
        if children
            .iter()
            .any(|&n| doc.element_name(n) == Some("position"))
        {
            let geom = scene.metadata(node, crate::native::Level::BedGeometry, None, losses)?;
            if !geom.is_canonical_bed(metadata::bed_position(self.slot))? {
                output.geometry = geom;
            }
        }
        let mut seen = BTreeSet::new();
        for child in children {
            let name = doc.element_name(child).unwrap_or_default();
            let value = doc.element_text(child).unwrap_or_default().trim();
            if matches!(
                name,
                "gain" | "importance" | "headLocked" | "headphoneVirtualise"
            ) && !seen.insert(name)
            {
                return Err(Error::invalid(format!("Duplicate bed {name}")));
            }
            if matches!(
                name,
                "gain" | "importance" | "headLocked" | "headphoneVirtualise" | "speakerLabel"
            ) {
                scene.children(child, &[], None, losses)?;
            }
            match name {
                "gain" => {
                    scene.attributes(child, &["gainUnit"], None, losses)?;
                    let v = number(value, "bed gain")?;
                    output.gain = Some(match doc.element_attribute(child, "gainUnit") {
                        None | Some("linear") => Gain::Linear(v),
                        Some("dB") => Gain::Decibels(v),
                        _ => return Err(Error::invalid("Invalid bed gain unit")),
                    });
                }
                "importance" => {
                    scene.attributes(child, &[], None, losses)?;
                    output.importance = Some(
                        value
                            .parse()
                            .map_err(|_| Error::invalid("Invalid bed importance"))?,
                    );
                }
                "headLocked" => {
                    scene.attributes(child, &[], None, losses)?;
                    has_head = true;
                }
                "headphoneVirtualise" => {
                    scene.attributes(child, &["bypass", "DRR"], None, losses)?;
                    output.headphone_virtualise = Some(headphone(doc, child)?);
                }
                "speakerLabel" => scene.attributes(child, &[], None, losses)?,
                _ => {}
            }
        }
        output.head_tracking = if has_head {
            resolve_head(
                head(block.head_locked),
                self.private.map(|v| v.head_tracking),
                losses,
                None,
                Some(block.start),
                "bed_head_tracking",
            )?
        } else {
            self.private
                .map_or(HeadTracking::SceneRelative, |v| v.head_tracking)
        };
        output.validate()?;
        Ok(output)
    }
}
pub struct BedBlocks {
    pub(super) path: PathBuf,
    pub(super) tracks: std::vec::IntoIter<BedTimeline>,
    pub(super) rate: u32,
    pub(super) policy: LossPolicy,
    pub(super) current: Option<(BedTimeline, ProjectedBlocks)>,
    pub(super) failed: bool,
}
impl BedBlocks {
    fn read(&mut self) -> Result<Option<AdmBedBlock>> {
        loop {
            if let Some((t, blocks)) = &mut self.current
                && let Some(item) = blocks.next()?
            {
                return t
                    .project(
                        &item,
                        &mut Losses {
                            policy: self.policy,
                            diagnostics: &mut Vec::new(),
                        },
                    )
                    .map(Some);
            }
            let Some(t) = self.tracks.find(|t| t.updates != 0) else {
                return Ok(None);
            };
            let mut blocks =
                ProjectedBlocks::new(&t.channels, &self.path, self.rate, t.start, t.locked)?;
            blocks
                .next()?
                .ok_or_else(|| Error::invalid("Missing first ADM bed block"))?;
            self.current = Some((t, blocks));
        }
    }
}
impl Iterator for BedBlocks {
    type Item = Result<AdmBedBlock>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        match self.read() {
            Ok(v) => v.map(Ok),
            Err(e) => {
                self.failed = true;
                Some(Err(located(&self.path, e)))
            }
        }
    }
}
