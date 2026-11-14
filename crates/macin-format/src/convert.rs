//! `mfadm convert`: a DAMF master, or DAMF metadata with a WAV, written to one or more profiles
//! in a single pass. Inputs are only read; outputs must not exist yet.
use crate::{Error, Result};
use macin_atmos::damf::DamfReader;
use macin_atmos::{LossPolicy, MappingDiagnostic, Profile, Target};
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

/// Convert `manifest` to every target. `audio` replaces the DAMF audio with a WAV whose channel
/// `n` carries DAMF ID `n`.
pub fn convert(
    manifest: &Path,
    audio: Option<&Path>,
    targets: &[Target],
    policy: LossPolicy,
) -> Result<ConvertReport> {
    let reader = DamfReader::open(manifest, policy)?;
    let source = match audio {
        Some(path) => reader.wave_audio(path)?,
        None => reader.audio()?,
    };
    let conversion = reader.convert(source, targets)?;
    Ok(ConvertReport {
        source: manifest.to_owned(),
        source_profile: profile_name(reader.spec().profile),
        audio: audio.map(Path::to_owned),
        objects: reader.spec().objects.len(),
        updates: reader.update_count(),
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
