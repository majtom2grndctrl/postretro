use super::*;
use glam::Vec3;
use postretro_entities::{EntityRegistry, RespawnRequest, Transform};
use postretro_foundation::{
    AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementComponent,
    PlayerMovementDescriptor, Seat, SpeedParams,
};

fn descriptor(name: &str, radius: f32, half_height: f32) -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        canonical_name: Some(name.into()),
        movement: Some(PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius,
                half_height,
                eye_height: 0.5,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 11.0,
                    crouch: 3.0,
                },
                accel: 10.0,
                step_height: 0.3,
                max_slope: 45.0,
                friction: GroundParams::DEFAULT_FRICTION,
                stop_speed: GroundParams::DEFAULT_STOP_SPEED,
            },
            air: AirParams {
                forward_steer: 0.0,
                accel: 0.7,
                max_control_speed: 0.5,
                bunny_hop: false,
                jumps: 0,
                jump_velocity: 5.5,
                jump_ceiling: 0.0,
            },
            fall: FallParams {
                terminal_velocity: 40.0,
            },
            stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
            stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
            knockback: Default::default(),
            sounds: None,
            dash: None,
            forgiveness: None,
            crouch: None,
            slide: None,
            swim: None,
            vault: None,
            view_feel: None,
        }),
        life_states: None,
        ghost: false,
        faction: None,
        tolerance: None,
        inventory: None,
        light: None,
        emitter: None,
        weapon: None,
        touchable: None,
        mesh: None,
        health: None,
        behavior: None,
    }
}

fn spawn(class: &str, origin: Vec3, yaw: f32, coop_only: bool) -> MapEntity {
    MapEntity {
        map_ordinal: None,
        classname: "player_spawn".into(),
        origin,
        angles: Vec3::new(0.0, yaw, 0.0),
        key_values: [
            ("entity_class".into(), class.into()),
            ("coop_only".into(), u8::from(coop_only).to_string()),
        ]
        .into_iter()
        .collect(),
        tags: Vec::new(),
    }
}

#[test]
fn respawn_placements_keep_map_order_and_first_player_spawn_eligibility() {
    let descriptors = vec![descriptor("small", 0.4, 0.8), descriptor("large", 0.7, 2.0)];
    let points = vec![
        spawn("small", Vec3::ZERO, 0.0, true),
        spawn("missing", Vec3::X, 0.0, false),
        spawn("large", Vec3::Y, 0.0, false),
        spawn("small", Vec3::Z, 0.0, false),
    ];
    let placements = build(&points, &descriptors, None, None);
    assert_eq!(
        placements
            .map_order
            .iter()
            .map(|placement| placement.map_index)
            .collect::<Vec<_>>(),
        [0, 2, 3]
    );
    assert_eq!(
        placements.first_player,
        Some(2),
        "first player skips coop-only and unresolved classes"
    );
    assert_eq!(
        build(&points, &descriptors, None, Some(3)).first_player,
        Some(3),
        "actual installed spawn takes precedence over an inferred one"
    );
    assert_eq!(
        build(&points[..1], &descriptors, None, None).first_player,
        Some(0),
        "all coop-only falls back just as initial spawn does"
    );
    assert!(
        build(&points[1..2], &descriptors, None, None)
            .map_order
            .is_empty()
    );
}

#[test]
fn respawn_placements_publish_actual_spawn_foot_and_yaw_across_different_capsules() {
    let descriptors = vec![descriptor("small", 0.4, 0.8), descriptor("large", 0.7, 2.0)];
    let points = vec![
        spawn("small", Vec3::new(-8.0, 1.0, 0.0), 0.7, false),
        spawn("large", Vec3::new(5.0, 3.0, 7.0), -1.2, false),
    ];
    let placements = build(&points, &descriptors, None, None);
    let mut registry = EntityRegistry::new();
    for (index, point) in points.iter().enumerate() {
        let pawn =
            crate::scripting::builtins::net_descriptor::spawn_net_slot_pawn_with_carried_loadout(
                point,
                &descriptors,
                &mut registry,
                None,
                None,
            )
            .unwrap();
        let actual = registry.get_component::<Transform>(pawn).unwrap();
        let movement = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap();
        let foot =
            actual.position - Vec3::Y * (movement.capsule.half_height + movement.capsule.radius);
        assert!((placements.map_order[index].foot_anchor - foot).length() < 1e-5);
        let yaw = actual.rotation.to_euler(glam::EulerRot::YXZ).0;
        assert!((placements.map_order[index].yaw - yaw).abs() < 1e-5);
    }
    // The small pawn retains its archetype at the large pawn's fallback point.
    // Publishing a large standing centre here would lift its feet by 1.5m.
    let retained = &descriptors[0].movement.as_ref().unwrap().capsule;
    let fallback_centre =
        placements.map_order[1].foot_anchor + Vec3::Y * (retained.radius + retained.half_height);
    let large_centre =
        crate::scripting::builtins::player_spawn::standing_position(&points[1], &descriptors, None);
    assert!((large_centre.y - fallback_centre.y - 1.5).abs() < 1e-5);
    assert!(
        (fallback_centre.y - retained.radius - retained.half_height - points[1].origin.y).abs()
            < 1e-5
    );
}

#[test]
fn respawn_placements_publish_assigned_seats_and_keep_actual_first_spawn_after_join() {
    let ctx = ScriptCtx::new();
    let descriptors = vec![descriptor("small", 0.4, 0.8)];
    let points: Vec<_> = (0..7)
        .map(|index| spawn("small", Vec3::X * index as f32 * 3.0, 0.0, false))
        .collect();
    let mut seats = SeatTable::local_only();
    let mut registry = EntityRegistry::new();
    let host = registry.spawn(Transform::default());
    seats.bind_level_spawn_placement(host, 3);
    seats.bind_pawn(&mut registry, Seat(0), host);
    assert_eq!(seats.placement_for_seat(Seat(0)), Some(3));
    assert_eq!(
        build(&points, &descriptors, Some(&seats), None).first_player,
        Some(3),
        "bound seat zero is the actual first player"
    );
    publish(&ctx, &points, &descriptors, Some(&seats), Some(3));
    for client in 1..=5 {
        let seat = seats.admit_or_reclaim(client, None, false).unwrap().seat;
        let assigned = seats
            .assign_joiner_placement(&registry, seat, &[0, 1, 2, 3, 4, 5, 6])
            .unwrap();
        publish(&ctx, &points, &descriptors, Some(&seats), None);
        assert_eq!(
            ctx.respawn_placements.borrow().seats.get(&seat),
            Some(&assigned)
        );
        let pawn = registry.spawn(Transform::default());
        seats.bind_pawn(&mut registry, seat, pawn);
    }
    assert_eq!(
        ctx.respawn_placements.borrow().seats.len(),
        6,
        "publication derives the whole assigned-seat set"
    );
    assert_eq!(ctx.respawn_placements.borrow().first_player, Some(3));
    ctx.owner_slot_writes_enabled.set(false);
    publish(&ctx, &[], &[], None, None);
    assert_eq!(
        ctx.respawn_placements.borrow().seats.len(),
        6,
        "client role cannot replace authoritative placement data"
    );
}

#[test]
fn respawn_o20_actual_level_unload_clears_requests_and_placements_while_suspend_keeps_them() {
    let mut app = crate::startup::lifecycle::tests::test_app();
    let ctx = crate::startup::lifecycle::tests::script_ctx(&app);
    let pawn = ctx.registry.borrow_mut().spawn(Transform::default());
    ctx.pending_respawns.borrow_mut().insert(
        pawn,
        RespawnRequest {
            pawn,
            health: Some(50.0),
            state: Some("down".into()),
        },
    );
    *ctx.respawn_placements.borrow_mut() = RespawnPlacements {
        seats: [(Seat(0), 3)].into_iter().collect(),
        map_order: vec![RespawnPlacement {
            map_index: 3,
            foot_anchor: Vec3::ZERO,
            yaw: 0.0,
        }],
        first_player: Some(3),
    };
    app.clear_surface_lifetime_level_state();
    assert_eq!(
        ctx.pending_respawns.borrow().len(),
        1,
        "surface/suspend cleanup is not a level retirement"
    );
    assert_eq!(ctx.respawn_placements.borrow().first_player, Some(3));
    app.unload_level();
    assert!(ctx.pending_respawns.borrow().is_empty());
    let placements = ctx.respawn_placements.borrow();
    assert!(placements.map_order.is_empty());
    assert!(placements.seats.is_empty());
    assert!(placements.first_player.is_none());
}
