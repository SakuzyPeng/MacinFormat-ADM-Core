//! Atmos 母版写入。
#![forbid(unsafe_code)]

pub mod dbmd;
mod error;
pub mod metadata;
mod model;
mod writer;
pub use error::{Error, ErrorKind, Result};
pub use model::*;
pub use writer::MasterWriter;
pub mod containers;
