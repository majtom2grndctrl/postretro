//! CPU-only runtime PRL loading and level data.
//! See: context/lib/build_pipeline.md §PRL Compilation

mod prl;
mod prl_loader;

pub use prl::{
    CellData, CellDrawIndex, CellLocatorChild, CellLocatorNodeData, CellLocatorSide,
    CellLocatorTrace, CellLocatorTraceStep, FaceMeta, FalloffModel, LevelWorld, LightType,
    LightmapMode, MapLight, PortalData, PrlLoadError, ShadowType,
};
pub use prl_loader::load_prl;
