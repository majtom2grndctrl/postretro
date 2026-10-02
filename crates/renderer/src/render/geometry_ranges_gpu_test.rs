// On-demand proof that a rejected direct install leaves GPU state unchanged.
// See: context/lib/rendering_pipeline.md §5

use super::{Renderer, validate_level_geometry_ranges};
use postretro_render_data::geometry::{BVH_NODE_FLAG_LEAF, BvhLeaf, BvhNode, BvhTree};

fn geometry<'a>(
    bvh: &'a postretro_render_data::geometry::BvhTree,
) -> crate::render::LevelGeometry<'a> {
    crate::render::LevelGeometry {
        vertices: &[],
        indices: &[],
        bvh,
        lights: &[],
        light_influences: &[],
        sh_volume: None,
        sh_storage: crate::render::LevelGeometryShStorage::Legacy,
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
        cell_draw_index: None,
        kinematic_geometry: None,
        cells: &[],
        texture_materials: &[],
    }
}

#[test]
#[ignore = "requires a GPU and a debug build; run on demand"]
fn install_range_rejection_preserves_offscreen_renderer_state() {
    let mut renderer = Renderer::new_offscreen(32, 32).expect("offscreen renderer needs a GPU");
    let mut bvh = BvhTree {
        nodes: vec![BvhNode {
            aabb_min: [0.0; 3],
            aabb_max: [1.0; 3],
            skip_index: 1,
            left_child_or_leaf_index: 0,
            flags: BVH_NODE_FLAG_LEAF,
        }],
        leaves: vec![BvhLeaf {
            aabb_min: [0.0; 3],
            aabb_max: [1.0; 3],
            material_bucket_id: 0,
            index_offset: 0,
            index_count: 3,
            cell_id: 0,
            chunk_range_start: 0,
            chunk_range_count: 0,
        }],
        root_node_index: 0,
    };
    let vertices = [postretro_render_data::geometry::WorldVertex {
        position: [0.0; 3],
        base_uv: [0.0; 2],
        normal_oct: [0; 2],
        tangent_packed: [0; 2],
        lightmap_uv: [0; 2],
        lightmap_block: 0,
        animated_block: 0,
    }; 3];
    let indices = [0, 1, 2];
    let valid = super::LevelGeometry {
        vertices: &vertices,
        indices: &indices,
        ..geometry(&bvh)
    };
    validate_level_geometry_ranges(&bvh.leaves, indices.len()).unwrap();
    renderer.install_level_geometry(&valid, Default::default());
    let index_buffer = renderer.full().index_buffer.clone();
    let vertex_buffer = renderer.full().vertex_buffer.clone();
    let leaf_buffer = renderer
        .full()
        .compute_cull
        .as_ref()
        .unwrap()
        .leaf_buffer()
        .clone();
    let index_count = renderer.full().index_count;
    let before_uploads = renderer.queue.counts();
    bvh.leaves[0].index_offset = 3;
    let invalid = super::LevelGeometry {
        vertices: &vertices,
        indices: &indices,
        ..geometry(&bvh)
    };
    assert!(validate_level_geometry_ranges(&bvh.leaves, indices.len()).is_err());
    // Direct callers keep a debug assertion as defense in depth, before the
    // installation scope, pool resets, or any resource replacement.
    let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        renderer.install_level_geometry(&invalid, Default::default());
    }));
    assert!(rejected.is_err());
    assert_eq!(renderer.full().index_buffer, index_buffer);
    assert_eq!(renderer.full().vertex_buffer, vertex_buffer);
    assert_eq!(
        *renderer.full().compute_cull.as_ref().unwrap().leaf_buffer(),
        leaf_buffer
    );
    assert_eq!(renderer.full().index_count, index_count);
    assert_eq!(renderer.full().bvh_leaves[0].index_offset, 0);
    let after_uploads = renderer.queue.counts();
    assert_eq!(after_uploads.writes, before_uploads.writes);
    assert_eq!(after_uploads.direct_writes, before_uploads.direct_writes);
    assert_eq!(after_uploads.submits, before_uploads.submits);
}
