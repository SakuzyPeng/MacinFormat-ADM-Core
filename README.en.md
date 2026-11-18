# MacinFormat-ADM-Core

[简体中文](README.md) · **English**

**A Rust foundation for ADM/BWF reading and writing and Atmos master authoring** · Rust 2024 · MSRV 1.98 · MIT

A Rust library that **only deals with file formats**: it parses, projects and writes back ADM XML, reads and
writes RIFF/RF64/BW64 containers, streams object audio into Atmos masters as ADM BWF or DAMF, and reads DAMF
and ADM BWF back into the same model to convert them to another profile. It gathers
in one place the format code that the renderer, the decoder's export path, object clustering and encoding
tools each used to carry their own copy of; scene models and algorithms stay with each consumer.

> It is not a renderer, decoder or encoder: it contains no audio algorithms, exports no C ABI, and does not
> depend on AC-4, a clustering implementation or tables derived from a specification. FFI is the caller's
> to wrap.

This page covers what the library does, how to depend on it and how to start. **How each step of writing is
handled, and what reading treats as loss or refuses outright, is in the
[format reference](docs/REFERENCE.en.md).**

## Contents

- [What it does](#what-it-does)
- [The four crates](#the-four-crates)
- [Getting it](#getting-it)
- [Quickstart](#quickstart)
- [The mfadm command](#the-mfadm-command)
- [Choosing a master profile](#choosing-a-master-profile)
- [Scope and known limits](#scope-and-known-limits)
- [External tool compatibility](#external-tool-compatibility)
- [Building from source](#building-from-source)
- [Documentation](#documentation)
- [Provenance and consumers](#provenance-and-consumers)
- [License](#license)

## What it does

- **Read ADM**: parse AXML, resolve references, and project Objects / DirectSpeakers / HOA into a flat
  `Snapshot`, without being limited to the Atmos master subset.
- **Edit ADM while preserving the source**: make semantic changes with `Patch`; everything untouched,
  unknown extensions included, is written back verbatim.
- **Generate ADM**: produce AXML and the matching `chna` for DirectSpeakers, HOA3 and Binaural render output.
- **Read and write WAVE containers**: streaming RIFF / RF64 / BW64 I/O, frame seeking, and reading and
  editing chunks such as `chna` / `bext`; PCM can follow libbw64's quantisation or the master writer's
  ([quantisation](docs/REFERENCE.en.md#pcm-quantisation)).
- **Write Atmos masters**: a fixed 7.1.2 bed plus up to 118 objects, with PCM and object metadata updates
  written block by block, output as Standard BW64, Logic RF64, DAMF home or DAMF 3DoF, and committed
  atomically on completion.
- **Write several at once**: `MultiWriter` writes one model to several profiles, ADM and DAMF side by side, in
  a single pass.
- **Read DAMF or ADM and convert it**: `damf::DamfReader` and `adm::AdmReader` read DAMF or an ADM BWF back
  into the writing model and convert it to any profile (ADM to DAMF, DAMF to ADM, or between profiles of one
  kind); the audio can be swapped for a WAV, which is how a master is written from a WAV plus metadata.
- **From the command line**: `mfadm` lists chunks, the audio format, `chna` and an ADM summary, and converts
  DAMF and ADM masters, with JSON output.

## The four crates

| Crate | Entry points | Responsibility |
| --- | --- | --- |
| `macin-adm` | `Document`, `Snapshot`, `Patch`, `generate` | ADM XML, reference resolution, Objects/DirectSpeakers/HOA projection, generation and source-preserving write-back; read-only access to the literal elements by `Snapshot` node number |
| `macin-bwf` | `Reader`, `ChunkReader`, `Writer`, `Chna` | RIFF/RF64/BW64, PCM and container edits; does not interpret ADM XML |
| `macin-atmos` | `MasterSpec`, `MasterWriter`, `MultiWriter`; `damf::DamfReader`, `adm::AdmReader` (feature `read`) | Source-agnostic ADM BWF / DAMF master writing, and DAMF / ADM reading and conversion; builds on the other two |
| `macin-format` (CLI) | the `mfadm` command | The command-line shell for inspection and conversion |

All four forbid unsafe code; the three libraries depend only on `quick-xml`, `libm` and `renamore`, pinned.
The readers sit behind the `read` feature, off by default, so consumers that only write keep their dependency
graph. See [dependencies and features](docs/REFERENCE.en.md#dependencies-and-features).

## Getting it

The three libraries are distributed through crates.io and require Rust 1.98 or newer. Add the dependencies you need.

```toml
[dependencies]
macin-atmos = "=0.1.0"
# Pull in only what you need when you just read or write ADM or containers:
macin-adm = "=0.1.0"
macin-bwf = "=0.1.0"
```

Enable `read` to read DAMF or ADM masters, or to convert them.

```toml
macin-atmos = { version = "=0.1.0", features = ["read"] }
```

A Git dependency can also pin the source to a specific commit.

```toml
macin-atmos = { git = "https://github.com/SakuzyPeng/MacinFormat-ADM-Core", rev = "<commit>" }
```

`mfadm` provides archives for Apple silicon macOS, Windows x64 and Linux x64. Extract the archive and run it in a terminal.
[v0.1.0 release](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/releases/tag/v0.1.0)

## Quickstart

### Reading an ADM BWF

```rust
use macin_adm::{Document, Snapshot, TrackUidDefinition};
use macin_bwf::{Chna, Reader};
use std::fs::File;

fn inspect(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut wav = Reader::new(File::open(path)?)?;
    let rate = wav.info().format.sample_rate;
    // Metadata is read within a budget the caller supplies.
    let axml = wav.metadata(*b"axml", 64 << 20)?.ok_or("no axml chunk")?;
    // Import-side decoding: reserved records at the end are tolerated; track index 0 marks an unused entry.
    let entries = match wav.metadata(*b"chna", 4 + 40 * u64::from(u16::MAX))? {
        Some(data) => Chna::decode_import(&data)?.entries,
        None => Vec::new(),
    };
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim_end_matches([' ', '\0']).to_owned();
    let mut channels = Vec::new();
    let mut definitions = Vec::new();
    for entry in entries.iter().filter(|e| e.track_index != 0) {
        let uid = text(&entry.uid);
        let mut format_id = text(&entry.track_format);
        // CHNA spells an AC reference with `_00` padding.
        if format_id.to_ascii_uppercase().starts_with("AC_") && format_id.ends_with("_00") {
            format_id.truncate(format_id.len() - 3);
        }
        let pack = text(&entry.pack_format);
        channels.push((uid.clone(), entry.track_index));
        definitions.push(TrackUidDefinition {
            uid,
            format_id,
            pack_id: (!pack.is_empty()).then_some(pack),
        });
    }
    // CHNA fills in any audioTrackUID the AXML leaves out; a conflict between the two is an error.
    let doc = Document::parse_with_track_uids(std::str::from_utf8(&axml)?, &definitions)
        .map_err(|e| e.message)?;
    let scene = Snapshot::extract(&doc, rate, &channels).map_err(|e| e.message)?;
    for object in &scene.objects {
        println!("{}: {} tracks", scene.strings[object.name as usize], object.tracks.len);
    }
    Ok(())
}
```

Some files define `audioTrackUID`s only in CHNA and leave them out of the AXML, so the example hands CHNA's
track definitions to `parse_with_track_uids` to fill them in, rather than using CHNA only to map track
numbers.

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

### Writing ADM and DAMF together

```rust
use macin_atmos::*;

fn both() -> Result<()> {
    let spec = MasterSpec {
        objects: vec![ObjectSpec::new(TrackId(42), "Cluster 0")],
        ..Default::default()
    };
    let targets = [
        Target { path: "result.wav".into(), profile: Profile::Logic },
        Target { path: "result-damf".into(), profile: Profile::DamfHome },
    ];
    // One model, one pass: the PCM is quantised once, and both outputs are published or neither is.
    let mut master = MultiWriter::create(&spec, &targets)?;
    let pcm = vec![0.0f32; 512 * 11];
    master.write_block(PcmBlock::F32Interleaved(&pcm), &[])?;
    let reports = master.finish()?; // one WriteReport per target
    assert_eq!(reports.len(), 2);
    Ok(())
}
```

### Converting DAMF to ADM

```rust
use macin_atmos::damf::DamfReader;
use macin_atmos::{LossPolicy, Profile, Result, Target};

fn convert() -> Result<()> {
    // Reading checks what the model cannot hold under the same LossPolicy.
    let reader = DamfReader::open("result-damf/master.atmos", LossPolicy::Allow)?;
    let targets = [Target { path: "from-damf.wav".into(), profile: Profile::Standard }];
    // reader.wave_audio("mix.wav")? in place of reader.audio()? is "a WAV plus DAMF metadata".
    let conversion = reader.convert(reader.audio()?, &targets)?;
    for loss in conversion.read.iter().chain(&conversion.outputs[0].diagnostics) {
        eprintln!("{:?} {}: {}", loss.track, loss.field, loss.message);
    }
    Ok(())
}
```

### Converting ADM to DAMF

```rust
use macin_atmos::adm::AdmReader;
use macin_atmos::{FrameRate, LossPolicy, Profile, Result, Target};

fn adm_to_damf() -> Result<()> {
    // Read the frame rate from DBMD by default; override it explicitly when needed.
    let reader = AdmReader::open("result.wav", LossPolicy::Reject)?.with_frame_rate(FrameRate::Fps25);
    let targets = [
        Target { path: "from-adm-home".into(), profile: Profile::DamfHome },
        Target { path: "from-adm-3dof".into(), profile: Profile::DamfThreeDof },
    ];
    let conversion = reader.convert(reader.audio()?, &targets)?;
    assert_eq!(conversion.outputs.len(), 2);
    Ok(())
}
```

## The mfadm command

`mfadm` shows what is inside a RIFF/RF64/BW64 file and converts DAMF and ADM masters. It never modifies an
input and never overwrites an existing output. To install it into `~/.cargo/bin`:

```sh
cargo install --locked --path crates/macin-format
```

| Subcommand | Output |
| --- | --- |
| `mfadm chunks <file>` | The container type and each chunk's ID, payload offset and size; decodes neither `fmt ` nor the audio |
| `mfadm info <file>` | Sample rate, channels, bit depth, frames and duration, and the `chna` track table |
| `mfadm adm <file>` | Resolves the ADM against the file's own `chna` and sample rate and summarises programmes, contents, objects, tracks and blocks, listing any skipped `typeDefinition` |
| `mfadm axml <file>` | Writes the `axml` chunk to stdout byte for byte, for comparison with external tools |
| `mfadm convert <x.atmos or x.wav> --to PROFILE=PATH …` | Writes a DAMF master or an ADM BWF to one or more profiles in one pass; `--audio <wav>` takes the audio from a WAV, `--fps` sets an ADM source's frame rate, `--allow-loss` degrades and lists the losses |

```text
$ mfadm adm result.wav
1 programmes, 1 contents, 3 objects, 0 HOA packs; 4 object blocks, 10 DirectSpeakers blocks
...
object AO_100b "Moving object"  Objects
  start 0:00:00.000, duration 0:00:02.000, gain 1
  ATU_0000000b -> track 11, 3 object blocks
```

```text
$ mfadm convert film/master.atmos --to standard=film.wav --to 3dof=film-3dof --allow-loss
film/master.atmos (home): 2 objects, 3 updates
-> film.wav (standard): 96000 frames, 12 channels, 48000 Hz
   lost binaural_mode (track 10, sample 36000): Property has no exact mapping in this ADM profile
-> film-3dof (3dof): 96000 frames, 12 channels, 48000 Hz
```

For a Logic master headed to Dolby tools, add `--dbmd-originator "Created using Dolby equipment"`; otherwise the
Conversion Tool reports that the file was not created by Dolby-approved software. `--json`, the metadata
budget, exit codes and the remaining options are in the [mfadm reference](docs/REFERENCE.en.md#mfadm-reference).

## Choosing a master profile

| Profile | File | Suited to |
| --- | --- | --- |
| `Standard` (default) | BW64 ADM BWF | General ADM tool chains such as EBU EAR |
| `Logic` | RF64 ADM BWF with DBMD | Importing into Logic Pro, or Dolby tools that only accept RIFF/RF64 |
| `DamfHome` | DAMF 0.5.1 (`.atmos`, `.atmos.metadata`, `.atmos.audio`) | Dolby Atmos Renderer and the Conversion Tool |
| `DamfThreeDof` | DAMF 0.6.0 / 3dof | Newer Dolby tools that take 3DoF DAMF |

When unsure, write several in one pass with `MultiWriter` or `mfadm convert --to … --to …`: the PCM is quantised
once and every output carries identical audio. What each profile can express and which clock it uses is in
[the four master profiles](docs/REFERENCE.en.md#the-four-master-profiles).

## Scope and known limits

- **Loss is refused by default**: under `LossPolicy::Reject`, semantics the target profile cannot express
  exactly are an error naming the field. `--approximate` authorises approximations per field; other
  degradations still require `Allow` (or `--allow-loss`). Both are recorded in diagnostics.
  See [semantic loss and diagnostics](docs/REFERENCE.en.md#semantic-loss-and-diagnostics).
- **Masters have one shape**: a fixed 7.1.2 bed plus up to 118 objects, 48/96 kHz, 24-bit; no multi-bed
  authoring or resampling.
- **Reading covers the Atmos subset**: one programme, at most one 7.1.2 bed, single-track Cartesian objects.
  Polar positions, HOA, several beds, nested objects and several presentations are refused. The full lists are
  under [reading DAMF](docs/REFERENCE.en.md#reading-and-converting-damf) and
  [reading ADM](docs/REFERENCE.en.md#reading-and-converting-adm). General `Document` / `Snapshot` parsing is
  not limited to it.
- **Native mappings**: ADM retains object parameters, bed blocks and track extension attributes. DAMF retains bed state and rendering events. Select interoperability-limited spellings individually with `--preserve-literal`. See the
  [format reference](docs/REFERENCE.en.md#native-adm-metadata-and-ancillary-chunks) for scope and limits.
- **Frame rate**: recovered from recognised DBMD, otherwise 24 fps; override it with `with_frame_rate` or `--fps`.
- **Atomic commit, no overwrite**: nothing is published until writing finishes, and an existing target is never
  overwritten; a failed `MultiWriter` publish withdraws what it already published, but cannot survive a crash
  that lands exactly between two renames.
- No rendering, clustering or coding; no C ABI.

## External tool compatibility

| Profile | Independent checks passed | Known tool limitations |
| --- | --- | --- |
| Standard BW64 | EBU EAR | Dolby Conversion Tool / atmos_info require RIFF/RF64 and do not recognise BW64 |
| Logic RF64 | Conversion Tool (48/96 kHz, PCM byte-identical), EAR | The default originator triggers Dolby tools' provenance messages; see `--dbmd-originator` above |
| DAMF home | Conversion Tool, atmos_info | — |
| DAMF 3DoF | A newer atmos_info | Conversion Tool 2.1.2 supports DAMF up to 0.5 |

In addition, 48 kHz / 24-bit Logic RF64 and DAMF home masters were tested on 2026-11-15 and work in Logic Pro and
Dolby Atmos Renderer (the Logic sample used the originator above). Tool versions, sample parameters and
per-case results are in the [validation record](docs/VALIDATION.md) (Chinese).

## Building from source

`rust-toolchain.toml` pins Rust 1.98, which rustup installs on its own.

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo clippy --locked -p macin-atmos --all-targets -- -D warnings
cargo test --locked -p macin-atmos
```

The last two check `macin-atmos` without `read` on its own. The tests use only synthetic fixtures in the
repository and need no media or external tools. CI runs the same checks on macOS arm64, Windows x64 and Linux x64. How to
run the external interop checker is in the [validation record](docs/VALIDATION.md#互操作检查器用法), and the
development conventions are in [AGENTS.md](AGENTS.md) (both Chinese).

## Documentation

The [format reference](docs/REFERENCE.en.md) ([简体中文](docs/REFERENCE.md)) is the full set of reading and
writing rules. The other documents are in Chinese:

[Validation record](docs/VALIDATION.md) ·
[crates.io release preparation](docs/CRATES_IO_RELEASE.md) ·
[CLI archives and GitHub releases](docs/BINARY_RELEASE.md) ·
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
