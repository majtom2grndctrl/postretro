// Mover light state on world-less frames: a held (Settling) frame hands the
// renderer the post-tick phase's state, so a restarted level's first
// presented frame carries its re-seeded t′; a frame with no level hands over
// none, so no stale state survives into a Loading or Frontend frame.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use glam::Vec3;
use postretro_level_loader::{KinematicGeometry, LoadedKinematicWaypoint};

use crate::app::world_less_frame::HeldLevelView;
use crate::kinematic_mover::{MoverTickStateTable, run_kinematic_mover_tick};
use crate::render::MoverLightState;
use crate::render_preparation::planar_test_fixtures::camera;
use crate::runtime_movers::tests::{mover, single_cell_world};
use crate::runtime_movers::{ENGINE_AUTO_CLOSE_MS, mover_entity, spawn_loaded_kinematic_movers};
use crate::startup::lifecycle::tests::test_app;

/// A level holding one 1 m door, mover 7, closed at its first waypoint and
/// opening at 1 m/s from spawn with no endpoint wait.
fn door_level() -> postretro_level_loader::LevelWorld {
    let mut door = mover(0);
    door.speed_mps = 1.0;
    door.wait_ms = 0.0;
    door.path = "a0".to_string();
    let waypoints = vec![
        LoadedKinematicWaypoint {
            name: "a0".to_string(),
            next: "a1".to_string(),
            origin: Vec3::ZERO,
        },
        LoadedKinematicWaypoint {
            name: "a1".to_string(),
            next: String::new(),
            origin: Vec3::Y,
        },
    ];
    single_cell_world(KinematicGeometry {
        movers: vec![door],
        waypoints,
    })
}

fn handed(app: &crate::App) -> Vec<MoverLightState> {
    app.kinematic_mover_render.light_states().to_vec()
}

/// P13 / D7, app half: the first held frame after a restart that re-seeds
/// the door hands over its seeded t′, never the pre-restart value. The
/// renderer half (install resets the state table; the first hand-over writes
/// live tiles at that t′) is `mover_depth_tiles_tests`'s relevel row.
#[test]
fn a_held_frame_after_a_restart_hands_over_the_reseeded_mover_state() {
    let mut app = test_app();
    let world = door_level();
    let registry = app
        .session
        .as_ref()
        .expect("a session")
        .scripting
        .script_ctx
        .registry
        .clone();
    spawn_loaded_kinematic_movers(&mut registry.borrow_mut(), &world, ENGINE_AUTO_CLOSE_MS)
        .expect("the door spawns");
    app.level = Some(world);
    let view = HeldLevelView { camera: camera() };

    app.held_level_visibility(&view, None)
        .expect("a held frame");
    assert_eq!(
        handed(&app),
        vec![MoverLightState {
            mover_id: 7,
            t: 0.0
        }]
    );

    let mut table = MoverTickStateTable::default();
    for _ in 0..2 {
        run_kinematic_mover_tick(&mut registry.borrow_mut(), &mut table, 0.25);
    }
    app.held_level_visibility(&view, None)
        .expect("a held frame");
    assert_eq!(
        handed(&app),
        vec![MoverLightState {
            mover_id: 7,
            t: 0.5
        }]
    );

    // Restart: the level's movers respawn at their seeded phase.
    {
        let mut registry = registry.borrow_mut();
        let door = mover_entity(&registry, 7).expect("the door");
        registry.despawn(door).expect("the door despawns");
        let world = app.level.as_ref().expect("the level");
        spawn_loaded_kinematic_movers(&mut registry, world, ENGINE_AUTO_CLOSE_MS)
            .expect("the door respawns");
    }
    app.held_level_visibility(&view, None)
        .expect("a held frame");
    assert_eq!(
        handed(&app),
        vec![MoverLightState {
            mover_id: 7,
            t: 0.0
        }],
        "the restart's first held frame hands over the seeded state"
    );
}

/// A world-less frame with no level (Loading, Frontend) hands over no state,
/// whatever the last world frame handed over.
#[test]
fn a_hand_over_with_no_level_leaves_no_state() {
    let mut app = test_app();
    let world = door_level();
    let registry = app
        .session
        .as_ref()
        .expect("a session")
        .scripting
        .script_ctx
        .registry
        .clone();
    spawn_loaded_kinematic_movers(&mut registry.borrow_mut(), &world, ENGINE_AUTO_CLOSE_MS)
        .expect("the door spawns");
    app.kinematic_mover_render
        .hand_over_light_states(&registry.borrow(), Some(&world), None);
    assert_eq!(handed(&app).len(), 1, "a world frame hands over the door");
    app.kinematic_mover_render
        .hand_over_light_states(&registry.borrow(), None, None);
    assert!(handed(&app).is_empty(), "no level, no state");
}
