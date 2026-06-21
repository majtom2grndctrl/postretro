// Renderer unit tests (split from the original `mod tests`).
// See: context/lib/testing_guide.md

use super::super::*;

#[test]
fn default_view_projection_is_finite() {
    let vp = build_default_view_projection(16.0 / 9.0);
    let cols = vp.to_cols_array();
    for (i, val) in cols.iter().enumerate() {
        assert!(val.is_finite(), "view_proj[{i}] is not finite: {val}");
    }
}

#[test]
fn mip_lod_max_clamp_derivation() {
    // The aniso sampler pool uses this clamp so no sampler reads past the uploaded mip chain.
    assert_eq!(mip_lod_max_clamp(1), 0.0);
    assert_eq!(mip_lod_max_clamp(8), 7.0);
    // mip_count 0 is degenerate; saturating_sub keeps it at the base level.
    assert_eq!(mip_lod_max_clamp(0), 0.0);
}

#[test]
fn cast_world_vertices_roundtrips() {
    let input = vec![
        crate::geometry::WorldVertex {
            position: [1.0, 2.0, 3.0],
            base_uv: [0.5, 0.75],
            normal_oct: [32768, 32768],
            tangent_packed: [65535, 32768],
            lightmap_uv: [100, 200],
        },
        crate::geometry::WorldVertex {
            position: [4.0, 5.0, 6.0],
            base_uv: [0.25, 0.125],
            normal_oct: [0, 32768],
            tangent_packed: [32768, 0],
            lightmap_uv: [0, 0],
        },
    ];
    let bytes = cast_world_vertices_to_bytes(&input);
    // 2 vertices * 32 bytes = 64 bytes
    assert_eq!(bytes.len(), 64);

    let pos_x = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
    let pos_y = f32::from_ne_bytes(bytes[4..8].try_into().unwrap());
    let pos_z = f32::from_ne_bytes(bytes[8..12].try_into().unwrap());
    let uv_u = f32::from_ne_bytes(bytes[12..16].try_into().unwrap());
    let uv_v = f32::from_ne_bytes(bytes[16..20].try_into().unwrap());
    let n_u = u16::from_ne_bytes(bytes[20..22].try_into().unwrap());
    let n_v = u16::from_ne_bytes(bytes[22..24].try_into().unwrap());
    let t_u = u16::from_ne_bytes(bytes[24..26].try_into().unwrap());
    let t_v = u16::from_ne_bytes(bytes[26..28].try_into().unwrap());
    let lm_u = u16::from_ne_bytes(bytes[28..30].try_into().unwrap());
    let lm_v = u16::from_ne_bytes(bytes[30..32].try_into().unwrap());

    assert_eq!([pos_x, pos_y, pos_z], [1.0, 2.0, 3.0]);
    assert_eq!([uv_u, uv_v], [0.5, 0.75]);
    assert_eq!([n_u, n_v], [32768, 32768]);
    assert_eq!([t_u, t_v], [65535, 32768]);
    assert_eq!([lm_u, lm_v], [100, 200]);
}

#[test]
fn byte_cast_u32_roundtrips() {
    let input = vec![100u32, 200, 300];
    let bytes = bytemuck_cast_slice_u32(&input);
    assert_eq!(bytes.len(), 12);

    let mut output = Vec::new();
    for chunk in bytes.chunks_exact(4) {
        output.push(u32::from_ne_bytes(chunk.try_into().unwrap()));
    }
    assert_eq!(output, vec![100, 200, 300]);
}

#[test]
fn line_indices_from_single_triangle_produces_three_edges() {
    let tri = vec![0u32, 1, 2];
    let lines = build_line_indices_from_triangles(&tri);
    assert_eq!(lines, vec![0, 1, 1, 2, 2, 0]);
}

#[test]
fn line_indices_from_two_triangles_produces_twelve_indices() {
    let tris = vec![0u32, 1, 2, 3, 4, 5];
    let lines = build_line_indices_from_triangles(&tris);
    assert_eq!(lines.len(), 12);
    assert_eq!(lines, vec![0, 1, 1, 2, 2, 0, 3, 4, 4, 5, 5, 3]);
}

#[test]
fn line_indices_from_empty_input_is_empty() {
    let lines = build_line_indices_from_triangles(&[]);
    assert!(lines.is_empty());
}

#[test]
fn line_indices_ignores_incomplete_trailing_triangle() {
    // 4 indices = 1 full triangle + 1 dangling index.
    let tris = vec![0u32, 1, 2, 3];
    let lines = build_line_indices_from_triangles(&tris);
    assert_eq!(lines, vec![0, 1, 1, 2, 2, 0]);
}
