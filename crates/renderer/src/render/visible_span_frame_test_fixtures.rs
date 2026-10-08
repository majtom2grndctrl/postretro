//! Small installed geometry and validated CSR spans for production-frame tests.
//! See: context/lib/rendering_pipeline.md §5, §7.1–7.3.

use crate::render::{LevelGeometry, LevelGeometryShStorage, Renderer};
use glam::Vec3;
use postretro_level_format::cell_draw_index::{CellDrawIndexSection, Span};
use postretro_level_loader::{CellData, MapLight};
use postretro_render_data::geometry::{BVH_NODE_FLAG_LEAF, BvhLeaf, BvhNode, BvhTree, WorldVertex};

pub(super) struct World {
    pub(super) bvh: BvhTree,
    index: CellDrawIndexSection,
    vertices: Vec<WorldVertex>,
    indices: Vec<u32>,
    cells: Vec<CellData>,
    pub(super) lights: Vec<MapLight>,
}

impl World {
    // Two buckets, with abutting cell runs and a nonadjacent run for cell 0.
    // Cell 3 is an empty CSR row; cell 4 is the last valid drawable cell.
    pub(super) fn new(reordered: bool) -> Self {
        let ownership = if reordered {
            [1, 0, 0, 2, 4, 1]
        } else {
            [0, 1, 2, 0, 4, 1]
        };
        let leaves: Vec<_> = ownership
            .iter()
            .enumerate()
            .map(|(slot, &cell_id)| BvhLeaf {
                aabb_min: [-0.25, -0.25, 0.25],
                aabb_max: [0.25, 0.25, 0.75],
                material_bucket_id: u32::from(slot >= 4),
                index_offset: slot as u32 * 3,
                index_count: 3,
                cell_id,
                chunk_range_start: 0,
                chunk_range_count: 0,
            })
            .collect();
        // One internal root followed by threaded leaf children: every skip
        // exits its subtree, and the root skip exits the whole node array.
        let mut nodes = vec![BvhNode {
            aabb_min: [-0.25, -0.25, 0.25],
            aabb_max: [0.25, 0.25, 0.75],
            skip_index: leaves.len() as u32 + 1,
            left_child_or_leaf_index: 1,
            flags: 0,
        }];
        nodes.extend(leaves.iter().enumerate().map(|(slot, leaf)| BvhNode {
            aabb_min: leaf.aabb_min,
            aabb_max: leaf.aabb_max,
            skip_index: slot as u32 + 2,
            left_child_or_leaf_index: slot as u32,
            flags: BVH_NODE_FLAG_LEAF,
        }));
        let mut spans = Vec::new();
        let mut offsets = vec![0];
        for cell in 0..5 {
            let mut slot = 0;
            while slot < leaves.len() {
                if leaves[slot].cell_id != cell {
                    slot += 1;
                    continue;
                }
                let start = slot;
                let bucket = leaves[slot].material_bucket_id;
                while slot < leaves.len()
                    && leaves[slot].cell_id == cell
                    && leaves[slot].material_bucket_id == bucket
                {
                    slot += 1;
                }
                spans.push(Span {
                    leaf_start: start as u32,
                    leaf_count: (slot - start) as u32,
                });
            }
            offsets.push(spans.len() as u32);
        }
        let index = CellDrawIndexSection {
            cell_count: 5,
            span_count: spans.len() as u32,
            cell_span_offset: offsets,
            spans,
        };
        // Decode the real section format, then check the cross-section load
        // contract explicitly (the loader's validator is crate-private).
        let index = CellDrawIndexSection::from_bytes(&index.to_bytes()).unwrap();
        let mut coverage = vec![0; leaves.len()];
        for cell in 0..5usize {
            for span in &index.spans
                [index.cell_span_offset[cell] as usize..index.cell_span_offset[cell + 1] as usize]
            {
                let bucket = leaves[span.leaf_start as usize].material_bucket_id;
                for slot in span.leaf_start..span.leaf_start + span.leaf_count {
                    assert_eq!(leaves[slot as usize].cell_id, cell as u32);
                    assert_eq!(leaves[slot as usize].material_bucket_id, bucket);
                    coverage[slot as usize] += 1;
                }
            }
        }
        assert!(coverage.iter().all(|&owners| owners == 1));
        let vertices = [[-0.25, -0.25, 0.5], [0.25, -0.25, 0.5], [0.0, 0.25, 0.5]]
            .map(|position| WorldVertex {
                position,
                base_uv: [0.0; 2],
                normal_oct: [32768; 2],
                tangent_packed: [0; 2],
                lightmap_uv: [0; 2],
                lightmap_block: 0,
                animated_block: 0,
            })
            .to_vec();
        let indices = (0..leaves.len())
            .flat_map(|_| [0, 1, 2])
            .collect::<Vec<_>>();
        crate::render::validate_level_geometry_ranges(&leaves, indices.len()).unwrap();
        let cells = (0..5)
            .map(|cell| CellData {
                bounds_min: Vec3::new(-0.25, -0.25, 0.25),
                bounds_max: Vec3::new(0.25, 0.25, 0.75),
                face_start: 0,
                face_count: u32::from(cell != 3),
                portal_ref_start: 0,
                portal_ref_count: 0,
                is_solid: false,
                is_exterior: false,
                is_drawable: cell != 3,
            })
            .collect();
        Self {
            bvh: BvhTree {
                nodes,
                leaves,
                root_node_index: 0,
            },
            index,
            vertices,
            indices,
            cells,
            lights: vec![],
        }
    }

    pub(super) fn install(&self, renderer: &mut Renderer, indexed: bool) {
        renderer.install_level_geometry(
            &LevelGeometry {
                vertices: &self.vertices,
                indices: &self.indices,
                bvh: &self.bvh,
                lights: &self.lights,
                light_influences: &[],
                sh_volume: None,
                sh_storage: LevelGeometryShStorage::Legacy,
                lightmap: None,
                lightmap_streaming: None,
                chunk_light_list: None,
                animated_light_chunks: None,
                animated_light_weight_maps: None,
                delta_sh_volumes: None,
                direct_sh_volume: None,
                direct_sh_delta_volumes: None,
                animated_direct_sh_delta_volumes: None,
                billboard_direct_scatter_volume: None,
                animated_billboard_direct_scatter_delta_volumes: None,
                entity_shadow_lights: &[],
                shadowmask_atlas: None,
                sdf_atlas: None,
                lightmap_mode: postretro_level_loader::LightmapMode::Shadowed,
                cell_draw_index: indexed.then_some(&self.index),
                kinematic_geometry: None,
                cells: &self.cells,
                texture_materials: &[],
            },
            Default::default(),
        );
    }
}
