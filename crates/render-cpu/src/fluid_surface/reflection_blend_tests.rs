// SPIKE honesty gates for the reflection blend-cost candidates' CPU reference.
// See: context/plans/drafts/reflection-probe-seams--blend-cost-spike/index.md

use super::*;
use crate::fluid_surface::reflection::reflection_fragment;
use crate::fluid_surface::reflection_style::{
    FLUID_REFLECTION_STYLE_PIXEL, FLUID_REFLECTION_STYLE_SMOOTH, FluidTexelAxes,
};
use crate::frame_uniforms::LightTermMask;
use crate::sh_compose::AnimatedLightScaleDescriptor;
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_TILE_BORDER, REFLECTION_PROBE_TILE_DIMENSION, ReflectionProbeLightShare,
};

const TILE: [u32; 2] = [
    REFLECTION_PROBE_TILE_DIMENSION,
    REFLECTION_PROBE_TILE_BORDER,
];

/// Three abutting 8 × 24 m boxes over one pool plane, each with its capture
/// point 2 mm above the water, and each probe binding four curve lights.
fn three_probes() -> Vec<ReflectionProbeRecord> {
    (0..3)
        .map(|i| {
            let x0 = -12.0 + 8.0 * i as f32;
            ReflectionProbeRecord {
                source_entity: i,
                box_min: [x0, -0.5, -12.0],
                box_max: [x0 + 8.0, 8.0, 12.0],
                capture_point: [x0 + 4.0, 0.002, 0.0],
                layer: i,
                lights: std::array::from_fn(|l| ReflectionProbeLightShare {
                    roster_index: l as u32,
                    reference: [1.0 + 0.5 * l as f32, 0.8, 0.3 + 0.2 * i as f32],
                    shares: std::array::from_fn(|lobe| {
                        let s = 0.05 + 0.04 * ((l + lobe + i as usize) % 5) as f32;
                        [s, s * 0.7, s * 1.3]
                    }),
                }),
            }
        })
        .collect()
}

fn curves() -> Vec<AnimatedLightScaleDescriptor<'static>> {
    const BRIGHTNESS: [&[f32]; 4] = [
        &[0.0, 2.0, 0.5, 1.5],
        &[1.0, 0.2, 3.0],
        &[0.4, 0.4, 2.5, 0.1, 1.0],
        &[1.2],
    ];
    const COLOR: [&[[f32; 3]]; 4] = [
        &[],
        &[[1.0, 0.2, 0.2], [0.2, 1.0, 0.2]],
        &[],
        &[[0.5, 0.5, 1.0], [1.0, 0.8, 0.0], [0.0, 1.0, 1.0]],
    ];
    (0..4)
        .map(|l| AnimatedLightScaleDescriptor {
            period: [0.9, 1.3, 1.7, -2.3][l],
            phase: 0.13 * l as f32,
            base_color: [1.5, 1.0 + 0.1 * l as f32, 0.6],
            brightness: BRIGHTNESS[l],
            color: COLOR[l],
            is_active: true,
        })
        .collect()
}

/// A distance tile with a surface 6 m out in every direction, so every
/// search brackets and refines.
fn ring_surface(_layer: u32, _uv: Vec2) -> f32 {
    1.0 / 6.0
}

/// A colour tile varying with layer and direction.
fn colour(layer: u32, uv: Vec2) -> Vec3 {
    Vec3::new(
        0.2 + uv.x,
        0.3 + uv.y * 0.5,
        0.1 + 0.2 * layer as f32 + uv.x * uv.y,
    )
}

fn fragment(probe: u32, world_position: Vec3, style: u32) -> FluidReflectionFragment {
    FluidReflectionFragment {
        probe,
        camera: Vec3::new(-2.0, 2.5, -9.0),
        world_position,
        outward: Vec3::Y,
        light_term_mask: LightTermMask::ALL.bits(),
        reflectivity: 6.0,
        style,
        texel_uv: Vec2::new(world_position.x, world_position.z) * 8.0,
        axes: FluidTexelAxes {
            u: Vec3::X / 8.0,
            v: Vec3::Z / 8.0,
        },
        time: 0.37,
        texels_per_pixel: 0.5,
    }
}

fn positions() -> Vec<Vec3> {
    let mut out = Vec::new();
    for xi in 0..12 {
        for zi in 0..7 {
            out.push(Vec3::new(
                -11.7 + 2.0 * xi as f32,
                0.0,
                -10.5 + 3.3 * zi as f32,
            ));
        }
    }
    out
}

fn own_probe(position: Vec3) -> u32 {
    (((position.x + 12.0) / 8.0).floor() as u32).min(2)
}

// Gate 1: with both neighbours the own probe, blend and precomputed equal
// single (the reference `reflection_fragment`) within 1e-5; single mode is
// the reference exactly.
#[test]
fn self_neighbours_make_blend_and_precomputed_equal_single() {
    let probes = three_probes();
    let descriptors = curves();
    let surface = Vec4::new(0.2, 0.3, 0.4, 0.6);
    let self_neighbours: Vec<[u32; 2]> = (0..3).map(|p| [p, p]).collect();
    for time in [0.0, 0.41, 1.9, 7.3] {
        let lights = FluidReflectionLights {
            descriptors: &descriptors,
            time,
        };
        for style in [FLUID_REFLECTION_STYLE_SMOOTH, FLUID_REFLECTION_STYLE_PIXEL] {
            for position in positions() {
                let frag = fragment(own_probe(position), position, style);
                let single = reflection_fragment(
                    surface,
                    &frag,
                    &probes,
                    TILE,
                    &lights,
                    colour,
                    ring_surface,
                );
                for mode in FluidReflectionBlendMode::ALL {
                    for coverage in [false, true] {
                        let lever = FluidBlendLever {
                            mode,
                            coverage,
                            band: 1.0,
                            neighbours: &self_neighbours,
                            slot: 3,
                        };
                        let out = reflection_blend_fragment(
                            surface,
                            &frag,
                            &probes,
                            TILE,
                            &lights,
                            &lever,
                            colour,
                            ring_surface,
                        );
                        if mode == FluidReflectionBlendMode::Single {
                            assert_eq!(out, single, "single mode is the reference");
                        }
                        assert!(
                            out.abs_diff_eq(single, 1.0e-5),
                            "{mode:?} coverage {coverage} at {position} t {time}: {out} vs {single}"
                        );
                    }
                }
            }
        }
    }
}

// Gate 2: the precomputed lobe ratio equals the per-fragment ratio for the
// same probe and direction within 1e-4, across directions, curve
// descriptors and times; the clamp placement matches (some lobes negative).
#[test]
fn precomputed_ratio_equals_the_per_fragment_ratio() {
    let mut probes = three_probes();
    // A dimming light with a large share drives some lobes below 0, so the
    // clamp's placement is exercised.
    for light in probes[1].lights.iter_mut() {
        light.reference = [40.0, 40.0, 40.0];
        light.shares = [[0.9, 0.1, 0.5]; REFLECTION_PROBE_LOBE_COUNT];
    }
    let descriptors = curves();
    let mut checked = 0;
    let mut clamped = 0;
    for time in (0..40).map(|i| i as f32 * 0.173) {
        let lights = FluidReflectionLights {
            descriptors: &descriptors,
            time,
        };
        for probe in &probes {
            let packed = FluidProbeGpu::pack(probe).lights;
            let lobes = ratio_lobes(&packed, &lights);
            for yi in 0..6 {
                for ai in 0..16 {
                    let y = yi as f32 / 5.0 * 1.2 - 0.2;
                    let a = ai as f32 / 16.0 * std::f32::consts::TAU;
                    let direction = Vec3::new(a.cos(), y, a.sin());
                    let expected = reflection_ratio(&packed, direction, &lights);
                    let actual = precomputed_ratio(&lobes, direction);
                    assert!(
                        actual.abs_diff_eq(expected, 1.0e-4),
                        "{direction} t {time}: {actual} vs {expected}"
                    );
                    if expected.min_element() == 0.0 {
                        clamped += 1;
                    }
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 10_000);
    assert!(clamped > 0, "the sweep reaches the clamp");
}

// Gate 3: over 10 000+ cells of one weight triple, each probe's pick
// frequency matches its weight within 2%, and a cell always picks the same.
#[test]
fn stochastic_pick_frequencies_match_the_weights_and_cells_are_stable() {
    for weights in [
        [0.5, 0.3, 0.2],
        [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0],
        [0.8, 0.0, 0.2],
        [0.6, 0.4, 0.0],
    ] {
        let mut counts = [0usize; 3];
        let mut cells = 0;
        for slot in [0u32, 5] {
            for x in -60..60 {
                for y in -50..50 {
                    let cell = IVec2::new(x, y);
                    let pick = stochastic_pick(weights, stochastic_draw(cell, slot));
                    assert_eq!(
                        pick,
                        stochastic_pick(weights, stochastic_draw(cell, slot)),
                        "a cell's pick is stable"
                    );
                    assert!(weights[pick] > 0.0, "never a zero-weight tap");
                    counts[pick] += 1;
                    cells += 1;
                }
            }
        }
        assert!(cells >= 10_000);
        for (count, weight) in counts.iter().zip(weights) {
            let frequency = *count as f32 / cells as f32;
            assert!(
                (frequency - weight).abs() <= 0.02,
                "{weights:?}: {counts:?} of {cells}"
            );
        }
    }
}

// Gate 3 (fragment level): every fragment of one pixel-style water cell
// takes the same picked probe, whatever its sub-cell position.
#[test]
fn every_fragment_of_a_pixel_cell_takes_one_stochastic_pick() {
    let probes = three_probes();
    let neighbours = probe_neighbours(&probes);
    let surface = Vec4::new(0.2, 0.3, 0.4, 0.6);
    let lights = FluidReflectionLights::NONE;
    let lever = FluidBlendLever {
        mode: FluidReflectionBlendMode::Stochastic,
        coverage: true,
        band: 1.0,
        neighbours: &neighbours,
        slot: 1,
    };
    for cell in 0..40 {
        let base = Vec3::new(-11.9 + 0.6 * cell as f32, 0.0, 0.3 + 0.25 * cell as f32);
        // Snap to the cell's corner (texels are 1/8 m).
        let corner = (base * 8.0).floor() / 8.0;
        let outs: Vec<Vec4> = [0.1, 0.45, 0.9]
            .iter()
            .flat_map(|&a| [0.2, 0.7].map(move |b| corner + Vec3::new(a, 0.0, b) / 8.0))
            .map(|p| {
                reflection_blend_fragment(
                    surface,
                    &fragment(own_probe(corner), p, FLUID_REFLECTION_STYLE_PIXEL),
                    &probes,
                    TILE,
                    &lights,
                    &lever,
                    colour,
                    ring_surface,
                )
            })
            .collect();
        for out in &outs {
            assert!(out.abs_diff_eq(outs[0], 1.0e-5), "cell {cell}: {outs:?}");
        }
    }
}

// Gate 4: coverage on gives three nonzero weights everywhere; coverage off
// with neighbours far away leaves one tap. Weights lie in [0, 1], sum to 1.
#[test]
fn coverage_forces_three_taps_and_far_neighbours_leave_one() {
    let probes = three_probes();
    let neighbours = probe_neighbours(&probes);
    for position in positions() {
        let own = own_probe(position);
        for (coverage, band) in [(true, 1.0), (true, 0.0), (false, 1.0), (false, 3.0)] {
            let taps = blend_taps(
                own,
                neighbours[own as usize],
                position,
                &probes,
                band,
                coverage,
            );
            let sum: f32 = taps.weights.iter().sum();
            assert!((sum - 1.0).abs() <= 1.0e-6, "{taps:?}");
            assert!(taps.weights.iter().all(|w| (0.0..=1.0).contains(w)));
            if coverage {
                assert!(
                    taps.weights.iter().all(|&w| w > 0.0),
                    "{position}: {taps:?}"
                );
            }
        }
    }
    // Far neighbours: a lone probe whose neighbours sit 50 m away.
    let mut far = three_probes();
    for (i, probe) in far.iter_mut().enumerate().skip(1) {
        probe.box_min[0] += 50.0 * i as f32;
        probe.box_max[0] += 50.0 * i as f32;
    }
    let taps = blend_taps(0, [1, 2], Vec3::new(-8.0, 0.0, 0.0), &far, 1.0, false);
    assert_eq!(taps.weights, [1.0, 0.0, 0.0]);
    // Inside the band of the abutting neighbour, it takes weight.
    let taps = blend_taps(0, [1, 2], Vec3::new(-4.5, 0.0, 0.0), &probes, 1.0, false);
    assert!(taps.weights[1] > 0.0 && taps.weights[2] == 0.0, "{taps:?}");
}

// Neighbour selection: same-plane boxes nearest first, ties to the lower
// index; fills from the next indices; NO_PROBE with fewer than three.
#[test]
fn neighbours_pick_nearest_same_plane_boxes() {
    let probes = three_probes();
    assert_eq!(probe_neighbours(&probes), vec![[1, 2], [0, 2], [1, 0]]);
    let mut lifted = three_probes();
    lifted[2].capture_point[1] = 3.0;
    let n = probe_neighbours(&lifted);
    assert_eq!(n[0], [1, 2], "filled from the next index");
    assert_eq!(n[2], [0, 1], "a lone plane fills from (p+1)%n, (p+2)%n");
    let two = &probes[..2];
    assert_eq!(
        probe_neighbours(two),
        vec![
            [1, FLUID_REFLECTION_NO_PROBE],
            [0, FLUID_REFLECTION_NO_PROBE]
        ]
    );
    assert_eq!(
        probe_neighbours(&probes[..1]),
        vec![[FLUID_REFLECTION_NO_PROBE; 2]]
    );
}

#[test]
fn blend_word_packs_mode_and_coverage() {
    assert_eq!(pack_blend_word(FluidReflectionBlendMode::Single, false), 0);
    assert_eq!(
        pack_blend_word(FluidReflectionBlendMode::Stochastic, true),
        0x103
    );
    for mode in FluidReflectionBlendMode::ALL {
        assert_eq!(FluidReflectionBlendMode::from_name(mode.name()), Some(mode));
    }
    assert_eq!(FLUID_RATIO_LOBES_BYTES, 20 * 1024);
}
