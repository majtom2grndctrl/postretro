// Embedded-emitter tests: the per-light sphere test and its threshold, the flagged early-out, the warning.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use glam::Vec3;
use postretro_test_log_capture::LogCapture;

use super::super::test_fixtures::*;
use super::super::{FOOTPRINT_PROBES, ProbeEarlyOut};
use super::*;
use crate::lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT;
use crate::map_data::DEFAULT_LIGHT_SIZE;

/// Face of the wall the emitters sit beside, `+X` of the receiver: it faces
/// `-X`, toward them, and stands clear of the floor, so it shadows no texel
/// from a light on its near side.
const WALL_FACE_X: f32 = 0.7;

/// Height of the fixture lights above the floor; their default 0.25 m
/// sphere stays clear of it.
const LIGHT_HEIGHT: f64 = 0.4;

/// A 32-texel floor with a slab wall beyond its `+X` edge.
fn walled_floor() -> Fixture {
    let receiver = Receiver::centred(32, 32);
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    scene.aabb(
        Vec3::new(WALL_FACE_X, 0.05, -0.6),
        Vec3::new(WALL_FACE_X + 0.3, 0.9, 0.6),
    );
    Fixture::new(scene, receiver)
}

/// [`walled_floor`] with a plate hovering 0.1 m over its `-X` half, so a
/// light beside the wall shadows a block of floor in umbra: every probe
/// blocked by the plate, not by the emitter's embedded cap.
fn walled_floor_under_plate() -> Fixture {
    let receiver = Receiver::centred(32, 32);
    let mut scene = SceneBuilder::default();
    receiver.add_floor(&mut scene);
    scene.aabb(
        Vec3::new(WALL_FACE_X, 0.05, -0.6),
        Vec3::new(WALL_FACE_X + 0.3, 0.9, 0.6),
    );
    scene.aabb(Vec3::new(-0.6, 0.1, -0.3), Vec3::new(0.2, 0.12, 0.3));
    Fixture::new(scene, receiver)
}

/// A default-size light `gap` metres from the wall's face.
fn beside_wall(gap: f64) -> MapLight {
    default_point([f64::from(WALL_FACE_X) - gap, LIGHT_HEIGHT, 0.0])
}

/// The share of a sphere beyond a plane `gap` from its centre, radius `r`.
fn cap_fraction(gap: f64, r: f64) -> f64 {
    (1.0 - gap / r).max(0.0) * 0.5
}

#[test]
fn the_emitter_test_flags_spheres_that_reach_into_geometry() {
    let fixture = walled_floor();
    let r = f64::from(DEFAULT_LIGHT_SIZE);
    // One direction's share, plus the lattice's own unevenness over a cap.
    let tolerance = 0.02;

    let clear = beside_wall(0.3);
    assert_eq!(fixture.embedded_emitter_fraction(&clear), 0.0);

    for gap in [0.2, 0.1] {
        let light = beside_wall(gap);
        let fraction = f64::from(fixture.embedded_emitter_fraction(&light));
        assert!(
            (fraction - cap_fraction(gap, r)).abs() < tolerance,
            "{gap} m from the wall: {fraction} embedded, cap {}",
            cap_fraction(gap, r)
        );
        let mut spot = light.clone();
        spot.light_type = LightType::Spot;
        spot.cone_direction = Some([0.0, -1.0, 0.0]);
        spot.cone_angle_inner = Some(1.2);
        spot.cone_angle_outer = Some(1.4);
        assert_eq!(
            fixture.embedded_emitter_fraction(&spot),
            fraction as f32,
            "a spot's emitter is the same sphere"
        );
    }

    // Snapped onto the face, or a hair in front of it: half the sphere,
    // never the half that leaves the wall.
    for gap in [0.0, 0.0005] {
        let light = beside_wall(gap);
        let fraction = f64::from(fixture.embedded_emitter_fraction(&light));
        assert!(
            (fraction - 0.5).abs() < tolerance,
            "centre {gap} m from the face: {fraction} embedded"
        );
    }
    // Half a millimetre inside, where the buried-light test keeps it: still
    // flagged, though only directions whose lead starts in front of the face
    // count (those more than 60° off the face plane's normal drop out).
    let inside = f64::from(fixture.embedded_emitter_fraction(&beside_wall(-0.0005)));
    assert!(
        (0.25 - tolerance..=0.5 + tolerance).contains(&inside),
        "centre 0.5 mm inside the face: {inside} embedded"
    );

    // No sphere: a hard light, a sun, and an `sdf` light that bakes no texel.
    let near = [f64::from(WALL_FACE_X) - 0.1, LIGHT_HEIGHT, 0.0];
    assert_eq!(fixture.embedded_emitter_fraction(&hard_point(near)), 0.0);
    let mut sun = hard_sun([0.3, -1.0, 0.0]);
    sun.angular_diameter = 2.0;
    sun.light_size = DEFAULT_LIGHT_SIZE;
    sun.origin = glam::DVec3::from_array(near);
    assert_eq!(fixture.embedded_emitter_fraction(&sun), 0.0);
    let mut sdf = default_point(near);
    sdf.shadow_type = ShadowType::Sdf;
    assert_eq!(fixture.embedded_emitter_fraction(&sdf), 0.0);

    // Keyed by the caller's index; only the embedded light is flagged.
    let embedded = beside_wall(0.2);
    let hard = hard_point(near);
    let set = EmbeddedEmitters::classify(
        [(3, &clear), (5, &embedded), (9, &hard)],
        &fixture.bvh,
        &fixture.primitives,
        &fixture.geometry,
        &BakeControl::unrestricted(),
    );
    assert!(!set.is_embedded(3) && set.is_embedded(5) && !set.is_embedded(9));
    assert!(!set.is_embedded(4), "an unknown key is clear");
    assert_eq!(
        set.fraction(5),
        fixture.embedded_emitter_fraction(&embedded)
    );
}

/// The regression: a default-size light 0.2 m from a wall has a tenth of its
/// sphere inside it. Texels across the floor look at that cap along the
/// light direction, where no probe aims, so the early-out stored 1.0 where
/// the full set reads about 0.9. Flagged, every open texel stores its
/// full-set visibility, through the layer walk and the weight-map bake alike.
#[test]
fn an_embedded_emitter_bakes_every_open_texel_at_its_full_set_visibility() {
    let fixture = walled_floor();
    let light = beside_wall(0.2);
    let count = DEFAULT_AREA_SAMPLE_COUNT;
    let samples = count.max(FOOTPRINT_PROBES as u32) as f32;
    let fraction = fixture.embedded_emitter_fraction(&light);
    assert!(
        fraction > 0.05,
        "the fixture embeds the emitter: {fraction}"
    );

    let baked = fixture.bake(&light, count, true);
    let reference = fixture.footprint_reference(&light, count, |_, _| true);
    let early_out = fixture.footprint_outcomes(&light, count, None);
    let receiver = fixture.receiver;

    let (mut texels, mut sum, mut sum_early) = (0usize, 0.0f64, 0.0f64);
    let mut snapped = 0usize;
    for ty in 0..receiver.height {
        for tx in 0..receiver.width {
            let Some(stored) = baked.get(tx, ty) else {
                continue;
            };
            let full = reference
                .get(tx, ty)
                .expect("the reference covers the bake");
            assert!(
                (stored - full).abs() <= 1.0 / samples,
                "texel ({tx}, {ty}) stores {stored}, its full set reads {full}"
            );
            let old = early_out[(ty * receiver.width + tx) as usize]
                .expect("the early-out walk covers the bake")
                .visibility;
            if old == 1.0 && full < 1.0 {
                snapped += 1;
            }
            texels += 1;
            sum += f64::from(stored);
            sum_early += f64::from(old);
        }
    }
    assert!(texels > 900, "the light reaches the floor: {texels} texels");
    assert!(
        snapped > texels / 4,
        "the fixture must show the defect: the early-out stores 1.0 at {snapped} of \
         {texels} texels whose full set reads less"
    );
    // The open floor reads the share of the emitter outside the wall.
    let mean = sum / texels as f64;
    let expected = 1.0 - f64::from(fraction) * f64::from(samples - 1.0) / f64::from(samples);
    assert!(
        (mean - expected).abs() < 0.02,
        "mean visibility {mean}, expected about {expected}"
    );
    assert!(
        sum_early / texels as f64 > mean + 0.02,
        "the early-out reads bright"
    );

    // The weight-map bake flags the light the same way.
    let animated = fixture.bake_weight_map(&light, count, true);
    let mut compared = 0;
    for ty in 0..receiver.height {
        for tx in 0..receiver.width {
            if let (Some(stat), Some(anim)) = (baked.get(tx, ty), animated.get(tx, ty)) {
                assert!(
                    (stat - anim).abs() <= 1.0 / samples + 1.0e-4,
                    "texel ({tx}, {ty}): static {stat}, animated {anim}"
                );
                compared += 1;
            }
        }
    }
    assert!(
        compared > 900,
        "the weight maps cover the floor: {compared}"
    );
}

/// A flagged light keeps the all-blocked early-out: a texel whose nine probes
/// the plate blocks stores exactly 0 without tracing the rest of its set,
/// as the same light stores it unflagged, while every texel with a clear
/// probe traces its full set. The production walk is exactly the replayed
/// flagged walk.
#[test]
fn an_embedded_emitter_keeps_the_all_blocked_early_out() {
    let fixture = walled_floor_under_plate();
    let light = beside_wall(0.2);
    let count = DEFAULT_AREA_SAMPLE_COUNT;
    let samples = count.max(FOOTPRINT_PROBES as u32) as f32;
    assert!(fixture.has_embedded_emitter(&light));

    let baked = fixture.bake(&light, count, true);
    let flagged = fixture.footprint_outcomes_with(&light, count, None, ProbeEarlyOut::Blocked);
    let unflagged = fixture.footprint_outcomes(&light, count, None);
    let reference = fixture.footprint_reference(&light, count, |_, _| true);
    let receiver = fixture.receiver;

    let (mut blocked, mut open) = (0usize, 0usize);
    for ty in 0..receiver.height {
        for tx in 0..receiver.width {
            let i = (ty * receiver.width + tx) as usize;
            assert_eq!(
                baked.get(tx, ty).map(f32::to_bits),
                flagged[i].map(|outcome| outcome.visibility.to_bits()),
                "texel ({tx}, {ty}): the stage bakes the flagged walk"
            );
            let (Some(flag), Some(plain)) = (flagged[i], unflagged[i]) else {
                continue;
            };
            let all_blocked = plain.early_out && plain.visibility == 0.0;
            assert_eq!(
                flag.early_out, all_blocked,
                "texel ({tx}, {ty}): flagged {flag:?}, unflagged {plain:?}"
            );
            if flag.early_out {
                assert_eq!(flag.visibility.to_bits(), 0.0f32.to_bits());
                blocked += 1;
            } else {
                let full = reference
                    .get(tx, ty)
                    .expect("the reference covers the walk");
                assert!(
                    (flag.visibility - full).abs() <= 1.0 / samples,
                    "texel ({tx}, {ty}) stores {}, its full set reads {full}",
                    flag.visibility
                );
                open += 1;
            }
        }
    }
    assert!(blocked > 100, "the plate puts texels in umbra: {blocked}");
    assert!(open > 400, "the rest of the floor sees the light: {open}");
}

/// A light whose sphere stands clear keeps the probe early-out: its bake is
/// exactly the early-out walk's, and the walk does early-out.
#[test]
fn a_clear_emitter_keeps_the_probe_early_out() {
    let fixture = walled_floor();
    let light = beside_wall(0.3);
    let count = DEFAULT_AREA_SAMPLE_COUNT;
    assert_eq!(fixture.embedded_emitter_fraction(&light), 0.0);

    let baked = fixture.bake(&light, count, true);
    let early_out = fixture.footprint_outcomes(&light, count, None);
    let receiver = fixture.receiver;
    let mut early = 0;
    for ty in 0..receiver.height {
        for tx in 0..receiver.width {
            let outcome = early_out[(ty * receiver.width + tx) as usize];
            assert_eq!(
                baked.get(tx, ty).map(f32::to_bits),
                outcome.map(|outcome| outcome.visibility.to_bits()),
                "texel ({tx}, {ty})"
            );
            early += usize::from(outcome.is_some_and(|outcome| outcome.early_out));
        }
    }
    assert!(early > 900, "open texels early-out: {early}");
}

#[test]
fn the_warning_names_each_embedded_light_with_its_fraction() {
    let fixture = walled_floor();
    let lights = [beside_wall(0.3), beside_wall(0.2)];
    let set = EmbeddedEmitters::classify(
        lights.iter().enumerate(),
        &fixture.bvh,
        &fixture.primitives,
        &fixture.geometry,
        &BakeControl::unrestricted(),
    );
    let labels = [
        "entity 4 'light' at origin \"0 0 0\"",
        "entity 9 'light_spot' at origin \"1016 424 104\"",
    ];

    let capture = LogCapture::start();
    set.warn("static", |i| lights.get(i), |i| labels.get(i).copied());
    let warnings: Vec<String> = capture
        .records()
        .iter()
        .filter(|record| record.level == log::Level::Warn)
        .map(|record| record.message.clone())
        .collect();

    assert_eq!(
        warnings.len(),
        1,
        "only the embedded light warns: {warnings:?}"
    );
    let percent = format!("{:.1}%", set.fraction(1) * 100.0);
    for part in [
        "emitter inside geometry",
        "static light entity 9 'light_spot' at origin \"1016 424 104\"",
        percent.as_str(),
        "0.250 m emitter sphere",
        "Move the light off the surface or shrink `_light_size`",
    ] {
        assert!(
            warnings[0].contains(part),
            "missing {part:?}: {}",
            warnings[0]
        );
    }
}

/// The flag callers read without the fraction stops at the threshold's
/// count, and equals `fraction >= EMBEDDED_EMITTER_MIN_FRACTION` exactly, as
/// the stage's classification does: for slivers of one direction and of one
/// short of the threshold, which are embedded yet unflagged and unwarned,
/// for a cap of exactly the threshold, across the sphere's tangent to the
/// wall, snapped onto and just inside the face, for a spot, and for lights
/// with no sphere.
#[test]
fn the_embedded_flag_is_exactly_a_fraction_at_the_threshold() {
    let fixture = walled_floor();
    let r = f64::from(DEFAULT_LIGHT_SIZE);
    let near = [f64::from(WALL_FACE_X) - 0.1, LIGHT_HEIGHT, 0.0];
    let directions = EMBEDDED_EMITTER_DIRECTIONS as usize;
    let min = EMBEDDED_EMITTER_MIN_DIRECTIONS as usize;

    // A direction's segment reaches the wall exactly when the sphere's
    // reach along it, `r * d.x`, passes the gap; a gap midway between the
    // n-th and (n + 1)-th largest reach embeds exactly n directions.
    let mut reach: Vec<f64> = (0..EMBEDDED_EMITTER_DIRECTIONS)
        .map(|index| {
            let direction =
                probe_sample_direction(&beside_wall(0.1), index, EMBEDDED_EMITTER_DIRECTIONS);
            r * f64::from(direction.x)
        })
        .collect();
    reach.sort_by(|a, b| b.total_cmp(a));
    let embedding = |n: usize| beside_wall((reach[n - 1] + reach[n]) * 0.5);
    for (n, flagged) in [(1, false), (min - 1, false), (min, true), (min + 1, true)] {
        let light = embedding(n);
        assert_eq!(
            fixture.embedded_emitter_fraction(&light),
            n as f32 / directions as f32,
            "{n} directions embedded"
        );
        assert_eq!(
            fixture.has_embedded_emitter(&light),
            flagged,
            "{n} directions"
        );
    }
    assert_eq!(
        fixture.embedded_emitter_fraction(&embedding(min)),
        EMBEDDED_EMITTER_MIN_FRACTION
    );

    let mut lights: Vec<MapLight> = [
        0.4,
        0.3,
        r + 0.001,
        r,
        r - 0.0005,
        r - 0.002,
        r - 0.01,
        0.2,
        0.1,
        0.0005,
        0.0,
        -0.0005,
    ]
    .into_iter()
    .map(beside_wall)
    .collect();
    let mut spot = beside_wall(0.1);
    spot.light_type = LightType::Spot;
    spot.cone_direction = Some([0.0, -1.0, 0.0]);
    spot.cone_angle_inner = Some(1.2);
    spot.cone_angle_outer = Some(1.4);
    let mut sdf = default_point(near);
    sdf.shadow_type = ShadowType::Sdf;
    lights.extend([spot, sdf, hard_point(near)]);
    lights.extend([1, min - 1, min, min + 1].map(embedding));

    let set = EmbeddedEmitters::classify(
        lights.iter().enumerate(),
        &fixture.bvh,
        &fixture.primitives,
        &fixture.geometry,
        &BakeControl::unrestricted(),
    );
    let (mut flagged, mut clear, mut slivers) = (0, 0, 0);
    for (key, light) in lights.iter().enumerate() {
        let fraction = fixture.embedded_emitter_fraction(light);
        let flag = fixture.has_embedded_emitter(light);
        let context = format!(
            "{:?} light at {:?}: fraction {fraction}",
            light.light_type, light.origin
        );
        assert_eq!(flag, fraction >= EMBEDDED_EMITTER_MIN_FRACTION, "{context}");
        assert_eq!(set.is_embedded(key), flag, "{context}");
        assert_eq!(
            set.fraction(key),
            if flag { fraction } else { 0.0 },
            "{context}"
        );
        if flag {
            flagged += 1;
        } else {
            clear += 1;
            slivers += usize::from(fraction > 0.0);
        }
    }
    assert!(
        flagged >= 6 && clear >= 4 && slivers >= 3,
        "{flagged} flagged, {clear} clear, {slivers} of them embedded slivers"
    );

    // Only flagged lights warn.
    let capture = LogCapture::start();
    set.warn("static", |key| lights.get(key), |_| None);
    let warnings = capture
        .records()
        .iter()
        .filter(|record| record.level == log::Level::Warn)
        .count();
    assert_eq!(warnings, flagged);
}
