//! CPU-only runtime PRL loading and level data.
//! See: context/lib/build_pipeline.md §PRL Compilation

mod prl;
#[cfg(feature = "load-prl")]
mod prl_loader;

pub use prl::{
    CellData, CellLocatorChild, CellLocatorNodeData, CellLocatorSide, CellLocatorTrace,
    CellLocatorTraceStep, FalloffModel, LevelWorld, LevelWorldValidationError, LightType, MapLight,
    PortalData, ShadowType,
};
#[cfg(feature = "load-prl")]
pub use prl::{CellDrawIndex, FaceMeta, LightmapMode, PrlLoadError};
#[cfg(feature = "load-prl")]
pub use prl_loader::load_prl;
