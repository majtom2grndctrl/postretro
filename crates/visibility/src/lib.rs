// CPU-side runtime portal traversal and frustum visibility, and the planar water mirror's crop and walk.
// See: context/lib/build_pipeline.md §Runtime visibility · context/lib/rendering_pipeline.md §2, §7.3a

mod cpu_stages;
mod mirror_crop;
mod mirror_visibility;
mod portal_vis;
mod region_walk;
mod visibility;

/// Epoch of the portal walk's output contract: which cells a pose reaches.
/// Baked consumers that sample the walk offline (the id-51 cell residency
/// set) hash it into their cache keys, so a walk change re-bakes them instead
/// of serving sets from the old walk. Bump it whenever a change can alter any
/// pose's visible set: traversal rule, region snapping, clipping, epsilons,
/// the step cap, the fallback set, or portal-plane orientation (built in
/// `postretro-level-loader`'s `portal_planes`, which the facing rule reads).
/// A pure speedup with identical output keeps it.
pub const PORTAL_WALK_EPOCH: u32 = 3;

pub use cpu_stages::VisibilityStage;
pub use mirror_crop::{MIRROR_CROP_PAD_RADIANS, MirrorCropScratch, crop_mirror_view_proj};
#[cfg(any(test, feature = "test-seams"))]
pub use mirror_visibility::determine_mirror_visible_cells_capped;
pub use mirror_visibility::{
    MIRROR_CLIP_BIAS, MIRROR_FOG_BAND, MirrorView, MirrorVisibility, MirrorVisibilityPath,
    determine_mirror_visible_cells,
};
pub use postretro_stage_timing::TimingGate;
pub use region_walk::{
    PortalTraversal, PortalWalkScratch, SeededWalk, portal_traverse, portal_walk_seeded,
};
pub use visibility::{
    CameraCullVisibility, Frustum, FrustumPlane, VisibilityPath, VisibilityResult, VisibilityStats,
    VisibleCells, determine_visible_cells, extract_frustum_planes,
};
