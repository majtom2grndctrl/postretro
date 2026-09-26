//! Plain-data sampled-SH region inputs shared with the application.
//!
//! Regions deliberately carry only world-space bounds. Affinity-row addresses,
//! residency, and scaled-node writer closure remain renderer-owned details.

use glam::Vec3;

/// One world-space AABB whose drawn pixels may sample the SH probe volume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShSampleRegion {
    pub min: Vec3,
    pub max: Vec3,
}

impl ShSampleRegion {
    pub const fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }
}

/// App-owned non-mesh consumer bounds for one frame. Renderer-admitted mesh
/// plans contribute separately, after budget and cache admission. The renderer
/// resolves all world regions to private affinity rows after the residency drain.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShSampleRegionSets<'a> {
    pub visible_cells: &'a [ShSampleRegion],
    /// Portal/fog-reachable cell bounds already materialized by visibility
    /// preparation. Empty retains the established DrawAll sentinel.
    pub fog_cells: &'a [(Vec3, Vec3)],
    pub movers: &'a [ShSampleRegion],
}
