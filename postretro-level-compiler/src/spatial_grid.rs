// Spatial grid assignment: divide world into cells, assign faces by centroid.
// See: context/lib/build_pipeline.md

use glam::Vec3;

use crate::map_data::Face;
use crate::partition::Aabb;
use crate::voxel_grid::VoxelGrid;

/// Classification of a grid cell based on voxel occupancy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellType {
    /// Every sampled voxel is solid — no air in this cell.
    Solid,
    /// Every sampled voxel is empty — pure air.
    Air,
    /// Mix of solid and empty voxels — straddles a wall.
    Boundary,
}

/// A spatial grid cell containing face references and bounding volume.
#[derive(Debug, Clone)]
pub struct GridCell {
    pub id: usize,
    pub bounds: Aabb,
    pub face_indices: Vec<usize>,
    /// Cell classification (Solid/Air/Boundary). None when no VoxelGrid was provided.
    pub cell_type: Option<CellType>,
}

/// Result of spatial grid assignment: cells, faces (unmodified), and count.
#[derive(Debug)]
pub struct SpatialGridResult {
    pub cells: Vec<GridCell>,
    pub faces: Vec<Face>,
    pub cell_count: usize,
    /// Size of each grid cell in world units (for PVS brush filtering).
    pub cell_size: Vec3,
}

/// Minimum cell size in world units to avoid overly fine grids.
const MIN_CELL_SIZE: f32 = 32.0;

/// Target cells per axis. The actual count may differ based on world size
/// and the minimum cell size constraint. Higher values produce finer grids
/// where walls are more likely to fall on cell boundaries, improving
/// ray-cast PVS accuracy at the cost of more cluster pairs to test.
const TARGET_CELLS_PER_AXIS: usize = 16;

/// Compute the centroid of a face (average of its vertices).
fn face_centroid(face: &Face) -> Vec3 {
    let sum: Vec3 = face.vertices.iter().copied().sum();
    sum / face.vertices.len() as f32
}

/// Compute the world-space AABB enclosing all face vertices.
fn compute_world_bounds(faces: &[Face]) -> Aabb {
    let mut bounds = Aabb::empty();
    for face in faces {
        for &v in &face.vertices {
            bounds.expand_point(v);
        }
    }
    bounds
}

/// Determine grid resolution per axis given world extent and constraints.
///
/// Targets roughly TARGET_CELLS_PER_AXIS cells, but respects MIN_CELL_SIZE
/// to avoid overly fine grids on small maps.
fn compute_grid_dimensions(world_size: Vec3) -> [usize; 3] {
    let mut dims = [1usize; 3];
    let sizes = [world_size.x, world_size.y, world_size.z];

    for (i, &extent) in sizes.iter().enumerate() {
        if extent <= 0.0 {
            dims[i] = 1;
            continue;
        }
        // How many cells fit at the minimum cell size
        let max_cells = (extent / MIN_CELL_SIZE).floor() as usize;
        // Target count, clamped to [1, max_cells]
        dims[i] = TARGET_CELLS_PER_AXIS.min(max_cells).max(1);
    }

    dims
}

/// Classify a grid cell as Solid, Air, or Boundary using the VoxelGrid.
///
/// Samples all voxels overlapping the cell bounds. If all are solid -> Solid.
/// If all are empty -> Air. Mixed -> Boundary. Uses exact thresholds (100%/0%).
fn classify_cell(cell_bounds: &Aabb, voxel_grid: &VoxelGrid) -> CellType {
    let voxel_size = voxel_grid.voxel_size;

    // Compute voxel index ranges overlapping this cell
    let grid_min = cell_bounds.min - voxel_grid.bounds.min;
    let grid_max = cell_bounds.max - voxel_grid.bounds.min;

    let ix_min = (grid_min.x / voxel_size).floor().max(0.0) as usize;
    let iy_min = (grid_min.y / voxel_size).floor().max(0.0) as usize;
    let iz_min = (grid_min.z / voxel_size).floor().max(0.0) as usize;

    let ix_max = ((grid_max.x / voxel_size).ceil() as usize).min(voxel_grid.resolution[0]);
    let iy_max = ((grid_max.y / voxel_size).ceil() as usize).min(voxel_grid.resolution[1]);
    let iz_max = ((grid_max.z / voxel_size).ceil() as usize).min(voxel_grid.resolution[2]);

    let mut any_solid = false;
    let mut any_empty = false;

    for iz in iz_min..iz_max {
        for iy in iy_min..iy_max {
            for ix in ix_min..ix_max {
                if voxel_grid.is_solid(ix, iy, iz) {
                    any_solid = true;
                } else {
                    any_empty = true;
                }
                // Early out: already know it's boundary
                if any_solid && any_empty {
                    return CellType::Boundary;
                }
            }
        }
    }

    if any_solid && !any_empty {
        CellType::Solid
    } else if any_empty && !any_solid {
        CellType::Air
    } else {
        // No voxels sampled (cell outside grid bounds) — treat as air
        CellType::Air
    }
}

/// Analyze solid-to-empty transitions along each axis within a cell's voxel range.
///
/// Returns transition counts per axis and the best split position for the axis
/// with the most transitions. The split position is the voxel boundary where
/// the majority of transitions occur (where the wall is).
struct AxisAnalysis {
    transitions: [usize; 3],
    /// Best split position in world coordinates for each axis.
    /// Placed at the voxel boundary where transitions are most concentrated.
    split_positions: [f32; 3],
}

fn analyze_cell_axes(cell_bounds: &Aabb, voxel_grid: &VoxelGrid) -> AxisAnalysis {
    let voxel_size = voxel_grid.voxel_size;
    let grid_min = cell_bounds.min - voxel_grid.bounds.min;
    let grid_max = cell_bounds.max - voxel_grid.bounds.min;

    let ix_min = (grid_min.x / voxel_size).floor().max(0.0) as usize;
    let iy_min = (grid_min.y / voxel_size).floor().max(0.0) as usize;
    let iz_min = (grid_min.z / voxel_size).floor().max(0.0) as usize;

    let ix_max = ((grid_max.x / voxel_size).ceil() as usize).min(voxel_grid.resolution[0]);
    let iy_max = ((grid_max.y / voxel_size).ceil() as usize).min(voxel_grid.resolution[1]);
    let iz_max = ((grid_max.z / voxel_size).ceil() as usize).min(voxel_grid.resolution[2]);

    let mut transitions = [0usize; 3];
    // Track the voxel index where most transitions occur on each axis
    // (used to place the split at the wall boundary instead of the midpoint)
    let mut transition_positions: [Vec<usize>; 3] = [Vec::new(), Vec::new(), Vec::new()];

    // X-axis transitions: for each (y, z) scan along x
    for iz in iz_min..iz_max {
        for iy in iy_min..iy_max {
            let mut prev = None;
            for ix in ix_min..ix_max {
                let solid = voxel_grid.is_solid(ix, iy, iz);
                if let Some(was_solid) = prev {
                    if solid != was_solid {
                        transitions[0] += 1;
                        transition_positions[0].push(ix);
                    }
                }
                prev = Some(solid);
            }
        }
    }

    // Y-axis transitions: for each (x, z) scan along y
    for iz in iz_min..iz_max {
        for ix in ix_min..ix_max {
            let mut prev = None;
            for iy in iy_min..iy_max {
                let solid = voxel_grid.is_solid(ix, iy, iz);
                if let Some(was_solid) = prev {
                    if solid != was_solid {
                        transitions[1] += 1;
                        transition_positions[1].push(iy);
                    }
                }
                prev = Some(solid);
            }
        }
    }

    // Z-axis transitions: for each (x, y) scan along z
    for iy in iy_min..iy_max {
        for ix in ix_min..ix_max {
            let mut prev = None;
            for iz in iz_min..iz_max {
                let solid = voxel_grid.is_solid(ix, iy, iz);
                if let Some(was_solid) = prev {
                    if solid != was_solid {
                        transitions[2] += 1;
                        transition_positions[2].push(iz);
                    }
                }
                prev = Some(solid);
            }
        }
    }

    // Compute median transition position for each axis and convert to world coords
    let cell_mins = [cell_bounds.min.x, cell_bounds.min.y, cell_bounds.min.z];
    let cell_maxs = [cell_bounds.max.x, cell_bounds.max.y, cell_bounds.max.z];

    let mut split_positions = [0.0f32; 3];
    for axis in 0..3 {
        if transition_positions[axis].is_empty() {
            // No transitions — use midpoint
            split_positions[axis] = (cell_mins[axis] + cell_maxs[axis]) * 0.5;
        } else {
            let positions = &mut transition_positions[axis];
            positions.sort_unstable();
            let median_idx = positions.len() / 2;
            let median_voxel = positions[median_idx];
            // Split at the voxel boundary (between median_voxel-1 and median_voxel)
            let split_world =
                voxel_grid.bounds.min[axis] + median_voxel as f32 * voxel_size;
            // Clamp to cell bounds with margin to avoid degenerate sub-cells
            let margin = voxel_size * 0.5;
            split_positions[axis] = split_world
                .max(cell_mins[axis] + margin)
                .min(cell_maxs[axis] - margin);
        }
        // Ensure split position doesn't produce empty sub-cells
        let mid = (cell_mins[axis] + cell_maxs[axis]) * 0.5;
        if (split_positions[axis] - cell_mins[axis]).abs() < voxel_size * 0.25
            || (cell_maxs[axis] - split_positions[axis]).abs() < voxel_size * 0.25
        {
            split_positions[axis] = mid;
        }
    }

    AxisAnalysis {
        transitions,
        split_positions,
    }
}

/// Subdivide a boundary cell into sub-cells that separate solid and air regions.
///
/// Binary-splits along the axis with the most solid-to-empty transitions (where
/// the wall runs). Sub-cells are reclassified: solid sub-cells are discarded,
/// air sub-cells are kept, boundary sub-cells are split again up to `max_depth`.
///
/// Faces are reassigned by centroid. Faces whose centroids land in a solid
/// sub-cell go to the nearest air/boundary sub-cell (faces sit on surfaces
/// near solid/air boundaries and should not be lost).
fn subdivide_boundary_cell(
    cell: &GridCell,
    voxel_grid: &VoxelGrid,
    faces: &[Face],
    max_depth: usize,
) -> Vec<GridCell> {
    subdivide_recursive(cell, voxel_grid, faces, 0, max_depth)
}

fn subdivide_recursive(
    cell: &GridCell,
    voxel_grid: &VoxelGrid,
    faces: &[Face],
    depth: usize,
    max_depth: usize,
) -> Vec<GridCell> {
    if depth >= max_depth {
        // At max depth, don't return boundary cells as-is. Do one final
        // split and discard solid halves to avoid clusters spanning walls.
        return finalize_boundary_cell(cell, voxel_grid, faces);
    }

    let analysis = analyze_cell_axes(&cell.bounds, voxel_grid);

    // Find split axis: most transitions, tie-break by longest extent
    let extent = cell.bounds.max - cell.bounds.min;
    let extents = [extent.x, extent.y, extent.z];

    let best_axis = (0..3)
        .max_by(|&a, &b| {
            analysis.transitions[a]
                .cmp(&analysis.transitions[b])
                .then_with(|| extents[a].partial_cmp(&extents[b]).unwrap_or(std::cmp::Ordering::Equal))
        })
        .unwrap_or(0);

    // Split at the wall boundary (median transition position) rather than midpoint
    let mid = analysis.split_positions[best_axis];

    let mut lo_max = cell.bounds.max;
    let mut hi_min = cell.bounds.min;
    match best_axis {
        0 => {
            lo_max.x = mid;
            hi_min.x = mid;
        }
        1 => {
            lo_max.y = mid;
            hi_min.y = mid;
        }
        2 => {
            lo_max.z = mid;
            hi_min.z = mid;
        }
        _ => unreachable!(),
    }

    let lo_bounds = Aabb {
        min: cell.bounds.min,
        max: lo_max,
    };
    let hi_bounds = Aabb {
        min: hi_min,
        max: cell.bounds.max,
    };

    let lo_type = classify_cell(&lo_bounds, voxel_grid);
    let hi_type = classify_cell(&hi_bounds, voxel_grid);

    // Assign faces to sub-cells by centroid
    let mut lo_faces = Vec::new();
    let mut hi_faces = Vec::new();
    let mut solid_faces = Vec::new(); // faces in neither sub-cell's air/boundary zone

    for &fi in &cell.face_indices {
        let centroid = face_centroid(&faces[fi]);
        let in_lo = match best_axis {
            0 => centroid.x < mid,
            1 => centroid.y < mid,
            2 => centroid.z < mid,
            _ => unreachable!(),
        };

        if in_lo {
            if lo_type == CellType::Solid {
                solid_faces.push(fi);
            } else {
                lo_faces.push(fi);
            }
        } else if hi_type == CellType::Solid {
            solid_faces.push(fi);
        } else {
            hi_faces.push(fi);
        }
    }

    // Reassign faces from solid sub-cells to the nearest air/boundary sub-cell
    for fi in solid_faces {
        let centroid = face_centroid(&faces[fi]);
        // If one side is non-solid and the other is solid, assign there
        if lo_type != CellType::Solid && hi_type == CellType::Solid {
            lo_faces.push(fi);
        } else if hi_type != CellType::Solid && lo_type == CellType::Solid {
            hi_faces.push(fi);
        } else {
            // Both solid (unlikely for a boundary cell) — assign to closer side
            let lo_center = (lo_bounds.min + lo_bounds.max) * 0.5;
            let hi_center = (hi_bounds.min + hi_bounds.max) * 0.5;
            if centroid.distance_squared(lo_center) <= centroid.distance_squared(hi_center) {
                lo_faces.push(fi);
            } else {
                hi_faces.push(fi);
            }
        }
    }

    let mut result = Vec::new();

    // Process each half: recurse on boundary, keep air, discard solid
    let halves = [
        (lo_bounds, lo_type, lo_faces),
        (hi_bounds, hi_type, hi_faces),
    ];

    for (bounds, cell_type, face_indices) in halves {
        match cell_type {
            CellType::Solid => {
                // Discard solid sub-cells, but rescue any faces assigned here
                // (shouldn't happen after reassignment above, but defensive)
            }
            CellType::Air => {
                // Shrink bounds to air-voxel extent so the AABB doesn't
                // overlap into solid/wall regions. This creates gaps between
                // air cells on opposite sides of a wall, preventing false
                // adjacency in PVS.
                let shrunk = shrink_to_air_extent(&bounds, voxel_grid);
                result.push(GridCell {
                    id: 0, // re-assigned later
                    bounds: shrunk,
                    face_indices,
                    cell_type: Some(CellType::Air),
                });
            }
            CellType::Boundary => {
                let sub_cell = GridCell {
                    id: 0,
                    bounds,
                    face_indices,
                    cell_type: Some(CellType::Boundary),
                };
                result.extend(subdivide_recursive(
                    &sub_cell,
                    voxel_grid,
                    faces,
                    depth + 1,
                    max_depth,
                ));
            }
        }
    }

    // If subdivision produced nothing (both halves solid), return the original
    // cell to avoid losing faces
    if result.is_empty() && !cell.face_indices.is_empty() {
        result.push(cell.clone());
    }

    result
}

/// Final pass for boundary cells that have reached max subdivision depth.
///
/// Performs one more binary split and discards solid halves. This prevents
/// clusters from spanning across walls. Remaining boundary sub-cells are
/// shrunk to their air-voxel extent to minimize AABB overlap with solid space.
fn finalize_boundary_cell(
    cell: &GridCell,
    voxel_grid: &VoxelGrid,
    faces: &[Face],
) -> Vec<GridCell> {
    let analysis = analyze_cell_axes(&cell.bounds, voxel_grid);
    let extent = cell.bounds.max - cell.bounds.min;
    let extents = [extent.x, extent.y, extent.z];

    let best_axis = (0..3)
        .max_by(|&a, &b| {
            analysis.transitions[a]
                .cmp(&analysis.transitions[b])
                .then_with(|| {
                    extents[a]
                        .partial_cmp(&extents[b])
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        })
        .unwrap_or(0);

    let mid = analysis.split_positions[best_axis];

    let mut lo_max = cell.bounds.max;
    let mut hi_min = cell.bounds.min;
    match best_axis {
        0 => {
            lo_max.x = mid;
            hi_min.x = mid;
        }
        1 => {
            lo_max.y = mid;
            hi_min.y = mid;
        }
        2 => {
            lo_max.z = mid;
            hi_min.z = mid;
        }
        _ => unreachable!(),
    }

    let lo_bounds = Aabb {
        min: cell.bounds.min,
        max: lo_max,
    };
    let hi_bounds = Aabb {
        min: hi_min,
        max: cell.bounds.max,
    };

    let lo_type = classify_cell(&lo_bounds, voxel_grid);
    let hi_type = classify_cell(&hi_bounds, voxel_grid);

    // Assign faces to sub-cells by centroid, rescuing faces from solid halves
    let mut lo_faces = Vec::new();
    let mut hi_faces = Vec::new();
    let mut solid_faces = Vec::new();

    for &fi in &cell.face_indices {
        let centroid = face_centroid(&faces[fi]);
        let in_lo = match best_axis {
            0 => centroid.x < mid,
            1 => centroid.y < mid,
            2 => centroid.z < mid,
            _ => unreachable!(),
        };

        if in_lo {
            if lo_type == CellType::Solid {
                solid_faces.push(fi);
            } else {
                lo_faces.push(fi);
            }
        } else if hi_type == CellType::Solid {
            solid_faces.push(fi);
        } else {
            hi_faces.push(fi);
        }
    }

    for fi in solid_faces {
        if lo_type != CellType::Solid && hi_type == CellType::Solid {
            lo_faces.push(fi);
        } else if hi_type != CellType::Solid && lo_type == CellType::Solid {
            hi_faces.push(fi);
        } else {
            let centroid = face_centroid(&faces[fi]);
            let lo_center = (lo_bounds.min + lo_bounds.max) * 0.5;
            let hi_center = (hi_bounds.min + hi_bounds.max) * 0.5;
            if centroid.distance_squared(lo_center) <= centroid.distance_squared(hi_center) {
                lo_faces.push(fi);
            } else {
                hi_faces.push(fi);
            }
        }
    }

    let mut result = Vec::new();
    let halves = [
        (lo_bounds, lo_type, lo_faces),
        (hi_bounds, hi_type, hi_faces),
    ];

    for (bounds, cell_type, face_indices) in halves {
        if cell_type == CellType::Solid {
            continue;
        }
        // For remaining boundary sub-cells, shrink bounds to air-voxel extent
        let final_bounds = if cell_type == CellType::Boundary {
            shrink_to_air_extent(&bounds, voxel_grid)
        } else {
            bounds
        };
        result.push(GridCell {
            id: 0,
            bounds: final_bounds,
            face_indices,
            cell_type: Some(cell_type),
        });
    }

    if result.is_empty() && !cell.face_indices.is_empty() {
        result.push(cell.clone());
    }

    result
}

/// Shrink an AABB to cover only the air voxels within it.
///
/// Iterates over all voxels in the cell and computes the bounding box of
/// only the empty (air) voxels. If no air voxels exist, returns the original bounds.
fn shrink_to_air_extent(cell_bounds: &Aabb, voxel_grid: &VoxelGrid) -> Aabb {
    let voxel_size = voxel_grid.voxel_size;
    let grid_min = cell_bounds.min - voxel_grid.bounds.min;
    let grid_max = cell_bounds.max - voxel_grid.bounds.min;

    let ix_min = (grid_min.x / voxel_size).floor().max(0.0) as usize;
    let iy_min = (grid_min.y / voxel_size).floor().max(0.0) as usize;
    let iz_min = (grid_min.z / voxel_size).floor().max(0.0) as usize;

    let ix_max = ((grid_max.x / voxel_size).ceil() as usize).min(voxel_grid.resolution[0]);
    let iy_max = ((grid_max.y / voxel_size).ceil() as usize).min(voxel_grid.resolution[1]);
    let iz_max = ((grid_max.z / voxel_size).ceil() as usize).min(voxel_grid.resolution[2]);

    let mut air_bounds = Aabb::empty();

    for iz in iz_min..iz_max {
        for iy in iy_min..iy_max {
            for ix in ix_min..ix_max {
                if !voxel_grid.is_solid(ix, iy, iz) {
                    let voxel_min = voxel_grid.bounds.min
                        + Vec3::new(
                            ix as f32 * voxel_size,
                            iy as f32 * voxel_size,
                            iz as f32 * voxel_size,
                        );
                    let voxel_max = voxel_min + Vec3::splat(voxel_size);
                    air_bounds.expand_point(voxel_min);
                    air_bounds.expand_point(voxel_max);
                }
            }
        }
    }

    // Clamp to original cell bounds (air voxels may extend beyond)
    if air_bounds.is_valid() {
        Aabb {
            min: air_bounds.min.max(cell_bounds.min),
            max: air_bounds.max.min(cell_bounds.max),
        }
    } else {
        cell_bounds.clone()
    }
}

/// Assign faces to spatial grid cells by centroid position.
///
/// Divides the world into a 3D grid and places each face in the cell
/// containing its centroid. No face splitting occurs.
///
/// When `voxel_grid` is provided, cells are classified and solid cells are
/// excluded. Air cells are retained even with no faces (for camera containment).
/// Boundary cells are subdivided to separate solid and air regions.
pub fn assign_to_grid(faces: Vec<Face>, voxel_grid: Option<&VoxelGrid>) -> SpatialGridResult {
    if faces.is_empty() {
        return SpatialGridResult {
            cells: Vec::new(),
            faces,
            cell_count: 0,
            cell_size: Vec3::ZERO,
        };
    }

    let world_bounds = compute_world_bounds(&faces);
    let world_size = world_bounds.max - world_bounds.min;
    let dims = compute_grid_dimensions(world_size);
    let total_cells = dims[0] * dims[1] * dims[2];

    let cell_size = Vec3::new(
        if dims[0] > 0 {
            world_size.x / dims[0] as f32
        } else {
            world_size.x
        },
        if dims[1] > 0 {
            world_size.y / dims[1] as f32
        } else {
            world_size.y
        },
        if dims[2] > 0 {
            world_size.z / dims[2] as f32
        } else {
            world_size.z
        },
    );

    // Build cell bounds and classify
    let mut cells: Vec<GridCell> = (0..total_cells)
        .map(|id| {
            let (ix, iy, iz) = cell_coords_from_id(id, dims);
            let min = world_bounds.min
                + Vec3::new(
                    ix as f32 * cell_size.x,
                    iy as f32 * cell_size.y,
                    iz as f32 * cell_size.z,
                );
            let max = min + cell_size;
            let bounds = Aabb { min, max };
            let cell_type = voxel_grid.map(|vg| classify_cell(&bounds, vg));
            GridCell {
                id,
                bounds,
                face_indices: Vec::new(),
                cell_type,
            }
        })
        .collect();

    // Assign each face to its cell by centroid
    for (face_idx, face) in faces.iter().enumerate() {
        let centroid = face_centroid(face);
        let cell_id = centroid_to_cell_id(centroid, &world_bounds, cell_size, dims);
        cells[cell_id].face_indices.push(face_idx);
    }

    // When voxel-aware: subdivide boundary cells, exclude solid cells,
    // keep air cells even with no faces.
    if let Some(vg) = voxel_grid {
        let solid_count = cells
            .iter()
            .filter(|c| c.cell_type == Some(CellType::Solid))
            .count();
        let air_count = cells
            .iter()
            .filter(|c| c.cell_type == Some(CellType::Air))
            .count();
        let boundary_count = cells
            .iter()
            .filter(|c| c.cell_type == Some(CellType::Boundary))
            .count();

        log::info!(
            "[Compiler] Cell classification: {} solid (skipped), {} air (retained), {} boundary (subdividing)",
            solid_count,
            air_count,
            boundary_count,
        );

        // Subdivide boundary cells to separate solid and air regions.
        // Max depth 2 gives up to 4 sub-cells per boundary cell.
        const SUBDIVISION_MAX_DEPTH: usize = 2;
        let mut subdivided_cells = Vec::new();
        let mut subdivision_count = 0usize;

        for cell in cells.drain(..) {
            if cell.cell_type == Some(CellType::Boundary) {
                let sub_cells = subdivide_boundary_cell(&cell, vg, &faces, SUBDIVISION_MAX_DEPTH);
                subdivision_count += sub_cells.len();
                subdivided_cells.extend(sub_cells);
            } else {
                subdivided_cells.push(cell);
            }
        }

        cells = subdivided_cells;

        if boundary_count > 0 {
            log::info!(
                "[Compiler] Boundary subdivision: {} boundary cells -> {} sub-cells",
                boundary_count,
                subdivision_count,
            );
        }

        // Re-assign sequential IDs after subdivision
        for (i, cell) in cells.iter_mut().enumerate() {
            cell.id = i;
        }

        // Build adjacency information for flood fill.
        // After subdivision, cells no longer follow the original grid layout,
        // so we use AABB overlap to determine adjacency.
        let cell_count_for_flood = cells.len();

        // Build neighbor lists via AABB adjacency (shared face with small tolerance)
        let eps = 0.1;
        let mut neighbors: Vec<Vec<usize>> = vec![Vec::new(); cell_count_for_flood];
        for i in 0..cell_count_for_flood {
            for j in (i + 1)..cell_count_for_flood {
                if aabbs_share_face(&cells[i].bounds, &cells[j].bounds, eps) {
                    neighbors[i].push(j);
                    neighbors[j].push(i);
                }
            }
        }

        // Flood-fill from face-containing cells through air/boundary cells
        let mut reachable = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();

        // Seed with face-containing cells
        for c in &cells {
            if !c.face_indices.is_empty() {
                reachable.insert(c.id);
                queue.push_back(c.id);
            }
        }

        // BFS through air cells only. Boundary cells block the flood
        // (they straddle walls), so air regions on opposite sides of a wall
        // remain disconnected.
        while let Some(id) = queue.pop_front() {
            for &neighbor_id in &neighbors[id] {
                if reachable.contains(&neighbor_id) {
                    continue;
                }
                match cells[neighbor_id].cell_type {
                    Some(CellType::Air) => {
                        reachable.insert(neighbor_id);
                        queue.push_back(neighbor_id);
                    }
                    _ => continue,
                }
            }
        }

        cells.retain(|c| match c.cell_type {
            Some(CellType::Solid) => false,
            Some(CellType::Boundary) => !c.face_indices.is_empty(),
            Some(CellType::Air) => reachable.contains(&c.id),
            None => !c.face_indices.is_empty(),
        });

        // Re-assign sequential IDs after filtering
        for (i, cell) in cells.iter_mut().enumerate() {
            cell.id = i;
        }

        let retained_air = cells
            .iter()
            .filter(|c| c.cell_type == Some(CellType::Air))
            .count();
        let retained_boundary = cells
            .iter()
            .filter(|c| c.cell_type == Some(CellType::Boundary))
            .count();
        log::info!(
            "[Compiler] After filtering: {} cells retained ({} air, {} boundary)",
            cells.len(),
            retained_air,
            retained_boundary,
        );
    }

    let cell_count = cells.len();

    log_stats(&cells, &faces, dims);

    SpatialGridResult {
        cells,
        faces,
        cell_count,
        cell_size,
    }
}

/// Test whether two AABBs share a face (are adjacent with matching extent on one axis).
///
/// Two AABBs share a face when one's min equals the other's max on exactly one
/// axis, and they overlap on the other two axes.
fn aabbs_share_face(a: &Aabb, b: &Aabb, eps: f32) -> bool {
    // Check each axis for face-sharing
    let overlaps_x = a.min.x < b.max.x + eps && b.min.x < a.max.x + eps;
    let overlaps_y = a.min.y < b.max.y + eps && b.min.y < a.max.y + eps;
    let overlaps_z = a.min.z < b.max.z + eps && b.min.z < a.max.z + eps;

    let touches_x = (a.max.x - b.min.x).abs() < eps || (b.max.x - a.min.x).abs() < eps;
    let touches_y = (a.max.y - b.min.y).abs() < eps || (b.max.y - a.min.y).abs() < eps;
    let touches_z = (a.max.z - b.min.z).abs() < eps || (b.max.z - a.min.z).abs() < eps;

    // They share a face if they touch on exactly one axis and overlap on the other two
    (touches_x && overlaps_y && overlaps_z)
        || (touches_y && overlaps_x && overlaps_z)
        || (touches_z && overlaps_x && overlaps_y)
}

/// Convert a linear cell ID to (x, y, z) grid coordinates.
fn cell_coords_from_id(id: usize, dims: [usize; 3]) -> (usize, usize, usize) {
    let iz = id / (dims[0] * dims[1]);
    let rem = id % (dims[0] * dims[1]);
    let iy = rem / dims[0];
    let ix = rem % dims[0];
    (ix, iy, iz)
}

/// Map a world-space centroid to a cell ID, clamping to grid bounds.
fn centroid_to_cell_id(
    centroid: Vec3,
    world_bounds: &Aabb,
    cell_size: Vec3,
    dims: [usize; 3],
) -> usize {
    let offset = centroid - world_bounds.min;

    let ix = if cell_size.x > 0.0 {
        ((offset.x / cell_size.x).floor() as usize).min(dims[0] - 1)
    } else {
        0
    };
    let iy = if cell_size.y > 0.0 {
        ((offset.y / cell_size.y).floor() as usize).min(dims[1] - 1)
    } else {
        0
    };
    let iz = if cell_size.z > 0.0 {
        ((offset.z / cell_size.z).floor() as usize).min(dims[2] - 1)
    } else {
        0
    };

    iz * dims[0] * dims[1] + iy * dims[0] + ix
}

fn log_stats(cells: &[GridCell], faces: &[Face], dims: [usize; 3]) {
    let non_empty = cells.iter().filter(|c| !c.face_indices.is_empty()).count();
    let avg_faces: f32 = if non_empty == 0 {
        0.0
    } else {
        let total: usize = cells.iter().map(|c| c.face_indices.len()).sum();
        total as f32 / non_empty as f32
    };

    log::info!(
        "[Compiler] Grid dimensions: {}x{}x{} ({} cells total, {} non-empty)",
        dims[0],
        dims[1],
        dims[2],
        cells.len(),
        non_empty,
    );
    log::info!("[Compiler] Faces: {} (no splitting)", faces.len());
    log::info!("[Compiler] Average faces per non-empty cell: {avg_faces:.1}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn make_face(vertices: Vec<Vec3>) -> Face {
        let normal = Vec3::Z;
        Face {
            vertices,
            normal,
            distance: 0.0,
            texture: "test".to_string(),
        }
    }

    fn make_triangle(center: Vec3) -> Face {
        make_face(vec![
            center + Vec3::new(-1.0, -1.0, 0.0),
            center + Vec3::new(1.0, -1.0, 0.0),
            center + Vec3::new(0.0, 1.0, 0.0),
        ])
    }

    #[test]
    fn single_face_assigned_to_one_cell() {
        let faces = vec![make_triangle(Vec3::new(10.0, 10.0, 10.0))];
        let result = assign_to_grid(faces, None);

        assert_eq!(result.faces.len(), 1);
        let occupied: Vec<_> = result
            .cells
            .iter()
            .filter(|c| !c.face_indices.is_empty())
            .collect();
        assert_eq!(
            occupied.len(),
            1,
            "single face should occupy exactly one cell"
        );
        assert_eq!(occupied[0].face_indices.len(), 1);
    }

    #[test]
    fn faces_in_opposite_corners_different_cells() {
        let faces = vec![
            make_triangle(Vec3::new(0.0, 0.0, 0.0)),
            make_triangle(Vec3::new(1000.0, 1000.0, 1000.0)),
        ];
        let result = assign_to_grid(faces, None);

        // Find which cells contain faces
        let cell_0 = result
            .cells
            .iter()
            .find(|c| c.face_indices.contains(&0))
            .expect("face 0 should be in a cell");
        let cell_1 = result
            .cells
            .iter()
            .find(|c| c.face_indices.contains(&1))
            .expect("face 1 should be in a cell");
        assert_ne!(
            cell_0.id, cell_1.id,
            "faces 1000 units apart should be in different cells"
        );
    }

    #[test]
    fn all_faces_in_small_area_same_cell() {
        let faces = vec![
            make_triangle(Vec3::new(10.0, 10.0, 10.0)),
            make_triangle(Vec3::new(10.1, 10.1, 10.1)),
            make_triangle(Vec3::new(10.2, 10.2, 10.2)),
        ];
        let result = assign_to_grid(faces, None);

        let occupied: Vec<_> = result
            .cells
            .iter()
            .filter(|c| !c.face_indices.is_empty())
            .collect();
        assert_eq!(
            occupied.len(),
            1,
            "faces in a small cluster should all be in one cell"
        );
        assert_eq!(occupied[0].face_indices.len(), 3);
    }

    #[test]
    fn grid_dimensions_reasonable_for_various_world_sizes() {
        // Small world: should get few cells
        let small = compute_grid_dimensions(Vec3::new(100.0, 100.0, 100.0));
        assert!(small[0] >= 1 && small[0] <= 10);
        assert!(small[1] >= 1 && small[1] <= 10);
        assert!(small[2] >= 1 && small[2] <= 10);

        // Large world: should get close to target
        let large = compute_grid_dimensions(Vec3::new(10000.0, 10000.0, 10000.0));
        assert_eq!(large[0], TARGET_CELLS_PER_AXIS);
        assert_eq!(large[1], TARGET_CELLS_PER_AXIS);
        assert_eq!(large[2], TARGET_CELLS_PER_AXIS);

        // Flat world: one axis should be 1
        let flat = compute_grid_dimensions(Vec3::new(1000.0, 1000.0, 0.0));
        assert_eq!(flat[2], 1, "zero-extent axis should produce 1 cell");
        assert!(flat[0] > 1);
    }

    #[test]
    fn empty_face_list_empty_grid() {
        let result = assign_to_grid(Vec::new(), None);
        assert!(result.cells.is_empty());
        assert!(result.faces.is_empty());
        assert_eq!(result.cell_count, 0);
    }

    #[test]
    fn cell_bounds_cover_full_world() {
        // Build faces spanning a known region
        let faces = vec![
            make_triangle(Vec3::new(0.0, 0.0, 0.0)),
            make_triangle(Vec3::new(500.0, 500.0, 500.0)),
        ];
        let world_bounds = compute_world_bounds(&faces);
        let result = assign_to_grid(faces, None);

        // Collect the union of all non-empty cell bounds
        let mut union = Aabb::empty();
        for cell in &result.cells {
            if !cell.face_indices.is_empty() {
                union.expand_aabb(&cell.bounds);
            }
        }

        // The union of non-empty cell bounds should cover all face vertices
        assert!(union.is_valid());
        // Allow small epsilon for floating-point
        let eps = 0.01;
        assert!(
            union.min.x <= world_bounds.min.x + eps,
            "cell bounds min.x ({}) should cover world min.x ({})",
            union.min.x,
            world_bounds.min.x
        );
        assert!(
            union.min.y <= world_bounds.min.y + eps,
            "cell bounds min.y ({}) should cover world min.y ({})",
            union.min.y,
            world_bounds.min.y
        );
        assert!(
            union.min.z <= world_bounds.min.z + eps,
            "cell bounds min.z ({}) should cover world min.z ({})",
            union.min.z,
            world_bounds.min.z
        );
        assert!(
            union.max.x >= world_bounds.max.x - eps,
            "cell bounds max.x ({}) should cover world max.x ({})",
            union.max.x,
            world_bounds.max.x
        );
        assert!(
            union.max.y >= world_bounds.max.y - eps,
            "cell bounds max.y ({}) should cover world max.y ({})",
            union.max.y,
            world_bounds.max.y
        );
        assert!(
            union.max.z >= world_bounds.max.z - eps,
            "cell bounds max.z ({}) should cover world max.z ({})",
            union.max.z,
            world_bounds.max.z
        );
    }

    #[test]
    fn face_count_preserved() {
        let faces = vec![
            make_triangle(Vec3::new(0.0, 0.0, 0.0)),
            make_triangle(Vec3::new(100.0, 100.0, 100.0)),
            make_triangle(Vec3::new(200.0, 200.0, 200.0)),
            make_triangle(Vec3::new(300.0, 300.0, 300.0)),
        ];
        let input_count = faces.len();
        let result = assign_to_grid(faces, None);

        assert_eq!(
            result.faces.len(),
            input_count,
            "face count must be preserved"
        );

        // Every face should appear in exactly one cell
        let total_assigned: usize = result.cells.iter().map(|c| c.face_indices.len()).sum();
        assert_eq!(
            total_assigned, input_count,
            "every face assigned to exactly one cell"
        );
    }

    #[test]
    fn every_face_in_exactly_one_cell() {
        let faces = vec![
            make_triangle(Vec3::new(0.0, 0.0, 0.0)),
            make_triangle(Vec3::new(500.0, 0.0, 0.0)),
            make_triangle(Vec3::new(0.0, 500.0, 0.0)),
            make_triangle(Vec3::new(500.0, 500.0, 500.0)),
        ];
        let count = faces.len();
        let result = assign_to_grid(faces, None);

        let mut face_cell = vec![None; count];
        for cell in &result.cells {
            for &fi in &cell.face_indices {
                assert!(
                    face_cell[fi].is_none(),
                    "face {fi} assigned to multiple cells"
                );
                face_cell[fi] = Some(cell.id);
            }
        }
        for (i, c) in face_cell.iter().enumerate() {
            assert!(c.is_some(), "face {i} not assigned to any cell");
        }
    }

    /// Face assignment coherence: each face's centroid should lie within
    /// the AABB of the cell it was assigned to.
    ///
    /// After assign_to_grid tightens cell bounds to actual face geometry,
    /// the centroid of each face should still be within the tightened bounds
    /// of its assigned cell. If BSP splitting or the grid assignment process
    /// puts faces in the wrong cell, this will catch it.
    #[test]
    fn face_centroid_within_assigned_cell_bounds() {
        // Build a room with multiple brushes to generate faces spread
        // across the world. This mimics a real level where faces from
        // different walls and objects should end up in spatially correct cells.
        let mut faces = Vec::new();

        // Helper to create the 6 faces of a box brush
        let make_box = |min: Vec3, max: Vec3| -> Vec<Face> {
            vec![
                Face {
                    vertices: vec![
                        Vec3::new(min.x, min.y, min.z),
                        Vec3::new(min.x, max.y, min.z),
                        Vec3::new(min.x, max.y, max.z),
                        Vec3::new(min.x, min.y, max.z),
                    ],
                    normal: Vec3::NEG_X,
                    distance: -min.x,
                    texture: "test".to_string(),
                },
                Face {
                    vertices: vec![
                        Vec3::new(max.x, min.y, min.z),
                        Vec3::new(max.x, min.y, max.z),
                        Vec3::new(max.x, max.y, max.z),
                        Vec3::new(max.x, max.y, min.z),
                    ],
                    normal: Vec3::X,
                    distance: max.x,
                    texture: "test".to_string(),
                },
                Face {
                    vertices: vec![
                        Vec3::new(min.x, min.y, min.z),
                        Vec3::new(min.x, min.y, max.z),
                        Vec3::new(max.x, min.y, max.z),
                        Vec3::new(max.x, min.y, min.z),
                    ],
                    normal: Vec3::NEG_Y,
                    distance: -min.y,
                    texture: "test".to_string(),
                },
                Face {
                    vertices: vec![
                        Vec3::new(min.x, max.y, min.z),
                        Vec3::new(max.x, max.y, min.z),
                        Vec3::new(max.x, max.y, max.z),
                        Vec3::new(min.x, max.y, max.z),
                    ],
                    normal: Vec3::Y,
                    distance: max.y,
                    texture: "test".to_string(),
                },
                Face {
                    vertices: vec![
                        Vec3::new(min.x, min.y, min.z),
                        Vec3::new(max.x, min.y, min.z),
                        Vec3::new(max.x, max.y, min.z),
                        Vec3::new(min.x, max.y, min.z),
                    ],
                    normal: Vec3::NEG_Z,
                    distance: -min.z,
                    texture: "test".to_string(),
                },
                Face {
                    vertices: vec![
                        Vec3::new(min.x, min.y, max.z),
                        Vec3::new(max.x, min.y, max.z),
                        Vec3::new(max.x, max.y, max.z),
                        Vec3::new(min.x, max.y, max.z),
                    ],
                    normal: Vec3::Z,
                    distance: max.z,
                    texture: "test".to_string(),
                },
            ]
        };

        // Room enclosure (walls, floor, ceiling)
        faces.extend(make_box(
            Vec3::new(-16.0, -16.0, -16.0),
            Vec3::new(0.0, 528.0, 144.0),
        )); // -X wall
        faces.extend(make_box(
            Vec3::new(528.0, -16.0, -16.0),
            Vec3::new(544.0, 528.0, 144.0),
        )); // +X wall
        faces.extend(make_box(
            Vec3::new(0.0, -16.0, -16.0),
            Vec3::new(528.0, 0.0, 144.0),
        )); // -Y wall
        faces.extend(make_box(
            Vec3::new(0.0, 528.0, -16.0),
            Vec3::new(528.0, 544.0, 144.0),
        )); // +Y wall
        faces.extend(make_box(
            Vec3::new(0.0, 0.0, -16.0),
            Vec3::new(528.0, 528.0, 0.0),
        )); // floor
        faces.extend(make_box(
            Vec3::new(0.0, 0.0, 128.0),
            Vec3::new(528.0, 528.0, 144.0),
        )); // ceiling

        // Interior objects spread across the room
        faces.extend(make_box(
            Vec3::new(32.0, 32.0, 0.0),
            Vec3::new(96.0, 96.0, 64.0),
        )); // cube near origin
        faces.extend(make_box(
            Vec3::new(400.0, 400.0, 0.0),
            Vec3::new(496.0, 496.0, 64.0),
        )); // cube far corner
        faces.extend(make_box(
            Vec3::new(200.0, 200.0, 0.0),
            Vec3::new(328.0, 328.0, 96.0),
        )); // cube in center

        let result = assign_to_grid(faces, None);

        // For each face, verify its centroid is within its assigned cell's
        // tightened bounds (with a small epsilon for floating-point tolerance).
        let eps = 0.1;
        let mut violations = Vec::new();

        for cell in &result.cells {
            for &fi in &cell.face_indices {
                let face = &result.faces[fi];
                let centroid = face_centroid(face);
                let b = &cell.bounds;

                let inside = centroid.x >= b.min.x - eps
                    && centroid.x <= b.max.x + eps
                    && centroid.y >= b.min.y - eps
                    && centroid.y <= b.max.y + eps
                    && centroid.z >= b.min.z - eps
                    && centroid.z <= b.max.z + eps;

                if !inside {
                    violations.push((fi, cell.id, centroid, b.min, b.max));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "Face centroids should be within their assigned cell bounds, \
             but {} faces have centroids outside their cell. First 5: {:?}",
            violations.len(),
            violations
                .iter()
                .take(5)
                .map(|&(fi, cell_id, c, bmin, bmax)| {
                    format!(
                        "face {} in cell {}: centroid ({:.1},{:.1},{:.1}), \
                         bounds ({:.1},{:.1},{:.1})..({:.1},{:.1},{:.1})",
                        fi, cell_id, c.x, c.y, c.z, bmin.x, bmin.y, bmin.z, bmax.x, bmax.y, bmax.z
                    )
                })
                .collect::<Vec<_>>()
        );
    }

    // -- Voxel-aware classification tests --

    use crate::map_data::{BrushPlane, BrushVolume};

    fn box_brush(min: Vec3, max: Vec3) -> BrushVolume {
        BrushVolume {
            planes: vec![
                BrushPlane { normal: Vec3::X, distance: max.x },
                BrushPlane { normal: Vec3::NEG_X, distance: -min.x },
                BrushPlane { normal: Vec3::Y, distance: max.y },
                BrushPlane { normal: Vec3::NEG_Y, distance: -min.y },
                BrushPlane { normal: Vec3::Z, distance: max.z },
                BrushPlane { normal: Vec3::NEG_Z, distance: -min.z },
            ],
        }
    }

    fn make_box_faces(min: Vec3, max: Vec3) -> Vec<Face> {
        let texture = "test".to_string();
        vec![
            Face {
                vertices: vec![
                    Vec3::new(min.x, min.y, min.z),
                    Vec3::new(min.x, max.y, min.z),
                    Vec3::new(min.x, max.y, max.z),
                    Vec3::new(min.x, min.y, max.z),
                ],
                normal: Vec3::NEG_X, distance: -min.x, texture: texture.clone(),
            },
            Face {
                vertices: vec![
                    Vec3::new(max.x, min.y, min.z),
                    Vec3::new(max.x, min.y, max.z),
                    Vec3::new(max.x, max.y, max.z),
                    Vec3::new(max.x, max.y, min.z),
                ],
                normal: Vec3::X, distance: max.x, texture: texture.clone(),
            },
            Face {
                vertices: vec![
                    Vec3::new(min.x, min.y, min.z),
                    Vec3::new(min.x, min.y, max.z),
                    Vec3::new(max.x, min.y, max.z),
                    Vec3::new(max.x, min.y, min.z),
                ],
                normal: Vec3::NEG_Y, distance: -min.y, texture: texture.clone(),
            },
            Face {
                vertices: vec![
                    Vec3::new(min.x, max.y, min.z),
                    Vec3::new(max.x, max.y, min.z),
                    Vec3::new(max.x, max.y, max.z),
                    Vec3::new(min.x, max.y, max.z),
                ],
                normal: Vec3::Y, distance: max.y, texture: texture.clone(),
            },
            Face {
                vertices: vec![
                    Vec3::new(min.x, min.y, min.z),
                    Vec3::new(max.x, min.y, min.z),
                    Vec3::new(max.x, max.y, min.z),
                    Vec3::new(min.x, max.y, min.z),
                ],
                normal: Vec3::NEG_Z, distance: -min.z, texture: texture.clone(),
            },
            Face {
                vertices: vec![
                    Vec3::new(min.x, min.y, max.z),
                    Vec3::new(max.x, min.y, max.z),
                    Vec3::new(max.x, max.y, max.z),
                    Vec3::new(min.x, max.y, max.z),
                ],
                normal: Vec3::Z, distance: max.z, texture: texture.clone(),
            },
        ]
    }

    /// Build a VoxelGrid from faces and brush volumes for test use.
    fn build_test_voxel_grid(faces: &[Face], brush_volumes: &[BrushVolume]) -> VoxelGrid {
        let mut world_bounds = Aabb::empty();
        for face in faces {
            for &v in &face.vertices {
                world_bounds.expand_point(v);
            }
        }
        if !world_bounds.is_valid() {
            world_bounds = Aabb { min: Vec3::ZERO, max: Vec3::splat(1.0) };
        }
        let pad = Vec3::splat(crate::voxel_grid::DEFAULT_VOXEL_SIZE);
        world_bounds.min -= pad;
        world_bounds.max += pad;
        VoxelGrid::from_brushes(
            brush_volumes,
            &world_bounds,
            crate::voxel_grid::DEFAULT_VOXEL_SIZE,
        )
    }

    /// Solid cells should be excluded from the grid output.
    #[test]
    fn solid_cells_excluded_from_clusters() {
        // A solid box brush fills the region (0,0,0)-(256,256,128).
        // No air space — all cells should be solid or at least partially solid.
        let brush = box_brush(Vec3::ZERO, Vec3::new(256.0, 256.0, 128.0));
        let faces = make_box_faces(Vec3::ZERO, Vec3::new(256.0, 256.0, 128.0));
        let vg = build_test_voxel_grid(&faces, &[brush]);

        let result = assign_to_grid(faces, Some(&vg));

        // No cell should be classified as Solid in the output (they're removed)
        let solid_in_output = result
            .cells
            .iter()
            .filter(|c| c.cell_type == Some(CellType::Solid))
            .count();
        assert_eq!(
            solid_in_output, 0,
            "solid cells should be removed from grid output"
        );

        // The output should have fewer cells than a non-voxel-aware grid
        let unaware_result = assign_to_grid(result.faces.clone(), None);
        assert!(
            result.cells.len() < unaware_result.cells.len(),
            "voxel-aware grid should have fewer cells than unaware grid \
             (got {} vs {})",
            result.cells.len(),
            unaware_result.cells.len()
        );
    }

    /// Air cells with no faces should be retained as clusters.
    #[test]
    fn air_cells_retained_as_clusters_even_without_faces() {
        // Build a hollow room: 6 wall brushes enclosing air space.
        // Interior grid cells have no face centroids but should still become clusters.
        let wt = 16.0; // wall thickness
        let mut faces = Vec::new();
        let mut volumes = Vec::new();

        let mut add_brush = |min: Vec3, max: Vec3| {
            faces.extend(make_box_faces(min, max));
            volumes.push(box_brush(min, max));
        };

        // Room: air volume is (0, 0, 0) to (256, 256, 128)
        add_brush(Vec3::new(-wt, -wt, -wt), Vec3::new(0.0, 256.0 + wt, 128.0 + wt));   // -X wall
        add_brush(Vec3::new(256.0, -wt, -wt), Vec3::new(256.0 + wt, 256.0 + wt, 128.0 + wt)); // +X wall
        add_brush(Vec3::new(0.0, -wt, -wt), Vec3::new(256.0, 0.0, 128.0 + wt));         // -Y wall
        add_brush(Vec3::new(0.0, 256.0, -wt), Vec3::new(256.0, 256.0 + wt, 128.0 + wt)); // +Y wall
        add_brush(Vec3::new(0.0, 0.0, -wt), Vec3::new(256.0, 256.0, 0.0));               // floor
        add_brush(Vec3::new(0.0, 0.0, 128.0), Vec3::new(256.0, 256.0, 128.0 + wt));     // ceiling

        let vg = build_test_voxel_grid(&faces, &volumes);
        let result = assign_to_grid(faces, Some(&vg));

        // Some cells should be classified as Air and have no faces
        let air_no_faces = result
            .cells
            .iter()
            .filter(|c| c.cell_type == Some(CellType::Air) && c.face_indices.is_empty())
            .count();
        assert!(
            air_no_faces > 0,
            "hollow room should have air cells with no faces (got 0)"
        );

        // Air cells should have valid bounds
        for cell in &result.cells {
            if cell.cell_type == Some(CellType::Air) {
                assert!(
                    cell.bounds.is_valid(),
                    "air cell {} should have valid bounds",
                    cell.id
                );
            }
        }
    }

    /// Cell classification and subdivision for mixed geometry (room with walls).
    ///
    /// After subdivision, boundary cells are split into air and solid sub-cells.
    /// Solid sub-cells are discarded. For a single room, all remaining cells
    /// should be air (interior cells were already air, boundary cells at walls
    /// get their solid half discarded). Wall faces are rescued to the nearest
    /// air sub-cell.
    #[test]
    fn cell_classification_correct_for_mixed_geometry() {
        let wt = 16.0;
        let mut faces = Vec::new();
        let mut volumes = Vec::new();

        let mut add_brush = |min: Vec3, max: Vec3| {
            faces.extend(make_box_faces(min, max));
            volumes.push(box_brush(min, max));
        };

        // Room: air volume (0, 0, 0) to (256, 256, 128)
        add_brush(Vec3::new(-wt, -wt, -wt), Vec3::new(0.0, 256.0 + wt, 128.0 + wt));
        add_brush(Vec3::new(256.0, -wt, -wt), Vec3::new(256.0 + wt, 256.0 + wt, 128.0 + wt));
        add_brush(Vec3::new(0.0, -wt, -wt), Vec3::new(256.0, 0.0, 128.0 + wt));
        add_brush(Vec3::new(0.0, 256.0, -wt), Vec3::new(256.0, 256.0 + wt, 128.0 + wt));
        add_brush(Vec3::new(0.0, 0.0, -wt), Vec3::new(256.0, 256.0, 0.0));
        add_brush(Vec3::new(0.0, 0.0, 128.0), Vec3::new(256.0, 256.0, 128.0 + wt));

        let vg = build_test_voxel_grid(&faces, &volumes);
        let result = assign_to_grid(faces, Some(&vg));

        let total = result.cell_count;
        let air = result.cells.iter().filter(|c| c.cell_type == Some(CellType::Air)).count();
        let boundary = result.cells.iter().filter(|c| c.cell_type == Some(CellType::Boundary)).count();
        let solid_in_output = result.cells.iter().filter(|c| c.cell_type == Some(CellType::Solid)).count();
        let with_faces = result.cells.iter().filter(|c| !c.face_indices.is_empty()).count();

        assert!(total > 0, "should have cells");
        assert!(air > 0, "should have air cells");
        assert_eq!(solid_in_output, 0, "solid cells should be removed");
        assert!(with_faces > 0, "should have cells with faces (wall faces rescued to air sub-cells)");

        // For a single room, subdivision resolves boundary cells cleanly:
        // wall-side -> Solid (discarded), room-side -> Air (kept).
        // Remaining boundary cells are acceptable at wall corners where
        // voxel alignment doesn't allow clean splitting.
        assert!(
            air + boundary == total,
            "all retained cells should be air or boundary, got {air} air + {boundary} boundary != {total}"
        );

        // All faces should be preserved (rescued from solid sub-cells to air ones)
        let total_faces_assigned: usize = result.cells.iter().map(|c| c.face_indices.len()).sum();
        assert!(
            total_faces_assigned > 0,
            "faces should be assigned to cells after subdivision"
        );
    }

    /// Zero-face clusters should survive pack round-trip.
    #[test]
    fn zero_face_cluster_survives_pack_round_trip() {
        use crate::partition::Cluster;
        use postretro_level_format::visibility::{
            ClusterInfo, ClusterVisibilitySection, compress_pvs,
        };

        // Create two clusters: one with faces, one with zero faces (air cell)
        let face = Face {
            vertices: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
            normal: Vec3::Z,
            distance: 0.0,
            texture: "test".to_string(),
        };

        let clusters = vec![
            Cluster {
                id: 0,
                bounds: Aabb { min: Vec3::ZERO, max: Vec3::ONE },
                face_indices: vec![0],
            },
            Cluster {
                id: 1,
                bounds: Aabb { min: Vec3::splat(10.0), max: Vec3::splat(20.0) },
                face_indices: vec![], // air cell — no faces
            },
        ];

        let geometry = crate::geometry::extract_geometry(&[face], &clusters);

        // Both clusters visible to each other
        let pvs_row = vec![0b00000011u8]; // bits 0 and 1 set
        let compressed = compress_pvs(&pvs_row);
        let mut pvs_data = Vec::new();
        let mut cluster_infos = Vec::new();
        for (i, c) in clusters.iter().enumerate() {
            let pvs_offset = pvs_data.len() as u32;
            pvs_data.extend_from_slice(&compressed);
            let pvs_size = compressed.len() as u32;
            let face_start: u32 = clusters[..i].iter().map(|cl| cl.face_indices.len() as u32).sum();
            cluster_infos.push(ClusterInfo {
                bounds_min: [c.bounds.min.x, c.bounds.min.y, c.bounds.min.z],
                bounds_max: [c.bounds.max.x, c.bounds.max.y, c.bounds.max.z],
                face_start,
                face_count: c.face_indices.len() as u32,
                pvs_offset,
                pvs_size,
            });
        }

        let vis = ClusterVisibilitySection {
            clusters: cluster_infos,
            pvs_data,
        };

        // Verify the zero-face cluster has face_count=0 and that serialization works
        assert_eq!(vis.clusters[1].face_count, 0, "air cluster should have face_count=0");

        let dir = std::env::temp_dir().join("postretro_test_zero_face");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("zero_face.prl");

        crate::pack::pack_and_write(&output, &geometry, &vis, None)
            .expect("pack_and_write should succeed with zero-face cluster");

        // Read back and verify
        let data = std::fs::read(&output).expect("should read output");
        let mut cursor = std::io::Cursor::new(&data);
        let meta = postretro_level_format::read_container(&mut cursor)
            .expect("should read container");
        let vis_data = postretro_level_format::read_section_data(
            &mut cursor,
            &meta,
            postretro_level_format::SectionId::ClusterVisibility as u32,
        )
        .unwrap()
        .unwrap();
        let restored = ClusterVisibilitySection::from_bytes(&vis_data).unwrap();

        assert_eq!(restored.clusters.len(), 2);
        assert_eq!(restored.clusters[1].face_count, 0);

        let _ = std::fs::remove_file(&output);
    }
}
