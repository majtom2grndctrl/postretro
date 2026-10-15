// Capture completeness: reference state, animated direct and indirect, sdf lights and light share.
// See: context/lib/resource_management.md §5.2 · rendering_pipeline.md §4, §7.3a

use glam::Vec3;
use log::Level as LogLevel;
use postretro_level_format::alpha_lights::{AlphaLightsSection, AlphaShadowType};
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_LOBE_COUNT, ReflectionProbesSection, reflection_probe_lobe_weights,
};
use postretro_test_log_capture::LogCapture;

use super::capture::{TILE_INTERIOR, octahedral_direction};
use super::reference::reference_value;
use super::reflection_probes_test_fixtures::rect;
use super::sdf_lights::SdfLights;
use super::shading::PRODUCTION_INDIRECT_WEIGHT;
use super::share::sample_solid_angle;
use super::tests::{
    CEILING, FLOOR, alpha_light, bake, descriptor, lit_floor_room, lit_floor_room_with, pool,
    room_sh, tile_texel, uniform_block, uniform_delta, uniform_weight_maps,
};
use crate::bc6h::decode_bc6h_rgb_to_f32_rgba;

/// BC6H's error on these smooth values: a few percent.
fn close(a: Vec3, b: Vec3, relative: f32) -> bool {
    (a - b)
        .abs()
        .cmple(b.abs() * relative + Vec3::splat(2e-3))
        .all()
}

/// A strobe whose spline overshoots below zero between its keys (P18).
fn strobe() -> Vec<f32> {
    vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
}

// Acceptance row 12, P18: with one animated light of reference m, the
// capture is the static result plus m × the light's unit contribution.
#[test]
fn one_animated_light_adds_its_reference_times_its_unit_contribution() {
    let mut room = lit_floor_room([255, 255, 255, 255]);
    let light = descriptor([1.0, 0.5, 0.25], strobe());
    let m = Vec3::from(reference_value(&light));
    room.extras.sh.animation_descriptors = vec![light];
    uniform_weight_maps(&mut room, &[(FLOOR, &[(0, 0.5)])]);
    let section = bake(&room, &pool()).unwrap();
    // White floor: static lightmap 1, plus 0.5 unit weight at reference m.
    let down = tile_texel(&section, 0, Vec3::NEG_Y);
    let expected = Vec3::ONE + m * 0.5;
    assert!(close(down, expected, 0.03), "{down} vs {expected}");
    // The ceiling has no animated light and stays black.
    assert!(tile_texel(&section, 0, Vec3::Y).length() < 0.01);

    // A light that starts inactive enters at the same reference.
    room.extras.sh.animation_descriptors[0].start_active = 0;
    assert_eq!(bake(&room, &pool()).unwrap().to_bytes(), section.to_bytes());

    // The light's share entry carries that reference.
    let entry = &section.probes[0].lights[0];
    assert_eq!(entry.roster_index, 0);
    assert_eq!(Vec3::from(entry.reference), m);
}

// Row 12: bounce enters at the production indirect weight, base and delta
// alike, never at full weight.
#[test]
fn a_room_lit_only_by_bounce_reflects_indirect_at_the_production_weight() {
    let mut room = lit_floor_room([255, 255, 255, 255]);
    room.lightmap.blocks[0] = uniform_block(8, 8, 0.0);
    room.extras.sh = room_sh(2.0);
    let light = descriptor([1.0, 1.0, 1.0], vec![0.5]);
    room.extras.sh.animation_descriptors = vec![light];
    room.extras.delta = Some(uniform_delta(&room.extras.sh, 1, 0, [1.0, 0.5, 0.0]));
    let section = bake(&room, &pool()).unwrap();
    let down = tile_texel(&section, 0, Vec3::NEG_Y);
    // Base 2 and the delta (1, 0.5, 0) at reference 0.5, both × 0.33.
    let expected = (Vec3::splat(2.0) + Vec3::new(1.0, 0.5, 0.0) * 0.5) * PRODUCTION_INDIRECT_WEIGHT;
    assert!(close(down, expected, 0.03), "{down} vs {expected}");
    assert!(down.x < 1.0, "full-weight bounce would read {}", 2.5);
    // The delta light owns a share of the bounce it adds.
    let entry = &section.probes[0].lights[0];
    assert_eq!(entry.roster_index, 0);
    let share = entry.shares[1][0];
    assert!((share - 0.5 / 2.5).abs() < 0.02, "{entry:?}");
}

fn sdf_room(occluder: bool) -> super::tests::LitFloorRoom {
    let mut room = lit_floor_room_with([255, 255, 255, 255], |builder| {
        if occluder {
            // A small closed box under the light, above the pool.
            let (lo, hi) = (Vec3::new(-0.3, 1.9, -0.3), Vec3::new(0.3, 2.1, 0.3));
            for (normal, axis, value) in [
                (Vec3::NEG_Y, 1, lo.y),
                (Vec3::Y, 1, hi.y),
                (Vec3::NEG_X, 0, lo.x),
                (Vec3::X, 0, hi.x),
                (Vec3::NEG_Z, 2, lo.z),
                (Vec3::Z, 2, hi.z),
            ] {
                builder.push_face(normal, &rect(lo, hi, axis, value), 1, lo, 0, [[0.0; 2]; 4]);
            }
        }
    });
    room.lightmap.blocks[0] = uniform_block(8, 8, 0.0);
    room.extras.alpha_lights = AlphaLightsSection {
        lights: vec![alpha_light(
            [0.0, 2.8, 0.0],
            [2.0, 2.0, 2.0],
            10.0,
            AlphaShadowType::Sdf,
        )],
    };
    room
}

/// The capture point of [`pool`]'s probe.
const CAPTURE: Vec3 = Vec3::new(0.0, 0.502, 0.0);

// P19: a room lit only by a baked sdf light; a hit occluded from it carries
// none of its direct.
#[test]
fn an_sdf_lit_room_reflects_its_direct_and_an_occluded_hit_carries_none() {
    let toward = |floor: Vec3| floor - CAPTURE;
    let far = Vec3::new(1.5, 0.0, 0.0);
    let room = sdf_room(false);
    let open = bake(&room, &pool()).unwrap();
    let lights = SdfLights::new(&room.extras.alpha_lights, &room.extras.chunk_list);
    let direct = |at: Vec3| lights.direct(&lights.select(at), at, Vec3::Y, |_| true);
    let below = tile_texel(&open, 0, Vec3::NEG_Y);
    assert!(close(below, direct(Vec3::ZERO), 0.03), "{below}");
    assert!(below.x > 0.1, "{below}");

    let shadowed = bake(&sdf_room(true), &pool()).unwrap();
    let below = tile_texel(&shadowed, 0, Vec3::NEG_Y);
    assert!(
        below.length() < 0.01,
        "the occluder shadows the floor below: {below}"
    );
    let lit = tile_texel(&shadowed, 0, toward(far));
    assert!(close(lit, direct(far), 0.05), "{lit} vs {}", direct(far));
}

// Row 13: at a hit reached by more than four sdf lights, the sdf direct is
// the sum over the four the runtime selection picks there.
#[test]
fn a_hit_reached_by_more_than_four_sdf_lights_carries_the_four_the_selection_picks() {
    let mut room = lit_floor_room([255, 255, 255, 255]);
    room.lightmap.blocks[0] = uniform_block(8, 8, 0.0);
    let positions: [([f64; 3], f32); 6] = [
        ([1.0, 2.0, 0.0], 1.0),
        ([-1.0, 2.0, 0.0], 0.9),
        ([0.0, 2.0, 1.0], 0.8),
        ([0.0, 2.0, -1.0], 0.7),
        ([1.5, 1.5, 1.5], 0.6),
        ([-1.5, 1.5, -1.5], 0.5),
    ];
    room.extras.alpha_lights = AlphaLightsSection {
        lights: positions
            .iter()
            .map(|&(origin, brightness)| {
                alpha_light(origin, [brightness; 3], 8.0, AlphaShadowType::Sdf)
            })
            .collect(),
    };
    let lights = SdfLights::new(&room.extras.alpha_lights, &room.extras.chunk_list);
    let selection = lights.select(Vec3::ZERO);
    assert_eq!(selection.lights(), [0, 1, 2, 3]);
    let picked = lights.direct(&selection, Vec3::ZERO, Vec3::Y, |_| true);
    let mut all = selection;
    all.indices = [4, 5, u32::MAX, u32::MAX];
    all.count = 2;
    let rest = lights.direct(&all, Vec3::ZERO, Vec3::Y, |_| true);
    assert!(
        rest.x > 0.1 * picked.x,
        "the two left out would show: {rest} vs {picked}"
    );

    let section = bake(&room, &pool()).unwrap();
    let below = tile_texel(&section, 0, Vec3::NEG_Y);
    assert!(
        close(below, picked, 0.03),
        "{below} vs the selection's {picked}"
    );
}

/// `∫ lobe(d) × tile(d) dΩ` per lobe and channel over a decoded tile's
/// interior, by texel centre.
fn lobe_integrals(
    section: &ReflectionProbesSection,
    layer: usize,
) -> [[f64; 3]; REFLECTION_PROBE_LOBE_COUNT] {
    let dimension = section.tile_dimension;
    let decoded = decode_bc6h_rgb_to_f32_rgba(section.tile(layer), dimension, dimension).unwrap();
    let mut sums = [[0.0f64; 3]; REFLECTION_PROBE_LOBE_COUNT];
    for y in 0..TILE_INTERIOR {
        for x in 0..TILE_INTERIOR {
            let (u, v) = (
                (x as f32 + 0.5) / TILE_INTERIOR as f32,
                (y as f32 + 0.5) / TILE_INTERIOR as f32,
            );
            let omega = sample_solid_angle(u, v, TILE_INTERIOR, 1);
            let lobes = reflection_probe_lobe_weights(octahedral_direction(u, v).to_array());
            let at = (((y + 1) * dimension + x + 1) * 4) as usize;
            for (lobe, w) in lobes.iter().enumerate() {
                for c in 0..3 {
                    sums[lobe][c] += f64::from(w * omega * decoded[at + c]);
                }
            }
        }
    }
    sums
}

fn shares_room() -> super::tests::LitFloorRoom {
    let mut room = lit_floor_room([255, 255, 255, 255]);
    room.extras.sh.animation_descriptors = vec![
        descriptor([1.0, 0.6, 0.2], strobe()),
        descriptor([0.2, 0.4, 1.0], vec![0.8]),
    ];
    // Light 0 on the floor beside its static light; light 1 on the ceiling.
    uniform_weight_maps(&mut room, &[(FLOOR, &[(0, 0.6)]), (CEILING, &[(1, 0.9)])]);
    room
}

// Row 14: each stored share is the light's fraction of the captured light in
// its lobe, measured independently from captures with the other lights off,
// and each entry's reference is the light's reference state.
#[test]
fn stored_shares_are_each_lights_fraction_of_the_captured_light_per_lobe() {
    let room = shares_room();
    let full = bake(&room, &pool()).unwrap();
    let total = lobe_integrals(&full, 0);
    for light in 0..2usize {
        let mut alone = shares_room();
        // Every other light off: brightness 0 gives reference 0.
        for (other, d) in alone.extras.sh.animation_descriptors.iter_mut().enumerate() {
            if other != light {
                d.brightness = vec![0.0];
            }
        }
        // And the static terms off: the floor's lightmap.
        alone.lightmap.blocks[0] = uniform_block(8, 8, 0.0);
        let part = lobe_integrals(&bake(&alone, &pool()).unwrap(), 0);
        let entry = full.probes[0]
            .lights
            .iter()
            .find(|e| e.roster_index == light as u32)
            .expect("both lights are stored");
        assert_eq!(
            entry.reference,
            reference_value(&room.extras.sh.animation_descriptors[light])
        );
        for lobe in 0..REFLECTION_PROBE_LOBE_COUNT {
            for c in 0..3 {
                let expected = if total[lobe][c] > 0.0 {
                    (part[lobe][c] / total[lobe][c]) as f32
                } else {
                    0.0
                };
                let stored = entry.shares[lobe][c];
                assert!(
                    (stored - expected).abs() < 0.03,
                    "light {light} lobe {lobe} channel {c}: stored {stored}, measured {expected}"
                );
            }
        }
    }
    // The up lobe is the ceiling light's; the floor light holds most of
    // every horizontal lobe's red, which the ceiling light barely has.
    let floor = &full.probes[0]
        .lights
        .iter()
        .find(|e| e.roster_index == 0)
        .unwrap();
    let ceiling = &full.probes[0]
        .lights
        .iter()
        .find(|e| e.roster_index == 1)
        .unwrap();
    assert!(ceiling.shares[0][2] > 0.9, "{ceiling:?}");
    assert!(floor.shares[0][0] < 0.05, "{floor:?}");
}

// P6: more animated lights than K; the smallest stays at reference and the
// log names the probe's covered share.
#[test]
fn past_k_lights_the_smallest_stays_out_and_the_log_names_the_covered_share() {
    let mut room = lit_floor_room([255, 255, 255, 255]);
    room.extras.sh.animation_descriptors =
        (0..5).map(|_| descriptor([1.0; 3], Vec::new())).collect();
    let weights: Vec<(u32, f32)> = vec![(0, 0.3), (1, 0.05), (2, 0.4), (3, 0.2), (4, 0.25)];
    uniform_weight_maps(&mut room, &[(FLOOR, &weights)]);
    let logs = LogCapture::start();
    let section = bake(&room, &pool()).unwrap();
    let stored: Vec<u32> = section.probes[0]
        .lights
        .iter()
        .map(|l| l.roster_index)
        .collect();
    assert_eq!(stored, [2, 0, 4, 3]);
    // Covered: every light lights the same floor, so by weight.
    let covered = (0.3 + 0.4 + 0.2 + 0.25) / 1.2;
    logs.assert_logged_once(
        LogLevel::Info,
        &format!(
            "probe 0: 1 animated light(s) past the 4 stored stay at reference; covered share {covered:.3}"
        ),
    );
    logs.assert_logged_once(
        LogLevel::Info,
        &format!("probe 0: covered share {covered:.3} of 5 animated light(s)"),
    );
}

// P20: a lobe that captured no light in a channel stores share 0 there, and
// nothing in the table is NaN.
#[test]
fn a_dark_lobe_stores_zero_shares_and_never_nan() {
    let mut room = lit_floor_room([255, 0, 0, 255]);
    room.lightmap.blocks[0] = uniform_block(8, 8, 0.0);
    room.extras.sh.animation_descriptors = vec![descriptor([1.0, 0.0, 0.0], Vec::new())];
    uniform_weight_maps(&mut room, &[(FLOOR, &[(0, 1.0)])]);
    let section = bake(&room, &pool()).unwrap();
    let entry = &section.probes[0].lights[0];
    assert_eq!(entry.roster_index, 0);
    // The up lobe sees only the unlit ceiling; green and blue see nothing.
    assert_eq!(entry.shares[0], [0.0; 3]);
    for lobe in &entry.shares[1..] {
        assert!((lobe[0] - 1.0).abs() < 1e-5, "{entry:?}");
        assert_eq!((lobe[1], lobe[2]), (0.0, 0.0));
    }
    assert!(
        section.probes[0]
            .lights
            .iter()
            .flat_map(|l| l.shares.iter().flatten().chain(&l.reference))
            .all(|v| v.is_finite())
    );
}
