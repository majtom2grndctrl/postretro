//! CPU-only runtime PRL loading and level data.
//! See: context/lib/build_pipeline.md §PRL Compilation

mod prl;
#[cfg(feature = "load-prl")]
mod prl_container;
#[cfg(feature = "load-prl")]
mod prl_lighting;
#[cfg(feature = "load-prl")]
mod prl_loader;
mod prl_queries;
#[cfg(feature = "load-prl")]
mod prl_streaming;
#[cfg(feature = "load-prl")]
mod sh_stream;
#[cfg(all(test, feature = "load-prl"))]
mod sh_stream_tests;

pub use prl::{
    CellData, CellId, CellLocatorChild, CellLocatorNodeData, CellLocatorSide, CellLocatorTrace,
    CellLocatorTraceStep, CellVisibility, CoupledCellPair, CouplingTuple, FalloffModel, LevelWorld,
    LevelWorldValidationError, LightType, MapLight, PortalData, ShadowType,
};
#[cfg(feature = "load-prl")]
pub use prl::{
    CellDrawIndex, FaceMeta, KinematicGeometry, LightmapMode, LoadedKinematicMover,
    LoadedKinematicWaypoint, LoadedMemberLight, PrlLoadError,
};
#[cfg(feature = "load-prl")]
pub use prl_lighting::LevelWorldLighting;
#[cfg(feature = "load-prl")]
pub use prl_streaming::load_prl;
#[cfg(feature = "load-prl")]
pub use sh_stream::{
    PreparedShCluster, ShDrainBatch, ShDrainOutcome, ShStorage, ShStreamBaseMetadata,
    ShStreamDirectMetadata, ShStreamManifest, ShStreamSeamPortal, ShStreamSourceMetadata,
    ShStreamSparseMetadata, ShStreamingMode, requested_streaming_mode,
};
