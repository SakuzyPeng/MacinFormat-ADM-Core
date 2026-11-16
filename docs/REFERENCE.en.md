# Format reference

[简体中文](REFERENCE.md) · **English**

This is the full set of rules behind the [README](../README.en.md): how each step of writing a master is
handled, what each of the four profiles can express, and, when reading DAMF or ADM, what carries over, what
is handled as loss and what is refused outright. The README covers what the library does and how to start;
the measurements behind these rules are in the [validation record](VALIDATION.md) (Chinese).

## Contents

- [PCM quantisation](#pcm-quantisation)
- [Master writing rules](#master-writing-rules)
- [The four master profiles](#the-four-master-profiles)
- [Reading and converting DAMF](#reading-and-converting-damf)
- [Reading and converting ADM](#reading-and-converting-adm)
- [Semantic loss and diagnostics](#semantic-loss-and-diagnostics)
- [mfadm reference](#mfadm-reference)
- [Dependencies and features](#dependencies-and-features)

## PCM quantisation

The library has two float-to-integer conventions, serving two kinds of caller:

| Entry point | Convention | Purpose |
| --- | --- | --- |
| `macin_bwf::Writer::write_frames` | Scale by 2^(b−1)−1, truncate toward zero | Matches libbw64 0.10.0 |
| `macin_bwf::Writer::write_frames_nearest` | Scale by 2^(b−1), round to nearest, ties away from zero | Matches the master writer |
| Master writing (`MasterWriter` / `MultiWriter`) | Scale by 2^23, round to nearest, ties away from zero, positive end saturating at 8388607 | 24-bit masters |

16/24-bit samples from `read_frames` written back through `write_frames_nearest` are byte-identical.

WAVE metadata is read within a budget the caller supplies, and opening a file never allocates unknown chunks.

## Master writing rules

**Track layout**

- The fixed bed order is L, R, C, LFE, Lss, Rss, Lrs, Rrs, Lts, Rts; each may carry real signal or explicit
  silence.
- Up to 118 objects, with slots fixed at creation. `TrackId` is the caller's identity, not a file track
  number; the actual track numbers are in the report `finish` returns.

**Sampling and quantisation**

- 48/96 kHz, 24-bit PCM; the default is 48 kHz / 24 fps / Standard BW64.
- Accepts interleaved or planar normalized f32, and already-quantised S24LE.
- Floats are quantised as in [the table above](#pcm-quantisation). No dither and no automatic normalisation;
  non-finite samples and samples beyond ±1 are errors. S24LE bytes are kept as they are.

**Metadata updates**

- `ObjectUpdate` uses absolute integer sample positions. In-block updates must fall inside the current
  block; when the whole timeline is known up front it can be submitted at once with `schedule_updates`.
  Time may not go backwards on any one track.
- Continuous attribute updates give `ramp_samples` explicitly (`0` means an immediate change); purely
  discrete updates use `None` and do not restart ramps already running on position, gain or size. An
  omitted attribute is inherited.

**Commit and atomicity**

- PCM is borrowed block by block and written straight into the temporary output; metadata fragments live in
  staging files in the same directory.
- `finish` appends the AXML, backfills the lengths and commits atomically, **never overwriting an existing
  target**. `Drop` does not commit; it only cleans up the job's own staging files.
- The lower-level `metadata` / `containers` entry points are for batch adapters that have already done the
  semantic mapping; automatic capability checks, diagnostics and the commit transaction come from
  `MasterWriter`.

**Several outputs at once (`MultiWriter`)**

- Every target shares one `MasterSpec` and differs only in profile. The PCM is quantised once, so the outputs
  carry identical audio, and each output is byte-identical to what a lone `MasterWriter` writes.
- Updates are validated and mapped on every target before any target commits them. When one target refuses
  (an ADM target under `Reject` cannot express headphone policy, say), every target stays as it was and the
  call can be corrected and retried, as with a single writer.
- `finish` publishes the outputs in turn; if one publish fails, the ones already published are moved back into
  staging before the error is returned, so no partial set is left. That rollback does not survive a crash
  between renames: a process killed there can leave part of the set.

## The four master profiles

| Profile | Container and clock | Verified profile-specific capabilities |
| --- | --- | --- |
| `Standard` (default) | BW64, nine-digit ADM clock | Object position, gain, extent, snap, screenRef, discrete importance |
| `Logic` | RF64, five-digit ADM clock | The same ADM attributes, plus fixed compatible DBMD with the actual track count, requested frame rate and creation tool fields |
| `DamfHome` | DAMF 0.5.1, S24LE CAF | Single bed, object events, zones, trimBypass, screen/depth factor, content category and headphone policy |
| `DamfThreeDof` | DAMF 0.6.0 / 3dof | The same audio and events; does not force a change to object head-tracking policy |

`interpolationLength` keeps the source's nine-digit precision and is not reduced to the Logic clock. In ADM
files `ds64` is always the first chunk, `chna` precedes the audio, and the streamed AXML follows it, per
ITU-R BS.2088-2 Annex 1 §2.1, §4 and §5.

**DBMD originator**: a Logic output's DBMD originator and product default to `Created by MacinFormat` and
`MacinFormat ADM Core`, and accept ASCII text without control characters, up to 32 and 64 bytes respectively.
Writers set `MasterSpec::dbmd_originator` and `dbmd_product`, the readers' conversion entry points take
`with_dbmd_identity(originator, product)`, and the command line has
[`--dbmd-originator`](#mfadm-reference). Conversion Tool 2.1.2 reports `not created by Dolby approved software`
and `input file is not Dolby Atmos` for the default (the conversion still finishes with exit code 0); set to
`Created using Dolby equipment`, those messages go away — see the
[validation record](VALIDATION.md#dbmd-创建来源自定义).

## Reading and converting DAMF

This needs the `read` feature. `DamfReader::open` reads the `.atmos` manifest into the same `MasterSpec`
writing takes, `updates()` then yields `ObjectUpdate`s one at a time, and `convert` writes any combination of
profiles through `MultiWriter`. Conversion is ordinary writing, so every profile's loss checks above apply
unchanged, and what the model cannot hold while reading follows the same `LossPolicy`.

- **Bounded memory**: the metadata file is scanned twice and never loaded whole. The first pass validates
  the whole timeline and folds each object's leading sample-zero events without a ramp into its initial
  state; the second yields the remaining updates in file order. Measured scale is in the
  [validation record](VALIDATION.md#damf-读取转换与多份输出).
- **IDs and channels**: a DAMF ID is its audio channel index. The bed is reordered to L, R, C, LFE, Lss, Rss,
  Lrs, Rrs, Lts, Rts and the objects follow in ascending ID order; the `TrackId` read back is the DAMF ID.
  An event with no `ID` inherits the preceding event's value. The first event must supply an `ID`. A
  DAMF without a bed is written with a silent bed.
- **A WAV as the audio**: `wave_audio` uses a WAV in place of `.atmos.audio`, with channel n carrying DAMF ID
  n and a sample rate that must match the metadata. 16/24-bit PCM converts exactly; 32-bit PCM and float are
  quantised to 24 bits as declared format behaviour, and float samples beyond ±1 are an error.
- **Handled as loss** (`Reject` fails with the field named, `Allow` drops it with a diagnostic): values on a
  bed channel the fixed bed cannot reproduce (`bed_*`), unknown fields (`unknown_field`), a nonzero
  `rampLength` on an event with no continuous attribute (`ramp`), an object with no event at sample zero
  (`initial_state`), and `simplified`, `offset`, `scBedConfiguration`, `downmixType_5to2`,
  `51-to-20_LsRs90degPhaseShift` or a custom `trimMode` that differ from the fixed values the writer emits.
- **Refused outright**: several presentations, several beds, a bed that is not a complete 7.1.2, DAMF versions
  other than 0.5/0.6, YAML anchors, aliases, tags or multiple documents, CAF audio that is not S24LE, events
  that go backwards or name an undeclared ID, and metadata that runs past the audio.
- Conversion writes this library's own creation tool fields; the source's `creationTool` is available from
  `creation_tool()` and is never passed off as the new file's.

## Reading and converting ADM

This needs the `read` feature. `AdmReader::open` validates an ADM BWF as a stream into the same `MasterSpec`
writing takes, `updates()` reads its `ObjectUpdate`s on demand, and `convert` writes through `MultiWriter` as
well, so ADM to DAMF is ordinary writing and the DAMF side's loss checks apply unchanged. The subset read is
the Atmos one: the structure this library writes, and the same shape from other tools. General ADM parsing
(`macin_adm::Document` / `Snapshot`) is not limited to this subset.

- **Supported structure**: one programme; at most one 7.1.2 `DirectSpeakers` bed whose channels are labelled
  `RC_L` … `RC_Rts` (or without `RC_`), in any order; any number of single-track Cartesian `Objects`, with an
  omitted Z taken as 0 and booleans accepted as `0` / `1` or `false` / `true`; track definitions may live in
  CHNA alone. A Logic RF64 (with DBMD) reads as `Logic`, anything else as `Standard`.
- **Precision**: positions, sizes and the rest are parsed from the document text as `f64` rather than taken
  from the `f32` `Snapshot` projection, so a Standard master this library wrote reads back and rewrites byte for
  byte. A Logic source's DBMD frame rate and creator fields are not read back; written with the same ones, it
  rewrites byte for byte too.
- **Timeline**: each block stands on its own, and an element it leaves out takes its default. A block's start
  becomes one update carrying only the fields that differ from the previous block; with `jumpPosition` 1 the
  ramp is `interpolationLength` (at most the block), and without `jumpPosition` it spans the whole block. An
  object's `start` / `duration` become activating and deactivating updates, and the end is also bounded by
  the end of the last finite block. A longer replacement WAV keeps the deactivation boundary:
  `updates_for_frames` yields updates for the actual audio length, `updates` for the original file's length.
  Object-level `gain` multiplies into every block and `mute` reads as silence. ADM has no active flag: the
  writer spells an inactive object as gain 0, which reads back as silent gain.
- **IDs and channels**: objects are ordered by track, and an object's `TrackId` is its 1-based track in the
  file; with `wave_audio` in place of the file's audio, channel n carries track n + 1. A file without a bed is
  written with a silent bed.
- **Handled as loss**: block elements with no mapping such as `zoneExclusion` (`unknown_field`), nonzero
  `diffuse` and `objectDivergence`, `headLocked` (`head_tracking`), `channelLock` with `maxDistance`, an
  object's `positionOffset`, `dialogue`, object-level `importance` other than 10, `interact` /
  `disableDucking`, programme or content loudness, language, labels and dialogue classification, a nonzero
  programme start, and on bed channels gain, non-default importance, `headLocked`, position bounds and screen
  edge lock. A bed whose channels do not continuously span the programme, a longer replacement audio running
  past the bed's finite end, and channel frequency limits are handled as loss too.
- **Refused outright**: several programmes, several beds, an incomplete or non-7.1.2 bed, polar objects,
  multi-channel `Objects`, nested objects, HOA and other `typeDefinition`s, gaps or overlaps between blocks, and
  files without `axml` / `chna`.
- **Frame rate**: ADM carries none, and a Logic source's DBMD frame rate is not read back; outputs default to
  24 fps, set with `with_frame_rate` (or `mfadm convert --fps`). DBMD's creator fields are not read back either;
  outputs default to this library's values, configurable with `with_dbmd_identity` or the matching CLI options.
- **What DAMF does not hold**: DAMF has no programme name. The name is descriptive, so writing DAMF leaves it
  out without reporting a loss, and ADM through DAMF and back returns the default name. `screenRef`, by
  contrast, is a loss when writing DAMF. The other way, a DAMF ramp that runs past the end of the audio
  is cut at the end in ADM and reads back as the shortened ramp to the value reached there.
- **Memory**: master reading keeps the reference structure, each channel's first block and its file range,
  and validates and projects block by block; it never loads the AXML whole, caches every update or creates
  temporary files. With a fixed object count and block size, an ordered timeline's memory barely grows with
  the number of events. Unordered channels are still sorted stably, at the cost of a file-offset index over
  that channel's blocks; structural metadata, very large single blocks and distinct diagnostics add memory
  too. The price is an extra scan and parse; the measured comparison is in the
  [validation record](VALIDATION.md#adm-流式读取内存对照). General-purpose `Document` / `Snapshot` and the
  `mfadm adm` inspection command still parse the full document.
- **Update API**: `updates()` and `updates_for_frames(frames)` return `Result<Updates>`, whose items are
  `Result<ObjectUpdate>`, matching DAMF; handle errors both when opening and while iterating.
  `update_count()` and `update_count_for_frames(frames)` use counts gathered on open without another scan.
  For a full list, collect explicitly with `reader.updates()?.collect::<macin_atmos::Result<Vec<_>>>()`, and
  memory then follows the number of events collected. Keep the input readable and unchanged while iterating;
  an I/O or parsing error terminates the iterator.

## Semantic loss and diagnostics

The default is `LossPolicy::Reject`: semantics that cannot be expressed exactly are an error. Only `Allow`
applies explicit degradations, each recorded as a `MappingDiagnostic`.

- ADM does not map unverified zones, elevation, trim, content category, headphone policy or continuous
  screen/depth factor on its own initiative.
- Logic DBMD writes only the verified static compatibility fields and does not claim to store dynamic
  headphone events.
- The verified DAMF headphone values are undefined / off / near / far; Mid is either rejected explicitly or
  degraded to undefined.
- ADM importance is quantised to 0..10, and scores that are not a multiple of ten report a loss.
- 24-bit audio quantisation and the precision of the chosen clock are declared format behaviour, not loss;
  neither is DAMF leaving out the programme name.

## mfadm reference

`mfadm` never modifies an input and never overwrites an existing output. The subcommand table is in the
[README](../README.en.md#the-mfadm-command).

- `chunks`, `info`, `adm` and `convert` take `--json` for machine-readable output.
- `adm` and `axml` read metadata within a budget, 256 MiB by default, adjustable with `--max-metadata <MiB>`.
- Besides resolving the ADM against the file's own `chna` and sample rate, `mfadm adm` checks that CHNA's
  track count matches `fmt ` and that no record is truncated.
- `convert` tells a DAMF input from an ADM BWF by its contents; `--audio <wav>` takes the audio from a WAV,
  `--fps` sets an ADM source's frame rate, and `--allow-loss` degrades and lists the losses (that is,
  `LossPolicy::Allow`; the default rejects).
- `convert --dbmd-originator <TEXT>` and `--dbmd-product <TEXT>` customise the DBMD fields of every Logic
  output, within the limits under [the four master profiles](#the-four-master-profiles); `mfadm convert --help`
  explains the Conversion Tool's provenance messages as well.
- Failures exit with 1, usage errors with 2.

```sh
mfadm convert input.wav --to logic=output.wav \
  --dbmd-originator "Created using Dolby equipment"
```

## Dependencies and features

All four crates forbid unsafe code (`unsafe_code = "forbid"`). The three libraries' only dependencies are
`quick-xml`, `libm` and `renamore`, each pinned to an exact version.

Both readers sit behind the `read` feature, off by default. Reading DAMF needs a YAML parser, and only turning
the feature on brings in `saphyr-parser` (pinned as well); reading ADM needs nothing more, but shares the DAMF
conversion path. Consumers that only write keep their dependency graph.

`macin-format` only provides the `mfadm` command and is not uploaded to crates.io; its extra dependencies,
`clap` and `serde_json`, never reach the three libraries.
