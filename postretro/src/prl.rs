// PRL level loading: read .prl files, produce BSP tree + leaf-based engine data structures.
// See: context/lib/build_pipeline.md §PRL

use std::path::Path;

use glam::Vec3;
use postretro_level_format::bsp::{BspLeavesSection, BspNodesSection};
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::leaf_pvs::LeafPvsSection;
use postretro_level_format::portals::PortalsSection;
use postretro_level_format::visibility::decompress_pvs;
use postretro_level_format::{self as prl_format, SectionId};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PrlLoadError {
    #[error("PRL file not found: {0}")]
    FileNotFound(String),
    #[error("failed to read PRL file: {0}")]
    IoError(#[from] std::io::Error),
    #[error("PRL format error: {0}")]
    FormatError(#[from] prl_format::FormatError),
    #[error("PRL file has no geometry section")]
    NoGeometry,
}

/// Per-face draw metadata for PRL levels.
#[derive(Debug, Clone)]
pub struct FaceMeta {
    pub index_offset: u32,
    pub index_count: u32,
}

/// A BSP tree child reference: either an interior node or a leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BspChild {
    Node(usize),
    Leaf(usize),
}

/// BSP interior node: splitting plane + front/back children.
#[derive(Debug, Clone)]
pub struct NodeData {
    pub plane_normal: Vec3,
    pub plane_distance: f32,
    pub front: BspChild,
    pub back: BspChild,
}

/// BSP leaf: contains face range, bounds, PVS, and solid flag.
#[derive(Debug, Clone)]
pub struct LeafData {
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    pub face_start: u32,
    pub face_count: u32,
    /// Decompressed PVS: pvs[i] = leaf i is visible from this leaf.
    pub pvs: Vec<bool>,
    pub is_solid: bool,
}

/// A portal connecting two adjacent BSP leaves, loaded from the Portals section.
#[derive(Debug, Clone)]
pub struct PortalData {
    /// Convex polygon vertices in world space.
    pub polygon: Vec<Vec3>,
    pub front_leaf: usize,
    pub back_leaf: usize,
}

/// BSP tree + leaf-based level data loaded from a .prl file.
#[derive(Debug)]
pub struct LevelWorld {
    pub vertices: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub face_meta: Vec<FaceMeta>,
    pub leaves: Vec<LeafData>,
    pub nodes: Vec<NodeData>,
    /// Root of the BSP tree. For a single-leaf tree (no nodes), this is BspChild::Leaf(0).
    pub root: BspChild,
    /// Whether PVS data was present in the file.
    pub has_pvs: bool,
    /// Portal polygons loaded from the Portals section.
    pub portals: Vec<PortalData>,
    /// Portal indices per leaf (adjacency list). `leaf_portals[i]` lists all
    /// portal indices touching leaf `i`.
    pub leaf_portals: Vec<Vec<usize>>,
    /// Whether portal data was present in the file.
    pub has_portals: bool,
}

impl LevelWorld {
    /// Find which BSP leaf contains the given position via BSP tree descent.
    ///
    /// At each node, tests the position against the splitting plane and descends
    /// into the appropriate child. Returns the leaf index.
    ///
    /// Fallback behavior:
    /// - If position is on the plane (within epsilon), chooses front.
    /// - If the tree is empty (no nodes), returns leaf 0.
    pub fn find_leaf(&self, position: Vec3) -> usize {
        let mut current = self.root;

        loop {
            match current {
                BspChild::Leaf(leaf_idx) => return leaf_idx,
                BspChild::Node(node_idx) => {
                    let node = &self.nodes[node_idx];
                    let side = node.plane_normal.dot(position) - node.plane_distance;
                    // side >= 0.0 means front (on-plane chooses front).
                    if side >= 0.0 {
                        current = node.front;
                    } else {
                        current = node.back;
                    }
                }
            }
        }
    }

    /// Compute a reasonable spawn position: center of the level's geometry bounds.
    pub fn spawn_position(&self) -> Vec3 {
        let mut mins = Vec3::splat(f32::MAX);
        let mut maxs = Vec3::splat(f32::MIN);
        for leaf in &self.leaves {
            if leaf.is_solid || leaf.face_count == 0 {
                continue;
            }
            mins = mins.min(leaf.bounds_min);
            maxs = maxs.max(leaf.bounds_max);
        }
        (mins + maxs) * 0.5
    }
}

/// Build a per-face leaf index mapping from leaf face ranges.
///
/// Returns a Vec where entry `i` is the leaf index that face `i` belongs to.
/// Used by the renderer to assign per-leaf wireframe colors.
#[allow(dead_code)]
pub fn face_leaf_indices(world: &LevelWorld) -> Vec<u32> {
    let mut indices = vec![0u32; world.face_meta.len()];
    for (leaf_idx, leaf) in world.leaves.iter().enumerate() {
        let start = leaf.face_start as usize;
        let count = leaf.face_count as usize;
        for face_idx in start..start + count {
            if let Some(slot) = indices.get_mut(face_idx) {
                *slot = leaf_idx as u32;
            }
        }
    }
    indices
}

/// Decode a PRL sentinel-encoded child reference.
///
/// Positive values are node indices; negative values encode leaves as `(-1 - leaf_index)`.
fn decode_child(value: i32) -> BspChild {
    if value >= 0 {
        BspChild::Node(value as usize)
    } else {
        BspChild::Leaf((-1 - value) as usize)
    }
}

pub fn load_prl(path: &str) -> Result<LevelWorld, PrlLoadError> {
    let path_ref = Path::new(path);
    if !path_ref.exists() {
        return Err(PrlLoadError::FileNotFound(path.to_string()));
    }

    let file_data = std::fs::read(path_ref)?;
    let mut cursor = std::io::Cursor::new(&file_data);

    let meta = prl_format::read_container(&mut cursor)?;

    // Geometry section (required).
    let geom_data = prl_format::read_section_data(&mut cursor, &meta, SectionId::Geometry as u32)?
        .ok_or(PrlLoadError::NoGeometry)?;
    let geom = GeometrySection::from_bytes(&geom_data)?;

    // BSP nodes section (optional — absent if tree is a single leaf).
    let nodes_section =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::BspNodes as u32)? {
            Some(data) => Some(BspNodesSection::from_bytes(&data)?),
            None => None,
        };

    // BSP leaves section (optional).
    let leaves_section =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::BspLeaves as u32)? {
            Some(data) => Some(BspLeavesSection::from_bytes(&data)?),
            None => None,
        };

    // Leaf PVS section (optional).
    let pvs_section =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::LeafPvs as u32)? {
            Some(data) => Some(LeafPvsSection::from_bytes(&data)?),
            None => None,
        };

    // Portals section (optional).
    let portals_section =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::Portals as u32)? {
            Some(data) => Some(PortalsSection::from_bytes(&data)?),
            None => None,
        };

    let has_pvs = pvs_section.is_some();
    let has_portals = portals_section.is_some();

    let face_meta: Vec<FaceMeta> = geom
        .faces
        .iter()
        .map(|f| FaceMeta {
            index_offset: f.index_offset,
            index_count: f.index_count,
        })
        .collect();

    // Build runtime nodes from the nodes section.
    let nodes: Vec<NodeData> = match &nodes_section {
        Some(section) => section
            .nodes
            .iter()
            .map(|n| NodeData {
                plane_normal: Vec3::from(n.plane_normal),
                plane_distance: n.plane_distance,
                front: decode_child(n.front),
                back: decode_child(n.back),
            })
            .collect(),
        None => Vec::new(),
    };

    // Build runtime leaves from the leaves section + PVS data.
    let leaves: Vec<LeafData> = match &leaves_section {
        Some(leaf_sec) => {
            let leaf_count = leaf_sec.leaves.len();
            let pvs_byte_count = leaf_count.div_ceil(8);

            leaf_sec
                .leaves
                .iter()
                .map(|lr| {
                    let pvs = if let Some(pvs_sec) = &pvs_section {
                        if lr.pvs_size > 0 && lr.is_solid == 0 {
                            let start = lr.pvs_offset as usize;
                            let end = start + lr.pvs_size as usize;
                            let pvs_slice = if end <= pvs_sec.pvs_data.len() {
                                &pvs_sec.pvs_data[start..end]
                            } else {
                                &[]
                            };

                            let decompressed = decompress_pvs(pvs_slice, pvs_byte_count);

                            // Convert byte bitfield to per-leaf bool vec.
                            let mut pvs_bools = Vec::with_capacity(leaf_count);
                            for leaf_idx in 0..leaf_count {
                                let byte_idx = leaf_idx / 8;
                                let bit_idx = leaf_idx % 8;
                                let visible = byte_idx < decompressed.len()
                                    && (decompressed[byte_idx] & (1 << bit_idx)) != 0;
                                pvs_bools.push(visible);
                            }
                            pvs_bools
                        } else {
                            // Solid leaf or no PVS data for this leaf.
                            vec![false; leaf_count]
                        }
                    } else {
                        // No PVS section at all — all leaves visible.
                        vec![true; leaf_count]
                    };

                    LeafData {
                        bounds_min: Vec3::from(lr.bounds_min),
                        bounds_max: Vec3::from(lr.bounds_max),
                        face_start: lr.face_start,
                        face_count: lr.face_count,
                        pvs,
                        is_solid: lr.is_solid != 0,
                    }
                })
                .collect()
        }
        None => {
            // No BSP leaves section — derive a single leaf from geometry.
            log::warn!("[PRL] No BSP leaves section — creating single-leaf fallback");
            let mut mins = Vec3::splat(f32::MAX);
            let mut maxs = Vec3::splat(f32::MIN);
            for v in &geom.vertices {
                let pos = Vec3::from(*v);
                mins = mins.min(pos);
                maxs = maxs.max(pos);
            }
            vec![LeafData {
                bounds_min: mins,
                bounds_max: maxs,
                face_start: 0,
                face_count: face_meta.len() as u32,
                pvs: vec![true],
                is_solid: false,
            }]
        }
    };

    // Determine BSP root. If nodes exist, root is node 0. Otherwise, leaf 0.
    let root = if nodes.is_empty() {
        BspChild::Leaf(0)
    } else {
        BspChild::Node(0)
    };

    // Load portal data and build adjacency list.
    let (portals, leaf_portals) = if let Some(ps) = &portals_section {
        let portal_data: Vec<PortalData> = ps
            .portals
            .iter()
            .map(|pr| {
                let start = pr.vertex_start as usize;
                let count = pr.vertex_count as usize;
                let end = start + count;
                if end > ps.vertices.len() {
                    return Err(PrlLoadError::FormatError(prl_format::FormatError::Io(
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!(
                                "portal vertex range {}..{} exceeds vertex count {}",
                                start,
                                end,
                                ps.vertices.len()
                            ),
                        ),
                    )));
                }
                let polygon: Vec<Vec3> = ps.vertices[start..end]
                    .iter()
                    .map(|v| Vec3::from(*v))
                    .collect();
                Ok(PortalData {
                    polygon,
                    front_leaf: pr.front_leaf as usize,
                    back_leaf: pr.back_leaf as usize,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Build per-leaf adjacency.
        let mut adjacency = vec![Vec::new(); leaves.len()];
        for (portal_idx, portal) in portal_data.iter().enumerate() {
            if portal.front_leaf < adjacency.len() {
                adjacency[portal.front_leaf].push(portal_idx);
            }
            if portal.back_leaf < adjacency.len() {
                adjacency[portal.back_leaf].push(portal_idx);
            }
        }

        (portal_data, adjacency)
    } else {
        (Vec::new(), vec![Vec::new(); leaves.len()])
    };

    log::info!(
        "[PRL] Loaded: {} vertices, {} indices ({} triangles), {} faces, {} nodes, {} leaves, pvs={}, portals={}",
        geom.vertices.len(),
        geom.indices.len(),
        geom.indices.len() / 3,
        face_meta.len(),
        nodes.len(),
        leaves.len(),
        has_pvs,
        portals.len(),
    );

    Ok(LevelWorld {
        vertices: geom.vertices,
        indices: geom.indices,
        face_meta,
        leaves,
        nodes,
        root,
        has_pvs,
        portals,
        leaf_portals,
        has_portals,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::bsp::{
        BspLeafRecord, BspLeavesSection, BspNodeRecord, BspNodesSection,
    };
    use postretro_level_format::geometry::{FaceMeta as FormatFaceMeta, GeometrySection};
    use postretro_level_format::leaf_pvs::LeafPvsSection;
    use postretro_level_format::visibility::compress_pvs;

    // -- find_leaf tests --

    /// Build a simple two-leaf BSP: one node splits space at X=0.
    /// Front (X >= 0) goes to leaf 0, back (X < 0) goes to leaf 1.
    fn two_leaf_world() -> LevelWorld {
        LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![NodeData {
                plane_normal: Vec3::X,
                plane_distance: 0.0,
                front: BspChild::Leaf(0),
                back: BspChild::Leaf(1),
            }],
            leaves: vec![
                LeafData {
                    bounds_min: Vec3::new(0.0, -100.0, -100.0),
                    bounds_max: Vec3::new(100.0, 100.0, 100.0),
                    face_start: 0,
                    face_count: 1,
                    pvs: vec![true, true],
                    is_solid: false,
                },
                LeafData {
                    bounds_min: Vec3::new(-100.0, -100.0, -100.0),
                    bounds_max: Vec3::new(0.0, 100.0, 100.0),
                    face_start: 1,
                    face_count: 1,
                    pvs: vec![true, true],
                    is_solid: false,
                },
            ],
            root: BspChild::Node(0),
            has_pvs: true,
            portals: vec![],
            leaf_portals: vec![vec![], vec![]],
            has_portals: false,
        }
    }

    #[test]
    fn find_leaf_front_side() {
        let world = two_leaf_world();
        assert_eq!(world.find_leaf(Vec3::new(10.0, 0.0, 0.0)), 0);
    }

    #[test]
    fn find_leaf_back_side() {
        let world = two_leaf_world();
        assert_eq!(world.find_leaf(Vec3::new(-10.0, 0.0, 0.0)), 1);
    }

    #[test]
    fn find_leaf_on_plane_goes_front() {
        let world = two_leaf_world();
        // Exactly on the plane (dot = 0.0 >= 0.0) should go to front.
        assert_eq!(world.find_leaf(Vec3::ZERO), 0);
    }

    #[test]
    fn find_leaf_single_leaf_tree() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![],
            leaves: vec![LeafData {
                bounds_min: Vec3::splat(-100.0),
                bounds_max: Vec3::splat(100.0),
                face_start: 0,
                face_count: 0,
                pvs: vec![true],
                is_solid: false,
            }],
            root: BspChild::Leaf(0),
            has_pvs: false,
            portals: vec![],
            leaf_portals: vec![vec![]],
            has_portals: false,
        };
        assert_eq!(world.find_leaf(Vec3::new(50.0, 50.0, 50.0)), 0);
    }

    #[test]
    fn find_leaf_deep_tree() {
        // 3-level tree: root splits on X=0, front splits on Y=0.
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![
                // Node 0: split on X=0
                NodeData {
                    plane_normal: Vec3::X,
                    plane_distance: 0.0,
                    front: BspChild::Node(1),
                    back: BspChild::Leaf(0),
                },
                // Node 1: split on Y=0
                NodeData {
                    plane_normal: Vec3::Y,
                    plane_distance: 0.0,
                    front: BspChild::Leaf(1),
                    back: BspChild::Leaf(2),
                },
            ],
            leaves: vec![
                LeafData {
                    bounds_min: Vec3::splat(-100.0),
                    bounds_max: Vec3::new(0.0, 100.0, 100.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
                LeafData {
                    bounds_min: Vec3::new(0.0, 0.0, -100.0),
                    bounds_max: Vec3::splat(100.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
                LeafData {
                    bounds_min: Vec3::new(0.0, -100.0, -100.0),
                    bounds_max: Vec3::new(100.0, 0.0, 100.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
            ],
            root: BspChild::Node(0),
            has_pvs: false,
            portals: vec![],
            leaf_portals: vec![vec![], vec![], vec![]],
            has_portals: false,
        };

        // X < 0 -> leaf 0
        assert_eq!(world.find_leaf(Vec3::new(-5.0, 0.0, 0.0)), 0);
        // X > 0, Y > 0 -> leaf 1
        assert_eq!(world.find_leaf(Vec3::new(5.0, 5.0, 0.0)), 1);
        // X > 0, Y < 0 -> leaf 2
        assert_eq!(world.find_leaf(Vec3::new(5.0, -5.0, 0.0)), 2);
    }

    // -- decode_child tests --

    #[test]
    fn decode_child_positive_is_node() {
        assert_eq!(decode_child(0), BspChild::Node(0));
        assert_eq!(decode_child(5), BspChild::Node(5));
    }

    #[test]
    fn decode_child_negative_is_leaf() {
        // -1 - leaf_index encoding
        assert_eq!(decode_child(-1), BspChild::Leaf(0));
        assert_eq!(decode_child(-6), BspChild::Leaf(5));
        assert_eq!(decode_child(-101), BspChild::Leaf(100));
    }

    // -- spawn_position tests --

    #[test]
    fn spawn_position_centers_non_solid_leaves() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![FaceMeta {
                index_offset: 0,
                index_count: 3,
            }],
            nodes: vec![],
            leaves: vec![
                LeafData {
                    bounds_min: Vec3::ZERO,
                    bounds_max: Vec3::splat(10.0),
                    face_start: 0,
                    face_count: 1,
                    pvs: vec![],
                    is_solid: false,
                },
                LeafData {
                    bounds_min: Vec3::ZERO,
                    bounds_max: Vec3::ZERO,
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: true,
                },
            ],
            root: BspChild::Leaf(0),
            has_pvs: false,
            portals: vec![],
            leaf_portals: vec![vec![], vec![]],
            has_portals: false,
        };

        let spawn = world.spawn_position();
        assert!((spawn - Vec3::splat(5.0)).length() < 0.01);
    }

    // -- face_leaf_indices tests --

    #[test]
    fn face_leaf_indices_maps_faces_to_leaves() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![
                FaceMeta {
                    index_offset: 0,
                    index_count: 3,
                },
                FaceMeta {
                    index_offset: 3,
                    index_count: 3,
                },
                FaceMeta {
                    index_offset: 6,
                    index_count: 3,
                },
            ],
            nodes: vec![],
            leaves: vec![
                LeafData {
                    bounds_min: Vec3::ZERO,
                    bounds_max: Vec3::ZERO,
                    face_start: 0,
                    face_count: 2,
                    pvs: vec![],
                    is_solid: false,
                },
                LeafData {
                    bounds_min: Vec3::ZERO,
                    bounds_max: Vec3::ZERO,
                    face_start: 2,
                    face_count: 1,
                    pvs: vec![],
                    is_solid: false,
                },
            ],
            root: BspChild::Leaf(0),
            has_pvs: false,
            portals: vec![],
            leaf_portals: vec![vec![], vec![]],
            has_portals: false,
        };

        let indices = face_leaf_indices(&world);
        assert_eq!(indices, vec![0, 0, 1]);
    }

    // -- load_prl error cases --

    #[test]
    fn load_prl_missing_file_returns_file_not_found() {
        let result = load_prl("nonexistent/path/to/map.prl");
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), PrlLoadError::FileNotFound(_)),
            "expected FileNotFound"
        );
    }

    // -- Round-trip: write a PRL file with BSP sections, load it --

    fn sample_geometry() -> GeometrySection {
        GeometrySection {
            vertices: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [10.0, 0.0, 0.0],
                [11.0, 0.0, 0.0],
                [11.0, 1.0, 0.0],
            ],
            indices: vec![0, 1, 2, 3, 4, 5],
            faces: vec![
                FormatFaceMeta {
                    index_offset: 0,
                    index_count: 3,
                    leaf_index: 0,
                },
                FormatFaceMeta {
                    index_offset: 3,
                    index_count: 3,
                    leaf_index: 1,
                },
            ],
        }
    }

    #[test]
    fn load_prl_round_trip_with_bsp_sections() {
        let geom = sample_geometry();

        // Build BSP sections: 1 node splitting two leaves.
        let nodes = BspNodesSection {
            nodes: vec![BspNodeRecord {
                plane_normal: [1.0, 0.0, 0.0],
                plane_distance: 5.0,
                front: -1 - 0, // leaf 0
                back: -1 - 1,  // leaf 1
            }],
        };

        // Build PVS: 2 leaves, each sees both.
        let pvs_uncompressed = vec![0b0000_0011u8]; // both bits set
        let compressed_0 = compress_pvs(&pvs_uncompressed);
        let compressed_1 = compress_pvs(&pvs_uncompressed);

        let mut pvs_data = Vec::new();
        let offset_0 = pvs_data.len() as u32;
        let size_0 = compressed_0.len() as u32;
        pvs_data.extend_from_slice(&compressed_0);
        let offset_1 = pvs_data.len() as u32;
        let size_1 = compressed_1.len() as u32;
        pvs_data.extend_from_slice(&compressed_1);

        let leaves = BspLeavesSection {
            leaves: vec![
                BspLeafRecord {
                    face_start: 0,
                    face_count: 1,
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [2.0, 2.0, 2.0],
                    pvs_offset: offset_0,
                    pvs_size: size_0,
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 1,
                    face_count: 1,
                    bounds_min: [9.0, 0.0, 0.0],
                    bounds_max: [12.0, 2.0, 2.0],
                    pvs_offset: offset_1,
                    pvs_size: size_1,
                    is_solid: 0,
                },
            ],
        };

        let pvs_section = LeafPvsSection { pvs_data };

        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: geom.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::BspNodes as u32,
                version: 1,
                data: nodes.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::BspLeaves as u32,
                version: 1,
                data: leaves.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::LeafPvs as u32,
                version: 1,
                data: pvs_section.to_bytes(),
            },
        ];

        let tmp = std::env::temp_dir().join("postretro_test_bsp_round_trip.prl");
        let mut file = std::fs::File::create(&tmp).unwrap();
        prl_format::write_prl(&mut file, &sections).unwrap();

        let world = load_prl(tmp.to_str().unwrap()).unwrap();
        assert_eq!(world.vertices.len(), 6);
        assert_eq!(world.indices.len(), 6);
        assert_eq!(world.face_meta.len(), 2);
        assert_eq!(world.nodes.len(), 1);
        assert_eq!(world.leaves.len(), 2);
        assert!(world.has_pvs);
        assert_eq!(world.root, BspChild::Node(0));

        // Verify leaf PVS decompression.
        assert_eq!(world.leaves[0].pvs.len(), 2);
        assert!(world.leaves[0].pvs[0]);
        assert!(world.leaves[0].pvs[1]);
        assert_eq!(world.leaves[1].pvs.len(), 2);
        assert!(world.leaves[1].pvs[0]);
        assert!(world.leaves[1].pvs[1]);

        // Verify BSP descent.
        // Node splits at X=5: front (X >= 5) -> leaf 0, back (X < 5) -> leaf 1.
        // Wait — front child is -1-0 = leaf 0, back child is -1-1 = leaf 1.
        // A point at X=10 has dot = 10 >= 5, so front -> leaf 0.
        assert_eq!(world.find_leaf(Vec3::new(10.0, 0.0, 0.0)), 0);
        // A point at X=0 has dot = 0 < 5, so back -> leaf 1.
        assert_eq!(world.find_leaf(Vec3::new(0.0, 0.0, 0.0)), 1);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_round_trip_geometry_only() {
        let geom = sample_geometry();

        let sections = vec![prl_format::SectionBlob {
            section_id: SectionId::Geometry as u32,
            version: 1,
            data: geom.to_bytes(),
        }];

        let tmp = std::env::temp_dir().join("postretro_test_geom_only.prl");
        let mut file = std::fs::File::create(&tmp).unwrap();
        prl_format::write_prl(&mut file, &sections).unwrap();

        let world = load_prl(tmp.to_str().unwrap()).unwrap();
        assert_eq!(world.vertices.len(), 6);
        assert_eq!(world.nodes.len(), 0);
        assert_eq!(world.leaves.len(), 1); // fallback single leaf
        assert!(!world.has_pvs);
        assert_eq!(world.root, BspChild::Leaf(0));

        // Single-leaf fallback: all faces in leaf 0.
        assert_eq!(world.leaves[0].face_start, 0);
        assert_eq!(world.leaves[0].face_count, 2);
        assert!(!world.leaves[0].is_solid);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_with_solid_leaf_has_empty_pvs() {
        let geom = sample_geometry();

        let nodes = BspNodesSection {
            nodes: vec![BspNodeRecord {
                plane_normal: [1.0, 0.0, 0.0],
                plane_distance: 5.0,
                front: -1 - 0, // leaf 0 (empty)
                back: -1 - 1,  // leaf 1 (solid)
            }],
        };

        let pvs_uncompressed = vec![0b0000_0001u8]; // only self visible
        let compressed = compress_pvs(&pvs_uncompressed);

        let leaves = BspLeavesSection {
            leaves: vec![
                BspLeafRecord {
                    face_start: 0,
                    face_count: 2,
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [10.0, 10.0, 10.0],
                    pvs_offset: 0,
                    pvs_size: compressed.len() as u32,
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 0,
                    face_count: 0,
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [0.0, 0.0, 0.0],
                    pvs_offset: 0,
                    pvs_size: 0,
                    is_solid: 1,
                },
            ],
        };

        let pvs_section = LeafPvsSection {
            pvs_data: compressed,
        };

        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: geom.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::BspNodes as u32,
                version: 1,
                data: nodes.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::BspLeaves as u32,
                version: 1,
                data: leaves.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::LeafPvs as u32,
                version: 1,
                data: pvs_section.to_bytes(),
            },
        ];

        let tmp = std::env::temp_dir().join("postretro_test_solid_leaf.prl");
        let mut file = std::fs::File::create(&tmp).unwrap();
        prl_format::write_prl(&mut file, &sections).unwrap();

        let world = load_prl(tmp.to_str().unwrap()).unwrap();

        // Solid leaf should have all-false PVS.
        assert!(world.leaves[1].is_solid);
        assert!(world.leaves[1].pvs.iter().all(|&v| !v));

        // Empty leaf should have valid PVS.
        assert!(!world.leaves[0].is_solid);
        assert!(world.leaves[0].pvs[0]); // sees self

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_invalid_magic_produces_clear_error() {
        let tmp = std::env::temp_dir().join("postretro_test_bad_magic.prl");
        std::fs::write(&tmp, b"NOPE extra data for length").unwrap();

        let result = load_prl(tmp.to_str().unwrap());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("magic"),
            "error should mention magic: {err_msg}"
        );

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_truncated_file_produces_clear_error() {
        let tmp = std::env::temp_dir().join("postretro_test_truncated.prl");
        std::fs::write(&tmp, &[0x50, 0x52, 0x4C]).unwrap(); // "PRL" only

        let result = load_prl(tmp.to_str().unwrap());
        assert!(result.is_err());

        std::fs::remove_file(&tmp).ok();
    }
}
