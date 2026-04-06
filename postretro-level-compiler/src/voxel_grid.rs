// 3D voxel bitmap for solid/empty classification and ray marching.
// See: context/plans/ready/voxel-pvs-rework/plan.md

use std::collections::VecDeque;

use glam::Vec3;

use crate::map_data::BrushVolume;
use crate::partition::Aabb;

/// Maximum voxels per axis. Grids exceeding this auto-coarsen.
const MAX_VOXELS_PER_AXIS: usize = 1024;

/// Default voxel size in world units.
pub const DEFAULT_VOXEL_SIZE: f32 = 4.0;

/// 3D bitmap marking each voxel as solid (inside a brush) or empty (air).
pub struct VoxelGrid {
    /// World-space bounding box of the grid.
    pub bounds: Aabb,
    /// Resolution per axis [x, y, z].
    pub resolution: [usize; 3],
    /// Flat bitset: bit at index (x + y*res_x + z*res_x*res_y) is 1 if solid.
    bits: Vec<u8>,
    /// Size of one voxel in world units.
    pub voxel_size: f32,
}

impl VoxelGrid {
    /// Build a voxel grid from brush volumes within the given world bounds.
    ///
    /// Each voxel's center is tested against all brush half-planes. A voxel is
    /// solid if its center is inside any brush. Brushes are pre-filtered by AABB
    /// overlap to avoid testing every brush against every voxel.
    ///
    /// If the grid would exceed MAX_VOXELS_PER_AXIS on any axis at the requested
    /// voxel_size, the size is increased until the grid fits.
    pub fn from_brushes(
        brush_volumes: &[BrushVolume],
        world_bounds: &Aabb,
        mut voxel_size: f32,
    ) -> Self {
        let extent = world_bounds.max - world_bounds.min;

        // Auto-coarsen if grid would be too large
        let needed = |size: f32| {
            [
                (extent.x / size).ceil() as usize,
                (extent.y / size).ceil() as usize,
                (extent.z / size).ceil() as usize,
            ]
        };

        let original_size = voxel_size;
        loop {
            let res = needed(voxel_size);
            if res[0] <= MAX_VOXELS_PER_AXIS
                && res[1] <= MAX_VOXELS_PER_AXIS
                && res[2] <= MAX_VOXELS_PER_AXIS
            {
                break;
            }
            voxel_size *= 2.0;
        }

        if voxel_size != original_size {
            log::warn!(
                "[VoxelGrid] Auto-coarsened voxel size from {:.1} to {:.1} to fit within {} max per axis",
                original_size,
                voxel_size,
                MAX_VOXELS_PER_AXIS,
            );
        }

        let resolution = needed(voxel_size);
        // Ensure at least 1 voxel per axis
        let resolution = [
            resolution[0].max(1),
            resolution[1].max(1),
            resolution[2].max(1),
        ];

        let total_voxels = resolution[0] * resolution[1] * resolution[2];
        let byte_count = total_voxels.div_ceil(8);
        let mut bits = vec![0u8; byte_count];

        // Pre-compute brush AABBs for overlap filtering
        let brush_aabbs: Vec<Option<Aabb>> =
            brush_volumes.iter().map(|b| brush_aabb(b)).collect();

        for z in 0..resolution[2] {
            for y in 0..resolution[1] {
                for x in 0..resolution[0] {
                    let center = voxel_center(world_bounds, &resolution, voxel_size, x, y, z);

                    let inside_any = brush_volumes.iter().enumerate().any(|(bi, brush)| {
                        // AABB pre-filter: skip brushes whose AABB doesn't contain this point
                        if let Some(ref aabb) = brush_aabbs[bi] {
                            if center.x < aabb.min.x
                                || center.x > aabb.max.x
                                || center.y < aabb.min.y
                                || center.y > aabb.max.y
                                || center.z < aabb.min.z
                                || center.z > aabb.max.z
                            {
                                return false;
                            }
                        }
                        point_inside_brush(center, brush)
                    });

                    if inside_any {
                        let idx = x + y * resolution[0] + z * resolution[0] * resolution[1];
                        bits[idx / 8] |= 1 << (idx % 8);
                    }
                }
            }
        }

        let solid_count = count_set_bits(&bits, total_voxels);
        let memory_kb = byte_count / 1024;
        log::info!(
            "[VoxelGrid] {}x{}x{} grid ({} voxels, {:.1} unit size), {} solid ({:.1}%), {} KB",
            resolution[0],
            resolution[1],
            resolution[2],
            total_voxels,
            voxel_size,
            solid_count,
            if total_voxels > 0 {
                solid_count as f64 / total_voxels as f64 * 100.0
            } else {
                0.0
            },
            memory_kb,
        );

        Self {
            bounds: world_bounds.clone(),
            resolution,
            bits,
            voxel_size,
        }
    }

    /// Query whether a specific voxel cell is solid.
    pub fn is_solid(&self, x: usize, y: usize, z: usize) -> bool {
        if x >= self.resolution[0] || y >= self.resolution[1] || z >= self.resolution[2] {
            return false;
        }
        let idx = x + y * self.resolution[0] + z * self.resolution[0] * self.resolution[1];
        (self.bits[idx / 8] & (1 << (idx % 8))) != 0
    }

    /// Flood-fill from a known interior point, then mark all unreached empty
    /// voxels as solid. This seals exterior void so rays cannot travel around
    /// the outside of the map.
    ///
    /// `interior_seed` is a world-space point guaranteed to be in playable air
    /// (typically `info_player_start` origin). If it falls inside a solid voxel,
    /// the grid is left unchanged and a warning is logged.
    pub fn seal_exterior(&mut self, interior_seed: Vec3) {
        let offset = interior_seed - self.bounds.min;
        let sx = (offset.x / self.voxel_size).floor() as isize;
        let sy = (offset.y / self.voxel_size).floor() as isize;
        let sz = (offset.z / self.voxel_size).floor() as isize;

        if sx < 0
            || sy < 0
            || sz < 0
            || sx >= self.resolution[0] as isize
            || sy >= self.resolution[1] as isize
            || sz >= self.resolution[2] as isize
        {
            log::warn!(
                "[VoxelGrid] Seal seed point {:?} is outside grid bounds, skipping exterior seal",
                interior_seed,
            );
            return;
        }

        let (sx, sy, sz) = (sx as usize, sy as usize, sz as usize);

        if self.is_solid(sx, sy, sz) {
            log::warn!(
                "[VoxelGrid] Seal seed point {:?} (voxel [{}, {}, {}]) is inside solid, \
                 skipping exterior seal",
                interior_seed,
                sx,
                sy,
                sz,
            );
            return;
        }

        let [rx, ry, rz] = self.resolution;
        let total_voxels = rx * ry * rz;
        let visited_bytes = total_voxels.div_ceil(8);
        let mut visited = vec![0u8; visited_bytes];

        // BFS flood fill from the seed through 6-connected empty neighbors
        let mut queue = VecDeque::new();
        let seed_idx = sx + sy * rx + sz * rx * ry;
        visited[seed_idx / 8] |= 1 << (seed_idx % 8);
        queue.push_back((sx, sy, sz));

        while let Some((x, y, z)) = queue.pop_front() {
            let neighbors: [(isize, isize, isize); 6] = [
                (1, 0, 0),
                (-1, 0, 0),
                (0, 1, 0),
                (0, -1, 0),
                (0, 0, 1),
                (0, 0, -1),
            ];

            for (dx, dy, dz) in neighbors {
                let nx = x as isize + dx;
                let ny = y as isize + dy;
                let nz = z as isize + dz;

                if nx < 0
                    || ny < 0
                    || nz < 0
                    || nx >= rx as isize
                    || ny >= ry as isize
                    || nz >= rz as isize
                {
                    continue;
                }

                let (nx, ny, nz) = (nx as usize, ny as usize, nz as usize);
                let idx = nx + ny * rx + nz * rx * ry;

                // Skip already-visited
                if (visited[idx / 8] & (1 << (idx % 8))) != 0 {
                    continue;
                }

                // Skip solid voxels
                if (self.bits[idx / 8] & (1 << (idx % 8))) != 0 {
                    continue;
                }

                visited[idx / 8] |= 1 << (idx % 8);
                queue.push_back((nx, ny, nz));
            }
        }

        // Mark unreached empty voxels as solid
        let empty_before = total_voxels - count_set_bits(&self.bits, total_voxels);
        let mut sealed_count = 0usize;

        for i in 0..total_voxels {
            let byte = i / 8;
            let bit = 1u8 << (i % 8);
            let is_solid = (self.bits[byte] & bit) != 0;
            let is_visited = (visited[byte] & bit) != 0;

            if !is_solid && !is_visited {
                self.bits[byte] |= bit;
                sealed_count += 1;
            }
        }

        let pct = if empty_before > 0 {
            sealed_count as f64 / empty_before as f64 * 100.0
        } else {
            0.0
        };

        log::info!(
            "[VoxelGrid] Sealed {} exterior void voxels ({:.1}% of {} originally empty)",
            sealed_count,
            pct,
            empty_before,
        );
    }

    /// Query whether a world-space point is in solid space.
    ///
    /// Converts the position to grid coordinates and checks the corresponding
    /// voxel. Out-of-bounds positions return false (treated as air).
    pub fn is_point_solid(&self, world_pos: Vec3) -> bool {
        let offset = world_pos - self.bounds.min;
        let gx = (offset.x / self.voxel_size).floor() as isize;
        let gy = (offset.y / self.voxel_size).floor() as isize;
        let gz = (offset.z / self.voxel_size).floor() as isize;

        if gx < 0 || gy < 0 || gz < 0 {
            return false;
        }

        self.is_solid(gx as usize, gy as usize, gz as usize)
    }


}

/// World-space center of a voxel cell.
fn voxel_center(
    bounds: &Aabb,
    _resolution: &[usize; 3],
    voxel_size: f32,
    x: usize,
    y: usize,
    z: usize,
) -> Vec3 {
    Vec3::new(
        bounds.min.x + (x as f32 + 0.5) * voxel_size,
        bounds.min.y + (y as f32 + 0.5) * voxel_size,
        bounds.min.z + (z as f32 + 0.5) * voxel_size,
    )
}

/// Test whether a point is inside a convex brush volume.
///
/// A point is inside when `dot(point, normal) - distance <= 0` for all planes.
fn point_inside_brush(point: Vec3, brush: &BrushVolume) -> bool {
    brush
        .planes
        .iter()
        .all(|plane| point.dot(plane.normal) - plane.distance <= 0.0)
}

/// Compute the AABB of a brush volume from its axis-aligned half-planes.
fn brush_aabb(brush: &BrushVolume) -> Option<Aabb> {
    let mut min = Vec3::splat(f32::NEG_INFINITY);
    let mut max = Vec3::splat(f32::INFINITY);

    for plane in &brush.planes {
        let n = plane.normal;
        let d = plane.distance;
        if n.x.abs() > 0.99 {
            if n.x > 0.0 {
                max.x = max.x.min(d);
            } else {
                min.x = min.x.max(-d);
            }
        }
        if n.y.abs() > 0.99 {
            if n.y > 0.0 {
                max.y = max.y.min(d);
            } else {
                min.y = min.y.max(-d);
            }
        }
        if n.z.abs() > 0.99 {
            if n.z > 0.0 {
                max.z = max.z.min(d);
            } else {
                min.z = min.z.max(-d);
            }
        }
    }

    if min.x.is_finite()
        && max.x.is_finite()
        && min.y.is_finite()
        && max.y.is_finite()
        && min.z.is_finite()
        && max.z.is_finite()
        && min.x <= max.x
        && min.y <= max.y
        && min.z <= max.z
    {
        Some(Aabb { min, max })
    } else {
        None
    }
}

/// Count set bits in a bitset up to `total` bits.
fn count_set_bits(bits: &[u8], total: usize) -> usize {
    let full_bytes = total / 8;
    let remainder = total % 8;
    let mut count: usize = bits[..full_bytes]
        .iter()
        .map(|b| b.count_ones() as usize)
        .sum();
    if remainder > 0 && full_bytes < bits.len() {
        // Only count bits within the valid range in the last byte
        let mask = (1u8 << remainder) - 1;
        count += (bits[full_bytes] & mask).count_ones() as usize;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_data::{BrushPlane, BrushVolume};

    fn box_brush(min: Vec3, max: Vec3) -> BrushVolume {
        BrushVolume {
            planes: vec![
                BrushPlane {
                    normal: Vec3::X,
                    distance: max.x,
                },
                BrushPlane {
                    normal: Vec3::NEG_X,
                    distance: -min.x,
                },
                BrushPlane {
                    normal: Vec3::Y,
                    distance: max.y,
                },
                BrushPlane {
                    normal: Vec3::NEG_Y,
                    distance: -min.y,
                },
                BrushPlane {
                    normal: Vec3::Z,
                    distance: max.z,
                },
                BrushPlane {
                    normal: Vec3::NEG_Z,
                    distance: -min.z,
                },
            ],
        }
    }

    #[test]
    fn empty_brushes_produce_all_empty_grid() {
        let bounds = Aabb {
            min: Vec3::ZERO,
            max: Vec3::splat(32.0),
        };
        let grid = VoxelGrid::from_brushes(&[], &bounds, 4.0);
        for z in 0..grid.resolution[2] {
            for y in 0..grid.resolution[1] {
                for x in 0..grid.resolution[0] {
                    assert!(!grid.is_solid(x, y, z), "voxel ({x},{y},{z}) should be empty");
                }
            }
        }
    }

    #[test]
    fn single_box_brush_solid_inside_empty_outside() {
        let brush = box_brush(Vec3::new(8.0, 8.0, 8.0), Vec3::new(24.0, 24.0, 24.0));
        let bounds = Aabb {
            min: Vec3::ZERO,
            max: Vec3::splat(32.0),
        };
        let grid = VoxelGrid::from_brushes(&[brush], &bounds, 4.0);

        // Center of the brush should be solid
        // Voxel (3,3,3) has center at (14, 14, 14) — inside the 8..24 brush
        assert!(grid.is_solid(3, 3, 3), "center voxel should be solid");

        // Corner voxel (0,0,0) has center at (2, 2, 2) — outside the brush
        assert!(!grid.is_solid(0, 0, 0), "corner voxel should be empty");

        // Far corner voxel (7,7,7) has center at (30, 30, 30) — outside the brush
        assert!(!grid.is_solid(7, 7, 7), "far corner voxel should be empty");
    }

    #[test]
    fn is_point_solid_returns_true_for_brush_center_false_for_air() {
        let brush = box_brush(Vec3::new(8.0, 8.0, 8.0), Vec3::new(24.0, 24.0, 24.0));
        let bounds = Aabb {
            min: Vec3::ZERO,
            max: Vec3::splat(32.0),
        };
        let grid = VoxelGrid::from_brushes(&[brush], &bounds, 4.0);

        // Brush center (16, 16, 16) should be solid
        assert!(
            grid.is_point_solid(Vec3::splat(16.0)),
            "brush center should be solid"
        );

        // Known air point (2, 2, 2) should be empty
        assert!(
            !grid.is_point_solid(Vec3::splat(2.0)),
            "air point should be empty"
        );
    }

    #[test]
    fn two_overlapping_brushes_overlap_is_solid() {
        // Brush A: 0..20, Brush B: 10..30. Overlap: 10..20
        let brush_a = box_brush(Vec3::ZERO, Vec3::splat(20.0));
        let brush_b = box_brush(Vec3::splat(10.0), Vec3::splat(30.0));
        let bounds = Aabb {
            min: Vec3::ZERO,
            max: Vec3::splat(32.0),
        };
        let grid = VoxelGrid::from_brushes(&[brush_a, brush_b], &bounds, 4.0);

        // Overlap center (15, 15, 15) — voxel (3,3,3) center at (14, 14, 14)
        assert!(
            grid.is_point_solid(Vec3::splat(15.0)),
            "overlap region should be solid"
        );

        // Point only in brush_a (5, 5, 5)
        assert!(
            grid.is_point_solid(Vec3::splat(5.0)),
            "brush_a interior should be solid"
        );

        // Point only in brush_b (25, 25, 25)
        assert!(
            grid.is_point_solid(Vec3::splat(25.0)),
            "brush_b interior should be solid"
        );
    }

    #[test]
    fn auto_coarsen_produces_valid_grid_for_large_world() {
        let brush = box_brush(Vec3::new(2000.0, 2000.0, 2000.0), Vec3::new(2100.0, 2100.0, 2100.0));
        let bounds = Aabb {
            min: Vec3::ZERO,
            max: Vec3::splat(8192.0),
        };
        // 8192 / 4.0 = 2048 voxels per axis, which exceeds MAX_VOXELS_PER_AXIS (1024).
        // The grid must auto-coarsen to fit.
        let grid = VoxelGrid::from_brushes(&[brush], &bounds, 4.0);

        for axis in 0..3 {
            assert!(
                grid.resolution[axis] <= MAX_VOXELS_PER_AXIS,
                "axis {axis} resolution {} exceeds MAX_VOXELS_PER_AXIS ({MAX_VOXELS_PER_AXIS})",
                grid.resolution[axis]
            );
        }

        assert!(
            grid.voxel_size > 4.0,
            "voxel size should be coarser than the requested 4.0, got {}",
            grid.voxel_size
        );

        assert!(
            grid.is_point_solid(Vec3::splat(2050.0)),
            "brush center should still be classified as solid after coarsening"
        );

        assert!(
            !grid.is_point_solid(Vec3::splat(500.0)),
            "known air point should still be classified as empty after coarsening"
        );
    }

    #[test]
    fn out_of_bounds_query_returns_false() {
        let brush = box_brush(Vec3::ZERO, Vec3::splat(16.0));
        let bounds = Aabb {
            min: Vec3::ZERO,
            max: Vec3::splat(16.0),
        };
        let grid = VoxelGrid::from_brushes(&[brush], &bounds, 4.0);

        // Negative coordinates
        assert!(
            !grid.is_point_solid(Vec3::splat(-10.0)),
            "negative position should return false"
        );

        // Far beyond grid
        assert!(
            !grid.is_point_solid(Vec3::splat(100.0)),
            "far out-of-bounds position should return false"
        );

        // Grid index out of range
        assert!(!grid.is_solid(999, 999, 999), "huge index should return false");
    }

    /// Build 6 wall brushes forming a hollow room with `wall_thickness` walls.
    /// Room interior spans from `wall_thickness` to `room_size - wall_thickness`
    /// on each axis.
    fn hollow_room_brushes(room_size: f32, wall_thickness: f32) -> Vec<BrushVolume> {
        let t = wall_thickness;
        let s = room_size;
        vec![
            // Floor (Y-)
            box_brush(Vec3::ZERO, Vec3::new(s, t, s)),
            // Ceiling (Y+)
            box_brush(Vec3::new(0.0, s - t, 0.0), Vec3::new(s, s, s)),
            // Wall X-
            box_brush(Vec3::ZERO, Vec3::new(t, s, s)),
            // Wall X+
            box_brush(Vec3::new(s - t, 0.0, 0.0), Vec3::new(s, s, s)),
            // Wall Z-
            box_brush(Vec3::ZERO, Vec3::new(s, s, t)),
            // Wall Z+
            box_brush(Vec3::new(0.0, 0.0, s - t), Vec3::new(s, s, s)),
        ]
    }

    #[test]
    fn seal_exterior_marks_void_as_solid() {
        let room_size = 64.0;
        let wall_thickness = 8.0;
        let brushes = hollow_room_brushes(room_size, wall_thickness);
        // Grid extends beyond the room to create exterior void
        let bounds = Aabb {
            min: Vec3::splat(-32.0),
            max: Vec3::splat(96.0),
        };
        let mut grid = VoxelGrid::from_brushes(&brushes, &bounds, 4.0);

        // Seed inside the room (center)
        let seed = Vec3::splat(32.0);
        assert!(!grid.is_point_solid(seed), "seed should be in air before seal");

        grid.seal_exterior(seed);

        // Interior air remains empty
        assert!(
            !grid.is_point_solid(seed),
            "room center should remain empty after seal"
        );

        // Exterior void (outside the room walls, inside the grid) is now solid
        let exterior = Vec3::splat(-16.0);
        assert!(
            grid.is_point_solid(exterior),
            "exterior void should be solid after seal"
        );

        // Wall brushes remain solid
        assert!(
            grid.is_point_solid(Vec3::new(2.0, 32.0, 32.0)),
            "wall brush should remain solid"
        );
    }

    #[test]
    fn seal_exterior_skips_when_seed_in_solid() {
        let brush = box_brush(Vec3::ZERO, Vec3::splat(32.0));
        let bounds = Aabb {
            min: Vec3::ZERO,
            max: Vec3::splat(32.0),
        };
        let mut grid = VoxelGrid::from_brushes(&[brush], &bounds, 4.0);

        // Snapshot the grid state before sealing
        let bits_before = grid.bits.clone();

        // Seed inside the solid brush
        grid.seal_exterior(Vec3::splat(16.0));

        // Grid should be unchanged
        assert_eq!(grid.bits, bits_before, "grid should be unchanged when seed is in solid");
    }

    #[test]
    fn seal_exterior_preserves_interior_air() {
        let room_size = 64.0;
        let wall_thickness = 8.0;
        let brushes = hollow_room_brushes(room_size, wall_thickness);
        let bounds = Aabb {
            min: Vec3::splat(-32.0),
            max: Vec3::splat(96.0),
        };
        let mut grid = VoxelGrid::from_brushes(&brushes, &bounds, 4.0);

        // Collect interior air voxels before sealing
        let interior_min = (wall_thickness + 2.0) as usize; // safely inside walls
        let interior_max = (room_size - wall_thickness - 2.0) as usize;
        let mut interior_points = Vec::new();
        // Sample several points well inside the room
        for coord in (interior_min..interior_max).step_by(8) {
            let p = Vec3::new(coord as f32, coord as f32, coord as f32);
            if !grid.is_point_solid(p) {
                interior_points.push(p);
            }
        }
        assert!(
            !interior_points.is_empty(),
            "should have some interior air points to test"
        );

        grid.seal_exterior(Vec3::splat(32.0));

        // All previously-empty interior points should remain empty
        for p in &interior_points {
            assert!(
                !grid.is_point_solid(*p),
                "interior air at {:?} should remain empty after seal",
                p,
            );
        }
    }
}
