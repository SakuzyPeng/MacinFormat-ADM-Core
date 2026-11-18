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
`metadata_at` addresses a chunk by table index, including different LIST subtypes that share a FourCC.

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
- Continuous position, gain, extent and native diffuse / divergence updates give `ramp_samples` explicitly
  (`0` means an immediate change); purely discrete updates use `None` and do not restart other ramps. An
  omitted attribute is inherited.
- `MasterSpec::bed` holds the ten canonical initial channel states. `BedUpdate::channel` is the zero-based
  index in `BED_LABELS`, independent of object `TrackId`s; submit it with `schedule_bed_updates`. Gain and DAMF bed position/extent
  updates require a ramp. Discrete updates leave ongoing ramps intact. State updates must fall within the half-open bed object interval. An update at the start sets the first block’s effective state. Each call checks sample order, the object interval and target clock limits before writing. `MultiWriter` checks every target first.
- An empty `MasterSpec::contents` produces the default single group. Custom groups must reference every
  object and the fixed bed; an object may belong to several groups, but a group cannot repeat or name an
  unknown object. The generated content ID range holds at most 0xefff groups. `ObjectSpec::pack_name` / `channel_names` and `bed_channel_names` retain independent format
  names, using the usual generated names when unspecified. The shared `macin_adm::records` layouts do not change.

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
| `Standard` (default) | BW64, nine-digit ADM clock | Object position, gain, extent, snap, screenRef, discrete importance, native headLocked, diffuse / divergence / maxDistance / screenEdgeLock, zone presets and elevation; layered ADM metadata and programme offset |
| `Logic` | RF64, five-digit ADM clock | The same position and zone attributes; DBMD for static head tracking, headphone mode, trimBypass, frame rate and the presentation settings below |
| `DamfHome` | DAMF 0.5.1, S24LE CAF | Single-bed states and events, object events, zones, trimBypass, screen/depth factor, content category and headphone policy |
| `DamfThreeDof` | DAMF 0.6.0 / 3dof | The same audio and events; does not force a change to object head-tracking policy |

`interpolationLength` keeps the source's nine-digit precision and is not reduced to the Logic clock. In ADM
files `ds64` is always the first chunk, `chna` precedes the audio, and the streamed AXML follows it, per
ITU-R BS.2088-2 Annex 1 §2.1, §4 and §5.

**Native parameters and target limits**

- `HeadTracking` supports SceneRelative, HeadRelative and Undefined. Standard expresses the first two with
  XML `headLocked`, including object changes over time. Undefined has no boolean equivalent and follows the
  loss policy. Logic expresses all three static values in DBMD and does not write XML head locking.
- Logic takes head tracking, headphone mode and trimBypass from the effective state at sample zero,
  including sample-zero updates submitted before audio. Later changes fail under Reject; Allow keeps the
  initial value and reports the loss. Standard has no exact headphone-mode or trimBypass mapping.
- The default binaural distance for objects and non-LFE bed channels is Mid. Bed LFE is fixed to Off. In Logic DBMD segment 10 the low three binaural mode bits distinguish 3 (explicit Mid) from 4 (not indicated, defaulting to Mid on ordinary channels). The library preserves both static codes. [Logic Pro guide](https://support.apple.com/guide/logicpro/lgcp789f000d/mac), [MediaInfo DBMD mode table](https://github.com/MediaArea/MediaInfoLib/blob/v26.01/Source/MediaInfo/Audio/File_DolbyAudioMetadata.cpp#L52-L59)
- ADM beds carry object gain/mute and an independent time span. Each DirectSpeakers block can carry gain, integer importance (0–10), its full speaker label list, geometry and headphoneVirtualise. Standard can change headLocked per block. Logic stores static head tracking, headphone mode and trimBypass in DBMD.
- DAMF beds retain active, gain, importance, head tracking, headphone mode and trim, plus explicit position, extent, snap, zone, elevation, decorrelation, dialog, music and screen/depth factors in `BedRendering`. Updates include resets and switches back to false. ADM targets report unsupported rendering fields individually as `bed_*`. Under Allow an inactive channel is rendered as zero metadata gain. A nonzero DAMF bed ramp follows `bed_ramp` on ADM output. Allow writes the target value.
- `MasterSpec::start_sample` is the external programme offset; object and bed events remain relative to
  audio sample zero. ADM writes programme start/end and DAMF writes offset. Nonnegative input seconds are
  rounded to the nearest sample.
- Logic/DAMF retain `sc_bed_configuration`, `downmix_mode`, `surround_phase_shift`, `bed_distribution`,
  `warp_mode` and `trim_mode`. Configuration codes are unique values in 0..=8; an empty model set means
  unspecified DBMD and can be retained in Logic. Missing or empty DAMF input lists mean `[3]`, so DAMF cannot
  express that unspecified state: Reject refuses it and Allow diagnoses it and uses `[3]`.
  Downmix accepts `LoRo_Stereo`, `LtRt_ProLogic`, `LtRt_PLII` and `directRender`. Warp accepts LoRo, ProLogicIIx, warping and normal. `WarpMode::None` remains an alias for Normal.
- Omitting warp, downmix or the 90° phase shift in DAMF maps to DBMD codes 4, 0 and 3 respectively. The first two use `NotIndicated`. Use `set_surround_phase_shift(Option<bool>)` for phase shift. DAMF omits unspecified keys and writes explicit settings. Newly authored masters retain the LoRo, LoRo and explicit-off defaults.
- `first_action_ns` holds the first frame of action (FFOA) in signed integer nanoseconds, independently of programme start. DAMF uses seconds for `ffoa`, preserves explicit zero and omits an unspecified value. Logic stores hours, minutes and 1/100,000 seconds, rounding to the nearest 10 µs. A set high bit in the hour byte indicates an unspecified time, encoded as 0xff on creation. An out-of-range value or a Standard target follows the `ffoa` loss policy.
- `bed_distribution_present` distinguishes unspecified from explicit false or true. The default is explicit false. Use `set_bed_distribution(Option<bool>)` to set both fields. DAMF omits an unspecified setting and DBMD keeps its native state.
- `trim_mode` has nine slots in `TrimConfiguration::ALL` order. The original indices correspond to 5.1, 5.1.2, 5.1.4 and 7.1. The additional slots are 2.0, 2.1.2, 7.1.2, 2.1.4 and 7.1.4. New masters use empty manual trims in the first four slots and automatic trims elsewhere. Omitted DAMF slots are automatic. The three gains use finite f32 values. Both front/back balances range from −1 to 1. The low seven bits store the nearest integer to 127 times the absolute value. A high bit of zero indicates negative and one indicates positive. Both zero codes are accepted. New zeros use 0x80 and native rewrites retain the source zero code.
- DBMD channel trimBypass code 2 uses `trim_bypass_unspecified` on beds and objects. An explicit boolean update clears it. DAMF and Standard targets follow the `trim_bypass` or `bed_trim_bypass` policy, using false when loss is allowed.
- Standard cannot carry non-default DBMD presentation settings and reports them by field. Unspecified
  bedDistribution and automatic trims differ from its explicit-false / empty-manual defaults and also report
  a loss. Neither ADM profile maps screen_size_ratio. External checks of static channels, presets and presentation settings are
  in the [validation record](VALIDATION.md).

**DBMD originator**: a Logic output's DBMD originator and product default to `Created by MacinFormat` and
`MacinFormat ADM Core`, and accept ASCII text without control characters, up to 32 and 64 bytes respectively.
Writers set `MasterSpec::dbmd_originator` and `dbmd_product`, the readers' conversion entry points take
`with_dbmd_identity(originator, product)`, and the command line has
[`--dbmd-originator`](#mfadm-reference). Conversion Tool 2.1.2 reports `not created by Dolby approved software`
and `input file is not Dolby Atmos` for the default (the conversion still finishes with exit code 0); set to
`Created using Dolby equipment`, those messages go away — see the
[validation record](VALIDATION.md#dbmd-创建来源自定义).

## Native ADM metadata and ancillary chunks

Recognised fields stay in the input model. Each output applies its own `LossPolicy`, so an ADM output can
retain native data while a DAMF output independently approximates or reports a loss.

- `ObjectState::diffuse` is separate from `decorrelation` and ranges over 0..=1. `ObjectDivergence` holds
  value and an optional Cartesian position_range (0..=1). The reader can also retain a legacy polar
  azimuth_range (0..=180). Writing that angle to Cartesian ADM follows `divergence_azimuth_range`. Allow
  retains the divergence value and drops the angle. Explicit literal preservation is described below. Supplying both ranges is invalid. `channel_lock_distance` holds
  maxDistance: None means unspecified and explicit values range over 0..=2√3. `screen_edge_lock` accepts
  Left/Right on X and Top/Bottom on Z, with no lock on Y. Both ADM profiles retain these native fields and
  updates, with new numeric fields written as round-trippable f64 decimal values. Missing block values reset
  to zero, None, None and no edge lock. Diffuse / divergence have
  independent ramps. A decorr-to-diffuse approximation copies the value at the event even when that event
  also starts a position ramp. Each AXML block has one interpolation time which a renderer may apply.
- `AdmMetadata` stores attributes and `AdmElement` subtrees at their original level: `MasterSpec::adm`
  for programme, `ContentSpec::adm` for content, `ObjectSpec::adm` / `pack_adm` / `channel_adm` for object,
  pack and channel, and `bed_adm` / `bed_pack_adm` / `bed_channel_adm` for the bed. Object and pack importance
  stay separate from block importance. Object gain, mute and positionOffset remain on the object. The imported `ObjectState::gain` and gain updates are the block’s own values. `ObjectSpec::adm.effective_gain(block_gain)` computes the combined gain. DAMF output combines the layers once at write time and retains the PCM samples.
- `MasterSpec::bed_timing` holds the bed start relative to the programme and optional duration. `bed_blocks[channel]` holds the first `AdmBedBlock`. Its `timing.start` is relative to the **bed object**. Stream subsequent blocks through `AdmReader::bed_blocks()` and `schedule_adm_bed_blocks`. A channel with a native first block continues with this interface. State-based authoring uses `bed_geometry` and `schedule_bed_updates`. Both interfaces work for single and multiple outputs. ADM retains each block’s duration, gaps, overlaps, fallback coordinates, bounds, screenEdgeLock and label order. DAMF represents start, end and gaps with per-channel active events. A block with omitted duration extends to the end of the bed object. Overlapping blocks follow `bed_timing` and Allow takes the later block. Multiple labels and noncanonical speaker coordinates follow `bed_position` on DAMF output.
- `ObjectSpec::custom_zones` holds initial custom exclusions. `native_updates()` / `native_updates_for_frames` and `schedule_native_updates` carry a `NativeObjectUpdate` with the numeric event and optional zone list. None inherits, an empty list clears. `updates()` remains the numeric projection and a zones-only event may have an empty numeric patch. Use the native update API for a full round trip. Six Cartesian bounds or four polar bounds, labels and zone order are retained. Combining a custom list with a preset or independent elevation restriction is an error. The batch `metadata::build_axml` API applies each object’s custom zones and `MasterSpec::preserve_literal` to its blocks.
  Object and bed blocks retain the optional bypass and DRR (−130..130 dB) in headphoneVirtualise. Custom zones and this headphone property follow `zone` and `headphone_virtualise` respectively on DAMF output.
- Recognised static fields are programme/content language, labels and loudnessMetadata; programme reference
  screen and maxDuckingDepth; object labels, positionOffset, importance, dialogue, interact, disableDucking
  and audioObjectInteraction ranges; pack importance; and channel frequency. The reference screen uses the
  aspectRatio attribute on output. Its legacy child form is also accepted, with conflicting values rejected.
  The legacy channelFrequency element is accepted and written as frequency. Text numbers retain their
  original precision and output is XML-escaped. Only recognised attributes and children are accepted, with
  no arbitrary XML or ID references injected through extensions. Unknown extensions still follow the loss
  policy. Public writing APIs validate numbers, enumerations, geometry and duplicate singleton fields.
- DAMF reports these static fields as `language`, `labels`, `loudness`, `reference_screen`, `position_offset`,
  `importance`, `pack_importance`, `dialog`, `interaction`, `ducking` or `frequency`; native block losses use
  `divergence`, `channel_lock_distance` and `screen_edge_lock`. Selecting diffuse approximation does not
  authorise those losses. Explicit object/pack importance=10 may be omitted as the default without loss. Replacing audio or changing the original programme duration invalidates source
  ADM loudness metadata: Reject fails and Allow clears it with a `loudness` diagnostic.
- `ContainerMetadata` retains WAVE bext, iXML, LIST/INFO and CAF info, with a total 4 MiB payload budget and
  no PCM cache. Byte preservation requires the same container family, original S24LE audio, channel count
  and order, sample rate, programme start and frame count. Changed conditions, WAVE/CAF conversion,
  excess payloads and other ancillary chunks follow `container_metadata`; Allow drops unsuitable chunks
  with diagnostics. Source and replacement container metadata follow the loss policy on audio replacement.
  Only the old CAF chunk headers need to be inspected, and a replacement WAV can stand in for a missing CAF.
  Callers directly submitting PCM to
  a writer or authoring payloads through `ContainerMetadata::new` are responsible for content consistency.
  Chunks may move after data; outer lengths and alignment are regenerated. Duplicate supported ancillary
  chunks are invalid input.

Field definitions are described by [ITU-R BS.2076-2, Tables 12, 17 and 39](https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.2076-2-201910-S%21%21PDF-E.pdf), [EBU Objects](https://adm.ebu.io/guidelines/reference/adm_elements/audio_channel_format_objects.html),
[programme](https://adm.ebu.io/guidelines/reference/adm_elements/audio_programme.html) and
[object](https://adm.ebu.io/guidelines/reference/adm_elements/audio_object.html).
Native preservation does not imply Dolby Conversion Tool will use or retain the field. Library round trips
and external-tool behaviour are recorded separately in [validation](VALIDATION.md). Dynamic DBMD limits remain unchanged.

## Reading and converting DAMF

This needs the `read` feature. `DamfReader::open` reads the `.atmos` manifest into the same `MasterSpec`
writing takes, `updates()` and `bed_updates()` yield `ObjectUpdate`s and `BedUpdate`s, and `convert` writes any combination of
profiles through `MultiWriter`. Conversion is ordinary writing, so every profile's loss checks above apply
unchanged, and what the model cannot hold while reading follows the same `LossPolicy`.

- **Bounded memory**: metadata is never loaded whole. The first pass validates the timeline and folds the
  leading sample-zero, zero-ramp events of objects and bed channels into their initial states. Object
  updates require another scan, and bed updates require an additional scan only when present. Each stream
  follows file order. `update_count()` counts objects; `bed_update_count()` counts beds separately. Measured scale is in the
  [validation record](VALIDATION.md#damf-读取转换与多份输出).
- **IDs and channels**: a DAMF ID is its audio channel index. The bed is reordered to L, R, C, LFE, Lss, Rss,
  Lrs, Rrs, Lts, Rts and the objects follow in ascending ID order; the `TrackId` read back is the DAMF ID.
  An event with no `ID` inherits the preceding event's value. The first event must supply an `ID`. A
  DAMF without a bed is written with a silent bed.
- **A WAV as the audio**: `wave_audio` uses a WAV in place of `.atmos.audio`, with channel n carrying DAMF ID
  n and a sample rate that must match the metadata. 16/24-bit PCM converts exactly; 32-bit PCM and float are
  quantised to 24 bits as declared format behaviour, and float samples beyond ±1 are an error.
- **Read-time losses** include unknown keys (`unknown_field`), a nonzero ramp without a continuous property (`ramp` / `bed_ramp`), missing sample-zero object state (`initial_state`), simplified=true and unidentified trim settings (`trim_mode`). A bed without an initial event uses its canonical channel defaults.
- An optional matching `.atmos.dbmd` file holds YAML encoding settings. `ddplus.compr1` and `ddplus.dynrng1` map to programme zero's RF and Line compression in DD+ segment 7. The six portable values are none, film_standard, film_light, music_standard, music_light and speech. Non-default settings generate a sidecar on Logic-to-DAMF conversion. A native rewrite retains source text and unmapped keys with matching audio, layout and compression settings. When the text cannot be retained, `dbmd_sidecar` governs the loss and Allow keeps the mapped compression settings.
- **Container metadata**: CAF headers before and after data are scanned. info follows the
  [ancillary preservation rules](#native-adm-metadata-and-ancillary-chunks); other non-desc/data/free chunks
  follow `container_metadata`. Only supported payloads within budget are loaded. Bad sizes and duplicate
  data chunks are rejected outright.
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
  `contents` retains content groups, names, object membership and dialogue classification. Independent
  pack/channel/stream/track names are retained too. A missing bed is still filled with ten silent channels
  and that silent bed is added to the first content.
- **Precision**: positions, sizes and the rest are parsed from the document text as `f64` rather than taken
  from the `f32` `Snapshot` projection, so within the default exact subset a Standard master this library wrote
  reads back and rewrites byte for byte. Logic recovers recognised DBMD fields; creator identity still uses
  this library's values or explicit overrides.
- **Timeline**: each block stands on its own, and an element it leaves out takes its default. A block's start
  becomes one update carrying only the fields that differ from the previous block; with `jumpPosition` 1 the
  ramp is `interpolationLength` (at most the block), and without `jumpPosition` it spans the whole block. An
  object's `start` / `duration` become activating and deactivating updates, and the end is also bounded by
  the end of the last finite block. A longer replacement WAV keeps the deactivation boundary:
  `updates_for_frames` yields updates for the actual audio length, `updates` for the original file's length.
  Block start and end times are rounded to sample positions separately. Their difference gives the block duration. Object-level gain and mute remain in object metadata. Block gain is independent. ADM has no active flag: the
  writer spells an inactive object as gain 0, which reads back as silent gain.
- **Programme span**: programme start becomes start_sample. An explicit end must follow start, and its
  duration must not exceed the audio. `audio()` delivers only the original programme duration, so conversion
  no longer stretches end to the physical file length. An explicitly substituted WAV still sets output
  length while preserving finite source programme, object and bed end boundaries. The bed object and its blocks are clipped to the intersection of the source programme and the output audio. Blocks starting at or after the programme end are skipped. A channel entirely outside the programme receives a silent zero-duration block. Shorter replacement audio must still cover the start of each in-programme block.
- **Head locking and zones**: object headLocked supplies the block default, which a block can override.
  An omitted value returns to the object default. A match on every preset name, bound and combination uses that preset, including the ZB/ZT elevation pair. Other complete zones retain their native values. An omitted zoneExclusion resets to All / elevation=true and clears the custom list.
- **DBMD** reads static channel metadata by source channel and remaps it with the PCM. Conflicting XML head locking fails under Reject. Allow keeps DBMD with a diagnostic. Verified segment layouts for versions 1.0.0.5 through 1.0.0.8 are supported. Logic preserves the source version and writes 1.0.0.6 for a new chunk. Segment 9 stores the frame rate in the low nibble. Logic preserves the original upper nibble. Unknown versions report `dbmd`. Unknown frame rate, head tracking, binaural, warp, downmix or invalid phase shift values use their field-specific diagnostic.
- A WAVE file with multiple `dbmd` chunks is invalid.
- `dbmd_source` retains native segment order. Logic preserves EBU Dolby E (1), Dolby Digital (3), DD+ (7), Audio Info (8), other opaque payloads and reserved bytes. Different programmes can have repeated segment types. Each of types 1, 9 and 10 must be unique. One alignment byte may follow the terminator. A zero size code represents 65,535 bytes and the checksum uses the low byte of the encoded size. Invalid checksums, truncation and malformed lengths are rejected. Segment 10 may have an extension tail. Its channel count must match the source or be zero.
- Layout-dependent opaque encoding data requires the original audio, channel order, rate, programme start and duration. When a target cannot retain the native payload, Reject fails and Allow reports `dbmd_encoding` and retains the mapped fields. The model updates compression profiles, per-channel settings and automatic/manual trims. The DBMD chunk and the legacy DAMF sidecar each have a 1 MiB limit.
- **IDs and channels**: objects are ordered by track, and an object's `TrackId` is its 1-based track in the
  file; with `wave_audio` in place of the file's audio, channel n carries track n + 1. A file without a bed is
  written with a silent bed.
- **Read-time losses** include unmapped elements and attributes (`unknown_field`, including extension attributes and children of content `dialogue`), incomplete or unrecognised zone geometry (`zone`), object position bound (`position_bound`) and multiple or nested pack references (`pack_structure`). Complete native fields are checked against each target on output. Ancillary chunks follow the container rules above.
- A stream can explicitly reference its track's assigned pack. The ADM writer rebinds the reference to the generated pack ID. Repeated, dangling or conflicting references follow `pack_structure`.
- **Track extension attributes** live in `ChannelNames::extensions` under stream, track and uid. ADM writes each attribute on its owner with its prefix, namespace URI, local name and value, and declares known prefixes used inside that value. Tabs and line breaks use XML character references. Vendor-specific identifiers and relative paths in values remain opaque text. DAMF output reports `track_extensions`.
- **Refused outright**: several programmes, several beds, an incomplete or non-7.1.2 bed, polar objects,
  multi-channel `Objects`, nested objects, HOA and other `typeDefinition`s, gaps or overlaps between blocks, and
  files without `axml` / `chna`.
- **Frame rate**: ADM XML carries none. Recognised DBMD supplies the frame rate, otherwise the default is
  24 fps; override it with `with_frame_rate` (or `mfadm convert --fps`). DBMD's creator fields are not inherited;
  outputs default to this library's values, configurable with `with_dbmd_identity` or the matching CLI options.
- **What DAMF does not hold**: DAMF has no programme name. The name is descriptive, so writing DAMF leaves it
  out without reporting a loss. A single content's name and independent format names are also omitted as
  descriptive information; conversion back uses generated names. Multiple content groups follow
  `content_structure`, and non-default content dialogue classification follows `dialog`; Mixed/0 may be
  omitted. `screenRef` is a loss by default when writing DAMF. `diffuse` and `screenRef` can opt into the individual
  [approximations below](#semantic-loss-and-diagnostics). The other way, a DAMF ramp that runs past the end of the audio
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

The default is `LossPolicy::Reject`: semantics that cannot be expressed exactly and have not been explicitly
approved for approximation are an error. `Allow` permits the declared degradations of the remaining fields.
`ApproximateMappings` enables approximations individually, all disabled by default. A selected approximation
can run under `Reject`; it does not permit losses in other fields or change the PCM.

| Switch / Rust field | Direction and rule |
| --- | --- |
| `--approximate diffuse-decorrelation` / `diffuse_decorrelation` | Keep native `diffuse` on read; map it to `decorrelation` / `decorr` only for DAMF output, or map `decorrelation` to `diffuse` for ADM output. Copy the numeric value and preserve integer sample timestamps as discrete updates; missing or reset values remain zero |
| `--approximate screen-ref-factor` / `screen_ref_factor` | Write ADM `screenRef` false/true as DAMF `screenFactor` 0/1. Write DAMF `screenFactor` zero as ADM false and nonzero as true. The continuous factor is not retained; an explicit zero turns the screen reference off again |

These are approximations selected by the caller, not claims of equivalent rendering semantics or guarantees
that another tool will retain them. See the [Conversion Tool comparison](VALIDATION.md#dolby-conversion-tool-元数据映射对照) and the [standalone Renderer audio and encoding measurements](VALIDATION.md#dolby-atmos-renderer-的-diffuse-与-decorr-实测).
`--allow-loss` does not enable these approximations. Without the switch, same-format native diffuse / decorr remain supported; cross-format unmapped `diffuse`, `decorrelation`,
`screenRef` and `screenFactor` information follows the loss policy. Dropping `screenFactor` preserves the
existing native `screenRef` rather than setting it to true. If an approximation conflicts with an explicit
destination field in the same update or a non-default initial destination value, `Reject` refuses it;
`Allow` keeps the destination value, drops the conflicting source field and records a loss.

Library callers select output approximations through `MasterSpec::approximate_mappings`. For reading and
conversion, use `AdmReader::open_with_options` / `DamfReader::open_with_options` with `MappingOptions`,
containing `loss_policy`, `approximate_mappings` and `preserve_literal`. The options follow the model through update iteration
and all conversion targets. `open(path, policy)` leaves approximations and literal preservation disabled. Low-level
`metadata` callers still perform their own mapping checks; individual blocks can use
`write_adm_block_with_approximations`.
Reading nonzero `diffuse` or writing it back to ADM needs no approximation switch; converting it to DAMF requires that authorisation.

`--preserve-literal <FIELD>` opts into each interoperability-limited spelling separately. Both flags default to off in `MasterSpec::preserve_literal` and `MappingOptions::preserve_literal`.

| Field | Authorised output |
| --- | --- |
| `cartesian-azimuth-range` | Write a legacy azimuthRange literally on a Cartesian object block. This falls outside the BS.2076-2 coordinate convention. There is no angle-to-distance conversion. Supplying both range types remains invalid. |
| `damf-mid` | Write literal `binauralRenderMode: mid` on a DAMF object or bed. Dolby Conversion Tool 2.1.2 rejects this spelling. Logic DBMD stores explicit Mid directly without a switch. |

`MappingDiagnostic::kind` distinguishes Loss, Approximation and PreservedLiteral. CLI `read_losses` / `outputs[].losses` use `kind` values `loss`, `approximation` and `preserved_literal`. Text output shows lost, approximate and preserved. Both explicit choices report the first occurrence per target, track and field, work under Reject, and leave unrelated losses subject to the selected policy.

- No equivalence is guessed between arbitrary regions and presets, content categories and dialog/music
  flags, or continuous screen/depth factors.
- Logic DBMD stores only verified static fields; dynamic head tracking, headphone and trimBypass changes
  must be rejected or diagnosed while retaining the initial values.
- To request DAMF's default middle distance, use `BinauralMode::Undefined`, written as `binauralRenderMode: undefined`. Omission on the first event selects the channel default. Later omission inherits the preceding value, while explicit undefined can reset Near/Far to the default. Converting explicit Mid to portable DAMF loses the explicit indication. Reject refuses it and Allow reports the loss and writes undefined (off for bed LFE). [Tool comparison](VALIDATION.md#双耳中距离的显式值与默认值)
- ADM importance is quantised to 0..10, and scores that are not a multiple of ten report a loss.
- 24-bit audio quantisation and the precision of the chosen clock are declared format behaviour, not loss;
  neither is DAMF leaving out the programme name.

## mfadm reference

`mfadm` never modifies an input and never overwrites an existing output. The subcommand table is in the
[README](../README.en.md#the-mfadm-command).

- `chunks`, `info`, `adm` and `convert` take `--json` for machine-readable output.
- Conversion `updates` counts objects, while `bed_updates` counts later source bed events or ADM blocks. Derived active boundaries are excluded.
- `adm` and `axml` read metadata within a budget, 256 MiB by default, adjustable with `--max-metadata <MiB>`.
- Besides resolving the ADM against the file's own `chna` and sample rate, `mfadm adm` checks that CHNA's
  track count matches `fmt ` and that no record is truncated.
- `convert` tells a DAMF input from an ADM BWF by its contents; `--audio <wav>` takes the audio from a WAV,
  `--fps` overrides the frame-rate declaration for ADM input without resampling, and `--allow-loss` degrades and lists the losses (that is,
  `LossPolicy::Allow`; the default rejects).
- `convert --preserve-literal <FIELD>` selects the legacy Cartesian angle or DAMF mid independently.
- `convert --approximate <MAPPING>` can be repeated and enables only the selected approximation;
  names and rules are listed under [semantic loss and diagnostics](#semantic-loss-and-diagnostics).
- `convert --dbmd-originator <TEXT>` and `--dbmd-product <TEXT>` customise the DBMD fields of every Logic
  output, within the limits under [the four master profiles](#the-four-master-profiles); `mfadm convert --help`
  explains the Conversion Tool's provenance messages as well.
- Failures exit with 1, usage errors with 2.

```sh
mfadm convert input.wav --to logic=output.wav \
  --dbmd-originator "Created using Dolby equipment"

mfadm convert input.wav --to home=output-home \
  --approximate diffuse-decorrelation --approximate screen-ref-factor
```

## Dependencies and features

All four crates forbid unsafe code (`unsafe_code = "forbid"`). The three libraries' only dependencies are
`quick-xml`, `libm` and `renamore`, each pinned to an exact version.

Both readers sit behind the `read` feature, off by default. Reading DAMF needs a YAML parser, and only turning
the feature on brings in `saphyr-parser` (pinned as well); reading ADM needs nothing more, but shares the DAMF
conversion path. Consumers that only write keep their dependency graph.

`macin-format` only provides the `mfadm` command and is not uploaded to crates.io; its extra dependencies,
`clap` and `serde_json`, never reach the three libraries.
