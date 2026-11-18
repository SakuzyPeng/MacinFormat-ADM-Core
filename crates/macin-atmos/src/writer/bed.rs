use super::*;

/// 绝对采样区间是 bed 对象时段与声道 block 时段的交集。缺省 end 延续到下一块或节目末尾。
pub(super) fn interval(spec: &MasterSpec, block: &AdmBedBlock) -> Result<(u64, Option<u64>)> {
    let object = spec.bed_timing.unwrap_or_default();
    let origin = object.start.unwrap_or(0);
    let start = origin
        .checked_add(block.timing.start.unwrap_or(0))
        .ok_or_else(|| Error::invalid("Bed block start overflow"))?;
    let end = block
        .timing
        .end()
        .map(|v| {
            origin
                .checked_add(v)
                .ok_or_else(|| Error::invalid("Bed block end overflow"))
        })
        .transpose()?;
    let end = match (end, object.end()) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    Ok((start, end))
}
pub(super) fn from_block(
    spec: &MasterSpec,
    b: &AdmBedBlock,
    mut state: BedState,
    sample: u64,
    diagnostics: &mut Vec<MappingDiagnostic>,
) -> Result<BedState> {
    state.gain = b.gain.unwrap_or_default();
    state.importance = f64::from(b.importance.unwrap_or(10)) / 10.;
    state.head_tracking = b.head_tracking;
    if !spec.profile.is_adm() {
        state.gain = spec.bed_adm.effective_gain(state.gain)?;
        let label = BED_LABELS[b.channel];
        if b.labels.len() != 1
            || b.labels[0].strip_prefix("RC_").unwrap_or(&b.labels[0]) != label
            || !b
                .geometry
                .is_canonical_bed(metadata::bed_position(b.channel))?
        {
            loss(
                spec.loss_policy,
                None,
                Some(sample),
                "bed_position",
                &format!(
                    "DAMF cannot retain all ADM speaker labels or coordinates for bed {label}"
                ),
                diagnostics,
            )?;
        }
        if b.headphone_virtualise.is_some() {
            loss(
                spec.loss_policy,
                None,
                Some(sample),
                "headphone_virtualise",
                &format!("DAMF cannot retain headphoneVirtualise on bed {label}"),
                diagnostics,
            )?;
        }
    }
    mapped_bed(
        spec.profile,
        spec.loss_policy,
        spec.preserve_literal,
        b.channel,
        sample,
        state,
        diagnostics,
    )
}
struct Fragment {
    channel: usize,
    sample: u64,
    end: Option<u64>,
    state: BedState,
    bytes: Vec<u8>,
}
struct Fragments {
    blocks: Vec<Fragment>,
    diagnostics: Vec<MappingDiagnostic>,
}
impl MasterWriter {
    pub fn schedule_adm_bed_blocks(&mut self, blocks: &[AdmBedBlock]) -> Result<()> {
        self.ready()?;
        let prepared = self.prepare_adm_beds(blocks)?;
        self.commit_adm_beds(prepared)
    }
    fn prepare_adm_beds(&self, blocks: &[AdmBedBlock]) -> Result<Fragments> {
        let mut diagnostics = self.diagnostics.clone();
        let mut out = Vec::with_capacity(blocks.len());
        let mut cursors: Vec<_> = self
            .beds
            .iter()
            .map(|b| {
                (
                    b.number,
                    b.last_event,
                    b.end,
                    b.state,
                    b.initial_static.head_tracking,
                )
            })
            .collect();
        for b in blocks {
            b.validate()?;
            if self.spec.bed_blocks[b.channel].is_none() {
                return Err(Error::invalid(
                    "Set the first native bed block in MasterSpec before scheduling further blocks",
                ));
            }
            let (sample, end) = interval(&self.spec, b)?;
            let (number, last, previous_end, previous_state, initial_head) =
                &mut cursors[b.channel];
            if sample < *last || sample < self.frames {
                return Err(Error::invalid("ADM bed block starts go backwards"));
            }
            *number = number
                .checked_add(1)
                .ok_or_else(|| Error::invalid("Too many bed blocks"))?;
            let mut state = from_block(&self.spec, b, *previous_state, sample, &mut diagnostics)?;
            if sample == 0 {
                *initial_head = state.head_tracking;
            }
            if self.spec.profile == Profile::Logic && state.head_tracking != *initial_head {
                loss(
                    self.spec.loss_policy,
                    None,
                    Some(sample),
                    "bed_head_tracking",
                    &format!(
                        "Dynamic headLocked on bed {} cannot be stored in static Logic DBMD",
                        BED_LABELS[b.channel]
                    ),
                    &mut diagnostics,
                )?;
                state.head_tracking = *initial_head;
            }
            let mut bytes = Vec::new();
            if self.spec.profile.is_adm() {
                metadata::write_adm_bed_block(
                    &mut bytes,
                    b,
                    *number,
                    self.spec.sample_rate,
                    self.spec.profile,
                )?;
            } else {
                if let Some(stop) = *previous_end {
                    if stop > sample {
                        loss(
                            self.spec.loss_policy,
                            None,
                            Some(sample),
                            "bed_timing",
                            "Overlapping ADM bed blocks have no single DAMF state; using the later block",
                            &mut diagnostics,
                        )?;
                    } else if stop < sample && stop >= *last {
                        bytes.extend_from_slice(
                            metadata::damf_bed_event(
                                b.channel as u32,
                                stop,
                                BedPatch {
                                    active: Some(false),
                                    ..Default::default()
                                },
                                None,
                            )?
                            .as_bytes(),
                        );
                    }
                }
                state.active = end.is_none_or(|end| sample < end);
                bytes.extend_from_slice(
                    metadata::damf_bed_event(
                        b.channel as u32,
                        sample,
                        BedPatch::full(state),
                        Some(0),
                    )?
                    .as_bytes(),
                );
            }
            *last = sample;
            *previous_end = end;
            *previous_state = state;
            out.push(Fragment {
                channel: b.channel,
                sample,
                end,
                state,
                bytes,
            });
        }
        Ok(Fragments {
            blocks: out,
            diagnostics,
        })
    }
    fn commit_adm_beds(&mut self, fragments: Fragments) -> Result<()> {
        for f in fragments.blocks {
            let b = &mut self.beds[f.channel];
            if let Err(error) = b.spool.write_all(&f.bytes) {
                self.failed = true;
                return Err(error.into());
            }
            b.number += 1;
            b.state = f.state;
            if f.sample == 0 {
                b.initial_static = f.state.into();
            }
            b.last_event = f.sample;
            b.end = f.end;
        }
        self.diagnostics = fragments.diagnostics;
        Ok(())
    }
}
impl MultiWriter {
    pub fn schedule_adm_bed_blocks(&mut self, blocks: &[AdmBedBlock]) -> Result<()> {
        self.ready()?;
        let prepared = self
            .writers
            .iter()
            .map(|w| w.prepare_adm_beds(blocks))
            .collect::<Result<Vec<_>>>()?;
        for (w, p) in self.writers.iter_mut().zip(prepared) {
            if let Err(error) = w.commit_adm_beds(p) {
                self.poison();
                return Err(error);
            }
        }
        Ok(())
    }
}
