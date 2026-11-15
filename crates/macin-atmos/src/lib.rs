//! Atmos 母版写入。
#![forbid(unsafe_code)]

pub mod dbmd;
mod error;
pub mod metadata;
mod model;
mod writer;
pub use error::{Error, ErrorKind, Result};
pub use model::*;
pub use writer::{MasterWriter, MultiWriter, Target};
#[cfg(feature = "read")]
pub mod adm;
pub mod containers;
#[cfg(feature = "read")]
pub mod damf;
#[cfg(feature = "read")]
mod yaml;
