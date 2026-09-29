// CPU-side runtime portal traversal and frustum visibility.
// See: context/lib/build_pipeline.md §Runtime visibility

mod cpu_stages;
mod portal_vis;
mod visibility;

/// Epoch of the portal walk's output contract: which cells a pose reaches.
/// Baked consumers that sample the walk offline (the id-51 cell residency
/// set) hash it into their cache keys, so a walk change re-bakes them instead
/// of serving sets from the old walk. Bump it whenever a change can alter any
/// pose's visible set: traversal rule, clipping, epsilons, the step cap or the
/// fallback set. A pure speedup with identical output keeps it.
pub const PORTAL_WALK_EPOCH: u32 = 1;

pub use cpu_stages::VisibilityStage;
pub use portal_vis::{narrow_frustum, portal_traverse};
pub use postretro_stage_timing::TimingGate;
pub use visibility::{
    CameraCullVisibility, Frustum, FrustumPlane, VisibilityPath, VisibilityResult, VisibilityStats,
    VisibleCells, determine_visible_cells, extract_frustum_planes,
};
