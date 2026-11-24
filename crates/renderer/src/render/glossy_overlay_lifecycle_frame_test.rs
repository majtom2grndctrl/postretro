//! Adapter-backed lifecycle and gate proofs for the glossy overlay: the Off
//! and Low tiers, live tier changes and a level built at Off, the overlay and
//! sums gates over camera ranges, masks and fluid draws, and the first frame
//! after a resize.
//! See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Reflection quality tiers)

use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::frame_uniforms::LightTermMask;
use postretro_visibility::VisibleCells;

use super::fluid_reflection_frame_test_fixtures::mirror;
use super::fluid_reflection_sums_frame_test_fixtures::{dispatches, take};
use super::fluid_surface_frame_test::look_draw;
use super::glossy_overlay_frame_test_fixtures::{
    floor_probe, floor_view, install_floor, overlays, renderer, with_floor, with_variant,
};
use super::glossy_overlay_gate_frame_fixtures::{
    ALL, assert_composite, binding_the_pool, frame_seeing, install_floor_with,
    matte_only_cell_index, pool,
};
use super::gpu_create::{GpuCreationCounts, gpu_creation_counts};
use super::{GLOSSY_STATIC_DIFFUSE, Renderer};
use FluidReflectionQuality::{High, Low, Off};

/// The composite row over the bound face, on the frame `renderer` draws now
/// (production, then the zero-adding variant at the same pose and clock for
/// forward's colour beside its D_static). Returns what the production frame
/// created.
fn assert_draws_the_probe_term(renderer: &mut Renderer, what: &str) -> GpuCreationCounts {
    let view = floor_view();
    let before = gpu_creation_counts();
    let production = frame_seeing(renderer, view, &ALL);
    let created = gpu_creation_counts().since(before);
    let forward = with_variant(renderer, "fs_test_zero", |renderer| {
        frame_seeing(renderer, view, &ALL)
    });
    assert!(production.static_diffuse.is_some(), "{what}: D_static");
    let checked = assert_composite(view, &production, &forward, &[0], &|_| false, what);
    assert!(
        checked > 100,
        "{what}: the bound face covers the view: {checked}"
    );
    created
}

// Acceptance row 47. At reflection quality off a glossy level records no
// overlay pass, allocates no D_static and keeps forward's one output, and its
// frame is bit for bit that of the same level whose id 54 binds no world
// face (what the level was before glossy faces). At Low the bound face draws
// the probe term alone: the composite row with no screen-space share.
#[test]
fn off_draws_the_level_as_before_and_low_draws_the_probe_term_alone() {
    let Some(mut glossy) = renderer() else {
        return;
    };
    let mut before = renderer().expect("a second renderer on the same adapter");
    install_floor(&mut glossy, &floor_probe(true));
    install_floor(&mut before, &floor_probe(false));
    assert!(before.level().glossy.is_none());
    let view = floor_view();
    for renderer in [&mut glossy, &mut before] {
        renderer.set_reflection_quality(Off);
    }
    let overlays_before = overlays(&glossy);
    let off = frame_seeing(&mut glossy, view, &ALL);
    let unglossy = frame_seeing(&mut before, view, &ALL);
    assert_eq!(overlays(&glossy), overlays_before, "off records no overlay");
    assert!(off.static_diffuse.is_none(), "off allocates no D_static");
    assert_eq!(
        off.scene.colour, unglossy.scene.colour,
        "off: the frame is the level's without glossy faces"
    );
    assert_eq!(off.scene.depth, unglossy.scene.depth);

    glossy.set_reflection_quality(Low);
    assert_draws_the_probe_term(&mut glossy, "low");
}

// Acceptance row 48, pin P7 (live half). Off → low → off, live: the off
// frames record no overlay and dispatch no sums; the first low frame after
// off draws the probe term (row 60, pin P22: every glossy pixel meets the
// composite row); the second off frame is the first's bit for bit. Each
// change builds no pipeline and uploads no probe resource: only low's
// D_static target and its inputs group.
#[test]
fn off_low_off_live_builds_no_pipeline_and_draws_the_term_only_at_low() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_floor(&mut renderer, &floor_probe(true));
    let view = floor_view();
    renderer.set_reflection_quality(Off);
    let (overlays_off, sums_off) = (overlays(&renderer), dispatches(&renderer));
    let first_off = frame_seeing(&mut renderer, view, &ALL);
    assert_eq!(overlays(&renderer), overlays_off);
    assert_eq!(dispatches(&renderer), sums_off);

    let before = gpu_creation_counts();
    renderer.set_reflection_quality(Low);
    let built = gpu_creation_counts().since(before);
    assert_eq!(
        (
            built.pipelines,
            built.buffers,
            built.textures,
            built.bind_groups
        ),
        (0, 0, 1, 1),
        "off → low builds D_static and its inputs group alone: {built:?}"
    );
    let created = assert_draws_the_probe_term(&mut renderer, "first low frame after off");
    assert_eq!(overlays(&renderer), overlays_off + 2);
    assert_eq!(dispatches(&renderer), sums_off + 2);
    assert_eq!(created.pipelines, 0, "the low frame builds no pipeline");

    let before = gpu_creation_counts();
    renderer.set_reflection_quality(Off);
    assert_eq!(
        gpu_creation_counts().since(before).total(),
        0,
        "low → off builds nothing"
    );
    let second_off = frame_seeing(&mut renderer, view, &ALL);
    assert_eq!(
        overlays(&renderer),
        overlays_off + 2,
        "off again: no overlay"
    );
    assert_eq!(dispatches(&renderer), sums_off + 2, "off again: no sums");
    assert!(second_off.static_diffuse.is_none());
    assert_eq!(second_off.scene.colour, first_off.scene.colour);
    let ledger = renderer
        .reflection_probe_residency_report()
        .expect("full renderer");
    assert_eq!(ledger.bytes(GLOSSY_STATIC_DIFFUSE), None);
}

/// Build the floor level off the main thread at the renderer's current
/// tiers, returning its set's id unpublished.
fn build_floor_off_main(renderer: &mut Renderer) -> super::LevelSetId {
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
    take(renderer, &mut pending)
}

/// Publish `id`, then give the glossy faces their material, as the
/// main-thread install does.
fn publish_floor(renderer: &mut Renderer, id: super::LevelSetId) {
    renderer.publish_level_set(id, super::level_resource_set::PreviousLevelSet::Retire);
    super::glossy_overlay_frame_test_fixtures::set_floor_material(
        renderer,
        super::glossy_overlay_frame_test_fixtures::FloorMaterial::WHITE_SHARP,
    );
}

// Acceptance row 48, pin P7 (load half). A level set built at off publishes
// with no D_static (a commit applies no tier); switched to low on its commit
// frame, the next frame draws the probe term, building no pipeline. A set
// built at off whose tier rose to low before its publish allocates D_static
// at the publish and draws the term on its first frame.
#[test]
fn a_level_built_at_off_draws_the_term_the_frame_after_a_switch_to_low() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    renderer.set_reflection_quality(Off);
    let id = build_floor_off_main(&mut renderer);
    let before = gpu_creation_counts();
    publish_floor(&mut renderer, id);
    let glossy = renderer.level().glossy.as_ref().expect("glossy state");
    assert!(!glossy.ready(), "built at off: no D_static");
    renderer.set_reflection_quality(Low);
    assert!(renderer.level().glossy.as_ref().is_some_and(|g| g.ready()));
    assert_eq!(gpu_creation_counts().since(before).pipelines, 0);
    let overlays_before = overlays(&renderer);
    assert_draws_the_probe_term(&mut renderer, "the frame after the switch");
    assert_eq!(overlays(&renderer), overlays_before + 2);

    // Built at off, raised to low while the build was in flight.
    renderer.set_reflection_quality(Off);
    let id = build_floor_off_main(&mut renderer);
    renderer.set_reflection_quality(Low);
    publish_floor(&mut renderer, id);
    assert!(
        renderer.level().glossy.as_ref().is_some_and(|g| g.ready()),
        "the publish allocates D_static at the kept tier"
    );
    assert_draws_the_probe_term(&mut renderer, "the first frame after the publish");
}

// Acceptance row 50, pins P17 and P20. Over a glossy level with a reflecting
// pool, the overlay records once on a frame whose camera ranges reach a
// glossy bucket, and the sums dispatch once on any frame with a probe-term
// draw, overlay or fluid, never twice. A frame whose ranges reach no glossy
// bucket records no overlay, and with no reflecting fluid draw dispatches no
// sums; at off, or under a mask with an all-terms bit off, neither records.
#[test]
fn the_overlay_and_sums_gates_follow_ranges_tier_mask_and_fluid_draws() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let probes = binding_the_pool(floor_probe(true));
    let index = matte_only_cell_index();
    let water = pool([-3.0, 3.0], [10.0, 15.0], 0.3);
    install_floor_with(&mut renderer, &probes, Some(&index), &[water]);
    let view = floor_view();
    let matte_only = VisibleCells::Culled(vec![0]);
    let all = LightTermMask::ALL;
    let mut cases = vec![
        (
            "glossy ranges and reflecting water",
            High,
            all,
            true,
            &ALL,
            (1, 1),
        ),
        ("glossy ranges alone", High, all, false, &ALL, (1, 1)),
        ("glossy ranges alone at low", Low, all, false, &ALL, (1, 1)),
        (
            "no glossy range, no water",
            High,
            all,
            false,
            &matte_only,
            (0, 0),
        ),
        (
            "no glossy range, reflecting water",
            High,
            all,
            true,
            &matte_only,
            (0, 1),
        ),
        ("off, glossy ranges and water", Off, all, true, &ALL, (0, 0)),
    ];
    // Each all-terms bit off in turn, over glossy ranges and water.
    for term in LightTermMask::ALL_TERMS {
        let mut mask = all;
        mask.set_enabled(term, false);
        cases.push(("an all-terms bit off", High, mask, true, &ALL, (0, 0)));
    }
    for (what, tier, mask, water_draws, cells, expected) in cases {
        renderer.set_reflection_quality(tier);
        renderer.set_light_term_mask(mask);
        let draws = if water_draws {
            vec![look_draw(0, mirror())]
        } else {
            Vec::new()
        };
        renderer.set_fluid_surface_draws(&draws);
        let (overlays_before, sums_before) = (overlays(&renderer), dispatches(&renderer));
        frame_seeing(&mut renderer, view, cells);
        assert_eq!(
            (
                overlays(&renderer) - overlays_before,
                dispatches(&renderer) - sums_before
            ),
            expected,
            "{what} ({mask:?}): overlay passes and sums dispatches"
        );
    }
}

// Acceptance row 60, pin P22 (resize half). After the scene extent changes,
// D_static follows it, and the first frame back at the proof extent meets
// the composite row on every glossy pixel, with no validation error.
#[test]
fn the_first_frame_after_a_resize_meets_the_composite_row() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_floor(&mut renderer, &floor_probe(true));
    let view = floor_view();
    frame_seeing(&mut renderer, view, &ALL);
    let extent = |renderer: &Renderer| {
        let texture = renderer
            .level()
            .glossy
            .as_ref()
            .and_then(|glossy| glossy.static_diffuse_texture())
            .expect("D_static");
        (texture.width(), texture.height())
    };
    renderer.record_surface_size(48, 40);
    super::fluid_reflection_sums_frame_test_fixtures::frame_at(&mut renderer, view, 0.0);
    assert_eq!(extent(&renderer), (48, 40), "D_static follows the scene");
    let size = super::fluid_reflection_frame_test_fixtures::FRAME;
    renderer.record_surface_size(size, size);
    assert_draws_the_probe_term(&mut renderer, "the first frame after a resize");
    assert_eq!(extent(&renderer), (size, size));
}
