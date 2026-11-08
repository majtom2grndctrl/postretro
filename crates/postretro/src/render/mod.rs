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
    pub use postretro_renderer::{
        CpuTimingPanel, DebugUi, LightmapStreamingTab, NetworkLagLever, NetworkSessionRole,
        draw_diagnostics_panel,
    };
}

#[allow(unused_imports)]
pub use postretro_renderer::{
    BloomRenderProfile, BloomResolution, BvhOverlayBudget, BvhOverlayColorMode,
    BvhOverlayDepthMode, BvhOverlayState, CachedModel, CameraCullDiagnostics, CameraCullPath,
    CaptureAdapterIdentity, CaptureGpuTimingPass, CaptureGpuTimingState, CaptureGpuTimingWindow,
    CapturePromotionRow, CellOverlayState, ClearColor, DEFAULT_AMBIENT_FLOOR,
    DEFAULT_DYNAMIC_DIRECT_SCALE, DEFAULT_INDIRECT_SCALE, FrameScene, KinematicMoverInstance,
    LevelBuildError, LevelBuildEvent, LevelBuildPart, LevelBuildSnapshotInputs, LevelGeometry,
    LevelGeometryParts, LevelGeometryRangeError, LevelInstallError, LevelSetBuilder,
    LevelSetBuilderProbe, LevelSetId, LightTermMask, LightmapResidencyReport, LocatorDiagnostics,
    ModelCacheCommit, ModelCacheHandle, ModelCacheOffer, ModelContentKey, ModelEntryIdentity,
    MoverOccluderAabb, PendingLevelSet, PortalOverlayState, PresentHandle, Renderer,
    ResidencyAllocation, ResidencyAllocationShape, ResidencyAllocationState, ResidencySource,
    SPLASH_CLEAR_COLOR, SdfShadowMode, ShResidencyReport, ShSampleRegion, ShSampleRegionSets,
    ShStreamingLifecycleSummary, SpatialCellSetDiagnostics, SpatialDiagnostics,
    SpriteCollectionRegistration, StreamingReachLever, WorldWireframeMode, level_world_to_geometry,
    lightmap_family_inputs, sprite_specular_exponent_is_valid, validate_level_geometry_ranges,
};

#[cfg(feature = "dev-tools")]
#[allow(unused_imports)]
pub use postretro_renderer::{
    AgentDiagnosticsRow, DeltaVolumeMeta, DoorOccluderDiagnosticsRow, FrameTimingSnapshot,
    MarkerMode, ShDiagnosticsState, TriggerDiagnosticsRow,
};
