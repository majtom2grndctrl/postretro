// World-texture and one-triangle level fixtures shared by the renderer's GPU
// tests: baked `.prm` sidecars and the smallest installable level. The
// off-main textures part is proved against the oracle in `level_build_tests`.
// See: context/lib/testing_guide.md §3 (GPU tests) · context/lib/boot_sequence.md §2

use super::*;
use crate::render::{LevelGeometry, LevelGeometryShStorage};
use glam::Vec3;
use postretro_level_format::cell_draw_index::{CellDrawIndexSection, Span};
use postretro_level_format::prm::{
    PrmFile, PrmFormat, PrmHeader, PrmSlot, PrmSlots, STAGE_VERSION, cache_filename_for_key,
    expected_level_count,
};
use postretro_level_loader::CellData;
use postretro_render_data::geometry::{BVH_NODE_FLAG_LEAF, BvhLeaf, BvhNode, BvhTree, WorldVertex};

/// A full mip chain of `format` with distinct bytes per level and texel.
pub(in crate::render) fn chain(format: PrmFormat, width: u16, height: u16, seed: u8) -> PrmSlot {
    let level_count = expected_level_count(width, height);
    let bpp = match format {
        PrmFormat::R8Unorm => 1,
        _ => 4,
    };
    let mut payload = Vec::new();
    for level in 0..level_count {
        let w = (u32::from(width) >> level).max(1);
        let h = (u32::from(height) >> level).max(1);
        payload.extend((0..bpp * w * h).map(|i| (i as u8).wrapping_mul(7) ^ seed ^ level));
    }
    PrmSlot {
        format,
        width,
        height,
        level_count,
        payload,
    }
}

pub(in crate::render) fn write_prm(root: &Path, key: [u8; 32], slots: [Option<PrmSlot>; 4]) {
    let mut slot_mask = PrmSlots::empty();
    for (present, bit) in slots.iter().zip([
        PrmSlots::DIFFUSE,
        PrmSlots::SPECULAR,
        PrmSlots::NORMAL,
        PrmSlots::EMISSIVE,
    ]) {
        if present.is_some() {
            slot_mask |= bit;
        }
    }
    let bytes = PrmFile {
        header: PrmHeader {
            stage_version: STAGE_VERSION,
            slot_mask,
            bundle_hash: [0u8; 32],
            total_body_bytes: 0,
            layer_count: 1,
        },
        slots,
    }
    .to_bytes()
    .expect("fixture .prm serializes");
    std::fs::write(
        root.join(format!("{}.prm", cache_filename_for_key(&key))),
        bytes,
    )
    .unwrap();
}

/// One triangle in one cell whose leaf draws material bucket 0.
pub(in crate::render) fn install_one_triangle(renderer: &mut Renderer, materials: &[Material]) {
    install_one_triangle_with_sh(renderer, materials, None, None);
}

/// [`install_one_triangle`] with an SH volume and baked direct SH, so the SH
/// and direct compose families bind real data.
pub(in crate::render) fn install_one_triangle_with_sh(
    renderer: &mut Renderer,
    materials: &[Material],
    sh_volume: Option<&postretro_level_format::sh_volume::OctahedralShVolumeSection>,
    direct_sh_volume: Option<&postretro_level_format::direct_sh_volume::DirectShVolumeSection>,
) {
    with_one_triangle(materials, sh_volume, direct_sh_volume, &[], |geometry| {
        renderer
            .install_level_geometry(geometry, Default::default())
            .expect("level resources install");
    });
}

/// The one-triangle level (one cell, one BVH leaf, the triangle at z = 0
/// facing +Z) with `lights`, handed to `use_geometry` to install or build.
pub(in crate::render) fn with_one_triangle<R>(
    materials: &[Material],
    sh_volume: Option<&postretro_level_format::sh_volume::OctahedralShVolumeSection>,
    direct_sh_volume: Option<&postretro_level_format::direct_sh_volume::DirectShVolumeSection>,
    lights: &[postretro_level_loader::MapLight],
    use_geometry: impl FnOnce(&LevelGeometry<'_>) -> R,
) -> R {
    let vertices: Vec<WorldVertex> = [[-0.4, -0.4, 0.0], [0.4, -0.4, 0.0], [0.0, 0.4, 0.0]]
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
    let indices = [0u32, 1, 2];
    let leaf = BvhLeaf {
        aabb_min: [-0.5; 3],
        material_bucket_id: 0,
        aabb_max: [0.5; 3],
        index_offset: 0,
        index_count: 3,
        cell_id: 0,
        chunk_range_start: 0,
        chunk_range_count: 0,
    };
    let bvh = BvhTree {
        nodes: vec![
            BvhNode {
                aabb_min: leaf.aabb_min,
                skip_index: 2,
                aabb_max: leaf.aabb_max,
                left_child_or_leaf_index: 0,
                flags: 0,
            },
            BvhNode {
                aabb_min: leaf.aabb_min,
                skip_index: 2,
                aabb_max: leaf.aabb_max,
                left_child_or_leaf_index: 0,
                flags: BVH_NODE_FLAG_LEAF,
            },
        ],
        leaves: vec![leaf],
        root_node_index: 0,
    };
    let index = CellDrawIndexSection::from_bytes(
        &CellDrawIndexSection {
            cell_count: 1,
            span_count: 1,
            cell_span_offset: vec![0, 1],
            spans: vec![Span {
                leaf_start: 0,
                leaf_count: 1,
            }],
        }
        .to_bytes(),
    )
    .unwrap();
    let cells = [CellData {
        bounds_min: Vec3::splat(-0.5),
        bounds_max: Vec3::splat(0.5),
        face_start: 0,
        face_count: 1,
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid: false,
        is_exterior: false,
        is_drawable: true,
    }];
    use_geometry(&LevelGeometry {
        vertices: &vertices,
        vertex_spec_owners: &[],
        indices: &indices,
        bvh: &bvh,
        lights,
        light_influences: &[],
        sh_volume,
        sh_storage: LevelGeometryShStorage::Legacy,
        probe_depth_tiles: None,
        lightmap: None,
        lightmap_streaming: None,
        chunk_light_list: None,
        animated_light_chunks: None,
        animated_light_weight_maps: None,
        delta_sh_volumes: None,
        direct_sh_volume,
        direct_sh_delta_volumes: None,
        animated_direct_sh_delta_volumes: None,
        billboard_direct_scatter_volume: None,
        animated_billboard_direct_scatter_delta_volumes: None,
        mover_probe_channel: None,
        entity_shadow_lights: &[],
        shadowmask_atlas: None,
        sdf_atlas: None,
        lightmap_mode: postretro_level_loader::LightmapMode::Shadowed,
        cell_draw_index: Some(&index),
        kinematic_geometry: None,
        cells: &cells,
        texture_materials: materials,
    })
}
