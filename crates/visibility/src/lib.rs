// CPU-side runtime portal traversal and frustum visibility.
// See: context/lib/build_pipeline.md §Runtime visibility

mod portal_vis;
mod visibility;

pub use portal_vis::{narrow_frustum, portal_traverse};
pub use visibility::{
    CameraCullVisibility, Frustum, FrustumPlane, VisibilityPath, VisibilityResult, VisibilityStats,
    VisibleCells, determine_visible_cells, extract_frustum_planes,
};
