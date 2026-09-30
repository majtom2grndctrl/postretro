//! App-side lightmap cell-block residency: demand, reads, and the drain handoff.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

pub(crate) mod block_map;
pub(crate) mod controller;
pub(crate) mod demand;
pub(crate) mod levers;
#[cfg(test)]
pub(crate) mod prl_test_fixture;
pub(crate) mod route;
pub(crate) mod source;
#[cfg(test)]
pub(crate) mod test_fixtures;

use postretro_level_loader::PrlLoadError;
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum LightmapResidencyError {
    #[error("lightmap streaming residency generation is exhausted")]
    GenerationExhausted,
    #[error("lightmap streaming level data is invalid: {0}")]
    InvalidLevel(String),
    #[error("lightmap streaming drain outcome is invalid: {0}")]
    InvalidDrainOutcome(String),
    #[error("lightmap streaming drain bytes overflow u64")]
    DrainBytesOverflow,
    #[error("lightmap streaming read issuer rejected a request: {0}")]
    Submit(&'static str),
    #[error("lightmap streaming block source failed: {0}")]
    Source(#[source] PrlLoadError),
    #[error("lightmap preload must run before any streaming read or drain")]
    PreloadAfterStart,
}
