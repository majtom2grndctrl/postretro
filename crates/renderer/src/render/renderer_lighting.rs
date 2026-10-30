// Renderer lighting: SDF-shadow SH-grid metadata and the shared light record
// size, with light selection, bridge uploads and fog uploads in submodules.
// See: context/lib/rendering_pipeline.md §4

mod fog_uploads;
mod light_bridge;
mod light_selection;

use super::*;

pub(crate) use light_selection::*;

pub(super) const LIGHT_INFLUENCE_SIZE: usize = 16;

/// Pack the SH grid metadata the SDF shadow pass needs for its open-space
/// skip uniform. Mirrors what the forward pass reads from `ShGridInfo` (group
/// 3) — replicating it here lets the shadow pass keep group 3 off its
/// pipeline layout. Returns the "empty SH" defaults when the section is
/// absent or marked not-present, matching the dummy 1×1×1 path in
/// `ShVolumeResources`.
pub(crate) fn build_sdf_shadow_sh_grid(
    sh_volume: Option<&postretro_level_format::sh_volume::OctahedralShVolumeSection>,
    present: bool,
) -> SdfShadowShGrid {
    if !present {
        return SdfShadowShGrid::default();
    }
    let Some(sec) = sh_volume else {
        return SdfShadowShGrid::default();
    };
    build_sdf_shadow_sh_grid_from_metadata(sec.grid_origin, sec.cell_size, sec.grid_dimensions)
}

/// Streaming retains these id-34 metadata fields while omitting the compact
/// atlas body. The SDF shadow pass consumes the streamed depth-moment view, so
/// it needs the same open-space grid rather than the legacy-section gate.
pub(crate) fn build_sdf_shadow_sh_grid_from_metadata(
    origin: [f32; 3],
    cell_size: [f32; 3],
    dimensions: [u32; 3],
) -> SdfShadowShGrid {
    SdfShadowShGrid {
        origin,
        cell_size,
        dimensions,
        has_volume: true,
    }
}

/// Per-light delta AABB overlays no longer have a source: the sparse CSR delta
/// format (v2) is keyed by affinity cell, not per-light AABB grids, so there are
/// no per-light origin/dims to draw. Returns empty; the diagnostics consumer
/// skips the delta-AABB loop. A future affinity-cell overlay could repopulate
/// this from `affinity_dims` + the base grid origin/cell-size.
#[cfg(feature = "dev-tools")]
pub(crate) fn collect_delta_volume_meta(
    _section: Option<&postretro_level_format::delta_sh_volumes::DeltaShVolumesSection>,
) -> Vec<sh_volume::DeltaVolumeMeta> {
    Vec::new()
}
