//! `mfadm convert`: a DAMF master or an ADM BWF, optionally with its audio taken from a WAV,
//! written to one or more profiles in a single pass. Inputs are only read; outputs must not exist
//! yet.
use crate::{Error, Result};
use macin_atmos::adm::AdmReader;
use macin_atmos::damf::DamfReader;
use macin_atmos::{FrameRate, LossPolicy, MappingDiagnostic, Profile, Target};
use serde::Serialize;
use std::path::{Path, PathBuf};

impl From<macin_atmos::Error> for Error {
    fn from(error: macin_atmos::Error) -> Self {
        let mut context = Vec::new();
        if let Some(track) = error.track {
            context.push(format!("track {}", track.0));
        }
        if let Some(sample) = error.sample {
            context.push(format!("sample {sample}"));
        }
        if let Some(field) = error.field {
            context.push(format!("field {field}"));
        }
        let message = if context.is_empty() {
            error.message.clone()
        } else {
            format!("{} ({})", error.message, context.join(", "))
        };
        Self {
            message,
            io_kind: std::error::Error::source(&error)
                .and_then(|e| e.downcast_ref::<std::io::Error>())
                .map(std::io::Error::kind),
        }
    }
}

/// The profile names `mfadm` accepts, matching the `synthetic_master` example.
pub fn parse_profile(name: &str) -> Result<Profile> {
    match name {
        "standard" => Ok(Profile::Standard),
        "logic" => Ok(Profile::Logic),
        "home" => Ok(Profile::DamfHome),
        "3dof" => Ok(Profile::DamfThreeDof),
        _ => Err(Error::msg(format!(
            "unknown profile {name:?} (standard, logic, home or 3dof)"
        ))),
    }
}
#[must_use]
pub fn profile_name(profile: Profile) -> &'static str {
    match profile {
        Profile::Standard => "standard",
        Profile::Logic => "logic",
        Profile::DamfHome => "home",
        Profile::DamfThreeDof => "3dof",
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Loss {
    pub track: Option<u64>,
    pub sample: Option<u64>,
    pub field: &'static str,
    pub message: String,
}
fn losses(diagnostics: &[MappingDiagnostic]) -> Vec<Loss> {
    diagnostics
        .iter()
        .map(|d| Loss {
            track: d.track.map(|t| t.0),
            sample: d.sample,
            field: d.field,
            message: d.message.clone(),
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct Output {
    pub path: PathBuf,
    pub profile: &'static str,
    pub frames: u64,
    pub channels: u16,
    pub sample_rate: u32,
    pub losses: Vec<Loss>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConvertReport {
    pub source: PathBuf,
    pub source_profile: &'static str,
    /// The WAV used in place of the DAMF audio, if any.
    pub audio: Option<PathBuf>,
    pub objects: usize,
    pub updates: u64,
    /// What reading dropped; only nonempty with `--allow-loss`.
    pub read_losses: Vec<Loss>,
    pub outputs: Vec<Output>,
}

/// The frame rate names `--fps` accepts, as DAMF spells them.
pub fn parse_frame_rate(name: &str) -> Result<FrameRate> {
    Ok(match name {
        "23.976" => FrameRate::Fps23976,
        "24" => FrameRate::Fps24,
        "25" => FrameRate::Fps25,
        "29.97" => FrameRate::Fps2997,
        "29.97df" => FrameRate::Fps2997Drop,
        "30" => FrameRate::Fps30,
        _ => {
            return Err(Error::msg(format!(
                "unknown frame rate {name:?} (23.976, 24, 25, 29.97, 29.97df or 30)"
            )));
        }
    })
}

/// Is `path` a RIFF/RF64/BW64 file (an ADM BWF) rather than a DAMF manifest?
fn is_wave(path: &Path) -> Result<bool> {
    let mut magic = [0; 4];
    let mut file = std::fs::File::open(path).map_err(|e| Error {
        message: format!("{}: {e}", path.display()),
        io_kind: Some(e.kind()),
    })?;
    let read = std::io::Read::read(&mut file, &mut magic)?;
    Ok(read == 4 && matches!(&magic, b"RIFF" | b"RF64" | b"BW64"))
}

/// Convert `source`, a DAMF manifest or an ADM BWF, to every target. `audio` replaces the
/// source's audio with a WAV whose channel `n` carries DAMF ID `n` (DAMF) or ADM track `n + 1`
/// (ADM). `frame_rate` sets the frame rate of an ADM source, which carries none.
pub fn convert(
    source: &Path,
    audio: Option<&Path>,
    targets: &[Target],
    policy: LossPolicy,
    frame_rate: Option<FrameRate>,
) -> Result<ConvertReport> {
    let (spec, updates, conversion) = if is_wave(source)? {
        let mut reader = AdmReader::open(source, policy)?;
        if let Some(rate) = frame_rate {
            reader = reader.with_frame_rate(rate);
        }
        let input = match audio {
            Some(path) => reader.wave_audio(path)?,
            None => reader.audio()?,
        };
        let conversion = reader.convert(input, targets)?;
        (
            reader.spec().clone(),
            reader.updates().len() as u64,
            conversion,
        )
    } else {
        if frame_rate.is_some() {
            return Err(Error::msg(
                "--fps applies to ADM input; DAMF carries its own frame rate",
            ));
        }
        let reader = DamfReader::open(source, policy)?;
        let input = match audio {
            Some(path) => reader.wave_audio(path)?,
            None => reader.audio()?,
        };
        let conversion = reader.convert(input, targets)?;
        (reader.spec().clone(), reader.update_count(), conversion)
    };
    Ok(ConvertReport {
        source: source.to_owned(),
        source_profile: profile_name(spec.profile),
        audio: audio.map(Path::to_owned),
        objects: spec.objects.len(),
        updates,
        read_losses: losses(&conversion.read),
        outputs: targets
            .iter()
            .zip(&conversion.outputs)
            .map(|(target, report)| Output {
                path: target.path.clone(),
                profile: profile_name(target.profile),
                frames: report.frames,
                channels: report.channels,
                sample_rate: report.sample_rate,
                losses: losses(&report.diagnostics),
            })
            .collect(),
    })
}
