use super::*;
use glam::Vec2;
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex,
};
use postretro_level_loader::{CellData, CellLocatorChild};
use proptest::prelude::*;

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

/// One crop case: a main camera over one rectangular water face.
#[derive(Debug, Clone, Copy)]
struct CropCase {
    eye: Vec3,
    yaw: f32,
    pitch: f32,
    vfov: f32,
    aspect: f32,
    near: f32,
    h: f32,
    face_centre: Vec2,
    face_half: Vec2,
    d: Option<f32>,
}

impl CropCase {
    fn view(&self) -> Mat4 {
        let look = Vec3::new(
            -self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            -self.yaw.cos() * self.pitch.cos(),
        );
        glam::camera::rh::view::look_at_mat4(self.eye, self.eye + look, Vec3::Y)
    }

    fn projection(&self) -> Mat4 {
        glam::camera::rh::proj::directx::perspective(self.vfov, self.aspect, self.near, 1000.0)
    }

    fn plane(&self) -> PlanarCandidate {
        let (lo, hi) = (
            self.face_centre - self.face_half,
            self.face_centre + self.face_half,
        );
        let bounds_min = Vec3::new(lo.x, self.h, lo.y);
        let bounds_max = Vec3::new(hi.x, self.h, hi.y);
        let face = PlanarFace {
            volume_index: 0,
            face_index: 0,
            vertices: vec![
                bounds_min,
                Vec3::new(lo.x, self.h, hi.y),
                bounds_max,
                Vec3::new(hi.x, self.h, lo.y),
            ],
            bounds_min,
            bounds_max,
            centroid: (bounds_min + bounds_max) * 0.5,
            area: 4.0 * self.face_half.x * self.face_half.y,
        };
        PlanarCandidate {
            h: self.h,
            bounds_min,
            bounds_max,
            faces: vec![face],
        }
    }
}

/// One on-screen sample: the main view's NDC, the face's texel-axis angles
/// (skewed axes allowed) and a direction angle around the pad cone.
type CropSample = (f32, f32, f32, f32, f32);

/// Where along a tilted ray a sample lands: from the water point itself out
/// to effectively infinite.
const CROP_DEPTHS: [f32; 5] = [0.0, 1.0e-3, 0.7, 40.0, 1.0e6];

fn inside_screen(clip: Vec4, slack: f32) -> bool {
    clip.w > 0.0 && clip.x.abs() <= clip.w * (1.0 + slack) && clip.y.abs() <= clip.w * (1.0 + slack)
}

/// AC 18 at `case`: every sampled water point on the main screen, displaced
/// at every depth along each pixel-style tilted ray it can reflect (every
/// displayed offset up to the extreme on both axes, built with the fluid
/// reflection helpers) and along a ray on the pad cone itself, projects
/// inside the cropped mirror cull frustum wherever it projects inside the
/// uncropped one. Returns the water points checked, and whether the crop
/// changed the cull view-projection.
fn check_crop_pad(case: &CropCase, samples: &[CropSample]) -> (usize, bool) {
    use crate::fluid_surface::reflection::{reflection_above_horizon, reflection_mirror};
    use crate::fluid_surface::reflection_style::{FluidTexelAxes, reflection_cell_normal};

    let (view, projection) = (case.view(), case.projection());
    let main_vp = projection * view;
    let Some(camera) = MirrorCamera::new(case.eye, view, projection, case.h, case.d) else {
        return (0, false);
    };
    let plane = case.plane();
    let mut scratch = MirrorCropScratch::default();
    let cropped = camera.cropped_to_plane(&plane, main_vp, &mut scratch);
    if let Some(cropped) = cropped {
        assert_eq!(
            cropped.raster_vp, camera.raster_vp,
            "the crop never moves a pixel"
        );
        assert_eq!(cropped.clip_plane, camera.clip_plane);
        assert_eq!(cropped.eye, camera.eye);
    }
    let inverse = main_vp.inverse();
    let max = MIRROR_CROP_MAX_OFFSET_TEXELS;
    let mut checked = 0;
    for &(sx, sy, axis_u, axis_v, cone) in samples {
        let near = inverse.project_point3(Vec3::new(sx, sy, 0.0));
        let far = inverse.project_point3(Vec3::new(sx, sy, 1.0));
        let dir = far - near;
        if dir.y >= 0.0 {
            continue;
        }
        let t = (case.h - near.y) / dir.y;
        if !(0.0..=1.0).contains(&t) {
            continue;
        }
        let p = near + dir * t;
        if p.x < plane.bounds_min.x
            || p.x > plane.bounds_max.x
            || p.z < plane.bounds_min.z
            || p.z > plane.bounds_max.z
        {
            continue;
        }
        let cropped = cropped
            .unwrap_or_else(|| panic!("water at {p} is on screen but nothing cropped: {case:?}"));
        checked += 1;

        let view_dir = (case.eye - p).normalize();
        let flat = reflection_mirror(view_dir, Vec3::Y);
        assert!(
            flat.abs_diff_eq((p - camera.eye).normalize(), 1.0e-3),
            "the flat ray is the mirror eye's ray through the water point"
        );
        let axes = FluidTexelAxes {
            u: Vec3::new(axis_u.cos(), 0.0, axis_u.sin()),
            v: Vec3::new(axis_v.cos(), 0.0, axis_v.sin()) * 0.37,
        };
        let mut rays = Vec::with_capacity(26);
        for ox in -max..=max {
            for oy in -max..=max {
                let normal = reflection_cell_normal(Vec3::Y, axes, glam::IVec2::new(ox, oy));
                let tilted = reflection_above_horizon(reflection_mirror(view_dir, normal));
                assert!(
                    tilted.angle_between(flat) <= MIRROR_CROP_PAD_RADIANS,
                    "offset ({ox}, {oy}) turns the ray {} rad, past the pad",
                    tilted.angle_between(flat)
                );
                rays.push(tilted);
            }
        }
        // A ray on the pad cone, just inside it.
        let (side_a, side_b) = flat.any_orthonormal_pair();
        let phi = MIRROR_CROP_PAD_RADIANS * 0.999;
        rays.push(flat * phi.cos() + (side_a * cone.cos() + side_b * cone.sin()) * phi.sin());

        for ray in rays {
            for depth in CROP_DEPTHS {
                let x = (p + ray * depth).extend(1.0);
                if !inside_screen(camera.cull_vp * x, 0.0) {
                    // Off the uncropped mirror too: it never held this.
                    continue;
                }
                let clip = cropped.cull_vp * x;
                assert!(
                    inside_screen(clip, 1.0e-3),
                    "a sample at {x} (water {p}, ray {ray}, depth {depth}) lands at NDC {} \
                     outside the crop at {case:?}",
                    clip.truncate() / clip.w
                );
            }
        }
    }
    let changed = cropped.is_some_and(|cropped| cropped.cull_vp != camera.cull_vp);
    (checked, changed)
}

/// Deterministic, evenly spread samples for the fixed-pose test.
fn crop_samples(n: usize) -> Vec<CropSample> {
    (0..n)
        .map(|i| {
            let a = i as f32 * 0.618_034;
            let b = i as f32 * 0.414_214;
            (
                (a.fract() * 2.0 - 1.0) * 0.999,
                (b.fract() * 2.0 - 1.0) * 0.999,
                a * 7.0,
                a * 7.0 + 1.0 + b,
                b * 11.0,
            )
        })
        .collect()
}

// AC 18, fixed poses: a lake under the eye that the near plane clips, and a
// distant pool whose crop is strictly inside the screen.
#[test]
fn mirror_crop_pad_holds_for_a_near_clipped_lake_and_a_distant_pool() {
    let lake = CropCase {
        eye: Vec3::new(0.0, 1.7, 0.0),
        yaw: 0.3,
        pitch: -0.4,
        vfov: 1.2,
        aspect: 16.0 / 9.0,
        near: 0.1,
        h: 0.0,
        face_centre: Vec2::ZERO,
        face_half: Vec2::splat(80.0),
        d: None,
    };
    let (checked, _) = check_crop_pad(&lake, &crop_samples(400));
    assert!(checked > 100, "lake checked {checked}");

    // Seen from 10 m up, a 2 m pool near the screen centre: its padded
    // bounds span under half the screen's width.
    let pool = CropCase {
        eye: Vec3::new(0.0, 10.0, 0.0),
        face_centre: Vec2::new(0.0, -8.0),
        face_half: Vec2::splat(1.0),
        pitch: -0.9,
        yaw: 0.0,
        d: Some(25.0),
        ..lake
    };
    let (checked, changed) = check_crop_pad(&pool, &crop_samples(6000));
    assert!(checked > 5, "pool checked {checked}");
    assert!(changed, "a small distant pool crops the mirror");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    // AC 18: over sampled eyes, views and plane extents, near-clipped planes
    // included, every tilted reflection a water point on screen can sample
    // lands inside the cropped mirror frustum.
    #[test]
    fn mirror_crop_pad_contains_every_tilted_reflection(
        eye in (-30.0f32..30.0, 0.09f32..40.0, -30.0f32..30.0),
        angles in (-std::f32::consts::PI..std::f32::consts::PI, -1.5f32..0.6),
        lens in (0.6f32..1.9, 1.0f32..2.4, 0.05f32..0.5),
        h in -20.0f32..20.0,
        face in (-60.0f32..60.0, -60.0f32..60.0, 0.2f32..200.0, 0.2f32..200.0),
        d in prop::option::of(5.0f32..200.0),
        samples in prop::collection::vec(
            (-0.999f32..0.999, -0.999f32..0.999, 0.0f32..6.3, 0.0f32..6.3, 0.0f32..6.3),
            64,
        ),
    ) {
        let case = CropCase {
            eye: Vec3::new(eye.0, h + eye.1, eye.2),
            yaw: angles.0,
            pitch: angles.1,
            vfov: lens.0,
            aspect: lens.1,
            near: lens.2,
            h,
            face_centre: Vec2::new(face.0, face.1),
            face_half: Vec2::new(face.2, face.3),
            d,
        };
        check_crop_pad(&case, &samples);
    }
}

// The pad bounds the tilt as derived: twice the largest normal tilt plus the
// horizon clamp's two moves, and not much more.
#[test]
fn mirror_crop_pad_covers_twice_the_largest_normal_tilt_and_the_horizon_clamp() {
    let tilt = (2 * MIRROR_CROP_MAX_OFFSET_TEXELS) as f32 * FLUID_REFLECTION_PIXEL_TILT;
    let exact = 2.0 * tilt.atan() + 2.0 * FLUID_REFLECTION_RAY_MIN_Y.asin();
    assert_eq!(MIRROR_CROP_MAX_OFFSET_TEXELS, 2);
    assert!(
        MIRROR_CROP_PAD_RADIANS >= exact,
        "{MIRROR_CROP_PAD_RADIANS} < {exact}"
    );
    assert!(
        MIRROR_CROP_PAD_RADIANS < exact * 1.05,
        "the pad stays tight"
    );
}
