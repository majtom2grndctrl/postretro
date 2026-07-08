// Binary-side render facade: re-export the GPU renderer crate and keep CPU-only
// splash decoding local to the engine boot path.
// See: context/lib/rendering_pipeline.md §7.8

pub mod splash;

#[cfg(test)]
mod ui_lifecycle_render_test;

#[cfg(feature = "dev-tools")]
pub(crate) mod nav_diagnostics;

#[cfg(feature = "dev-tools")]
pub mod debug_ui {
    pub use postretro_renderer::{DebugUi, draw_diagnostics_panel};
}

#[allow(unused_imports)]
pub use postretro_renderer::{
    BvhOverlayBudget, BvhOverlayColorMode, BvhOverlayDepthMode, BvhOverlayState,
    CameraCullDiagnostics, CameraCullPath, CellOverlayState, ClearColor, DEFAULT_AMBIENT_FLOOR,
    DEFAULT_DYNAMIC_DIRECT_SCALE, DEFAULT_INDIRECT_SCALE, DynamicDirectIsolation,
    KinematicMoverInstance, LevelGeometry, LightingIsolation, LocatorDiagnostics,
    PortalOverlayState, PresentHandle, Renderer, SdfShadowMode, SpatialCellSetDiagnostics,
    SpatialDiagnostics, WorldWireframeMode, level_world_to_geometry,
};

#[cfg(feature = "dev-tools")]
#[allow(unused_imports)]
pub use postretro_renderer::{
    AgentDiagnosticsRow, DeltaVolumeMeta, FrameTimingSnapshot, MarkerMode, ShDiagnosticsState,
};
