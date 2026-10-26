//! Planar target lifecycle and plane-switch proofs: tiers, candidates, resize, unload, routing.
//! See: context/lib/rendering_pipeline.md §7.3a (Target lifecycle) · context/lib/testing_guide.md

use super::frame_test_fixtures::*;
use crate::render::Renderer;
use crate::render::fluid_reflection_frame_test_fixtures::{
    self as fixture, FRAME, capture_checked, gap, pixels, rgb,
};
use crate::render::fluid_surface_frame_test::look_draw;
use glam::Vec4;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;
use postretro_render_cpu::planar_reflection::PLANAR_MIN_PLANE_AREA;
use postretro_visibility::VisibleCells;

const BELOW_PLANAR: [FluidReflectionQuality; 4] = [
    FluidReflectionQuality::High,
    FluidReflectionQuality::Medium,
    FluidReflectionQuality::Low,
    FluidReflectionQuality::Off,
];

fn ssr_records(renderer: &Renderer) -> u32 {
    renderer.full().fluid_surface.reflection_pass_records()
}

fn layer(renderer: &Renderer) -> Option<(u32, u32)> {
    renderer.full().fluid_surface.planar_layer_extent()
}

/// Planar layer builds and mirror target builds since full init.
fn builds(renderer: &Renderer) -> (u32, u32) {
    (
        renderer.full().fluid_surface.planar_layer_builds(),
        renderer.planar_mirror_target_builds(),
    )
}

/// The layer and the mirror targets at `extent`, or both released.
fn assert_targets(renderer: &Renderer, extent: Option<(u32, u32)>, what: &str) {
    assert_eq!(layer(renderer), extent, "{what}: planar layer");
    assert_eq!(renderer.planar_mirror_extent(), extent, "{what}: mirror");
}

// AC 31 (renderer half, O8): at high and below, with a candidate plane
// installed and a mirror view handed in, no planar target exists and no
// planar pass records.
#[test]
fn planar_gpu_tiers_below_very_high_allocate_and_record_nothing() {
    let Some((mut renderer, view)) = standing_quad_renderer() else {
        return;
    };
    for quality in BELOW_PLANAR {
        renderer.set_reflection_quality(quality);
        assert_targets(&renderer, None, &format!("{quality:?}"));
        assert!(renderer.planar_reflection_preset().is_none());
        select(&mut renderer, view, 0.0);
        let records = renderer.planar_reflection_records();
        capture_checked(&mut renderer, view);
        assert_eq!(
            renderer.planar_reflection_records(),
            records,
            "{quality:?}: no planar pass"
        );
        assert_targets(&renderer, None, &format!("{quality:?} after a frame"));
    }
    renderer.release_level_resources();
}

// AC 32 (O8, O9, O10): at a pose with a selected plane, high → very high →
// high allocates the planar layer and mirror targets once, at the preset's
// scale, then releases them; the drop frame records no planar pass. At very
// high, frames with no selected plane keep both (O10). A resize rebuilds the
// layer, its group 5 with its scale, and the mirror targets at the new
// extent, once, also when the resize lands on the allocating frame (O9).
// Unload releases them. No pipeline is created on any of these paths
// (`planar_lifecycle_compiles_nothing` pins the call graph).
#[test]
fn planar_gpu_tier_round_trip_keeps_no_plane_frames_and_rebuilds_on_resize() {
    let Some((mut renderer, view)) = standing_quad_renderer() else {
        return;
    };
    // Very high's own preset: half scale.
    renderer.set_planar_preset_override(None);
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    assert_targets(&renderer, None, "high");
    select(&mut renderer, view, 0.0);
    let records = renderer.planar_reflection_records();
    capture_checked(&mut renderer, view);
    assert_eq!(renderer.planar_reflection_records(), records);

    let (layers, targets) = builds(&renderer);
    renderer.set_reflection_quality(FluidReflectionQuality::VeryHigh);
    let half = Some((FRAME / 2, FRAME / 2));
    assert_targets(&renderer, half, "high to very high allocates");
    assert_eq!(builds(&renderer), (layers + 1, targets + 1), "once");
    select(&mut renderer, view, 0.0);
    capture_checked(&mut renderer, view);
    assert_eq!(renderer.planar_reflection_records(), records + 1);

    // O10: selection drops for some frames at very high.
    let held = builds(&renderer);
    for _ in 0..3 {
        deselect(&mut renderer);
        let records = renderer.planar_reflection_records();
        capture_checked(&mut renderer, view);
        assert_eq!(renderer.planar_reflection_records(), records, "no plane");
        assert_targets(&renderer, half, "a frame with no plane keeps them");
        assert_eq!(builds(&renderer), held, "and reallocates nothing");
    }
    select(&mut renderer, view, 0.0);
    capture_checked(&mut renderer, view);
    assert_eq!(
        builds(&renderer),
        held,
        "the plane returns to the same target"
    );

    // The drop: released on the tier change, nothing recorded that frame.
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    assert_targets(&renderer, None, "very high to high releases");
    let records = renderer.planar_reflection_records();
    capture_checked(&mut renderer, view);
    assert_eq!(renderer.planar_reflection_records(), records);
    assert_eq!(builds(&renderer), held, "a release builds nothing");

    // O9: a resize recorded on the frame the tier allocates. The build
    // waits for the frame-start commit and lands once, at the new extent.
    renderer.record_surface_size(96, 48);
    renderer.set_reflection_quality(FluidReflectionQuality::VeryHigh);
    assert_eq!(builds(&renderer), held, "deferred to the commit");
    assert!(renderer.commit_extents().is_some());
    assert_targets(&renderer, Some((48, 24)), "built at the new extent");
    assert_eq!(
        builds(&renderer),
        (held.0 + 1, held.1 + 1),
        "one allocation"
    );
    assert_group_follows(&renderer, (48, 24));

    // A plain resize at very high rebuilds both, and the group, once.
    let before = builds(&renderer);
    renderer.record_surface_size(FRAME, FRAME);
    assert!(renderer.commit_extents().is_some());
    assert_targets(&renderer, half, "the resize rebuilds at the new extent");
    assert_eq!(builds(&renderer), (before.0 + 1, before.1 + 1));
    assert_group_follows(&renderer, (FRAME / 2, FRAME / 2));
    select(&mut renderer, view, 0.0);
    let records = renderer.planar_reflection_records();
    capture_checked(&mut renderer, view);
    assert_eq!(renderer.planar_reflection_records(), records + 1);
    assert_eq!(
        builds(&renderer),
        (before.0 + 1, before.1 + 1),
        "drawing builds nothing"
    );

    renderer.release_level_resources();
    assert_targets(&renderer, None, "unload");
}

// Regression: a build deferred to a pending scene-moving resize was lost when
// the next recorded size settled back on the committed scene, since only a
// scene-moving commit rebuilt the layer. O9's deferral must land on any
// commit: the layer and mirror targets exist after it, built once.
#[test]
fn planar_gpu_deferred_build_lands_when_the_commit_moves_no_scene() {
    let Some((mut renderer, view)) = standing_quad_renderer() else {
        return;
    };
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    assert_targets(&renderer, None, "high");
    let before = builds(&renderer);

    renderer.record_surface_size(96, 48);
    renderer.set_reflection_quality(FluidReflectionQuality::VeryHigh);
    assert_targets(&renderer, None, "deferred to the commit");
    renderer.record_surface_size(FRAME, FRAME);
    assert!(
        renderer.commit_extents().is_none(),
        "the recorded size is the committed one"
    );
    assert_targets(
        &renderer,
        Some((FRAME / 2, FRAME / 2)),
        "built on a commit that moved nothing",
    );
    assert_eq!(builds(&renderer), (before.0 + 1, before.1 + 1), "once");
    assert!(renderer.commit_extents().is_none());
    assert_eq!(
        builds(&renderer),
        (before.0 + 1, before.1 + 1),
        "a later commit builds nothing"
    );

    select(&mut renderer, view, 0.0);
    let records = renderer.planar_reflection_records();
    capture_checked(&mut renderer, view);
    assert_eq!(renderer.planar_reflection_records(), records + 1);
    renderer.release_level_resources();
}

// The preset override reaches the planar layer only on a full renderer;
// without one it changes nothing and reports nothing applied, as its sibling
// setters no-op.
#[test]
fn planar_preset_override_without_a_full_renderer_applies_nothing() {
    let Some(mut renderer) = fixture::ssr_renderer() else {
        return;
    };
    renderer.set_reflection_quality(FluidReflectionQuality::VeryHigh);
    renderer.full = None;
    assert!(!renderer.set_planar_preset_override(Some(full_scale())));
    assert!(!renderer.set_planar_preset_override(None));
}

/// The planar group 5 is built over the current scene depth and carries the
/// layer's scale against the scene.
fn assert_group_follows(renderer: &Renderer, extent: (u32, u32)) {
    let full = renderer.full();
    let group = full
        .fluid_surface
        .planar_group()
        .expect("the planar group lives with the layer");
    let scene = renderer.scene_extent();
    assert_eq!(
        group.pending().ssr_scale,
        [
            extent.0 as f32 / scene.width as f32,
            extent.1 as f32 / scene.height as f32,
        ],
        "the group's scale"
    );
    assert!(
        group.scene_depth().0 == &full.depth_view,
        "the group binds the new scene depth"
    );
}

/// A pool of `area` m² at height 0: `POOL_Z` is 7.5 m deep.
fn pool_of_area(area: f32) -> postretro_level_format::environment_volumes::EnvironmentVolumeRecord {
    let [z0, z1] = fixture::POOL_Z;
    fixture::pool(0, 0.0, &[-1.0, -1.0 + area / (z1 - z0)])
}

// AC 33: a level with no candidate plane (a puddle under the area floor)
// allocates no planar target at very high; a level with one holds it from
// install, before any plane is selected; the app's verdict from a fluid
// refresh that removes the last candidate releases it, and one that brings
// a candidate back allocates it again. With zero selected planes the planar
// pass records nothing and SSR covers the water.
#[test]
fn planar_gpu_target_lives_only_with_a_candidate_plane() {
    let Some(mut renderer) = fixture::ssr_renderer() else {
        return;
    };
    fixture::install_world_quads(&mut renderer, &[fixture::back_wall()]);
    renderer.set_reflection_quality(FluidReflectionQuality::VeryHigh);
    let probe = fixture::green_probe(&[(0, 0)]);
    renderer.install_fluid_surfaces(&[pool_of_area(PLANAR_MIN_PLANE_AREA * 0.9)], Some(&probe));
    assert_targets(&renderer, None, "a puddle-only level");
    renderer.install_fluid_surfaces(&[pool_of_area(PLANAR_MIN_PLANE_AREA * 1.1)], Some(&probe));
    assert_targets(&renderer, Some((FRAME / 2, FRAME / 2)), "a candidate");

    let held = builds(&renderer);
    renderer.set_planar_candidate(false);
    assert_targets(&renderer, None, "the refresh removed the last candidate");
    renderer.set_planar_candidate(true);
    assert_targets(&renderer, Some((FRAME / 2, FRAME / 2)), "a candidate again");
    assert_eq!(builds(&renderer), (held.0 + 1, held.1 + 1));
    renderer.set_planar_candidate(false);
    renderer.install_fluid_surfaces(&[fixture::base_pool()], Some(&probe));
    assert_targets(
        &renderer,
        None,
        "the verdict holds across the fluid install",
    );
    renderer.set_planar_candidate(true);
    assert!(layer(&renderer).is_some());

    renderer.set_fluid_surface_draws(&[look_draw(0, fixture::mirror())]);
    let view = fixture::base_view();
    deselect(&mut renderer);
    let (planar, ssr) = (renderer.planar_reflection_records(), ssr_records(&renderer));
    capture_checked(&mut renderer, view);
    assert_eq!(renderer.planar_reflection_records(), planar, "no plane");
    assert_eq!(ssr_records(&renderer), ssr + 1, "SSR covers the water");
    renderer.release_level_resources();
}

// AC 29 (O4, O5), GPU half: on the frame selection moves from the large
// plane (A) to the second plane (B), B reads this frame's mirror and A reads
// SSR in that same frame: the planar layer holds B's mirror and nothing on
// A, the SSR target holds A and nothing on B, and A's pixels match the
// SSR-only frame. The same holds when that frame's mirror walk trips: the
// app hands in the trip set instead of a walked reach (here an explicit,
// empty cell list; the fixture has no cell draw index, so its world still
// draws),
// and the routing still follows B. The app half proves the trip set itself.
#[test]
fn planar_gpu_switch_frame_routes_new_plane_to_this_mirror() {
    let Some(mut renderer) = two_plane_renderer(
        FluidReflectionQuality::VeryHigh,
        FluidReflectionStyle::Smooth,
        &two_plane_world(),
    ) else {
        return;
    };
    renderer.set_planar_preset_override(Some(full_scale()));
    let view = fixture::base_view();
    deselect(&mut renderer);
    let ssr_only = capture_checked(&mut renderer, view);
    let scene = fixture::read_scene(&renderer);

    for (case, visible) in [
        ("walked", VisibleCells::DrawAll),
        ("tripped", VisibleCells::Culled(Vec::new())),
    ] {
        select(&mut renderer, view, 0.0);
        capture_checked(&mut renderer, view);
        let on_a = read_planar_layer(&renderer)
            .texels
            .iter()
            .filter(|texel| texel.w > 0.0)
            .count();
        assert!(on_a > 0, "{case}: frame N−1 mirrors A");

        // Frame N: the switch.
        renderer.set_planar_reflection_view(Some(mirror_camera(view, SECOND_H)), &visible);
        let (planar, ssr) = (renderer.planar_reflection_records(), ssr_records(&renderer));
        let frame = capture_checked(&mut renderer, view);
        assert_eq!(renderer.planar_reflection_records(), planar + 1, "{case}");
        assert_eq!(ssr_records(&renderer), ssr + 1, "{case}: SSR records for A");
        let planar_layer = read_planar_layer(&renderer);
        let target = fixture::read_reflection_target(&renderer);
        let (mut a_pixels, mut a_ssr, mut b_pixels, mut b_planar) = (0, 0, 0, 0);
        for pixel in pixels() {
            let at = pixel.as_ivec2();
            let large = visible_on(view, &scene, pixel, 0.0, LARGE_X, 0.05);
            let near_second = on_extent(view, pixel, SECOND_H, SECOND_X, -0.05);
            if large.is_some() && near_second.is_none() {
                a_pixels += 1;
                assert_eq!(
                    planar_layer.at(at).w,
                    0.0,
                    "{case}: no mirror of A on frame N at {pixel}"
                );
                assert!(
                    gap(rgb(&frame, pixel), rgb(&ssr_only, pixel)) <= 1,
                    "{case}: A reads SSR at {pixel}"
                );
                if target.at(at).w > 0.0 {
                    a_ssr += 1;
                }
            }
            let second = visible_on(view, &scene, pixel, SECOND_H, SECOND_X, 0.05);
            let near_large = on_extent(view, pixel, 0.0, LARGE_X, -0.05);
            if second.is_some() && near_large.is_none() {
                b_pixels += 1;
                assert_eq!(
                    target.at(at),
                    Vec4::ZERO,
                    "{case}: SSR skips B, this frame's selected plane, at {pixel}"
                );
                if planar_layer.at(at).w > 0.0 {
                    b_planar += 1;
                }
            }
        }
        eprintln!(
            "[PlanarProof] switch ({case}): A {a_pixels} px, {a_ssr} SSR-written; B {b_pixels} px, \
             {b_planar} mirrored"
        );
        assert!(a_pixels > 100 && a_ssr > 20 && b_pixels > 40, "{case}");
        if case == "walked" {
            assert!(b_planar > 20, "B reads this frame's mirror");
        }
    }
    renderer.release_level_resources();
}

/// The body of the first `fn {name}` after `from` in `source`, braces
/// balanced; comments are not stripped, which the names searched never are.
fn body<'a>(source: &'a str, from: &str, name: &str) -> &'a str {
    let start = source.find(from).unwrap_or_else(|| panic!("`{from}`"));
    let source = &source[start..];
    let at = source
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("`fn {name}` after `{from}`"));
    let open = at + source[at..].find('{').expect("a body");
    let mut depth = 0;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[at..open + offset + 1];
                }
            }
            _ => {}
        }
    }
    panic!("`fn {name}` closes");
}

// AC 32 (no pipeline) and the hot-path rule: the planar lifecycle paths a
// tier change, an install, a candidate verdict, a resize or an unload take
// create no pipeline, pipeline layout or shader module (those build at full
// init, at every tier), and nothing per frame reaches them but the extent
// commit's check for a deferred build. They build textures, views and bind
// groups, and `FluidSceneInputs::new` builds one group-5 bind-group layout
// each time the planar group is allocated after a release: sharing the main
// group's layout would need a constructor the `fluid_scene_inputs.rs` pin
// forbids.
#[test]
fn planar_lifecycle_compiles_nothing_and_never_runs_per_frame() {
    let api = include_str!("api.rs");
    let routing = include_str!("../fluid_surface/planar_routing.rs");
    let targets = include_str!("targets.rs");
    let fluid = include_str!("../fluid_surface.rs");
    let inputs = include_str!("../fluid_scene_inputs.rs");
    let bodies = [
        body(api, "impl Renderer", "set_planar_candidate"),
        body(api, "impl Renderer", "sync_planar_layer"),
        body(api, "impl Renderer", "resume_deferred_planar_build"),
        body(api, "impl Renderer", "rebuild_planar_layer"),
        body(api, "impl Renderer", "apply_planar_layer"),
        body(routing, "impl FluidSurfacePass", "set_planar_layer"),
        body(routing, "impl FluidSurfacePass", "has_planar_candidate"),
        body(routing, "impl FluidSurfacePass", "set_planar_app_candidate"),
        body(routing, "", "has_candidate_plane"),
        body(routing, "impl PlanarLayer", "new"),
        body(inputs, "impl FluidSceneInputs", "new"),
        body(inputs, "impl FluidSceneInputs", "rebuild"),
        body(targets, "impl PlanarTargets", "new"),
        body(fluid, "impl Renderer", "set_reflection_quality"),
        body(fluid, "impl Renderer", "install_fluid_surfaces"),
    ];
    for text in bodies {
        for compile in [
            "create_render_pipeline(",
            "create_shader_module(",
            "create_pipeline_layout(",
            "create_compute_pipeline(",
        ] {
            assert!(!text.contains(compile), "{compile} in:\n{text}");
        }
    }
    for (file, source) in [
        (
            "renderer_render_frame.rs",
            include_str!("../renderer_render_frame.rs"),
        ),
        (
            "renderer_pre_scene.rs",
            include_str!("../renderer_pre_scene.rs"),
        ),
        ("record.rs", include_str!("record.rs")),
    ] {
        for lifecycle in [
            "sync_planar_layer(",
            "rebuild_planar_layer(",
            "set_planar_layer(",
            "PlanarTargets::new(",
            "set_planar_candidate(",
        ] {
            assert!(!source.contains(lifecycle), "{file} reaches {lifecycle}");
        }
    }
    // The rebuild follows the scene targets alone; the sync follows the
    // tier, the fluid install and the app's verdict, and the commit reaches
    // it only through a deferred build.
    let extent = include_str!("../renderer_extent.rs");
    assert_eq!(extent.matches("self.rebuild_planar_layer();").count(), 1);
    assert_eq!(
        extent
            .matches("self.resume_deferred_planar_build();")
            .count(),
        1
    );
    assert!(!extent.contains("sync_planar_layer("));
    assert_eq!(fluid.matches("self.sync_planar_layer();").count(), 2);
}
