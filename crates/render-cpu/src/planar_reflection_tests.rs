use super::candidates::{face_binds_probe, group_planes};
use super::*;
use crate::fluid_surface::fog::FLUID_FOG_BAND;
use crate::fluid_surface::reflection::usable_probe_section;
use glam::Vec2;
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H;
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1, REFLECTION_PROBE_LIGHTS_PER_PROBE,
    ReflectionProbeBinding, ReflectionProbeLightShare, ReflectionProbeRecord,
    ReflectionProbesSection, reflection_probe_zero_distances,
};
use postretro_level_loader::{CellData, CellLocatorChild, LevelWorld};
use postretro_visibility::MirrorCropScratch;
use proptest::prelude::*;

/// Relative float comparison for computed values.
fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1.0e-5 * b.abs().max(1.0)
}

fn projection() -> Mat4 {
    glam::camera::rh::proj::directx::perspective(75.0_f32.to_radians(), 1.5, 0.1, 128.0)
}

fn view(eye: Vec3) -> Mat4 {
    glam::camera::rh::view::look_at_mat4(eye, Vec3::new(0.0, 0.0, -8.0), Vec3::Y)
}

/// A 4 × 4 m water face at height `h`, centred at `x`, 4 to 8 m ahead.
fn water(h: f32, x: f32) -> EnvironmentVolumeRecord {
    water_rect(h, Vec2::new(x - 2.0, -8.0), Vec2::new(x + 2.0, -4.0))
}

/// One up-facing water face at height `h` over `min..max` in x and z.
fn water_rect(h: f32, min: Vec2, max: Vec2) -> EnvironmentVolumeRecord {
    EnvironmentVolumeRecord {
        source_entity: 0,
        priority: 0,
        gravity: None,
        push: None,
        fluid: "water".into(),
        brushes: Vec::new(),
        surface_faces: vec![water_face(h, min, max)],
    }
}

fn water_face(h: f32, min: Vec2, max: Vec2) -> EnvironmentSurfaceFace {
    EnvironmentSurfaceFace {
        texture_index: 0,
        vertices: [
            [min.x, h, min.y],
            [min.x, h, max.y],
            [max.x, h, max.y],
            [max.x, h, min.y],
        ]
        .into_iter()
        .map(|position| EnvironmentSurfaceVertex {
            position,
            uv: [0.0; 2],
        })
        .collect(),
    }
}

/// A square water face of `area` m² at height `h`, centred on `centre`.
fn water_square(h: f32, centre: Vec2, area: f32) -> EnvironmentSurfaceFace {
    let half = Vec2::splat(area.sqrt() * 0.5);
    water_face(h, centre - half, centre + half)
}

/// A usable one-probe section binding every surface face of `volumes`.
fn bind_every_face(volumes: &[EnvironmentVolumeRecord]) -> ReflectionProbesSection {
    let mut section = probe_section();
    for (volume, record) in volumes.iter().enumerate() {
        for face in 0..record.surface_faces.len() {
            section.bindings.push(ReflectionProbeBinding {
                volume: volume as u32,
                face: face as u32,
                probe: 0,
            });
        }
    }
    section
}

/// A usable one-probe section with no bindings.
fn probe_section() -> ReflectionProbesSection {
    ReflectionProbesSection {
        tile_dimension: 4,
        tile_border: 1,
        atlas_format: IRRADIANCE_FORMAT_BC6H,
        probes: vec![ReflectionProbeRecord {
            source_entity: 0,
            box_min: [-100.0; 3],
            box_max: [100.0; 3],
            capture_point: [0.0, 0.01, -6.0],
            layer: 0,
            lights: [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE],
        }],
        bindings: Vec::new(),
        tiles: vec![0; 16],
        distance_format: REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1,
        distances: reflection_probe_zero_distances(1, 4),
    }
}

/// `volumes`' candidates with every volume reflecting and every face bound.
fn candidates_of(volumes: &[EnvironmentVolumeRecord]) -> PlanarCandidates {
    let all: Vec<u32> = (0..volumes.len() as u32).collect();
    PlanarCandidates::from_volumes(volumes, &all, Some(&bind_every_face(volumes)))
}

/// One candidate plane as given, the area floor not applied, for scoring a
/// plane alone.
fn plane_alone(volume: EnvironmentVolumeRecord) -> PlanarCandidates {
    PlanarCandidates {
        planes: group_planes(&[volume], |_, _| true),
    }
}

/// Every plane seeded: for selection tests whose seeds are not the subject.
const SEEDED: [bool; 8] = [true; 8];

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
        assert!(
            mirror
                .eye
                .abs_diff_eq(Vec3::new(eye.x, -eye.y, eye.z), 1.0e-5),
            "{}",
            mirror.eye
        );
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
    for (distance, limit, expected) in [
        (1.0e6, None, 1.0),
        (26.0, Some(30.0), 1.0),
        (28.0, Some(30.0), 0.5),
        (30.0, Some(30.0), 0.0),
        (31.0, Some(30.0), 0.0),
    ] {
        let fade = planar_fade(distance, limit, 4.0);
        assert!(close(fade, expected), "{distance} {limit:?}: {fade}");
    }
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
    let planes = group_planes(&volumes, |volume, _| volume < 4);
    assert_eq!(planes.len(), 2);
    assert_eq!(planes[0].faces.len(), 2);
    assert_eq!(planes[0].faces[1].volume_index, 1);
    assert!(close(planes[0].faces[0].area, 16.0));
    assert!(close(planes[0].area, 32.0), "a plane's area sums its faces");
}

// AC 26 (probe gate): planar keeps the reflecting gate's probe requirement
// per face. A face with no bound probe adds nothing to a plane, a plane whose
// faces bind none never becomes a candidate, and no usable probe section
// means no candidate at all, even for a lake far above the area floor.
#[test]
fn planar_candidates_count_only_probe_bound_reflecting_faces() {
    let lake = |h| water_rect(h, Vec2::new(-20.0, -40.0), Vec2::new(20.0, -10.0));
    let mut two_faced = lake(0.0);
    two_faced.surface_faces.push(water_face(
        0.0,
        Vec2::new(30.0, -40.0),
        Vec2::new(70.0, -10.0),
    ));
    let volumes = [two_faced, lake(3.0), lake(-3.0)];
    let mut section = probe_section();
    section.bindings = vec![
        ReflectionProbeBinding {
            volume: 0,
            face: 1,
            probe: 0,
        },
        // A binding naming a probe the section lacks binds nothing.
        ReflectionProbeBinding {
            volume: 1,
            face: 0,
            probe: 7,
        },
        ReflectionProbeBinding {
            volume: 2,
            face: 0,
            probe: 0,
        },
    ];
    let candidates = PlanarCandidates::from_volumes(&volumes, &[0, 1, 2], Some(&section));
    let heights: Vec<f32> = candidates.planes.iter().map(|plane| plane.h).collect();
    assert!(
        heights.len() == 2 && close(heights[0], -3.0) && close(heights[1], 0.0),
        "the unbound lake at 3 m never forms: {heights:?}"
    );
    let plane = &candidates.planes[1];
    assert_eq!(plane.faces.len(), 1);
    assert_eq!(plane.faces[0].face_index, 1, "only the bound face counts");
    assert!(close(plane.area, 1200.0), "{}", plane.area);
    assert_eq!(plane.bounds_min.x, 30.0, "the unbound face widens nothing");

    let unreflecting = PlanarCandidates::from_volumes(&volumes, &[0, 1], Some(&section));
    assert_eq!(unreflecting.planes.len(), 1, "volume 2 does not reflect");
    assert!(
        PlanarCandidates::from_volumes(&volumes, &[0, 1, 2], None)
            .planes
            .is_empty()
    );
    let mut unusable = section.clone();
    unusable.tiles.clear();
    assert!(
        PlanarCandidates::from_volumes(&volumes, &[0, 1, 2], Some(&unusable))
            .planes
            .is_empty(),
        "a section the renderer cannot install binds no face"
    );
    let eye = Vec3::new(0.0, 2.0, 2.0);
    let vp = projection() * view(eye);
    let none = PlanarCandidates::from_volumes(&volumes[1..2], &[0], Some(&section));
    assert!(select_plane(&none, eye, vp, &SEEDED, None).is_none());
}

// AC 25: the floor reads a plane's total area. A puddle just under it never
// becomes a candidate, though alone it outscores the lake in projected solid
// angle; one just over it, or two coplanar faces each under it that sum over
// it, is a candidate and wins.
#[test]
fn planar_selection_respects_the_area_floor() {
    let eye = Vec3::new(0.0, 2.0, 2.0);
    let vp = projection() * view(eye);
    let lake = water_rect(0.0, Vec2::new(-20.0, -40.0), Vec2::new(20.0, -10.0));
    let puddle = |area: f32| EnvironmentVolumeRecord {
        surface_faces: vec![water_square(1.0, Vec2::ZERO, area)],
        ..lake.clone()
    };
    let below = PLANAR_MIN_PLANE_AREA * 0.99;
    let above = PLANAR_MIN_PLANE_AREA * 1.01;
    let score = |volume| {
        select_plane(&plane_alone(volume), eye, vp, &SEEDED, None)
            .expect("in view")
            .score
    };
    assert!(
        score(puddle(below)) > score(lake.clone()),
        "the sub-floor puddle is nearer and larger in projected solid angle"
    );

    let candidates = candidates_of(&[lake.clone(), puddle(below)]);
    assert_eq!(candidates.planes.len(), 1, "the puddle never competes");
    assert!(close(candidates.planes[0].h, 0.0));
    let selected = select_plane(&candidates, eye, vp, &SEEDED, None).unwrap();
    assert!(close(selected.h, 0.0), "the lake is selected");
    assert!(candidates_of(&[puddle(below)]).planes.is_empty());

    let candidates = candidates_of(&[lake.clone(), puddle(above)]);
    assert_eq!(candidates.planes.len(), 2);
    let selected = select_plane(&candidates, eye, vp, &SEEDED, None).unwrap();
    assert!(
        close(selected.h, 1.0),
        "a plane just over the floor is selectable"
    );

    let mut split = puddle(PLANAR_MIN_PLANE_AREA * 0.6);
    split.surface_faces.push(water_square(
        1.0,
        Vec2::new(-1.0, -9.0),
        PLANAR_MIN_PLANE_AREA * 0.6,
    ));
    let candidates = candidates_of(&[lake, split]);
    assert_eq!(
        candidates.planes.len(),
        2,
        "the floor sums the plane's faces, not each face"
    );
}

// AC 26 (O12): seeds filter candidates before scoring and hysteresis. A
// higher-scoring plane in the frustum with no seed cell in the camera's reach
// is not selected and a lower-scoring seeded one is; a held plane that loses
// its seeds yields that frame; with no seeded plane none is selected.
#[test]
fn planar_selection_takes_only_planes_seeded_in_the_cameras_reach() {
    let eye = Vec3::new(0.0, 2.0, 2.0);
    let vp = projection() * view(eye);
    let candidates = candidates_of(&[
        water_rect(0.0, Vec2::new(-20.0, -40.0), Vec2::new(20.0, -10.0)),
        water_rect(0.5, Vec2::new(-3.0, -9.0), Vec2::new(3.0, -5.0)),
    ]);
    let (lake, pool) = (0, 1);
    let both = select_plane(&candidates, eye, vp, &[true, true], None).unwrap();
    assert_eq!(both.candidate_index, lake, "the lake outscores the pool");
    let pool_only = select_plane(&candidates, eye, vp, &[false, true], None).unwrap();
    assert_eq!(pool_only.candidate_index, pool);
    assert!(pool_only.score < both.score);
    // A short flag slice reads the missing planes as unseeded.
    assert_eq!(
        select_plane(&candidates, eye, vp, &[true], None)
            .unwrap()
            .candidate_index,
        lake
    );
    // Held without seeds: yields, whatever its score.
    let held = select_plane(&candidates, eye, vp, &[false, true], Some(both)).unwrap();
    assert_eq!(held.candidate_index, pool);
    assert!(select_plane(&candidates, eye, vp, &[false, false], Some(both)).is_none());
    assert!(select_plane(&candidates, eye, vp, &[], None).is_none());
}

// AC 27: with the eye below a plane, or above it but within its fog band, the
// plane is not selected, before scoring: no mirror camera forms for it, so
// its water reads SSR at the tier. Another plane well below the eye still
// is; with none qualifying, none is selected.
#[test]
fn planar_selection_refuses_eye_below_or_in_band() {
    let eye = Vec3::new(0.0, 2.0, 2.0);
    let vp = projection() * view(eye);
    let low = water_rect(0.0, Vec2::new(-20.0, -40.0), Vec2::new(20.0, -10.0));
    for h in [
        eye.y + 0.5,
        eye.y,
        eye.y - FLUID_FOG_BAND * 0.5,
        // Just inside the band's edge. The exact edge is held by
        // `planar_preparation_selects_no_plane_the_eye_is_below_or_in_the_band_of`
        // in the app crate, where the eye sits exactly `FLUID_FOG_BAND` over
        // a plane at 0; here `eye.y − h` would round.
        eye.y - FLUID_FOG_BAND * 0.999,
    ] {
        let near = water_rect(h, Vec2::new(-6.0, -10.0), Vec2::new(6.0, -2.0));
        let only = candidates_of(std::slice::from_ref(&near));
        assert_eq!(only.planes.len(), 1);
        assert!(
            select_plane(&only, eye, vp, &SEEDED, None).is_none(),
            "plane at {h}: none qualifies, none is selected"
        );
        assert!(MirrorCamera::new(eye, view(eye), projection(), h, None).is_none());
        let held = SelectedPlane {
            candidate_index: 0,
            h,
            bounds_min: only.planes[0].bounds_min,
            bounds_max: only.planes[0].bounds_max,
            score: 1.0,
        };
        assert!(
            select_plane(&only, eye, vp, &SEEDED, Some(held)).is_none(),
            "a held plane the eye drops into does not hold"
        );

        let two = candidates_of(&[low.clone(), near]);
        let held = SelectedPlane {
            candidate_index: 1,
            ..held
        };
        let selected = select_plane(&two, eye, vp, &SEEDED, Some(held)).unwrap();
        assert!(
            close(selected.h, 0.0),
            "plane at {h}: the low plane qualifies"
        );
    }
    let just_above = eye.y - FLUID_FOG_BAND * 1.5;
    let near = water_rect(just_above, Vec2::new(-6.0, -10.0), Vec2::new(6.0, -2.0));
    assert!(select_plane(&candidates_of(&[near]), eye, vp, &SEEDED, None).is_some());
}

// AC 28 (O13), with the floor and seed rules in place: a challenger at or
// below 1.25× the held score does not switch and one above it does;
// hysteresis applies only to a held plane with a non-zero score, so a held
// plane out of the frustum yields that frame.
#[test]
fn planar_selection_hysteresis_holds_at_the_margin_and_yields_when_the_held_plane_scores_zero() {
    assert!(!challenger_switches(4.0, 5.0), "exactly 1.25× holds");
    assert!(challenger_switches(
        4.0,
        f32::from_bits(5.0_f32.to_bits() + 1)
    ));
    assert!(!challenger_switches(4.0, 3.0));

    let eye = Vec3::new(0.0, 2.0, 2.0);
    let vp = projection() * view(eye);
    let mut candidates = candidates_of(&[water(0.0, 0.0), water(0.1, 0.0)]);
    assert_eq!(candidates.planes.len(), 2, "both pools clear the floor");
    let original = select_plane(&candidates, eye, vp, &SEEDED, None).unwrap();
    let challenger = 1 - original.candidate_index;
    let alone = |candidates: &PlanarCandidates| {
        select_plane(
            &PlanarCandidates {
                planes: vec![candidates.planes[challenger].clone()],
            },
            eye,
            vp,
            &SEEDED,
            None,
        )
        .unwrap()
        .score
    };
    let scale = |candidates: &mut PlanarCandidates, ratio: f32| {
        let factor = original.score * ratio / alone(candidates);
        candidates.planes[challenger].faces[0].area *= factor;
    };
    scale(&mut candidates, PLANE_SELECTION_HYSTERESIS * (1.0 - 1.0e-4));
    assert_eq!(
        select_plane(&candidates, eye, vp, &SEEDED, Some(original))
            .unwrap()
            .candidate_index,
        original.candidate_index,
        "at the margin the held plane stays"
    );
    assert_eq!(
        select_plane(&candidates, eye, vp, &SEEDED, None)
            .unwrap()
            .candidate_index,
        challenger,
        "without history the challenger wins"
    );
    scale(&mut candidates, PLANE_SELECTION_HYSTERESIS * (1.0 + 1.0e-3));
    assert_eq!(
        select_plane(&candidates, eye, vp, &SEEDED, Some(original))
            .unwrap()
            .candidate_index,
        challenger,
        "past the margin the challenger switches"
    );

    // O13: the held lake leaves the frustum while a far smaller pool, out of
    // view when the lake was held, scores.
    let candidates = candidates_of(&[
        water_rect(0.0, Vec2::new(-40.0, -15.0), Vec2::new(-10.0, 15.0)),
        water_rect(0.5, Vec2::new(10.0, -2.0), Vec2::new(14.0, 2.0)),
    ]);
    let toward =
        |target: Vec3| projection() * glam::camera::rh::view::look_at_mat4(eye, target, Vec3::Y);
    let left = toward(Vec3::new(-25.0, 0.0, 0.0));
    let right = toward(Vec3::new(12.0, 0.5, 0.0));
    let held = select_plane(&candidates, eye, left, &SEEDED, None).unwrap();
    assert_eq!(held.candidate_index, 0);
    assert!(
        select_plane(&candidates, eye, right, &SEEDED, None)
            .is_some_and(|p| p.candidate_index == 1)
    );
    let ahead = select_plane(&candidates, eye, right, &SEEDED, Some(held)).unwrap();
    assert_eq!(ahead.candidate_index, 1, "the held plane yields that frame");
    assert!(ahead.score < held.score * PLANE_SELECTION_HYSTERESIS);
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
    let candidates = candidates_of(&[water(0.0, 0.0)]);
    let eye = Vec3::new(0.0, 2.0, 2.0);
    let pick = select_plane(&candidates, eye, projection() * view(eye), &SEEDED, None).unwrap();
    let plane = &candidates.planes[pick.candidate_index];
    let mut seeds = Vec::new();
    let mut seeded = vec![true; 3];
    collect_mirror_seed_cells(&[0, 1, 2, 3], &world, plane, eye, None, &mut seeds);
    assert_eq!(seeds, vec![0, 3]);
    mark_seeded_planes(&candidates, &[0, 1, 2, 3], &world, eye, None, &mut seeded);
    assert_eq!(seeded, vec![true], "the filter takes the seeds' own rule");
    for reach in [&[1u32, 2][..], &[][..], &[99][..]] {
        collect_mirror_seed_cells(reach, &world, plane, eye, None, &mut seeds);
        mark_seeded_planes(&candidates, reach, &world, eye, None, &mut seeded);
        assert!(seeds.is_empty() && seeded == [false], "{reach:?}");
    }
    collect_mirror_seed_cells(&[0, 1, 2, 3], &world, plane, eye, Some(1.0), &mut seeds);
    mark_seeded_planes(
        &candidates,
        &[0, 1, 2, 3],
        &world,
        eye,
        Some(1.0),
        &mut seeded,
    );
    assert!(seeds.is_empty());
    assert_eq!(
        seeded,
        vec![false],
        "receiver reach applies to the filter too"
    );
}

/// A non-solid interior cell over `min..max`.
fn open_cell(min: Vec3, max: Vec3) -> CellData {
    CellData {
        bounds_min: min,
        bounds_max: max,
        face_start: 0,
        face_count: 0,
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid: false,
        is_exterior: false,
        is_drawable: false,
    }
}

/// A portal-free world of `cells`, for seed tests.
fn cells_world(cells: Vec<CellData>) -> LevelWorld {
    LevelWorld::new_visibility_only(
        cells,
        Vec::new(),
        CellLocatorChild::Cell(0),
        Vec::new(),
        Vec::new(),
        false,
    )
    .unwrap()
}

// O12, per face: an L-shaped lake's union bounds hold a dry cell that
// overlaps none of its faces. That cell seeds the pool it holds but never
// the lake, so with only it in reach the lower-scoring pool wins; once a cell
// over the lake's water is in reach, the lake wins.
#[test]
fn planar_selection_seeds_a_plane_only_from_cells_over_its_faces() {
    let mut lake = water_rect(0.0, Vec2::new(-20.0, -40.0), Vec2::new(20.0, -30.0));
    lake.surface_faces.push(water_face(
        0.0,
        Vec2::new(-20.0, -30.0),
        Vec2::new(-10.0, -10.0),
    ));
    let pool = water_rect(0.5, Vec2::new(2.0, -24.0), Vec2::new(8.0, -16.0));
    let candidates = candidates_of(&[lake, pool]);
    let (lake, pool) = (0, 1);
    assert_eq!(candidates.planes.len(), 2);
    assert_eq!(candidates.planes[lake].faces.len(), 2);

    let dry = open_cell(Vec3::new(0.0, -1.0, -25.0), Vec3::new(10.0, 1.0, -15.0));
    let plane = &candidates.planes[lake];
    assert!(
        dry.bounds_min.x >= plane.bounds_min.x
            && dry.bounds_max.x <= plane.bounds_max.x
            && dry.bounds_min.z >= plane.bounds_min.z
            && dry.bounds_max.z <= plane.bounds_max.z,
        "the dry cell lies inside the lake's union bounds"
    );
    let wet = open_cell(Vec3::new(-20.0, -1.0, -40.0), Vec3::new(20.0, 1.0, -30.0));
    let world = cells_world(vec![dry, wet]);
    let eye = Vec3::new(0.0, 2.0, 2.0);
    let vp = projection() * view(eye);

    let mut seeded = Vec::new();
    mark_seeded_planes(&candidates, &[0], &world, eye, None, &mut seeded);
    assert_eq!(
        seeded,
        vec![false, true],
        "the dry cell seeds the pool alone"
    );
    let selected = select_plane(&candidates, eye, vp, &seeded, None).unwrap();
    assert_eq!(selected.candidate_index, pool);
    let mut seeds = Vec::new();
    collect_mirror_seed_cells(&[0, 1], &world, plane, eye, None, &mut seeds);
    assert_eq!(seeds, vec![1], "only the wet cell seeds the lake's mirror");

    mark_seeded_planes(&candidates, &[0, 1], &world, eye, None, &mut seeded);
    assert_eq!(seeded, vec![true, true]);
    let both = select_plane(&candidates, eye, vp, &seeded, None).unwrap();
    assert_eq!(both.candidate_index, lake, "the lake outscores the pool");
    assert!(selected.score < both.score);
}

// The cull frustum's near plane is the biased water plane the walk takes:
// with the eye barely above the fog band, content just above the water and
// closer to the mirror eye than the projection's near distance is kept,
// and content below the bias is still culled.
#[test]
fn planar_reflection_cull_planes_keep_content_above_the_water_inside_the_near_distance() {
    let eye = Vec3::new(0.0, FLUID_FOG_BAND + 0.01, 0.0);
    let pitch = 60.0_f32.to_radians();
    let forward = Vec3::new(0.0, -pitch.sin(), -pitch.cos());
    let main_view = glam::camera::rh::view::look_at_mat4(eye, eye + forward, Vec3::Y);
    let mirror = MirrorCamera::new(eye, main_view, projection(), 0.0, None).unwrap();
    let planes = mirror.cull_planes();
    assert_eq!(planes[4], mirror.clip_plane);

    // Along the mirror's forward ray, 0.095 m out: inside the 0.1 m near
    // distance, above the water.
    let mirror_forward = Vec3::new(forward.x, -forward.y, forward.z);
    let above = mirror.eye + mirror_forward * 0.095;
    assert!(above.y > -PLANAR_CLIP_BIAS + 0.005, "{above}");
    let half = Vec3::splat(0.002);
    assert!(
        !aabb_in_planes(
            above - half,
            above + half,
            &view_proj_planes(mirror.cull_vp)
        ),
        "the projection's own near plane would drop it"
    );
    assert!(aabb_in_planes(above - half, above + half, &planes));

    let below = mirror.eye + mirror_forward * 0.05;
    assert!(below.y + 0.002 < -PLANAR_CLIP_BIAS, "{below}");
    assert!(!aabb_in_planes(below - half, below + half, &planes));

    // The side planes still bound it to in front of the eye, and the far
    // plane still culls.
    let behind = mirror.eye + Vec3::new(0.0, 0.5, 3.0);
    assert!(behind.y > 0.0 && (behind - mirror.eye).dot(mirror_forward) < -1.0);
    assert!(!aabb_in_planes(behind - half, behind + half, &planes));
    let far = mirror.eye + mirror_forward * 500.0;
    assert!(!aabb_in_planes(far - half, far + half, &planes));
}

// The visibility crate keeps copies of these values for its mirror sweep and
// cropped headroom test, which cannot depend on render-cpu; they must match
// the values the app's mirror walk takes.
#[test]
fn planar_reflection_constants_match_the_visibility_crate_copies() {
    for (name, ours, copy) in [
        (
            "fog band",
            FLUID_FOG_BAND,
            postretro_visibility::MIRROR_FOG_BAND,
        ),
        (
            "clip bias",
            PLANAR_CLIP_BIAS,
            postretro_visibility::MIRROR_CLIP_BIAS,
        ),
        (
            "crop pad",
            MIRROR_CROP_PAD_RADIANS,
            postretro_visibility::MIRROR_CROP_PAD_RADIANS,
        ),
    ] {
        assert!((ours - copy).abs() <= 1.0e-6, "{name}: {ours} != {copy}");
    }
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
            area: face.area,
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

/// The area floor's measurement (`POSTRETRO_PLANAR_AREA_PRL`, a comma list of
/// baked `.prl` paths): every height group of flat up-facing fluid faces, its
/// total area over every face, and over its probe-bound faces only (what
/// install keeps, before each fluid's reflectivity). Reads the environment
/// volumes and reflection probes sections straight from the file.
#[test]
#[ignore = "measurement over local baked maps; set POSTRETRO_PLANAR_AREA_PRL"]
fn planar_plane_areas_on_baked_maps() {
    use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
    use postretro_level_format::{SectionId, read_container, read_section_data};
    let paths = std::env::var("POSTRETRO_PLANAR_AREA_PRL").expect("set POSTRETRO_PLANAR_AREA_PRL");
    for path in paths.split(',').filter(|path| !path.is_empty()) {
        let mut file = std::fs::File::open(path).expect("open prl");
        let meta = read_container(&mut file).expect("prl container");
        let mut section =
            |id: SectionId| read_section_data(&mut file, &meta, id as u32).expect("section read");
        let volumes = section(SectionId::EnvironmentVolumes)
            .map(|data| EnvironmentVolumesSection::from_bytes(&data).expect("volumes"))
            .map(|section| section.volumes)
            .unwrap_or_default();
        let probes = section(SectionId::ReflectionProbes)
            .map(|data| ReflectionProbesSection::from_bytes(&data).expect("probes"));
        let probes = usable_probe_section(probes.as_ref());
        let faces: usize = volumes.iter().map(|v| v.surface_faces.len()).sum();
        eprintln!(
            "[planar area] {path}: volumes={} surface_faces={faces} usable_probes={}",
            volumes.len(),
            probes.map_or(0, |section| section.probes.len()),
        );
        let bound = group_planes(&volumes, |volume, face| {
            probes.is_some_and(|section| face_binds_probe(section, volume, face))
        });
        for plane in group_planes(&volumes, |_, _| true) {
            let kept = bound
                .iter()
                .find(|b| (b.h - plane.h).abs() <= PLANE_HEIGHT_TOLERANCE)
                .map_or(0.0, |b| b.area);
            let extent = plane.bounds_max - plane.bounds_min;
            eprintln!(
                "[planar area]   h={:.3} faces={} area={:.1} m2 probe_bound={kept:.1} m2 \
                 extent={:.1}x{:.1} m candidate={}",
                plane.h,
                plane.faces.len(),
                plane.area,
                extent.x,
                extent.z,
                kept >= PLANAR_MIN_PLANE_AREA,
            );
        }
    }
}
