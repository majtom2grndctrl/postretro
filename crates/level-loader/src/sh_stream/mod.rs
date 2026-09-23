//! Metadata-first SH streaming manifest and positional chunk reads.
//! See: context/plans/in-progress/sh-probe-streaming--cluster-residency/index.md

mod boundary;
mod manifest;
mod metadata_base;
mod metadata_sparse;
mod positional_io;
mod projection;

pub use boundary::ShStreamingMode;
pub use boundary::{PreparedShCluster, ShDrainBatch, ShDrainOutcome, ShStorage};
pub use manifest::ShStreamManifest;
pub(crate) use manifest::load_manifest_positionally;
#[cfg(test)]
pub(crate) use positional_io::observe_positional_reads;
pub(crate) use positional_io::{
    read_container_positionally, read_section_positionally, read_vec_at,
};
pub use projection::{
    ShStreamBaseMetadata, ShStreamDirectMetadata, ShStreamSourceMetadata, ShStreamSparseMetadata,
};

use crate::prl::PrlLoadError;

/// Resolve the developer/test gate only after id-50 structural validation.
/// A PRL without id 50 ignores this setting and remains legacy-compatible.
pub fn requested_streaming_mode() -> Result<ShStreamingMode, PrlLoadError> {
    match std::env::var("POSTRETRO_SH_STREAMING") {
        Err(std::env::VarError::NotPresent) => Ok(ShStreamingMode::Async),
        Ok(value) if value == "off" => Ok(ShStreamingMode::Off),
        Ok(value) if value == "sync-proof" => Ok(ShStreamingMode::SyncProof),
        Ok(value) if value == "async" => Ok(ShStreamingMode::Async),
        Ok(value) => Err(PrlLoadError::InvalidShStreamingMode { value }),
        Err(std::env::VarError::NotUnicode(_)) => Err(PrlLoadError::InvalidShStreamingMode {
            value: "<non-unicode>".into(),
        }),
    }
}

pub(super) fn stream_error(message: impl Into<String>) -> PrlLoadError {
    PrlLoadError::SectionValidation {
        section: "SH streaming",
        message: message.into(),
    }
}

#[cfg(test)]
mod tests;
