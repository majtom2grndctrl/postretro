// Capture fill: covering sides read through excluded brushes, chains, seals and per-probe contacts.
// See: context/lib/resource_management.md §5.2 (Exclusion)

use glam::Vec3;
use postretro_level_format::alpha_lights::{AlphaLightsSection, AlphaShadowType};

use super::capture::{SUPERSAMPLE_OFFSETS, TILE_INTERIOR, octahedral_direction, sample_angle};
use super::capture_exclusion::{ProbeExclusion, RayOutcome};
use super::capture_exclusion_test_fixtures::{
    CEILING_DEPTH, CEILING_TEXTURE, CaptureFixture, PILLAR_TEXTURE, ROOM_MAX, ROOM_MIN,
    TRIM_TEXTURE, WALL_TEXTURE, exclusion, pillar, pool_between, pool_entity, pool_entity_to,
    textured_room, toward,
};
use super::capture_exclusion_tests::{IN_BACK_WALL, IN_CEILING, IN_PILLAR, only_probe};
use super::hit::SurfacePoint;
use super::placement::PlacedProbe;
use super::placement_test_fixtures::{engine, engine_box, reflection_volume, textured_box_brush};
use super::reference::reference_value;
use super::shading::{AnimatedTerms, CaptureScene};
use super::tests::{alpha_light, descriptor};
use crate::bake_control::BakeControl;

/// Points on the ceiling inside the pillar's top outline, where CSG cut the
/// contact hole. Texel centres of the ceiling's level-1 mip (odd map units),
/// off the axis planes through the capture point.
fn ceiling_patch() -> Vec<[f64; 3]> {
    let mut points = Vec::new();
    for x in [41.0, 45.0, 49.0, 53.0] {
        for y in [-5.0, -1.0, 3.0] {
            points.push([x, y, ROOM_MAX[2]]);
        }
    }
    points
}

/// Every supersample direction of a probe's tile.
fn tile_directions() -> Vec<Vec3> {
    let interior = TILE_INTERIOR as f32;
    let mut directions = Vec::new();
    for y in 0..TILE_INTERIOR {
        for x in 0..TILE_INTERIOR {
            for [dx, dy] in SUPERSAMPLE_OFFSETS {
                directions.push(octahedral_direction(
                    (x as f32 + 0.5 + dx) / interior,
                    (y as f32 + 0.5 + dy) / interior,
                ));
            }
        }
    }
    directions
}

/// Whether `p` lies in the map-unit box `lo`..`hi` within a millimetre.
fn within(p: Vec3, lo: [f64; 3], hi: [f64; 3]) -> bool {
    let (a, b) = (engine(lo), engine(hi));
    let (min, max) = (a.min(b).as_vec3(), a.max(b).as_vec3());
    p.cmpge(min - 1e-3).all() && p.cmple(max + 1e-3).all()
}

/// Every tile direction from `probe` under `filter`: none misses, and every
/// surface it reads lies inside the map-unit box `lo`..`hi`. Returns the
/// leak-guard count.
fn sweep_stays_inside(
    scene: &CaptureScene<'_>,
    probe: &PlacedProbe,
    filter: &ProbeExclusion<'_>,
    (lo, hi): ([f64; 3], [f64; 3]),
) -> usize {
    let capture = probe.capture_point_vec3();
    let mut guarded = 0;
    for direction in tile_directions() {
        let outcome = scene.trace(capture, direction, filter);
        let distance = outcome
            .distance()
            .unwrap_or_else(|| panic!("{direction} misses"));
        let end = capture + direction * distance;
        assert!(within(end, lo, hi), "{direction} reads {end}: {outcome:?}");
        guarded += usize::from(matches!(outcome, RayOutcome::LeakGuard(_)));
    }
    guarded
}

/// The fill a ray toward the map-unit point `q` reads, asserting it lies on
/// `q`'s plane at `q`'s distance and fills from `brushes`.
fn fill_toward(
    scene: &CaptureScene<'_>,
    capture: Vec3,
    filter: &ProbeExclusion<'_>,
    q: [f64; 3],
    brushes: &[u32],
) -> RayOutcome {
    let (direction, distance) = toward(capture, q);
    let outcome = scene.trace(capture, direction, filter);
    match outcome {
        RayOutcome::Fill(fill) => {
            assert!(brushes.contains(&fill.brush), "{q:?}: {fill:?}");
            assert!(
                (fill.distance - distance).abs() < 1e-3,
                "{q:?}: {} vs {distance}",
                fill.distance
            );
        }
        other => panic!("{q:?} fills, not {other:?}"),
    }
    outcome
}

/// The brush whose hull is the map-unit box `min`..`max`.
fn brush_with_box(room: &CaptureFixture, min: [f64; 3], max: [f64; 3]) -> u32 {
    let (lo, hi) = engine_box(min, max);
    room.world
        .brushes
        .iter()
        .position(|b| (b.aabb.min - lo).length() < 1e-6 && (b.aabb.max - hi).length() < 1e-6)
        .expect("the brush survives parsing") as u32
}

fn texture_index(scene: &CaptureScene<'_>, name: &str) -> u32 {
    scene
        .tracer
        .geometry()
        .texture_names
        .names
        .iter()
        .position(|n| n == name)
        .unwrap_or_else(|| panic!("{name} is in the level")) as u32
}

/// The pool room with a lightmap over the ceiling whose texels vary across
/// it, and base SH varying across the room.
fn lit_pool_room(pillar_top: Option<f64>) -> CaptureFixture {
    let mut room = CaptureFixture::pool_room(pillar_top);
    let ceiling = room.brush_at(IN_CEILING);
    room.lightmap_brush(ceiling, |u, v| 0.6 + 0.3 * u + 0.2 * v);
    room.set_sh(|x, y, z| [0.4 + 0.05 * x as f32 + 0.02 * y as f32 + 0.03 * z as f32; 3]);
    room
}

/// The surface a ray toward `q` reads under `filter`, and its distance.
fn surface_toward(
    scene: &CaptureScene<'_>,
    capture: Vec3,
    filter: &ProbeExclusion<'_>,
    q: Vec3,
) -> (SurfacePoint, f32) {
    let direction = (q - capture).normalize();
    let outcome = scene.trace(capture, direction, filter);
    scene
        .surface_of(&outcome, capture, direction)
        .expect("the room is closed")
}

/// The traced kept ceiling at `q`, past the excluded pillar.
fn kept_ceiling(
    scene: &CaptureScene<'_>,
    capture: Vec3,
    filter: &ProbeExclusion<'_>,
    q: Vec3,
    ceiling: u32,
) -> SurfacePoint {
    let direction = (q - capture).normalize();
    let outcome = scene.trace(capture, direction, filter);
    let RayOutcome::Hit(hit) = outcome else {
        panic!("{q} reads the kept ceiling, not {outcome:?}");
    };
    assert_eq!(scene.brushes.face_brushes[hit.face as usize], Some(ceiling));
    scene.tracer.surface(&hit, capture, direction)
}

fn relative_error(a: Vec3, b: Vec3) -> f32 {
    ((a - b).abs() / b.abs().max(Vec3::splat(1e-6))).max_element()
}

// Directions through the filled ceiling patch read the ceiling's
// material at the ceiling plane's distance, its albedo at the fill point's
// UV, and static direct and SH indirect within 5% of the adjacent kept
// ceiling's; the ceiling spans several faces and its lightmap varies. The
// same fill builds byte-identically twice.
#[test]
fn the_filled_ceiling_patch_reads_the_ceilings_material_and_lighting() {
    let room = lit_pool_room(Some(ROOM_MAX[2]));
    let deleted = lit_pool_room(None);
    let (pillar, ceiling) = (room.brush_at(IN_PILLAR), room.brush_at(IN_CEILING));
    let ceiling_faces = room
        .world
        .face_brushes
        .iter()
        .filter(|&&brush| brush == Some(ceiling))
        .count();
    assert!(
        ceiling_faces > 1,
        "the ceiling around the hole spans {ceiling_faces} face(s)"
    );
    let capture = room.with_scene(|_, probes| only_probe(probes).capture_point_vec3());
    let angle = sample_angle();

    // The same points in the room with the pillar deleted: the intact
    // ceiling's UV and albedo there.
    let intact: Vec<(SurfacePoint, Vec3)> = deleted.with_scene(|scene, probes| {
        let none = exclusion(scene, only_probe(probes), &[]);
        ceiling_patch()
            .into_iter()
            .map(|q| {
                let (point, distance) = surface_toward(scene, capture, &none, engine(q).as_vec3());
                let albedo = scene
                    .materials
                    .sample(
                        point.texture_index,
                        point.uv,
                        distance * angle * point.texel_density,
                    )
                    .albedo;
                (point, albedo)
            })
            .collect()
    });

    room.with_scene(|scene, probes| {
        let probe = only_probe(probes);
        let filter = exclusion(scene, probe, &[pillar]);
        let mut direct_seen = Vec::new();
        for (q, (expected, expected_albedo)) in ceiling_patch().into_iter().zip(&intact) {
            let outcome = fill_toward(scene, capture, &filter, q, &[ceiling]);
            let (direction, _) = toward(capture, q);
            let (point, distance) = scene.surface_of(&outcome, capture, direction).unwrap();
            assert_eq!(point.texture_index, texture_index(scene, CEILING_TEXTURE));
            assert_eq!(point.texture_index, expected.texture_index);
            for axis in 0..2 {
                assert!(
                    (point.uv[axis] - expected.uv[axis]).abs() < 1e-2,
                    "{q:?}: {point:?}"
                );
            }
            let albedo = scene
                .materials
                .sample(
                    point.texture_index,
                    point.uv,
                    distance * angle * point.texel_density,
                )
                .albedo;
            assert_eq!(albedo, *expected_albedo, "{q:?}");

            // The kept ceiling a centimetre outside the hole, beside the
            // face point the fill is lit from, seen past the pillar.
            let centre = engine([48.0, 0.0, ROOM_MAX[2]]).as_vec3();
            let mut outward = point.position - centre;
            outward.y = 0.0;
            let beside = point.position + outward.normalize() * 0.01;
            let adjacent = kept_ceiling(scene, capture, &filter, beside, ceiling);
            let direct = scene
                .lightmap
                .irradiance(point.lightmap_block, point.lightmap_uv)
                .unwrap();
            let kept = scene
                .lightmap
                .irradiance(adjacent.lightmap_block, adjacent.lightmap_uv)
                .unwrap();
            assert!(
                kept.x > 0.5 && relative_error(direct, kept) < 0.05,
                "{q:?}: {direct} vs {kept}"
            );
            let indirect = scene
                .sh
                .indirect_and_slots(point.position, point.normal, None);
            let kept = scene
                .sh
                .indirect_and_slots(adjacent.position, adjacent.normal, None);
            assert!(
                relative_error(indirect, kept) < 0.05,
                "{q:?}: {indirect} vs {kept}"
            );
            direct_seen.push(direct.x);
        }
        let spread = direct_seen.iter().fold(0.0f32, |a, &b| a.max(b))
            - direct_seen.iter().fold(f32::MAX, |a, &b| a.min(b));
        assert!(
            spread > 1e-3,
            "the fills read varying lightmap texels: {direct_seen:?}"
        );

        let bake = || {
            super::capture_section(
                scene,
                probes,
                &[vec![pillar]],
                &[],
                &BakeControl::unrestricted(),
            )
            .unwrap()
            .0
            .to_bytes()
        };
        assert_eq!(bake(), bake());
    });
}

// With an sdf light below the ceiling, the patch's sdf direct is
// within 5% of the adjacent kept ceiling's, and so is its unit term for an
// animated light whose weight map covers the ceiling; an animated light
// whose weight map covers only the pillar takes no share once it is excluded.
#[test]
fn the_filled_patch_takes_the_ceilings_sdf_and_animated_light_and_the_pillars_light_drops_out() {
    let mut room = lit_pool_room(Some(ROOM_MAX[2]));
    let pillar = room.brush_at(IN_PILLAR);
    room.lightmap_brush(pillar, |_, _| 0.5);
    room.weight_lightmap_blocks(&[(1, &[(0, 0.8)]), (2, &[(1, 0.9)])]);
    let descriptors = vec![
        descriptor([1.0; 3], Vec::new()),
        descriptor([1.0, 0.2, 0.2], Vec::new()),
    ];
    let references: Vec<[f32; 3]> = descriptors.iter().map(reference_value).collect();
    room.sh.animation_descriptors = descriptors;
    // In front of the pillar, toward the capture point, so neither the patch's
    // front rim nor the ceiling beside it lies in the pillar's shadow.
    room.alpha_lights = AlphaLightsSection {
        lights: vec![alpha_light(
            [0.0, 2.6, 0.5],
            [2.0; 3],
            10.0,
            AlphaShadowType::Sdf,
        )],
    };
    let ceiling = room.brush_at(IN_CEILING);
    room.with_scene(|scene, probes| {
        let probe = only_probe(probes);
        let capture = probe.capture_point_vec3();
        let filter = exclusion(scene, probe, &[pillar]);
        let sdf_direct = |point: &SurfacePoint| {
            let shadow_origin = point.position + point.normal * 2.0e-3;
            scene.sdf.direct(
                &scene.sdf.select(point.position),
                point.position,
                point.normal,
                |light| scene.tracer.segment_clear(shadow_origin, light),
            )
        };
        let unit = |point: &SurfacePoint, light: u32| {
            let mut terms = AnimatedTerms::default();
            scene.animated_direct.as_ref().unwrap().accumulate(
                point.animated_block,
                point.lightmap_block,
                point.lightmap_uv,
                Vec3::ONE,
                &mut terms,
            );
            terms
                .lights()
                .iter()
                .find(|(l, _)| *l == light)
                .map_or(Vec3::ZERO, |&(_, value)| value)
        };
        // Points whose nearest rim is the hole's front edge.
        for y in [-3.5, 0.5, 3.5] {
            let q = [41.5, y, ROOM_MAX[2]];
            let outcome = fill_toward(scene, capture, &filter, q, &[ceiling]);
            let (direction, _) = toward(capture, q);
            let (point, _) = scene.surface_of(&outcome, capture, direction).unwrap();
            let beside = point.position + Vec3::Z * 0.01;
            let adjacent = kept_ceiling(scene, capture, &filter, beside, ceiling);
            let (fill_sdf, kept_sdf) = (sdf_direct(&point), sdf_direct(&adjacent));
            assert!(
                kept_sdf.x > 0.01 && relative_error(fill_sdf, kept_sdf) < 0.05,
                "{fill_sdf} vs {kept_sdf}"
            );
            let (fill_unit, kept_unit) = (unit(&point, 0), unit(&adjacent, 0));
            assert!(
                kept_unit.x > 0.1 && relative_error(fill_unit, kept_unit) < 0.05,
                "{fill_unit} vs {kept_unit}"
            );
        }

        let stores_pillar_light = |excluded: Vec<u32>| {
            let (section, _) = super::capture_section(
                scene,
                probes,
                &[excluded],
                &references,
                &BakeControl::unrestricted(),
            )
            .unwrap();
            section.probes[0]
                .lights
                .iter()
                .any(|light| light.is_used() && light.roster_index == 1)
        };
        assert!(
            stores_pillar_light(Vec::new()),
            "the unfiltered capture sees the pillar's light"
        );
        assert!(!stores_pillar_light(vec![pillar]));
    });
}

// On the pillar fixture's derived probe, whose box lies on the
// wall behind and the ceiling, no direction through the pillar or its filled
// ceiling patch, above or below the horizon, takes the leak guard.
#[test]
fn no_ray_through_the_pillar_or_its_filled_patches_takes_the_leak_guard() {
    let room = CaptureFixture::pool_room(Some(ROOM_MAX[2]));
    let (pillar, ceiling) = (room.brush_at(IN_PILLAR), room.brush_at(IN_CEILING));
    room.with_scene(|scene, probes| {
        let probe = only_probe(probes);
        let (back, top) = (
            engine([128.0, 0.0, 0.0]).z as f32,
            engine(ROOM_MAX).y as f32,
        );
        assert!((probe.box_min[2] - back).abs() < 1e-3, "{probe:?}");
        assert!((probe.box_max[1] - top).abs() < 1e-3, "{probe:?}");
        let filter = exclusion(scene, probe, &[pillar]);
        let capture = probe.capture_point_vec3();
        for q in ceiling_patch() {
            fill_toward(scene, capture, &filter, q, &[ceiling]);
        }
        let guarded = sweep_stays_inside(scene, probe, &filter, (ROOM_MIN, ROOM_MAX));
        assert_eq!(guarded, 0);
    });
}

// A pillar half a millimetre short of the ceiling still cuts the hole (brush
// CSG treats the gap as contact); the ray leaves the pillar's top before the
// ceiling plane, and fills there within the contact tolerance, at the ceiling
// plane's distance.
#[test]
fn rays_through_a_pillar_half_a_millimetre_short_of_the_ceiling_fill_at_the_ceiling() {
    let room = CaptureFixture::pool_room(Some(ROOM_MAX[2] - 0.0005 / 0.0254));
    let (pillar, ceiling) = (room.brush_at(IN_PILLAR), room.brush_at(IN_CEILING));
    room.with_scene(|scene, probes| {
        let probe = only_probe(probes);
        let filter = exclusion(scene, probe, &[pillar]);
        for q in ceiling_patch() {
            fill_toward(scene, probe.capture_point_vec3(), &filter, q, &[ceiling]);
        }
    });
}

/// From `q`, a map-unit point on the ceiling, back along a ray rising at
/// 10° toward map +x: that ray's origin `back` metres short of `q`.
fn rising_shallowly_to(q: [f64; 3], back: f64) -> Vec3 {
    let elevation = 10f64.to_radians();
    let rise = engine([elevation.cos(), 0.0, elevation.sin()]).normalize();
    (engine(q) - rise * back).as_vec3()
}

// Regression: a shallow ray crossing a sub-millimetre gap took the leak guard.
// Rising at 10°, a ray enters the pillar's side, leaves through its top and
// meets the ceiling plane 2.9 mm along the ray but half a millimetre off the
// pillar, within CSG's contact: it fills there.
#[test]
fn shallow_rays_through_a_pillar_half_a_millimetre_short_of_the_ceiling_fill_at_the_ceiling() {
    let room = CaptureFixture::pool_room(Some(ROOM_MAX[2] - 0.0005 / 0.0254));
    let (pillar, ceiling) = (room.brush_at(IN_PILLAR), room.brush_at(IN_CEILING));
    room.with_scene(|scene, probes| {
        let probe = only_probe(probes);
        let none = exclusion(scene, probe, &[]);
        let filter = exclusion(scene, probe, &[pillar]);
        for (x, y) in [(45.0, -3.5), (48.0, 0.5), (51.0, 3.5)] {
            let q = [x, y, ROOM_MAX[2]];
            let origin = rising_shallowly_to(q, 1.0);
            let (direction, _) = toward(origin, q);
            let first = scene.trace(origin, direction, &none);
            let first = room.face_brush(first.surface().expect("closed room").face);
            assert_eq!(first, Some(pillar), "{q:?} enters the pillar's side");
            fill_toward(scene, origin, &filter, q, &[ceiling]);
        }
    });
}

// A pillar sunk half a metre into the ceiling fills like a flush one; and
// with a second kept brush overlapping the ceiling above the pillar, its
// bottom coplanar with the ceiling's, either map order fills from the ceiling
// surface's faces and never takes the leak guard.
#[test]
fn a_sunk_pillar_and_an_overlapping_kept_brush_both_fill_the_ceiling_patch() {
    let sunk = CaptureFixture::pool_room(Some(ROOM_MAX[2] + 0.5 / 0.0254));
    const { assert!(0.5 / 0.0254 < CEILING_DEPTH) };
    let (sunk_pillar, ceiling) = (sunk.brush_at(IN_PILLAR), sunk.brush_at(IN_CEILING));
    sunk.with_scene(|scene, probes| {
        let probe = only_probe(probes);
        let filter = exclusion(scene, probe, &[sunk_pillar]);
        for q in ceiling_patch() {
            fill_toward(scene, probe.capture_point_vec3(), &filter, q, &[ceiling]);
        }
    });

    let over = textured_box_brush([32.0, -16.0, 128.0], [64.0, 16.0, 140.0], CEILING_TEXTURE);
    for over_first in [true, false] {
        let mut world = textured_room();
        if over_first {
            world.insert(1, over.clone());
        } else {
            world.push(over.clone());
        }
        world.push(pillar(ROOM_MAX[2]));
        let room = CaptureFixture::compile(&world, &[pool_entity()]);
        let pillar = room.brush_at(IN_PILLAR);
        let ceiling = room.brush_at([0.0, 0.0, 150.0]);
        let over = brush_with_box(&room, [32.0, -16.0, 128.0], [64.0, 16.0, 140.0]);
        room.with_scene(|scene, probes| {
            let probe = only_probe(probes);
            let filter = exclusion(scene, probe, &[pillar]);
            for q in ceiling_patch() {
                fill_toward(
                    scene,
                    probe.capture_point_vec3(),
                    &filter,
                    q,
                    &[ceiling, over],
                );
            }
        });
    }
}

// In a closed room, a crate against a wall, a beam under the
// ceiling and a block embedded in a wall are all excluded; no ray misses and
// none reads a surface outside the room.
#[test]
fn a_closed_room_with_every_decor_brush_excluded_never_misses_or_leaves_the_room() {
    let mut world = textured_room();
    world.push(textured_box_brush(
        [112.0, 40.0, 0.0],
        [128.0, 56.0, 16.0],
        TRIM_TEXTURE,
    ));
    world.push(textured_box_brush(
        [-40.0, -64.0, 112.0],
        [-24.0, 64.0, 128.0],
        TRIM_TEXTURE,
    ));
    world.push(textured_box_brush(
        [0.0, -136.0, 48.0],
        [16.0, -120.0, 64.0],
        TRIM_TEXTURE,
    ));
    let room = CaptureFixture::compile(&world, &[pool_entity()]);
    let mut decor = vec![
        room.brush_at([120.0, 48.0, 8.0]),
        room.brush_at([-32.0, 0.5, 120.0]),
        room.brush_at([8.0, -124.0, 56.0]),
    ];
    decor.sort_unstable();
    room.with_scene(|scene, probes| {
        let probe = only_probe(probes);
        let filter = exclusion(scene, probe, &decor);
        sweep_stays_inside(scene, probe, &filter, (ROOM_MIN, ROOM_MAX));
    });
}

// Three excluded pillars in a row, the farthest touching the
// wall and the ceiling, and two touching excluded crates stacked against a
// side wall, the upper touching the ceiling. No ray through them misses or
// leaves the room; rays reaching the far pillar's wall and ceiling patches
// and the crates' ceiling and wall patches fill, through the chain.
#[test]
fn a_chained_closed_room_fills_every_contact_patch_behind_its_excluded_brushes() {
    let mut world = textured_room();
    for (x0, x1, top) in [
        (30.0, 38.0, 64.0),
        (60.0, 68.0, 64.0),
        (112.0, 128.0, 128.0),
    ] {
        world.push(textured_box_brush(
            [x0, -6.0, 0.0],
            [x1, 6.0, top],
            PILLAR_TEXTURE,
        ));
    }
    world.push(textured_box_brush(
        [-24.0, -128.0, 0.0],
        [8.0, -96.0, 64.0],
        TRIM_TEXTURE,
    ));
    world.push(textured_box_brush(
        [-24.0, -128.0, 64.0],
        [8.0, -96.0, 128.0],
        TRIM_TEXTURE,
    ));
    let room = CaptureFixture::compile(&world, &[pool_entity_to(24.0)]);
    let mut excluded: Vec<u32> = [
        [34.0, 0.5, 32.0],
        [64.0, 0.5, 32.0],
        [120.0, 0.5, 64.0],
        [-8.0, -112.0, 32.0],
        [-8.0, -112.0, 96.0],
    ]
    .map(|q| room.brush_at(q))
    .to_vec();
    excluded.sort_unstable();
    let back = room.brush_at(IN_BACK_WALL);
    let ceiling = room.brush_at(IN_CEILING);
    let side = room.brush_at([0.0, -136.0, 64.0]);
    room.with_scene(|scene, probes| {
        let probe = only_probe(probes);
        let capture = probe.capture_point_vec3();
        let filter = exclusion(scene, probe, &excluded);
        // Through all three pillars to the far one's wall patch.
        for q in [[128.0, -2.5, 20.0], [128.0, 1.5, 30.0]] {
            fill_toward(scene, capture, &filter, q, &[back]);
        }
        for q in [
            [120.0, 1.5, 128.0],
            [116.0, -2.5, 128.0],
            [-8.5, -112.5, 128.0],
        ] {
            fill_toward(scene, capture, &filter, q, &[ceiling]);
        }
        // Into the lower crate, up into the upper one, onto the wall.
        fill_toward(scene, capture, &filter, [-8.5, -128.0, 70.0], &[side]);
        sweep_stays_inside(scene, probe, &filter, (ROOM_MIN, ROOM_MAX));
    });
}

// Two stacked excluded crates half a millimetre apart, the upper touching
// the ceiling. Brush CSG treats the gap as contact and cuts both facing
// faces, so a ray leaving the lower crate's top crosses straight into the
// upper one: rays through the lower crate toward the upper crate's ceiling
// patch read the ceiling fill and never take the leak guard.
#[test]
fn rays_through_stacked_crates_half_a_millimetre_apart_fill_the_ceiling_patch() {
    let gap = 0.0005 / 0.0254;
    let split = 112.0;
    let mut world = textured_room();
    world.push(textured_box_brush(
        [-24.0, -128.0, 0.0],
        [8.0, -96.0, split - gap],
        TRIM_TEXTURE,
    ));
    world.push(textured_box_brush(
        [-24.0, -128.0, split],
        [8.0, -96.0, ROOM_MAX[2]],
        TRIM_TEXTURE,
    ));
    let room = CaptureFixture::compile(&world, &[pool_entity()]);
    let lower = room.brush_at([-8.0, -112.0, 32.0]);
    let upper = room.brush_at([-8.0, -112.0, 120.0]);
    let ceiling = room.brush_at(IN_CEILING);
    assert_eq!(
        (
            room.faces_on_plane(lower, 2, [0.0, 0.0, split - gap]),
            room.faces_on_plane(upper, 2, [0.0, 0.0, split]),
        ),
        (0, 0),
        "CSG cuts both crates' facing sides at a half-millimetre gap"
    );
    let mut excluded = vec![lower, upper];
    excluded.sort_unstable();
    room.with_scene(|scene, probes| {
        let probe = only_probe(probes);
        let capture = probe.capture_point_vec3();
        let none = exclusion(scene, probe, &[]);
        let filter = exclusion(scene, probe, &excluded);
        let mut through_lower = 0;
        for x in [-19.5, -8.5, 2.5] {
            for y in [-123.5, -112.5, -101.5] {
                let q = [x, y, ROOM_MAX[2]];
                let (direction, _) = toward(capture, q);
                let first = scene.trace(capture, direction, &none);
                let first = room.face_brush(first.surface().expect("closed room").face);
                assert!(first == Some(lower) || first == Some(upper), "{q:?}");
                through_lower += usize::from(first == Some(lower));
                fill_toward(scene, capture, &filter, q, &[ceiling]);
            }
        }
        assert!(through_lower > 0, "some rays enter the lower crate first");
        let guarded = sweep_stays_inside(scene, probe, &filter, (ROOM_MIN, ROOM_MAX));
        assert_eq!(guarded, 0);
    });
}

// Regression: a shallow ray crossing a sub-millimetre gap between excluded
// crates took the leak guard.
// The upper crate is thin, so a ray rising at 10° that enters the lower
// crate's side and leaves through its top crosses the gap into the upper
// crate, 2.9 mm along the ray, and meets the ceiling inside it: it fills at
// the upper crate's ceiling patch.
#[test]
fn shallow_rays_from_a_crate_into_one_half_a_millimetre_above_fill_the_ceiling_patch() {
    let gap = 0.0005 / 0.0254;
    let split = ROOM_MAX[2] - 4.0;
    let mut world = textured_room();
    world.push(textured_box_brush(
        [-24.0, -128.0, 0.0],
        [8.0, -96.0, split - gap],
        TRIM_TEXTURE,
    ));
    world.push(textured_box_brush(
        [-24.0, -128.0, split],
        [8.0, -96.0, ROOM_MAX[2]],
        TRIM_TEXTURE,
    ));
    let room = CaptureFixture::compile(&world, &[pool_entity()]);
    let lower = room.brush_at([-8.0, -112.0, 32.0]);
    let upper = room.brush_at([-8.0, -112.0, split + 2.0]);
    let ceiling = room.brush_at(IN_CEILING);
    assert_eq!(
        (
            room.faces_on_plane(lower, 2, [0.0, 0.0, split - gap]),
            room.faces_on_plane(upper, 2, [0.0, 0.0, split]),
        ),
        (0, 0),
        "CSG cuts both crates' facing sides at a half-millimetre gap"
    );
    let mut excluded = vec![lower, upper];
    excluded.sort_unstable();
    room.with_scene(|scene, probes| {
        let probe = only_probe(probes);
        let none = exclusion(scene, probe, &[]);
        let filter = exclusion(scene, probe, &excluded);
        for (x, y) in [(-0.5, -123.5), (0.5, -112.5), (4.5, -101.5)] {
            let q = [x, y, ROOM_MAX[2]];
            let origin = rising_shallowly_to(q, 1.5);
            let (direction, _) = toward(origin, q);
            let first = scene.trace(origin, direction, &none);
            let first = room.face_brush(first.surface().expect("closed room").face);
            assert_eq!(first, Some(lower), "{q:?} enters the lower crate's side");
            fill_toward(scene, origin, &filter, q, &[ceiling]);
        }
    });
}

/// A second room behind the back wall: map x 144..400, sharing that wall.
fn room_behind() -> Vec<String> {
    let w = 16.0;
    let (x0, x1) = (144.0, 400.0);
    vec![
        textured_box_brush([x0, -144.0, -w], [x1 + w, 144.0, 0.0], WALL_TEXTURE),
        textured_box_brush([x0, -144.0, 128.0], [x1 + w, 144.0, 144.0], WALL_TEXTURE),
        textured_box_brush([x1, -144.0, 0.0], [x1 + w, 144.0, 128.0], WALL_TEXTURE),
        textured_box_brush([x0, -144.0, 0.0], [x1, -128.0, 128.0], WALL_TEXTURE),
        textured_box_brush([x0, 128.0, 0.0], [x1, 144.0, 128.0], WALL_TEXTURE),
    ]
}

// An outer wall set to `never`, with the exterior behind it and with a second
// room behind it, and a 2 m wall stub between two openings into that second
// room, read their own surfaces and never miss or land outside the projection
// box. A ray that skips an excluded post and then the `never` wall, exterior
// behind, reads the post at its distance.
#[test]
fn excluded_seals_read_their_own_surfaces_and_a_post_before_one_reads_itself() {
    let post = textured_box_brush([96.0, -4.0, 0.0], [104.0, 4.0, 96.0], PILLAR_TEXTURE);
    for second_room in [false, true] {
        let mut world = textured_room();
        world.push(post.clone());
        if second_room {
            world.extend(room_behind());
        }
        let room = CaptureFixture::compile(&world, &[pool_entity()]);
        let wall = room.brush_at(IN_BACK_WALL);
        let post = room.brush_at([100.0, 0.5, 48.0]);
        let mut excluded = vec![wall, post];
        excluded.sort_unstable();
        room.with_scene(|scene, probes| {
            let probe = only_probe(probes);
            let capture = probe.capture_point_vec3();
            let filter = exclusion(scene, probe, &excluded);
            let none = exclusion(scene, probe, &[]);
            for (q, own) in [([128.0, 60.5, 40.0], wall), ([128.0, 1.5, 40.0], post)] {
                let (direction, _) = toward(capture, q);
                let first = *scene.trace(capture, direction, &none).surface().unwrap();
                assert_eq!(room.face_brush(first.face), Some(own), "{q:?}");
                assert_eq!(
                    scene.trace(capture, direction, &filter),
                    RayOutcome::LeakGuard(first),
                    "{q:?} with a second room: {second_room}"
                );
            }
            let (lo, hi) = (Vec3::from(probe.box_min), Vec3::from(probe.box_max));
            for direction in tile_directions() {
                let outcome = scene.trace(capture, direction, &filter);
                let end = capture + direction * outcome.distance().expect("never a miss");
                let inside = end.x >= lo.x - 1e-3
                    && end.x <= hi.x + 1e-3
                    && end.z >= lo.z - 1e-3
                    && end.z <= hi.z + 1e-3
                    && end.y <= hi.y + 1e-3;
                assert!(inside, "{direction} reads {end}: {outcome:?}");
            }
        });
    }

    // The stub between two openings into the second room, under an authored
    // box over the first room, so the box cannot reach through the openings.
    let mut world = textured_room();
    world.remove(3);
    for (y0, y1) in [(-144.0, -80.0), (-40.0, 40.0), (80.0, 144.0)] {
        world.push(textured_box_brush(
            [128.0, y0, 0.0],
            [144.0, y1, 128.0],
            WALL_TEXTURE,
        ));
    }
    world.extend(room_behind());
    let room = CaptureFixture::compile(
        &world,
        &[pool_entity(), reflection_volume("", ROOM_MIN, ROOM_MAX)],
    );
    let stub = room.brush_at([136.0, 0.5, 64.0]);
    room.with_scene(|scene, probes| {
        let probe = only_probe(probes);
        let capture = probe.capture_point_vec3();
        let filter = exclusion(scene, probe, &[stub]);
        let none = exclusion(scene, probe, &[]);
        for q in [
            [128.0, 1.5, 40.0],
            [128.0, -20.5, 90.0],
            [128.0, 30.5, 10.0],
        ] {
            let (direction, _) = toward(capture, q);
            let first = *scene.trace(capture, direction, &none).surface().unwrap();
            assert_eq!(room.face_brush(first.face), Some(stub), "{q:?}");
            assert_eq!(
                scene.trace(capture, direction, &filter),
                RayOutcome::LeakGuard(first)
            );
        }
        for direction in tile_directions() {
            assert!(
                scene.trace(capture, direction, &filter) != RayOutcome::Miss,
                "{direction}"
            );
        }
    });
}

// A kept wall and an excluded trim whose front sides are
// coplanar and partly overlap, the trim straddling the wall's end, read the
// wall in either map order: through the fill with the trim first (it wins
// the coplanar overlap), traced with the wall first; none misses or takes
// the leak guard.
#[test]
fn a_coplanar_trim_over_a_wall_reads_the_wall_in_either_map_order() {
    let (wall_min, wall_max) = ([96.0, -40.0, 0.0], [112.0, 40.0, 96.0]);
    let (trim_min, trim_max) = ([96.0, 32.0, 40.0], [104.0, 56.0, 56.0]);
    for trim_first in [true, false] {
        let mut world = textured_room();
        let (wall, trim) = (
            textured_box_brush(wall_min, wall_max, WALL_TEXTURE),
            textured_box_brush(trim_min, trim_max, TRIM_TEXTURE),
        );
        if trim_first {
            world.extend([trim, wall]);
        } else {
            world.extend([wall, trim]);
        }
        let room = CaptureFixture::compile(&world, &[pool_entity()]);
        let trim = brush_with_box(&room, trim_min, trim_max);
        let wall = brush_with_box(&room, wall_min, wall_max);
        room.with_scene(|scene, probes| {
            let probe = only_probe(probes);
            let capture = probe.capture_point_vec3();
            let filter = exclusion(scene, probe, &[trim]);
            for y in [33.5, 37.5] {
                for z in [44.0, 52.0] {
                    let q = [96.0, y, z];
                    if trim_first {
                        fill_toward(scene, capture, &filter, q, &[wall]);
                        continue;
                    }
                    let (direction, distance) = toward(capture, q);
                    match scene.trace(capture, direction, &filter) {
                        RayOutcome::Hit(hit) => {
                            assert_eq!(room.face_brush(hit.face), Some(wall));
                            assert!((hit.distance - distance).abs() < 1e-3);
                        }
                        other => panic!("{q:?} reads the wall, not {other:?}"),
                    }
                }
            }
            sweep_stays_inside(scene, probe, &filter, (ROOM_MIN, ROOM_MAX));
        });
    }
}

// One map, two probes. A beam the small probe excludes and
// the large one keeps; a crate both exclude hangs under it. Rays through the
// crate read the beam's covering side in the large probe's capture, and
// never the beam's surface or material in the small probe's.
#[test]
fn a_brush_one_probe_excludes_fills_only_in_the_probe_that_keeps_it() {
    let mut world = textured_room();
    world.push(textured_box_brush(
        [64.0, -16.0, 96.0],
        [96.0, 16.0, 112.0],
        TRIM_TEXTURE,
    ));
    world.push(textured_box_brush(
        [72.0, -8.0, 80.0],
        [88.0, 8.0, 96.0],
        PILLAR_TEXTURE,
    ));
    let room = CaptureFixture::compile(
        &world,
        &[pool_between(-96.0, -8.0), pool_between(8.0, 56.0)],
    );
    let beam = room.brush_at([80.0, 12.0, 104.0]);
    let crate_brush = room.brush_at([80.0, 0.5, 88.0]);
    room.with_scene(|scene, probes| {
        assert_eq!(probes.len(), 2, "{probes:?}");
        let trim = texture_index(scene, TRIM_TEXTURE);
        let mut small_set = vec![beam, crate_brush];
        small_set.sort_unstable();
        let small = exclusion(scene, &probes[0], &small_set);
        let large = exclusion(scene, &probes[1], &[crate_brush]);
        for q in [[78.5, -3.5, 96.0], [80.5, 0.5, 96.0], [84.5, 3.5, 96.0]] {
            fill_toward(scene, probes[1].capture_point_vec3(), &large, q, &[beam]);
            let capture = probes[0].capture_point_vec3();
            let (direction, _) = toward(capture, q);
            let outcome = scene.trace(capture, direction, &small);
            match outcome {
                RayOutcome::Fill(fill) => assert_ne!(fill.brush, beam, "{q:?}"),
                RayOutcome::Hit(hit) | RayOutcome::LeakGuard(hit) => {
                    assert_ne!(room.face_brush(hit.face), Some(beam), "{q:?}")
                }
                RayOutcome::Miss => panic!("{q:?} misses"),
            }
            let (point, _) = scene.surface_of(&outcome, capture, direction).unwrap();
            assert_ne!(point.texture_index, trim, "{q:?}: {outcome:?}");
        }
    });
}
