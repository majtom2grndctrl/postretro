//! Full-tick restore boundaries sharing the respawn fixture.
use super::*;

#[test]
fn respawn_remote_without_command_uses_transform_yaw_and_first_elapsed_pass() {
    use crate::player_events::tests::{input, number, on_player, player_event};
    use postretro_scripting_core::data_descriptors::PlayerEventEdge;
    let rig = Rig::new();
    let (host, _) = rig.spawn(0, Vec3::new(100.0, 1.2, 0.0), true);
    let (pawn, _) = rig.spawn(1, Vec3::new(60.0, 1.2, 0.0), false);
    let before = rig.pose(pawn).rotation.to_euler(glam::EulerRot::YXZ).0;
    for subject in [host, pawn] {
        crate::life_state_commands::set_life_state(
            &rig.ctx,
            &mut rig.ctx.registry.borrow_mut(),
            &[subject],
            "down",
        );
        let mut movement = rig.movement(subject);
        movement.capsule.half_height = 0.4;
        movement.capsule.eye_height = 0.2;
        movement.movement_state = MovementState::Crouching { eye_current: 0.2 };
        movement.velocity = Vec3::splat(7.0);
        movement.knockback_velocity = Vec3::splat(2.0);
        movement.push_velocity = Vec3::splat(1.0);
        let mut pose = rig.pose(subject);
        pose.position.y = 0.8;
        let mut registry = rig.ctx.registry.borrow_mut();
        registry.set_component(subject, movement).unwrap();
        registry.set_component(subject, pose).unwrap();
    }
    rig.enqueue(host, None, None);
    rig.enqueue(pawn, Some(50.0), None);
    rig.tick(&sim_command(false, false), &[]);
    for (subject, x) in [(host, 0.0), (pawn, 8.0)] {
        let movement = rig.movement(subject);
        assert_eq!(movement.capsule.half_height, 0.8);
        assert_eq!(movement.capsule.eye_height, 0.5);
        assert_eq!(movement.ground, postretro_foundation::GroundRef::World);
        assert_eq!(movement.velocity, Vec3::ZERO);
        assert_eq!(movement.knockback_velocity, Vec3::ZERO);
        assert_eq!(movement.push_velocity, Vec3::ZERO);
        assert_eq!(rig.pose(subject).position, Vec3::new(x, 1.2, 0.0));
        assert!(((rig.pose(subject).rotation * Vec3::NEG_Z) - Vec3::NEG_X).length() < EPS);
        assert_eq!(movement.life_state_elapsed_ticks, 1);
        assert_eq!(rig.name(subject), "alive");
    }
    let movement = rig.movement(pawn);
    assert_eq!(rig.pose(pawn).position, Vec3::new(8.0, 1.2, 0.0));
    assert_eq!(movement.velocity, Vec3::ZERO);
    assert!(
        (movement.teleport.accumulated_yaw - (std::f32::consts::FRAC_PI_2 - before)).abs() < EPS
    );
    assert!(((rig.pose(pawn).rotation * Vec3::NEG_Z) - Vec3::NEG_X).length() < EPS);
    assert_eq!(
        movement.life_state_elapsed_ticks, 1,
        "unstepped remote pawn still advances once"
    );
    {
        let mut data = rig.ctx.data_registry.borrow_mut();
        data.set_level_reactions(vec![on_player(
            "witness",
            "grantHealth",
            json!({"amount":25}),
        )]);
        data.set_level_player_events(vec![player_event(
            PlayerEventEdge::Becomes,
            postretro_foundation::IrNode::Eq {
                a: Box::new(input("player.lifeStateMs")),
                b: Box::new(number(16.0)),
            },
            &["witness"],
        )]);
        data.recompose(&[]);
    }
    let mut table = crate::player_events::PlayerEventTable::build(
        &rig.ctx,
        Default::default(),
        Default::default(),
        None,
    );
    table.run_tick(&rig.ctx, &[], &mut Vec::new());
    assert_eq!(
        rig.health(pawn).current,
        75.0,
        "T+1 player-event pass sees 16ms after respawn"
    );
}
