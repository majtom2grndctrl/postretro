//! Adapter-backed gates for the glossy world overlay's first slice: the
//! composite forward + W·(R − D_static), the Equal landing (one overlay
//! fragment per still-showing glossy pixel), forward's second output and its
//! forms, and the overlay's gate with the sums' dispatch.
//! See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces)

use glam::Vec3;
use postretro_render_cpu::fluid_surface::reflection::reflection_fresnel;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::frame_uniforms::LightTermMask;

use super::fluid_reflection_frame_test_fixtures::{centre, on_plane, pixels};
use super::fluid_reflection_sums_frame_test_fixtures::dispatches;
use super::glossy_overlay_frame_test_fixtures::{
    GlossyFrame, PROBE_COLOUR, face_at, floor_probe, floor_view, glossy_frame, half_step,
    install_floor, near_any_face, overlays, renderer, with_floor, with_variant,
};
use super::{GLOSSY_STATIC_DIFFUSE, Renderer};

/// The CPU edge margin, metres: pixels this close to a face edge are left out
/// of per-face expectations.
const MARGIN: f32 = 0.05;

fn assert_channels_within(actual: Vec3, expected: Vec3, slack: f32, what: &str) {
    for c in 0..3 {
        let tolerance = half_step(expected[c]) + slack;
        assert!(
            (actual[c] - expected[c]).abs() <= tolerance,
            "{what}: channel {c} is {} against {} (tolerance {tolerance})",
            actual[c],
            expected[c]
        );
    }
}

/// Production, then the zero-adding variant (forward's colour beside its
/// D_static), at one pose and clock.
fn production_and_forward(renderer: &mut Renderer) -> (GlossyFrame, GlossyFrame) {
    let view = floor_view();
    let production = glossy_frame(renderer, view);
    let forward = with_variant(renderer, "fs_test_zero", |renderer| {
        glossy_frame(renderer, view)
    });
    (production, forward)
}

// A bound glossy texel shows forward + W·(R − D_static)
// within one half-float step, W the head-on-0.04 Schlick weight times the
// capture-shadow weight (1 here: zero distances); where W is 0 (an unbound
// glossy face, a non-glossy face, everything off the floor) the output is
// byte-identical to forward's.
#[test]
fn glossy_floor_composites_forward_plus_weighted_probe_minus_static_diffuse() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_floor(&mut renderer, &floor_probe(true));
    let view = floor_view();
    let (production, forward) = production_and_forward(&mut renderer);
    let mut checked = 0;
    for pixel in pixels() {
        match face_at(view, pixel, MARGIN) {
            Some(0) => {
                let point = on_plane(view, centre(pixel), 0.0).expect("on the floor");
                let to_eye = (view.eye - point).normalize();
                let weight = reflection_fresnel(to_eye.y, 2.0);
                let d_static = forward.static_diffuse(pixel);
                let expected = forward.colour(pixel) + weight * (PROBE_COLOUR - d_static);
                // f32 rasterization against the CPU's ray-plane point moves
                // W by about 1e-6.
                assert_channels_within(production.colour(pixel), expected, 1.0e-5, "bound");
                checked += 1;
            }
            Some(_) => assert_eq!(
                production.colour(pixel),
                forward.colour(pixel),
                "W = 0 off a bound face leaves forward's colour at {pixel}"
            ),
            None if !near_any_face(view, pixel, MARGIN) => {
                assert_eq!(production.colour(pixel), forward.colour(pixel))
            }
            None => {}
        }
    }
    assert!(checked > 100, "the bound face covers the view: {checked}");
}

// With R forced equal to forward's own static diffuse
// read back on the same frame, every glossy pixel is within one half-float
// step of forward's; a deliberately perturbed D_static fails that check.
#[test]
fn reflection_equal_to_static_diffuse_leaves_forward_and_a_perturbed_static_diffuse_does_not() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_floor(&mut renderer, &floor_probe(true));
    let view = floor_view();
    let forward = with_variant(&mut renderer, "fs_test_zero", |renderer| {
        glossy_frame(renderer, view)
    });
    let parity = with_variant(&mut renderer, "fs_test_reflection_is_static_diffuse", |r| {
        glossy_frame(r, view)
    });
    let perturbed = with_variant(&mut renderer, "fs_test_perturbed_static_diffuse", |r| {
        glossy_frame(r, view)
    });
    let mut failing = 0;
    for pixel in pixels() {
        if face_at(view, pixel, MARGIN) != Some(0) {
            continue;
        }
        assert_channels_within(
            parity.colour(pixel),
            forward.colour(pixel),
            0.0,
            "R = D_static",
        );
        let gap = perturbed.colour(pixel) - forward.colour(pixel);
        if (0..3).any(|c| gap[c].abs() > half_step(forward.colour(pixel)[c])) {
            failing += 1;
        }
    }
    assert!(
        failing > 100,
        "the perturbed term fails the check: {failing}"
    );
}

// Every forward-shaded pixel of a glossy face, bound or
// with its probe dropped, takes exactly one overlay fragment and no other
// pixel takes one, measured by a variant adding a constant per fragment; in
// production the unbound glossy face is byte-identical to forward's.
#[test]
fn every_glossy_pixel_takes_exactly_one_overlay_fragment() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_floor(&mut renderer, &floor_probe(true));
    let view = floor_view();
    let forward = with_variant(&mut renderer, "fs_test_zero", |renderer| {
        glossy_frame(renderer, view)
    });
    let counted = with_variant(&mut renderer, "fs_test_constant", |renderer| {
        glossy_frame(renderer, view)
    });
    let constant = Vec3::splat(0.125);
    let mut glossy_pixels = [0; 2];
    for pixel in pixels() {
        match face_at(view, pixel, MARGIN) {
            Some(face @ (0 | 1)) => {
                assert_channels_within(
                    counted.colour(pixel),
                    forward.colour(pixel) + constant,
                    0.0,
                    "one constant per glossy pixel",
                );
                glossy_pixels[face] += 1;
            }
            Some(_) => assert_eq!(counted.colour(pixel), forward.colour(pixel)),
            None if !near_any_face(view, pixel, MARGIN) => {
                assert_eq!(counted.colour(pixel), forward.colour(pixel))
            }
            None => {}
        }
    }
    assert!(
        glossy_pixels.iter().all(|&count| count > 100),
        "{glossy_pixels:?}"
    );
}

// GPU half: forward's colour from its two-output forms
// equals its one-output form's at every pixel, a level without glossy faces
// allocates no D_static and records no overlay, and at reflection quality
// off neither does a glossy level.
#[test]
fn forwards_two_output_forms_keep_its_colour_and_off_writes_none() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_floor(&mut renderer, &floor_probe(true));
    let view = floor_view();
    let two_outputs = with_variant(&mut renderer, "fs_test_zero", |renderer| {
        glossy_frame(renderer, view)
    });
    assert!(two_outputs.static_diffuse.is_some());

    let before = overlays(&renderer);
    renderer.set_reflection_quality(FluidReflectionQuality::Off);
    let off = glossy_frame(&mut renderer, view);
    assert!(off.static_diffuse.is_none(), "off releases D_static");
    assert_eq!(overlays(&renderer), before, "off records no overlay");
    assert_eq!(
        off.scene.colour, two_outputs.scene.colour,
        "forward's colour is the same through every form"
    );
    let ledger = renderer
        .reflection_probe_residency_report()
        .expect("full renderer");
    assert_eq!(ledger.bytes(GLOSSY_STATIC_DIFFUSE), None);

    renderer.set_reflection_quality(FluidReflectionQuality::Low);
    let low = glossy_frame(&mut renderer, view);
    assert!(low.static_diffuse.is_some(), "low allocates D_static again");
    assert_eq!(overlays(&renderer), before + 1, "low records the overlay");
    let ledger = renderer
        .reflection_probe_residency_report()
        .expect("full renderer");
    let area = u64::from(super::fluid_reflection_frame_test_fixtures::FRAME).pow(2);
    assert!(
        ledger
            .bytes(GLOSSY_STATIC_DIFFUSE)
            .is_some_and(|bytes| bytes >= area * 4),
        "the ledger counts D_static beside the probes"
    );

    // A level whose id 54 binds no world face: no stream, no target, one
    // forward output, no overlay.
    let mut plain = renderer_or_skip();
    install_floor(&mut plain, &floor_probe(false));
    assert!(plain.level().glossy.is_none());
    let frame = glossy_frame(&mut plain, view);
    assert!(frame.static_diffuse.is_none());
    assert_eq!(overlays(&plain), 0);
}

fn renderer_or_skip() -> Renderer {
    renderer().expect("a second renderer on the adapter the first found")
}

// Partial coverage: on the glossy faces, with no dynamic
// light, highlight or glow in the fixture, forward's colour is its static
// diffuse bit for bit — D_static is stored by the scene colour's own
// conversion, so it never exceeds the colour (no undershoot at W = 1) — and
// D_static follows a static term (the ambient floor) as the colour does; off
// the glossy buckets D_static stays cleared.
#[test]
fn static_diffuse_carries_the_static_terms_on_glossy_buckets_only() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_floor(&mut renderer, &floor_probe(true));
    let view = floor_view();
    for ambient in [0.0, 0.5] {
        renderer.set_ambient_floor(ambient);
        let frame = with_variant(&mut renderer, "fs_test_zero", |renderer| {
            glossy_frame(renderer, view)
        });
        let mut glossy = 0;
        for pixel in pixels() {
            match face_at(view, pixel, MARGIN) {
                Some(0 | 1) => {
                    assert_eq!(
                        frame.static_diffuse(pixel),
                        frame.colour(pixel),
                        "D_static against forward at {pixel}"
                    );
                    glossy += 1;
                }
                Some(_) => assert_eq!(frame.static_diffuse(pixel), Vec3::ZERO),
                None => {}
            }
        }
        assert!(glossy > 200);
    }
}

// a glossy level with no fluid records one overlay pass
// and dispatches the sums once on a frame with a glossy range; a frame with
// an all-terms mask bit off records neither; a frame whose glossy ranges the
// GPU culls whole leaves forward's output byte-identical.
#[test]
fn a_glossy_level_without_fluid_draws_the_probe_term_and_dispatches_the_sums() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_floor(&mut renderer, &floor_probe(true));
    assert!(renderer.fluid_surfaces().is_none(), "no fluid face");
    let view = floor_view();
    let (overlays_before, sums_before) = (overlays(&renderer), dispatches(&renderer));
    glossy_frame(&mut renderer, view);
    assert_eq!(overlays(&renderer), overlays_before + 1);
    assert_eq!(dispatches(&renderer), sums_before + 1);

    let mut mask = LightTermMask::ALL;
    mask.set_enabled(LightTermMask::SPECULAR, false);
    renderer.set_light_term_mask(mask);
    glossy_frame(&mut renderer, view);
    assert_eq!(
        overlays(&renderer),
        overlays_before + 1,
        "masked: no overlay"
    );
    assert_eq!(dispatches(&renderer), sums_before + 1, "masked: no sums");
    renderer.set_light_term_mask(LightTermMask::ALL);

    // Looking up and away: the camera ranges still hold every leaf under
    // draw-all, so the overlay records and its draws cull to nothing; the
    // frame is byte-identical to forward's.
    let away = super::fluid_reflection_frame_test_fixtures::view_from(
        Vec3::new(0.0, 1.5, 0.0),
        Vec3::new(0.0, 3.0, -7.0),
    );
    let culled = glossy_frame(&mut renderer, away);
    let forward = with_variant(&mut renderer, "fs_test_zero", |renderer| {
        glossy_frame(renderer, away)
    });
    assert_eq!(culled.scene.colour, forward.scene.colour);
}

// The off-main build binds the same glossy state as the main-thread install
// (stream, probe group, D_static at the build's tier).
#[test]
fn an_off_main_build_publishes_a_ready_glossy_state() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let probes = floor_probe(true);
    let tiers = renderer.level_build_tiers();
    let (builder, mut pending) = renderer.begin_level_set_build(tiers);
    let section = probes.clone();
    builder.geometry_from(
        Box::new(move |visit| {
            with_floor(Some(&section), |geometry| {
                visit(
                    geometry,
                    super::level_build::GeometryExtras {
                        volumes: &[],
                        reflection_probes: Some(&section),
                        fog_pixel_scale: 0,
                        fog_cell_masks: None,
                    },
                )
            })
        }),
        super::level_build::GeometryRest {
            gpu_lighting_payloads: Default::default(),
            sh_plan: None,
            lightmap_plan: None,
            fluid: None,
            planar_candidate: None,
        },
    );
    builder.finish();
    let id = super::fluid_reflection_sums_frame_test_fixtures::take(&mut renderer, &mut pending);
    renderer.publish_level_set(id, super::level_resource_set::PreviousLevelSet::Retire);
    let glossy = renderer.level().glossy.as_ref().expect("glossy state");
    assert!(glossy.ready(), "probes bound and D_static allocated");
}
