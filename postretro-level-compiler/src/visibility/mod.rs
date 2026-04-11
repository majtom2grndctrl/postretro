// Portal-based PVS: generate portals, flood-fill visibility, encode bitsets.
// See: context/lib/build_pipeline.md §PRL

pub mod portal_vis;

use std::collections::{HashSet, VecDeque};

use crate::partition::{self, BspChild, BspTree};
use crate::portals::Portal;
use glam::DVec3;
use postretro_level_format::bsp::{
    BspLeafRecord, BspLeavesSection, BspNodeRecord, BspNodesSection,
};
use postretro_level_format::leaf_pvs::LeafPvsSection;
use postretro_level_format::visibility::compress_pvs;

/// Narrow a compile-time `DVec3` to the `[f32; 3]` layout used in the PRL file
/// format. Called at the compiler's output boundary only — internal geometry
/// stays in double precision.
#[inline]
fn dvec3_to_f32_array(v: DVec3) -> [f32; 3] {
    [v.x as f32, v.y as f32, v.z as f32]
}

/// Computed visibility data from the compiler's visibility pass.
pub struct VisibilityResult {
    pub nodes_section: BspNodesSection,
    pub leaves_section: BspLeavesSection,
    pub leaf_pvs_section: LeafPvsSection,
    pub empty_leaf_count: usize,
    pub compressed_pvs_bytes: usize,
}

/// Identify BSP leaves reachable from outside the sealed map volume.
///
/// Mirrors Doom 3 dmap's exterior flood-fill: probe a point just outside the
/// map's AABB to find a void seed leaf, then BFS through the portal graph to
/// every leaf reachable from that seed. Reachable leaves represent open space
/// the player can never legitimately occupy, and their geometry (the back of
/// brush exterior faces) is culled from the packed output.
///
/// Returns an empty set if the probe lands in a solid leaf (brush touches the
/// boundary), so culling becomes a no-op and geometry is unchanged. See the
/// exterior-leaf-culling plan for the design.
pub fn find_exterior_leaves(tree: &BspTree, portals: &[Portal]) -> HashSet<usize> {
    if tree.leaves.is_empty() {
        return HashSet::new();
    }

    // Compute AABB over all leaf bounds. Skip faceless leaves whose bounds are
    // still the empty sentinel (min=+INF, max=-INF) so they don't poison the
    // extent.
    let mut map_min = DVec3::splat(f64::INFINITY);
    let mut map_max = DVec3::splat(f64::NEG_INFINITY);
    let mut had_bounds = false;
    for leaf in &tree.leaves {
        let b = &leaf.bounds;
        if !b.is_valid() {
            continue;
        }
        map_min = map_min.min(b.min);
        map_max = map_max.max(b.max);
        had_bounds = true;
    }

    if !had_bounds {
        return HashSet::new();
    }

    // Probe one unit outside the AABB's maximum corner. Any point outside the
    // sealed volume lands in the exterior region.
    let probe = map_max + DVec3::splat(1.0);
    let seed = partition::find_leaf_for_point(tree, probe);

    if tree.leaves[seed].is_solid {
        log::warn!(
            "[Compiler] WARNING: void probe landed in a solid leaf — exterior leaf culling skipped"
        );
        return HashSet::new();
    }

    // Adjacency: for each leaf index, list of neighbor leaves via portals.
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); tree.leaves.len()];
    for portal in portals {
        adjacency[portal.front_leaf].push(portal.back_leaf);
        adjacency[portal.back_leaf].push(portal.front_leaf);
    }

    let mut exterior: HashSet<usize> = HashSet::new();
    let mut queue: VecDeque<usize> = VecDeque::new();
    exterior.insert(seed);
    queue.push_back(seed);

    while let Some(leaf_idx) = queue.pop_front() {
        for &neighbor in &adjacency[leaf_idx] {
            if tree.leaves[neighbor].is_solid {
                continue;
            }
            if exterior.insert(neighbor) {
                queue.push_back(neighbor);
            }
        }
    }

    let exterior_count = exterior.len();
    let interior_empty_count = tree
        .leaves
        .iter()
        .enumerate()
        .filter(|(idx, leaf)| !leaf.is_solid && !exterior.contains(idx))
        .count();

    log::info!(
        "[Compiler] Exterior flood-fill: {exterior_count} exterior leaves, {interior_empty_count} interior empty leaves"
    );

    if exterior_count > 0 && interior_empty_count == 0 {
        log::warn!(
            "[Compiler] WARNING: no interior empty leaves remain after exterior culling — map may be unsealed or have a leak"
        );
    }

    exterior
}

/// Build per-leaf PVS via portal flood-fill and encode PRL sections.
///
/// 1. Compute per-leaf PVS by BFS through the portal graph.
/// 2. Encode BSP nodes and leaves into their PRL sections.
/// 3. Zero out face counts for any leaf in `exterior_leaves` so exterior
///    geometry is dropped in lockstep with the geometry section.
///
/// The BSP tree and per-leaf PVS data are owned by the caller; portals are
/// generated upstream so the same portal graph can drive both the exterior
/// flood-fill and PVS computation.
pub fn encode_vis(
    tree: &BspTree,
    portals: &[Portal],
    exterior_leaves: &HashSet<usize>,
) -> VisibilityResult {
    let solid: Vec<bool> = tree.leaves.iter().map(|l| l.is_solid).collect();
    let leaf_count = tree.leaves.len();

    let pvs = portal_vis::compute_pvs(portals, leaf_count, &solid);

    encode_bsp_and_pvs(tree, &pvs, exterior_leaves)
}

/// Encode the BSP tree and per-leaf PVS into the new PRL section types.
///
/// PVS bitsets use the full leaf array index (not empty-leaf-only indexing),
/// since BspLeavesSection includes all leaves (solid and empty). Solid leaves
/// have `pvs_offset = 0` and `pvs_size = 0`. Exterior leaves are kept in the
/// leaf array (so BSP child indices remain valid) but are emitted with
/// `face_count = 0` so no geometry references them.
fn encode_bsp_and_pvs(
    tree: &BspTree,
    pvs: &[Vec<bool>],
    exterior_leaves: &HashSet<usize>,
) -> VisibilityResult {
    let nodes_section = encode_nodes(tree);
    let (leaves_section, leaf_pvs_section, empty_leaf_count, compressed_pvs_bytes) =
        encode_leaves_and_pvs(tree, pvs, exterior_leaves);

    VisibilityResult {
        nodes_section,
        leaves_section,
        leaf_pvs_section,
        empty_leaf_count,
        compressed_pvs_bytes,
    }
}

/// Encode BSP interior nodes into the flat node array.
fn encode_nodes(tree: &BspTree) -> BspNodesSection {
    let nodes = tree
        .nodes
        .iter()
        .map(|n| {
            let front = match &n.front {
                BspChild::Node(idx) => *idx as i32,
                BspChild::Leaf(idx) => -1 - (*idx as i32),
            };
            let back = match &n.back {
                BspChild::Node(idx) => *idx as i32,
                BspChild::Leaf(idx) => -1 - (*idx as i32),
            };
            BspNodeRecord {
                plane_normal: dvec3_to_f32_array(n.plane_normal),
                plane_distance: n.plane_distance as f32,
                front,
                back,
            }
        })
        .collect();

    BspNodesSection { nodes }
}

/// Encode BSP leaves and their PVS data.
///
/// Faces are ordered by leaf in the geometry section (all faces for leaf 0 first,
/// then leaf 1, etc.). `face_start` / `face_count` index into that ordering.
/// Only empty leaves contribute faces; solid leaves get face_start=0, face_count=0.
fn encode_leaves_and_pvs(
    tree: &BspTree,
    pvs: &[Vec<bool>],
    exterior_leaves: &HashSet<usize>,
) -> (BspLeavesSection, LeafPvsSection, usize, usize) {
    let leaf_count = tree.leaves.len();
    let pvs_byte_count = leaf_count.div_ceil(8);

    let mut pvs_blob: Vec<u8> = Vec::new();
    let mut leaf_records: Vec<BspLeafRecord> = Vec::with_capacity(leaf_count);
    let mut empty_leaf_count = 0usize;

    // Face cursor tracks the face_start for each empty leaf in the geometry
    // section's leaf-ordered face list.
    let mut face_cursor: u32 = 0;

    for (bsp_leaf_idx, leaf) in tree.leaves.iter().enumerate() {
        let b = &leaf.bounds;

        if leaf.is_solid {
            leaf_records.push(BspLeafRecord {
                face_start: 0,
                face_count: 0,
                bounds_min: dvec3_to_f32_array(b.min),
                bounds_max: dvec3_to_f32_array(b.max),
                pvs_offset: 0,
                pvs_size: 0,
                is_solid: 1,
            });
            continue;
        }

        empty_leaf_count += 1;

        // Build uncompressed PVS bitset using full leaf indices.
        let mut bitset = vec![0u8; pvs_byte_count];
        for (other_idx, &visible) in pvs[bsp_leaf_idx].iter().enumerate() {
            if visible {
                bitset[other_idx / 8] |= 1u8 << (other_idx % 8);
            }
        }

        let compressed = compress_pvs(&bitset);
        let pvs_offset = pvs_blob.len() as u32;
        let pvs_size = compressed.len() as u32;
        pvs_blob.extend_from_slice(&compressed);

        // Exterior leaves are retained in the section so BSP child indices
        // stay valid, but carry zero faces so no geometry references them.
        // Must stay in lockstep with the exterior filter in
        // `geometry::build_leaf_ordered_faces`.
        let face_count = if exterior_leaves.contains(&bsp_leaf_idx) {
            0u32
        } else {
            leaf.face_indices.len() as u32
        };

        leaf_records.push(BspLeafRecord {
            face_start: face_cursor,
            face_count,
            bounds_min: dvec3_to_f32_array(b.min),
            bounds_max: dvec3_to_f32_array(b.max),
            pvs_offset,
            pvs_size,
            is_solid: 0,
        });

        face_cursor += face_count;
    }

    let compressed_pvs_bytes = pvs_blob.len();

    (
        BspLeavesSection {
            leaves: leaf_records,
        },
        LeafPvsSection { pvs_data: pvs_blob },
        empty_leaf_count,
        compressed_pvs_bytes,
    )
}

/// Log visibility statistics.
pub fn log_stats(result: &VisibilityResult, portal_count: usize) {
    if result.empty_leaf_count == 0 {
        log::info!("[Compiler] Visibility: 0 empty leaves, 0 portals");
        return;
    }

    let leaf_count = result.leaves_section.leaves.len();
    let pvs_byte_count = leaf_count.div_ceil(8);
    let mut visible_counts: Vec<usize> = Vec::with_capacity(result.empty_leaf_count);

    for leaf in &result.leaves_section.leaves {
        if leaf.is_solid != 0 {
            continue;
        }

        let compressed = &result.leaf_pvs_section.pvs_data
            [leaf.pvs_offset as usize..(leaf.pvs_offset + leaf.pvs_size) as usize];
        let decompressed =
            postretro_level_format::visibility::decompress_pvs(compressed, pvs_byte_count);

        let mut count = 0usize;
        for byte in decompressed.iter().take(pvs_byte_count) {
            count += byte.count_ones() as usize;
        }
        visible_counts.push(count);
    }

    let min_vis = visible_counts.iter().copied().min().unwrap_or(0);
    let max_vis = visible_counts.iter().copied().max().unwrap_or(0);
    let avg_vis = if visible_counts.is_empty() {
        0.0
    } else {
        visible_counts.iter().sum::<usize>() as f64 / visible_counts.len() as f64
    };

    log::info!(
        "[Compiler] Visibility: {} empty leaves, {} portals, {} bytes compressed PVS",
        result.empty_leaf_count,
        portal_count,
        result.compressed_pvs_bytes,
    );
    log::info!(
        "[Compiler] Visible leaves per leaf: min={min_vis}, max={max_vis}, avg={avg_vis:.1}",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::{Aabb, BspChild, BspLeaf, BspNode, BspTree};
    use glam::DVec3;
    use postretro_level_format::visibility::decompress_pvs;

    fn make_tree(leaves: Vec<(Vec<usize>, bool)>) -> BspTree {
        let bsp_leaves: Vec<BspLeaf> = leaves
            .into_iter()
            .map(|(face_indices, is_solid)| BspLeaf {
                face_indices,
                bounds: Aabb {
                    min: DVec3::ZERO,
                    max: DVec3::splat(64.0),
                },
                is_solid,
            })
            .collect();

        BspTree {
            nodes: Vec::new(),
            leaves: bsp_leaves,
        }
    }

    /// Test helper: run the full portal-vis pipeline with no exterior culling.
    /// Mirrors the pre-split `build_portal_pvs` call shape used by the legacy
    /// visibility round-trip tests.
    fn build_portal_pvs_for_test(tree: &BspTree) -> (VisibilityResult, Vec<Portal>) {
        let generated_portals = crate::portals::generate_portals(tree);
        let exterior = HashSet::new();
        let result = encode_vis(tree, &generated_portals, &exterior);
        (result, generated_portals)
    }

    #[test]
    fn empty_tree_produces_empty_vis() {
        let tree = BspTree {
            nodes: Vec::new(),
            leaves: Vec::new(),
        };
        let (result, portals) = build_portal_pvs_for_test(&tree);
        assert_eq!(result.empty_leaf_count, 0);
        assert!(result.leaves_section.leaves.is_empty());
        assert!(result.leaf_pvs_section.pvs_data.is_empty());
        assert!(portals.is_empty());
    }

    #[test]
    fn all_solid_leaves_produce_empty_pvs_blob() {
        let tree = make_tree(vec![(vec![0], true), (vec![1], true)]);
        let (result, _) = build_portal_pvs_for_test(&tree);
        assert_eq!(result.empty_leaf_count, 0);
        // Solid leaves exist but have no PVS data
        assert_eq!(result.leaves_section.leaves.len(), 2);
        assert!(result.leaf_pvs_section.pvs_data.is_empty());
        for leaf in &result.leaves_section.leaves {
            assert_eq!(leaf.is_solid, 1);
            assert_eq!(leaf.pvs_offset, 0);
            assert_eq!(leaf.pvs_size, 0);
        }
    }

    #[test]
    fn single_empty_leaf_sees_itself() {
        let tree = make_tree(vec![(vec![0, 1], false)]);

        let (result, _) = build_portal_pvs_for_test(&tree);
        assert_eq!(result.empty_leaf_count, 1);
        assert_eq!(result.leaves_section.leaves.len(), 1);
        assert_eq!(result.leaves_section.leaves[0].face_start, 0);
        assert_eq!(result.leaves_section.leaves[0].face_count, 2);

        let pvs_bytes = 1usize.div_ceil(8);
        let leaf = &result.leaves_section.leaves[0];
        let decompressed = decompress_pvs(
            &result.leaf_pvs_section.pvs_data
                [leaf.pvs_offset as usize..(leaf.pvs_offset + leaf.pvs_size) as usize],
            pvs_bytes,
        );
        assert_ne!(decompressed[0] & 1, 0, "leaf 0 should see itself");
    }

    #[test]
    fn bsp_nodes_section_round_trips() {
        let tree = make_tree(vec![(vec![0], false), (vec![1], false), (vec![2], false)]);

        let (result, _) = build_portal_pvs_for_test(&tree);
        let bytes = result.nodes_section.to_bytes();
        let restored = BspNodesSection::from_bytes(&bytes).unwrap();
        assert_eq!(result.nodes_section, restored);
    }

    #[test]
    fn bsp_leaves_section_round_trips() {
        let tree = make_tree(vec![(vec![0], false), (vec![1], false), (vec![2], false)]);

        let (result, _) = build_portal_pvs_for_test(&tree);
        let bytes = result.leaves_section.to_bytes();
        let restored = BspLeavesSection::from_bytes(&bytes).unwrap();
        assert_eq!(result.leaves_section, restored);
    }

    #[test]
    fn leaf_pvs_section_round_trips() {
        let tree = make_tree(vec![(vec![0], false), (vec![1], false), (vec![2], false)]);

        let (result, _) = build_portal_pvs_for_test(&tree);
        let bytes = result.leaf_pvs_section.to_bytes();
        let restored = LeafPvsSection::from_bytes(&bytes).unwrap();
        assert_eq!(result.leaf_pvs_section, restored);
    }

    // -- Exterior leaf culling --

    /// Build a BSP tree with a single splitting plane on X=0. The front child
    /// is the "interior" leaf and the back child is the "exterior" void leaf.
    /// Leaf bounds are set so the combined AABB max corner is strictly inside
    /// the void leaf's half-space, so `find_exterior_leaves` probes the back
    /// leaf. A pair of portals models the sealed/unsealed topology.
    fn two_leaf_tree(interior_solid: bool, exterior_solid: bool) -> BspTree {
        // interior leaf (index 0) occupies -10..0 in X, the "inside" of the map.
        // exterior leaf (index 1) occupies 0..10 in X, the void.
        // The probe lands at max + (1,1,1) = (11, 11, 11), which is strictly
        // on the positive X side of the plane, so find_leaf_for_point walks
        // to the exterior child.
        let interior = BspLeaf {
            face_indices: vec![0],
            bounds: Aabb {
                min: DVec3::new(-10.0, 0.0, 0.0),
                max: DVec3::new(0.0, 10.0, 10.0),
            },
            is_solid: interior_solid,
        };
        let exterior = BspLeaf {
            face_indices: vec![],
            bounds: Aabb {
                min: DVec3::new(0.0, 0.0, 0.0),
                max: DVec3::new(10.0, 10.0, 10.0),
            },
            is_solid: exterior_solid,
        };

        let root = BspNode {
            plane_normal: DVec3::X,
            plane_distance: 0.0,
            // Back of plane (negative X) is interior leaf; front of plane is
            // exterior leaf. The probe at +11 X lands in the exterior leaf.
            front: BspChild::Leaf(1),
            back: BspChild::Leaf(0),
            parent: None,
        };

        BspTree {
            nodes: vec![root],
            leaves: vec![interior, exterior],
        }
    }

    #[test]
    fn find_exterior_leaves_normal_sealed_map_classifies_void_leaf() {
        let tree = two_leaf_tree(false, false);
        // No portals: interior is sealed off from exterior. Only the void
        // seed leaf itself should be marked exterior.
        let portals: Vec<Portal> = Vec::new();
        let exterior = find_exterior_leaves(&tree, &portals);
        assert_eq!(exterior.len(), 1);
        assert!(exterior.contains(&1), "exterior void leaf should be marked");
        assert!(
            !exterior.contains(&0),
            "sealed interior leaf should not be marked exterior"
        );
    }

    #[test]
    fn find_exterior_leaves_flood_fills_through_portals() {
        // A portal connecting the interior and exterior leaves means the
        // interior is not sealed: the flood-fill reaches it from the void.
        let tree = two_leaf_tree(false, false);
        let leak_portal = Portal {
            polygon: Vec::new(),
            front_leaf: 1,
            back_leaf: 0,
        };
        let exterior = find_exterior_leaves(&tree, &[leak_portal]);
        assert_eq!(exterior.len(), 2);
        assert!(exterior.contains(&0));
        assert!(exterior.contains(&1));
    }

    #[test]
    fn find_exterior_leaves_solid_seed_guard_returns_empty_set() {
        // The probe lands in a solid leaf (brush touches the map boundary).
        // Culling is skipped; the returned set is empty and the compiler keeps
        // all geometry unchanged.
        let tree = two_leaf_tree(false, true);
        let exterior = find_exterior_leaves(&tree, &[]);
        assert!(
            exterior.is_empty(),
            "solid seed guard should return empty set, got {exterior:?}"
        );
    }

    #[test]
    fn find_exterior_leaves_zero_portal_edge_case() {
        // With no portals at all, BFS from the seed cannot spread. Only the
        // seed itself is classified exterior.
        let tree = two_leaf_tree(false, false);
        let exterior = find_exterior_leaves(&tree, &[]);
        assert_eq!(exterior.len(), 1);
        assert!(exterior.contains(&1));
    }

    #[test]
    fn find_exterior_leaves_empty_tree_returns_empty() {
        let tree = BspTree {
            nodes: Vec::new(),
            leaves: Vec::new(),
        };
        let exterior = find_exterior_leaves(&tree, &[]);
        assert!(exterior.is_empty());
    }

    #[test]
    fn exterior_leaves_get_zero_face_count_in_encoded_section() {
        // Verify acceptance criterion 6: BspLeafRecord.face_count == 0 for
        // every exterior leaf in the packed output.
        let tree = two_leaf_tree(false, false);
        let mut exterior = HashSet::new();
        exterior.insert(1usize);

        // Give the exterior leaf a face index to prove it gets zeroed out.
        let mut tree = tree;
        tree.leaves[1].face_indices = vec![42];

        let result = encode_vis(&tree, &[], &exterior);
        assert_eq!(result.leaves_section.leaves.len(), 2);
        let ext_record = &result.leaves_section.leaves[1];
        assert_eq!(
            ext_record.face_count, 0,
            "exterior leaf should carry zero faces regardless of BSP face_indices"
        );
        // Interior leaf keeps its one face.
        assert_eq!(result.leaves_section.leaves[0].face_count, 1);
    }

    // -- Full-pipeline sealed box diagnostic --

    /// Construct six faces of an axis-aligned box with outward-facing normals.
    fn box_faces(min: DVec3, max: DVec3) -> Vec<crate::map_data::Face> {
        use crate::map_data::Face;
        let tex = "test".to_string();
        let mk = |verts: Vec<DVec3>, normal: DVec3, distance: f64| Face {
            vertices: verts,
            normal,
            distance,
            texture: tex.clone(),
            tex_projection: Default::default(),
            brush_index: 0,
        };
        vec![
            mk(
                vec![
                    DVec3::new(min.x, min.y, min.z),
                    DVec3::new(min.x, max.y, min.z),
                    DVec3::new(min.x, max.y, max.z),
                    DVec3::new(min.x, min.y, max.z),
                ],
                DVec3::NEG_X,
                -min.x,
            ),
            mk(
                vec![
                    DVec3::new(max.x, min.y, min.z),
                    DVec3::new(max.x, min.y, max.z),
                    DVec3::new(max.x, max.y, max.z),
                    DVec3::new(max.x, max.y, min.z),
                ],
                DVec3::X,
                max.x,
            ),
            mk(
                vec![
                    DVec3::new(min.x, min.y, min.z),
                    DVec3::new(min.x, min.y, max.z),
                    DVec3::new(max.x, min.y, max.z),
                    DVec3::new(max.x, min.y, min.z),
                ],
                DVec3::NEG_Y,
                -min.y,
            ),
            mk(
                vec![
                    DVec3::new(min.x, max.y, min.z),
                    DVec3::new(max.x, max.y, min.z),
                    DVec3::new(max.x, max.y, max.z),
                    DVec3::new(min.x, max.y, max.z),
                ],
                DVec3::Y,
                max.y,
            ),
            mk(
                vec![
                    DVec3::new(min.x, min.y, min.z),
                    DVec3::new(max.x, min.y, min.z),
                    DVec3::new(max.x, max.y, min.z),
                    DVec3::new(min.x, max.y, min.z),
                ],
                DVec3::NEG_Z,
                -min.z,
            ),
            mk(
                vec![
                    DVec3::new(min.x, min.y, max.z),
                    DVec3::new(max.x, min.y, max.z),
                    DVec3::new(max.x, max.y, max.z),
                    DVec3::new(min.x, max.y, max.z),
                ],
                DVec3::Z,
                max.z,
            ),
        ]
    }

    /// Construct a BrushVolume for an axis-aligned box.
    fn box_volume(min: DVec3, max: DVec3) -> crate::map_data::BrushVolume {
        use crate::map_data::{BrushPlane, BrushVolume};
        BrushVolume {
            planes: vec![
                BrushPlane {
                    normal: DVec3::X,
                    distance: max.x,
                },
                BrushPlane {
                    normal: DVec3::NEG_X,
                    distance: -min.x,
                },
                BrushPlane {
                    normal: DVec3::Y,
                    distance: max.y,
                },
                BrushPlane {
                    normal: DVec3::NEG_Y,
                    distance: -min.y,
                },
                BrushPlane {
                    normal: DVec3::Z,
                    distance: max.z,
                },
                BrushPlane {
                    normal: DVec3::NEG_Z,
                    distance: -min.z,
                },
            ],
            aabb: Aabb { min, max },
        }
    }

    /// Build a sealed room as six wall slabs around an interior air pocket.
    /// Outer extent: -60..60 on each axis. Interior air: -50..50 on each axis.
    /// Wall thickness: 10.
    fn sealed_box() -> (Vec<crate::map_data::Face>, Vec<crate::map_data::BrushVolume>) {
        let wall_slabs = [
            // -X wall
            (DVec3::new(-60.0, -60.0, -60.0), DVec3::new(-50.0, 60.0, 60.0)),
            // +X wall
            (DVec3::new(50.0, -60.0, -60.0), DVec3::new(60.0, 60.0, 60.0)),
            // -Y wall (floor)
            (DVec3::new(-60.0, -60.0, -60.0), DVec3::new(60.0, -50.0, 60.0)),
            // +Y wall (ceiling)
            (DVec3::new(-60.0, 50.0, -60.0), DVec3::new(60.0, 60.0, 60.0)),
            // -Z wall
            (DVec3::new(-60.0, -60.0, -60.0), DVec3::new(60.0, 60.0, -50.0)),
            // +Z wall
            (DVec3::new(-60.0, -60.0, 50.0), DVec3::new(60.0, 60.0, 60.0)),
        ];

        let mut faces = Vec::new();
        let mut brushes = Vec::new();
        for (min, max) in wall_slabs {
            faces.extend(box_faces(min, max));
            brushes.push(box_volume(min, max));
        }
        (faces, brushes)
    }

    #[test]
    fn sealed_box_center_point_is_not_classified_exterior() {
        // Diagnostic test for the "leak or false positive?" question.
        //
        // A hand-crafted sealed box (six wall brushes around an interior air
        // pocket) is, by construction, sealed. There is no hole to the void.
        // The leaf containing the center point (0, 0, 0) must therefore be
        // interior, not exterior, after running the real compiler pipeline
        // (CSG clip, BSP partition + classify_leaf_solidity, portal
        // generation, exterior flood-fill).
        //
        // If this assertion fails, the flood-fill is walking through leaves
        // that should be solid barriers — i.e., `classify_leaf_solidity` is
        // failing to mark any leaf as solid on a definitionally-sealed map.
        // That's a false-positive leak detection, not a real leak.
        let (faces, brushes) = sealed_box();

        let clipped = crate::csg::csg_clip_faces(&faces, &brushes);
        let result = crate::partition::partition(clipped, &brushes)
            .expect("partition should succeed on sealed box");
        let generated_portals = crate::portals::generate_portals(&result.tree);
        let exterior = find_exterior_leaves(&result.tree, &generated_portals);

        let center_leaf = crate::partition::find_leaf_for_point(&result.tree, DVec3::ZERO);

        let solid_count = result.tree.leaves.iter().filter(|l| l.is_solid).count();
        let empty_count = result.tree.leaves.len() - solid_count;

        assert!(
            !exterior.contains(&center_leaf),
            "center point (0,0,0) of a sealed box was classified as exterior. \
             Leaf index: {center_leaf}, total leaves: {}, solid: {solid_count}, \
             empty: {empty_count}, exterior: {}, portals: {}. \
             The box is definitionally sealed, so this is a false-positive leak \
             detection — the solidity classifier is not marking wall leaves solid.",
            result.tree.leaves.len(),
            exterior.len(),
            generated_portals.len(),
        );
    }

    #[test]
    fn exterior_leaf_face_ranges_stay_in_lockstep_with_geometry_filter() {
        // Integration check for acceptance criterion 7: BspLeavesSection face
        // ranges and what the geometry pipeline would produce agree when the
        // exterior set is threaded through both.
        //
        // Two interior leaves and one exterior leaf. The exterior leaf sits
        // between them in the tree; its faces should not consume any space in
        // the face cursor, so the third leaf's face_start equals the first
        // leaf's face_count.
        let tree = BspTree {
            nodes: Vec::new(),
            leaves: vec![
                BspLeaf {
                    face_indices: vec![0, 1],
                    bounds: Aabb {
                        min: DVec3::ZERO,
                        max: DVec3::splat(10.0),
                    },
                    is_solid: false,
                },
                BspLeaf {
                    face_indices: vec![2, 3],
                    bounds: Aabb {
                        min: DVec3::ZERO,
                        max: DVec3::splat(10.0),
                    },
                    is_solid: false,
                },
                BspLeaf {
                    face_indices: vec![4, 5],
                    bounds: Aabb {
                        min: DVec3::ZERO,
                        max: DVec3::splat(10.0),
                    },
                    is_solid: false,
                },
            ],
        };

        let mut exterior = HashSet::new();
        exterior.insert(1usize);

        let result = encode_vis(&tree, &[], &exterior);

        assert_eq!(result.leaves_section.leaves[0].face_start, 0);
        assert_eq!(result.leaves_section.leaves[0].face_count, 2);
        // Exterior leaf contributes zero, but still has a face_start at the
        // current cursor position (which is whatever the previous cumulative
        // count was).
        assert_eq!(result.leaves_section.leaves[1].face_count, 0);
        assert_eq!(result.leaves_section.leaves[1].face_start, 2);
        // The third leaf picks up right where the first left off — the
        // exterior leaf did not consume any face-range slots.
        assert_eq!(result.leaves_section.leaves[2].face_start, 2);
        assert_eq!(result.leaves_section.leaves[2].face_count, 2);
    }
}
