use crate::TrackId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    InvalidInput,
    Unsupported,
    OutputExists,
    Io,
    Failed,
}

#[derive(Debug)]
pub struct Error {
    pub kind: ErrorKind,
    pub track: Option<TrackId>,
    pub sample: Option<u64>,
    pub field: Option<&'static str>,
    pub message: String,
    source: Option<std::io::Error>,
}
impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            track: None,
            sample: None,
            field: None,
            message: message.into(),
            source: None,
        }
    }
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput, message)
    }
    pub(crate) fn context(mut self, track: TrackId, sample: u64, field: &'static str) -> Self {
        self.track = Some(track);
        self.sample = Some(sample);
        self.field = Some(field);
        self
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|e| e as _)
    }
}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        let kind = if error.kind() == std::io::ErrorKind::AlreadyExists {
            ErrorKind::OutputExists
        } else {
            ErrorKind::Io
        };
        Self {
            kind,
            message: error.to_string(),
            source: Some(error),
            track: None,
            sample: None,
            field: None,
        }
    }
}
impl From<macin_bwf::Error> for Error {
    fn from(error: macin_bwf::Error) -> Self {
        Self::new(
            match error.code {
                1 => ErrorKind::InvalidInput,
                2 => ErrorKind::Unsupported,
                _ => ErrorKind::Io,
            },
            error.message,
        )
    }
}
pub type Result<T> = std::result::Result<T, Error>;
