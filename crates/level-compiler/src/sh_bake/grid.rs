// SH probe grid: layout, per-probe positions, and validity.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::collections::HashSet;

use glam::{DVec3, Vec3};

use super::{ShBakeCtx, ShConfig};
use crate::partition::{BspTree, find_leaf_for_point};

/// Geometry-derived layout of the probe grid, independent of lighting. Shared by
/// the whole-volume bake and the per-group warm path (`sh_group.rs`) so both
/// resolve the same grid origin/dims, per-probe positions, validity, and
/// `far_sentinel` — the warm group bake therefore lands probes at byte-identical
/// positions and seeds to the monolithic bake.
///
/// `probe_positions`/`validity` are indexed by the flat linear probe index
/// (`x + y*nx + z*nx*ny`, x-fastest). `is_empty()` is true when the map has no
/// geometry (the empty-section degradation path).
pub(crate) struct ProbeGridLayout {
    pub(crate) world_min: DVec3,
    pub(crate) dims: [u32; 3],
    pub(crate) cell_size: [f32; 3],
    pub(crate) far_sentinel: f32,
    pub(crate) probe_positions: Vec<DVec3>,
    pub(crate) validity: Vec<u8>,
}

impl ProbeGridLayout {
    pub(crate) fn is_empty(&self) -> bool {
        self.probe_positions.is_empty()
    }

    pub(crate) fn total_probes(&self) -> usize {
        self.probe_positions.len()
    }
}

/// Build the probe-grid layout from geometry + probe spacing. Mirrors the grid
/// setup `bake_sh_volume` performs; both call this so the warm group path stays
/// aligned to the whole-volume grid.
pub(crate) fn probe_grid_layout(inputs: &ShBakeCtx<'_>, config: &ShConfig) -> ProbeGridLayout {
    let probe_spacing_meters = config.probe_spacing;
    let cell_size = [probe_spacing_meters; 3];
    if inputs.geometry.geometry.vertices.is_empty() {
        return ProbeGridLayout {
            world_min: DVec3::ZERO,
            dims: [0, 0, 0],
            cell_size,
            far_sentinel: 4.0 * Vec3::from(cell_size).length(),
            probe_positions: Vec::new(),
            validity: Vec::new(),
        };
    }

    let (world_min, world_max) = world_aabb(inputs);
    let dims = grid_dimensions(world_min, world_max, probe_spacing_meters);
    let total = dims[0] as usize * dims[1] as usize * dims[2] as usize;

    let probe_positions: Vec<DVec3> = (0..total)
        .map(|i| probe_position(i, dims, world_min, probe_spacing_meters))
        .collect();

    let validity: Vec<u8> = probe_positions
        .iter()
        .map(|&p| {
            if probe_is_valid(inputs.tree, inputs.exterior_leaves, p) {
                1
            } else {
                0
            }
        })
        .collect();

    // Sky-miss rays contribute this distance to the depth moments. Cell-relative
    // (4× the full 3D cell diagonal) so it reads as "fully open" at the
    // probe-spacing scale the runtime Chebyshev interpolant operates in.
    let far_sentinel = 4.0 * Vec3::from(cell_size).length();

    ProbeGridLayout {
        world_min,
        dims,
        cell_size,
        far_sentinel,
        probe_positions,
        validity,
    }
}

fn world_aabb(inputs: &ShBakeCtx<'_>) -> (DVec3, DVec3) {
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for v in &inputs.geometry.geometry.vertices {
        let p = DVec3::new(
            v.position[0] as f64,
            v.position[1] as f64,
            v.position[2] as f64,
        );
        min = min.min(p);
        max = max.max(p);
    }
    (min, max)
}

pub(super) fn grid_dimensions(min: DVec3, max: DVec3, spacing: f32) -> [u32; 3] {
    let extents = (max - min).max(DVec3::splat(0.0));
    let spacing = spacing.max(1.0e-4) as f64;
    [
        ((extents.x / spacing).ceil() as u32 + 1).max(1),
        ((extents.y / spacing).ceil() as u32 + 1).max(1),
        ((extents.z / spacing).ceil() as u32 + 1).max(1),
    ]
}

/// z-major, then y, then x — matches the format crate's probe iteration order.
pub(super) fn probe_index_to_xyz(linear: usize, dims: [u32; 3]) -> (u32, u32, u32) {
    let nx = dims[0] as usize;
    let ny = dims[1] as usize;
    let z = linear / (nx * ny);
    let rem = linear - z * nx * ny;
    let y = rem / nx;
    let x = rem - y * nx;
    (x as u32, y as u32, z as u32)
}

fn probe_position(linear: usize, dims: [u32; 3], origin: DVec3, spacing: f32) -> DVec3 {
    let (x, y, z) = probe_index_to_xyz(linear, dims);
    DVec3::new(
        origin.x + x as f64 * spacing as f64,
        origin.y + y as f64 * spacing as f64,
        origin.z + z as f64 * spacing as f64,
    )
}

pub(super) fn probe_is_valid(tree: &BspTree, exterior: &HashSet<usize>, pos: DVec3) -> bool {
    if tree.leaves.is_empty() {
        return true;
    }
    let leaf = find_leaf_for_point(tree, pos);
    if tree.leaves[leaf].is_solid {
        return false;
    }
    // Exterior leaves pour out-of-map radiance into trilinear neighbors — see sub-plan 10 §"Fix D".
    !exterior.contains(&leaf)
}

pub(crate) fn vec3_from(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

pub(crate) fn probe_is_valid_pub(tree: &BspTree, exterior: &HashSet<usize>, pos: DVec3) -> bool {
    probe_is_valid(tree, exterior, pos)
}
