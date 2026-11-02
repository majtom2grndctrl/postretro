//! Plain-data sampled-SH region inputs shared with the application.
//!
//! Inputs carry only world-space bounds and runtime cell ids. Affinity-row
//! addresses, residency, and scaled-node writer closure remain renderer-owned
//! details.

use postretro_visibility::VisibleCells;

/// One world-space AABB whose drawn pixels may sample the SH probe volume.
/// Defined beside the residency plan, which indexes cell bounds with it.
pub use postretro_render_cpu::sh_residency::ShSampleRegion;

/// App-owned non-mesh consumers for one frame. Cells are named by runtime
/// cell id: the renderer indexed each cell's bounds at level install, so a
/// frame's cell gate costs a lookup rather than a world-space walk.
/// Renderer-admitted mesh plans contribute separately, after budget and cache
/// admission. The renderer resolves all of these to private affinity rows
/// after the residency drain.
#[derive(Debug, Clone, Copy)]
pub struct ShSampleRegionSets<'a> {
    /// Drawable cells from visibility; `DrawAll` names every runtime cell.
    pub visible_cells: &'a VisibleCells,
    /// Portal/fog-reachable cell ids from visibility. Empty retains the
    /// established DrawAll sentinel.
    pub fog_cells: &'a [u32],
    /// Drawn movers' swept bounds.
    pub movers: &'a [ShSampleRegion],
}
