//! `mfadm`: inspection of RIFF/RF64/BW64 files and their ADM metadata, and DAMF conversion.
#![forbid(unsafe_code)]

use clap::{Args, Parser, Subcommand};
use macin_atmos::{LossPolicy, Target};
use macin_format::convert::{ConvertReport, Loss, parse_profile};
use macin_format::{
    AdmReport, ChunksReport, DEFAULT_METADATA_BUDGET, Error, InfoReport, Result, timecode,
};
use serde::Serialize;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

/// Inspect RIFF/RF64/BW64 containers and their ADM metadata, and convert DAMF masters.
/// Never modifies an input and never overwrites an output.
#[derive(Parser)]
#[command(name = "mfadm", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List the container type and every chunk's offset and size.
    Chunks(Common),
    /// Show the audio format, length and the chna track table.
    Info(Common),
    /// Summarise programmes, contents, objects, tracks and blocks from the ADM metadata.
    Adm {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        budget: Budget,
    },
    /// Write the raw axml chunk to stdout, byte for byte.
    Axml {
        file: PathBuf,
        #[command(flatten)]
        budget: Budget,
    },
    /// Write a DAMF master, or DAMF metadata with a WAV, to one or more profiles in one pass.
    Convert {
        /// The DAMF manifest (`.atmos`).
        manifest: PathBuf,
        /// An output as PROFILE=PATH, where PROFILE is standard, logic, home or 3dof. Repeat it
        /// to write several outputs from one pass; a DAMF PATH becomes a directory.
        #[arg(long = "to", value_name = "PROFILE=PATH", required = true, value_parser = target)]
        to: Vec<Target>,
        /// Use this WAV in place of the DAMF audio; its channel n carries DAMF ID n.
        #[arg(long, value_name = "WAV")]
        audio: Option<PathBuf>,
        /// Drop what a profile cannot hold and report it, instead of failing.
        #[arg(long)]
        allow_loss: bool,
        /// Print machine-readable JSON instead of text.
        #[arg(long)]
        json: bool,
    },
}

fn target(value: &str) -> std::result::Result<Target, String> {
    let (profile, path) = value
        .split_once('=')
        .ok_or_else(|| "expected PROFILE=PATH".to_owned())?;
    if path.is_empty() {
        return Err("the output path is empty".into());
    }
    Ok(Target {
        profile: parse_profile(profile).map_err(|e| e.message)?,
        path: path.into(),
    })
}

#[derive(Args)]
struct Common {
    file: PathBuf,
    /// Print machine-readable JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct Budget {
    /// Largest metadata chunk to read into memory, in MiB.
    #[arg(long, value_name = "MIB", default_value_t = DEFAULT_METADATA_BUDGET >> 20)]
    max_metadata: u64,
}
impl Budget {
    fn bytes(&self) -> Result<u64> {
        self.max_metadata
            .checked_mul(1 << 20)
            .ok_or_else(|| Error::msg("--max-metadata is too large"))
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        // A reader that stops early (`mfadm axml f.wav | head`) is not a failure of the inspection.
        Err(error) if error.io_kind == Some(std::io::ErrorKind::BrokenPipe) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mfadm: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let mut out = std::io::stdout().lock();
    match cli.command {
        Command::Chunks(c) => emit(
            &mut out,
            &macin_format::chunks(&c.file)?,
            c.json,
            text_chunks,
        ),
        Command::Info(c) => emit(&mut out, &macin_format::info(&c.file)?, c.json, text_info),
        Command::Adm { common: c, budget } => emit(
            &mut out,
            &macin_format::adm(&c.file, budget.bytes()?)?,
            c.json,
            text_adm,
        ),
        Command::Convert {
            manifest,
            to,
            audio,
            allow_loss,
            json,
        } => {
            let policy = if allow_loss {
                LossPolicy::Allow
            } else {
                LossPolicy::Reject
            };
            let report = macin_format::convert::convert(&manifest, audio.as_deref(), &to, policy)?;
            emit(&mut out, &report, json, text_convert)
        }
        Command::Axml { file, budget } => {
            out.write_all(&macin_format::axml(&file, budget.bytes()?)?)?;
            Ok(out.flush()?)
        }
    }
}

fn emit<T: Serialize>(
    out: &mut impl Write,
    report: &T,
    json: bool,
    text: fn(&T) -> String,
) -> Result<()> {
    if json {
        serde_json::to_writer_pretty(&mut *out, report)
            .map_err(|e| Error::from(std::io::Error::from(e)))?;
        writeln!(out)?;
    } else {
        out.write_all(text(report).as_bytes())?;
    }
    Ok(out.flush()?)
}

fn text_chunks(r: &ChunksReport) -> String {
    let mut s = format!("{}  {} bytes\n", r.container, r.end);
    s += &format!("{:<6} {:>14} {:>14}\n", "id", "offset", "size");
    for c in &r.chunks {
        s += &format!("{:<6} {:>14} {:>14}\n", c.id, c.offset, c.size);
    }
    s
}

fn text_info(r: &InfoReport) -> String {
    let precision = if r.valid_bits == r.bits {
        format!("{}-bit {}", r.bits, r.sample_format)
    } else {
        format!(
            "{}-bit {} ({} valid)",
            r.bits, r.sample_format, r.valid_bits
        )
    };
    let mut s = format!(
        "container    {}\nformat       {} Hz, {} ch, {precision}\n",
        r.container, r.sample_rate, r.channels
    );
    if let Some(mask) = r.channel_mask {
        s += &format!("channel mask 0x{mask:08x}\n");
    }
    s += &format!(
        "length       {} frames ({}), {} data bytes\n",
        r.frames,
        timecode(r.frames, r.sample_rate),
        r.data_bytes
    );
    match &r.chna {
        None => s += "chna         none\n",
        Some(rows) => {
            s += &format!("chna         {} entries\n", rows.len());
            s += &format!(
                "  {:>5}  {:<12} {:<14} {:<11}\n",
                "track", "uid", "track format", "pack format"
            );
            for e in rows {
                s += &format!(
                    "  {:>5}  {:<12} {:<14} {:<11}\n",
                    e.track_index, e.uid, e.track_format, e.pack_format
                );
            }
        }
    }
    s
}

fn text_adm(r: &AdmReport) -> String {
    let mut s = format!(
        "{} programmes, {} contents, {} objects, {} HOA packs; {} object blocks, {} DirectSpeakers blocks\n",
        r.programmes.len(),
        r.contents.len(),
        r.objects.len(),
        r.hoa.len(),
        r.object_blocks,
        r.direct_speaker_blocks
    );
    for p in &r.programmes {
        s += &format!("\nprogramme {} \"{}\"", p.id, p.name);
        if let Some(language) = &p.language {
            s += &format!(" [{language}]");
        }
        s += &format!("\n  contents: {}\n", p.contents.join(", "));
    }
    for c in &r.contents {
        s += &format!("\ncontent {} \"{}\"", c.id, c.name);
        if let Some(language) = &c.language {
            s += &format!(" [{language}]");
        }
        s += &format!("\n  objects: {}\n", c.objects.join(", "));
    }
    for o in &r.objects {
        s += &format!(
            "\nobject {} \"{}\"  {}\n  start {}",
            o.id,
            o.name,
            o.kind.as_deref().unwrap_or("(container)"),
            timecode(o.start, r.sample_rate)
        );
        if let Some(duration) = o.duration {
            s += &format!(", duration {}", timecode(duration, r.sample_rate));
        }
        s += &format!(", gain {}", o.gain);
        if o.mute {
            s += ", muted";
        }
        s += "\n";
        if !o.children.is_empty() {
            s += &format!("  children: {}\n", o.children.join(", "));
        }
        for t in &o.tracks {
            let track = t
                .track_index
                .map_or_else(|| "unmapped".into(), |i| format!("track {i}"));
            let mut blocks = Vec::new();
            if t.object_blocks > 0 {
                blocks.push(format!("{} object blocks", t.object_blocks));
            }
            if t.direct_speaker_blocks > 0 {
                blocks.push(format!("{} DirectSpeakers blocks", t.direct_speaker_blocks));
            }
            s += &format!("  {} -> {track}", t.uid);
            if !blocks.is_empty() {
                s += &format!(", {}", blocks.join(", "));
            }
            s += "\n";
        }
    }
    for h in &r.hoa {
        s += &format!("\nHOA {} on {}: {} channels", h.pack, h.object, h.channels);
        if let Some(order) = h.max_order {
            s += &format!(", order {order}");
        }
        s += "\n";
    }
    if !r.warnings.is_empty() {
        s += "\nwarnings:\n";
        for w in &r.warnings {
            s += &format!("  {w}\n");
        }
    }
    s
}

fn text_losses(s: &mut String, losses: &[Loss]) {
    for loss in losses {
        let mut at = Vec::new();
        if let Some(track) = loss.track {
            at.push(format!("track {track}"));
        }
        if let Some(sample) = loss.sample {
            at.push(format!("sample {sample}"));
        }
        let at = if at.is_empty() {
            String::new()
        } else {
            format!(" ({})", at.join(", "))
        };
        *s += &format!("   lost {}{at}: {}\n", loss.field, loss.message);
    }
}

fn text_convert(r: &ConvertReport) -> String {
    let mut s = format!(
        "{} ({}): {} objects, {} updates",
        r.source.display(),
        r.source_profile,
        r.objects,
        r.updates
    );
    if let Some(audio) = &r.audio {
        s += &format!(", audio from {}", audio.display());
    }
    s += "\n";
    text_losses(&mut s, &r.read_losses);
    for o in &r.outputs {
        s += &format!(
            "-> {} ({}): {} frames, {} channels, {} Hz\n",
            o.path.display(),
            o.profile,
            o.frames,
            o.channels,
            o.sample_rate
        );
        text_losses(&mut s, &o.losses);
    }
    s
}
