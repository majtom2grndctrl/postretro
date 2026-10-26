// Planar mirror preparation: O1 order, the main view's sets, the mirror's unions and tiers.
// See: context/lib/testing_guide.md

use super::test_fixtures::*;
use super::*;
use glam::Vec3;
use postretro_render_cpu::fluid_surface::fog::FLUID_FOG_BAND;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_test_log_capture::LogCapture;

const PLANAR: FluidReflectionQuality = FluidReflectionQuality::VeryHigh;

fn mirror_only() -> Vec<u32> {
    let mut cells = MIRROR_ROOMS.to_vec();
    cells.push(EMPTY_ATTIC);
    cells
}

// AC 16 (O1): the sets the main draw cull and fog pass receive are identical
// with planar on and off at the same pose, and so is the main view's light
// reach; the cells only the mirror reaches come as their own tagged set
// (O15: frame N's reaches, before ranking).
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
    assert_eq!(
        on.light_reachable_cell_mask, off.light_reachable_cell_mask,
        "the main view's light reach is the planar-off one"
    );
    assert_eq!(on.reachable_cell_aabbs, off.reachable_cell_aabbs);
    assert!(off.mirror_light_cell_aabbs.is_empty());
    assert!(
        !on.mirror_light_cell_aabbs.is_empty(),
        "the mirror's light reach is tagged apart"
    );
}

// Rewrite of the spike's union test: a completed mirror walk with the main
// view on a walk path unions the mirror's reach (an empty cell included, for
// entities) into collection, residency and SH sets, and hands its lighting
// reach over tagged mirror-only (Task 8), while the main sets, light reach
// included, stay the main view's.
#[test]
fn planar_walk_path_unions_mirror_reach_into_collection_residency_and_sh_and_tags_its_lighting() {
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
    assert_eq!(lit, &[WATER_ROOM], "the main view's light reach");
    assert_eq!(main.reachable_cell_aabbs.len(), 1);
    let mirror_only_bounds: Vec<_> = mirror_only()
        .iter()
        .map(|&cell| {
            let cell = &world.cells[cell as usize];
            (cell.bounds_min, cell.bounds_max)
        })
        .collect();
    // Every cell only the mirror reaches, the empty attic included, and no
    // main-view cell.
    assert_eq!(main.mirror_light_cell_aabbs.len(), mirror_only_bounds.len());
    for bounds in &mirror_only_bounds {
        assert!(main.mirror_light_cell_aabbs.contains(bounds), "{bounds:?}");
    }
    // A second frame replaces the set rather than growing it.
    main.include_mirror_lighting(&world, &mirror);
    assert_eq!(main.mirror_light_cell_aabbs.len(), mirror_only_bounds.len());
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
    assert!(
        main.mirror_light_cell_aabbs.is_empty(),
        "every cell-assigned light is main-view on the sentinel"
    );
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
        assert!(main.mirror_light_cell_aabbs.is_empty(), "{quality:?}");
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

// Task 4 (AC 17's app side, AC 36's seam, O5): at the corner pool the
// cropped mirror reaches a strict subset of the uncropped walk at the same
// pose, its cull is cropped while raster, clip plane and eye stay, a forced
// trip draws only cells inside the cropped frustum, and turning the crop off
// restores the uncropped walk. The pool's plane clears the area floor through
// a coplanar pool behind the camera, which the crop leaves out.
#[test]
fn planar_crop_reaches_a_subset_of_the_uncropped_walk_and_crop_off_restores_it() {
    let world = world_with_waters(WATER_ROOM, &[(0.0, &[CORNER_POOL, BEHIND_POOL])]);
    let main = main_view(&world);
    let mut mirror = installed(&world);
    prepare(&mut mirror, &world, &main, PLANAR);
    let cropped = mirror.camera().expect("the pool is on screen");
    let cropped_reach = mirror.reach().to_vec();
    assert_eq!(mirror.path(), Some(MirrorVisibilityPath::Walked));

    mirror.set_crop(false);
    mirror.install(&world, &[0]);
    prepare(&mut mirror, &world, &main, PLANAR);
    let whole = mirror.camera().unwrap();
    let main_camera = camera();
    let uncropped = MirrorCamera::new(
        main_camera.eye_position,
        main_camera.view_matrix,
        main_camera.view_projection * main_camera.view_matrix.inverse(),
        0.0,
        None,
    )
    .unwrap();
    assert_eq!(
        whole, uncropped,
        "crop off is the uncropped mirror, kept across install"
    );
    assert_eq!(mirror.reach(), &[WATER_ROOM, 5, 6, EMPTY_ATTIC]);
    assert_eq!(
        (cropped.raster_vp, cropped.clip_plane, cropped.eye),
        (whole.raster_vp, whole.clip_plane, whole.eye),
        "the crop moves no pixel"
    );
    assert_ne!(cropped.cull_vp, whole.cull_vp);
    assert!(
        cropped_reach
            .iter()
            .all(|cell| mirror.reach().contains(cell)),
        "cropped {cropped_reach:?} within uncropped {:?}",
        mirror.reach()
    );
    // The pool's crop keeps the screen's right half: a box left of the eye,
    // in the uncropped mirror view, falls outside it.
    let (lo, hi) = (Vec3::new(-2.5, 0.2, -1.5), Vec3::new(-1.5, 0.3, -1.4));
    let in_view = postretro_render_cpu::planar_reflection::aabb_in_planes;
    assert!(in_view(lo, hi, &whole.cull_planes()));
    assert!(!in_view(lo, hi, &cropped.cull_planes()));

    mirror.set_crop(true);
    mirror.set_step_cap(Some(0));
    prepare(&mut mirror, &world, &main, PLANAR);
    assert!(matches!(
        mirror.path(),
        Some(MirrorVisibilityPath::Tripped { .. })
    ));
    let planes = mirror.camera().unwrap().cull_planes();
    assert!(!cells(mirror.visible()).is_empty());
    for &cell in cells(mirror.visible()) {
        let data = &world.cells[cell as usize];
        assert!(
            postretro_render_cpu::planar_reflection::aabb_in_planes(
                data.bounds_min,
                data.bounds_max,
                &planes
            ),
            "O5: the trip set keeps to the cropped frustum"
        );
    }
}

/// A lake past the water room's far wall (z < −4): in the main frustum, but
/// no cell of the camera's reach straddles it.
const LAKE_BEHIND_WALL: WaterRect = ([-30.0, -40.0], [30.0, -6.0]);
/// A 16 m² pool in the water room's near-left quarter: just over the area
/// floor, and outscored by the lake.
const ROOM_POOL: WaterRect = ([0.0, -4.0], [4.0, 0.0]);
/// The water room's whole floor.
const ROOM_WATER: WaterRect = ([-4.0, -4.0], [4.0, 4.0]);

// AC 26 (O12) and Task 3's open item: the lake behind the wall is in the
// frustum and outscores the room's pool, but has no seed cell in the camera's
// reach, so the pool is selected and walks its mirror. With the pool gone no
// plane is selected and no mirror camera reaches the renderer, so the water
// reads SSR; unfiltered, the lake would have been selected and walked from no
// seed (`NoSeeds`), handing the renderer an empty planar layer.
#[test]
fn planar_selection_skips_a_plane_with_no_seed_in_the_cameras_reach() {
    let world = world_with_waters(
        WATER_ROOM,
        &[(-0.5, &[LAKE_BEHIND_WALL]), (0.0, &[ROOM_POOL])],
    );
    let main = main_view(&world);
    let mut mirror = installed(&world);
    assert_eq!(mirror.candidate_planes(), 2);
    let eye = camera().eye_position;
    let vp = camera().view_projection;
    let unfiltered = select_plane(&mirror.candidates, eye, vp, &[true, true], None).unwrap();
    assert_eq!(unfiltered.h, -0.5, "the lake outscores the pool");

    prepare(&mut mirror, &world, &main, PLANAR);
    assert_eq!(mirror.held_selection().map(|plane| plane.h), Some(0.0));
    assert!(mirror.camera().is_some());
    assert_eq!(mirror.path(), Some(MirrorVisibilityPath::Walked));
    assert!(!mirror.seed_cells.is_empty());

    let world = world_with_waters(WATER_ROOM, &[(-0.5, &[LAKE_BEHIND_WALL])]);
    let main = main_view(&world);
    let mut mirror = installed(&world);
    assert_eq!(mirror.candidate_planes(), 1);
    for _ in 0..2 {
        prepare(&mut mirror, &world, &main, PLANAR);
        assert!(mirror.held_selection().is_none(), "no seeded plane");
        assert!(mirror.camera().is_none(), "no mirror on NoSeeds");
        assert_eq!(mirror.path(), None);
        assert!(cells(mirror.visible()).is_empty());
        assert!(std::ptr::eq(
            mirror.drawn_cells(&main.visible_cells),
            &main.visible_cells
        ));
        assert!(mirror.reach().is_empty());
    }
}

// AC 26 (O14): on a main-view fallback the camera's reach is its drawn set.
// The solid-cell fallback has no fog reach, and a level with no portals walks
// nothing, yet the water room each draws seeds the plane, so it stays
// selectable and its mirror draws.
#[test]
fn planar_selection_reads_the_drawn_set_as_reach_on_a_main_fallback() {
    let mut no_portals = world(WATER_ROOM);
    no_portals.has_portals = false;
    for (world, main_path, mirror_path) in [
        (
            world(SOLID_CAMERA_CELL),
            VisibilityPath::SolidCellFallback,
            MirrorVisibilityPath::Walked,
        ),
        (
            no_portals,
            VisibilityPath::NoPortalsFallback,
            MirrorVisibilityPath::NoPortals,
        ),
    ] {
        let main = main_view(&world);
        assert_eq!(
            std::mem::discriminant(&main.stats.path),
            std::mem::discriminant(&main_path)
        );
        assert!(cells(&main.visible_cells).contains(&WATER_ROOM));
        let mut mirror = installed(&world);
        prepare(&mut mirror, &world, &main, PLANAR);
        assert_eq!(
            mirror.held_selection().map(|plane| plane.h),
            Some(0.0),
            "{main_path:?}"
        );
        assert!(mirror.camera().is_some(), "{main_path:?}");
        assert_eq!(mirror.path(), Some(mirror_path));
    }
}

// AC 26 (probe gate): planar keeps the reflecting gate's probe requirement.
// Water whose faces bind no probe, or a level with no probe section, installs
// no candidate, so it is never selected and draws no mirror.
#[test]
fn planar_selection_never_takes_a_plane_with_no_bound_probe() {
    let mut unbound = world(WATER_ROOM);
    unbound
        .reflection_probes
        .as_mut()
        .expect("the fixture binds its water")
        .bindings
        .clear();
    let mut no_section = world(WATER_ROOM);
    no_section.reflection_probes = None;
    for world in [unbound, no_section] {
        let main = main_view(&world);
        let mut mirror = installed(&world);
        assert_eq!(mirror.candidate_planes(), 0);
        prepare(&mut mirror, &world, &main, PLANAR);
        assert!(mirror.held_selection().is_none());
        assert!(mirror.camera().is_none());
    }
    let world = world(WATER_ROOM);
    assert_eq!(
        installed(&world).candidate_planes(),
        1,
        "bound, it competes"
    );
}

// AC 27 at the app: with the eye inside the room water's fog band, or below
// it, that plane is not selected and no mirror camera reaches the renderer,
// so its water reads SSR at the tier. A plane well below the eye still is;
// with none qualifying, none is.
#[test]
fn planar_preparation_selects_no_plane_the_eye_is_below_or_in_the_band_of() {
    let low = (-0.9, &[ROOM_WATER][..]);
    let room = (0.0, &[ROOM_WATER][..]);
    for eye_y in [FLUID_FOG_BAND * 0.5, FLUID_FOG_BAND, -0.5] {
        let camera = camera_at(Vec3::new(0.0, eye_y, 3.0));
        let only_room = world_with_waters(WATER_ROOM, &[room]);
        let main = main_view_from(&only_room, &camera);
        let mut mirror = installed(&only_room);
        prepare_from(&mut mirror, &only_room, &camera, &main, PLANAR);
        assert!(mirror.held_selection().is_none(), "eye at {eye_y}");
        assert!(mirror.camera().is_none(), "eye at {eye_y}: SSR at the tier");

        let both = world_with_waters(WATER_ROOM, &[low, room]);
        let main = main_view_from(&both, &camera);
        let mut mirror = installed(&both);
        prepare_from(&mut mirror, &both, &camera, &main, PLANAR);
        assert_eq!(
            mirror.held_selection().map(|plane| plane.h),
            Some(-0.9),
            "eye at {eye_y}: the low plane qualifies"
        );
        assert_eq!(mirror.camera().map(|mirror| mirror.h), Some(-0.9));
    }
}

// AC 30 (O11): after a level change the first frame selects without
// hysteresis, from the new level's candidates. Level A holds its index 1;
// level B's index 0 scores just over its index 1, under the hysteresis
// margin, so A's history kept would hold index 1 (the control), while the
// install reset selects index 0.
#[test]
fn planar_install_resets_selection_history() {
    let level_a = world_with_waters(WATER_ROOM, &[(0.0, &[ROOM_POOL]), (0.05, &[ROOM_WATER])]);
    let level_b = world_with_waters(WATER_ROOM, &[(0.0, &[ROOM_WATER]), (0.05, &[ROOM_WATER])]);
    let mut mirror = installed(&level_a);
    prepare(&mut mirror, &level_a, &main_view(&level_a), PLANAR);
    let held = mirror.held_selection().expect("level A holds a plane");
    assert_eq!(held.candidate_index, 1);

    let main_b = main_view(&level_b);
    let mut kept = installed(&level_b);
    kept.selected = Some(held);
    prepare(&mut kept, &level_b, &main_b, PLANAR);
    assert_eq!(
        kept.held_selection().map(|plane| plane.candidate_index),
        Some(1),
        "control: A's history would hold index 1 on level B"
    );

    mirror.install(&level_b, &[0, 1]);
    assert!(mirror.held_selection().is_none(), "install resets history");
    prepare(&mut mirror, &level_b, &main_b, PLANAR);
    assert_eq!(
        mirror.held_selection().map(|plane| plane.candidate_index),
        Some(0),
        "the first frame selects by score alone"
    );
    assert_eq!(mirror.camera().map(|mirror| mirror.h), Some(0.0));
}
