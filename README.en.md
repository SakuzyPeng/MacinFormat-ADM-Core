# MacinFormat-ADM-Core

[简体中文](README.md) · **English**

**A Rust foundation for ADM/BWF reading and writing and Atmos master authoring** · Rust 2024 · MSRV 1.98 · MIT

A Rust library that **only deals with file formats**: it parses, projects and writes back ADM XML, reads and
writes RIFF/RF64/BW64 containers, and streams object audio into Atmos masters as ADM BWF or DAMF. It gathers
in one place the format code that the renderer, the decoder's export path, object clustering and encoding
tools each used to carry their own copy of; scene models and algorithms stay with each consumer.

> It is not a renderer, decoder or encoder: it contains no audio algorithms, exports no C ABI, and does not
> depend on AC-4, a clustering implementation or tables derived from a specification. FFI is the caller's
> to wrap.

## Contents

- [What it does](#what-it-does)
- [The three crates](#the-three-crates)
- [Getting it](#getting-it)
- [Quickstart](#quickstart)
- [The mfadm command](#the-mfadm-command)
- [Master writing rules](#master-writing-rules)
- [The four master profiles](#the-four-master-profiles)
- [Semantic loss and diagnostics](#semantic-loss-and-diagnostics)
- [Out of scope](#out-of-scope)
- [Development and acceptance](#development-and-acceptance)
- [Interoperability with external tools](#interoperability-with-external-tools)
- [Documentation](#documentation)
- [Provenance and consumers](#provenance-and-consumers)
- [License](#license)

## What it does

- **Read ADM**: parse AXML, resolve references, and project Objects / DirectSpeakers / HOA into a flat
  `Snapshot`, without being limited by the Atmos writing subset described below.
- **Edit ADM while preserving the source**: make semantic changes with `Patch`; everything untouched,
  unknown extensions included, is written back verbatim.
- **Generate ADM**: produce AXML and the matching `chna` for DirectSpeakers, HOA3 and Binaural render output.
- **Read and write WAVE containers**: streaming RIFF / RF64 / BW64 I/O, frame seeking, and reading and
  editing chunks such as `chna` / `bext`. Metadata is read within a budget the caller supplies, and opening
  a file never allocates unknown chunks.
- **Inspect from the command line**: `mfadm` lists chunks, the audio format, `chna` and an ADM summary,
  with JSON output.
- **Write Atmos masters**: a fixed 7.1.2 bed plus up to 118 objects, with PCM and object metadata updates
  written block by block, output as Standard BW64, Logic RF64, DAMF home or DAMF 3DoF, and committed
  atomically on completion.

## The three crates

| Crate | Entry points | Responsibility |
| --- | --- | --- |
| `macin-adm` | `Document`, `Snapshot`, `Patch`, `generate` | ADM XML, reference resolution, Objects/DirectSpeakers/HOA projection, generation and source-preserving write-back |
| `macin-bwf` | `Reader`, `ChunkReader`, `Writer`, `Chna` | RIFF/RF64/BW64, PCM and container edits; does not interpret ADM XML |
| `macin-atmos` | `MasterSpec`, `MasterWriter` | Source-agnostic ADM BWF / DAMF master writing; builds on the other two |

All three forbid unsafe code (`unsafe_code = "forbid"`). Their only dependencies are `quick-xml`, `libm` and
`renamore`, each pinned to an exact version.

A fourth, unpublished crate, `macin-format`, provides the read-only [`mfadm`](#the-mfadm-command) command;
its extra dependencies, `clap` and `serde_json`, never reach the three libraries.

## Getting it

The source is distributed through the public GitHub repository; pin a commit with a Git dependency:

```toml
[dependencies]
macin-atmos = { git = "https://github.com/SakuzyPeng/MacinFormat-ADM-Core", rev = "<commit>" }
# Pull in only what you need when you just read or write ADM or containers:
macin-adm = { git = "https://github.com/SakuzyPeng/MacinFormat-ADM-Core", rev = "<commit>" }
macin-bwf = { git = "https://github.com/SakuzyPeng/MacinFormat-ADM-Core", rev = "<commit>" }
```

The three crates carry `0.1.0` package metadata but **have not been uploaded to crates.io yet**; the upload
steps are in [crates.io release preparation](docs/CRATES_IO_RELEASE.md) (Chinese).

## Quickstart

### Reading an ADM BWF

```rust
use macin_adm::{Document, Snapshot};
use macin_bwf::Reader;
use std::fs::File;

fn inspect(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut wav = Reader::new(File::open(path)?)?;
    let info = wav.info();
    // Metadata is read within a budget the caller supplies.
    let axml = wav.metadata(*b"axml", 64 << 20)?.ok_or("no axml chunk")?;
    let chna: Vec<(String, u16)> = wav
        .chna()?
        .map(|chna| {
            chna.entries
                .iter()
                .map(|e| (String::from_utf8_lossy(&e.uid).into_owned(), e.track_index))
                .collect()
        })
        .unwrap_or_default();
    let doc = Document::parse(std::str::from_utf8(&axml)?).map_err(|e| e.message)?;
    let scene = Snapshot::extract(&doc, info.format.sample_rate, &chna).map_err(|e| e.message)?;
    for object in &scene.objects {
        println!("{}: {} tracks", scene.strings[object.name as usize], object.tracks.len);
    }
    Ok(())
}
```

`Snapshot` is a set of flat tables: names and IDs are indices into `strings`, and child elements are `Span`s
into the corresponding table. `warnings` are `strings` indices too, saying which unsupported
`typeDefinition`s were skipped.

### Writing an Atmos master

```rust
use macin_atmos::*;

fn example() -> Result<()> {
    let spec = MasterSpec {
        objects: vec![ObjectSpec::new(TrackId(42), "Cluster 0")],
        ..Default::default()
    };
    let mut master = MasterWriter::create("result.wav", spec)?;
    // Ten 7.1.2 bed channels, then spec.objects in declaration order.
    let pcm = vec![0.0f32; 512 * 11];
    master.write_block(PcmBlock::F32Interleaved(&pcm), &[])?;
    let report = master.finish()?;
    assert_eq!(report.objects[0].track_index, 11);
    Ok(())
}
```

A complete example with a position ramp, headphone policy events, all four profiles and both sample rates is
[`synthetic_master`](crates/macin-atmos/examples/synthetic_master.rs):

```sh
cargo run -p macin-atmos --example synthetic_master -- result.wav standard 96000
```

The second argument is one of `standard` / `logic` / `home` / `3dof`, the third `48000` / `96000`.

## The mfadm command

`mfadm` reads and never writes: it shows what is inside a RIFF/RF64/BW64 file. To install it into
`~/.cargo/bin`:

```sh
cargo install --locked --path crates/macin-format
```

| Subcommand | Output |
| --- | --- |
| `mfadm chunks <file>` | The container type and each chunk's ID, payload offset and size; decodes neither `fmt ` nor the audio |
| `mfadm info <file>` | Sample rate, channels, bit depth, frames and duration, and the `chna` track table |
| `mfadm adm <file>` | Resolves the ADM against the file's own `chna` and sample rate and summarises programmes, contents, objects, tracks and blocks, listing any skipped `typeDefinition` |
| `mfadm axml <file>` | Writes the `axml` chunk to stdout byte for byte, for comparison with external tools |

`chunks`, `info` and `adm` take `--json` for machine-readable output. `adm` and `axml` read metadata within a
budget, 256 MiB by default, adjustable with `--max-metadata <MiB>`. Failures exit with 1, usage errors with 2.

```text
$ mfadm adm result.wav
1 programmes, 1 contents, 3 objects, 0 HOA packs; 4 object blocks, 10 DirectSpeakers blocks
...
object AO_100b "Moving object"  Objects
  start 0:00:00.000, duration 0:00:02.000, gain 1
  ATU_0000000b -> track 11, 3 object blocks
```

## Master writing rules

**Track layout**

- The fixed bed order is L, R, C, LFE, Lss, Rss, Lrs, Rrs, Lts, Rts; each may carry real signal or explicit
  silence.
- Up to 118 objects, with slots fixed at creation. `TrackId` is the caller's identity, not a file track
  number; the actual track numbers are in the report `finish` returns.

**Sampling and quantisation**

- 48/96 kHz, 24-bit PCM; the default is 48 kHz / 24 fps / Standard BW64.
- Accepts interleaved or planar normalized f32, and already-quantised S24LE.
- Floats are multiplied by 2^23 and rounded to nearest (ties away from zero), with the positive end
  saturating at 8388607. No dither and no automatic normalisation; non-finite samples and samples beyond ±1
  are errors. S24LE bytes are kept as they are.

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
- 24-bit audio quantisation and the precision of the chosen clock are declared format behaviour, not loss.

## Out of scope

- No DAMF **reading**, multi-bed authoring, resampling, rendering or encoding algorithms (first release).
- No rendering, clustering or coding; no C ABI.
- `mfadm` neither modifies nor converts files: a format conversion would drop semantics without saying so,
  which runs against `LossPolicy::Reject`.
- No faked creation tool provenance: the real creator is kept even when an external checker rejects the file
  for it (see below).

## Development and acceptance

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
```

`rust-toolchain.toml` pins Rust 1.98. Reuse an existing build directory through `CARGO_TARGET_DIR` or an
ignored `.cargo/config.toml`; don't copy a consumer's target directory or media.

The tests use standalone XML/WAVE fixtures and cover block-size invariance, ramps, all four profiles at both
sample rates, atomic failure, and a virtual 4 GiB boundary check that creates no huge file. The native
verification platforms are macOS arm64 and Windows x64, and CI runs the same checks on both.

## Interoperability with external tools

`scripts/check_interop.py` validates synthetic output with external tools the caller provides (and an
optional mradm reader). Reading back with the same code is integration testing only and does not replace
independent format acceptance.

| Profile | Independent validation | Known tool limitations |
| --- | --- | --- |
| Standard BW64 | EBU EAR | The local Dolby Conversion Tool / atmos_info require RIFF/RF64 and do not recognise BW64 |
| Logic RF64 | Conversion Tool conversion (48/96 kHz, PCM byte-identical), EAR | atmos_info checks creation tool provenance on self-generated RF64; the library does not rewrite the creator as Dolby to pass it |
| DAMF home | Conversion Tool, atmos_info | — |
| DAMF 3DoF | A newer atmos_info | Conversion Tool 2.1.2 supports DAMF up to 0.5 |

The checker records these boundaries as explicit tool limitations rather than writing a rejection up as a
pass; any unknown failure still fails the check. The full matrix needs EBU EAR installed in the same Python
environment and the paths to the external tools:

```sh
python scripts/check_interop.py --ear \
  --conversion-tool /path/to/cmdline_atmos_conversion_tool \
  --atmos-info /path/to/atmos_info \
  --report artifacts/interop.json
```

Tool versions, per-case results and the byte-for-byte comparisons from migrating the consumers are in the
[validation record](docs/VALIDATION.md) (Chinese).

## Documentation

The design and validation documents are in Chinese:

[Validation record](docs/VALIDATION.md) ·
[crates.io release preparation](docs/CRATES_IO_RELEASE.md) ·
[Source manifest (SOURCES.json)](docs/SOURCES.json) ·
[Interop summary (interop-summary.json)](docs/interop-summary.json)

## Provenance and consumers

The ADM/BWF implementation was moved from MacinRender-ADM-Core, and the Atmos serialisation takes over the
verified writing side of MacinDecode-AC4-Core. The source commit and SHA-256 of every imported file are
recorded in [SOURCES.json](docs/SOURCES.json).

| Repository | Relationship to this library |
| --- | --- |
| [MacinRender-ADM-Core](https://github.com/SakuzyPeng/MacinRender-ADM-Core) | Source of the ADM/BWF reading and writing; consumer on the rendering side |
| [MacinDecode-AC4-Core](https://github.com/SakuzyPeng/MacinDecode-AC4-Core) | Source of the Atmos master writing side; consumer for decode export |

Object clustering and encoding tools are consumers of this library as well.

## License

This project is licensed under the [MIT License](LICENSE).

The ADM common definitions bundled in `macin-adm` are unmodified data from
[libadm](https://github.com/ebu/libadm) 0.14.0 under Apache-2.0; their license, NOTICE and source hash are kept
alongside the resource in [`crates/macin-adm/assets/`](crates/macin-adm/assets/).
