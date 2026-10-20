//! Adapter-backed proofs of the screen-space reflection layer: each frame
//! drawn through the production capture recorder, its expectation from the
//! CPU references run on the frame's own depth and scene colour, read back
//! from the GPU after the frame.
//! See: context/lib/rendering_pipeline.md §7.3a

use super::Renderer;
use super::fluid_reflection_frame_test_fixtures::*;
use super::fluid_surface_frame_test::look_draw;
use glam::{UVec2, Vec2, Vec3, Vec4};
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::{
    FLUID_REFLECTION_STYLE_SMOOTH, FluidReflectionStyle,
};
use postretro_render_cpu::fluid_surface::{FluidShaderVariant, FluidSurfaceLook};
use postretro_render_cpu::frame_uniforms::LightTermMask;

fn pass_records(renderer: &Renderer) -> u32 {
    renderer.full().fluid_surface.reflection_pass_records()
}

fn target_builds(renderer: &Renderer) -> u32 {
    renderer.full().fluid_surface.reflection_target_builds()
}

fn group_5_builds(renderer: &Renderer) -> u32 {
    renderer.full().fluid_surface.scene_inputs().scene_depth().1
}

/// A probe-bound look at `opacity`, reflecting at `reflectivity`.
fn bound_look(opacity: f32, reflectivity: f32) -> FluidSurfaceLook {
    FluidSurfaceLook {
        surface_reflectivity: reflectivity,
        has_bound_face: true,
        ..FluidSurfaceLook::surface_only(opacity)
    }
}

/// Whether `expected`'s march stopped on the standing quad: the only
/// magenta in the opaque scene.
fn hits_quad(scene: &SceneReadback, expected: &Expected) -> bool {
    expected.march.confidence > 0.0 && scene.colour_at(expected.march.pixel.as_uvec2()).x > 0.01
}

/// The base level's opaque scene through `view`, read back after an Off
/// frame of the zero-opacity mirror (which adds nothing to it), and the
/// Off frame itself. Leaves the tier at Off.
fn off_frame(renderer: &mut Renderer, view: View) -> (Vec<u8>, SceneReadback) {
    renderer.set_reflection_quality(FluidReflectionQuality::Off);
    renderer.set_fluid_surface_draws(&[look_draw(0, mirror())]);
    let off = capture_checked(renderer, view);
    (off, read_scene(renderer))
}

/// The CPU smooth composite a fragment at `pixel` reads from a target of
/// `extent` the reflection pass wrote by the CPU references: each texel of
/// its bilinear footprint marched from the pool point under that texel's
/// centre, or a miss where the pool does not cover the centre.
fn smooth_through_target(reference: &Reference<'_>, pixel: UVec2, extent: UVec2) -> Vec4 {
    bilinear_footprint(pixel, extent)
        .into_iter()
        .map(|(texel, weight)| {
            let texel = texel.clamp(glam::IVec2::ZERO, extent.as_ivec2() - 1);
            let at = (texel.as_vec2() + 0.5) * FRAME as f32 / extent.as_vec2();
            let written = on_plane(reference.view, at, 0.0).filter(|point| on_pool(*point, 0.0));
            written.map_or(Vec4::ZERO, |point| reference.smooth(point).screen) * weight
        })
        .sum()
}

// fluid-ssr-reflections row 14, P12. A horizontal pool bound to a probe whose capture
// is uniform green, with an opaque magenta quad standing in it and a black
// wall behind, drawn as a zero-opacity mirror (F = 1) in smooth style. The
// CPU march runs on the frame's own depth, read back from the GPU after the
// frame (not built analytically), and samples the read-back scene colour.
// At high, every water fragment whose CPU march hits the quad reads the CPU
// layered composite within 2/255 and differs from the same frame at Off. At
// medium (a half-extent target, marched by medium's preset) each of those
// fragments reads the CPU composite of its bilinear footprint of CPU-marched
// texels within 2/255, and wherever that whole
// footprint hits the quad, the quad's reflection. A face seen from below, a
// face bound to no probe beside a bound one, and an isolating mask each draw
// the reflection-free frame byte for byte. Every frame is captured inside a
// validation error scope, which would hold any validation error the
// reflection and fluid passes raised; each scope pops empty.
#[test]
fn a_standing_quad_reflects_at_the_cpu_composite_and_refused_faces_draw_reflection_free() {
    let Some(mut renderer) = ssr_renderer() else {
        return;
    };
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    install_base_level(&mut renderer);
    let view = base_view();
    let (off, scene) = off_frame(&mut renderer, view);
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    let records = pass_records(&renderer);
    let high = capture_checked(&mut renderer, view);
    assert_eq!(
        pass_records(&renderer),
        records + 1,
        "high records the pass"
    );
    let reference = Reference::new(view, &scene, march_tuning(&renderer));

    let quad_hits: Vec<(UVec2, Vec3, Expected)> = pixels()
        .filter_map(|pixel| {
            let water = visible_water(view, &scene, pixel, 0.0)?;
            let expected = reference.smooth(water);
            hits_quad(&scene, &expected).then_some((pixel, water, expected))
        })
        .collect();
    let faded = quad_hits
        .iter()
        .filter(|(.., expected)| expected.march.confidence < 1.0)
        .count();
    let worst = quad_hits
        .iter()
        .map(|(pixel, _, expected)| (gap(rgb(&high, *pixel), expected.encoded), *pixel))
        .max_by_key(|(deviation, _)| *deviation)
        .unwrap_or_default();
    let same_as_off = quad_hits
        .iter()
        .filter(|(pixel, ..)| gap(rgb(&high, *pixel), rgb(&off, *pixel)) <= 16)
        .count();
    // Every visible water fragment, whatever its march met (the quad, the
    // wall, nothing, or a stop in the screen-edge fade band), reads its CPU
    // composite too.
    let (mut every_worst, mut fading) = ((0u8, UVec2::ZERO), 0);
    for pixel in pixels() {
        let Some(water) = visible_water(view, &scene, pixel, 0.0) else {
            continue;
        };
        let expected = reference.smooth(water);
        if expected.march.confidence > 0.0 && expected.march.confidence < 1.0 {
            fading += 1;
        }
        let deviation = gap(rgb(&high, pixel), expected.encoded);
        if deviation > every_worst.0 {
            every_worst = (deviation, pixel);
        }
    }

    // Medium: a half-extent target, read bilinearly, marched by medium's
    // own preset.
    renderer.set_reflection_quality(FluidReflectionQuality::Medium);
    let low = capture_checked(&mut renderer, view);
    let target = read_reflection_target(&renderer);
    assert_eq!(target.extent, UVec2::splat(FRAME / 2));
    let medium = Reference::new(view, &scene, march_tuning(&renderer));
    let mut low_worst = (0u8, UVec2::ZERO);
    let mut interior = 0;
    for (pixel, water, _) in &quad_hits {
        let screen = smooth_through_target(&medium, *pixel, target.extent);
        let want = medium.composite(FLUID_REFLECTION_STYLE_SMOOTH, *water, Vec2::ZERO, screen);
        let deviation = gap(rgb(&low, *pixel), want);
        if deviation > low_worst.0 {
            low_worst = (deviation, *pixel);
        }
        let whole = bilinear_footprint(*pixel, target.extent)
            .into_iter()
            .filter(|(_, weight)| *weight > 0.0)
            .all(|(texel, _)| {
                let at = (texel.as_vec2() + 0.5) * FRAME as f32 / target.extent.as_vec2();
                on_plane(view, at, 0.0)
                    .filter(|point| on_pool(*point, 0.0))
                    .is_some_and(|point| {
                        let texel = medium.smooth(point);
                        hits_quad(&scene, &texel) && texel.march.confidence >= 1.0
                    })
            });
        if whole {
            interior += 1;
            let probe_only = medium.probe_only(FLUID_REFLECTION_STYLE_SMOOTH, *water, Vec2::ZERO);
            assert!(
                gap(rgb(&low, *pixel), probe_only) > 16,
                "medium water {pixel} whose footprint all hits the quad reads it, not the probe: \
                 {:?} vs {probe_only:?}",
                rgb(&low, *pixel),
            );
        }
    }
    eprintln!(
        "[FluidSurfaceProof] fluid-ssr-reflections row 14 standing-quad SSR GPU case ran: CPU \
         march on read-back depth \
         hits the quad at {} water pixels ({faded} in a fade band), high worst {} at {}, {} \
         within 16/255 of Off; every water pixel worst {} at {} ({fading} with 0 < c < 1); \
         medium worst {} at {}, {interior} with a whole-quad footprint",
        quad_hits.len(),
        worst.0,
        worst.1,
        same_as_off,
        every_worst.0,
        every_worst.1,
        low_worst.0,
        low_worst.1
    );
    assert!(quad_hits.len() >= 40, "the quad's reflection covers water");
    assert!(
        worst.0 <= 2,
        "high reads the CPU composite: off by {} at {}",
        worst.0,
        worst.1
    );
    assert_eq!(same_as_off, 0, "every quad-hit fragment differs from Off");
    assert!(
        every_worst.0 <= 2 && fading >= 10,
        "every water fragment reads its CPU composite, fades included: off by {} at {}, \
         {fading} fading",
        every_worst.0,
        every_worst.1
    );
    assert!(
        low_worst.0 <= 2,
        "medium reads the CPU composite of its footprint: off by {} at {}",
        low_worst.0,
        low_worst.1
    );
    assert!(
        interior >= 10,
        "medium keeps whole-quad footprints: {interior}"
    );

    // Refused faces, each against the same frame with reflectivity 0 (the
    // reflection-free variant). From below: the gate shuts in both passes.
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    let below = view_from(Vec3::new(0.0, -1.0, 0.0), Vec3::new(0.0, -0.3, 5.0));
    let drawn = |renderer: &mut Renderer, reflectivity: f32, view: View| {
        renderer.set_fluid_surface_draws(&[look_draw(0, bound_look(0.5, reflectivity))]);
        capture_checked(renderer, view)
    };
    let records = pass_records(&renderer);
    let below_reflecting = drawn(&mut renderer, 50.0, below);
    assert_eq!(
        renderer
            .full()
            .fluid_surface
            .frame_variants(LightTermMask::ALL),
        [FluidShaderVariant::Reflecting]
    );
    assert_eq!(
        pass_records(&renderer),
        records + 1,
        "the pass ran and drew nothing"
    );
    let below_free = drawn(&mut renderer, 0.0, below);
    assert!(
        below_reflecting == below_free,
        "a face seen from below draws reflection-free"
    );

    // An isolating mask: neither pass reflects (P9).
    let mut isolating = LightTermMask::ALL;
    isolating.set_enabled(LightTermMask::SPECULAR, false);
    renderer.set_light_term_mask(isolating);
    let records = pass_records(&renderer);
    let isolated_reflecting = drawn(&mut renderer, 50.0, view);
    let isolated_free = drawn(&mut renderer, 0.0, view);
    assert_eq!(
        pass_records(&renderer),
        records,
        "an isolating mask records no pass"
    );
    assert!(
        isolated_reflecting == isolated_free,
        "an isolating mask draws reflection-free"
    );
    renderer.set_light_term_mask(LightTermMask::ALL);

    // A face bound to no probe beside a bound one, in one draw: the left
    // face binds the probe, the right one, under the quad's reflection,
    // binds nothing.
    let cut = -1.0;
    renderer.install_fluid_surfaces(
        &[pool(0, 0.0, &[POOL_X[0], cut, POOL_X[1]])],
        Some(&green_probe(&[(0, 0)])),
    );
    let split_reflecting = drawn(&mut renderer, 50.0, view);
    let split_free = drawn(&mut renderer, 0.0, view);
    let (mut unbound, mut unbound_would_hit, mut bound_moved) = (0, 0, 0);
    for pixel in pixels() {
        let Some(water) = visible_water(view, &scene, pixel, 0.0) else {
            continue;
        };
        if water.x > cut + 0.05 {
            unbound += 1;
            assert_eq!(
                rgb(&split_reflecting, pixel),
                rgb(&split_free, pixel),
                "the unbound face at {pixel} draws reflection-free"
            );
            if hits_quad(&scene, &reference.smooth(water)) {
                unbound_would_hit += 1;
            }
        } else if water.x < cut - 0.05 && rgb(&split_reflecting, pixel) != rgb(&split_free, pixel) {
            bound_moved += 1;
        }
    }
    eprintln!(
        "[FluidSurfaceProof] fluid-ssr-reflections row 14 refused-face GPU case ran: below and \
         isolating frames \
         byte-equal, {unbound} unbound-face pixels byte-equal ({unbound_would_hit} would reflect \
         the quad if bound), {bound_moved} bound-face pixels reflect"
    );
    assert!(unbound_would_hit >= 20 && bound_moved >= 100);
    renderer.release_level_resources();
}

// fluid-ssr-reflections rows 1, 2 and 19, the low tier, on the standing-quad
// scene in smooth style. Stepping high → low releases the target and
// records no reflection pass, yet the draw keeps the reflecting variant: every
// visible water fragment reads the CPU probe-only composite (c = 0) within
// 2/255, every fragment whose high march hits the quad differs from the high
// frame, and the probe term still differs from Off.
#[test]
fn the_low_tier_draws_the_probe_only_composite_and_drops_the_ssr_layer() {
    let Some(mut renderer) = ssr_renderer() else {
        return;
    };
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    install_base_level(&mut renderer);
    let view = base_view();
    let (off, scene) = off_frame(&mut renderer, view);
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    let high = capture_checked(&mut renderer, view);
    let reference = Reference::new(view, &scene, march_tuning(&renderer));

    renderer.set_reflection_quality(FluidReflectionQuality::Low);
    let (records, targets) = (pass_records(&renderer), target_builds(&renderer));
    let low = capture_checked(&mut renderer, view);
    assert_eq!(pass_records(&renderer), records, "low records no pass");
    assert_eq!(target_builds(&renderer), targets, "low creates nothing");
    assert_eq!(
        renderer.full().fluid_surface.reflection_target_extent(),
        None,
        "low keeps only the placeholder"
    );
    assert_eq!(
        renderer
            .full()
            .fluid_surface
            .scene_inputs()
            .pending()
            .ssr_scale,
        [0.0; 2]
    );
    assert_eq!(
        renderer
            .full()
            .fluid_surface
            .frame_variants(LightTermMask::ALL),
        [FluidShaderVariant::Reflecting]
    );

    let (mut water, mut quad_hits, mut differs_from_high, mut differs_from_off) = (0, 0, 0, 0);
    let mut worst = (0u8, UVec2::ZERO);
    for pixel in pixels() {
        let Some(point) = visible_water(view, &scene, pixel, 0.0) else {
            continue;
        };
        water += 1;
        let probe_only = reference.probe_only(FLUID_REFLECTION_STYLE_SMOOTH, point, Vec2::ZERO);
        let deviation = gap(rgb(&low, pixel), probe_only);
        if deviation > worst.0 {
            worst = (deviation, pixel);
        }
        if gap(rgb(&low, pixel), rgb(&off, pixel)) > 16 {
            differs_from_off += 1;
        }
        if hits_quad(&scene, &reference.smooth(point)) {
            quad_hits += 1;
            if gap(rgb(&low, pixel), rgb(&high, pixel)) > 16 {
                differs_from_high += 1;
            }
        }
    }
    eprintln!(
        "[FluidSurfaceProof] fluid-ssr-reflections low-tier GPU case ran: {water} water \
         pixels, probe-only worst {} at {}, {differs_from_off} differ from Off, \
         {differs_from_high} of {quad_hits} quad hits differ from high",
        worst.0, worst.1
    );
    assert!(water >= 100, "the pool covers the frame");
    assert!(
        worst.0 <= 2,
        "low reads the CPU probe-only composite: off by {} at {}",
        worst.0,
        worst.1
    );
    assert!(
        quad_hits >= 40,
        "the quad's reflection covers water at high"
    );
    assert_eq!(
        differs_from_high, quad_hits,
        "every quad hit at high loses its SSR at low"
    );
    assert_eq!(
        differs_from_off, water,
        "the probe term draws at every water fragment at low"
    );
    renderer.release_level_resources();
}

/// The visible water fragments of each pixel-style cell through `view`,
/// keyed by cell; fragments within a hundredth of a texel of a cell edge
/// are left out, so the GPU's cell agrees.
fn visible_cells(
    view: View,
    scene: &SceneReadback,
) -> std::collections::BTreeMap<(i32, i32), Vec<UVec2>> {
    let mut cells = std::collections::BTreeMap::<(i32, i32), Vec<UVec2>>::new();
    for pixel in pixels() {
        let Some(water) = visible_water(view, scene, pixel, 0.0) else {
            continue;
        };
        if cell_margin(water) < 0.01 {
            continue;
        }
        let cell = cell_of(water);
        cells.entry((cell.x, cell.y)).or_default().push(pixel);
    }
    cells
}

/// The largest per-channel spread across `fragments` of one frame.
fn spread(frame: &[u8], fragments: &[UVec2]) -> u8 {
    (0..3)
        .map(|channel| {
            let values = fragments.iter().map(|pixel| rgb(frame, *pixel)[channel]);
            values.clone().max().unwrap_or(0) - values.min().unwrap_or(0)
        })
        .max()
        .unwrap_or(0)
}

fn on_screen(at: Vec2) -> bool {
    at.cmpge(Vec2::ZERO).all() && at.cmplt(Vec2::splat(FRAME as f32)).all()
}

// fluid-ssr-reflections row 15, P15. Pixel style, the frame capture's default, over the
// base level with 1 m water texel cells and SSR hitting in view. At every
// screen-space tier every visible fragment of each cell reads one value, and
// every cell whose CPU march at that tier's preset (from its centre, about
// its tilted normal, on the read-back depth) hits differs from the same
// frame at Off. At high
// each cell reads its CPU composite within 2/255. A cell whose centre the
// standing quad hides, with fragments visible beside the quad, reads its own
// cell's SSR at every visible fragment: its march starts behind the quad,
// skips that starting overlap and hits the wall above the quad's top, never
// the quad's back. A cell whose centre is off screen, or past the pool's
// edge, reads c = 0, the probe term alone, though the past-edge cell's own
// fragments wrote hits to the target.
#[test]
fn pixel_style_reads_one_reflection_per_cell_and_hidden_or_lost_centres_read_their_own() {
    use postretro_render_cpu::fluid_surface::reflection_style::FLUID_REFLECTION_STYLE_PIXEL;
    let Some(mut renderer) = ssr_renderer() else {
        return;
    };
    assert_eq!(
        renderer.fluid_reflection_style(),
        FluidReflectionStyle::Pixel
    );
    install_base_level(&mut renderer);
    let view = base_view();
    let (off, scene) = off_frame(&mut renderer, view);
    let cells = visible_cells(view, &scene);
    let cell_index = |cell: (i32, i32)| glam::IVec2::new(cell.0, cell.1);

    let mut hit_cells = 0;
    for quality in [
        FluidReflectionQuality::Medium,
        FluidReflectionQuality::VeryHigh,
        FluidReflectionQuality::Ultra,
        FluidReflectionQuality::High,
    ] {
        renderer.set_reflection_quality(quality);
        // Each tier marches its own preset.
        let reference = Reference::new(view, &scene, march_tuning(&renderer));
        let frame = capture_checked(&mut renderer, view);
        let mut worst_spread = (0u8, (0, 0));
        hit_cells = 0;
        for (&cell, fragments) in &cells {
            let cell_spread = spread(&frame, fragments);
            if cell_spread > worst_spread.0 {
                worst_spread = (cell_spread, cell);
            }
            let centre = cell_centre(cell_index(cell), 0.0);
            let (at, _) = project(view, centre);
            if !(on_pool(centre, 0.0) && on_screen(at))
                || reference.pixel_cell(cell_index(cell), 0.0).march.confidence <= 0.0
            {
                continue;
            }
            hit_cells += 1;
            for pixel in fragments {
                assert!(
                    rgb(&frame, *pixel) != rgb(&off, *pixel),
                    "{quality:?}: hit cell {cell:?} at {pixel} differs from Off"
                );
            }
        }
        eprintln!(
            "[FluidSurfaceProof] fluid-ssr-reflections row 15 pixel-cell GPU case ran at \
             {quality:?}: {} cells, worst \
             spread {} in {:?}, {hit_cells} hit cells differ from Off",
            cells.len(),
            worst_spread.0,
            worst_spread.1
        );
        assert!(
            worst_spread.0 <= 1,
            "{quality:?}: every fragment of a cell reads one value: {worst_spread:?}"
        );
    }
    assert!(hit_cells >= 8, "SSR hits in view: {hit_cells} cells");

    // High, the tier left set: each cell against the CPU references, by
    // where its centre lies.
    assert_eq!(renderer.reflection_quality(), FluidReflectionQuality::High);
    let reference = Reference::new(view, &scene, march_tuning(&renderer));
    let frame = capture_checked(&mut renderer, view);
    let target = read_reflection_target(&renderer);
    let (mut matched, mut worst) = (0, (0u8, (0, 0)));
    let (mut hidden, mut hidden_hits, mut off_screen, mut past_edge, mut past_edge_wrote) =
        (0, 0, 0, 0, 0);
    for (&cell, fragments) in &cells {
        let centre = cell_centre(cell_index(cell), 0.0);
        let texel_uv = Vec2::new(cell.0 as f32, cell.1 as f32) + 0.5;
        let (at, depth) = project(view, centre);
        let own = reference.pixel_cell(cell_index(cell), 0.0);
        let want = if !on_screen(at) || !on_pool(centre, 0.0) {
            // Off screen or past the edge: the probe term alone.
            if fragments.len() < 2 {
                continue;
            }
            if on_screen(at) {
                past_edge += 1;
                let wrote = fragments.iter().any(|pixel| {
                    target
                        .at(target_texel(
                            super::fluid_reflection_frame_test_fixtures::centre(*pixel),
                            target.extent,
                        ))
                        .w
                        > 0.5
                });
                past_edge_wrote += usize::from(wrote);
            } else {
                off_screen += 1;
            }
            reference.probe_only(FLUID_REFLECTION_STYLE_PIXEL, centre, texel_uv)
        } else {
            if depth > scene.depth_at(at.as_uvec2()) + 1.0e-4 && fragments.len() >= 3 {
                // Hidden behind the quad: its own SSR, never the quad's
                // back, the colour a hit at its start would read.
                hidden += 1;
                let back = reference.composite(
                    FLUID_REFLECTION_STYLE_PIXEL,
                    centre,
                    texel_uv,
                    scene.colour_at(at.as_uvec2()).extend(1.0),
                );
                assert!(
                    gap(own.encoded, back) > 16,
                    "hidden cell {cell:?}: its own SSR {:?} reads apart from the quad's back \
                     {back:?}",
                    own.encoded
                );
                if own.march.confidence >= 0.5 {
                    hidden_hits += 1;
                }
            }
            own.encoded
        };
        matched += 1;
        for pixel in fragments {
            let deviation = gap(rgb(&frame, *pixel), want);
            if deviation > worst.0 {
                worst = (deviation, cell);
            }
        }
    }
    eprintln!(
        "[FluidSurfaceProof] fluid-ssr-reflections row 15 pixel-centre GPU case ran: {matched} \
         cells against the CPU, \
         worst {} in {:?}; {hidden} hidden-centre cells ({hidden_hits} hitting), {off_screen} \
         off-screen-centre cells, {past_edge} past-edge cells ({past_edge_wrote} whose own \
         fragments wrote hits)",
        worst.0, worst.1
    );
    assert!(worst.0 <= 2, "each cell reads its CPU composite: {worst:?}");
    assert!(
        hidden_hits >= 1,
        "a hidden-centre cell reflects a hit of its own"
    );
    assert!(off_screen >= 1 && past_edge >= 1 && past_edge_wrote >= 1);
    renderer.release_level_resources();
}

/// Every visible water fragment of the base pool through `reference`'s
/// view against its smooth CPU composite: the worst deviation and where,
/// and how many fragments' marches hit the quad.
fn smooth_worst(frame: &[u8], reference: &Reference<'_>) -> ((u8, UVec2), usize) {
    let (mut worst, mut quad) = ((0u8, UVec2::ZERO), 0);
    for pixel in pixels() {
        let Some(water) = visible_water(reference.view, reference.scene, pixel, 0.0) else {
            continue;
        };
        let expected = reference.smooth(water);
        quad += usize::from(hits_quad(reference.scene, &expected));
        let deviation = gap(rgb(frame, pixel), expected.encoded);
        if deviation > worst.0 {
            worst = (deviation, pixel);
        }
    }
    (worst, quad)
}

// fluid-ssr-reflections row 16, P16. Two reflecting pools share target texels: the
// visible one at height 0 and a hidden one 0.5 m beneath it, under an opaque
// floor between them, both bound to the probe, in smooth style at high.
// Drawn visible first, then hidden (the order a far-to-near sort by box
// centre can give), every visible fragment reads its own pool's SSR, the
// CPU composite of its own march on the read-back depth within 2/255, though
// the hidden pool's march at the same pixels would read apart; the reverse
// order draws the same frame byte for byte. The pass's own depth test
// decides each texel's owner, not draw order.
#[test]
fn overlapping_reflecting_surfaces_each_read_their_own_reflection_in_either_draw_order() {
    let Some(mut renderer) = ssr_renderer() else {
        return;
    };
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    install_world_quads(
        &mut renderer,
        &[
            standing_quad(),
            back_wall(),
            level([-7.0, 4.0], [0.0, 9.0], -0.25, BLACK_UV),
        ],
    );
    renderer.install_fluid_surfaces(
        &[pool(0, 0.0, &POOL_X), pool(1, -0.5, &POOL_X)],
        Some(&green_probe(&[(0, 0), (1, 0)])),
    );
    let view = base_view();
    renderer.set_reflection_quality(FluidReflectionQuality::Off);
    renderer.set_fluid_surface_draws(&[look_draw(0, mirror()), look_draw(1, mirror())]);
    capture_checked(&mut renderer, view);
    let scene = read_scene(&renderer);
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    let visible_first = capture_checked(&mut renderer, view);
    assert_eq!(
        renderer
            .full()
            .fluid_surface
            .frame_variants(LightTermMask::ALL),
        [FluidShaderVariant::Reflecting; 2],
        "both pools reflect, so both draw into the target"
    );
    renderer.set_fluid_surface_draws(&[look_draw(1, mirror()), look_draw(0, mirror())]);
    let hidden_first = capture_checked(&mut renderer, view);

    let reference = Reference::new(view, &scene, march_tuning(&renderer));
    let (worst, quad) = smooth_worst(&visible_first, &reference);
    let apart = pixels()
        .filter(|pixel| {
            let Some(water) = visible_water(view, &scene, *pixel, 0.0) else {
                return false;
            };
            let under = on_plane(view, centre(*pixel), -0.5).expect("the hidden pool");
            gap(
                reference.smooth(water).encoded,
                reference.smooth(under).encoded,
            ) > 16
        })
        .count();
    eprintln!(
        "[FluidSurfaceProof] fluid-ssr-reflections row 16 overlapping-surfaces GPU case ran: \
         visible-first worst {} at \
         {} ({quad} quad hits), {apart} pixels where the hidden pool's SSR reads apart, orders \
         byte-equal: {}",
        worst.0,
        worst.1,
        visible_first == hidden_first
    );
    assert!(
        worst.0 <= 2,
        "each visible fragment reads its own SSR: {worst:?}"
    );
    assert!(
        quad >= 40 && apart >= 40,
        "the two pools' reflections differ"
    );
    assert!(
        visible_first == hidden_first,
        "draw order never decides a texel's owner"
    );
    renderer.release_level_resources();
}

// fluid-ssr-reflections row 17, P17. On the base level at high, smooth style, the
// probe-bound pool drawn at reflectivity 0 takes the reflection-free variant
// and records no reflection pass. Raising its reflectivity through
// `set_fluid_surface_draws`, the per-frame collect a staged reload or tuning
// payload feeds, shows SSR on the next frame: the pass records and every
// water fragment reads its CPU composite, the quad's reflection included.
// Neither change creates a target or rebuilds group 5. Lowering it to 0
// draws the reflection-free frame byte for byte again.
#[test]
fn raising_reflectivity_from_zero_shows_ssr_next_frame_without_a_rebuild() {
    let Some(mut renderer) = ssr_renderer() else {
        return;
    };
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    install_base_level(&mut renderer);
    let view = base_view();
    let (_, scene) = off_frame(&mut renderer, view);
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    let (targets, group_5) = (target_builds(&renderer), group_5_builds(&renderer));
    let variants = |renderer: &Renderer| {
        renderer
            .full()
            .fluid_surface
            .frame_variants(LightTermMask::ALL)
    };

    renderer.set_fluid_surface_draws(&[look_draw(0, mirror_at(0.0))]);
    let records = pass_records(&renderer);
    let dull = capture_checked(&mut renderer, view);
    assert_eq!(variants(&renderer), [FluidShaderVariant::ReflectionFree]);
    assert_eq!(pass_records(&renderer), records);

    renderer.set_fluid_surface_draws(&[look_draw(0, mirror())]);
    let raised = capture_checked(&mut renderer, view);
    assert_eq!(variants(&renderer), [FluidShaderVariant::Reflecting]);
    assert_eq!(
        pass_records(&renderer),
        records + 1,
        "the next frame records the pass"
    );
    let reference = Reference::new(view, &scene, march_tuning(&renderer));
    let (worst, quad) = smooth_worst(&raised, &reference);

    renderer.set_fluid_surface_draws(&[look_draw(0, mirror_at(0.0))]);
    let lowered = capture_checked(&mut renderer, view);
    assert_eq!(variants(&renderer), [FluidShaderVariant::ReflectionFree]);
    eprintln!(
        "[FluidSurfaceProof] fluid-ssr-reflections row 17 live-reflectivity GPU case ran: raised \
         frame worst {} at {} \
         ({quad} quad hits), lowered frame byte-equal: {}, target builds {} → {}, group 5 \
         builds {} → {}",
        worst.0,
        worst.1,
        lowered == dull,
        targets,
        target_builds(&renderer),
        group_5,
        group_5_builds(&renderer)
    );
    assert!(worst.0 <= 2 && quad >= 40, "SSR shows on the next frame");
    assert_eq!(pass_records(&renderer), records + 1, "0 records no pass");
    assert!(lowered == dull, "0 draws the reflection-free frame");
    assert_eq!(target_builds(&renderer), targets, "no target rebuild");
    assert_eq!(group_5_builds(&renderer), group_5, "no group 5 rebuild");
    renderer.release_level_resources();
}

/// The base level at high in `style` on a fresh renderer.
fn fresh_base_level(style: FluidReflectionStyle) -> Option<Renderer> {
    let mut renderer = ssr_renderer()?;
    renderer.set_fluid_reflection_style(style);
    install_base_level(&mut renderer);
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    renderer.set_fluid_surface_draws(&[look_draw(0, mirror())]);
    Some(renderer)
}

// fluid-ssr-reflections row 18, P7, first half. In each style, a frame of the base
// level whose hits reach the pool's right edge: the target texels there hold
// c > 0.5. Then the camera moves 1 m right, so those texels lie off the
// water: the next frame's target holds c = 0 at every one of them, and the
// frame equals that same frame drawn by a fresh renderer byte for byte.
#[test]
fn a_texel_no_reflecting_draw_wrote_this_frame_reads_no_earlier_hit() {
    for style in FluidReflectionStyle::ALL {
        let Some(mut renderer) = fresh_base_level(style) else {
            return;
        };
        let first = base_view();
        capture_checked(&mut renderer, first);
        let target = read_reflection_target(&renderer);
        assert_eq!(target.extent, UVec2::splat(FRAME));
        let moved = view_from(Vec3::new(1.0, 2.0, 0.0), Vec3::new(1.0, 0.5, 5.0));
        let edge: Vec<UVec2> = pixels()
            .filter(|pixel| {
                let at = centre(*pixel);
                target.at(pixel.as_ivec2()).w > 0.5
                    && on_plane(first, at, 0.0)
                        .is_some_and(|point| on_pool(point, 0.0) && point.x > POOL_X[1] - 0.3)
            })
            .collect();
        for pixel in &edge {
            let point = on_plane(moved, centre(*pixel), 0.0).expect("the plane");
            assert!(
                !on_pool(point, 0.0),
                "{pixel} lies off the water after the move"
            );
        }
        let after = capture_checked(&mut renderer, moved);
        let target = read_reflection_target(&renderer);
        for pixel in &edge {
            assert_eq!(
                target.at(pixel.as_ivec2()),
                Vec4::ZERO,
                "{style:?}: {pixel} reads c = 0"
            );
        }
        let fresh = {
            let Some(mut fresh) = fresh_base_level(style) else {
                return;
            };
            capture_checked(&mut fresh, moved)
        };
        eprintln!(
            "[FluidSurfaceProof] fluid-ssr-reflections row 18 unwritten-texel GPU case ran \
             ({style:?}): {} edge texels \
             held hits, then lay off the water and read c = 0; equals a fresh renderer: {}",
            edge.len(),
            after == fresh
        );
        assert!(edge.len() >= 4, "{style:?}: hits reach the water's edge");
        assert!(after == fresh, "{style:?}: no earlier hit survives");
    }
}

// fluid-ssr-reflections row 18, second half. Smooth style at medium, the back wall
// magenta so the pool's edge reflects a colour: the target is half the scene
// extent, so a water fragment at the pool's right edge reads a bilinear
// footprint straddling texels the pass wrote with hits and texels past the
// edge it never wrote (c = 0). Each such fragment reads the CPU composite of
// the read-back texels' bilinear mix within 2/255, which the premultiplied
// target keeps between the probe-only composite and the composite of each
// written texel: never darker than both, as a raw-colour target, filtered
// and then multiplied by its filtered confidence, would be.
#[test]
fn a_footprint_straddling_written_and_unwritten_texels_reads_between_probe_and_ssr() {
    let Some(mut renderer) = ssr_renderer() else {
        return;
    };
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    install_world_quads(
        &mut renderer,
        &[
            standing_quad(),
            upright([-6.0, 6.0], [0.0, 4.0], POOL_Z[1], MAGENTA_UV),
        ],
    );
    renderer.install_fluid_surfaces(&[base_pool()], Some(&green_probe(&[(0, 0)])));
    let view = base_view();
    let (_, scene) = off_frame(&mut renderer, view);
    renderer.set_reflection_quality(FluidReflectionQuality::Medium);
    let frame = capture_checked(&mut renderer, view);
    let target = read_reflection_target(&renderer);
    assert_eq!(target.extent, UVec2::splat(FRAME / 2));
    let reference = Reference::new(view, &scene, march_tuning(&renderer));
    let (mut straddling, mut worst, mut outside) = (0, (0u8, UVec2::ZERO), 0);
    for pixel in pixels() {
        let Some(water) = visible_water(view, &scene, pixel, 0.0) else {
            continue;
        };
        let footprint: Vec<_> = bilinear_footprint(pixel, target.extent)
            .into_iter()
            .filter(|(_, weight)| *weight > 0.05)
            .collect();
        let written: Vec<Vec4> = footprint
            .iter()
            .map(|(texel, _)| target.at(*texel))
            .filter(|value| value.w >= 0.25)
            .collect();
        let unwritten = footprint.iter().any(|(texel, _)| {
            let at = (texel.as_vec2() + 0.5) * FRAME as f32 / target.extent.as_vec2();
            target.at(*texel) == Vec4::ZERO
                && on_plane(view, at, 0.0).is_none_or(|point| !on_pool(point, 0.0))
        });
        if written.is_empty() || !unwritten {
            continue;
        }
        straddling += 1;
        let mix: Vec4 = bilinear_footprint(pixel, target.extent)
            .into_iter()
            .map(|(texel, weight)| target.at(texel) * weight)
            .sum();
        let got = rgb(&frame, pixel);
        let want = reference.composite(FLUID_REFLECTION_STYLE_SMOOTH, water, Vec2::ZERO, mix);
        let deviation = gap(got, want);
        if deviation > worst.0 {
            worst = (deviation, pixel);
        }
        let bounds: Vec<[u8; 3]> = written
            .iter()
            .map(|value| {
                reference.composite(FLUID_REFLECTION_STYLE_SMOOTH, water, Vec2::ZERO, *value)
            })
            .chain([reference.probe_only(FLUID_REFLECTION_STYLE_SMOOTH, water, Vec2::ZERO)])
            .collect();
        let between = (0..3).all(|channel| {
            let low = bounds.iter().map(|b| b[channel]).min().unwrap();
            let high = bounds.iter().map(|b| b[channel]).max().unwrap();
            got[channel] + 2 >= low && got[channel] <= high.saturating_add(2)
        });
        outside += usize::from(!between);
    }
    eprintln!(
        "[FluidSurfaceProof] fluid-ssr-reflections row 18 bilinear-straddle GPU case ran: \
         {straddling} edge fragments \
         straddle written and unwritten texels, worst {} at {} against the CPU mix, {outside} \
         outside the probe-to-SSR range",
        worst.0, worst.1
    );
    assert!(
        straddling >= 3,
        "the pool's edge straddles written and unwritten texels"
    );
    assert!(
        worst.0 <= 2,
        "the straddle reads the premultiplied mix: {worst:?}"
    );
    assert_eq!(
        outside, 0,
        "never darker than both, never outside the range"
    );
}

// fluid-ssr-reflections row 22, P8. With SSR hitting at high, a style switch between
// two frames draws the new style whole on the first frame after it: that
// frame equals the next (steady) frame of the new style, and a fresh
// renderer's frame in that style, byte for byte, both ways. The scene and
// reflection uniforms upload before the reflection pass, so the reflection
// pass marches and the fluid pass samples by one style in one frame.
#[test]
fn a_style_switch_draws_the_new_style_whole_on_its_first_frame() {
    use postretro_render_cpu::fluid_surface::reflection_style::FLUID_REFLECTION_STYLE_PIXEL;
    let Some(mut renderer) = fresh_base_level(FluidReflectionStyle::Pixel) else {
        return;
    };
    let view = base_view();
    let styles = |renderer: &Renderer| {
        let fluid = &renderer.full().fluid_surface;
        (
            fluid.scene_inputs().uploaded().reflection_style,
            fluid.reflection_uploaded_uniform().0.reflection_style,
        )
    };
    let hits = |renderer: &Renderer| {
        read_reflection_target(renderer)
            .texels
            .iter()
            .filter(|texel| texel.w > 0.5)
            .count()
    };
    capture_checked(&mut renderer, view);
    let pixel_steady = capture_checked(&mut renderer, view);
    let pixel_hits = hits(&renderer);

    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    let smooth_first = capture_checked(&mut renderer, view);
    assert_eq!(
        styles(&renderer),
        (FLUID_REFLECTION_STYLE_SMOOTH, FLUID_REFLECTION_STYLE_SMOOTH),
        "both passes drew smooth on the switch frame"
    );
    let smooth_hits = hits(&renderer);
    let smooth_steady = capture_checked(&mut renderer, view);
    let smooth_fresh = {
        let Some(mut fresh) = fresh_base_level(FluidReflectionStyle::Smooth) else {
            return;
        };
        capture_checked(&mut fresh, view)
    };

    renderer.set_fluid_reflection_style(FluidReflectionStyle::Pixel);
    let pixel_first = capture_checked(&mut renderer, view);
    assert_eq!(
        styles(&renderer),
        (FLUID_REFLECTION_STYLE_PIXEL, FLUID_REFLECTION_STYLE_PIXEL)
    );
    eprintln!(
        "[FluidSurfaceProof] fluid-ssr-reflections row 22 style-switch GPU case ran: {pixel_hits} \
         pixel and \
         {smooth_hits} smooth target texels hit; pixel → smooth first frame equals steady: {}, \
         fresh: {}; smooth → pixel first frame equals steady: {}",
        smooth_first == smooth_steady,
        smooth_first == smooth_fresh,
        pixel_first == pixel_steady
    );
    assert!(
        pixel_hits >= 40 && smooth_hits >= 40,
        "SSR hits in both styles"
    );
    assert!(smooth_first != pixel_steady, "the styles draw apart");
    assert!(smooth_first == smooth_steady && smooth_first == smooth_fresh);
    assert!(pixel_first == pixel_steady);
    renderer.release_level_resources();
}

// fluid-ssr-reflections row 23, P10. In each style, the base level at high through a
// 2° view along the water, down the middle of a column of water texel
// cells: from 1 mm above the plane, the water within half a degree under
// the horizon reflects the standing quad's base, so the target holds SSR
// hits and the frame differs from Off; 1 mm below the plane, the gate shuts
// in both passes and the frame equals the same frame at Off byte for byte.
#[test]
fn the_camera_a_millimetre_below_the_plane_draws_the_off_frame() {
    for style in FluidReflectionStyle::ALL {
        let Some(mut renderer) = fresh_base_level(style) else {
            return;
        };
        let eye = |height: f32| Vec3::new(0.5 / POOL_TEXELS_PER_METRE, height, 0.0);
        let along = |height: f32| view_through(eye(height), eye(height) + Vec3::Z, 2.0);
        let (above, below) = (along(1.0e-3), along(-1.0e-3));
        let over = capture_checked(&mut renderer, above);
        let hits = read_reflection_target(&renderer)
            .texels
            .iter()
            .filter(|texel| texel.w > 0.5)
            .count();
        let records = pass_records(&renderer);
        let under = capture_checked(&mut renderer, below);
        assert_eq!(
            pass_records(&renderer),
            records + 1,
            "the pass ran and drew nothing"
        );
        renderer.set_reflection_quality(FluidReflectionQuality::Off);
        let over_off = capture_checked(&mut renderer, above);
        let off = capture_checked(&mut renderer, below);
        eprintln!(
            "[FluidSurfaceProof] fluid-ssr-reflections row 23 below-plane GPU case ran \
             ({style:?}): {hits} target \
             texels hit from 1 mm above, that frame differs from Off: {}; 1 mm below equals \
             Off: {}",
            over != over_off,
            under == off
        );
        assert!(
            hits >= 20 && over != over_off,
            "{style:?}: SSR hits in view"
        );
        assert!(
            under == off,
            "{style:?}: below the plane draws the Off frame"
        );
    }
}

// Dev tuning. The march's step budget, thickness and stride divisor
// reach the reflection uniform alone: setting one changes the pending
// uniform, which uploads once on the next frame and not again until another
// change, and builds no target or group 5. A one-step budget marches no hit,
// so every water fragment reads its CPU composite at that budget (the probe
// term alone); restoring the defaults restores the frame byte for byte.
// Degenerate values clamp: a zero budget or divisor to 1, a negative
// thickness to 0, and a NaN thickness keeps the last.
#[test]
fn reflection_march_tuning_writes_only_the_reflection_uniform() {
    let Some(mut renderer) = ssr_renderer() else {
        return;
    };
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    install_base_level(&mut renderer);
    let view = base_view();
    let (_, scene) = off_frame(&mut renderer, view);
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    let defaults = capture_checked(&mut renderer, view);
    let (budget, thickness, rows) = (
        renderer.reflection_march_step_budget(),
        renderer.reflection_march_thickness(),
        renderer.reflection_march_rows_per_stride_pixel(),
    );
    // High's preset.
    assert_eq!((budget, thickness, rows), (96, 1.0, 270));
    let (targets, group_5) = (target_builds(&renderer), group_5_builds(&renderer));
    let uploads = |renderer: &Renderer| {
        renderer
            .full()
            .fluid_surface
            .reflection_uploaded_uniform()
            .1
    };
    let settled = uploads(&renderer);
    capture_checked(&mut renderer, view);
    assert_eq!(
        uploads(&renderer),
        settled,
        "an unchanged uniform writes nothing"
    );

    renderer.set_reflection_march_step_budget(1);
    assert_eq!(march_tuning(&renderer).budget, 1);
    assert_eq!(
        uploads(&renderer),
        settled,
        "the setter writes nothing itself"
    );
    let one_step = capture_checked(&mut renderer, view);
    assert_eq!(
        uploads(&renderer),
        settled + 1,
        "the next frame writes it once"
    );
    capture_checked(&mut renderer, view);
    assert_eq!(uploads(&renderer), settled + 1);
    let reference = Reference::new(view, &scene, march_tuning(&renderer));
    let (worst, quad) = smooth_worst(&one_step, &reference);
    assert!(
        worst.0 <= 2 && quad == 0,
        "one step hits nothing: {worst:?}, {quad}"
    );
    assert!(one_step != defaults);

    renderer.set_reflection_march_step_budget(0);
    renderer.set_reflection_march_thickness(-1.0);
    renderer.set_reflection_march_rows_per_stride_pixel(0);
    assert_eq!(renderer.reflection_march_step_budget(), 1);
    assert_eq!(renderer.reflection_march_thickness(), 0.0);
    assert_eq!(renderer.reflection_march_rows_per_stride_pixel(), 1);
    assert_eq!(march_tuning(&renderer).stride, FRAME as f32);
    renderer.set_reflection_march_thickness(f32::NAN);
    assert_eq!(renderer.reflection_march_thickness(), 0.0);

    renderer.set_reflection_march_step_budget(budget);
    renderer.set_reflection_march_thickness(thickness);
    renderer.set_reflection_march_rows_per_stride_pixel(rows);
    let restored = capture_checked(&mut renderer, view);
    eprintln!(
        "[FluidSurfaceProof] dev-tuning GPU case ran: one-step frame worst {} at {} \
         against its CPU composite, restored frame byte-equal: {}, {} uniform writes",
        worst.0,
        worst.1,
        restored == defaults,
        uploads(&renderer)
    );
    assert!(restored == defaults, "the defaults restore the frame");
    assert_eq!(target_builds(&renderer), targets);
    assert_eq!(group_5_builds(&renderer), group_5);
    renderer.release_level_resources();
}

// Tier presets. High, very high and ultra share a fraction, so switching
// between them builds no target and no group 5: the reflection uniform alone
// changes, to the new tier's preset march, uploaded once on the next frame,
// and each frame reads its CPU composite at that tier's preset. A tier change
// resets a dev override to the new tier's preset; the tier it holds keeps it.
// Returning to high restores high's frame byte for byte.
#[test]
fn switching_between_same_fraction_tiers_changes_only_the_reflection_uniform() {
    let Some(mut renderer) = ssr_renderer() else {
        return;
    };
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    install_base_level(&mut renderer);
    let view = base_view();
    let (_, scene) = off_frame(&mut renderer, view);
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    let high = capture_checked(&mut renderer, view);
    let (targets, group_5) = (target_builds(&renderer), group_5_builds(&renderer));
    let target = renderer.full().fluid_surface.reflection_target_extent();
    assert_eq!(target, Some((FRAME, FRAME)));
    let uploads = |renderer: &Renderer| {
        renderer
            .full()
            .fluid_surface
            .reflection_uploaded_uniform()
            .1
    };
    let tuning = |renderer: &Renderer| {
        (
            renderer.reflection_march_step_budget(),
            renderer.reflection_march_rows_per_stride_pixel(),
            renderer.reflection_march_thickness(),
        )
    };
    let preset = |quality: FluidReflectionQuality| {
        let march = quality.preset().expect("a screen-space tier").march;
        (
            march.step_budget,
            march.rows_per_stride_pixel,
            march.thickness,
        )
    };
    assert_eq!(tuning(&renderer), preset(FluidReflectionQuality::High));

    let mut worsts = Vec::new();
    for quality in [
        FluidReflectionQuality::VeryHigh,
        FluidReflectionQuality::Ultra,
    ] {
        let settled = uploads(&renderer);
        renderer.set_reflection_quality(quality);
        assert_eq!(tuning(&renderer), preset(quality), "{quality:?}'s preset");
        assert_eq!(uploads(&renderer), settled, "the setter writes nothing");
        let frame = capture_checked(&mut renderer, view);
        assert_eq!(
            uploads(&renderer),
            settled + 1,
            "{quality:?}: the next frame writes the uniform once"
        );
        let reference = Reference::new(view, &scene, march_tuning(&renderer));
        let (worst, quad) = smooth_worst(&frame, &reference);
        assert!(
            worst.0 <= 2 && quad >= 40,
            "{quality:?} reads its CPU composite: {worst:?}, {quad} quad hits"
        );
        worsts.push((quality, worst.0, quad));
        assert_eq!(target_builds(&renderer), targets, "{quality:?}: no target");
        assert_eq!(
            group_5_builds(&renderer),
            group_5,
            "{quality:?}: no group 5"
        );
        assert_eq!(
            renderer.full().fluid_surface.reflection_target_extent(),
            target
        );
    }

    // A dev override survives the tier it holds and resets on a change.
    renderer.set_reflection_march_step_budget(7);
    renderer.set_reflection_quality(FluidReflectionQuality::Ultra);
    assert_eq!(
        renderer.reflection_march_step_budget(),
        7,
        "the tier it holds"
    );
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    assert_eq!(tuning(&renderer), preset(FluidReflectionQuality::High));
    let restored = capture_checked(&mut renderer, view);
    eprintln!(
        "[FluidSurfaceProof] same-fraction tier GPU case ran: {worsts:?} (tier, worst, quad \
         hits), high restored byte-equal: {}, {} target and {} group 5 builds unchanged",
        restored == high,
        target_builds(&renderer),
        group_5_builds(&renderer)
    );
    assert!(restored == high, "high's preset restores high's frame");
    assert_eq!(target_builds(&renderer), targets);
    assert_eq!(group_5_builds(&renderer), group_5);
    renderer.release_level_resources();
}
