// Named ReflectionProbes (id 54) refusals: the decoder's checks and the
// loader's world-binding check against Geometry (id 17).
// See: context/lib/build_pipeline.md §PRL section IDs

use thiserror::Error;

/// Every way id 54 refuses a level. Each message names the section.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ReflectionProbesError {
    #[error(
        "ReflectionProbesVersionMismatch: version {version}, expected {expected}; re-bake the level"
    )]
    VersionMismatch { version: u16, expected: u16 },
    #[error("ReflectionProbesTruncated: truncated {what}")]
    Truncated { what: String },
    #[error(
        "ReflectionProbesColourMipCount: {count} colour mip(s) of a {tile_dimension}² tile; \
         the count must be at least 1 and every mip whole 4×4 blocks of at least 4²"
    )]
    ColourMipCount { count: u32, tile_dimension: u32 },
    #[error(
        "ReflectionProbesFluidBindingOrder: binding {index} (volume {volume}, face {face}) is \
         out of order"
    )]
    FluidBindingOrder {
        index: usize,
        volume: u32,
        face: u32,
    },
    #[error("ReflectionProbesFluidFaceBoundTwice: volume {volume} face {face} is bound twice")]
    FluidFaceBoundTwice { volume: u32, face: u32 },
    #[error("ReflectionProbesWorldBindingOrder: binding {index} (face {face}) is out of order")]
    WorldBindingOrder { index: usize, face: u32 },
    #[error("ReflectionProbesWorldFaceBoundTwice: face {face} is bound twice")]
    WorldFaceBoundTwice { face: u32 },
    #[error(
        "ReflectionProbesBindingProbePastCount: {list} binding {index} names probe {probe}, past \
         the {probe_count} probes"
    )]
    BindingProbePastCount {
        list: BindingList,
        index: usize,
        probe: u32,
        probe_count: usize,
    },
    #[error(
        "ReflectionProbesWorldFacePastGeometry: world binding {index} names face {face}, past \
         Geometry (id 17)'s {face_count} faces"
    )]
    WorldFacePastGeometry {
        index: usize,
        face: u32,
        face_count: usize,
    },
    /// Every other structural refusal: a header off the engine's contract, a
    /// malformed probe or share entry, a payload of the wrong length.
    #[error("ReflectionProbesInvalid: {0}")]
    Invalid(String),
}

/// Which binding list a refusal names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingList {
    Fluid,
    World,
}

impl std::fmt::Display for BindingList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Fluid => "fluid",
            Self::World => "world",
        })
    }
}
