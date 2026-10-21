// The first-record camera hold on a connected client: a joiner whose first pawn
// record could not merge shows a stale gated pose, and the record that finally
// writes the authoritative pose must not be blended to from it.
// See: context/lib/networking.md §Snapshot apply ordering

use super::app_tests::{client_app, level};
use super::*;
use crate::frame_timing::{FrameTiming, InterpolableState};
use postretro_foundation::{GroundRef, PlayerMovementComponent};

const ALPHAS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];
const STALE_X: f32 = 0.0;
const MERGED_X: f32 = 40.0;

/// Rendered eye x at each interpolation alpha.
fn rendered_eye_x(app: &crate::App) -> [f32; 5] {
    ALPHAS.map(|alpha| {
        app.frame_timing
            .previous_state
            .lerp(&app.frame_timing.current_state, alpha)
            .position
            .x
    })
}

/// Run a gated join to the merge frame: three ticks on the stale pose with
/// prediction inert, then the reconcile write of the authoritative pose the host
/// moved the pawn to, then the app's adoption step with `hold`. Returns the app
/// and the rendered eye on the merge frame and on the tick after it.
fn gated_merge(hold: bool) -> (crate::App, [f32; 5], [f32; 5]) {
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform {
        position: Vec3::X * STALE_X,
        ..Default::default()
    });
    let movement =
        PlayerMovementComponent::from_descriptor(&crate::mod_digest::tests::movement_descriptor());
    let eye_height = movement.capsule.eye_height;
    registry.set_component(pawn, movement).unwrap();
    registry.mark_local_player_pawn(pawn).unwrap();
    let mut app = client_app(&level(Vec::new()), registry);
    app.camera = crate::camera::Camera::new(Vec3::new(STALE_X, eye_height, 0.0), 0.3, 0.2);
    app.frame_timing = FrameTiming::new(InterpolableState::new(app.camera.position));
    app.mover_yaw_carry_ground = GroundRef::Mover(7);
    for _ in 0..3 {
        app.run_client_predicted_tick(true, |_| None);
    }
    for x in rendered_eye_x(&app) {
        assert!(
            (x - STALE_X).abs() < 1.0e-5,
            "fixture: the gated eye is stale"
        );
    }

    let ctx = app.session.as_ref().unwrap().scripting.script_ctx.clone();
    {
        let mut registry = ctx.registry.borrow_mut();
        let merged = Transform {
            position: Vec3::X * MERGED_X,
            ..Default::default()
        };
        // As reconcile writes it: previous == current, so the mesh does not lerp.
        registry.set_presentation_transform(pawn, merged).unwrap();
    }
    {
        let registry = ctx.registry.borrow();
        apply_local_adoptions(
            &mut app.camera,
            &mut app.frame_timing,
            &mut app.mover_yaw_carry_ground,
            &mut app.pending_teleport_sounds,
            &app.collision_world.teleporters,
            &registry,
            &[],
            hold.then_some(pawn),
        );
    }
    let merge_frame = rendered_eye_x(&app);
    app.run_client_predicted_tick(true, |_| None);
    let next_tick = rendered_eye_x(&app);
    (app, merge_frame, next_tick)
}

// The joiner's pawn walked or teleported on the host during the gate. The hold
// snaps the eye to the merged authoritative pose: at no alpha, on the merge frame
// or the tick after it, does the camera render between the stale and merged eye.
// The record is still adopted silently: no cue, no view turn, no carry change.
#[test]
fn teleport_first_record_hold_snaps_camera_to_merged_eye_without_cue_turn_or_carry() {
    let (app, merge_frame, next_tick) = gated_merge(true);
    for (frame, xs) in [("merge frame", merge_frame), ("next tick", next_tick)] {
        for x in xs {
            assert!(
                (x - MERGED_X).abs() < 1.0e-5,
                "{frame}: rendered eye at x {x}, not the merged eye"
            );
        }
    }
    assert!((app.camera.yaw - 0.3).abs() < 1.0e-6, "no view turn");
    assert!((app.camera.pitch - 0.2).abs() < 1.0e-6);
    assert!(app.pending_teleport_sounds.is_empty(), "no cue");
    assert_eq!(
        app.mover_yaw_carry_ground,
        GroundRef::Mover(7),
        "carry unchanged"
    );

    // Without the hold, the next tick blends from the stale eye.
    let (_, _, unheld) = gated_merge(false);
    assert!(
        unheld
            .iter()
            .any(|&x| x > STALE_X + 1.0 && x < MERGED_X - 1.0),
        "control: an unheld merge renders between the poses: {unheld:?}"
    );
}
