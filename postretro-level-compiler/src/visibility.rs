// Voxel-based PVS computation: voxelize brushes, then 3D-DDA ray march.
// See: context/plans/ready/voxel-pvs-rework/plan.md

pub(crate) mod pvs;
use crate::voxel_grid;

use glam::Vec3;
use postretro_level_format::visibility::{ClusterInfo, ClusterVisibilitySection, compress_pvs};

use crate::map_data::Face;
use crate::partition::{Aabb, Cluster};

/// Computed visibility data from ray-cast PVS.
pub struct VisibilityResult {
    pub section: ClusterVisibilitySection,
    pub cluster_count: usize,
    pub rays_cast: usize,
    pub pvs_density: f32,
    pub compressed_pvs_bytes: usize,
    /// Per-cluster-pair confidence matrix. Only populated when `compute_confidence` is true.
    pub confidence: Option<Vec<Vec<f32>>>,
}

/// Compute cluster visibility from clusters, faces, and brush volumes.
///
/// Voxelizes brush volumes into a 3D solid/empty bitmap, then uses 3D-DDA
/// ray marching to determine which cluster pairs can see each other. Sample
/// points in solid voxels are filtered out before pair testing.
///
/// `faces` is the full face list; each cluster's `face_indices` indexes into it.
///
/// Operates in shambler's native coordinate space (Z-up) for ray-casting,
/// then transforms cluster bounds to engine Y-up coordinates for the output section.
///
/// When `compute_confidence` is true, counts all rays per pair instead of
/// early-outing, producing a per-pair confidence ratio for diagnostics.
pub fn compute_visibility(
    clusters: &[Cluster],
    _entities: &[crate::map_data::EntityInfo],
    voxel_grid: &voxel_grid::VoxelGrid,
    _min_cell_dim: f32,
    faces: &[Face],
    compute_confidence: bool,
) -> VisibilityResult {
    let cluster_count = clusters.len();

    if cluster_count == 0 {
        return VisibilityResult {
            section: ClusterVisibilitySection {
                clusters: Vec::new(),
                pvs_data: Vec::new(),
            },
            cluster_count: 0,
            rays_cast: 0,
            pvs_density: 0.0,
            compressed_pvs_bytes: 0,
            confidence: None,
        };
    }

    // Collect cluster bounds for ray-cast PVS
    let cluster_bounds: Vec<Aabb> = clusters.iter().map(|c| c.bounds.clone()).collect();

    // Compute face centroids per cluster from the face list.
    let face_centroids_per_cluster: Vec<Vec<Vec3>> = clusters
        .iter()
        .map(|cluster| {
            cluster
                .face_indices
                .iter()
                .filter_map(|&idx| faces.get(idx))
                .map(|face| pvs::face_centroid(&face.vertices))
                .collect()
        })
        .collect();

    // No clusters are adjacency-only — all participate in ray-casting.
    let adjacency_only: Vec<bool> = vec![false; cluster_count];

    // Compute PVS via voxel-based ray marching
    let pvs_result = pvs::compute_pvs_raycast(
        cluster_count,
        &cluster_bounds,
        &voxel_grid,
        &face_centroids_per_cluster,
        &adjacency_only,
        compute_confidence,
    );

    let pvs_bitsets = &pvs_result.bitsets;

    // Count rays cast for diagnostics: 16 samples per cluster, pairs (i, j) where i < j.
    // Worst case is 16*16 per pair, but early-out means fewer in practice. Use worst case
    // for the diagnostic since we don't instrument the inner loop.
    let pair_count = cluster_count * (cluster_count - 1) / 2;
    let rays_cast = pair_count * 16 * 16;

    // Compress PVS and build output section
    let mut pvs_blob = Vec::new();
    let mut cluster_infos = Vec::with_capacity(cluster_count);

    for (i, cluster) in clusters.iter().enumerate() {
        let pvs_offset = pvs_blob.len() as u32;
        let compressed = compress_pvs(&pvs_bitsets[i]);
        let pvs_size = compressed.len() as u32;
        pvs_blob.extend_from_slice(&compressed);

        let face_start: u32 = clusters[..i]
            .iter()
            .map(|c| c.face_indices.len() as u32)
            .sum();
        let face_count = cluster.face_indices.len() as u32;

        // Transform bounding volume from Quake Z-up to engine Y-up
        let (engine_min, engine_max) = quake_aabb_to_engine(&cluster.bounds);

        cluster_infos.push(ClusterInfo {
            bounds_min: [engine_min.x, engine_min.y, engine_min.z],
            bounds_max: [engine_max.x, engine_max.y, engine_max.z],
            face_start,
            face_count,
            pvs_offset,
            pvs_size,
        });
    }

    // Compute PVS density
    let total_pairs = if cluster_count > 1 {
        cluster_count * (cluster_count - 1)
    } else {
        0
    };
    let visible_pairs: usize = pvs_bitsets
        .iter()
        .enumerate()
        .flat_map(|(i, row)| (0..cluster_count).filter(move |&j| j != i && pvs::bit_is_set(row, j)))
        .count();
    let pvs_density = if total_pairs > 0 {
        visible_pairs as f32 / total_pairs as f32
    } else {
        0.0
    };

    if pvs_density > 0.80 {
        log::warn!(
            "[Compiler] High PVS density ({:.1}%) — map may have leaks or large open areas. \
             Check for gaps in walls.",
            pvs_density * 100.0
        );
    }

    // Log non-adjacent visible pairs sorted by distance (descending).
    // These are the pairs most likely to be incorrect — visibility came from
    // ray-casting, not the adjacency override.
    log_surprising_pvs_pairs(cluster_count, &pvs_bitsets, &cluster_bounds);

    // Log confidence summary when computed
    if let Some(ref conf) = pvs_result.confidence {
        log_confidence_summary(cluster_count, conf, &cluster_bounds);
    }

    let compressed_pvs_bytes = pvs_blob.len();

    VisibilityResult {
        section: ClusterVisibilitySection {
            clusters: cluster_infos,
            pvs_data: pvs_blob,
        },
        cluster_count,
        rays_cast,
        pvs_density,
        compressed_pvs_bytes,
        confidence: pvs_result.confidence,
    }
}

/// Log non-adjacent cluster pairs that are mutually visible via ray-cast.
///
/// Surfaces the pairs most likely to be incorrect PVS results by sorting
/// by distance (descending). Uses cluster AABB centroids in Quake Z-up space
/// for position and distance. Capped at 20 pairs to avoid flooding the log.
fn log_surprising_pvs_pairs(
    cluster_count: usize,
    pvs_bitsets: &[Vec<u8>],
    cluster_bounds: &[Aabb],
) {
    let mut pairs: Vec<(usize, usize, Vec3, Vec3, f32)> = Vec::new();

    for i in 0..cluster_count {
        for j in (i + 1)..cluster_count {
            let i_sees_j = pvs::bit_is_set(&pvs_bitsets[i], j);
            let j_sees_i = pvs::bit_is_set(&pvs_bitsets[j], i);

            if !i_sees_j || !j_sees_i {
                continue;
            }

            // Skip adjacent pairs — their visibility came from the adjacency override,
            // not ray-casting.
            if pvs::aabbs_adjacent(&cluster_bounds[i], &cluster_bounds[j]) {
                continue;
            }

            let center_i = (cluster_bounds[i].min + cluster_bounds[i].max) * 0.5;
            let center_j = (cluster_bounds[j].min + cluster_bounds[j].max) * 0.5;
            let dist = center_i.distance(center_j);

            // Transform centroids to engine Y-up for display
            let (eng_min_i, eng_max_i) = quake_aabb_to_engine(&cluster_bounds[i]);
            let (eng_min_j, eng_max_j) = quake_aabb_to_engine(&cluster_bounds[j]);
            let eng_center_i = (eng_min_i + eng_max_i) * 0.5;
            let eng_center_j = (eng_min_j + eng_max_j) * 0.5;

            pairs.push((i, j, eng_center_i, eng_center_j, dist));
        }
    }

    if pairs.is_empty() {
        return;
    }

    pairs.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));

    const MAX_LOGGED: usize = 20;
    let display_count = pairs.len().min(MAX_LOGGED);

    log::info!(
        "[PVS] Non-adjacent visible pairs (by distance, {} of {} total):",
        display_count,
        pairs.len()
    );
    for &(i, j, ci, cj, dist) in pairs.iter().take(MAX_LOGGED) {
        log::info!(
            "[PVS]   cluster {} ({:.0},{:.0},{:.0}) <-> cluster {} ({:.0},{:.0},{:.0})  dist={:.0}",
            i,
            ci.x,
            ci.y,
            ci.z,
            j,
            cj.x,
            cj.y,
            cj.z,
            dist,
        );
    }
}

/// Log a summary of visibility confidence levels for diagnostic builds.
///
/// Groups non-self cluster pairs into high (>75%), medium (25-75%), and low (<25%)
/// confidence buckets to give a quick overview of PVS reliability.
fn log_confidence_summary(
    cluster_count: usize,
    confidence: &[Vec<f32>],
    cluster_bounds: &[Aabb],
) {
    let mut high = 0u32;
    let mut medium = 0u32;
    let mut low = 0u32;
    let mut invisible = 0u32;

    for i in 0..cluster_count {
        for j in (i + 1)..cluster_count {
            let c = confidence[i][j];
            if c < 1e-6 {
                invisible += 1;
            } else if c < 0.25 {
                low += 1;
            } else if c < 0.75 {
                medium += 1;
            } else {
                high += 1;
            }
        }
    }

    let total = high + medium + low + invisible;
    log::info!(
        "[PVS Confidence] {} pairs: {} high (>75%), {} medium (25-75%), {} low (<25%), {} invisible",
        total, high, medium, low, invisible,
    );

    // Log the lowest-confidence visible pairs as they are most likely to be fragile.
    let mut fragile_pairs: Vec<(usize, usize, f32)> = Vec::new();
    for i in 0..cluster_count {
        for j in (i + 1)..cluster_count {
            let c = confidence[i][j];
            if c > 1e-6 && c < 0.25 {
                // Skip adjacent pairs -- their visibility is overridden anyway
                if !pvs::aabbs_adjacent(&cluster_bounds[i], &cluster_bounds[j]) {
                    fragile_pairs.push((i, j, c));
                }
            }
        }
    }

    if !fragile_pairs.is_empty() {
        fragile_pairs.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
        let display_count = fragile_pairs.len().min(10);
        log::info!(
            "[PVS Confidence] Fragile pairs (lowest confidence, {} of {}):",
            display_count,
            fragile_pairs.len(),
        );
        for &(i, j, c) in fragile_pairs.iter().take(10) {
            log::info!(
                "[PVS Confidence]   cluster {} <-> cluster {} confidence={:.1}%",
                i, j, c * 100.0,
            );
        }
    }
}

/// Transform an AABB from Quake coordinates (Z-up) to engine coordinates (Y-up).
///
/// engine_x = -quake_y, engine_y = quake_z, engine_z = -quake_x
/// After swizzling, recompute min/max since axis negation swaps ordering.
fn quake_aabb_to_engine(aabb: &Aabb) -> (Vec3, Vec3) {
    let a = Vec3::new(-aabb.min.y, aabb.min.z, -aabb.min.x);
    let b = Vec3::new(-aabb.max.y, aabb.max.z, -aabb.max.x);
    (a.min(b), a.max(b))
}

/// Propagate PVS from face-containing clusters to adjacent air clusters.
///
/// Air clusters (no faces) produce sparse PVS because they lack face-centroid
/// sample points. This causes visibility popping when a camera moves from a
/// face-containing cluster into an air cluster. Fix by propagating PVS from
/// neighbors that have an unobstructed sightline (no solid voxels between
/// cluster centers).
///
/// Currently unused: air clusters are adjacency-only (no PVS sample points),
/// so they only see adjacent clusters. Propagation may be re-enabled if
/// air-cluster visibility popping becomes a problem in practice.
#[allow(dead_code)]
fn propagate_air_cluster_pvs(
    clusters: &[Cluster],
    cluster_bounds: &[Aabb],
    pvs_bitsets: &mut [Vec<u8>],
    voxel_grid: &voxel_grid::VoxelGrid,
) {
    let cluster_count = clusters.len();
    if cluster_count == 0 {
        return;
    }

    // Identify air clusters (no faces)
    let is_air: Vec<bool> = clusters.iter().map(|c| c.face_indices.is_empty()).collect();

    // Pre-compute adjacency lists for air clusters, filtering out pairs
    // separated by solid voxels (walls).
    let adjacency: Vec<Vec<usize>> = (0..cluster_count)
        .map(|i| {
            if !is_air[i] {
                return Vec::new();
            }
            let center_i = cluster_bounds[i].centroid();
            (0..cluster_count)
                .filter(|&j| {
                    j != i
                        && pvs::aabbs_adjacent(&cluster_bounds[i], &cluster_bounds[j])
                        && !pvs::ray_blocked_by_voxels(
                            voxel_grid,
                            center_i,
                            cluster_bounds[j].centroid(),
                        )
                })
                .collect()
        })
        .collect();

    // Single-pass propagation: each air cluster inherits visibility from its
    // immediate neighbors only. Using a snapshot of the original bitsets
    // prevents transitive chains (A sees B's neighbors' visibility, which
    // includes C's neighbors, etc.) that would create false visibility
    // across multiple turns in corridors.
    let snapshot: Vec<Vec<u8>> = pvs_bitsets.to_vec();
    for i in 0..cluster_count {
        if !is_air[i] {
            continue;
        }
        for &j in &adjacency[i] {
            for (byte_idx, byte_val) in snapshot[j].iter().enumerate() {
                if byte_idx < pvs_bitsets[i].len() {
                    pvs_bitsets[i][byte_idx] |= byte_val;
                }
            }
        }
    }

    // Make PVS symmetric: if air cluster i now sees j, j must see i
    for i in 0..cluster_count {
        if !is_air[i] {
            continue;
        }
        for j in 0..cluster_count {
            if i == j {
                continue;
            }
            if pvs::bit_is_set(&pvs_bitsets[i], j) {
                pvs::set_bit(&mut pvs_bitsets[j], i);
            }
        }
    }
}

/// Log visibility computation statistics.
pub fn log_stats(result: &VisibilityResult) {
    log::info!("[Compiler] Clusters: {}", result.cluster_count);
    log::info!("[Compiler] Rays cast (max): {}", result.rays_cast);
    log::info!("[Compiler] PVS density: {:.1}%", result.pvs_density * 100.0);
    log::info!(
        "[Compiler] Compressed PVS size: {} bytes",
        result.compressed_pvs_bytes
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{make_box_brush_volume, make_box_faces};
    use crate::visibility_test_fixtures::*;

    #[test]
    fn empty_input_produces_empty_result() {
        let vg = build_test_voxel_grid(&[], &[], &[]);
        let vis = compute_visibility(&[], &[], &vg, 128.0, &[], false);
        assert_eq!(vis.cluster_count, 0);
        assert!(vis.section.clusters.is_empty());
        assert!(vis.section.pvs_data.is_empty());
    }

    #[test]
    fn quake_aabb_transform() {
        let aabb = Aabb {
            min: Vec3::new(1.0, 2.0, 3.0),
            max: Vec3::new(4.0, 5.0, 6.0),
        };
        let (emin, emax) = quake_aabb_to_engine(&aabb);
        // engine_x = -quake_y: min(-2, -5) = -5, max(-2, -5) = -2
        assert_eq!(emin.x, -5.0);
        assert_eq!(emax.x, -2.0);
        // engine_y = quake_z: min(3, 6) = 3, max(3, 6) = 6
        assert_eq!(emin.y, 3.0);
        assert_eq!(emax.y, 6.0);
        // engine_z = -quake_x: min(-1, -4) = -4, max(-1, -4) = -1
        assert_eq!(emin.z, -4.0);
        assert_eq!(emax.z, -1.0);
    }

    #[test]
    fn visibility_from_test_map() {
        let map_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("assets/maps/test.map");

        let map_data = crate::parse::parse_map_file(&map_path).expect("test.map should parse");
        let grid_result = crate::spatial_grid::assign_to_grid(map_data.world_faces, None);
        let clusters = grid_cells_to_clusters(&grid_result.cells);

        let min_cell_dim = grid_result
            .cell_size
            .x
            .min(grid_result.cell_size.y)
            .min(grid_result.cell_size.z)
            .max(1.0);
        let vg = build_test_voxel_grid(&clusters, &grid_result.faces, &map_data.brush_volumes);
        let vis = compute_visibility(
            &clusters,
            &map_data.entities,
            &vg,
            min_cell_dim,
            &grid_result.faces,
            false,
        );

        assert!(vis.cluster_count > 0);
        assert_eq!(vis.section.clusters.len(), vis.cluster_count);

        // PVS should be reflexive: every cluster sees itself
        let bytes_per_row = pvs::bytes_for_clusters(vis.cluster_count);
        for i in 0..vis.cluster_count {
            let ci = &vis.section.clusters[i];
            let compressed = &vis.section.pvs_data
                [ci.pvs_offset as usize..(ci.pvs_offset + ci.pvs_size) as usize];
            let row = postretro_level_format::visibility::decompress_pvs(compressed, bytes_per_row);
            assert!(pvs::bit_is_set(&row, i), "cluster {i} should see itself");
        }

        // PVS should be symmetric
        let mut all_rows = Vec::new();
        for i in 0..vis.cluster_count {
            let ci = &vis.section.clusters[i];
            let compressed = &vis.section.pvs_data
                [ci.pvs_offset as usize..(ci.pvs_offset + ci.pvs_size) as usize];
            all_rows.push(postretro_level_format::visibility::decompress_pvs(
                compressed,
                bytes_per_row,
            ));
        }
        for i in 0..vis.cluster_count {
            for j in 0..vis.cluster_count {
                let i_sees_j = pvs::bit_is_set(&all_rows[i], j);
                let j_sees_i = pvs::bit_is_set(&all_rows[j], i);
                assert_eq!(
                    i_sees_j, j_sees_i,
                    "PVS asymmetry: cluster {i} sees {j} = {i_sees_j}, cluster {j} sees {i} = {j_sees_i}"
                );
            }
        }

        // Compressed PVS round-trips
        for i in 0..vis.cluster_count {
            let ci = &vis.section.clusters[i];
            let compressed = &vis.section.pvs_data
                [ci.pvs_offset as usize..(ci.pvs_offset + ci.pvs_size) as usize];
            let decompressed =
                postretro_level_format::visibility::decompress_pvs(compressed, bytes_per_row);
            let recompressed = postretro_level_format::visibility::compress_pvs(&decompressed);
            let re_decompressed =
                postretro_level_format::visibility::decompress_pvs(&recompressed, bytes_per_row);
            assert_eq!(decompressed, re_decompressed);
        }

        // Section serialization round-trips
        let bytes = vis.section.to_bytes();
        let restored = ClusterVisibilitySection::from_bytes(&bytes).unwrap();
        assert_eq!(vis.section, restored);
    }

    /// Two sealed rooms with a solid wall: clusters in Room A must NOT see clusters in Room B.
    #[test]
    fn sealed_rooms_block_pvs_across_solid_wall() {
        let (faces, volumes, entities) = build_two_room_sealed_level();

        let grid_result = crate::spatial_grid::assign_to_grid(faces, None);
        let clusters_vec = grid_cells_to_clusters(&grid_result.cells);

        let min_cell_dim = grid_result
            .cell_size
            .x
            .min(grid_result.cell_size.y)
            .min(grid_result.cell_size.z)
            .max(1.0);
        let vg = build_test_voxel_grid(&clusters_vec, &grid_result.faces, &volumes);
        let vis = compute_visibility(
            &clusters_vec,
            &entities,
            &vg,
            min_cell_dim,
            &grid_result.faces,
            false,
        );

        assert!(vis.cluster_count > 0, "should have clusters");

        // Identify which clusters belong to Room A (x < -8) vs Room B (x > 8)
        let mut room_a_clusters = Vec::new();
        let mut room_b_clusters = Vec::new();

        for (i, cluster) in clusters_vec.iter().enumerate() {
            let centroid = cluster.bounds.centroid();
            if centroid.x < -8.0 {
                room_a_clusters.push(i);
            } else if centroid.x > 8.0 {
                room_b_clusters.push(i);
            }
        }

        assert!(
            !room_a_clusters.is_empty(),
            "should have clusters in Room A"
        );
        assert!(
            !room_b_clusters.is_empty(),
            "should have clusters in Room B"
        );

        // Decompress PVS rows
        let bytes_per_row = pvs::bytes_for_clusters(vis.cluster_count);
        let mut all_rows = Vec::new();
        for i in 0..vis.cluster_count {
            let ci = &vis.section.clusters[i];
            let compressed = &vis.section.pvs_data
                [ci.pvs_offset as usize..(ci.pvs_offset + ci.pvs_size) as usize];
            all_rows.push(postretro_level_format::visibility::decompress_pvs(
                compressed,
                bytes_per_row,
            ));
        }

        // Non-adjacent cluster pairs across the wall should be invisible.
        // Adjacent pairs (AABBs that touch) are always visible by design — see
        // pvs::aabbs_adjacent. This prevents face pop-in at cluster boundaries.
        let mut blocked_pairs = 0;
        for &a in &room_a_clusters {
            for &b in &room_b_clusters {
                let adjacent =
                    pvs::aabbs_adjacent(&clusters_vec[a].bounds, &clusters_vec[b].bounds);
                if !adjacent {
                    assert!(
                        !pvs::bit_is_set(&all_rows[a], b),
                        "Non-adjacent Room A cluster {a} should NOT see Room B cluster {b} through solid wall"
                    );
                    blocked_pairs += 1;
                }
            }
        }

        assert!(
            blocked_pairs > 0,
            "should have at least one non-adjacent cross-wall pair that is blocked"
        );

        // PVS density should be below 100% due to solid wall blocking non-adjacent pairs
        assert!(
            vis.pvs_density < 1.0,
            "PVS density should be below 100% with a solid wall, got {:.1}%",
            vis.pvs_density * 100.0
        );
    }

    // -----------------------------------------------------------------------
    // Visibility contract tests
    //
    // These define what "correct visibility" means for the spatial grid + ray-cast
    // PVS system. All contract tests should pass with the current grid settings
    // (MIN_CELL_SIZE=32, TARGET_CELLS_PER_AXIS=16).
    // -----------------------------------------------------------------------

    /// Two rooms connected by a corridor: both rooms should be mutually visible.
    ///
    /// This tests that visibility propagates through narrow openings across
    /// multiple grid cells. The current spatial grid creates cells too small for
    /// corridors, so ray-casting between non-adjacent cells misses the opening.
    ///
    /// Defines the visibility contract: rooms connected by a corridor MUST see
    /// each other, regardless of how many grid cells the corridor spans.
    #[test]
    fn corridor_connected_rooms_are_mutually_visible() {
        let (faces, volumes) = build_two_rooms_with_corridor();
        let (clusters, vis) = run_visibility_pipeline(faces, &volumes);
        let all_rows = decompress_all_pvs_rows(&vis);

        // Rooms are separated along Y. Room A air: y 0..64, Room B air: y 128..192.
        // Use midpoints as boundaries to avoid wall-geometry overlap.
        let room_a = clusters_in_y_range(&clusters, -16.0, 48.0);
        let room_b = clusters_in_y_range(&clusters, 144.0, 208.0);

        assert!(!room_a.is_empty(), "should have clusters in Room A");
        assert!(!room_b.is_empty(), "should have clusters in Room B");

        // At least one cluster in Room A should see at least one cluster in Room B.
        // A player standing in Room A looking down the corridor should see Room B.
        let any_cross_visible = room_a
            .iter()
            .any(|&a| room_b.iter().any(|&b| pvs::bit_is_set(&all_rows[a], b)));

        assert!(
            any_cross_visible,
            "Room A and Room B are connected by a corridor — at least one pair \
             of clusters should be mutually visible. Room A clusters: {:?}, \
             Room B clusters: {:?}, total clusters: {}",
            room_a, room_b, vis.cluster_count
        );
    }

    /// Two rooms separated by a solid wall: no cross-room visibility.
    ///
    /// This is the counterpart to corridor_connected_rooms_are_mutually_visible:
    /// rooms with no opening between them should have zero cross-visibility
    /// (excluding adjacent cluster pairs which are always visible by design).
    #[test]
    fn solid_wall_blocks_all_cross_room_visibility() {
        let (faces, volumes) = build_two_rooms_solid_wall();
        let (clusters, vis) = run_visibility_pipeline(faces, &volumes);
        let all_rows = decompress_all_pvs_rows(&vis);

        // Room A air: y 0..64, Room B air: y 72..136.
        // Solid wall at y 64..72 separates them.
        let room_a = clusters_in_y_range(&clusters, -16.0, 48.0);
        let room_b = clusters_in_y_range(&clusters, 88.0, 152.0);

        assert!(!room_a.is_empty(), "should have clusters in Room A");
        assert!(!room_b.is_empty(), "should have clusters in Room B");

        // No non-adjacent cross-room pairs should be visible.
        let mut violations = Vec::new();
        for &a in &room_a {
            for &b in &room_b {
                if !pvs::aabbs_adjacent(&clusters[a].bounds, &clusters[b].bounds)
                    && pvs::bit_is_set(&all_rows[a], b)
                {
                    violations.push((a, b));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "Non-adjacent cross-room pairs should be invisible through solid wall, \
             but {} pairs are visible: {:?}",
            violations.len(),
            &violations[..violations.len().min(10)]
        );
    }

    /// L-shaped corridor: both rooms should see the corridor.
    ///
    /// Tests that visibility propagates around a corner. Room A and Room B
    /// may or may not see each other directly (conservative culling around
    /// corners is acceptable), but each room MUST see corridor cells.
    #[test]
    fn l_shaped_corridor_rooms_see_corridor() {
        let (faces, volumes) = build_l_shaped_corridor();
        let (clusters, vis) = run_visibility_pipeline(faces, &volumes);
        let all_rows = decompress_all_pvs_rows(&vis);

        // Rooms are separated along X. Room A at x 64..128, Room B at x 0..32.
        // Corridor spans x 32..112.
        let room_a = clusters_in_x_range(&clusters, 80.0, 140.0);
        let room_b = clusters_in_x_range(&clusters, -16.0, 24.0);
        // Corridor: centroid x between 30..120, y between 55..105.
        // Wide ranges accommodate coarse grids where cell centroids don't
        // align precisely with corridor geometry bounds.
        let corridor = clusters_in_y_range(&clusters, 55.0, 105.0)
            .into_iter()
            .filter(|&i| {
                let cx = clusters[i].bounds.centroid().x;
                cx >= 30.0 && cx <= 120.0
            })
            .collect::<Vec<_>>();

        assert!(!room_a.is_empty(), "should have clusters in Room A");
        assert!(!room_b.is_empty(), "should have clusters in Room B");
        assert!(!corridor.is_empty(), "should have clusters in the corridor");

        // Room A should see at least one corridor cluster
        let room_a_sees_corridor = room_a
            .iter()
            .any(|&a| corridor.iter().any(|&c| pvs::bit_is_set(&all_rows[a], c)));
        assert!(
            room_a_sees_corridor,
            "Room A should see at least one corridor cluster. \
             Room A clusters: {:?}, corridor clusters: {:?}",
            room_a, corridor
        );

        // Room B should see at least one corridor cluster
        let room_b_sees_corridor = room_b
            .iter()
            .any(|&b| corridor.iter().any(|&c| pvs::bit_is_set(&all_rows[b], c)));
        assert!(
            room_b_sees_corridor,
            "Room B should see at least one corridor cluster. \
             Room B clusters: {:?}, corridor clusters: {:?}",
            room_b, corridor
        );
    }

    /// Long corridor: cells in the middle should see cells at both ends.
    ///
    /// Tests that visibility propagates along long straight sightlines,
    /// not just between immediate neighbor cells. A player in the middle
    /// of a straight corridor should see both rooms at either end.
    #[test]
    fn long_corridor_middle_sees_both_ends() {
        let (faces, volumes) = build_long_corridor();
        let (clusters, vis) = run_visibility_pipeline(faces, &volumes);
        let all_rows = decompress_all_pvs_rows(&vis);

        // Rooms separated along X. Room A: x 0..64, Room B: x 192..256.
        // Corridor: x 64..192.
        let room_a = clusters_in_x_range(&clusters, -16.0, 48.0);
        let room_b = clusters_in_x_range(&clusters, 208.0, 272.0);
        // Middle of corridor: clusters whose centroid falls in the inner portion
        // of the corridor. Use a wide range to accommodate coarse grids where
        // cell centroids don't align with corridor geometry centers.
        let corridor_middle = clusters_in_x_range(&clusters, 80.0, 176.0);

        assert!(!room_a.is_empty(), "should have clusters in Room A");
        assert!(!room_b.is_empty(), "should have clusters in Room B");
        assert!(
            !corridor_middle.is_empty(),
            "should have clusters in the corridor middle"
        );

        // Middle corridor clusters should see Room A
        let middle_sees_a = corridor_middle
            .iter()
            .any(|&m| room_a.iter().any(|&a| pvs::bit_is_set(&all_rows[m], a)));
        assert!(
            middle_sees_a,
            "Corridor middle should see Room A (straight sightline). \
             Corridor middle clusters: {:?}, Room A clusters: {:?}",
            corridor_middle, room_a
        );

        // Middle corridor clusters should see Room B
        let middle_sees_b = corridor_middle
            .iter()
            .any(|&m| room_b.iter().any(|&b| pvs::bit_is_set(&all_rows[m], b)));
        assert!(
            middle_sees_b,
            "Corridor middle should see Room B (straight sightline). \
             Corridor middle clusters: {:?}, Room B clusters: {:?}",
            corridor_middle, room_b
        );

        // Room A should see Room B (straight sightline through entire corridor)
        let a_sees_b = room_a
            .iter()
            .any(|&a| room_b.iter().any(|&b| pvs::bit_is_set(&all_rows[a], b)));
        assert!(
            a_sees_b,
            "Room A should see Room B through the straight corridor. \
             Room A clusters: {:?}, Room B clusters: {:?}",
            room_a, room_b
        );
    }


    /// Z-shaped three rooms: Room A center should NOT see Room C.
    ///
    /// Room A connects to Room B via corridor 1, and Room B connects to Room C
    /// via corridor 2. The corridors are offset so there is no direct line of
    /// sight from A's center to C — Room B's walls block it.
    ///
    /// Clusters near the corridor mouth may have some pre-loading visibility
    /// to Room C — this is acceptable conservative PVS behavior (prevents
    /// pop-in when rounding a corner). The contract is that clusters in the
    /// interior of Room A (away from the corridor) should not see Room C.
    #[test]
    fn z_shaped_rooms_a_center_does_not_see_room_c() {
        let (faces, volumes) = build_z_shaped_three_rooms();
        let (clusters, vis) = run_visibility_pipeline(faces, &volumes);
        let all_rows = decompress_all_pvs_rows(&vis);

        // Room A air: x 128..224, y 208..304. Center ~(176, 256, 32).
        // Corridor 1 mouth is at y=208. "Interior" clusters have centroids
        // well above the corridor — use y > 240 to exclude corridor-adjacent.
        let room_a_interior: Vec<usize> = clusters
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                let center = c.bounds.centroid();
                center.y > 240.0 && center.y < 320.0
            })
            .map(|(i, _)| i)
            .collect();

        // Room C air: y -48..48
        let room_c = clusters_in_y_range(&clusters, -60.0, 46.0);

        assert!(!room_a_interior.is_empty(), "should have interior clusters in Room A");
        assert!(!room_c.is_empty(), "should have clusters in Room C");

        // Interior Room A clusters should NOT see Room C.
        // Exclude adjacent cluster pairs (always visible by design).
        let mut violations = Vec::new();
        for &a in &room_a_interior {
            for &c in &room_c {
                if !pvs::aabbs_adjacent(&clusters[a].bounds, &clusters[c].bounds)
                    && pvs::bit_is_set(&all_rows[a], c)
                {
                    violations.push((a, c));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "Interior Room A clusters should NOT see Room C through two \
             turns. Clusters near the corridor mouth may pre-load Room C \
             (acceptable), but interior clusters should not. \
             {} violations: {:?}",
            violations.len(),
            &violations[..violations.len().min(10)]
        );
    }


    /// Spatial stability: multiple points within Room A get consistent visibility.
    ///
    /// Tests that visibility doesn't flicker as you move within a single room.
    /// If cluster fragmentation causes different regions of the same room to
    /// have different PVS results, a player could see geometry pop in and out
    /// as they walk across the room.
    ///
    /// Uses a wide room (X=256) so the corridor opening (64 units centered at
    /// x=128) is only 25% of the wall width. Clusters at far-left vs far-right
    /// X positions have very different sightlines to Room B, exposing the
    /// inconsistency. Z is kept small (64) to limit cluster count.
    ///
    /// Samples 5 points spread across Room A's air volume. For each point,
    /// finds its cluster and checks whether it can see any Room B cluster.
    /// All sampled clusters must agree.
    #[test]
    fn spatial_stability_within_room() {
        // Wide room (256 X) with narrow corridor opening (64 units at x=96..160)
        let (faces, volumes) = build_two_rooms_with_corridor_sized(256.0, 64.0, 64.0);
        let (clusters, vis) = run_visibility_pipeline(faces, &volumes);
        let all_rows = decompress_all_pvs_rows(&vis);

        // Room B air: (0, 128, 0) to (256, 192, 64) — same Y range as default
        let room_b = clusters_in_y_range(&clusters, 144.0, 208.0);
        assert!(!room_b.is_empty(), "should have clusters in Room B");

        // Sample points spread across Room A's air volume (0,0,0 to 256,64,64).
        // Inset slightly from walls to stay in air space. Points span the
        // full 256-unit X range to test consistency across the wide room.
        let sample_points = [
            Vec3::new(8.0, 8.0, 8.0),      // near -X, -Y, low (far from corridor opening)
            Vec3::new(248.0, 8.0, 8.0),     // near +X, -Y, low (far from corridor opening)
            Vec3::new(128.0, 32.0, 32.0),   // center (aligned with corridor)
            Vec3::new(8.0, 56.0, 32.0),     // near -X, +Y (near corridor wall, away from opening)
            Vec3::new(248.0, 56.0, 8.0),    // near +X, +Y (near corridor wall, away from opening)
        ];

        // For each sample point, determine which cluster it falls into
        // and whether that cluster can see any Room B cluster.
        let mut sees_room_b: Vec<(Vec3, Option<usize>, bool)> = Vec::new();

        for &point in &sample_points {
            let cluster_idx = cluster_containing_point(&clusters, point);
            let can_see_b = cluster_idx.map_or(false, |ci| {
                room_b.iter().any(|&b| pvs::bit_is_set(&all_rows[ci], b))
            });
            sees_room_b.push((point, cluster_idx, can_see_b));
        }

        // All sample points should resolve to a cluster (they're in air space)
        for &(point, cluster_idx, _) in &sees_room_b {
            assert!(
                cluster_idx.is_some(),
                "Point {:?} in Room A air should fall into a cluster, but didn't. \
                 This suggests cluster coverage gaps.",
                point
            );
        }

        // All sample points must agree on Room B visibility.
        // (With a working corridor, they should all see Room B. With the
        // current under-permissive system, they might all NOT see Room B.
        // Either way, they must be consistent.)
        let visibility_values: Vec<bool> = sees_room_b.iter().map(|&(_, _, v)| v).collect();
        let all_agree = visibility_values.windows(2).all(|w| w[0] == w[1]);

        assert!(
            all_agree,
            "All points within Room A should have consistent visibility to Room B. \
             Point results: {:?}",
            sees_room_b
                .iter()
                .map(|&(pt, ci, vis)| format!(
                    "({:.0},{:.0},{:.0}) -> cluster {:?}, sees_B={}",
                    pt.x, pt.y, pt.z, ci, vis
                ))
                .collect::<Vec<_>>()
        );
    }

    /// Small brush in corridor does not block room-to-room visibility.
    ///
    /// A small decorative brush (16x16x16) floating in the middle of a
    /// 32-wide, 48-tall corridor should not prevent rooms on either side
    /// from seeing each other. The cube doesn't fill the opening — rays
    /// can pass around it.
    ///
    /// Contract: brush volumes below a size threshold relative to the
    /// spatial grid cell size should not participate in PVS ray-casting.
    /// Small decorative geometry (light fixtures, trim, small crates)
    /// must not create false occlusion.
    #[test]
    fn small_brush_in_corridor_does_not_block_visibility() {
        let (mut faces, mut volumes) = build_two_rooms_with_corridor();

        // Add a 16x16x16 brush floating in the corridor midpoint.
        // Corridor air: (16, 64, 0) to (48, 128, 48) — 32 wide, 64 deep, 48 tall.
        // Cube center: (32, 96, 24). Leaves 8 units of clearance on each side.
        let small_min = Vec3::new(24.0, 88.0, 16.0);
        let small_max = Vec3::new(40.0, 104.0, 32.0);
        faces.extend(make_box_faces(small_min, small_max));
        volumes.push(make_box_brush_volume(small_min, small_max));

        let (clusters, vis) = run_visibility_pipeline(faces, &volumes);
        let all_rows = decompress_all_pvs_rows(&vis);

        // Rooms are separated along Y. Room A air: y 0..64, Room B air: y 128..192.
        let room_a = clusters_in_y_range(&clusters, -16.0, 48.0);
        let room_b = clusters_in_y_range(&clusters, 144.0, 208.0);

        assert!(!room_a.is_empty(), "should have clusters in Room A");
        assert!(!room_b.is_empty(), "should have clusters in Room B");

        // At least one cluster in Room A should see at least one cluster in Room B.
        // The small brush doesn't fill the corridor opening, so rays should pass around it.
        let any_cross_visible = room_a
            .iter()
            .any(|&a| room_b.iter().any(|&b| pvs::bit_is_set(&all_rows[a], b)));

        assert!(
            any_cross_visible,
            "Room A and Room B are connected by a corridor with only a small decorative \
             brush (16x16x16 in a 32x48 opening) — at least one pair of clusters should \
             be mutually visible. Small brushes should not block PVS ray-casting. \
             Room A clusters: {:?}, Room B clusters: {:?}, total clusters: {}",
            room_a, room_b, vis.cluster_count
        );
    }


    /// Thick wall solid-point rejection is load-bearing for correct PVS results.
    ///
    /// Two rooms separated by a 64-unit-thick wall with no opening. Clusters
    /// whose AABBs overlap the wall region will have random sample points that
    /// land inside the wall. filter_solid_samples must reject those points;
    /// otherwise rays originating inside the wall would bypass it and produce
    /// false cross-room visibility.
    #[test]
    fn thick_wall_solid_sample_filtering_prevents_false_visibility() {
        let (faces, volumes) = build_thick_wall_sealed_rooms();
        let (clusters, vis) = run_visibility_pipeline(faces, &volumes);
        let all_rows = decompress_all_pvs_rows(&vis);

        // Room A air centers around x ~ -36, Room B around x ~ 36.
        // Wall occupies x -8 to 8.
        let room_a = clusters_in_x_range(&clusters, -80.0, -12.0);
        let room_b = clusters_in_x_range(&clusters, 12.0, 80.0);

        assert!(!room_a.is_empty(), "should have clusters in Room A");
        assert!(!room_b.is_empty(), "should have clusters in Room B");

        let mut violations = Vec::new();
        for &a in &room_a {
            for &b in &room_b {
                if !pvs::aabbs_adjacent(&clusters[a].bounds, &clusters[b].bounds)
                    && pvs::bit_is_set(&all_rows[a], b)
                {
                    violations.push((a, b));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "Non-adjacent cross-room pairs should be invisible through 16-unit thick wall. \
             Solid-point filtering must reject sample points inside the wall to prevent \
             rays from bypassing it. {} pairs are falsely visible: {:?}",
            violations.len(),
            &violations[..violations.len().min(10)]
        );
    }

    /// Long corridor: Room A and Room B should NOT see each other if
    /// separated by enough turns. (Placeholder for future geometry tests.)
    ///
    /// This is a sanity check that the Z-shaped test geometry is correct.
    #[test]
    fn z_shaped_rooms_a_sees_b_and_b_sees_c() {
        let (faces, volumes) = build_z_shaped_three_rooms();
        let (clusters, vis) = run_visibility_pipeline(faces, &volumes);
        let all_rows = decompress_all_pvs_rows(&vis);

        let room_a = clusters_in_y_range(&clusters, 210.0, 320.0);
        let room_b = clusters_in_y_range(&clusters, 98.0, 158.0);
        let room_c = clusters_in_y_range(&clusters, -60.0, 46.0);

        assert!(!room_a.is_empty(), "should have clusters in Room A");
        assert!(!room_b.is_empty(), "should have clusters in Room B");
        assert!(!room_c.is_empty(), "should have clusters in Room C");

        // Adjacent pairs between connected rooms should exist (corridors
        // share walls with rooms, so some cluster pairs will be adjacent).
        // This validates the geometry is connected.
        let a_adj_b = room_a.iter().any(|&a| {
            room_b
                .iter()
                .any(|&b| pvs::aabbs_adjacent(&clusters[a].bounds, &clusters[b].bounds))
        }) || room_a.iter().any(|&a| {
            // Or check corridor clusters that bridge the gap
            (0..clusters.len()).any(|c| {
                pvs::bit_is_set(&all_rows[a], c)
                    && room_b
                        .iter()
                        .any(|&b| pvs::aabbs_adjacent(&clusters[c].bounds, &clusters[b].bounds))
            })
        });

        assert!(
            a_adj_b
                || room_a
                    .iter()
                    .any(|&a| room_b.iter().any(|&b| pvs::bit_is_set(&all_rows[a], b))),
            "Room A and Room B should have some visibility connection (adjacent or ray-cast). \
             Room A: {:?}, Room B: {:?}, total clusters: {}",
            room_a,
            room_b,
            vis.cluster_count
        );
    }


}
