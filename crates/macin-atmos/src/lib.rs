//! Atmos 母版写入。
#![forbid(unsafe_code)]

mod ancillary;
mod block;
pub use block::{
    AdmBedBlock, AdmTiming, ExclusionZone, HeadphoneVirtualise, NativeObjectUpdate,
    TrackExtensions, VendorAttribute, ZoneBounds,
};
pub mod dbmd;
mod error;
pub mod metadata;
mod model;
mod native;
mod writer;
mod zones;
pub use ancillary::{
    CONTAINER_METADATA_LIMIT, ContainerMetadata, MetadataChunk, MetadataContainer,
};
pub use error::{Error, ErrorKind, Result};
pub use model::*;
pub use native::{AdmElement, AdmMetadata, ObjectDivergence, ScreenEdge};
pub use writer::{MasterWriter, MultiWriter, Target};
#[cfg(feature = "read")]
pub mod adm;
pub mod containers;
#[cfg(feature = "read")]
pub mod damf;
#[cfg(feature = "read")]
mod yaml;
