// Planar mirror preparation: O1 order, the main view's sets, the mirror's unions and tiers.
// See: context/lib/testing_guide.md

use super::test_fixtures::*;
use super::*;
use glam::Vec3;
use postretro_render_cpu::fluid_surface::fog::FLUID_FOG_BAND;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_test_log_capture::LogCapture;

/// Every cell only the mirror reaches, the empty attic included.
fn mirror_only_reach() -> Vec<u32> {
    let mut cells = MIRROR_ROOMS.to_vec();
    cells.push(EMPTY_ATTIC);
    cells
}

/// The cells the frame draws with the fixture's mirror: the water room and
/// the mirror's two rooms, never the empty attic.
fn drawn_with_mirror() -> Vec<u32> {
    [&[WATER_ROOM][..], &MIRROR_ROOMS].concat()
}

// AC 16 (O1): the sets the main draw cull and fog pass receive are identical
// with planar on and off at the same pose, and so is the main view's light
// reach; the cells only the mirror reaches come as their own tagged set
// (O15: frame N's reaches, before ranking).
#[test]
fn planar_preparation_leaves_main_visibility_unchanged() {
    let world = world(WATER_ROOM);
    let off = main_view(&world);
    let on = main_view(&world);
    let mut mirror = installed(&world);
    prepare(&mut mirror, &world, &on, PLANAR);

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
    assert!(
        !mirror.mirror_light_cell_aabbs().is_empty(),
        "the mirror's light reach is tagged apart"
    );
}

// A completed mirror walk with the main view on a walk path unions the cells
// the mirror draws into collection, residency and SH sets, as the main view
// contributes only its drawable cells; its whole reach, the empty attic
// included, joins the SH fog cells and its lighting reach, tagged
// mirror-only. The main sets, light reach included, stay the main view's.
#[test]
fn planar_walk_path_unions_mirror_cells_into_collection_residency_and_sh_and_tags_its_lighting() {
    let world = world(WATER_ROOM);
    let main = main_view(&world);
    let mut mirror = installed(&world);
    prepare(&mut mirror, &world, &main, PLANAR);

    assert!(mirror.camera().is_some());
    assert_eq!(
        cells(mirror.visible()),
        &[0, 5, 6],
        "the attic draws nothing"
    );
    let drawn = drawn_with_mirror();
    assert_eq!(cells(mirror.drawn_cells(&main.visible_cells)), drawn);
    assert_eq!(cells(mirror.sh_visible_cells(&main.visible_cells)), drawn);
    assert_eq!(mirror.mirror_only_cells(&main.visible_cells), 2);
    let reach = [&[WATER_ROOM][..], &mirror_only_reach()].concat();
    assert_eq!(mirror.sample_fog_cells(&main.fog_reachable), reach);

    let lit: Vec<u32> = (0..world.cell_count() as u32)
        .filter(|&cell| main.light_reachable_cell_mask[cell as usize])
        .collect();
    assert_eq!(lit, &[WATER_ROOM], "the main view's light reach");
    assert_eq!(main.reachable_cell_aabbs.len(), 1);
    let mirror_only_bounds: Vec<_> = mirror_only_reach()
        .iter()
        .map(|&cell| {
            let cell = &world.cells[cell as usize];
            (cell.bounds_min, cell.bounds_max)
        })
        .collect();
    // Every cell only the mirror reaches, the empty attic included, and no
    // main-view cell.
    let tagged = mirror.mirror_light_cell_aabbs();
    assert_eq!(tagged.len(), mirror_only_bounds.len());
    for bounds in &mirror_only_bounds {
        assert!(tagged.contains(bounds), "{bounds:?}");
    }
    // A second frame replaces the set rather than growing it.
    prepare(&mut mirror, &world, &main, PLANAR);
    assert_eq!(
        mirror.mirror_light_cell_aabbs().len(),
        mirror_only_bounds.len()
    );
    assert_eq!(cells(&main.visible_cells), &[WATER_ROOM]);
    assert_eq!(main.fog_reachable, vec![WATER_ROOM]);
}

// From the real solid-cell fallback: the main view's empty fog set means
// "no portal isolation", so neither SH fog regions nor lighting gain a cell
// list, and the mirror's cells stay out of SH visible targeting (they add no
// SH reads).
#[test]
fn planar_preparation_preserves_unisolated_fog_sentinel_and_adds_no_sh_reads_on_main_fallback() {
    let world = world(SOLID_CAMERA_CELL);
    let main = main_view(&world);
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
        drawn_with_mirror()
    );
    assert!(main.light_reachable_cell_mask.is_empty());
    assert!(main.reachable_cell_aabbs.is_empty());
    assert!(
        mirror.mirror_light_cell_aabbs().is_empty(),
        "every cell-assigned light is main-view on the sentinel"
    );
}

// The tier is the switch: below very high the mirror's sets fall back to
// the main view's, the held plane drops, and a reinstall clears selection
// history.
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
        let main = main_view(&world);
        let mut mirror = installed(&world);
        prepare(&mut mirror, &world, &main, quality);

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
        assert!(mirror.mirror_light_cell_aabbs().is_empty(), "{quality:?}");
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
    assert!(
        is_height(Some(unfiltered.h), -0.5),
        "the lake outscores the pool"
    );

    prepare(&mut mirror, &world, &main, PLANAR);
    assert!(is_height(mirror.held_selection().map(|plane| plane.h), 0.0));
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
        assert!(
            is_height(mirror.held_selection().map(|plane| plane.h), 0.0),
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
        assert!(
            is_height(mirror.held_selection().map(|plane| plane.h), -0.9),
            "eye at {eye_y}: the low plane qualifies"
        );
        assert!(is_height(mirror.camera().map(|mirror| mirror.h), -0.9));
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
    assert!(is_height(mirror.camera().map(|mirror| mirror.h), 0.0));
}

/// Plane A's face near the camera in [`yield_pose`]: off screen below and
/// left of the view, yet its bounds reach inside every main frustum plane on
/// their own, so a per-face box test can score it while no part of it
/// projects on screen.
const OFF_SCREEN_FACE: WaterRect = ([-2.0, 1.0], [2.0, 3.0]);
/// Plane B, under A: on screen, scoring under A's box-test score.
const ON_SCREEN_POOL: WaterRect = ([-2.5, -3.75], [2.5, 1.0]);
const PLANE_A: f32 = 0.0;
const PLANE_B: f32 = -0.5;

/// A yawed view from inside the water room, looking past plane A's face.
fn yield_pose() -> crate::camera::RenderCamera {
    camera_aimed(Vec3::new(1.0, 1.0, 1.0), Vec3::new(-3.0, 0.0, -6.0))
}

/// `world`'s plane A held from an earlier frame.
fn hold_plane_a(mirror: &mut PlanarRenderPreparation, index: usize) {
    let plane = &mirror.candidates.planes[index];
    assert!(is_height(Some(plane.h), PLANE_A));
    mirror.selected = Some(SelectedPlane {
        candidate_index: index,
        h: plane.h,
        bounds_min: plane.bounds_min,
        bounds_max: plane.bounds_max,
        score: 1.0,
    });
}

// O13 at the app: a held plane whose face lies just outside the frustum
// crops to nothing, so it yields that frame, and the on-screen challenger
// scoring under 1.25× its box-test score is selected and draws this frame's
// mirror, rather than the held plane keeping selection while its water reads
// SSR. With no challenger, no plane is selected and no mirror camera
// reaches the renderer. A's area clears the floor through a coplanar pool
// behind the camera, which neither scores nor crops.
#[test]
fn planar_held_plane_off_screen_yields_to_an_on_screen_challenger_that_frame() {
    let pose = yield_pose();
    let world = world_with_waters(
        WATER_ROOM,
        &[
            (PLANE_A, &[OFF_SCREEN_FACE, BEHIND_POOL]),
            (PLANE_B, &[ON_SCREEN_POOL]),
        ],
    );
    let main = main_view_from(&world, &pose);
    let mut mirror = installed(&world);
    assert_eq!(mirror.candidate_planes(), 2);
    // Candidates ascend by height: B is index 0, A index 1.
    let plane_a = mirror.candidates.planes[1].clone();
    let a_mirror = MirrorCamera::new(
        pose.eye_position,
        pose.view_matrix,
        pose.view_projection * pose.view_matrix.inverse(),
        plane_a.h,
        None,
    )
    .expect("the eye is above A");
    let mut scratch = MirrorCropScratch::default();
    assert!(
        a_mirror
            .cropped_to_plane(&plane_a, pose.view_projection, &mut scratch)
            .is_none(),
        "control: no part of A is on screen"
    );
    hold_plane_a(&mut mirror, 1);

    prepare_from(&mut mirror, &world, &pose, &main, PLANAR);
    assert_eq!(
        mirror.held_selection().map(|plane| plane.candidate_index),
        Some(0),
        "the challenger is selected the frame A yields"
    );
    assert!(is_height(mirror.camera().map(|mirror| mirror.h), PLANE_B));
    assert_eq!(mirror.path(), Some(MirrorVisibilityPath::Walked));

    let only_a = world_with_waters(WATER_ROOM, &[(PLANE_A, &[OFF_SCREEN_FACE, BEHIND_POOL])]);
    let main = main_view_from(&only_a, &pose);
    let mut mirror = installed(&only_a);
    hold_plane_a(&mut mirror, 0);
    prepare_from(&mut mirror, &only_a, &pose, &main, PLANAR);
    assert!(mirror.held_selection().is_none(), "nothing qualifies");
    assert!(mirror.camera().is_none(), "no mirror camera, no demand");
    assert!(std::ptr::eq(
        mirror.drawn_cells(&main.visible_cells),
        &main.visible_cells
    ));
    assert!(mirror.mirror_light_cell_aabbs().is_empty());
}

/// Held plane B in the three-plane yield fixture: [`ON_SCREEN_POOL`] raised
/// over A, scoring low enough that A's box-test score clears 1.25× B's.
const HELD_B: f32 = 0.2;
/// Challenger C: [`ON_SCREEN_POOL`] again, between A and B, scoring above B
/// but within 1.25× B's score, and under A's.
const CHALLENGER_C: f32 = 0.1;

// O13 with hysteresis kept: plane A outscores held B past the margin, then
// yields every frame, as in the two-plane case. The retry still holds B, so
// challenger C, scoring within 1.25× B, does not take selection from it:
// without B held, the retry picks C by raw score, and B and C would pop
// whenever A's yield left them to the raw scores.
#[test]
fn planar_retry_after_a_yield_keeps_hysteresis_between_the_remaining_planes() {
    let pose = yield_pose();
    let world = world_with_waters(
        WATER_ROOM,
        &[
            (PLANE_A, &[OFF_SCREEN_FACE, BEHIND_POOL]),
            (HELD_B, &[ON_SCREEN_POOL]),
            (CHALLENGER_C, &[ON_SCREEN_POOL]),
        ],
    );
    let main = main_view_from(&world, &pose);

    // Candidates ascend by height: A is index 0, C index 1, B index 2.
    let mut unheld = installed(&world);
    assert_eq!(unheld.candidate_planes(), 3);
    prepare_from(&mut unheld, &world, &pose, &main, PLANAR);
    assert_eq!(
        unheld.held_selection().map(|plane| plane.candidate_index),
        Some(1),
        "control: with nothing held, A yields and the retry picks C by score"
    );

    let mut mirror = installed(&world);
    let plane_b = &mirror.candidates.planes[2];
    assert!(is_height(Some(plane_b.h), HELD_B));
    mirror.selected = Some(SelectedPlane {
        candidate_index: 2,
        h: plane_b.h,
        bounds_min: plane_b.bounds_min,
        bounds_max: plane_b.bounds_max,
        score: 1.0,
    });
    for frame in 0..2 {
        prepare_from(&mut mirror, &world, &pose, &main, PLANAR);
        assert_eq!(
            mirror.held_selection().map(|plane| plane.candidate_index),
            Some(2),
            "frame {frame}: B stays selected while A yields"
        );
        assert!(is_height(mirror.camera().map(|mirror| mirror.h), HELD_B));
    }
}

// Capture and the walk measurement load no fluid manifest: every fluid
// volume with a probe-bound face reflects; a volume with no bound face, or
// no fluid, does not.
#[test]
fn planar_manifest_less_reflecting_volumes_are_the_probe_bound_fluid_ones() {
    let mut world = world_with_waters(WATER_ROOM, &[(0.0, &[ROOM_WATER]), (-0.5, &[ROOM_POOL])]);
    assert_eq!(
        crate::render_preparation::probe_bound_fluid_volumes(&world),
        vec![0, 1]
    );
    world
        .reflection_probes
        .as_mut()
        .expect("the fixture binds its water")
        .bindings
        .retain(|binding| binding.volume != 0);
    assert_eq!(probe_bound_fluid_volumes(&world), vec![1]);
    world.environment_volumes.volumes[1].fluid = String::new();
    assert!(probe_bound_fluid_volumes(&world).is_empty());
}
