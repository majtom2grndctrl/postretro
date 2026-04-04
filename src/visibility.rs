// PVS-based visibility culling: point-in-leaf, PVS decompression, visible face collection.
// See: context/plans/phase_1/task_04_pvs_culling.md

use glam::Vec3;

use crate::bsp::BspWorld;

/// A draw range referencing a contiguous run of indices in the shared index buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawRange {
    pub index_offset: u32,
    pub index_count: u32,
}

/// Result of per-frame visibility determination.
#[derive(Debug)]
pub enum VisibleFaces {
    /// PVS data is available; draw only these face ranges.
    Culled(Vec<DrawRange>),
    /// No PVS data; draw everything.
    DrawAll,
}

/// Walk the BSP node tree to find which leaf contains the given point.
///
/// Returns the leaf index into `bsp_world.leaves`. At each internal node, tests the
/// point against the split plane and descends into the appropriate child.
pub fn find_camera_leaf(position: Vec3, world: &BspWorld) -> u32 {
    // Start at the root node.
    let mut node_idx = world.root_node;
    let mut is_leaf = false;

    loop {
        if is_leaf {
            return node_idx;
        }

        let node = &world.nodes[node_idx as usize];

        // Plane test: dot(normal, point) - dist.
        // The normal has been transformed to engine Y-up via quake_to_engine (an orthonormal
        // rotation), and the camera position is already in engine coordinates. Since the
        // transform preserves distances, the original dist value is still valid.
        let side = node.plane_normal.dot(position) - node.plane_dist;

        if side >= 0.0 {
            node_idx = node.front;
            is_leaf = node.front_is_leaf;
        } else {
            node_idx = node.back;
            is_leaf = node.back_is_leaf;
        }
    }
}

/// Decompress the PVS bitfield for a leaf using Quake's standard RLE format.
///
/// Returns a `Vec<bool>` indexed by leaf index, where `true` means the leaf is potentially
/// visible. Returns `None` if the leaf has no PVS data (visdata_offset is negative or
/// visdata is empty).
///
/// The RLE format: read bytes from `visdata[offset..]`.
/// - Non-zero byte: 8 raw visibility bits (LSB = lowest leaf index in this group).
/// - Zero byte: the next byte is the count of zero bytes to expand (run-length encoding
///   of groups of 8 invisible leaves).
///
/// Leaf 0 is always the "invalid" / out-of-bounds leaf in Quake BSP, so the bitfield
/// starts counting at leaf 1.
pub fn decompress_pvs(leaf_index: u32, world: &BspWorld) -> Option<Vec<bool>> {
    let leaf = world.leaves.get(leaf_index as usize)?;

    if leaf.visdata_offset < 0 || world.visdata.is_empty() {
        return None;
    }

    let offset = leaf.visdata_offset as usize;
    if offset >= world.visdata.len() {
        return None;
    }

    let num_leaves = world.leaves.len();
    // The bitfield covers leaves 1..num_leaves. We need ceil(num_leaves / 8) bytes
    // of decompressed data, but we index by leaf_index directly, so allocate num_leaves.
    let mut visible = vec![false; num_leaves];

    // Leaf 0 is the out-of-bounds sentinel; PVS bits start at leaf 1.
    let mut leaf_bit = 1usize;
    let data = &world.visdata[offset..];
    let mut pos = 0;

    while leaf_bit < num_leaves && pos < data.len() {
        let byte = data[pos];
        pos += 1;

        if byte == 0 {
            // RLE: next byte is the count of zero bytes to expand.
            if pos >= data.len() {
                break;
            }
            let count = data[pos] as usize;
            pos += 1;
            leaf_bit += 8 * count;
        } else {
            // Raw visibility byte: 8 bits, LSB first.
            for bit in 0..8 {
                if leaf_bit >= num_leaves {
                    break;
                }
                if byte & (1 << bit) != 0 {
                    visible[leaf_bit] = true;
                }
                leaf_bit += 1;
            }
        }
    }

    Some(visible)
}

/// Collect draw ranges for all faces belonging to visible leaves.
///
/// Given a visibility bitfield (from `decompress_pvs`), iterates visible leaves and
/// gathers their face draw ranges. The camera's own leaf is always included.
pub fn collect_visible_faces(
    visible_leaves: &[bool],
    camera_leaf: u32,
    world: &BspWorld,
) -> Vec<DrawRange> {
    let mut ranges = Vec::new();

    for (leaf_idx, leaf) in world.leaves.iter().enumerate() {
        // Include this leaf if it's marked visible in the PVS or it's the camera's leaf.
        let is_visible = visible_leaves
            .get(leaf_idx)
            .copied()
            .unwrap_or(false);
        let is_camera_leaf = leaf_idx as u32 == camera_leaf;

        if !is_visible && !is_camera_leaf {
            continue;
        }

        for &face_idx in &leaf.face_indices {
            if let Some(face) = world.face_meta.get(face_idx as usize) {
                if face.index_count > 0 {
                    ranges.push(DrawRange {
                        index_offset: face.index_offset,
                        index_count: face.index_count,
                    });
                }
            }
        }
    }

    ranges
}

/// Perform full visibility determination for a single frame.
///
/// Returns `VisibleFaces::Culled` with draw ranges when PVS data is available,
/// or `VisibleFaces::DrawAll` when it is not.
pub fn determine_visibility(camera_position: Vec3, world: &BspWorld) -> VisibleFaces {
    if world.nodes.is_empty() || world.leaves.is_empty() {
        return VisibleFaces::DrawAll;
    }

    let camera_leaf = find_camera_leaf(camera_position, world);

    match decompress_pvs(camera_leaf, world) {
        Some(visible_leaves) => {
            let ranges = collect_visible_faces(&visible_leaves, camera_leaf, world);
            log::trace!(
                "[Visibility] leaf={}, visible_ranges={}, total_faces={}",
                camera_leaf,
                ranges.len(),
                world.face_meta.len(),
            );
            VisibleFaces::Culled(ranges)
        }
        None => {
            log::trace!(
                "[Visibility] leaf={}, no PVS data — drawing all faces",
                camera_leaf,
            );
            VisibleFaces::DrawAll
        }
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bsp::{BspLeafData, BspNodeData, BspWorld, FaceMeta};

    // -- Helper: build a minimal BspWorld --

    fn empty_world() -> BspWorld {
        BspWorld {
            vertices: Vec::new(),
            indices: Vec::new(),
            face_meta: Vec::new(),
            nodes: Vec::new(),
            leaves: Vec::new(),
            visdata: Vec::new(),
            root_node: 0,
        }
    }

    /// Build a simple two-leaf BSP: one node splits space at X=0.
    /// Front (X >= 0) goes to leaf 1, back (X < 0) goes to leaf 2.
    /// Leaf 0 is the out-of-bounds sentinel.
    fn two_leaf_world() -> BspWorld {
        let nodes = vec![BspNodeData {
            plane_normal: Vec3::X,
            plane_dist: 0.0,
            front: 1,      // leaf 1
            front_is_leaf: true,
            back: 2,        // leaf 2
            back_is_leaf: true,
        }];

        let leaves = vec![
            // Leaf 0: out-of-bounds sentinel
            BspLeafData {
                mins: Vec3::ZERO,
                maxs: Vec3::ZERO,
                face_indices: Vec::new(),
                visdata_offset: -1,
            },
            // Leaf 1: front half, contains face 0
            BspLeafData {
                mins: Vec3::new(0.0, -100.0, -100.0),
                maxs: Vec3::new(100.0, 100.0, 100.0),
                face_indices: vec![0],
                visdata_offset: 0,
            },
            // Leaf 2: back half, contains face 1
            BspLeafData {
                mins: Vec3::new(-100.0, -100.0, -100.0),
                maxs: Vec3::new(0.0, 100.0, 100.0),
                face_indices: vec![1],
                visdata_offset: 1,
            },
        ];

        let face_meta = vec![
            FaceMeta {
                index_offset: 0,
                index_count: 3,
                leaf_index: 1,
            },
            FaceMeta {
                index_offset: 3,
                index_count: 6,
                leaf_index: 2,
            },
        ];

        // Visdata: leaf 1 can see leaf 2 and vice versa.
        // Bit layout for 3 leaves (leaf 0 excluded from bitfield):
        // Byte at offset 0 (leaf 1's PVS): bit 0 = leaf 1, bit 1 = leaf 2 -> 0b11 = 3
        // Byte at offset 1 (leaf 2's PVS): bit 0 = leaf 1, bit 1 = leaf 2 -> 0b11 = 3
        let visdata = vec![0b0000_0011, 0b0000_0011];

        BspWorld {
            vertices: vec![[0.0; 3]; 6],
            indices: vec![0, 1, 2, 3, 4, 5, 3, 5, 6],
            face_meta,
            nodes,
            leaves,
            visdata,
            root_node: 0,
        }
    }

    // -- Point-in-leaf tests --

    #[test]
    fn point_in_leaf_front_side() {
        let world = two_leaf_world();
        let leaf = find_camera_leaf(Vec3::new(10.0, 0.0, 0.0), &world);
        assert_eq!(leaf, 1, "point on positive X side should be in leaf 1");
    }

    #[test]
    fn point_in_leaf_back_side() {
        let world = two_leaf_world();
        let leaf = find_camera_leaf(Vec3::new(-10.0, 0.0, 0.0), &world);
        assert_eq!(leaf, 2, "point on negative X side should be in leaf 2");
    }

    #[test]
    fn point_in_leaf_on_plane_goes_front() {
        let world = two_leaf_world();
        // Exactly on the plane (dot = 0.0 >= 0.0) should go to front.
        let leaf = find_camera_leaf(Vec3::ZERO, &world);
        assert_eq!(leaf, 1, "point on plane should go to front child (leaf 1)");
    }

    #[test]
    fn point_in_leaf_deep_tree() {
        // Build a 3-level tree: root splits on X=0, front splits on Y=0.
        let nodes = vec![
            // Node 0: split on X=0
            BspNodeData {
                plane_normal: Vec3::X,
                plane_dist: 0.0,
                front: 1,           // node 1
                front_is_leaf: false,
                back: 1,            // leaf 1
                back_is_leaf: true,
            },
            // Node 1: split on Y=0
            BspNodeData {
                plane_normal: Vec3::Y,
                plane_dist: 0.0,
                front: 2,           // leaf 2
                front_is_leaf: true,
                back: 3,            // leaf 3
                back_is_leaf: true,
            },
        ];

        let leaves = vec![
            BspLeafData {
                mins: Vec3::ZERO,
                maxs: Vec3::ZERO,
                face_indices: Vec::new(),
                visdata_offset: -1,
            },
            BspLeafData {
                mins: Vec3::splat(-100.0),
                maxs: Vec3::new(0.0, 100.0, 100.0),
                face_indices: Vec::new(),
                visdata_offset: -1,
            },
            BspLeafData {
                mins: Vec3::new(0.0, 0.0, -100.0),
                maxs: Vec3::splat(100.0),
                face_indices: Vec::new(),
                visdata_offset: -1,
            },
            BspLeafData {
                mins: Vec3::new(0.0, -100.0, -100.0),
                maxs: Vec3::new(100.0, 0.0, 100.0),
                face_indices: Vec::new(),
                visdata_offset: -1,
            },
        ];

        let world = BspWorld {
            vertices: Vec::new(),
            indices: Vec::new(),
            face_meta: Vec::new(),
            nodes,
            leaves,
            visdata: Vec::new(),
            root_node: 0,
        };

        // X < 0 -> leaf 1
        assert_eq!(find_camera_leaf(Vec3::new(-5.0, 0.0, 0.0), &world), 1);
        // X > 0, Y > 0 -> leaf 2
        assert_eq!(find_camera_leaf(Vec3::new(5.0, 5.0, 0.0), &world), 2);
        // X > 0, Y < 0 -> leaf 3
        assert_eq!(find_camera_leaf(Vec3::new(5.0, -5.0, 0.0), &world), 3);
    }

    // -- PVS decompression tests --

    #[test]
    fn decompress_pvs_simple_raw_byte() {
        // 3 leaves total. Visdata byte 0b11 means leaves 1 and 2 are visible.
        let world = two_leaf_world();
        let visible = decompress_pvs(1, &world).expect("should have PVS");
        assert_eq!(visible.len(), 3);
        assert!(!visible[0], "leaf 0 (sentinel) should never be visible");
        assert!(visible[1], "leaf 1 should be visible");
        assert!(visible[2], "leaf 2 should be visible");
    }

    #[test]
    fn decompress_pvs_rle_zeros() {
        // Test RLE: 0x00, count=2 skips 16 leaves, then 0b0000_0001 marks the next leaf.
        // Leaves: 0 (sentinel), 1..16 (skipped by RLE), 17 (visible), 18..24 (not visible).
        let visdata = vec![0x00, 0x02, 0b0000_0001];

        let mut world = empty_world();
        world.leaves = (0..25)
            .map(|i| BspLeafData {
                mins: Vec3::ZERO,
                maxs: Vec3::ZERO,
                face_indices: Vec::new(),
                visdata_offset: if i == 1 { 0 } else { -1 },
            })
            .collect();
        world.visdata = visdata;

        let visible = decompress_pvs(1, &world).expect("should have PVS");
        assert_eq!(visible.len(), 25);

        // Leaves 1..16 should be invisible (RLE skip).
        for i in 1..=16 {
            assert!(!visible[i], "leaf {i} should be invisible (RLE skip)");
        }
        // Leaf 17 should be visible.
        assert!(visible[17], "leaf 17 should be visible");
        // Leaves 18..24 should be invisible.
        for i in 18..25 {
            assert!(!visible[i], "leaf {i} should be invisible");
        }
    }

    #[test]
    fn decompress_pvs_negative_offset_returns_none() {
        let mut world = empty_world();
        world.leaves.push(BspLeafData {
            mins: Vec3::ZERO,
            maxs: Vec3::ZERO,
            face_indices: Vec::new(),
            visdata_offset: -1,
        });
        assert!(decompress_pvs(0, &world).is_none());
    }

    #[test]
    fn decompress_pvs_empty_visdata_returns_none() {
        let mut world = empty_world();
        world.leaves.push(BspLeafData {
            mins: Vec3::ZERO,
            maxs: Vec3::ZERO,
            face_indices: Vec::new(),
            visdata_offset: 0,
        });
        // visdata is empty
        assert!(decompress_pvs(0, &world).is_none());
    }

    #[test]
    fn decompress_pvs_out_of_bounds_leaf_returns_none() {
        let world = empty_world();
        assert!(decompress_pvs(999, &world).is_none());
    }

    #[test]
    fn decompress_pvs_matches_qbsp_reference() {
        // Use the same test data as qbsp's own test suite.
        // TEST_VISDATA: [0b1010_0111, 0, 5, 0b0000_0001, 0b0001_0000, 0, 12, 0b1000_0000]
        // Expected visible leaves: 1, 2, 3, 6, 8, 49, 61, 168
        let visdata = vec![0b1010_0111, 0, 5, 0b0000_0001, 0b0001_0000, 0, 12, 0b1000_0000];

        let mut world = empty_world();
        world.leaves = (0..256)
            .map(|i| BspLeafData {
                mins: Vec3::ZERO,
                maxs: Vec3::ZERO,
                face_indices: Vec::new(),
                visdata_offset: if i == 1 { 0 } else { -1 },
            })
            .collect();
        world.visdata = visdata;

        let visible = decompress_pvs(1, &world).expect("should have PVS");

        let visible_indices: Vec<usize> = visible
            .iter()
            .enumerate()
            .filter(|(_, v)| **v)
            .map(|(i, _)| i)
            .collect();

        assert_eq!(
            visible_indices,
            vec![1, 2, 3, 6, 8, 49, 61, 168],
            "should match qbsp reference test data"
        );
    }

    // -- Visible face collection tests --

    #[test]
    fn collect_faces_from_visible_leaves() {
        let world = two_leaf_world();
        // Both leaves visible.
        let visible = vec![false, true, true];
        let ranges = collect_visible_faces(&visible, 1, &world);

        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0], DrawRange { index_offset: 0, index_count: 3 });
        assert_eq!(ranges[1], DrawRange { index_offset: 3, index_count: 6 });
    }

    #[test]
    fn collect_faces_only_camera_leaf_when_others_invisible() {
        let world = two_leaf_world();
        // Only leaf 1 is visible (PVS says nothing else visible).
        let visible = vec![false, true, false];
        let ranges = collect_visible_faces(&visible, 1, &world);

        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], DrawRange { index_offset: 0, index_count: 3 });
    }

    #[test]
    fn collect_faces_camera_leaf_always_included() {
        let world = two_leaf_world();
        // PVS says nothing visible at all — but camera leaf is always included.
        let visible = vec![false, false, false];
        let ranges = collect_visible_faces(&visible, 1, &world);

        assert_eq!(ranges.len(), 1, "camera leaf should always be included");
        assert_eq!(ranges[0], DrawRange { index_offset: 0, index_count: 3 });
    }

    #[test]
    fn collect_faces_empty_world() {
        let world = empty_world();
        let visible: Vec<bool> = Vec::new();
        let ranges = collect_visible_faces(&visible, 0, &world);
        assert!(ranges.is_empty());
    }

    // -- determine_visibility integration tests --

    #[test]
    fn determine_visibility_with_pvs() {
        let world = two_leaf_world();
        let result = determine_visibility(Vec3::new(10.0, 0.0, 0.0), &world);
        match result {
            VisibleFaces::Culled(ranges) => {
                assert!(!ranges.is_empty(), "should have draw ranges");
            }
            VisibleFaces::DrawAll => panic!("expected Culled, got DrawAll"),
        }
    }

    #[test]
    fn determine_visibility_without_pvs_draws_all() {
        let mut world = two_leaf_world();
        world.visdata.clear();
        let result = determine_visibility(Vec3::new(10.0, 0.0, 0.0), &world);
        assert!(
            matches!(result, VisibleFaces::DrawAll),
            "should draw all when visdata is empty"
        );
    }

    #[test]
    fn determine_visibility_empty_world_draws_all() {
        let world = empty_world();
        let result = determine_visibility(Vec3::ZERO, &world);
        assert!(matches!(result, VisibleFaces::DrawAll));
    }
}
