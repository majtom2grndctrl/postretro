//! Adapter-backed gates for the glossy overlay's weights and colour: the
//! sharpness fade and `_s` scale, Fresnel on the 0.04 base raised by tint,
//! the highlight-tint colour, the static diffuse fading by (1 − W) with no
//! channel below 0, the all-terms mask gate on its toggle frames, and
//! dynamic light and glow passing through the overlay unchanged.
//! See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces)

use glam::{UVec2, Vec3};
use postretro_level_format::specular_smoothness::smoothness_byte_exponent;
use postretro_render_cpu::fluid_surface::reflection::REFLECTION_SCREEN_MISS;
use postretro_render_cpu::frame_uniforms::LightTermMask;
use postretro_render_cpu::glossy_reflection::{
    GLOSSY_FULL_WEIGHT_EXPONENT, GLOSSY_SHARPNESS_FLOOR_EXPONENT, glossy_composite, glossy_fresnel,
    glossy_texel,
};

use super::Renderer;
use super::fluid_reflection_frame_test_fixtures::{View, centre, on_plane, pixels, view_from};
use super::glossy_overlay_frame_test_fixtures::{
    FloorMaterial, GlossyFrame, PROBE_COLOUR, face_at, floor_probe_of, glossy_frame, half_step,
    install_floor, overlays, renderer, set_floor_material, with_variant,
};

/// Pixels this close (metres) to a face edge are left out.
const MARGIN: f32 = 0.05;
/// The bound glossy face.
const BOUND: usize = 0;
/// The ambient floor: static light on every face, beside the fixture's SH.
const AMBIENT: f32 = 0.5;
const NO_TINT: u8 = 255;
const FULL_TINT: u8 = 254;
const SHARP: u8 = 255;

/// Looking down the floor from 1.5 m: Fresnel between about 0.2 and 0.6
/// of the way from head-on to grazing.
fn across() -> View {
    view_from(Vec3::new(0.0, 1.5, 0.0), Vec3::new(0.0, 0.0, 7.0))
}

/// Nearly straight down over the bound face: within a few degrees of head-on.
fn head_on() -> View {
    view_from(Vec3::new(-1.5, 3.0, 5.0), Vec3::new(-1.5, 0.0, 5.3))
}

/// A few centimetres above the floor, looking along it: grazing.
fn grazing() -> View {
    view_from(Vec3::new(-1.5, 0.08, 0.2), Vec3::new(-1.5, 0.0, 12.0))
}

fn material(albedo: [u8; 4], slot: [u8; 4]) -> FloorMaterial {
    FloorMaterial {
        albedo,
        slot,
        glow: None,
    }
}

/// The last smoothness byte at or below the floor, the first past it, and
/// the first at full weight.
fn band_bytes() -> (u8, u8, u8) {
    let past = (1..=u8::MAX)
        .find(|&b| smoothness_byte_exponent(b) > GLOSSY_SHARPNESS_FLOOR_EXPONENT)
        .expect("a byte decodes past the floor");
    let full = (1..=u8::MAX)
        .find(|&b| smoothness_byte_exponent(b) >= GLOSSY_FULL_WEIGHT_EXPONENT)
        .expect("a byte decodes past the band");
    (past - 1, past, full)
}

/// The bound face's pixels at `view`, each with its unit fragment-to-eye
/// cosine against the floor's normal.
fn bound_pixels(view: View) -> Vec<(UVec2, f32)> {
    pixels()
        .filter(|&pixel| face_at(view, pixel, MARGIN) == Some(BOUND))
        .map(|pixel| {
            let point = on_plane(view, centre(pixel), 0.0).expect("on the floor");
            (pixel, (view.eye - point).normalize().y)
        })
        .collect()
}

/// Production at `view` and the zero-adding variant (forward's colour and
/// D_static) at the same pose and clock, under `floor`.
fn frames(renderer: &mut Renderer, floor: FloorMaterial, view: View) -> (GlossyFrame, GlossyFrame) {
    set_floor_material(renderer, floor);
    let production = glossy_frame(renderer, view);
    let forward = with_variant(renderer, "fs_test_zero", |renderer| {
        glossy_frame(renderer, view)
    });
    (production, forward)
}

fn glossy_renderer(probe: Vec3) -> Option<Renderer> {
    let mut renderer = renderer()?;
    install_floor(&mut renderer, &floor_probe_of(true, probe));
    renderer.set_ambient_floor(AMBIENT);
    Some(renderer)
}

/// What the overlay adds at a bound pixel under `floor`, by the CPU
/// reference, over the forward frame's own colour and D_static, and the
/// reflection's own magnitude there. The albedo is the one the GPU sampled:
/// forward's colour over a white floor's under the same light (the
/// hardware's sRGB decode differs from the exact curve by about 0.2%).
fn expected(
    floor: FloorMaterial,
    probe: Vec3,
    forward: &GlossyFrame,
    white: &GlossyFrame,
    pixel: UVec2,
    cos: f32,
) -> (Vec3, Vec3) {
    let albedo = forward.colour(pixel) / white.colour(pixel);
    let texel = glossy_texel(floor.slot_sample(), albedo);
    let fresnel = glossy_fresnel(&texel, cos);
    let reflected = glossy_composite(
        Vec3::ZERO,
        probe,
        1.0,
        REFLECTION_SCREEN_MISS,
        fresnel,
        texel.colour,
    );
    let output = forward.colour(pixel)
        + glossy_composite(
            forward.static_diffuse(pixel),
            probe,
            1.0,
            REFLECTION_SCREEN_MISS,
            fresnel,
            texel.colour,
        );
    (output, reflected)
}

/// One f16 step, plus the GPU's own transcendental slack.
fn tolerance(value: f32) -> f32 {
    half_step(value) + 2.0e-4 * value.abs().max(1.0)
}

/// Forward's frame over a white floor of `floor`'s slot: the light alone.
fn white_light(renderer: &mut Renderer, floor: FloorMaterial, view: View) -> GlossyFrame {
    frames(
        renderer,
        FloorMaterial {
            albedo: [255; 4],
            ..floor
        },
        view,
    )
    .1
}

fn assert_matches_reference(
    production: &GlossyFrame,
    forward: &GlossyFrame,
    white: &GlossyFrame,
    floor: FloorMaterial,
    probe: Vec3,
    view: View,
) -> usize {
    let mut checked = 0;
    for (pixel, cos) in bound_pixels(view) {
        // At grazing one pixel spans metres of floor, and the Fresnel weight
        // moves fast with the angle: the reference brackets the fragment's
        // angle over a tenth of a pixel either side of its centre.
        let bracket = [-0.1, 0.1].map(|dy| {
            let point = on_plane(view, centre(pixel) + glam::Vec2::new(0.0, dy), 0.0)
                .expect("on the floor");
            expected(
                floor,
                probe,
                forward,
                white,
                pixel,
                (view.eye - point).normalize().y,
            )
        });
        let (expected, reflected) = expected(floor, probe, forward, white, pixel, cos);
        let actual = production.colour(pixel);
        for c in 0..3 {
            // The derived albedo carries two f16 roundings into the colour.
            let slack = tolerance(expected[c]) + 2.0e-3 * reflected[c];
            let low = bracket[0].0[c].min(bracket[1].0[c]).min(expected[c]) - slack;
            let high = bracket[0].0[c].max(bracket[1].0[c]).max(expected[c]) + slack;
            assert!(
                (low..=high).contains(&actual[c]),
                "{floor:?} at {pixel} (cos {cos}): channel {c} is {} against {} ({low}..={high})",
                actual[c],
                expected[c]
            );
        }
        checked += 1;
    }
    checked
}

// GPU half: every bound pixel's output is
// forward plus the CPU reference's addition for its texel — the sharpness
// fade at, inside and past the band, `_s` scaling, Fresnel from the 0.04
// base raised by tint, and the highlight's colour — at head-on, across and
// grazing views.
#[test]
fn bound_pixels_add_the_reference_weight_and_colour_at_every_angle() {
    let Some(mut renderer) = glossy_renderer(PROBE_COLOUR) else {
        return;
    };
    let (at_floor, past_floor, full) = band_bytes();
    let mid_band = past_floor + (full - past_floor) / 2;
    let albedo = [230, 120, 40, 255];
    let floors = [
        material([255; 4], [255, 0, SHARP, 0]),
        material(albedo, [255, 0, SHARP, 0]),
        material(albedo, [255, 0, SHARP, FULL_TINT]),
        material(albedo, [255, 0, SHARP, 127]),
        material(albedo, [255, 0, SHARP, NO_TINT]),
        material(albedo, [127, 0, SHARP, NO_TINT]),
        material(albedo, [255, 0, past_floor, NO_TINT]),
        material(albedo, [255, 0, mid_band, FULL_TINT]),
        material(albedo, [200, 0, full, 60]),
        material(albedo, [255, 0, at_floor, FULL_TINT]),
        material(albedo, [0, 0, SHARP, FULL_TINT]),
    ];
    for view in [head_on(), across(), grazing()] {
        for floor in floors {
            let white = white_light(&mut renderer, floor, view);
            let (production, forward) = frames(&mut renderer, floor, view);
            let checked =
                assert_matches_reference(&production, &forward, &white, floor, PROBE_COLOUR, view);
            assert!(checked > 100, "{floor:?}: {checked} bound pixels");
        }
    }
}

// GPU half: a texel at or below the floor, and one with
// `_s` 0, draws no term (byte-identical to forward); inside the band the
// term rises with the exponent; halving `_s` halves the addition; at
// grazing, adjacent texels across the floor contour differ by no more than
// adjacent texels inside the band.
#[test]
fn the_sharpness_fade_and_specular_scale_shape_the_drawn_term() {
    let Some(mut renderer) = glossy_renderer(PROBE_COLOUR) else {
        return;
    };
    let (at_floor, past_floor, full) = band_bytes();
    let view = grazing();
    let bound = bound_pixels(view);
    let delta = |renderer: &mut Renderer, slot: [u8; 4]| -> Vec<Vec3> {
        let (production, forward) = frames(renderer, material([255; 4], slot), view);
        bound
            .iter()
            .map(|&(pixel, _)| production.colour(pixel) - forward.colour(pixel))
            .collect()
    };
    // Each addition is the difference of two f16 colours no brighter than
    // forward's: one step of forward's colour bounds each rounding.
    let forward = frames(
        &mut renderer,
        material([255; 4], [0, 0, SHARP, NO_TINT]),
        view,
    )
    .1;
    let step_at = |p: usize| half_step(forward.colour(bound[p].0).max_element());
    for slot in [[255, 0, at_floor, NO_TINT], [0, 0, SHARP, FULL_TINT]] {
        assert!(
            delta(&mut renderer, slot).iter().all(|d| *d == Vec3::ZERO),
            "{slot:?} draws no term"
        );
    }
    // The addition W·(R − D_static) keeps one sign per channel and grows in
    // magnitude with the weight.
    let band: Vec<Vec<Vec3>> = (at_floor..=full)
        .map(|b| delta(&mut renderer, [255, 0, b, NO_TINT]))
        .collect();
    for (i, pair) in band.windows(2).enumerate() {
        for (p, (lower, higher)) in pair[0].iter().zip(&pair[1]).enumerate() {
            assert!(
                higher.y.abs() + 2.0 * step_at(p) >= lower.y.abs(),
                "byte {} at {:?}: {} after {}",
                at_floor as usize + i + 1,
                bound[p].0,
                higher.y,
                lower.y
            );
        }
    }
    // Across the contour against inside the band, pixel by pixel.
    let step = |a: &[Vec3], b: &[Vec3], p: usize| (b[p].y - a[p].y).abs();
    let inside = band.len() - 2;
    for (p, (pixel, _)) in bound.iter().enumerate() {
        let contour = step(&band[0], &band[1], p);
        let smallest_inside = (1..inside)
            .map(|i| step(&band[i], &band[i + 1], p))
            .fold(f32::INFINITY, f32::min);
        assert!(
            contour <= smallest_inside + 4.0 * step_at(p),
            "at {pixel:?}: the contour step {contour} passes the band's {smallest_inside}"
        );
    }
    assert!(past_floor < full);
    // Halving `_s` halves the addition.
    let whole = delta(&mut renderer, [254, 0, SHARP, NO_TINT]);
    let half = delta(&mut renderer, [127, 0, SHARP, NO_TINT]);
    for (p, (w, h)) in whole.iter().zip(&half).enumerate() {
        for c in 0..3 {
            assert!(
                (h[c] * 2.0 - w[c]).abs() <= 6.0 * step_at(p),
                "at {:?}: half {} against whole {}",
                bound[p].0,
                h[c],
                w[c]
            );
        }
    }
}

/// Static light values whose rounding to a short float lands above the f16
/// value of the same light: the adversarial pixel for an undershoot.
const ADVERSARIAL_AMBIENTS: [f32; 6] = [
    1.0 + 1.5 / 128.0,
    0.5 + 1.5 / 256.0,
    2.0 + 1.5 / 64.0,
    0.25 + 1.4 / 512.0,
    1.0 + 1.0 / 32.0 + 1.5 / 128.0,
    3.0 + 1.5 / 64.0,
];

// Over a dark reflected ceiling (R = 0), with
// only static light shading the floor, no output channel falls below 0 —
// head-on and grazing, untinted and tint-1, where W reaches 1 (tint-1 head-on
// and every texel at grazing).
#[test]
fn no_channel_falls_below_zero_over_a_dark_reflection() {
    let Some(mut renderer) = glossy_renderer(Vec3::ZERO) else {
        return;
    };
    let mut lowest = f32::INFINITY;
    let mut checked = 0;
    let mut replaced = 0;
    for ambient in ADVERSARIAL_AMBIENTS {
        renderer.set_ambient_floor(ambient);
        for view in [head_on(), grazing()] {
            for tint in [0, FULL_TINT] {
                let floor = material([255; 4], [255, 0, SHARP, tint]);
                set_floor_material(&mut renderer, floor);
                let frame = glossy_frame(&mut renderer, view);
                for (pixel, cos) in bound_pixels(view) {
                    let colour = frame.colour(pixel);
                    lowest = lowest.min(colour.min_element());
                    assert!(
                        colour.min_element() >= 0.0,
                        "ambient {ambient}, tint {tint}, {pixel}: {colour}"
                    );
                    // Where W reaches 1 (tint 1 seen head-on), the static
                    // light is replaced whole: no share of it survives.
                    if tint == FULL_TINT && cos > 0.9 {
                        assert!(
                            colour.max_element() <= 2f32.powi(-14),
                            "ambient {ambient}, {pixel}: {colour} survives W = 1"
                        );
                        replaced += 1;
                    }
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 1000, "{checked}");
    assert!(replaced > 500, "{replaced}");
    assert!(lowest >= 0.0, "lowest channel {lowest}");
}

// The static diffuse — a character's promoted-light
// shadow included, which forward subtracts from it before writing D_static
// — fades by (1 − W): two static light levels differ on a glossy
// pixel by (1 − W) × their difference without the overlay.
#[test]
fn static_diffuse_differences_fade_by_one_minus_w() {
    let Some(mut renderer) = glossy_renderer(PROBE_COLOUR) else {
        return;
    };
    let view = across();
    let floor = material([255; 4], [255, 0, SHARP, 127]);
    let texel = glossy_texel(floor.slot_sample(), Vec3::ONE);
    let mut at = |ambient| {
        renderer.set_ambient_floor(ambient);
        frames(&mut renderer, floor, view)
    };
    let (lit, lit_forward) = at(0.75);
    let (shadowed, shadowed_forward) = at(0.25);
    let mut checked = 0;
    for (pixel, cos) in bound_pixels(view) {
        let weight = glossy_fresnel(&texel, cos);
        assert!(weight > 0.4 && weight < 0.95, "W {weight}");
        let without = lit_forward.colour(pixel) - shadowed_forward.colour(pixel);
        let with = lit.colour(pixel) - shadowed.colour(pixel);
        for c in 0..3 {
            let expected = without[c] * (1.0 - weight);
            assert!(
                (with[c] - expected).abs()
                    <= tolerance(lit.colour(pixel)[c]) + tolerance(shadowed.colour(pixel)[c]),
                "at {pixel}: {} against {expected} (W {weight})",
                with[c]
            );
        }
        checked += 1;
    }
    assert!(checked > 100);
}

// A mask with any bit of the all-terms set off
// draws no overlay term from its toggle frame on — no pass recorded, and the
// frame byte-identical to forward's under that mask — and the term returns on
// the toggle frame back, as before.
#[test]
fn any_all_terms_bit_off_draws_no_term_from_its_toggle_frame() {
    let Some(mut renderer) = glossy_renderer(PROBE_COLOUR) else {
        return;
    };
    let view = across();
    glossy_frame(&mut renderer, view);
    let all = glossy_frame(&mut renderer, view);
    for term in LightTermMask::ALL_TERMS {
        let mut mask = LightTermMask::ALL;
        mask.set_enabled(term, false);
        let before = overlays(&renderer);
        renderer.set_light_term_mask(mask);
        let toggled = glossy_frame(&mut renderer, view);
        assert_eq!(
            overlays(&renderer),
            before,
            "{term:?}: no overlay on its toggle frame"
        );
        let forward = with_variant(&mut renderer, "fs_test_zero", |renderer| {
            glossy_frame(renderer, view)
        });
        assert_eq!(
            toggled.scene.colour, forward.scene.colour,
            "{term:?}: forward's frame under the mask"
        );
        renderer.set_light_term_mask(LightTermMask::ALL);
        let back = glossy_frame(&mut renderer, view);
        assert_eq!(
            overlays(&renderer),
            before + 1,
            "{term:?}: the overlay returns"
        );
        assert_eq!(
            back.scene.colour, all.scene.colour,
            "{term:?}: the term returns"
        );
    }
    // The overlay draws a term at all under the all-terms mask.
    let forward = with_variant(&mut renderer, "fs_test_zero", |renderer| {
        glossy_frame(renderer, view)
    });
    assert_ne!(all.scene.colour, forward.scene.colour);
}

/// A runtime point light's GPU record (`GpuLight`, 64 bytes) and its
/// influence sphere (16 bytes).
pub(super) fn point_light(
    position: Vec3,
    colour: Vec3,
    range: f32,
    falloff: u32,
) -> ([u8; 64], [u8; 16]) {
    let words: [u32; 16] = [
        position.x.to_bits(),
        position.y.to_bits(),
        position.z.to_bits(),
        0, // point
        colour.x.to_bits(),
        colour.y.to_bits(),
        colour.z.to_bits(),
        falloff,
        0.0f32.to_bits(),
        (-1.0f32).to_bits(),
        0.0f32.to_bits(),
        range.to_bits(),
        0,
        0,
        u32::MAX, // no spot shadow slot
        u32::MAX, // no cube shadow slot
    ];
    let influence = [position.x, position.y, position.z, range];
    (
        bytemuck::cast(words),
        bytemuck::cast(influence.map(f32::to_bits)),
    )
}

pub(super) fn set_light(renderer: &mut Renderer, light: Option<([u8; 64], [u8; 16])>) {
    match light {
        Some((record, influence)) => {
            renderer.upload_bridge_lights(&record);
            renderer.upload_bridge_influences(&influence);
        }
        None => {
            renderer.upload_bridge_lights(&[]);
            renderer.upload_bridge_influences(&[]);
        }
    }
}

/// A source's difference to the bound pixels, light on minus light off,
/// with the production overlay and with the zero-adding one.
fn differences(
    renderer: &mut Renderer,
    view: View,
    on: &mut dyn FnMut(&mut Renderer, bool),
) -> Vec<(UVec2, Vec3, Vec3, f32)> {
    let mut capture = |renderer: &mut Renderer, lit: bool, entry: Option<&str>| {
        on(renderer, lit);
        match entry {
            Some(entry) => with_variant(renderer, entry, |r| glossy_frame(r, view)),
            None => glossy_frame(renderer, view),
        }
    };
    let off = capture(renderer, false, None);
    let lit = capture(renderer, true, None);
    let off_forward = capture(renderer, false, Some("fs_test_zero"));
    let lit_forward = capture(renderer, true, Some("fs_test_zero"));
    on(renderer, false);
    bound_pixels(view)
        .into_iter()
        .map(|(pixel, _)| {
            let slack = tolerance(lit.colour(pixel).max_element())
                + tolerance(off.colour(pixel).max_element());
            (
                pixel,
                lit.colour(pixel) - off.colour(pixel),
                lit_forward.colour(pixel) - off_forward.colour(pixel),
                slack,
            )
        })
        .collect()
}

fn assert_differences_match(what: &str, differences: &[(UVec2, Vec3, Vec3, f32)]) {
    let mut lit = 0;
    for &(pixel, with, without, slack) in differences {
        for c in 0..3 {
            assert!(
                (with[c] - without[c]).abs() <= slack,
                "{what} at {pixel}: {} with the overlay against {} without",
                with[c],
                without[c]
            );
        }
        if without.max_element() > 0.01 {
            lit += 1;
        }
    }
    assert!(lit > 50, "{what} lights the floor: {lit} pixels");
}

// GPU half: the difference a dynamic point
// light, a muzzle flash (a short-range inverse-square runtime light at the
// eye, as the weapon's flash reaches the renderer) and a material's glow
// make to a glossy floor, on minus off, is the same with and without the
// overlay within one half-float step, at head-on, grazing and on a tint-1
// metal. The overlay subtracts only D_static, which none of them reaches.
#[test]
fn dynamic_light_flash_and_glow_pass_through_the_overlay_unchanged() {
    let Some(mut renderer) = glossy_renderer(PROBE_COLOUR) else {
        return;
    };
    let lamp = point_light(Vec3::new(-1.5, 1.2, 4.0), Vec3::new(2.0, 1.5, 1.0), 6.0, 0);
    for (view, eye) in [
        (head_on(), Vec3::new(-1.5, 3.0, 5.0)),
        (grazing(), Vec3::new(-1.5, 0.08, 0.2)),
    ] {
        let flash = point_light(
            eye + Vec3::new(0.2, -0.05, 0.3),
            Vec3::new(6.0, 4.0, 1.5),
            5.0,
            2,
        );
        for tint in [0, FULL_TINT] {
            let floor = material([200, 180, 160, 255], [255, 0, SHARP, tint]);
            set_floor_material(&mut renderer, floor);
            for (what, light) in [("lamp", lamp), ("muzzle flash", flash)] {
                let found = differences(&mut renderer, view, &mut |r, lit| {
                    set_light(r, lit.then_some(light))
                });
                assert_differences_match(&format!("{what}, tint {tint}"), &found);
            }
            let glowing = FloorMaterial {
                glow: Some(([200, 120, 60, 255], 2.0)),
                ..floor
            };
            let found = differences(&mut renderer, view, &mut |r, lit| {
                set_floor_material(r, if lit { glowing } else { floor })
            });
            assert_differences_match(&format!("glow, tint {tint}"), &found);
        }
    }
}
