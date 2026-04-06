// Ray-cast PVS: cluster visibility via voxel-based 3D-DDA ray marching.
// See: context/plans/ready/voxel-pvs-rework/plan.md

use glam::Vec3;

use crate::partition::Aabb;
use crate::voxel_grid::VoxelGrid;

/// Compute the number of bytes needed for a cluster bitset.
pub fn bytes_for_clusters(cluster_count: usize) -> usize {
    cluster_count.div_ceil(8)
}

/// Test whether bit `idx` is set in a bitset byte array.
pub fn bit_is_set(bitset: &[u8], idx: usize) -> bool {
    let byte_idx = idx / 8;
    let bit_idx = idx % 8;
    byte_idx < bitset.len() && (bitset[byte_idx] & (1 << bit_idx)) != 0
}

/// Set bit `idx` in a bitset byte array.
pub fn set_bit(bitset: &mut [u8], idx: usize) {
    let byte_idx = idx / 8;
    let bit_idx = idx % 8;
    if byte_idx < bitset.len() {
        bitset[byte_idx] |= 1 << bit_idx;
    }
}

/// Test whether a ray segment is blocked by solid voxels using Amanatides & Woo 3D-DDA.
///
/// Steps through voxels along the ray from `start` to `end`. Returns true if
/// any solid voxel is encountered. If `start` is in a solid voxel, returns true
/// immediately. Rays outside the grid bounds are treated as unblocked.
pub fn ray_blocked_by_voxels(grid: &VoxelGrid, start: Vec3, end: Vec3) -> bool {
    // If start is in solid, the ray is blocked immediately
    if grid.is_point_solid(start) {
        return true;
    }

    let dir = end - start;
    let length = dir.length();
    if length < 1e-10 {
        return false;
    }

    // Convert start/end to grid-space (floating point)
    let gs = (start - grid.bounds.min) / grid.voxel_size;
    let ge = (end - grid.bounds.min) / grid.voxel_size;

    // Clip ray to grid bounds [0, resolution] to avoid walking outside
    let res = [
        grid.resolution[0] as f32,
        grid.resolution[1] as f32,
        grid.resolution[2] as f32,
    ];

    // Find t range where ray is inside the grid box [0, res]
    let gdir = ge - gs;
    let (mut t_enter, mut t_exit) = (0.0f32, 1.0f32);

    for axis in 0..3 {
        let origin = [gs.x, gs.y, gs.z][axis];
        let direction = [gdir.x, gdir.y, gdir.z][axis];
        let bound = res[axis];

        if direction.abs() < 1e-10 {
            // Parallel to this axis — if outside, no intersection
            if origin < 0.0 || origin >= bound {
                return false;
            }
            continue;
        }

        let t0 = (0.0 - origin) / direction;
        let t1 = (bound - origin) / direction;
        let (t_near, t_far) = if t0 < t1 { (t0, t1) } else { (t1, t0) };

        t_enter = t_enter.max(t_near);
        t_exit = t_exit.min(t_far);

        if t_enter > t_exit {
            return false;
        }
    }

    // Clamp t_enter to [0, 1] and start DDA from the entry point
    t_enter = t_enter.max(0.0);
    if t_enter >= t_exit {
        return false;
    }

    let entry = gs + gdir * t_enter;

    // Current voxel integer coordinates
    let mut vx = (entry.x.floor() as isize).clamp(0, grid.resolution[0] as isize - 1);
    let mut vy = (entry.y.floor() as isize).clamp(0, grid.resolution[1] as isize - 1);
    let mut vz = (entry.z.floor() as isize).clamp(0, grid.resolution[2] as isize - 1);

    // Step direction (+1 or -1) and t_delta / t_max per axis
    let step_x: isize = if gdir.x > 0.0 { 1 } else { -1 };
    let step_y: isize = if gdir.y > 0.0 { 1 } else { -1 };
    let step_z: isize = if gdir.z > 0.0 { 1 } else { -1 };

    // t_delta: how much t to cross one full voxel on each axis
    let t_delta_x = if gdir.x.abs() > 1e-10 {
        (1.0 / gdir.x).abs()
    } else {
        f32::INFINITY
    };
    let t_delta_y = if gdir.y.abs() > 1e-10 {
        (1.0 / gdir.y).abs()
    } else {
        f32::INFINITY
    };
    let t_delta_z = if gdir.z.abs() > 1e-10 {
        (1.0 / gdir.z).abs()
    } else {
        f32::INFINITY
    };

    // t_max: the t value at which the ray crosses the next voxel boundary
    let next_boundary = |pos: f32, step: isize| -> f32 {
        if step > 0 {
            pos.floor() + 1.0
        } else {
            pos.ceil() - 1.0
        }
    };

    // Use a small epsilon to handle rays starting exactly on boundaries
    let eps = 1e-5;
    let mut t_max_x = if gdir.x.abs() > 1e-10 {
        ((next_boundary(entry.x + eps * step_x as f32, step_x) - entry.x) / gdir.x).max(0.0)
    } else {
        f32::INFINITY
    };
    let mut t_max_y = if gdir.y.abs() > 1e-10 {
        ((next_boundary(entry.y + eps * step_y as f32, step_y) - entry.y) / gdir.y).max(0.0)
    } else {
        f32::INFINITY
    };
    let mut t_max_z = if gdir.z.abs() > 1e-10 {
        ((next_boundary(entry.z + eps * step_z as f32, step_z) - entry.z) / gdir.z).max(0.0)
    } else {
        f32::INFINITY
    };

    // Walk through voxels until we exit the ray segment or the grid.
    // The ray segment ends at t=1.0 (normalized). When the smallest
    // t_max exceeds 1.0, the next step would be past the endpoint.
    let max_steps = grid.resolution[0] + grid.resolution[1] + grid.resolution[2] + 3;

    for _ in 0..max_steps {
        // Check current voxel
        if grid.is_solid(vx as usize, vy as usize, vz as usize) {
            return true;
        }

        // Find which axis crosses the next boundary soonest
        let t_min = t_max_x.min(t_max_y).min(t_max_z);

        // If the next boundary crossing is past the ray endpoint, we're done
        if t_min > 1.0 {
            return false;
        }

        // Advance to next voxel boundary
        if t_max_x < t_max_y {
            if t_max_x < t_max_z {
                vx += step_x;
                t_max_x += t_delta_x;
            } else {
                vz += step_z;
                t_max_z += t_delta_z;
            }
        } else if t_max_y < t_max_z {
            vy += step_y;
            t_max_y += t_delta_y;
        } else {
            vz += step_z;
            t_max_z += t_delta_z;
        }

        // Out of bounds check
        if vx < 0
            || vy < 0
            || vz < 0
            || vx >= grid.resolution[0] as isize
            || vy >= grid.resolution[1] as isize
            || vz >= grid.resolution[2] as isize
        {
            return false;
        }
    }

    false
}

/// Remove sample points that fall inside solid voxels.
pub fn filter_solid_samples(grid: &VoxelGrid, samples: &[Vec3]) -> Vec<Vec3> {
    samples
        .iter()
        .copied()
        .filter(|&p| !grid.is_point_solid(p))
        .collect()
}

/// Simple deterministic RNG (xorshift64) for sample point generation.
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        // Avoid zero state which is a fixed point for xorshift
        Self { state: seed | 1 }
    }

    fn next_f32(&mut self) -> f32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        // Map to [0, 1)
        (self.state & 0x00FF_FFFF) as f32 / 16_777_216.0
    }
}

/// Generate deterministic random sample points within an AABB.
///
/// Uses a simple xorshift64 RNG seeded by `seed` for reproducibility.
pub fn generate_cluster_samples(bounds: &Aabb, count: usize, seed: u64) -> Vec<Vec3> {
    let mut rng = Rng::new(seed);
    let size = bounds.max - bounds.min;
    (0..count)
        .map(|_| {
            Vec3::new(
                bounds.min.x + rng.next_f32() * size.x,
                bounds.min.y + rng.next_f32() * size.y,
                bounds.min.z + rng.next_f32() * size.z,
            )
        })
        .collect()
}

/// Compute the centroid of a face from its vertices.
pub fn face_centroid(vertices: &[Vec3]) -> Vec3 {
    if vertices.is_empty() {
        return Vec3::ZERO;
    }
    let sum: Vec3 = vertices.iter().copied().sum();
    sum / vertices.len() as f32
}

/// Select up to `max_count` evenly-spaced centroids from a list.
///
/// When a cluster has more face centroids than the cap, picks evenly-spaced
/// indices to maintain spatial coverage across the cluster's faces.
fn select_evenly_spaced(centroids: &[Vec3], max_count: usize) -> Vec<Vec3> {
    if centroids.len() <= max_count {
        return centroids.to_vec();
    }
    let step = centroids.len() as f32 / max_count as f32;
    (0..max_count)
        .map(|i| centroids[(i as f32 * step) as usize])
        .collect()
}

/// Test whether two AABBs are adjacent (touching or overlapping on all three axes).
///
/// Uses a small epsilon to catch AABBs that share an exact boundary, since
/// floating-point cluster bounds from BSP splitting may not overlap by even
/// a fraction of a unit.
pub fn aabbs_adjacent(a: &Aabb, b: &Aabb) -> bool {
    const EPSILON: f32 = 1.0;
    a.min.x <= b.max.x + EPSILON
        && b.min.x <= a.max.x + EPSILON
        && a.min.y <= b.max.y + EPSILON
        && b.min.y <= a.max.y + EPSILON
        && a.min.z <= b.max.z + EPSILON
        && b.min.z <= a.max.z + EPSILON
}

/// Result of PVS ray-cast computation.
pub struct PvsResult {
    /// Per-cluster PVS bitsets (binary visible/not-visible).
    pub bitsets: Vec<Vec<u8>>,
    /// Per-cluster-pair confidence values (ratio of unblocked rays).
    /// Only populated when `compute_confidence` is true.
    /// `confidence[i][j]` is the fraction of rays from cluster i to j that were unblocked.
    /// Adjacent pairs and self-visibility get 1.0.
    pub confidence: Option<Vec<Vec<f32>>>,
}

/// Compute PVS bitsets for all clusters using voxel-based ray marching.
///
/// Adjacent clusters (AABBs that touch or overlap) are always marked mutually
/// visible, bypassing ray-casting. This prevents faces from disappearing when
/// the camera is near a cluster boundary where all sample rays happen to hit
/// brush geometry. Non-adjacent pairs use 3D-DDA ray marching through the
/// voxel grid.
///
/// Sample points combine random AABB samples with face centroids, then filter
/// out points in solid voxels. Face centroids target corridor openings where
/// random points in a large cell have poor odds of landing, improving
/// cross-room visibility through narrow passages.
///
/// `face_centroids_per_cluster` provides pre-computed face centroids for each
/// cluster. Pass an empty slice to use only random AABB samples.
///
/// `adjacency_only` marks clusters that should receive visibility only from
/// adjacency, not from ray-casting. These clusters get no sample points and
/// are skipped as both sources and targets. Air cells (no faces) use this to
/// avoid false visibility from rays that trace through multiple corridor
/// openings at diagonal angles.
///
/// When `compute_confidence` is true, counts all unblocked rays per pair instead
/// of early-outing on the first. This is significantly slower but produces
/// per-pair confidence ratios for diagnostics.
pub fn compute_pvs_raycast(
    cluster_count: usize,
    cluster_bounds: &[Aabb],
    voxel_grid: &VoxelGrid,
    face_centroids_per_cluster: &[Vec<Vec3>],
    adjacency_only: &[bool],
    compute_confidence: bool,
) -> PvsResult {
    let row_bytes = bytes_for_clusters(cluster_count);
    const RANDOM_SAMPLES: usize = 16;
    const MAX_CENTROID_SAMPLES: usize = 16;

    // Minimum fraction of unblocked rays required to mark a pair visible.
    // Filters narrow diagonal sightlines that thread through multiple
    // corridor openings — these produce very few unblocked rays (1-4%)
    // compared to legitimate sightlines through single openings (>10%).
    const MIN_VISIBILITY_RATIO: f32 = 0.03;

    // Start with identity: every cluster sees itself
    let mut pvs: Vec<Vec<u8>> = (0..cluster_count)
        .map(|i| {
            let mut row = vec![0u8; row_bytes];
            set_bit(&mut row, i);
            row
        })
        .collect();

    let mut confidence: Option<Vec<Vec<f32>>> = if compute_confidence {
        // Initialize with 0.0; self-visibility set to 1.0
        let mut matrix = vec![vec![0.0f32; cluster_count]; cluster_count];
        for i in 0..cluster_count {
            matrix[i][i] = 1.0;
        }
        Some(matrix)
    } else {
        None
    };

    if cluster_count <= 1 {
        return PvsResult {
            bitsets: pvs,
            confidence,
        };
    }

    // Pre-generate combined sample points for each cluster, filtering
    // out points in solid space. Face centroids are inset slightly toward
    // the cluster centroid to pull them off brush surfaces, then filtered.
    // Adjacency-only clusters get no samples (they rely on adjacency alone).
    let samples: Vec<Vec<Vec3>> = (0..cluster_count)
        .map(|i| {
            if i < adjacency_only.len() && adjacency_only[i] {
                return Vec::new();
            }
            let mut pts = filter_solid_samples(
                voxel_grid,
                &generate_cluster_samples(&cluster_bounds[i], RANDOM_SAMPLES, i as u64),
            );
            if i < face_centroids_per_cluster.len() {
                let centroids =
                    select_evenly_spaced(&face_centroids_per_cluster[i], MAX_CENTROID_SAMPLES);
                let cluster_center = (cluster_bounds[i].min + cluster_bounds[i].max) * 0.5;
                let inset: Vec<Vec3> = centroids
                    .iter()
                    .map(|&c| {
                        let to_center = cluster_center - c;
                        let dist = to_center.length();
                        if dist > 1e-6 {
                            // Pull centroid 4 units toward cluster center (one voxel size)
                            c + to_center.normalize() * dist.min(voxel_grid.voxel_size)
                        } else {
                            c
                        }
                    })
                    .collect();
                pts.extend(filter_solid_samples(voxel_grid, &inset));
            }
            pts
        })
        .collect();

    // Test each cluster pair
    for i in 0..cluster_count {
        for j in (i + 1)..cluster_count {
            // Adjacent clusters are always mutually visible — ray-casting
            // can produce false negatives when all sample rays hit geometry.
            if aabbs_adjacent(&cluster_bounds[i], &cluster_bounds[j]) {
                set_bit(&mut pvs[i], j);
                set_bit(&mut pvs[j], i);
                if let Some(ref mut conf) = confidence {
                    conf[i][j] = 1.0;
                    conf[j][i] = 1.0;
                }
                continue;
            }

            // Count unblocked rays and require a minimum fraction to mark
            // as visible. A single unblocked ray through a narrow diagonal
            // sightline (e.g., threading two corridor openings at extreme
            // angles) should not create visibility — the sightline must be
            // wide enough that multiple sample pairs confirm it.
            let total_rays = samples[i].len() * samples[j].len();
            let min_unblocked = if total_rays > 0 {
                (total_rays as f32 * MIN_VISIBILITY_RATIO).ceil() as u32
            } else {
                1
            };

            let mut unblocked_rays = 0u32;
            for src in &samples[i] {
                for dst in &samples[j] {
                    if !ray_blocked_by_voxels(voxel_grid, *src, *dst) {
                        unblocked_rays += 1;
                        // Early-out once we've confirmed enough rays
                        // (skip when computing confidence — need full count)
                        if !compute_confidence && unblocked_rays >= min_unblocked {
                            break;
                        }
                    }
                }
                if !compute_confidence && unblocked_rays >= min_unblocked {
                    break;
                }
            }

            if compute_confidence {
                let ratio = if total_rays > 0 {
                    unblocked_rays as f32 / total_rays as f32
                } else {
                    0.0
                };
                if let Some(ref mut conf) = confidence {
                    conf[i][j] = ratio;
                    conf[j][i] = ratio;
                }
            }

            if unblocked_rays >= min_unblocked {
                set_bit(&mut pvs[i], j);
                set_bit(&mut pvs[j], i);
            }
        }
    }

    PvsResult {
        bitsets: pvs,
        confidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_data::{BrushPlane, BrushVolume};
    use crate::partition::Aabb;

    /// Build an axis-aligned box brush volume from min/max corners.
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

    /// Build a VoxelGrid from brushes using a world bounds that covers all geometry.
    fn make_grid(brushes: &[BrushVolume], world_min: Vec3, world_max: Vec3) -> VoxelGrid {
        let bounds = Aabb {
            min: world_min,
            max: world_max,
        };
        VoxelGrid::from_brushes(brushes, &bounds, 4.0)
    }

    // -- ray_blocked_by_voxels tests (replaces ray_vs_brush) --

    #[test]
    fn dda_ray_through_empty_grid_returns_unblocked() {
        let grid = make_grid(&[], Vec3::ZERO, Vec3::splat(100.0));
        assert!(
            !ray_blocked_by_voxels(&grid, Vec3::ZERO, Vec3::new(100.0, 0.0, 0.0)),
            "ray through empty grid should be unblocked"
        );
    }

    #[test]
    fn dda_ray_through_solid_region_returns_blocked() {
        let brush = box_brush(Vec3::new(40.0, 40.0, 40.0), Vec3::new(60.0, 60.0, 60.0));
        let grid = make_grid(&[brush], Vec3::ZERO, Vec3::splat(100.0));
        // Ray along the diagonal passes through the solid brush
        assert!(
            ray_blocked_by_voxels(&grid, Vec3::splat(10.0), Vec3::splat(90.0)),
            "ray through solid region should be blocked"
        );
    }

    #[test]
    fn dda_ray_around_solid_region_returns_unblocked() {
        // Brush in the center, ray passes below it
        let brush = box_brush(Vec3::new(40.0, 40.0, 40.0), Vec3::new(60.0, 60.0, 60.0));
        let grid = make_grid(&[brush], Vec3::ZERO, Vec3::splat(100.0));
        // Ray at y=10, z=10 — well below the brush at y=40..60
        assert!(
            !ray_blocked_by_voxels(
                &grid,
                Vec3::new(0.0, 10.0, 10.0),
                Vec3::new(100.0, 10.0, 10.0)
            ),
            "ray around solid region should be unblocked"
        );
    }

    #[test]
    fn dda_ray_starting_in_solid_returns_blocked() {
        let brush = box_brush(Vec3::new(0.0, 0.0, 0.0), Vec3::new(50.0, 50.0, 50.0));
        let grid = make_grid(&[brush], Vec3::ZERO, Vec3::splat(100.0));
        // Start inside the brush
        assert!(
            ray_blocked_by_voxels(&grid, Vec3::splat(25.0), Vec3::splat(90.0)),
            "ray starting in solid voxel should be blocked"
        );
    }

    // -- filter_solid_samples tests --

    #[test]
    fn filter_solid_samples_keeps_air_removes_solid() {
        let brush = box_brush(Vec3::new(10.0, 10.0, 10.0), Vec3::new(30.0, 30.0, 30.0));
        let grid = make_grid(&[brush], Vec3::ZERO, Vec3::splat(40.0));

        let samples = vec![
            Vec3::splat(5.0),  // air
            Vec3::splat(20.0), // solid (inside brush)
            Vec3::splat(35.0), // air
        ];

        let filtered = filter_solid_samples(&grid, &samples);
        assert_eq!(filtered.len(), 2, "should keep 2 air points");
        assert!(
            (filtered[0] - Vec3::splat(5.0)).length() < 1e-6,
            "first air point preserved"
        );
        assert!(
            (filtered[1] - Vec3::splat(35.0)).length() < 1e-6,
            "second air point preserved"
        );
    }

    // -- generate_cluster_samples tests --

    #[test]
    fn samples_are_within_aabb_bounds() {
        let bounds = Aabb {
            min: Vec3::new(-10.0, -20.0, -30.0),
            max: Vec3::new(10.0, 20.0, 30.0),
        };
        let samples = generate_cluster_samples(&bounds, 100, 42);
        assert_eq!(samples.len(), 100);
        for s in &samples {
            assert!(
                s.x >= bounds.min.x && s.x <= bounds.max.x,
                "x={} out of bounds",
                s.x
            );
            assert!(
                s.y >= bounds.min.y && s.y <= bounds.max.y,
                "y={} out of bounds",
                s.y
            );
            assert!(
                s.z >= bounds.min.z && s.z <= bounds.max.z,
                "z={} out of bounds",
                s.z
            );
        }
    }

    #[test]
    fn same_seed_produces_same_samples() {
        let bounds = Aabb {
            min: Vec3::ZERO,
            max: Vec3::ONE * 100.0,
        };
        let a = generate_cluster_samples(&bounds, 16, 7);
        let b = generate_cluster_samples(&bounds, 16, 7);
        assert_eq!(a, b);
    }

    #[test]
    fn different_seeds_produce_different_samples() {
        let bounds = Aabb {
            min: Vec3::ZERO,
            max: Vec3::ONE * 100.0,
        };
        let a = generate_cluster_samples(&bounds, 16, 1);
        let b = generate_cluster_samples(&bounds, 16, 2);
        assert_ne!(a, b);
    }

    // -- compute_pvs_raycast tests (adapted for VoxelGrid) --

    #[test]
    fn single_cluster_sees_only_itself() {
        let bounds = vec![Aabb {
            min: Vec3::ZERO,
            max: Vec3::ONE * 10.0,
        }];
        let world = Aabb {
            min: Vec3::splat(-10.0),
            max: Vec3::splat(20.0),
        };
        let grid = VoxelGrid::from_brushes(&[], &world, 4.0);
        let result = compute_pvs_raycast(1, &bounds, &grid, &[], &[], false);
        assert_eq!(result.bitsets.len(), 1);
        assert!(bit_is_set(&result.bitsets[0], 0));
    }

    #[test]
    fn two_clusters_no_brushes_are_mutually_visible() {
        let bounds = vec![
            Aabb {
                min: Vec3::new(-100.0, -50.0, -50.0),
                max: Vec3::new(-10.0, 50.0, 50.0),
            },
            Aabb {
                min: Vec3::new(10.0, -50.0, -50.0),
                max: Vec3::new(100.0, 50.0, 50.0),
            },
        ];
        let world = Aabb {
            min: Vec3::splat(-110.0),
            max: Vec3::splat(110.0),
        };
        let grid = VoxelGrid::from_brushes(&[], &world, 4.0);
        let result = compute_pvs_raycast(2, &bounds, &grid, &[], &[], false);
        assert!(
            bit_is_set(&result.bitsets[0], 1),
            "cluster 0 should see cluster 1 with no brushes"
        );
        assert!(
            bit_is_set(&result.bitsets[1], 0),
            "cluster 1 should see cluster 0 with no brushes"
        );
    }

    #[test]
    fn two_clusters_with_wall_brush_not_visible() {
        let bounds = vec![
            Aabb {
                min: Vec3::new(-200.0, -100.0, -100.0),
                max: Vec3::new(-20.0, 100.0, 100.0),
            },
            Aabb {
                min: Vec3::new(20.0, -100.0, -100.0),
                max: Vec3::new(200.0, 100.0, 100.0),
            },
        ];
        let wall = box_brush(
            Vec3::new(-10.0, -200.0, -200.0),
            Vec3::new(10.0, 200.0, 200.0),
        );
        let world = Aabb {
            min: Vec3::splat(-210.0),
            max: Vec3::splat(210.0),
        };
        let grid = VoxelGrid::from_brushes(&[wall], &world, 4.0);
        let result = compute_pvs_raycast(2, &bounds, &grid, &[], &[], false);
        assert!(
            !bit_is_set(&result.bitsets[0], 1),
            "cluster 0 should NOT see cluster 1 through wall"
        );
        assert!(
            !bit_is_set(&result.bitsets[1], 0),
            "cluster 1 should NOT see cluster 0 through wall"
        );
    }

    #[test]
    fn pvs_is_symmetric() {
        let bounds = vec![
            Aabb {
                min: Vec3::new(-100.0, -50.0, -50.0),
                max: Vec3::new(-10.0, 50.0, 50.0),
            },
            Aabb {
                min: Vec3::new(10.0, -50.0, -50.0),
                max: Vec3::new(100.0, 50.0, 50.0),
            },
            Aabb {
                min: Vec3::new(-50.0, 100.0, -50.0),
                max: Vec3::new(50.0, 200.0, 50.0),
            },
        ];
        let world = Aabb {
            min: Vec3::splat(-110.0),
            max: Vec3::splat(210.0),
        };
        let grid = VoxelGrid::from_brushes(&[], &world, 4.0);
        let result = compute_pvs_raycast(3, &bounds, &grid, &[], &[], false);
        for i in 0..3 {
            for j in 0..3 {
                assert_eq!(
                    bit_is_set(&result.bitsets[i], j),
                    bit_is_set(&result.bitsets[j], i),
                    "PVS asymmetry: cluster {i} sees {j} != cluster {j} sees {i}"
                );
            }
        }
    }

    // -- bitset utility tests --

    #[test]
    fn bytes_for_clusters_correct() {
        assert_eq!(bytes_for_clusters(0), 0);
        assert_eq!(bytes_for_clusters(1), 1);
        assert_eq!(bytes_for_clusters(8), 1);
        assert_eq!(bytes_for_clusters(9), 2);
        assert_eq!(bytes_for_clusters(16), 2);
        assert_eq!(bytes_for_clusters(17), 3);
    }

    #[test]
    fn bit_operations_correct() {
        let mut bitset = vec![0u8; 4];
        set_bit(&mut bitset, 0);
        assert!(bit_is_set(&bitset, 0));
        assert!(!bit_is_set(&bitset, 1));

        set_bit(&mut bitset, 7);
        assert!(bit_is_set(&bitset, 7));

        set_bit(&mut bitset, 8);
        assert!(bit_is_set(&bitset, 8));
        assert!(!bit_is_set(&bitset, 9));

        set_bit(&mut bitset, 31);
        assert!(bit_is_set(&bitset, 31));
    }

    // -- aabbs_adjacent tests --

    #[test]
    fn aabbs_sharing_face_are_adjacent() {
        let a = Aabb {
            min: Vec3::new(0.0, 0.0, 0.0),
            max: Vec3::new(10.0, 10.0, 10.0),
        };
        let b = Aabb {
            min: Vec3::new(10.0, 0.0, 0.0),
            max: Vec3::new(20.0, 10.0, 10.0),
        };
        assert!(
            aabbs_adjacent(&a, &b),
            "AABBs sharing a face should be adjacent"
        );
        assert!(aabbs_adjacent(&b, &a), "adjacency should be symmetric");
    }

    #[test]
    fn aabbs_overlapping_are_adjacent() {
        let a = Aabb {
            min: Vec3::new(0.0, 0.0, 0.0),
            max: Vec3::new(15.0, 10.0, 10.0),
        };
        let b = Aabb {
            min: Vec3::new(5.0, 0.0, 0.0),
            max: Vec3::new(20.0, 10.0, 10.0),
        };
        assert!(
            aabbs_adjacent(&a, &b),
            "overlapping AABBs should be adjacent"
        );
    }

    #[test]
    fn aabbs_with_gap_are_not_adjacent() {
        let a = Aabb {
            min: Vec3::new(0.0, 0.0, 0.0),
            max: Vec3::new(10.0, 10.0, 10.0),
        };
        let b = Aabb {
            min: Vec3::new(20.0, 0.0, 0.0),
            max: Vec3::new(30.0, 10.0, 10.0),
        };
        assert!(
            !aabbs_adjacent(&a, &b),
            "AABBs with a gap should not be adjacent"
        );
    }

    #[test]
    fn aabbs_touching_at_edge_are_adjacent() {
        let a = Aabb {
            min: Vec3::new(0.0, 0.0, 0.0),
            max: Vec3::new(10.0, 10.0, 10.0),
        };
        let b = Aabb {
            min: Vec3::new(10.0, 10.0, 0.0),
            max: Vec3::new(20.0, 20.0, 10.0),
        };
        assert!(
            aabbs_adjacent(&a, &b),
            "AABBs touching at an edge should be adjacent"
        );
    }

    // -- PVS adjacency override tests --

    #[test]
    fn adjacent_clusters_visible_despite_wall_brush() {
        let bounds = vec![
            Aabb {
                min: Vec3::new(-100.0, -100.0, -100.0),
                max: Vec3::new(0.0, 100.0, 100.0),
            },
            Aabb {
                min: Vec3::new(0.0, -100.0, -100.0),
                max: Vec3::new(100.0, 100.0, 100.0),
            },
        ];
        let wall = box_brush(
            Vec3::new(-5.0, -200.0, -200.0),
            Vec3::new(5.0, 200.0, 200.0),
        );
        let world = Aabb {
            min: Vec3::splat(-210.0),
            max: Vec3::splat(210.0),
        };
        let grid = VoxelGrid::from_brushes(&[wall], &world, 4.0);
        let result = compute_pvs_raycast(2, &bounds, &grid, &[], &[], false);
        assert!(
            bit_is_set(&result.bitsets[0], 1),
            "adjacent cluster 0 should see cluster 1 despite wall"
        );
        assert!(
            bit_is_set(&result.bitsets[1], 0),
            "adjacent cluster 1 should see cluster 0 despite wall"
        );
    }

    #[test]
    fn non_adjacent_clusters_blocked_by_wall_brush() {
        let bounds = vec![
            Aabb {
                min: Vec3::new(-200.0, -100.0, -100.0),
                max: Vec3::new(-20.0, 100.0, 100.0),
            },
            Aabb {
                min: Vec3::new(20.0, -100.0, -100.0),
                max: Vec3::new(200.0, 100.0, 100.0),
            },
        ];
        let wall = box_brush(
            Vec3::new(-10.0, -200.0, -200.0),
            Vec3::new(10.0, 200.0, 200.0),
        );
        let world = Aabb {
            min: Vec3::splat(-210.0),
            max: Vec3::splat(210.0),
        };
        let grid = VoxelGrid::from_brushes(&[wall], &world, 4.0);
        let result = compute_pvs_raycast(2, &bounds, &grid, &[], &[], false);
        assert!(
            !bit_is_set(&result.bitsets[0], 1),
            "non-adjacent cluster 0 should NOT see cluster 1 through wall"
        );
        assert!(
            !bit_is_set(&result.bitsets[1], 0),
            "non-adjacent cluster 1 should NOT see cluster 0 through wall"
        );
    }

    // -- face_centroid tests --

    #[test]
    fn face_centroid_computes_average_of_vertices() {
        let vertices = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(10.0, 10.0, 0.0),
            Vec3::new(0.0, 10.0, 0.0),
        ];
        let centroid = face_centroid(&vertices);
        assert!((centroid.x - 5.0).abs() < 1e-6);
        assert!((centroid.y - 5.0).abs() < 1e-6);
        assert!(centroid.z.abs() < 1e-6);
    }

    #[test]
    fn face_centroid_empty_vertices_returns_zero() {
        assert_eq!(face_centroid(&[]), Vec3::ZERO);
    }

    #[test]
    fn face_centroids_included_in_pvs_sample_points() {
        let bounds = vec![
            Aabb {
                min: Vec3::new(-100.0, -50.0, -50.0),
                max: Vec3::new(-10.0, 50.0, 50.0),
            },
            Aabb {
                min: Vec3::new(10.0, -50.0, -50.0),
                max: Vec3::new(100.0, 50.0, 50.0),
            },
        ];
        let centroids = vec![
            vec![Vec3::new(-10.0, 0.0, 0.0)],
            vec![Vec3::new(10.0, 0.0, 0.0)],
        ];
        let world = Aabb {
            min: Vec3::splat(-110.0),
            max: Vec3::splat(110.0),
        };
        let grid = VoxelGrid::from_brushes(&[], &world, 4.0);
        let result = compute_pvs_raycast(2, &bounds, &grid, &centroids, &[], false);
        assert!(
            bit_is_set(&result.bitsets[0], 1),
            "clusters should be visible with face centroid samples"
        );
        assert!(
            bit_is_set(&result.bitsets[1], 0),
            "visibility should be symmetric with face centroid samples"
        );
    }

    #[test]
    fn select_evenly_spaced_caps_centroid_count() {
        let centroids: Vec<Vec3> = (0..50).map(|i| Vec3::new(i as f32, 0.0, 0.0)).collect();
        let selected = select_evenly_spaced(&centroids, 16);
        assert_eq!(selected.len(), 16);
        assert_eq!(selected[0], centroids[0]);
    }

    #[test]
    fn select_evenly_spaced_returns_all_when_under_cap() {
        let centroids = vec![Vec3::X, Vec3::Y, Vec3::Z];
        let selected = select_evenly_spaced(&centroids, 16);
        assert_eq!(selected.len(), 3);
        assert_eq!(selected, centroids);
    }

    // -- confidence mode tests --

    #[test]
    fn confidence_mode_returns_none_when_disabled() {
        let bounds = vec![Aabb {
            min: Vec3::ZERO,
            max: Vec3::ONE * 10.0,
        }];
        let world = Aabb {
            min: Vec3::splat(-10.0),
            max: Vec3::splat(20.0),
        };
        let grid = VoxelGrid::from_brushes(&[], &world, 4.0);
        let result = compute_pvs_raycast(1, &bounds, &grid, &[], &[], false);
        assert!(result.confidence.is_none());
    }

    #[test]
    fn confidence_mode_returns_matrix_when_enabled() {
        let bounds = vec![
            Aabb {
                min: Vec3::new(-100.0, -50.0, -50.0),
                max: Vec3::new(-10.0, 50.0, 50.0),
            },
            Aabb {
                min: Vec3::new(10.0, -50.0, -50.0),
                max: Vec3::new(100.0, 50.0, 50.0),
            },
        ];
        let world = Aabb {
            min: Vec3::splat(-110.0),
            max: Vec3::splat(110.0),
        };
        let grid = VoxelGrid::from_brushes(&[], &world, 4.0);
        let result = compute_pvs_raycast(2, &bounds, &grid, &[], &[], true);
        let conf = result.confidence.expect("confidence should be Some");
        assert_eq!(conf.len(), 2);
        assert_eq!(conf[0].len(), 2);
        // Self-visibility is always 1.0
        assert!((conf[0][0] - 1.0).abs() < 1e-6);
        assert!((conf[1][1] - 1.0).abs() < 1e-6);
        // With no brushes, all rays get through: confidence should be 1.0
        assert!((conf[0][1] - 1.0).abs() < 1e-6);
        assert!((conf[1][0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn confidence_is_zero_when_fully_blocked() {
        let bounds = vec![
            Aabb {
                min: Vec3::new(-200.0, -100.0, -100.0),
                max: Vec3::new(-20.0, 100.0, 100.0),
            },
            Aabb {
                min: Vec3::new(20.0, -100.0, -100.0),
                max: Vec3::new(200.0, 100.0, 100.0),
            },
        ];
        let wall = box_brush(
            Vec3::new(-10.0, -200.0, -200.0),
            Vec3::new(10.0, 200.0, 200.0),
        );
        let world = Aabb {
            min: Vec3::splat(-210.0),
            max: Vec3::splat(210.0),
        };
        let grid = VoxelGrid::from_brushes(&[wall], &world, 4.0);
        let result = compute_pvs_raycast(2, &bounds, &grid, &[], &[], true);
        let conf = result.confidence.expect("confidence should be Some");
        assert!(
            (conf[0][1]).abs() < 1e-6,
            "blocked pair should have 0 confidence"
        );
        assert!(
            (conf[1][0]).abs() < 1e-6,
            "blocked pair should have 0 confidence"
        );
    }

    #[test]
    fn confidence_is_symmetric() {
        let bounds = vec![
            Aabb {
                min: Vec3::new(-100.0, -50.0, -50.0),
                max: Vec3::new(-10.0, 50.0, 50.0),
            },
            Aabb {
                min: Vec3::new(10.0, -50.0, -50.0),
                max: Vec3::new(100.0, 50.0, 50.0),
            },
            Aabb {
                min: Vec3::new(-50.0, 100.0, -50.0),
                max: Vec3::new(50.0, 200.0, 50.0),
            },
        ];
        let world = Aabb {
            min: Vec3::splat(-110.0),
            max: Vec3::splat(210.0),
        };
        let grid = VoxelGrid::from_brushes(&[], &world, 4.0);
        let result = compute_pvs_raycast(3, &bounds, &grid, &[], &[], true);
        let conf = result.confidence.expect("confidence should be Some");
        for i in 0..3 {
            for j in 0..3 {
                assert!(
                    (conf[i][j] - conf[j][i]).abs() < 1e-6,
                    "confidence asymmetry: [{i}][{j}]={} != [{j}][{i}]={}",
                    conf[i][j],
                    conf[j][i],
                );
            }
        }
    }
}
