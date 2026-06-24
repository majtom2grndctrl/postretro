// Free helpers for the renderer: default view-projection, vertex/index byte
// casts, line-index expansion, and PRL world -> LevelGeometry adaptation.
// See: context/lib/rendering_pipeline.md

use super::*;

pub(crate) fn build_default_view_projection(aspect: f32) -> Mat4 {
    let eye = glam::Vec3::new(0.0, 200.0, 500.0);
    let center = glam::Vec3::ZERO;
    let up = glam::Vec3::Y;

    let view = Mat4::look_at_rh(eye, center, up);
    let projection = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, aspect, 0.1, 4096.0);

    projection * view
}

pub(crate) fn cast_world_vertices_to_bytes(data: &[crate::geometry::WorldVertex]) -> Vec<u8> {
    let byte_len = data.len() * crate::geometry::WorldVertex::STRIDE;
    let mut bytes = Vec::with_capacity(byte_len);
    for vertex in data {
        for &c in &vertex.position {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.base_uv {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.normal_oct {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.tangent_packed {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.lightmap_uv {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
    }
    bytes
}

// Each triangle [a, b, c] → three line pairs [a,b, b,c, c,a].
// Shared edges are emitted multiple times; fine for a debug overlay.
pub(crate) fn build_line_indices_from_triangles(tri_indices: &[u32]) -> Vec<u32> {
    let tri_count = tri_indices.len() / 3;
    let mut lines = Vec::with_capacity(tri_count * 6);
    for tri in tri_indices.chunks_exact(3) {
        let (a, b, c) = (tri[0], tri[1], tri[2]);
        lines.push(a);
        lines.push(b);
        lines.push(b);
        lines.push(c);
        lines.push(c);
        lines.push(a);
    }
    lines
}

pub(crate) fn bytemuck_cast_slice_u32(data: &[u32]) -> Vec<u8> {
    let byte_len = std::mem::size_of_val(data);
    let mut bytes = Vec::with_capacity(byte_len);
    for &val in data {
        bytes.extend_from_slice(&val.to_ne_bytes());
    }
    bytes
}

/// See: context/lib/boot_sequence.md §3 (Level Install Order)
pub fn level_world_to_geometry<'a>(
    world: &'a crate::prl::LevelWorld,
    texture_materials: &'a [Material],
) -> LevelGeometry<'a> {
    LevelGeometry {
        vertices: &world.vertices,
        indices: &world.indices,
        bvh: &world.bvh,
        lights: &world.lights,
        light_influences: &world.light_influences,
        sh_volume: world.sh_volume.as_ref(),
        lightmap: world.lightmap.as_ref(),
        chunk_light_list: world.chunk_light_list.as_ref(),
        animated_light_chunks: world.animated_light_chunks.as_ref(),
        animated_light_weight_maps: world.animated_light_weight_maps.as_ref(),
        delta_sh_volumes: world.delta_sh_volumes.as_ref(),
        direct_sh_volume: world.direct_sh_volume.as_ref(),
        sdf_atlas: world.sdf_atlas.as_ref(),
        lightmap_mode: world.lightmap_mode,
        cell_draw_index: world.cell_draw_index.as_ref(),
        texture_materials,
    }
}
