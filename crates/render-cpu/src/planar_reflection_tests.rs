use super::*;
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex,
};
use postretro_level_loader::{CellData, CellLocatorChild};

fn projection() -> Mat4 {
    glam::camera::rh::proj::directx::perspective(75.0_f32.to_radians(), 1.5, 0.1, 128.0)
}

fn view(eye: Vec3) -> Mat4 {
    glam::camera::rh::view::look_at_mat4(eye, Vec3::new(0.0, 0.0, -8.0), Vec3::Y)
}

fn water(h: f32, x: f32) -> EnvironmentVolumeRecord {
    let vertices = [
        [x - 2.0, h, -8.0],
        [x - 2.0, h, -4.0],
        [x + 2.0, h, -4.0],
        [x + 2.0, h, -8.0],
    ]
    .into_iter()
    .map(|position| EnvironmentSurfaceVertex {
        position,
        uv: [0.0; 2],
    })
    .collect();
    EnvironmentVolumeRecord {
        source_entity: 0,
        priority: 0,
        gravity: None,
        push: None,
        fluid: "water".into(),
        brushes: Vec::new(),
        surface_faces: vec![EnvironmentSurfaceFace {
            texture_index: 0,
            vertices,
        }],
    }
}

fn ndc(matrix: Mat4, p: Vec3) -> Vec3 {
    let clip = matrix * p.extend(1.0);
    clip.truncate() / clip.w
}

#[test]
fn planar_reflection_plane_points_keep_screen_xy_after_resolve_flip() {
    for eye in [
        Vec3::new(0.0, 2.0, 2.0),
        Vec3::new(1.5, 0.1, 3.0),
        Vec3::new(-3.0, 7.0, 1.0),
    ] {
        let view = view(eye);
        let mirror =
            MirrorCamera::new(eye, view, projection(), 0.0, Some(15.0)).expect("valid water view");
        assert_eq!(mirror.eye, Vec3::new(eye.x, -eye.y, eye.z));
        for p in [Vec3::new(-1.0, 0.0, -7.0), Vec3::new(1.5, 0.0, -5.0)] {
            let main = ndc(projection() * view, p);
            let reflected = ndc(mirror.raster_vp, p);
            assert!((main.x + reflected.x).abs() < 1.0e-5);
            assert!((main.y - reflected.y).abs() < 1.0e-5);
        }
    }
}

#[test]
fn planar_reflection_oblique_depth_clips_beneath_biased_water_plane() {
    let eye = Vec3::new(0.0, 2.0, 2.0);
    let mirror = MirrorCamera::new(eye, view(eye), projection(), 0.0, Some(15.0)).unwrap();
    let on = ndc(mirror.raster_vp, Vec3::new(0.0, -PLANAR_CLIP_BIAS, -6.0));
    assert!(on.z.abs() < 1.0e-5, "on-plane depth = {}", on.z);
    let above = ndc(mirror.raster_vp, Vec3::new(0.0, 1.0, -6.0));
    assert!(above.z > 0.0 && above.z < 1.0, "above = {}", above.z);
    let below = ndc(mirror.raster_vp, Vec3::new(0.0, -0.5, -6.0));
    assert!(below.z < 0.0, "below = {}", below.z);
    let unlimited = MirrorCamera::new(eye, view(eye), projection(), 0.0, None).unwrap();
    assert_eq!(
        unlimited.raster_vp, mirror.raster_vp,
        "D changes culling only"
    );
}

#[test]
fn planar_reflection_two_flips_preserve_pipeline_winding() {
    let eye = Vec3::new(0.0, 2.0, 2.0);
    let mirror = MirrorCamera::new(eye, view(eye), projection(), 0.0, None).unwrap();
    let main = projection() * view(eye);
    // Perspective has a negative determinant. Winding correction is positive.
    assert!(mirror.raster_vp.determinant() / main.determinant() > 0.0);
    let reflection = Mat4::from_diagonal(Vec4::new(1.0, -1.0, 1.0, 1.0));
    let flip_x = Mat4::from_diagonal(Vec4::new(-1.0, 1.0, 1.0, 1.0));
    assert!((flip_x * view(eye) * reflection).determinant() > 0.0);
}

// Presets are engine constants: very high and ultra start at half scale with
// D and R off; every lower tier runs no planar layer. Fades and sanitizing
// keep absolute metres.
#[test]
fn planar_reflection_fade_and_settings_use_absolute_metres() {
    assert_eq!(planar_fade(1.0e6, None, 4.0), 1.0);
    assert_eq!(planar_fade(26.0, Some(30.0), 4.0), 1.0);
    assert_eq!(planar_fade(28.0, Some(30.0), 4.0), 0.5);
    assert_eq!(planar_fade(30.0, Some(30.0), 4.0), 0.0);
    assert_eq!(planar_fade(31.0, Some(30.0), 4.0), 0.0);
    for quality in [
        FluidReflectionQuality::VeryHigh,
        FluidReflectionQuality::Ultra,
    ] {
        let preset = planar_preset(quality).expect("a planar tier");
        assert_eq!(preset.scale, 0.5, "{quality:?}");
        assert!(preset.d.is_none() && preset.r.is_none(), "{quality:?}");
        assert_eq!((preset.band_d, preset.band_r), (4.0, 4.0), "{quality:?}");
        assert_eq!(preset.target_extent((801, 601)), (401, 301));
    }
    for quality in [
        FluidReflectionQuality::Off,
        FluidReflectionQuality::Low,
        FluidReflectionQuality::Medium,
        FluidReflectionQuality::High,
    ] {
        assert!(planar_preset(quality).is_none(), "{quality:?}");
    }
    let clean = PlanarPreset {
        scale: f32::NAN,
        d: Some(-1.0),
        r: Some(f32::INFINITY),
        band_d: -1.0,
        band_r: f32::NAN,
    }
    .sanitized();
    assert_eq!(clean.scale, 0.5);
    assert!(clean.d.is_none() && clean.r.is_none());
    assert!(clean.band_d > 0.0 && clean.band_r > 0.0);
    let snapped = PlanarPreset {
        scale: 0.9,
        ..PLANAR_PRESET_START
    }
    .sanitized();
    assert_eq!(snapped.scale, 1.0);
}

#[test]
fn planar_reflection_groups_only_horizontal_reflecting_up_faces() {
    let mut down = water(0.0, 0.0);
    down.surface_faces[0].vertices.reverse();
    let volumes = [
        water(0.0, -3.0),
        water(0.009, 3.0),
        water(0.025, 0.0),
        down,
        water(2.0, 0.0),
    ];
    let candidates = PlanarCandidates::from_volumes(&volumes, &[0, 1, 2, 3]);
    assert_eq!(candidates.planes.len(), 2);
    assert_eq!(candidates.planes[0].faces.len(), 2);
    assert_eq!(candidates.planes[0].faces[1].volume_index, 1);
    assert_eq!(candidates.planes[0].faces[0].area, 16.0);
}

#[test]
fn planar_reflection_selection_hysteresis_and_eye_band() {
    let eye = Vec3::new(0.0, 2.0, 2.0);
    let vp = projection() * view(eye);
    let mut candidates =
        PlanarCandidates::from_volumes(&[water(0.0, 0.0), water(0.1, 0.0)], &[0, 1]);
    let original = select_plane(&candidates, eye, vp, None).unwrap();
    let challenger = 1 - original.candidate_index;
    candidates.planes[challenger].faces[0].area *= original.score
        / select_plane(
            &PlanarCandidates {
                planes: vec![candidates.planes[challenger].clone()],
            },
            eye,
            vp,
            None,
        )
        .unwrap()
        .score
        * 1.2;
    assert_eq!(
        select_plane(&candidates, eye, vp, Some(original))
            .unwrap()
            .candidate_index,
        original.candidate_index
    );
    candidates.planes[challenger].faces[0].area *= 1.1;
    assert_eq!(
        select_plane(&candidates, eye, vp, Some(original))
            .unwrap()
            .candidate_index,
        challenger
    );
    let only = PlanarCandidates::from_volumes(&[water(0.0, 0.0)], &[0]);
    for y in [-1.0, 0.0, FLUID_FOG_BAND * 0.5, FLUID_FOG_BAND] {
        let eye = Vec3::new(0.0, y, 2.0);
        assert!(select_plane(&only, eye, projection() * view(eye), None).is_none());
        assert!(MirrorCamera::new(eye, view(eye), projection(), 0.0, None).is_none());
    }
}

#[test]
fn planar_reflection_seed_cells_intersect_water_and_receiver_sphere() {
    let cell = |x, solid| CellData {
        bounds_min: Vec3::new(x, -1.0, -8.0),
        bounds_max: Vec3::new(x + 1.0, 1.0, -4.0),
        face_start: 0,
        face_count: 0,
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid: solid,
        is_exterior: false,
        is_drawable: false,
    };
    let world = LevelWorld::new_visibility_only(
        vec![
            cell(0.0, false),
            cell(1.0, true),
            cell(20.0, false),
            cell(-2.0, false),
        ],
        Vec::new(),
        CellLocatorChild::Cell(1),
        Vec::new(),
        Vec::new(),
        false,
    )
    .unwrap();
    let candidates = PlanarCandidates::from_volumes(&[water(0.0, 0.0)], &[0]);
    let eye = Vec3::new(0.0, 2.0, 2.0);
    let plane = select_plane(&candidates, eye, projection() * view(eye), None).unwrap();
    let mut seeds = Vec::new();
    collect_mirror_seed_cells(&[0, 1, 2, 3], &world, &plane, eye, None, &mut seeds);
    assert_eq!(seeds, vec![0, 3]);
    collect_mirror_seed_cells(&[0, 1, 2, 3], &world, &plane, eye, Some(1.0), &mut seeds);
    assert!(seeds.is_empty());
}

// The allocation-free cull planes agree with the visibility crate's frustum
// on boxes inside, outside and straddling the mirror's cull frustum.
#[test]
fn planar_reflection_cull_planes_match_the_frustum_extraction() {
    let eye = Vec3::new(0.0, 2.0, 2.0);
    let mirror = MirrorCamera::new(eye, view(eye), projection(), 0.0, Some(20.0)).unwrap();
    let planes = mirror.cull_planes();
    let frustum = extract_frustum_planes(mirror.cull_vp);
    let boxes = [
        (Vec3::new(-0.5, 0.5, -6.5), Vec3::new(0.5, 1.5, -5.5)),
        (Vec3::new(-0.5, 0.5, 30.0), Vec3::new(0.5, 1.5, 31.0)),
        (Vec3::new(-0.5, 0.5, -60.0), Vec3::new(0.5, 1.5, -59.0)),
        (Vec3::new(-50.0, 0.5, -6.5), Vec3::new(-49.0, 1.5, -5.5)),
        (Vec3::new(-50.0, -1.0, -10.0), Vec3::new(50.0, 1.0, -9.0)),
    ];
    let mut inside = 0;
    for (min, max) in boxes {
        let ours = aabb_in_planes(min, max, &planes);
        assert_eq!(ours, bounds_in_frustum(min, max, &frustum), "{min} {max}");
        inside += usize::from(ours);
    }
    assert!(inside >= 2 && inside < boxes.len(), "{inside}");
}
