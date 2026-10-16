// Reflection-probe splitting tests: per-sample sight and parallax triggers, split placement, rounds, both caps and the sky substrate.
// See: context/lib/resource_management.md §5.1 · rendering_pipeline.md §7.3a

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use glam::Vec3;
use log::Level;
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::octahedral::{
    irradiance_interior_texel_direction, irradiance_tile_source_texel,
};
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_TILE_BORDER, REFLECTION_PROBE_TILE_DIMENSION, ReflectionProbesSection,
};
use postretro_test_log_capture::LogCapture;

use super::capture::{
    F16_MIN_POSITIVE, TILE_INTERIOR, distance_bits, encode_distance_tile, inverse_l1_distance,
};
use super::capture_visibility::{CAPTURE_SHADOW_MARGIN, DistanceTile, seen_fraction};
use super::distance_tests::distance_texel;
use super::hit::SceneTracer;
use super::placement::up_facing_faces;
use super::placement_test_fixtures::PlacementFixture;
use super::reflection_probes_test_fixtures::{
    GeometryBuilder, fluid_volume, rect, up_rect, volumes,
};
use super::split::MAX_SPLIT_ROUNDS;
use super::split_triggers::{
    FaceVerdict, PLAYER_EYE_HEIGHT_M, SampleVerdict, failing_faces, hit_is_coarse, hit_is_hidden,
    judge_face, mirror_fan,
};
use super::tests::{
    CEILING, LitFloorRoom, bake, bake_limited, lit_floor_room, lit_floor_room_with, pool,
    with_inputs,
};
use super::{STAGE_ID, StageLimits, bake_or_load_with_limits};
use crate::bake_control::BakeControl;
use crate::bc6h::f16_bits_to_f32;
use crate::cache::StageCache;
use crate::governor::Governor;
use crate::map_data::MapReflectionVolume;
use crate::reporter::StageProgress;

const NO_ROUNDS: StageLimits = StageLimits {
    rounds: 0,
    ..StageLimits::PRODUCTION
};
/// The capture-visibility warning, as it names how much water a probe sees.
const HIDDEN_WATER: &str = "of the water it binds after splitting";

/// Each probe's bound faces, `(volume, face)` ascending.
fn bound(section: &ReflectionProbesSection) -> BTreeMap<u32, Vec<(u32, u32)>> {
    let mut out: BTreeMap<u32, Vec<(u32, u32)>> = BTreeMap::new();
    for b in &section.bindings {
        out.entry(b.probe).or_default().push((b.volume, b.face));
    }
    out
}

/// The fraction of `keys`' water probe `probe` of `section` sees.
fn seen(
    section: &ReflectionProbesSection,
    environment: &EnvironmentVolumesSection,
    probe: usize,
    keys: &[(u32, u32)],
) -> f64 {
    let record = &section.probes[probe];
    seen_fraction(
        &DistanceTile::decode(section, record.layer as usize),
        Vec3::from(record.capture_point),
        keys,
        &up_facing_faces(environment),
    )
}

// ------------------------------------------------------------- L-shaped pool

/// An L-shaped pool at 0.5 m in [`lit_floor_room`]: a narrow west leg
/// (x −2.8..−2, z −2.8..0.6, face 0) turning into a wide north leg
/// (z 0.6..2.8, face 1), 2.72 m² and 12.32 m².
fn l_pool() -> EnvironmentVolumesSection {
    volumes(vec![fluid_volume(
        5,
        &[
            up_rect(-2.8, -2.8, -2.0, 0.6, 0.5),
            up_rect(-2.8, 0.6, 2.8, 2.8, 0.5),
        ],
    )])
}

/// One authored box over the whole L, its capture point moved into the
/// south end of the west leg, (−2.4, −2.4): the faces' area centroid
/// (−0.434, 1.194) plus the offset.
fn l_pool_box() -> MapReflectionVolume {
    MapReflectionVolume {
        source_entity: 6,
        source_label: "entity 6 'reflection_volume'".to_string(),
        min: [-2.9, 0.4, -2.9],
        max: [2.9, 2.9, 2.9],
        capture_offset: Some([-1.966, -3.594]),
    }
}

/// A full-height wall from the east wall to x −2, between the legs: it
/// hides most of the north leg from a point in the west leg.
fn dividing_wall(builder: &mut GeometryBuilder) {
    let (lo, hi) = (Vec3::new(-2.0, 0.0, -0.25), Vec3::new(3.0, 3.0, 0.25));
    for (normal, axis, value) in [
        (Vec3::NEG_X, 0, lo.x),
        (Vec3::NEG_Z, 2, lo.z),
        (Vec3::Z, 2, hi.z),
    ] {
        builder.push_face(normal, &rect(lo, hi, axis, value), 1, lo, 0, [[0.0; 2]; 4]);
    }
}

fn walled_l_room() -> LitFloorRoom {
    let mut room = lit_floor_room_with([255, 255, 255, 255], dividing_wall);
    room.extras.authored = vec![l_pool_box()];
    room
}

/// The percentage a capture-visibility warning names.
fn warned_percent(logs: &LogCapture) -> f64 {
    let message = logs
        .records()
        .into_iter()
        .find(|r| r.message.contains(HIDDEN_WATER))
        .expect("the capture-visibility warning")
        .message;
    message
        .split(" sees ")
        .nth(1)
        .and_then(|rest| rest.split('%').next())
        .and_then(|n| n.parse().ok())
        .expect("the warning names the fraction seen")
}

// S1 (supersedes A2): the dividing wall hides most of the north leg from the
// authored point in the west leg. The north leg leaves as a split probe that
// shares the box, captured 2 mm over its own centroid; each point sees at
// least half of its water, and nothing warns. Held to zero rounds, the one
// probe sees about a third of its water and warns, naming itself and its
// source entity.
#[test]
fn an_l_pool_mostly_hidden_from_its_point_splits_by_sight_and_warns_only_without_rounds() {
    let room = walled_l_room();
    let environment = l_pool();
    let logs = LogCapture::start();
    let section = bake(&room, &environment).unwrap();
    logs.assert_not_logged(Level::Warn, HIDDEN_WATER);
    assert_eq!(section.probes.len(), 2, "{:?}", section.probes);
    let (parent, split) = (&section.probes[0], &section.probes[1]);
    assert_eq!(
        (parent.source_entity, split.source_entity),
        (6, 6),
        "a split probe's source is its parent's"
    );
    assert_eq!(
        (split.box_min, split.box_max),
        (parent.box_min, parent.box_max)
    );
    let point = Vec3::from(parent.capture_point);
    assert!(
        (point - Vec3::new(-2.4, 0.502, -2.4)).length() < 1e-3,
        "{point}"
    );
    // The north leg's centroid, (0, 1.7), 2 mm over its plane.
    let point = Vec3::from(split.capture_point);
    assert!(
        (point - Vec3::new(0.0, 0.502, 1.7)).length() < 1e-3,
        "{point}"
    );
    let bound = bound(&section);
    assert_eq!(bound[&0], vec![(0, 0)]);
    assert_eq!(bound[&1], vec![(0, 1)]);
    for probe in 0..2 {
        let fraction = seen(&section, &environment, probe, &bound[&(probe as u32)]);
        assert!(fraction >= 0.5, "probe {probe} sees {fraction}");
    }

    logs.clear();
    let unsplit = bake_limited(&room, &environment, NO_ROUNDS).unwrap();
    assert_eq!(unsplit.probes.len(), 1);
    assert_eq!(
        unsplit.probes[0], section.probes[0],
        "the parent never moves"
    );
    logs.assert_logged_once(Level::Warn, "probe 0 of entity 6: its capture point");
    let percent = warned_percent(&logs);
    assert!((20.0..45.0).contains(&percent), "{percent}");

    // The same L without the wall, and the plain pool, warn nothing.
    logs.clear();
    let mut open = lit_floor_room([255, 255, 255, 255]);
    open.extras.authored = vec![l_pool_box()];
    assert!(bake(&open, &environment).is_some());
    assert!(bake(&lit_floor_room([255, 255, 255, 255]), &pool()).is_some());
    logs.assert_not_logged(Level::Warn, HIDDEN_WATER);
}

// Splitting decides serially in probe order: the split section is
// byte-identical at one worker and at four.
#[test]
fn a_split_section_is_byte_identical_across_worker_counts() {
    let room = walled_l_room();
    let environment = l_pool();
    let run = |threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| bake(&room, &environment))
            .unwrap()
    };
    let one = run(1);
    assert_eq!(one.probes.len(), 2);
    assert_eq!(one.to_bytes(), run(4).to_bytes());
}

// S4: a probe whose point sees its water within budget never moves: with
// splitting on, the plain pool's section is byte-identical to the unsplit
// one.
#[test]
fn a_probe_that_sees_its_water_within_budget_keeps_its_box_point_and_binding() {
    let room = lit_floor_room([255, 255, 255, 255]);
    let split = bake(&room, &pool()).unwrap();
    assert_eq!(split.probes.len(), 1);
    assert_eq!(
        split.to_bytes(),
        bake_limited(&room, &pool(), NO_ROUNDS).unwrap().to_bytes()
    );
}

// S6: the split probe counts toward the cap. With room for the authored
// probe alone, it drops in the one cap warning naming the count, its water
// returns to the probe it split from, the build succeeds, and the parent,
// stopped by the cap, warns that it still sees under half its water.
#[test]
fn a_split_probe_past_the_cap_drops_and_its_water_returns_to_its_parent() {
    let logs = LogCapture::start();
    let section = bake_limited(
        &walled_l_room(),
        &l_pool(),
        StageLimits {
            rounds: MAX_SPLIT_ROUNDS,
            cap: 1,
        },
    )
    .expect("splitting never refuses the build");
    assert_eq!(section.probes.len(), 1);
    assert_eq!(bound(&section)[&0], vec![(0, 0), (0, 1)]);
    logs.assert_logged_once(
        Level::Warn,
        "1 derived or split probe(s) past the 1-probe cap dropped",
    );
    logs.assert_logged_once(Level::Warn, HIDDEN_WATER);
}

// A warm build replays what the split and the cap warned on the cold one,
// from the cache entry, without capturing.
#[test]
fn a_warm_build_replays_the_cap_and_capture_visibility_warnings() {
    let room = walled_l_room();
    let environment = l_pool();
    let dir = tempfile::tempdir().unwrap();
    let cache = StageCache::new(dir.path()).unwrap();
    let control = BakeControl::unrestricted();
    let limits = StageLimits {
        rounds: MAX_SPLIT_ROUNDS,
        cap: 1,
    };
    let logs = LogCapture::start();
    let (cold, warm) = with_inputs(&room, &environment, |inputs| {
        (
            bake_or_load_with_limits(inputs, Some(&cache), &control, limits).unwrap(),
            bake_or_load_with_limits(inputs, Some(&cache), &control, limits).unwrap(),
        )
    });
    let access = cache.test_access(STAGE_ID);
    assert_eq!((access.writes, access.read_hits), (1, 1));
    assert_eq!(cold, warm);
    for warning in ["past the 1-probe cap dropped", HIDDEN_WATER] {
        let count = logs
            .records()
            .into_iter()
            .filter(|r| r.level == Level::Warn && r.message.contains(warning))
            .count();
        assert_eq!(count, 2, "{warning}");
    }
}

// ------------------------------------------------------------- the test map

/// The manual test map, compiled by the real front end; lit by nothing, so
/// only distance tiles, splitting and the cap are real.
fn test_map() -> PlacementFixture {
    PlacementFixture::from_map_file(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/dev/maps/reflection-probes-test.map"),
    )
}

/// The probe indices of `section` whose source is `entity`, in wire order.
fn probes_of(section: &ReflectionProbesSection, entity: u32) -> Vec<usize> {
    (0..section.probes.len())
        .filter(|&i| section.probes[i].source_entity == entity)
        .collect()
}

// S2 and S4 on `reflection-probes-test.map`. Room B's authored box (entity
// 4) captures west of the pillar; the canal water the pillar hides from that
// point binds split probes that share the box and see it. The box keeps its
// projection box, its point and its claim. The pool probe (entity 2) is
// unchanged by splitting, at the point the shipped compile stores.
#[test]
fn test_map_water_behind_room_bs_pillar_binds_a_split_probe_and_the_pool_probe_is_unchanged() {
    let fixture = test_map();
    let environment = &fixture.environment;
    let unsplit = fixture.bake(NO_ROUNDS).unwrap();
    let split = fixture.bake(StageLimits::PRODUCTION).unwrap();
    let (unsplit_bound, split_bound) = (bound(&unsplit), bound(&split));

    // S4: the pool probe.
    let (pools_before, pools_after) = (probes_of(&unsplit, 2), probes_of(&split, 2));
    assert_eq!((pools_before.len(), pools_after.len()), (1, 1));
    let (pool_before, pool_after) = (pools_before[0], pools_after[0]);
    let (before, after) = (&unsplit.probes[pool_before], &split.probes[pool_after]);
    assert_eq!(
        (before.box_min, before.box_max, before.capture_point),
        (after.box_min, after.box_max, after.capture_point)
    );
    assert_eq!(
        unsplit_bound[&(pool_before as u32)],
        split_bound[&(pool_after as u32)]
    );
    let shipped = Vec3::new(-6.5024, 0.002, -6.5024);
    assert!((Vec3::from(after.capture_point) - shipped).length() < 1e-4);

    // S2: room B.
    let room_b_before = probes_of(&unsplit, 4);
    let room_b_after = probes_of(&split, 4);
    assert_eq!(room_b_before.len(), 1);
    assert!(room_b_after.len() > 1, "room B splits: {room_b_after:?}");
    let (authored_before, authored) = (room_b_before[0], room_b_after[0]);
    let box_point = |s: &ReflectionProbesSection, i: usize| {
        let p = &s.probes[i];
        (p.box_min, p.box_max, p.capture_point)
    };
    assert_eq!(
        box_point(&unsplit, authored_before),
        box_point(&split, authored)
    );
    // The claim: every face the box bound unsplit binds a probe of the box.
    let claim = &unsplit_bound[&(authored_before as u32)];
    let mut held: Vec<(u32, u32)> = room_b_after
        .iter()
        .flat_map(|&i| split_bound[&(i as u32)].clone())
        .collect();
    held.sort_unstable();
    assert_eq!(&held, claim);
    for &i in &room_b_after[1..] {
        let p = &split.probes[i];
        assert_eq!(
            (p.box_min, p.box_max),
            (
                split.probes[authored].box_min,
                split.probes[authored].box_max
            )
        );
    }
    // The water behind the pillar: faces the authored point sees under half.
    let hidden: Vec<(u32, u32)> = claim
        .iter()
        .copied()
        .filter(|&key| seen(&unsplit, environment, authored_before, &[key]) < 0.5)
        .collect();
    assert!(!hidden.is_empty(), "the pillar hides canal water");
    for key in hidden {
        let (&probe, _) = split_bound
            .iter()
            .find(|(_, keys)| keys.contains(&key))
            .expect("hidden water stays bound");
        let probe = probe as usize;
        assert_ne!(probe, authored, "{key:?} left the authored probe");
        assert_eq!(split.probes[probe].source_entity, 4);
        let fraction = seen(&split, environment, probe, &[key]);
        assert!(fraction > 0.95, "{key:?}: its split probe sees {fraction}");
    }
}

// ------------------------------------------------------------- sky substrate

// S8: a finite hit at any in-level distance stores a non-zero distance, a
// texel only one supersample of which hit included; a miss stores 0.
#[test]
fn a_finite_hit_at_any_distance_stores_non_zero_and_a_miss_stores_zero() {
    assert_eq!(distance_bits(0.0), 0);
    let one_far_hit =
        inverse_l1_distance(Some(40_000.0), Vec3::new(1.0, 1.0, 1.0).normalize()) / 4.0;
    for w in [1.0e-12, 1.0e-9, F16_MIN_POSITIVE, one_far_hit, 3.0e-5, 0.5] {
        let bits = distance_bits(w);
        assert_ne!(bits, 0, "{w}");
        assert!(f16_bits_to_f32(bits) > 0.0, "{w}");
    }
    // Through the tile encoder: a texel holding a vanishing mean keeps it.
    let mut rows = vec![vec![0.0f32; TILE_INTERIOR as usize]; TILE_INTERIOR as usize];
    rows[10][20] = 1.0e-10;
    let rows: Vec<&[f32]> = rows.iter().map(Vec::as_slice).collect();
    let bytes = encode_distance_tile(&rows);
    let section = ReflectionProbesSection {
        distances: bytes,
        ..single_layer_section()
    };
    assert!(distance_texel(&section, 0, 21, 11) > 0.0);
    assert_eq!(distance_texel(&section, 0, 22, 11), 0.0);
}

fn single_layer_section() -> ReflectionProbesSection {
    use postretro_level_format::reflection_probes::{
        REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1, ReflectionProbeLightShare, ReflectionProbeRecord,
    };
    ReflectionProbesSection {
        tile_dimension: REFLECTION_PROBE_TILE_DIMENSION,
        tile_border: REFLECTION_PROBE_TILE_BORDER,
        atlas_format: postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H,
        probes: vec![ReflectionProbeRecord {
            source_entity: 0,
            box_min: [0.0; 3],
            box_max: [1.0; 3],
            capture_point: [0.5; 3],
            layer: 0,
            lights: [ReflectionProbeLightShare::UNUSED; 4],
        }],
        bindings: Vec::new(),
        tiles: Vec::new(),
        distance_format: REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1,
        distances: Vec::new(),
    }
}

/// A distance tile storing `w(direction)` at every texel, border included.
fn tile_of(w: impl Fn(Vec3) -> f32) -> DistanceTile {
    let dimension = REFLECTION_PROBE_TILE_DIMENSION;
    let border = REFLECTION_PROBE_TILE_BORDER;
    let mut bytes = Vec::new();
    for y in 0..dimension {
        for x in 0..dimension {
            let [ix, iy] = irradiance_tile_source_texel(x, y, dimension, border);
            let d = Vec3::from(irradiance_interior_texel_direction(
                ix, iy, dimension, border,
            ));
            bytes.extend_from_slice(&f32_to_f16_bits(w(d)).to_le_bytes());
        }
    }
    DistanceTile::from_bytes(dimension, border, &bytes)
}

/// A point at L1 distance `l1` from the origin, `degrees` above +X.
fn at(l1: f32, degrees: f32) -> Vec3 {
    let (sin, cos) = degrees.to_radians().sin_cos();
    let d = Vec3::new(cos, sin, 0.0);
    d * (l1 / d.abs().element_sum())
}

// The hidden-hit test: a hit passes while the tile stores a surface at or
// beyond it (within the 5% margin) within two texels of its direction, and
// fails deep behind a nearer surface.
#[test]
fn a_hit_is_hidden_only_behind_every_surface_stored_near_its_direction() {
    // A surface at L1 2 in every direction.
    let shell = tile_of(|_| 0.5);
    assert!(!hit_is_hidden(&shell, Vec3::ZERO, at(1.5, 30.0)));
    assert!(
        !hit_is_hidden(&shell, Vec3::ZERO, at(2.05, 30.0)),
        "inside the margin"
    );
    assert!(hit_is_hidden(&shell, Vec3::ZERO, at(3.0, 30.0)));
    // A near occluder (L1 1) within 10° of +X before a wall at L1 4.
    let occluded = tile_of(|d| {
        if d.angle_between(Vec3::X) < 10f32.to_radians() {
            1.0
        } else {
            0.25
        }
    });
    assert!(hit_is_hidden(&occluded, Vec3::ZERO, at(4.0, 0.0)));
    assert!(hit_is_hidden(&occluded, Vec3::ZERO, at(4.0, 5.0)));
    assert!(
        !hit_is_hidden(&occluded, Vec3::ZERO, at(4.0, 11.5)),
        "within the budget of the edge"
    );
    assert!(!hit_is_hidden(&occluded, Vec3::ZERO, at(4.0, 20.0)));
}

// S8: the parallax trigger treats a miss as passing. A capture that stored
// no surface anywhere hides no hit; in a room with an open top, fan rays
// that leave through it hit nothing and pass, so the pool's probe never
// splits.
#[test]
fn the_parallax_trigger_passes_capture_misses_and_fan_rays_that_hit_nothing() {
    let sky = tile_of(|_| 0.0);
    for hit in [at(0.5, 10.0), at(40.0, 60.0), Vec3::new(-3.0, 2.0, 7.0)] {
        assert!(!hit_is_hidden(&sky, Vec3::ZERO, hit));
    }

    // The ceiling wound away from the room: the capture culls it, so every
    // ray leaving through the top misses.
    let mut room = lit_floor_room([255, 255, 255, 255]);
    let ceiling = CEILING * 6;
    for triangle in room.geometry.geometry.indices[ceiling..ceiling + 6].chunks_mut(3) {
        triangle.swap(1, 2);
    }
    let section = bake(&room, &pool()).unwrap();
    assert_eq!(
        section.to_bytes(),
        bake_limited(&room, &pool(), NO_ROUNDS).unwrap().to_bytes(),
        "the open-top pool never splits"
    );
    with_inputs(&room, &pool(), |inputs| {
        let tracer = SceneTracer::new(inputs.bvh, inputs.primitives, inputs.geometry);
        let faces = up_facing_faces(&pool());
        let fan = mirror_fan();
        let capture = Vec3::from(section.probes[0].capture_point);
        let tile = DistanceTile::decode(&section, 0);
        let verdict = judge_face(&tracer, &tile, capture, &faces[0], &fan, 100.0);
        assert!(
            verdict
                .samples
                .iter()
                .all(|s| s.seen && s.unserved_rays == 0)
        );
        // The steep rays do leave through the top unhit.
        let origin = capture + Vec3::new(0.5, 0.0, 0.5);
        let misses = fan
            .iter()
            .filter(|&&d| tracer.closest_hit(origin, d, 100.0).is_none())
            .count();
        assert!(misses >= 8, "{misses} fan rays miss");
    });
}

// Sight and the hidden-hit test measure in the fluid shader's units: their
// depth margin is the capture shadow's κ, and the capture texel is the angle
// the shader's capture-shadow fade counts in.
#[test]
fn the_hidden_hit_test_reads_the_tile_in_the_shaders_capture_shadow_units() {
    use postretro_render_cpu::fluid_surface::reflection_search::{
        FLUID_REFLECTION_SHADOW_FADE_TEXELS, FLUID_REFLECTION_SHADOW_MARGIN,
        reflection_shadow_fade_angle,
    };
    assert_eq!(CAPTURE_SHADOW_MARGIN, FLUID_REFLECTION_SHADOW_MARGIN);
    let tile = tile_of(|_| 0.0);
    let shader = reflection_shadow_fade_angle([
        REFLECTION_PROBE_TILE_DIMENSION,
        REFLECTION_PROBE_TILE_BORDER,
    ]) / FLUID_REFLECTION_SHADOW_FADE_TEXELS;
    assert!((tile.texel_angle() - shader).abs() < 1e-7);
}

// Sight means "the shader draws a probe term": h < κ, not h < 0. Water a
// little behind the stored surface, inside the margin, draws a faded term
// and is seen; water past the margin draws none.
#[test]
fn sight_holds_inside_the_capture_shadow_margin_and_fails_past_it() {
    // A surface at L1 2 in every direction: h = l1 / 2 − 1.
    let shell = tile_of(|_| 0.5);
    let seen = |l1: f32| shell.sees(Vec3::ZERO, at(l1, 20.0));
    assert!(seen(1.9), "in front of the surface");
    assert!(seen(2.0 * (1.0 + 0.5 * CAPTURE_SHADOW_MARGIN)), "h = κ/2");
    assert!(!seen(2.0 * (1.0 + 2.0 * CAPTURE_SHADOW_MARGIN)), "h = 2κ");
}

// ------------------------------------------------------------- per-sample fail bit

fn sample(area: f64, seen: bool, unserved_rays: u32) -> SampleVerdict {
    SampleVerdict {
        point: glam::DVec2::ZERO,
        area,
        seen,
        unserved_rays,
        fan_rays: 24,
    }
}

fn face_of(samples: Vec<SampleVerdict>) -> FaceVerdict {
    FaceVerdict {
        key: (0, 0),
        samples,
    }
}

// A sample fails when its capture point does not see it, or when more than a
// quarter of its fan rays are unserved; a face fails when more than half of
// its area fails. The face verdict reads the samples' bits alone: water whose
// rays fail in one half and pass in the other passes, though its face-wide
// unserved share (0.5) is twice the per-sample limit.
#[test]
fn a_face_fails_by_the_area_majority_of_its_failing_samples() {
    assert!(
        !sample(1.0, true, 6).fails(),
        "6 of 24 is not past a quarter"
    );
    assert!(sample(1.0, true, 7).fails());
    assert!(sample(1.0, false, 0).fails(), "unseen water fails by sight");

    let unseen_and_unserved = face_of(vec![
        sample(1.0, false, 0),
        sample(1.0, true, 7),
        sample(1.0, true, 0),
    ]);
    assert!(unseen_and_unserved.fails());
    let half = face_of(vec![sample(1.0, false, 0), sample(1.0, true, 0)]);
    assert!(!half.fails(), "exactly half fails: not over half");
    let area_weighted = face_of(vec![sample(1.5, true, 24), sample(1.0, true, 0)]);
    assert!(area_weighted.fails(), "60% of the area fails");
    let split_rays = face_of(vec![sample(1.0, true, 24), sample(1.0, true, 0)]);
    assert!((split_rays.unserved_share() - 0.5).abs() < 1e-12);
    assert!(!split_rays.fails());

    assert_eq!(
        failing_faces(&[unseen_and_unserved, half, area_weighted, split_rays]),
        vec![true, false, true, false]
    );
}

// ------------------------------------------------------------- coarse hits

/// The fan direction at `elevation` degrees toward +X.
fn toward_x(elevation: f32) -> Vec3 {
    let (sin, cos) = elevation.to_radians().sin_cos();
    Vec3::new(cos, sin, 0.0)
}

/// The capture texels one capture texel at the hit spans from the eye the
/// ray stands for: `|H − C| / (eye / sin e + t)`.
fn texel_span(capture: Vec3, origin: Vec3, elevation: f32, distance: f32) -> f32 {
    let direction = toward_x(elevation);
    let hit = origin + direction * distance;
    hit.distance(capture) / (PLAYER_EYE_HEIGHT_M / direction.y + distance)
}

// The coarse-hit term separates a dock's underside from a pool's coping. A
// plain dock deck 0.5 m over the lake, its capture point 10 m out: under the
// deck the 30° and 60° rays hit the underside within a metre, magnified 2.5
// and 4.3 texels from the eye they stand for, while the grazing 10° ray's
// longer path keeps it under budget. A 0.15 m coping 1.2 m from the pool's
// capture point stays under 0.7 texels at every elevation, from a sample
// 0.3 m off the rim (10°) to one at its foot (60°).
#[test]
fn a_hit_near_the_water_far_from_the_capture_point_is_coarse_and_a_pools_coping_is_not() {
    let lake = Vec3::new(-10.0, 0.002, 0.0);
    let under_deck = Vec3::new(0.0, 0.002, 0.0);
    let deck = 0.5;
    for (elevation, coarse) in [(10.0, false), (30.0, true), (60.0, true)] {
        let distance = (deck - under_deck.y) / toward_x(elevation).y;
        let hit = under_deck + toward_x(elevation) * distance;
        assert_eq!(
            hit_is_coarse(lake, toward_x(elevation), distance, hit),
            coarse,
            "{elevation}°: {} texels",
            texel_span(lake, under_deck, elevation, distance)
        );
    }
    assert!(texel_span(lake, under_deck, 60.0, 0.498 / toward_x(60.0).y) > 4.0);

    let pool = Vec3::new(-1.2, 0.002, 0.0);
    for (elevation, off_rim) in [(10.0, 0.3), (30.0, 0.2), (60.0, 0.05)] {
        let origin = Vec3::new(-off_rim, 0.002, 0.0);
        let distance = off_rim / toward_x(elevation).x;
        let hit = origin + toward_x(elevation) * distance;
        assert!(hit.y < 0.15, "{elevation}° hits the coping");
        let span = texel_span(pool, origin, elevation, distance);
        assert!(span <= 0.7, "{elevation}°: {span} texels");
        assert!(!hit_is_coarse(pool, toward_x(elevation), distance, hit));
    }
}

// The eye the coarse term places on the view ray is the standing player's
// eye above its feet, from the reference player descriptor: the engine has
// no eye-height constant of its own, so the mirror is pinned to that source.
#[test]
fn the_coarse_terms_eye_height_is_the_reference_players_standing_eye() {
    let source = include_str!("../../../../content/dev/scripts/player.ts");
    let line = source
        .lines()
        .find(|line| line.trim_start().starts_with("capsule:"))
        .expect("player.ts declares the player capsule");
    let field = |name: &str| -> f32 {
        line.split(&format!("{name}:"))
            .nth(1)
            .and_then(|rest| rest.split([',', '}']).next())
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or_else(|| panic!("the capsule declares {name}: {line}"))
    };
    let eye = field("halfHeight") + field("radius") + field("eyeHeight");
    assert!((PLAYER_EYE_HEIGHT_M - eye).abs() < 1e-6, "{eye}");
}

// ------------------------------------------------------------- cap before capture

/// Three pools in [`lit_floor_room`], each its own fluid entity and so its
/// own derived probe: 4, 4.14 and 1 m².
fn three_pools() -> EnvironmentVolumesSection {
    volumes(vec![
        fluid_volume(5, &[up_rect(-2.8, -2.8, -0.8, -0.8, 0.5)]),
        fluid_volume(7, &[up_rect(0.5, -2.8, 2.8, -1.0, 0.5)]),
        fluid_volume(9, &[up_rect(-2.8, 1.5, -1.8, 2.5, 0.5)]),
    ])
}

// S9: derived probes past the cap by placement area drop before capture.
// Under a 2-probe cap the stage captures exactly two tiles' rows, the two
// largest pools' probes are kept, the smallest pool binds nothing, and the
// one cap warning names the drop, on the cold build and on the warm one.
#[test]
fn derived_probes_past_the_cap_by_placement_area_are_never_captured() {
    let room = lit_floor_room([255, 255, 255, 255]);
    let environment = three_pools();
    let limits = StageLimits {
        cap: 2,
        ..NO_ROUNDS
    };
    let dir = tempfile::tempdir().unwrap();
    let cache = StageCache::new(dir.path()).unwrap();
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::new(Governor::new(usize::MAX, false)), &progress);
    let logs = LogCapture::start();
    let (cold, warm) = with_inputs(&room, &environment, |inputs| {
        (
            bake_or_load_with_limits(inputs, Some(&cache), &control, limits).unwrap(),
            bake_or_load_with_limits(inputs, Some(&cache), &control, limits).unwrap(),
        )
    });
    assert_eq!(
        progress.completed(),
        2 * TILE_INTERIOR as usize,
        "two probes captured, cold and warm together"
    );
    let section = cold.expect("two probes kept");
    assert_eq!(Some(&section), warm.as_ref());
    let sources: Vec<u32> = section.probes.iter().map(|p| p.source_entity).collect();
    assert_eq!(sources, vec![5, 7]);
    assert!(section.bindings.iter().all(|b| b.volume != 2));
    let count = logs
        .records()
        .into_iter()
        .filter(|r| {
            r.level == Level::Warn
                && r.message
                    .contains("1 derived or split probe(s) past the 2-probe cap dropped")
        })
        .count();
    assert_eq!(count, 2, "one cap warning per build");

    // Under the production cap all three are captured.
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::new(Governor::new(usize::MAX, false)), &progress);
    let all = with_inputs(&room, &environment, |inputs| {
        bake_or_load_with_limits(inputs, None, &control, NO_ROUNDS).unwrap()
    })
    .unwrap();
    assert_eq!(all.probes.len(), 3);
    assert_eq!(progress.completed(), 3 * TILE_INTERIOR as usize);
}
