//! GPU gates for the live-lighting sums pass: its dispatch, its sums, and
//! frames against the per-fragment oracle (the reflecting variant with the
//! kept per-fragment ratio).
//! See: context/lib/rendering_pipeline.md §7.3a (Live lighting)

use glam::Vec3;
use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_level_format::sh_volume::AnimationDescriptor;
use postretro_render_cpu::fluid_surface::FluidSurfaceLook;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;
use postretro_render_cpu::frame_uniforms::LightTermMask;
use postretro_render_cpu::planar_reflection::{MirrorCamera, PlanarPreset, planar_preset};
use postretro_visibility::VisibleCells;

use super::Renderer;
use super::fluid_reflection_frame_test_fixtures::{
    FRAME, POOL_X, View, base_view, mirror, pool, read_scene, view_from,
};
use super::fluid_reflection_sums_frame_test_fixtures::*;
use super::fluid_surface_frame_test::look_draw;
use super::gpu_create::gpu_creation_counts;
use super::gpu_test_harness::offscreen_renderer_or_skip;
use super::level_resource_set::PreviousLevelSet;
use super::renderer_render_frame::FrameScene;

/// The frames' animation clock, where a case does not set its own.
const T: f32 = 1.0;

/// The sums prefill standing in for stale or uninitialised contents.
const POISON: f32 = 1.0e6;

/// Each roster light's reference: the value its probes stored it at.
const REFS: [[f32; 3]; 4] = [
    [1.0, 1.0, 1.0],
    [0.8, 0.6, 0.4],
    [0.5, 0.9, 1.2],
    [1.5, 1.5, 1.5],
];

fn renderer(test: &str) -> Option<Renderer> {
    offscreen_renderer_or_skip(test, FRAME, FRAME)
}

/// Every roster light on at its reference: the probes read their captures.
fn roster_at_reference() -> Vec<AnimationDescriptor> {
    REFS.iter()
        .map(|&reference| flat_light(reference, true))
        .collect()
}

/// The pool split at x = −0.5 into two volumes, left and right, both on
/// screen from [`base_view`].
fn two_pools() -> Vec<EnvironmentVolumeRecord> {
    vec![
        pool(0, 0.0, &[POOL_X[0], -0.5]),
        pool(1, 0.0, &[-0.5, POOL_X[1]]),
    ]
}

/// The two pools, the left bound to probe 0 and the right to probe 1, and a
/// third probe bound to no face; each probe stores two roster lights.
fn basic_level() -> SumsLevel {
    SumsLevel {
        roster: roster_at_reference(),
        volumes: two_pools(),
        probes: probe_section(
            vec![
                probe(
                    0,
                    &[
                        stored_light(0, REFS[0], 0.5, 0.6),
                        stored_light(1, REFS[1], 0.25, 0.1),
                    ],
                ),
                probe(
                    1,
                    &[
                        stored_light(1, REFS[1], 0.4, 0.3),
                        stored_light(2, REFS[2], 0.3, 0.5),
                    ],
                ),
                probe(
                    2,
                    &[
                        stored_light(2, REFS[2], 0.6, 0.6),
                        stored_light(3, REFS[3], 0.2, 0.4),
                    ],
                ),
            ],
            &[(0, 0, 0), (1, 0, 1)],
        ),
    }
}

/// Both pools drawn as probe-bound mirrors.
fn draw_both(renderer: &mut Renderer) {
    renderer.set_fluid_surface_draws(&[look_draw(0, mirror()), look_draw(1, mirror())]);
}

/// Rewrite roster light `slot` through the bridge's descriptor write, as a
/// script's light change arrives, keeping `roster` as the GPU will hold it
/// after the frame's flush.
fn force(
    renderer: &mut Renderer,
    roster: &mut [AnimationDescriptor],
    slot: usize,
    light: AnimationDescriptor,
) {
    renderer.write_animated_compose_descriptor(slot as u32, &descriptor_record(&light));
    roster[slot] = light;
}

fn ssr_records(renderer: &Renderer) -> u32 {
    renderer.full().fluid_surface.reflection_pass_records()
}

/// Assert the frame matches the per-fragment oracle's within 1/255 per
/// channel.
fn assert_matches_before(production: &[u8], before: &[u8], case: &str) -> u8 {
    let (gap, pixel) = worst_gap(production, before);
    assert!(
        gap <= 1,
        "{case}: off the per-fragment oracle by {gap}/255 at pixel {pixel}"
    );
    gap
}

/// Assert `frame` shows a forced light: at least `min` pixels differ from
/// the at-reference frame by more than 8/255.
fn assert_shows_light(frame: &[u8], at_reference: &[u8], min: usize, case: &str) -> usize {
    let differing = pixels_differing(frame, at_reference, 8);
    assert!(
        differing >= min,
        "{case}: the forced light shows on only {differing} pixels"
    );
    differing
}

/// Assert the active level's sums, read back now, match the CPU reference
/// over `roster` at `time` for every installed probe.
fn assert_sums_current(
    renderer: &Renderer,
    level: &SumsLevel,
    roster: &[AnimationDescriptor],
    time: f32,
    case: &str,
) -> f32 {
    let gpu = read_active_sums(renderer);
    let (worst, at) = worst_sums_deviation(&gpu, &level.cpu_sums(roster, time));
    assert!(
        worst <= SUMS_TOLERANCE,
        "{case}: sums off the CPU reference by {worst} at (probe, lobe) {at:?}"
    );
    worst
}

/// Select the pool's plane (height 0) for the planar layer through `view`.
fn select_pool_plane(renderer: &mut Renderer, view: View) {
    let projection =
        glam::camera::rh::proj::directx::perspective(60.0_f32.to_radians(), 1.0, 0.1, 100.0);
    let main_view = projection.inverse() * view.view_proj;
    let camera = MirrorCamera::new(view.eye, main_view, projection, 0.0, None)
        .expect("the eye is above the plane");
    renderer.set_planar_reflection_view(Some(camera), &VisibleCells::DrawAll);
}

/// The per-fragment oracle is the reflecting variant with the kept
/// per-fragment ratio by construction: its source differs from production's
/// only in the reflecting fragment's one ratio call, restored to the kept
/// per-fragment ratio, so it can never silently equal production.
#[test]
fn the_build_before_oracle_restores_exactly_the_one_per_fragment_ratio_call() {
    let production = super::fluid_surface::composed_shader_source();
    let oracle = super::fluid_surface::per_fragment_oracle::per_fragment_ratio_oracle_source();
    let calls = |source: &str, name: &str| {
        source
            .match_indices(name)
            .filter(|&(at, _)| !source[..at].ends_with("fn "))
            .count()
    };
    let (from_sums, per_fragment) = (
        "fluid_reflection_ratio_from_up_sums(",
        "fluid_reflection_ratio(",
    );
    assert_eq!(
        calls(production, from_sums),
        1,
        "production blends the sums"
    );
    assert_eq!(
        calls(production, per_fragment),
        0,
        "production never reaches the per-fragment ratio"
    );
    assert_eq!(calls(&oracle, from_sums), 0, "the oracle reads no sums");
    assert_eq!(
        calls(&oracle, per_fragment),
        1,
        "the oracle evaluates its lights"
    );
    assert_eq!(oracle.len() + "_from_up_sums".len(), production.len());
    let differs_at = production
        .bytes()
        .zip(oracle.bytes())
        .position(|(a, b)| a != b)
        .expect("the sources differ");
    assert!(
        production[differs_at..].starts_with("_from_up_sums(probe, search.lookup)")
            && production[differs_at + "_from_up_sums".len()..] == oracle[differs_at..],
        "the sources differ only in the one call"
    );
    eprintln!("[FluidSumsProof] oracle substitution: 1 call site restored at byte {differs_at}");
}

// AC row 12 (P2). No fluid level, reflection-free fluid only, a lighting mask
// shutting every reflecting draw, and the Off tier dispatch no sums pass; a
// frame with reflecting draws dispatches exactly one, whatever their count.
#[test]
fn sums_pass_dispatches_once_per_reflecting_frame_and_never_without_a_reflecting_draw() {
    let test = "sums_pass_dispatches_once_per_reflecting_frame_and_never_without_a_reflecting_draw";
    let Some(mut renderer) = renderer(test) else {
        return;
    };
    let view = base_view();
    let frame_dispatches = |renderer: &mut Renderer| {
        let before = dispatches(renderer);
        frame_at(renderer, view, T);
        dispatches(renderer) - before
    };

    renderer.install_empty_level_for_test();
    let no_fluid = frame_dispatches(&mut renderer);
    assert_eq!(no_fluid, 0, "no fluid level");

    // Eight volumes across the pool, volume i bound to probe i % 3.
    let edges: Vec<f32> = (0..=8)
        .map(|i| POOL_X[0] + (POOL_X[1] - POOL_X[0]) * i as f32 / 8.0)
        .collect();
    let level = SumsLevel {
        volumes: (0..8)
            .map(|i| pool(i, 0.0, &edges[i as usize..i as usize + 2]))
            .collect(),
        probes: probe_section(
            basic_level().probes.probes,
            &(0..8).map(|i| (i, 0, i % 3)).collect::<Vec<_>>(),
        ),
        ..basic_level()
    };
    level.install(&mut renderer);

    let plain = FluidSurfaceLook::surface_only(0.5);
    let zero_reflectivity = FluidSurfaceLook {
        surface_reflectivity: 0.0,
        ..mirror()
    };
    renderer.set_fluid_surface_draws(&[look_draw(0, plain), look_draw(1, zero_reflectivity)]);
    let reflection_free = frame_dispatches(&mut renderer);
    assert_eq!(reflection_free, 0, "reflection-free fluid only");

    let all_draws: Vec<_> = (0..8).map(|i| look_draw(i, mirror())).collect();
    renderer.set_fluid_surface_draws(&all_draws);
    let mut isolating = LightTermMask::ALL;
    isolating.set_enabled(LightTermMask::SPECULAR, false);
    renderer.set_light_term_mask(isolating);
    let masked = frame_dispatches(&mut renderer);
    assert_eq!(masked, 0, "a mask shutting every reflecting draw");
    renderer.set_light_term_mask(LightTermMask::ALL);

    renderer.set_reflection_quality(FluidReflectionQuality::Off);
    let off = frame_dispatches(&mut renderer);
    assert_eq!(off, 0, "the Off tier");
    renderer.set_reflection_quality(FluidReflectionQuality::High);

    let mut per_count = Vec::new();
    for count in [1usize, 3, 8] {
        renderer.set_fluid_surface_draws(&all_draws[..count]);
        let one = frame_dispatches(&mut renderer);
        assert_eq!(one, 1, "{count} reflecting draws dispatch once");
        per_count.push((count, one));
    }
    // Reflecting and reflection-free draws in one frame: still one.
    renderer.set_fluid_surface_draws(&[
        look_draw(0, mirror()),
        look_draw(1, plain),
        look_draw(2, mirror()),
    ]);
    let mixed = frame_dispatches(&mut renderer);
    assert_eq!(mixed, 1, "a mixed draw set dispatches once");
    eprintln!(
        "[FluidSumsProof] row 12 dispatches: no fluid {no_fluid}, reflection-free {reflection_free}, \
         masked {masked}, off {off}, reflecting (draws, dispatches) {per_count:?}, mixed {mixed}"
    );
}

// AC row 13 (P6). The dispatch follows the fluid pass's reflecting draws, not
// the screen-space pass's: at Low (no SSR pass) and at very high with every
// reflecting draw on the selected plane (no SSR pass), one dispatch, and the
// frame matches the build before (the per-fragment oracle) with a stored
// light forced off its reference, and shows that light against the same
// tier's frame at reference; a compose-only frame with the level held
// dispatches none.
#[test]
fn sums_pass_follows_the_fluid_pass_reflecting_draws_at_low_and_on_the_selected_plane() {
    let test = "sums_pass_follows_the_fluid_pass_reflecting_draws_at_low_and_on_the_selected_plane";
    let Some(mut renderer) = renderer(test) else {
        return;
    };
    let view = base_view();
    let level = basic_level();
    level.install(&mut renderer);
    draw_both(&mut renderer);
    let mut oracle = Oracle::new(&renderer);
    let mut roster = level.roster.clone();

    let mut report = Vec::new();
    for (case, quality) in [
        ("low", FluidReflectionQuality::Low),
        (
            "very high, selected plane",
            FluidReflectionQuality::VeryHigh,
        ),
    ] {
        renderer.set_reflection_quality(quality);
        let planar = quality == FluidReflectionQuality::VeryHigh;
        if planar {
            assert!(renderer.set_planar_preset_override(Some(PlanarPreset {
                scale: 1.0,
                ..planar_preset(quality).expect("a planar tier")
            })));
            select_pool_plane(&mut renderer, view);
        }
        // At reference on this tier and route, so the light, not the tier,
        // is what the forced frame shows.
        for slot in [0, 2] {
            force(&mut renderer, &mut roster, slot, level.roster[slot].clone());
        }
        let at_reference = frame_at(&mut renderer, view, T);
        force(
            &mut renderer,
            &mut roster,
            0,
            flat_light([0.2, 0.5, 1.4], true),
        );
        force(
            &mut renderer,
            &mut roster,
            2,
            flat_light([1.1, 0.2, 0.6], true),
        );
        let (sums, ssr, mirrors) = (
            dispatches(&renderer),
            ssr_records(&renderer),
            renderer.planar_reflection_records(),
        );
        let production = frame_at(&mut renderer, view, T);
        assert_eq!(dispatches(&renderer), sums + 1, "{case}: one dispatch");
        assert_eq!(ssr_records(&renderer), ssr, "{case}: no SSR pass");
        assert_eq!(
            renderer.planar_reflection_records(),
            mirrors + u32::from(planar),
            "{case}: the planar pass records only on the planar tier"
        );
        assert_sums_current(&renderer, &level, &roster, T, case);
        let before = oracle.before(&mut renderer, |renderer| frame_at(renderer, view, T));
        let gap = assert_matches_before(&production, &before, case);
        let shown = assert_shows_light(&production, &at_reference, 100, case);
        report.push((case, gap, shown));
    }

    let sums = dispatches(&renderer);
    frame_split_at_upload(&mut renderer, view, T, FrameScene::ComposeOnly, |_| {});
    let compose_only = dispatches(&renderer) - sums;
    assert!(renderer.has_level_resources(), "the level is held");
    assert_eq!(compose_only, 0, "a compose-only frame dispatches none");
    eprintln!(
        "[FluidSumsProof] row 13 (case, worst gap, pixels showing the light): {report:?}; \
         compose-only dispatches {compose_only}"
    );
}

// AC row 13, dispatch condition (P6). A mask set between a frame's uniform
// upload and its render moves neither the variants nor the dispatch: the
// frame draws and dispatches by the uploaded mask, and the next upload takes
// the new one.
#[test]
fn a_mask_set_after_the_uniform_upload_moves_neither_variants_nor_dispatch_until_the_next() {
    let test =
        "a_mask_set_after_the_uniform_upload_moves_neither_variants_nor_dispatch_until_the_next";
    let Some(mut renderer) = renderer(test) else {
        return;
    };
    let view = base_view();
    let level = basic_level();
    level.install(&mut renderer);
    draw_both(&mut renderer);
    let mut roster = level.roster.clone();
    force(
        &mut renderer,
        &mut roster,
        0,
        flat_light([0.2, 0.5, 1.4], true),
    );
    let mut isolating = LightTermMask::ALL;
    isolating.set_enabled(LightTermMask::SPECULAR, false);

    let scene_after = |renderer: &mut Renderer, frame: &dyn Fn(&mut Renderer)| {
        let before = dispatches(renderer);
        frame(renderer);
        (dispatches(renderer) - before, read_scene(renderer).colour)
    };
    let worst = |a: &[Vec3], b: &[Vec3]| {
        a.iter()
            .zip(b)
            .map(|(a, b)| (*a - *b).abs().max_element())
            .fold(0.0f32, f32::max)
    };

    renderer.set_light_term_mask(LightTermMask::ALL);
    let (all_dispatch, all) = scene_after(&mut renderer, &|r| {
        frame_at(r, view, T);
    });
    renderer.set_light_term_mask(isolating);
    let (isolated_dispatch, isolated) = scene_after(&mut renderer, &|r| {
        frame_at(r, view, T);
    });
    assert_eq!((all_dispatch, isolated_dispatch), (1, 0));
    assert!(
        worst(&all, &isolated) > 0.05,
        "the two masks draw different variants"
    );

    // Uploaded with every term, the mask isolated before the render.
    renderer.set_light_term_mask(LightTermMask::ALL);
    let (split_dispatch, split) = scene_after(&mut renderer, &|r| {
        frame_split_at_upload(r, view, T, FrameScene::World, |r| {
            r.set_light_term_mask(isolating)
        });
    });
    let split_gap = worst(&split, &all);
    assert_eq!(split_dispatch, 1, "the uploaded mask dispatches");
    assert!(
        split_gap <= 1.0e-5,
        "the uploaded mask's variants: {split_gap}"
    );
    let (next_dispatch, next) = scene_after(&mut renderer, &|r| {
        frame_at(r, view, T);
    });
    assert_eq!(next_dispatch, 0, "the next upload takes the isolating mask");
    assert!(worst(&next, &isolated) <= 1.0e-5);

    // Uploaded isolated, every term set before the render.
    let (reverse_dispatch, reverse) = scene_after(&mut renderer, &|r| {
        frame_split_at_upload(r, view, T, FrameScene::World, |r| {
            r.set_light_term_mask(LightTermMask::ALL)
        });
    });
    let reverse_gap = worst(&reverse, &isolated);
    assert_eq!(
        reverse_dispatch, 0,
        "the uploaded isolating mask dispatches none"
    );
    assert!(
        reverse_gap <= 1.0e-5,
        "the uploaded mask's variants: {reverse_gap}"
    );
    let (last_dispatch, last) = scene_after(&mut renderer, &|r| {
        frame_at(r, view, T);
    });
    assert_eq!(last_dispatch, 1, "the next upload takes every term");
    assert!(worst(&last, &all) <= 1.0e-5);
    eprintln!(
        "[FluidSumsProof] row 13 mask snapshot: split frames dispatch {split_dispatch} then \
         {reverse_dispatch} (uploaded masks), next frames {next_dispatch} then {last_dispatch}; \
         scene gaps {split_gap} and {reverse_gap}"
    );
}

// AC row 14 (P1). A script switches a stored light off on frame N, through
// the light bridge's snapshot and the frame's one descriptor flush: frame N's
// reflection drops it, as the per-fragment oracle's does, and N's sums carry
// it off.
#[test]
fn a_script_switching_a_stored_light_off_drops_it_from_the_reflection_on_that_frame() {
    let test = "a_script_switching_a_stored_light_off_drops_it_from_the_reflection_on_that_frame";
    let Some(mut renderer) = renderer(test) else {
        return;
    };
    let view = base_view();
    let level = basic_level();
    level.install(&mut renderer);
    draw_both(&mut renderer);
    let mut oracle = Oracle::new(&renderer);
    let mut roster = level.roster.clone();
    let on = frame_at(&mut renderer, view, T);

    // Frame N: the script's write, then the frame.
    let off = flat_light(REFS[1], false);
    assert!(
        renderer.upload_light_bridge_snapshot(
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[(1, descriptor_record(&off))]
        ),
        "the bridge commits the script's snapshot"
    );
    roster[1] = off;
    let production = frame_at(&mut renderer, view, T);
    let worst_sums = assert_sums_current(&renderer, &level, &roster, T, "frame N");
    let before = oracle.before(&mut renderer, |renderer| frame_at(renderer, view, T));
    let gap = assert_matches_before(&production, &before, "frame N");
    // Light 1 is stored by both drawn probes; off, both pools read darker.
    let dropped = assert_shows_light(&production, &on, 300, "frame N");
    let sums = &level.cpu_sums(&roster, T);
    assert!(
        sums[0][1].x < -0.2 && sums[1][1].x < -0.3,
        "the light's share leaves both probes: {:?}",
        &sums[..2]
    );
    eprintln!(
        "[FluidSumsProof] row 14 script switch-off: frame N differs from N-1 on {dropped} pixels, \
         worst gap to the per-fragment oracle {gap}/255, worst sums deviation {worst_sums}"
    );
}

// AC row 15 (P3). A reflecting face first drawn this frame (after a camera
// turn, or out from behind a wall: absent from last frame's draws), with a
// stored light of its probe forced off its reference since, shows that light
// on that frame, as the per-fragment oracle does. On a frame whose drawn
// faces all bind one probe, the undrawn probes' sums carry their forced
// lights.
#[test]
fn a_reflecting_face_first_drawn_this_frame_shows_its_forced_light_and_undrawn_sums_are_current() {
    let test = "a_reflecting_face_first_drawn_this_frame_shows_its_forced_light_and_undrawn_sums_are_current";
    let Some(mut renderer) = renderer(test) else {
        return;
    };
    let view = base_view();
    // Turned left, away from the right pool.
    let turned = view_from(Vec3::new(0.0, 2.0, 0.0), Vec3::new(-6.0, 0.5, 3.0));
    let level = basic_level();
    level.install(&mut renderer);
    let mut oracle = Oracle::new(&renderer);
    let mut roster = level.roster.clone();
    draw_both(&mut renderer);
    let at_reference = frame_at(&mut renderer, view, T);

    let mut report = Vec::new();
    for (case, previous_view, forced) in [
        ("camera turn", turned, flat_light([0.3, 1.2, 0.1], true)),
        ("behind a wall", view, flat_light([2.0, 0.1, 0.9], true)),
    ] {
        // Frame N−1 draws only the left pool, which binds probe 0 alone.
        // The right pool's probe (1) and the unbound probe (2) are undrawn,
        // their lights forced first.
        renderer.set_fluid_surface_draws(&[look_draw(0, mirror())]);
        force(
            &mut renderer,
            &mut roster,
            3,
            flat_light([0.4, 2.2, 1.0], true),
        );
        force(
            &mut renderer,
            &mut roster,
            2,
            flat_light([0.1, 0.3, 2.5], true),
        );
        frame_at(&mut renderer, previous_view, T);
        let undrawn = assert_sums_current(&renderer, &level, &roster, T, case);
        let sums = level.cpu_sums(&roster, T);
        assert!(
            sums[1]
                .iter()
                .chain(&sums[2])
                .all(|lobe| lobe.length() > 0.01),
            "{case}: the undrawn probes' forced lights move their sums"
        );

        // Frame N: probe 1's light 2 forced again, the right pool drawn.
        force(&mut renderer, &mut roster, 2, forced);
        draw_both(&mut renderer);
        let production = frame_at(&mut renderer, view, T);
        let current = assert_sums_current(&renderer, &level, &roster, T, case);
        let before = oracle.before(&mut renderer, |renderer| frame_at(renderer, view, T));
        let gap = assert_matches_before(&production, &before, case);
        let shown = assert_shows_light(&production, &at_reference, 100, case);
        report.push((case, undrawn, current, gap, shown));
    }
    eprintln!(
        "[FluidSumsProof] row 15 (case, undrawn-frame sums deviation, first-drawn sums deviation, \
         worst gap, pixels showing the light): {report:?}"
    );
}

/// Probe `p`'s stored lights for the readback level: up to four distinct
/// roster lights of eight, one past the roster on every fifth probe, with
/// per-lobe shares that vary by probe, light and lobe.
fn readback_lights(
    p: usize,
) -> Vec<postretro_level_format::reflection_probes::ReflectionProbeLightShare> {
    let count = 1 + p % 4;
    (0..count)
        .map(|k| {
            let roster_index = if k == 3 && p.is_multiple_of(5) {
                200
            } else {
                ((p + 3 * k) % 8) as u32
            };
            let mut shares =
                [[0.0; 3]; postretro_level_format::reflection_probes::REFLECTION_PROBE_LOBE_COUNT];
            for (lobe, share) in shares.iter_mut().enumerate() {
                let base = ((p * 7 + lobe * 3 + k * 5) % 11) as f32 / 10.0;
                *share = [base, (base * 0.7 + 0.1).min(1.0), (1.0 - base) * 0.8];
            }
            postretro_level_format::reflection_probes::ReflectionProbeLightShare {
                roster_index,
                reference: [0.5 + 0.1 * k as f32, 0.8, 1.0 + 0.2 * (p % 3) as f32],
                shares,
            }
        })
        .collect()
}

// AC row 16 (P3). Every installed probe's sums, read back on two consecutive
// dispatching frames with a stored light changed between them, match the CPU
// reference on both, at 1 probe, one more than a workgroup's width, and the
// cap. One face binds probe 0; every other probe is undrawn.
#[test]
fn every_installed_probes_sums_match_the_cpu_reference_on_consecutive_frames() {
    use postretro_level_format::reflection_probes::MAX_REFLECTION_PROBES;
    use postretro_render_cpu::fluid_surface::reflection::FLUID_PROBE_SUMS_WORKGROUP_WIDTH;
    let test = "every_installed_probes_sums_match_the_cpu_reference_on_consecutive_frames";
    let Some(mut renderer) = renderer(test) else {
        return;
    };
    let view = base_view();
    let roster: Vec<AnimationDescriptor> = (0..8)
        .map(|i| flat_light([0.6 + 0.1 * i as f32, 0.9, 1.1 - 0.05 * i as f32], true))
        .collect();
    let mut report = Vec::new();
    for probe_count in [
        1,
        FLUID_PROBE_SUMS_WORKGROUP_WIDTH as usize + 1,
        MAX_REFLECTION_PROBES,
    ] {
        let level = SumsLevel {
            roster: roster.clone(),
            volumes: vec![pool(0, 0.0, &POOL_X)],
            probes: probe_section(
                (0..probe_count)
                    .map(|p| probe(p as u32, &readback_lights(p)))
                    .collect(),
                &[(0, 0, 0)],
            ),
        };
        level.install(&mut renderer);
        renderer.set_fluid_surface_draws(&[look_draw(0, mirror())]);
        let mut live = level.roster.clone();

        let before = dispatches(&renderer);
        frame_at(&mut renderer, view, T);
        let first = assert_sums_current(&renderer, &level, &live, T, "frame 1");
        let first_sums = level.cpu_sums(&live, T);

        force(
            &mut renderer,
            &mut live,
            0,
            flat_light([1.9, 0.2, 0.7], true),
        );
        frame_at(&mut renderer, view, T);
        let second = assert_sums_current(&renderer, &level, &live, T, "frame 2");
        assert_eq!(dispatches(&renderer), before + 2, "both frames dispatch");
        let changed = level
            .cpu_sums(&live, T)
            .iter()
            .zip(&first_sums)
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            changed > 0,
            "{probe_count} probes: the change moves some sums"
        );
        report.push((probe_count, first, second, changed));
    }
    eprintln!(
        "[FluidSumsProof] row 16 (probes, worst sums deviation frame 1, frame 2, probes the \
         change moved): {report:?}"
    );
}

// AC row 17. A frame with reflecting draws creates no buffer, bind group or
// pipeline beyond a reflection-free frame's, across consecutive frames with a
// stored light changed between them.
#[test]
fn a_reflecting_frame_creates_no_gpu_object_beyond_a_reflection_free_frame() {
    let test = "a_reflecting_frame_creates_no_gpu_object_beyond_a_reflection_free_frame";
    let Some(mut renderer) = renderer(test) else {
        return;
    };
    let view = base_view();
    let level = basic_level();
    level.install(&mut renderer);
    let mut roster = level.roster.clone();
    let reflection_free = [
        look_draw(0, FluidSurfaceLook::surface_only(0.5)),
        look_draw(1, FluidSurfaceLook::surface_only(0.5)),
    ];
    // Warm both kinds of frame first.
    for _ in 0..2 {
        renderer.set_fluid_surface_draws(&reflection_free);
        frame_at(&mut renderer, view, T);
        draw_both(&mut renderer);
        frame_at(&mut renderer, view, T);
    }
    let frame_creations = |renderer: &mut Renderer| {
        let before = gpu_creation_counts();
        let sums = dispatches(renderer);
        frame_at(renderer, view, T);
        (
            gpu_creation_counts().since(before),
            dispatches(renderer) - sums,
        )
    };
    renderer.set_fluid_surface_draws(&reflection_free);
    let (free, free_dispatch) = frame_creations(&mut renderer);
    assert_eq!(free_dispatch, 0);
    draw_both(&mut renderer);
    let mut reflecting = Vec::new();
    for light in [
        flat_light([0.2, 0.5, 1.4], true),
        flat_light([1.3, 0.9, 0.1], true),
        flat_light([0.0; 3], false),
    ] {
        force(&mut renderer, &mut roster, 0, light);
        let (created, dispatched) = frame_creations(&mut renderer);
        assert_eq!(dispatched, 1, "a reflecting frame dispatches");
        assert_eq!(
            (created.buffers, created.bind_groups, created.pipelines),
            (free.buffers, free.bind_groups, free.pipelines),
            "a reflecting frame creates {created:?} against a reflection-free frame's {free:?}"
        );
        reflecting.push(created);
    }
    eprintln!(
        "[FluidSumsProof] row 17 creations: reflection-free frame {free:?}, reflecting frames \
         with the light changed between {reflecting:?}"
    );
}

// AC row 18 (P4). Against the build before (the per-fragment oracle) within
// 1/255 where a stored light differs from its reference, in pixel and smooth
// style, the sums current in each; and for a stored light on an animated
// curve at time 0, then at a later time reached after a prior frame at a
// different time, its sums at that frame's time.
#[test]
fn the_reflection_matches_the_build_before_in_both_styles_and_on_an_animated_curve() {
    let test = "the_reflection_matches_the_build_before_in_both_styles_and_on_an_animated_curve";
    let Some(mut renderer) = renderer(test) else {
        return;
    };
    let view = base_view();
    let curve_base = [1.0, 0.8, 0.6];
    let mut level = basic_level();
    // Roster light 3 rides a brightness curve; probe 1 stores it at its
    // curve's unit brightness.
    level.roster[3] = AnimationDescriptor {
        period: 2.0,
        phase: 0.0,
        base_color: curve_base,
        brightness: vec![0.2, 1.6, 0.5, 1.2],
        ..flat_light(curve_base, true)
    };
    level.probes.probes[1] = probe(
        1,
        &[
            stored_light(1, REFS[1], 0.4, 0.3),
            stored_light(3, curve_base, 0.5, 0.7),
        ],
    );
    level.install(&mut renderer);
    draw_both(&mut renderer);
    let mut oracle = Oracle::new(&renderer);
    let mut roster = level.roster.clone();
    force(
        &mut renderer,
        &mut roster,
        0,
        flat_light([0.4, 0.7, 1.5], true),
    );

    let forced_sums = level.cpu_sums(&roster, T)[0];
    assert!(
        forced_sums
            .iter()
            .all(|lobe| lobe.abs().max_element() > 0.1),
        "the forced light differs from its reference: {forced_sums:?}"
    );
    let mut report = Vec::new();
    for (case, style) in [
        ("pixel", FluidReflectionStyle::Pixel),
        ("smooth", FluidReflectionStyle::Smooth),
    ] {
        renderer.set_fluid_reflection_style(style);
        let (production, before) =
            oracle.pair(&mut renderer, |renderer| frame_at(renderer, view, T));
        let sums = assert_sums_current(&renderer, &level, &roster, T, case);
        let gap = assert_matches_before(&production, &before, case);
        report.push((case.to_string(), gap, sums));
    }

    for (case, prior, time) in [
        ("time 0", None, 0.0),
        ("time 0.45 after 1.3", Some(1.3), 0.45),
    ] {
        if let Some(prior) = prior {
            frame_at(&mut renderer, view, prior);
        }
        let production = frame_at(&mut renderer, view, time);
        let sums = assert_sums_current(&renderer, &level, &roster, time, case);
        let before = oracle.before(&mut renderer, |renderer| frame_at(renderer, view, time));
        let gap = assert_matches_before(&production, &before, case);
        report.push((case.to_string(), gap, sums));
    }
    // The curve reads differently at the two frames' times, so a sum taken
    // at the prior frame's time would not match.
    let at = |time| level.cpu_sums(&roster, time)[1][0];
    assert!(
        (at(0.45) - at(1.3)).abs().max_element() > 0.1,
        "the curve moves between the frames: {} vs {}",
        at(0.45),
        at(1.3)
    );
    eprintln!("[FluidSumsProof] row 18 (case, worst gap, worst sums deviation): {report:?}");
}

// AC row 19 (P3). Sums pre-filled with a poison value render identically to
// cleared ones on the first frame after a level is published, on the main
// thread's install and on an off-main build's publish alike.
#[test]
fn sums_prefilled_with_poison_render_like_cleared_sums_on_the_first_published_frame() {
    let test = "sums_prefilled_with_poison_render_like_cleared_sums_on_the_first_published_frame";
    let Some(mut renderer) = renderer(test) else {
        return;
    };
    let view = base_view();
    let mut level = basic_level();
    level.roster[0] = flat_light([0.2, 0.5, 1.4], true);

    let first_frame = |renderer: &mut Renderer, off_main: bool, fill: f32| {
        if off_main {
            let id = level.build_off_main(renderer);
            let buffer = renderer
                .level_set(id)
                .and_then(|set| set.reflection_probes.as_ref())
                .expect("the built set holds its probes")
                .sums_buffer()
                .clone();
            let bytes: Vec<u8> = std::iter::repeat_n(
                fill.to_le_bytes(),
                postretro_render_cpu::fluid_surface::reflection::FLUID_PROBE_SUMS_BYTES / 4,
            )
            .flatten()
            .collect();
            renderer.queue.raw().write_buffer(&buffer, 0, &bytes);
            renderer.publish_level_set(id, PreviousLevelSet::Retire);
        } else {
            level.install(renderer);
            prefill_active_sums(renderer, fill);
        }
        draw_both(renderer);
        let before = dispatches(renderer);
        let pixels = frame_at(renderer, view, T);
        assert_eq!(dispatches(renderer), before + 1);
        assert_sums_current(renderer, &level, &level.roster, T, "first frame");
        pixels
    };
    let mut report = Vec::new();
    for (path, off_main) in [("install", false), ("off-main publish", true)] {
        let cleared = first_frame(&mut renderer, off_main, 0.0);
        let poisoned = first_frame(&mut renderer, off_main, POISON);
        let (gap, pixel) = worst_gap(&cleared, &poisoned);
        assert_eq!(
            cleared, poisoned,
            "{path}: poisoned sums differ by {gap}/255 at pixel {pixel}"
        );
        report.push((path, gap));
    }
    eprintln!(
        "[FluidSumsProof] row 19 poison {POISON:e} vs cleared, first frame after publish \
         (path, worst gap): {report:?}"
    );
}

/// Level B for the publish cases: five probes over three volumes, its own
/// roster of three lights, each live off its stored reference.
fn level_b() -> SumsLevel {
    let refs = [[0.6, 0.6, 0.9], [1.2, 0.4, 0.4], [0.3, 0.8, 0.5]];
    let edges = [POOL_X[0], -2.0, 0.5, POOL_X[1]];
    SumsLevel {
        roster: vec![
            flat_light([1.4, 0.2, 0.9], true),
            flat_light([0.3, 1.1, 0.6], true),
            flat_light([0.9, 0.9, 0.1], true),
        ],
        volumes: (0..3)
            .map(|i| pool(i, 0.0, &edges[i as usize..i as usize + 2]))
            .collect(),
        probes: probe_section(
            (0..5)
                .map(|p| {
                    probe(
                        p,
                        &[
                            stored_light(p % 3, refs[(p % 3) as usize], 0.5, 0.4),
                            stored_light((p + 1) % 3, refs[((p + 1) % 3) as usize], 0.2, 0.6),
                        ],
                    )
                })
                .collect(),
            &[(0, 0, 4), (1, 0, 2), (2, 0, 0)],
        ),
    }
}

fn draw_three(renderer: &mut Renderer) {
    renderer.set_fluid_surface_draws(&[
        look_draw(0, mirror()),
        look_draw(1, mirror()),
        look_draw(2, mirror()),
    ]);
}

// AC row 20 (P5). Level B, with a different probe count and different stored
// lights, built off the main thread while A draws and then published in A's
// place: the first frame after the publish reads B's probes and matches the
// build before (the per-fragment oracle); a frame drawn between B's build and
// its publish still matches A's.
#[test]
fn a_level_built_beside_the_drawing_one_reads_its_own_probes_on_its_first_published_frame() {
    let test =
        "a_level_built_beside_the_drawing_one_reads_its_own_probes_on_its_first_published_frame";
    let Some(mut renderer) = renderer(test) else {
        return;
    };
    let view = base_view();
    let a = basic_level();
    a.install(&mut renderer);
    draw_both(&mut renderer);
    let mut oracle = Oracle::new(&renderer);
    let mut a_roster = a.roster.clone();
    force(
        &mut renderer,
        &mut a_roster,
        1,
        flat_light([0.2, 1.3, 0.5], true),
    );
    let (a_frame, a_before) = oracle.pair(&mut renderer, |renderer| frame_at(renderer, view, T));
    let a_gap = assert_matches_before(&a_frame, &a_before, "A");

    // B builds on the upload thread while A draws.
    let b = level_b();
    let mut pending = b.begin_off_main(&mut renderer);
    let during = frame_at(&mut renderer, view, T);
    assert_eq!(during, a_frame, "a frame while B builds draws A's");
    let b_id = take(&mut renderer, &mut pending);
    assert_ne!(
        renderer.active_level_set(),
        Some(b_id),
        "B is built, unpublished"
    );
    let between = frame_at(&mut renderer, view, T);
    assert_sums_current(&renderer, &a, &a_roster, T, "between B's build and publish");
    let between_gap = worst_gap(&between, &a_frame).0;
    assert_eq!(
        between, a_frame,
        "a frame between B's build and publish draws A's"
    );

    renderer.publish_level_set(b_id, PreviousLevelSet::Retire);
    draw_three(&mut renderer);
    let sums = dispatches(&renderer);
    let b_frame = frame_at(&mut renderer, view, T);
    assert_eq!(dispatches(&renderer), sums + 1);
    let b_sums = assert_sums_current(&renderer, &b, &b.roster, T, "B's first frame");
    let b_before = oracle.before(&mut renderer, |renderer| frame_at(renderer, view, T));
    let b_gap = assert_matches_before(&b_frame, &b_before, "B's first frame");
    let differs = assert_shows_light(&b_frame, &a_frame, 100, "B's first frame against A's");
    eprintln!(
        "[FluidSumsProof] row 20 publish: A gap {a_gap}, between-frame gap to A {between_gap}, \
         B (5 probes) first-frame gap {b_gap}, sums deviation {b_sums}, {differs} pixels differ from A"
    );
}

// AC row 21 (P7). Level A kept resident while B is published, a stored light
// of A forced off its reference while A is unpublished, then A published
// again: the publish creates no buffer, pipeline or bind group, the publish
// through A's first frame creates no more than a steady reflecting frame of
// A, and A's first frame shows that light and matches the build before (the
// per-fragment oracle).
#[test]
fn a_level_published_again_reads_its_lights_changed_while_resident_and_builds_nothing() {
    let test = "a_level_published_again_reads_its_lights_changed_while_resident_and_builds_nothing";
    let Some(mut renderer) = renderer(test) else {
        return;
    };
    let view = base_view();
    let a = basic_level();
    a.install(&mut renderer);
    let a_id = renderer.active_level_set().expect("A is published");
    draw_both(&mut renderer);
    let mut oracle = Oracle::new(&renderer);
    let a_at_reference = frame_at(&mut renderer, view, T);
    // A steady reflecting frame of A: the draws set, then the frame.
    let steady_frame = |renderer: &mut Renderer| {
        draw_both(renderer);
        frame_at(renderer, view, T)
    };
    let creations_before = gpu_creation_counts();
    steady_frame(&mut renderer);
    let steady = gpu_creation_counts().since(creations_before);

    let b = level_b();
    let b_id = b.build_off_main(&mut renderer);
    renderer.publish_level_set(b_id, PreviousLevelSet::KeepResident);
    draw_three(&mut renderer);
    frame_at(&mut renderer, view, T);
    assert_sums_current(&renderer, &b, &b.roster, T, "B's frame");

    // Forced in A's own descriptor mirror while A is resident, unpublished.
    let mut a_roster = a.roster.clone();
    let forced = flat_light([0.1, 0.4, 1.9], true);
    renderer
        .full_mut()
        .resident_levels
        .iter_mut()
        .find(|set| set.id == a_id)
        .expect("A is resident")
        .sh_volume_resources
        .animation
        .write_descriptor(0, &descriptor_record(&forced));
    a_roster[0] = forced;

    let before = gpu_creation_counts();
    renderer.publish_level_set(a_id, PreviousLevelSet::KeepResident);
    let created = gpu_creation_counts().since(before);
    assert_eq!(
        (created.buffers, created.bind_groups, created.pipelines),
        (0, 0, 0),
        "publishing A again creates nothing: {created:?}"
    );
    let sums = dispatches(&renderer);
    let production = steady_frame(&mut renderer);
    assert_eq!(dispatches(&renderer), sums + 1);
    let through_first_frame = gpu_creation_counts().since(before);
    assert_eq!(
        (
            through_first_frame.buffers,
            through_first_frame.bind_groups,
            through_first_frame.pipelines
        ),
        (steady.buffers, steady.bind_groups, steady.pipelines),
        "publishing A again through its first frame creates {through_first_frame:?} against \
         a steady reflecting frame's {steady:?}"
    );
    let a_sums = assert_sums_current(&renderer, &a, &a_roster, T, "A's first frame");
    let a_before = oracle.before(&mut renderer, |renderer| frame_at(renderer, view, T));
    let gap = assert_matches_before(&production, &a_before, "A's first frame");
    let shown = assert_shows_light(&production, &a_at_reference, 100, "A's first frame");
    eprintln!(
        "[FluidSumsProof] row 21 re-publish: creations {created:?}, through the first frame \
         {through_first_frame:?} against a steady frame's {steady:?}, A's first frame gap {gap}, \
         sums deviation {a_sums}, {shown} pixels show the light forced while resident"
    );
}

// AC row 22. The reflection-free variant and every §7.3a Off case (a face
// seen from its down side, a face bound to no probe, reflectivity 0, a mask
// with a term off, the Off tier) draw exactly as the build before (the
// per-fragment oracle), with stored lights forced off their references. The
// four cases that draw no reflecting variant never draw the oracle either,
// so each also draws byte-identically with its lights forced and at
// reference, and with its sums poisoned and cleared.
#[test]
fn the_reflection_free_variant_and_every_off_case_draw_exactly_as_the_build_before() {
    let test = "the_reflection_free_variant_and_every_off_case_draw_exactly_as_the_build_before";
    let Some(mut renderer) = renderer(test) else {
        return;
    };
    let view = base_view();
    let below = view_from(Vec3::new(0.0, -1.5, 0.0), Vec3::new(0.0, 0.0, 5.0));
    let opaque_mirror = FluidSurfaceLook {
        surface_reflectivity: 50.0,
        has_bound_face: true,
        ..FluidSurfaceLook::surface_only(0.5)
    };
    // One volume whose on-screen face binds no probe; its only bound face
    // lies far off screen, so the volume draws the reflecting variant.
    let mut unbound_on_screen = pool(0, 0.0, &POOL_X);
    unbound_on_screen
        .surface_faces
        .push(pool(0, 0.0, &[30.0, 32.0]).surface_faces.remove(0));
    let unbound_level = SumsLevel {
        volumes: vec![unbound_on_screen],
        probes: probe_section(basic_level().probes.probes, &[(0, 1, 0)]),
        ..basic_level()
    };
    let mut isolating = LightTermMask::ALL;
    isolating.set_enabled(LightTermMask::SPECULAR, false);

    struct Case {
        name: &'static str,
        level: SumsLevel,
        look: FluidSurfaceLook,
        view: View,
        mask: LightTermMask,
        quality: FluidReflectionQuality,
        reflecting: bool,
    }
    let cases = [
        Case {
            name: "reflection-free variant",
            level: basic_level(),
            look: FluidSurfaceLook::surface_only(0.5),
            view,
            mask: LightTermMask::ALL,
            quality: FluidReflectionQuality::High,
            reflecting: false,
        },
        Case {
            name: "seen from the down side",
            level: basic_level(),
            look: opaque_mirror,
            view: below,
            mask: LightTermMask::ALL,
            quality: FluidReflectionQuality::High,
            reflecting: true,
        },
        Case {
            name: "face bound to no probe",
            level: unbound_level,
            look: opaque_mirror,
            view,
            mask: LightTermMask::ALL,
            quality: FluidReflectionQuality::High,
            reflecting: true,
        },
        Case {
            name: "reflectivity 0",
            level: basic_level(),
            look: FluidSurfaceLook {
                surface_reflectivity: 0.0,
                ..opaque_mirror
            },
            view,
            mask: LightTermMask::ALL,
            quality: FluidReflectionQuality::High,
            reflecting: false,
        },
        Case {
            name: "a mask term off",
            level: basic_level(),
            look: opaque_mirror,
            view,
            mask: isolating,
            quality: FluidReflectionQuality::High,
            reflecting: false,
        },
        Case {
            name: "Off tier",
            level: basic_level(),
            look: opaque_mirror,
            view,
            mask: LightTermMask::ALL,
            quality: FluidReflectionQuality::Off,
            reflecting: false,
        },
    ];
    let mut report = Vec::new();
    for case in cases {
        renderer.set_light_term_mask(case.mask);
        renderer.set_reflection_quality(case.quality);
        case.level.install(&mut renderer);
        let draws: Vec<_> = (0..case.level.volumes.len() as u32)
            .map(|volume| look_draw(volume, case.look))
            .collect();
        renderer.set_fluid_surface_draws(&draws);
        let at_reference = frame_at(&mut renderer, case.view, T);
        let mut roster = case.level.roster.clone();
        force(
            &mut renderer,
            &mut roster,
            0,
            flat_light([0.2, 0.5, 1.4], true),
        );
        force(
            &mut renderer,
            &mut roster,
            2,
            flat_light([1.1, 0.2, 0.6], true),
        );
        let mut oracle = Oracle::new(&renderer);
        let sums = dispatches(&renderer);
        let production = frame_at(&mut renderer, case.view, T);
        let dispatched = dispatches(&renderer) - sums;
        assert_eq!(
            dispatched,
            u32::from(case.reflecting),
            "{}: draws the reflecting variant only where its gate is shut per fragment",
            case.name
        );
        let before = oracle.before(&mut renderer, |renderer| frame_at(renderer, case.view, T));
        let (gap, pixel) = worst_gap(&production, &before);
        assert_eq!(
            production, before,
            "{}: off the per-fragment oracle by {gap}/255 at pixel {pixel}",
            case.name
        );
        if !case.reflecting {
            let (gap, pixel) = worst_gap(&production, &at_reference);
            assert_eq!(
                production, at_reference,
                "{}: the forced lights move the frame by {gap}/255 at pixel {pixel}",
                case.name
            );
            prefill_active_sums(&renderer, POISON);
            let poisoned = frame_at(&mut renderer, case.view, T);
            prefill_active_sums(&renderer, 0.0);
            let cleared = frame_at(&mut renderer, case.view, T);
            let (gap, pixel) = worst_gap(&poisoned, &cleared);
            assert_eq!(
                poisoned, cleared,
                "{}: poisoned sums differ from cleared by {gap}/255 at pixel {pixel}",
                case.name
            );
            assert_eq!(cleared, production, "{}: cleared sums", case.name);
        }
        renderer.set_fluid_surface_draws(&[]);
        let no_fluid = frame_at(&mut renderer, case.view, T);
        let drawn = pixels_differing(&production, &no_fluid, 0);
        assert!(
            drawn > 100,
            "{}: the fluid draws ({drawn} pixels)",
            case.name
        );
        report.push((case.name, dispatched, drawn));
    }
    renderer.set_light_term_mask(LightTermMask::ALL);
    eprintln!(
        "[FluidSumsProof] row 22 identical to the per-fragment oracle, and for the four \
         non-reflecting cases to the frame at reference and with poisoned sums \
         (case, dispatches, pixels drawn): \
         {report:?}"
    );
}
