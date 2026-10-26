// Planar mirror preparation: O1 order, the main view's sets, the mirror's unions and tiers.
// See: context/lib/testing_guide.md

use super::test_fixtures::*;
use super::*;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_test_log_capture::LogCapture;

const PLANAR: FluidReflectionQuality = FluidReflectionQuality::VeryHigh;

fn mirror_only() -> Vec<u32> {
    let mut cells = MIRROR_ROOMS.to_vec();
    cells.push(EMPTY_ATTIC);
    cells
}

// AC 16 (O1): the sets the main draw cull and fog pass receive are identical
// with planar on and off at the same pose, while the mirror reaches cells
// the main view does not and widens only the lighting inputs.
#[test]
fn planar_preparation_leaves_main_visibility_unchanged() {
    let world = world(WATER_ROOM);
    let off = main_view(&world);
    let mut on = main_view(&world);
    let mut mirror = installed(&world);
    prepare(&mut mirror, &world, &on, PLANAR);
    on.include_mirror_lighting(&world, &mirror);

    assert_eq!(
        mirror.path(),
        Some(MirrorVisibilityPath::Walked),
        "the mirror walks this frame"
    );
    assert!(matches!(on.stats.path, VisibilityPath::PrlPortal { .. }));
    assert_eq!(cells(&on.visible_cells), cells(&off.visible_cells));
    assert_eq!(cells(&on.visible_cells), &[WATER_ROOM]);
    assert_eq!(on.fog_reachable, off.fog_reachable);
    assert_eq!(cells(on.camera_cull().cells), cells(&off.visible_cells));
    assert_ne!(
        on.light_reachable_cell_mask, off.light_reachable_cell_mask,
        "only lighting widens"
    );
}

// Rewrite of the spike's union test: a completed mirror walk with the main
// view on a walk path unions the mirror's reach (an empty cell included, for
// entities) into collection, residency and SH sets, and into lighting
// untagged, while the main sets stay the main view's.
#[test]
fn planar_walk_path_unions_mirror_reach_with_empty_cells_into_collection_residency_sh_and_lighting()
{
    let world = world(WATER_ROOM);
    let mut main = main_view(&world);
    let mut mirror = installed(&world);
    prepare(&mut mirror, &world, &main, PLANAR);

    assert!(mirror.camera().is_some());
    assert_eq!(
        cells(mirror.visible()),
        &[0, 5, 6],
        "the attic draws nothing"
    );
    let drawn = [&[WATER_ROOM][..], &mirror_only()].concat();
    assert_eq!(cells(mirror.drawn_cells(&main.visible_cells)), drawn);
    assert_eq!(cells(mirror.sh_visible_cells(&main.visible_cells)), drawn);
    assert_eq!(mirror.sample_fog_cells(&main.fog_reachable), drawn);

    main.include_mirror_lighting(&world, &mirror);
    let lit: Vec<u32> = (0..world.cell_count() as u32)
        .filter(|&cell| main.light_reachable_cell_mask[cell as usize])
        .collect();
    assert_eq!(lit, drawn);
    assert_eq!(main.reachable_cell_aabbs.len(), drawn.len());
    assert_eq!(cells(&main.visible_cells), &[WATER_ROOM]);
    assert_eq!(main.fog_reachable, vec![WATER_ROOM]);
}

// Rewrite of the spike's sentinel test, now from the real solid-cell
// fallback: the main view's empty fog set means "no portal isolation", so
// neither SH fog regions nor lighting gain a cell list, and the mirror's
// cells stay out of SH visible targeting (they add no SH reads).
#[test]
fn planar_preparation_preserves_unisolated_fog_sentinel_and_adds_no_sh_reads_on_main_fallback() {
    let world = world(SOLID_CAMERA_CELL);
    let mut main = main_view(&world);
    assert!(matches!(main.stats.path, VisibilityPath::SolidCellFallback));
    assert!(main.fog_reachable.is_empty());
    let mut mirror = installed(&world);
    prepare(&mut mirror, &world, &main, PLANAR);

    assert_eq!(
        mirror.path(),
        Some(MirrorVisibilityPath::Walked),
        "O14: the drawn set seeds the mirror on a fallback"
    );
    assert!(mirror.sample_fog_cells(&main.fog_reachable).is_empty());
    assert!(std::ptr::eq(
        mirror.sh_visible_cells(&main.visible_cells),
        &main.visible_cells
    ));
    // Drawn cells still reach collection and the lightmap, whose path rules
    // hold them without reads on this path.
    assert_eq!(
        cells(mirror.drawn_cells(&main.visible_cells)),
        [&[WATER_ROOM][..], &mirror_only()].concat()
    );
    main.include_mirror_lighting(&world, &mirror);
    assert!(main.light_reachable_cell_mask.is_empty());
    assert!(main.reachable_cell_aabbs.is_empty());
}

// Rewrite of the spike's off-switch test, with the tier as the switch: below
// very high the mirror's sets fall back to the main view's, the held plane
// drops, and a reinstall clears selection history.
#[test]
fn planar_preparation_below_very_high_restores_main_inputs_and_reinstall_drops_selection() {
    let world = world(WATER_ROOM);
    let main = main_view(&world);
    let mut mirror = installed(&world);
    prepare(&mut mirror, &world, &main, PLANAR);
    assert!(mirror.held_selection().is_some());

    prepare(&mut mirror, &world, &main, FluidReflectionQuality::High);
    assert!(mirror.camera().is_none());
    assert!(mirror.plane().is_none());
    assert!(mirror.held_selection().is_none());
    assert!(std::ptr::eq(
        mirror.drawn_cells(&main.visible_cells),
        &main.visible_cells
    ));
    assert_eq!(mirror.sample_fog_cells(&main.fog_reachable), &[WATER_ROOM]);

    prepare(&mut mirror, &world, &main, PLANAR);
    assert!(mirror.held_selection().is_some());
    mirror.install(&world, &[0]);
    assert!(
        mirror.held_selection().is_none(),
        "O11: install resets selection history"
    );
    mirror.install(&world, &[]);
    prepare(&mut mirror, &world, &main, PLANAR);
    assert!(mirror.camera().is_none(), "no candidate, no planar");
    assert!(cells(mirror.visible()).is_empty());
}

// AC 31 (selection half, O8): at high and below no plane is selected, no
// mirror walk runs, and the mirror adds nothing to demand or light ranking.
#[test]
fn planar_preparation_at_high_selects_nothing_walks_nothing_and_adds_no_demand() {
    let world = world(WATER_ROOM);
    for quality in [
        FluidReflectionQuality::Off,
        FluidReflectionQuality::Low,
        FluidReflectionQuality::Medium,
        FluidReflectionQuality::High,
    ] {
        let off = main_view(&world);
        let mut main = main_view(&world);
        let mut mirror = installed(&world);
        prepare(&mut mirror, &world, &main, quality);
        main.include_mirror_lighting(&world, &mirror);

        assert!(mirror.held_selection().is_none(), "{quality:?}");
        assert!(mirror.camera().is_none());
        assert_eq!(mirror.path(), None);
        assert!(
            main.stats.cpu.value(VisibilityStage::MirrorWalk).is_none(),
            "no mirror walk at {quality:?}"
        );
        assert!(std::ptr::eq(
            mirror.drawn_cells(&main.visible_cells),
            &main.visible_cells
        ));
        assert!(std::ptr::eq(
            mirror.sh_visible_cells(&main.visible_cells),
            &main.visible_cells
        ));
        assert_eq!(mirror.sample_fog_cells(&main.fog_reachable), &[WATER_ROOM]);
        assert_eq!(
            main.light_reachable_cell_mask,
            off.light_reachable_cell_mask
        );
        assert!(mirror.reach().is_empty());
    }
    let main = main_view(&world);
    let mut mirror = installed(&world);
    prepare(&mut mirror, &world, &main, FluidReflectionQuality::Ultra);
    assert!(
        main.stats.cpu.value(VisibilityStage::MirrorWalk).is_some(),
        "ultra walks the mirror under the frame's visibility stages"
    );
}

// AC 20 (O5): a mirror walk forced to trip draws only cells the camera's
// visible set holds inside the mirror frustum, so every set it hands out is
// the main view's, and the trip logs no solid-cell fallback.
#[test]
fn planar_forced_mirror_trip_draws_camera_visible_cells_adds_no_reads_and_logs_no_solid_fallback() {
    let capture = LogCapture::start();
    let world = world(WATER_ROOM);
    let main = main_view(&world);
    let mut mirror = installed(&world);
    mirror.set_step_cap(Some(0));
    prepare(&mut mirror, &world, &main, PLANAR);

    assert!(matches!(
        mirror.path(),
        Some(MirrorVisibilityPath::Tripped { .. })
    ));
    assert!(mirror.camera().is_some(), "the tripped mirror still draws");
    assert_eq!(cells(mirror.visible()), &[WATER_ROOM]);
    assert!(
        cells(mirror.visible())
            .iter()
            .all(|cell| cells(&main.visible_cells).contains(cell))
    );
    assert_eq!(
        cells(mirror.drawn_cells(&main.visible_cells)),
        cells(&main.visible_cells),
        "lightmap demand and collection gain nothing"
    );
    assert!(std::ptr::eq(
        mirror.sh_visible_cells(&main.visible_cells),
        &main.visible_cells
    ));
    capture.assert_not_logged(log::Level::Warn, "SolidCellFallback");
}

// AC 21 (sample-region and compose half): mirror-only cells on a walk path
// reach SH sample regions and the cells handed to the renderer, whose mirror
// cull and animated-lightmap compose union read them. No probe surface takes
// them: probes are bound per face at compile time, and nothing here names one.
#[test]
fn planar_mirror_cells_join_sample_regions_and_the_renderer_view_on_a_walk_path() {
    let world = world(WATER_ROOM);
    let main = main_view(&world);
    let mut mirror = installed(&world);
    prepare(&mut mirror, &world, &main, PLANAR);
    let sample_visible = cells(mirror.drawn_cells(&main.visible_cells));
    let sample_fog = mirror.sample_fog_cells(&main.fog_reachable);
    for room in MIRROR_ROOMS {
        assert!(!cells(&main.visible_cells).contains(&room));
        assert!(sample_visible.contains(&room));
        assert!(sample_fog.contains(&room));
        assert!(cells(mirror.visible()).contains(&room));
    }
}
