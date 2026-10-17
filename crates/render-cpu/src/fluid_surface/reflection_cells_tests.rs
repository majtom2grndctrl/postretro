use glam::{IVec2, UVec2, Vec2, Vec3};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};

use super::*;
use crate::fluid_surface::pack_fluid_surfaces;

fn horizontal_volume(x0: f32, x1: f32, texels_per_metre: f32) -> EnvironmentVolumeRecord {
    let corner = |x: f32, z: f32| EnvironmentSurfaceVertex {
        position: [x, 0.0, z],
        uv: [x * texels_per_metre, z * texels_per_metre],
    };
    EnvironmentVolumeRecord {
        source_entity: 0,
        priority: 0,
        gravity: None,
        push: None,
        fluid: "water".to_string(),
        brushes: Vec::new(),
        surface_faces: vec![EnvironmentSurfaceFace {
            texture_index: 0,
            vertices: vec![
                corner(x0, x0),
                corner(x0, x1),
                corner(x1, x1),
                corner(x1, x0),
            ],
        }],
    }
}

#[test]
fn the_texel_fit_recovers_an_exact_affine_map_and_refuses_a_degenerate_one() {
    let u = Vec3::new(0.25, 0.0, 0.0);
    let v = Vec3::new(0.0, 0.1, 0.2);
    let origin = Vec3::new(3.0, -1.0, 7.0);
    let texels = [
        Vec2::new(-3.0, 2.0),
        Vec2::new(5.5, 2.0),
        Vec2::new(5.5, 9.25),
        Vec2::new(-3.0, 9.25),
    ];
    let positions: Vec<Vec3> = texels.iter().map(|t| origin + u * t.x + v * t.y).collect();
    let map = fit_texel_map(&positions, &texels).expect("planar affine face fits");
    assert!((map.u.as_vec3() - u).length() < 1.0e-6);
    assert!((map.v.as_vec3() - v).length() < 1.0e-6);
    assert!((map.origin.as_vec3() - origin).length() < 1.0e-5);
    let collinear = [Vec2::ZERO, Vec2::ONE, Vec2::splat(2.0)];
    assert!(fit_texel_map(&positions[..3], &collinear).is_none());
    let mut bent = positions.clone();
    bent[2].y += 0.5;
    assert!(fit_texel_map(&bent, &texels).is_none(), "a non-affine face is refused");
}

#[test]
fn a_cell_rect_floors_its_min_and_excludes_a_whole_texel_far_edge() {
    let (min, extent) = cell_rect(&[Vec2::new(-1.5, 0.0), Vec2::new(4.0, 3.25)]);
    assert_eq!(min, IVec2::new(-2, 0));
    assert_eq!(extent, UVec2::new(6, 4));
}

#[test]
fn shelf_packing_never_overlaps_and_skips_what_cannot_fit() {
    let rects = [[5, 3], [8, 8], [2, 9], [8, 1], [9, 2], [4, 4]];
    let (placements, height) = shelf_pack(&rects, 8, 32);
    assert!(placements[4].is_none(), "wider than the atlas");
    let placed: Vec<([u32; 2], [u32; 2])> = placements
        .iter()
        .zip(&rects)
        .filter_map(|(p, r)| p.map(|p| (p, *r)))
        .collect();
    assert_eq!(placed.len(), 5);
    for (i, (a, ra)) in placed.iter().enumerate() {
        assert!(a[0] + ra[0] <= 8 && a[1] + ra[1] <= height);
        for (b, rb) in &placed[i + 1..] {
            let apart = a[0] + ra[0] <= b[0]
                || b[0] + rb[0] <= a[0]
                || a[1] + ra[1] <= b[1]
                || b[1] + rb[1] <= a[1];
            assert!(apart, "{a:?}{ra:?} overlaps {b:?}{rb:?}");
        }
    }
    let (tight, _) = shelf_pack(&rects, 8, 9);
    assert!(tight.iter().filter(|p| p.is_some()).count() < 5);
}

#[test]
fn the_atlas_gives_bound_faces_entries_tiles_and_vertex_words() {
    // 4 texels per metre over 0..3 m: a 12 × 12-cell face, two tiles a side.
    let volumes = [horizontal_volume(0.0, 3.0, 4.0), horizontal_volume(5.0, 6.0, 4.0)];
    let mesh = pack_fluid_surfaces(&volumes, &[[1, 1]], 1);
    let mut probes = vec![FLUID_REFLECTION_NO_PROBE; mesh.vertices.len()];
    let first = mesh.face_drift_maps[0].unwrap();
    probes[first.vertex_start as usize..(first.vertex_start + first.vertex_count) as usize]
        .fill(0);
    let atlas = build_cell_atlas(&volumes, &mesh, &probes, &[[1, 1]], 1, 8192);
    assert_eq!(atlas.faces.len(), 1);
    assert_eq!(atlas.skipped_faces, 0);
    let face = atlas.faces[0];
    assert_eq!(face.extent, [12, 12]);
    assert_eq!((face.cell_min_x, face.cell_min_y), (0, 0));
    assert_eq!(atlas.slot_tiles[0], 0..4);
    assert_eq!(atlas.slot_tiles[1], 4..4);
    assert_eq!(atlas.slot_cells, vec![144, 0]);
    assert!(atlas.width >= 12 && atlas.height >= 12);
    let words = &atlas.vertex_words;
    assert!(words[..4].iter().all(|w| w[1] == 12 | (12 << 16)));
    assert!(words[4..].iter().all(|w| w[0] == FLUID_CELL_NO_ATLAS));
    let centre = Vec3::from(face.origin)
        + Vec3::from(face.axis_u) * 0.5
        + Vec3::from(face.axis_v) * 0.5;
    assert!((centre - Vec3::new(0.125, 0.0, 0.125)).length() < 1.0e-5);

    let mut frame = Vec::new();
    assert_eq!(atlas.fill_frame_tiles([(1, 1.0), (0, 2.5)].into_iter(), &mut frame), 144);
    assert_eq!(frame.len(), 4);
    assert!(frame.iter().all(|tile| tile.reflectivity == 2.5));
}

#[test]
fn analytic_texels_per_pixel_matches_projected_finite_differences() {
    let extent = Vec2::new(640.0, 360.0);
    let camera = Vec3::new(0.3, 2.0, -1.0);
    let view_proj = glam::camera::rh::proj::directx::perspective(
        70f32.to_radians(),
        extent.x / extent.y,
        0.1,
        100.0,
    ) * glam::camera::rh::view::look_at_mat4(camera, Vec3::new(1.0, 0.0, 4.0), Vec3::Y);
    let u = Vec3::new(0.0625, 0.0, 0.0);
    let v = Vec3::new(0.0, 0.0, 0.0625);
    // In f64, so the differences measure the projection, not f32 rounding.
    let dview_proj = view_proj.as_dmat4();
    let to_pixel = |p: glam::DVec3| {
        let c = dview_proj * p.extend(1.0);
        glam::DVec2::new(c.x / c.w, c.y / c.w) * extent.as_dvec2() * 0.5
    };
    for centre in [Vec3::new(1.0, 0.0, 4.0), Vec3::new(-0.5, 0.0, 1.5), Vec3::new(2.0, 0.0, 9.0)] {
        let analytic = cell_texels_per_pixel(view_proj, extent, centre, u, v);
        // Pixel steps per texel by central differences, then inverted.
        let (c, du, dv) = (centre.as_dvec3(), u.as_dvec3(), v.as_dvec3());
        let h = 1.0e-4;
        let ju = (to_pixel(c + du * h) - to_pixel(c - du * h)) / (2.0 * h);
        let jv = (to_pixel(c + dv * h) - to_pixel(c - dv * h)) / (2.0 * h);
        let jacobian = glam::DMat2::from_cols(ju, jv).inverse();
        let numeric = jacobian.x_axis.length().max(jacobian.y_axis.length()) as f32;
        assert!(
            (analytic - numeric).abs() <= 1.0e-3 * numeric,
            "{centre}: analytic {analytic}, numeric {numeric}"
        );
    }
    let behind = cell_texels_per_pixel(view_proj, extent, camera - Vec3::Z * 3.0, u, v);
    assert_eq!(behind, FLUID_CELL_FLAT_TEXELS_PER_PIXEL);
}

#[test]
fn frustum_planes_hold_what_the_view_sees_and_drop_what_lies_behind() {
    let view_proj = glam::camera::rh::proj::directx::perspective(60f32.to_radians(), 1.0, 0.1, 50.0)
        * glam::camera::rh::view::look_at_mat4(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), Vec3::Y);
    let planes = frustum_planes(view_proj);
    assert!(sphere_in_frustum(&planes, Vec3::new(0.0, 0.0, -5.0), 0.0));
    assert!(!sphere_in_frustum(&planes, Vec3::new(0.0, 0.0, 5.0), 0.5));
    assert!(!sphere_in_frustum(&planes, Vec3::new(20.0, 0.0, -5.0), 0.5));
    assert!(sphere_in_frustum(&planes, Vec3::new(3.2, 0.0, -5.0), 0.5), "touching the edge");
    assert!(!sphere_in_frustum(&planes, Vec3::new(0.0, 0.0, -60.0), 0.5), "past far");
    assert_eq!(cell_radius(Vec3::X, Vec3::Z), 0.5 * 2f32.sqrt());
}

#[test]
fn the_dispatch_covers_every_tile_within_the_dimension_limit() {
    assert_eq!(cell_dispatch(0, 65535), (0, 0));
    assert_eq!(cell_dispatch(10, 65535), (10, 1));
    let (x, y) = cell_dispatch(200_000, 65535);
    assert_eq!(x, 65535);
    assert!(x * y >= 200_000 && x * (y - 1) < 200_000);
}
