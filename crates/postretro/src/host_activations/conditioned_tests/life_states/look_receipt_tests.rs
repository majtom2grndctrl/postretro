//! Owner look sampling across delayed authoritative life-state receipt.
//! See: context/lib/networking.md §Snapshot apply ordering

use super::*;

#[test]
fn life_state_o38_received_look_gate_freezes_sent_facing_and_host_accepts_in_flight_turns() {
    let mut life = LifeFixture::new(descriptor(false, false, 1, "press"), true);
    {
        let mut descriptors = life.ctx.data_registry.borrow().entities.clone();
        let states = descriptors
            .iter_mut()
            .find_map(|entity| entity.life_states.as_mut())
            .unwrap();
        states.states.get_mut("down").unwrap().look = false;
        life.ctx
            .data_registry
            .borrow_mut()
            .replace_entity_types(descriptors);
        life.fixture.descriptors = life.ctx.data_registry.borrow().entities.clone();
    }
    let data = life.ctx.data_registry.borrow();
    let tuning = netcode::tuning_payload_for_pawn(
        &life.fixture.host.borrow(),
        life.fixture.host_actors.pawn,
        &data.entities,
        None,
        &postretro_combat_model::EnvironmentTuning {
            level_gravity: 0.0,
            fluids: Default::default(),
        },
        &data.life_state_names,
        data.life_state_table_revision,
    );
    drop(data);
    netcode::host_send_tuning_if_changed(
        &mut life.fixture.server,
        &mut Default::default(),
        CLIENT,
        tuning,
    );
    for _ in 0..40 {
        life.step(neutral());
    }
    life.fixture.life_owner_adoptions.clear();

    let mut camera = crate::camera::Camera::new(Vec3::ZERO, 0.0, 0.0);
    life.zero_shooter(false);
    life.step(neutral());
    let mut samples = BTreeMap::new();
    let mut frozen = None;
    let mut turns_before_receipt = 0;
    // The camera takes the production writes around each tick: render-rate
    // look, the tick's mover yaw carry, then receipt-time teleport adoption.
    // Receipt may stop look; it may never move the view.
    let mut timing = crate::frame_timing::FrameTiming::new(
        crate::frame_timing::InterpolableState::new(camera.position),
    );
    let mut carry_ground = crate::local_player_ground(&life.fixture.local.borrow());
    let mut arrival_sounds = Vec::new();
    let mut adopted = 0;
    for _ in 0..100 {
        let allowed = {
            let registry = life.fixture.local.borrow();
            let movement = registry
                .get_component::<PlayerMovementComponent>(life.fixture.local_actors.pawn)
                .unwrap();
            player_capability(movement, Capability::Look).unwrap()
        };
        let before = (camera.yaw, camera.pitch);
        crate::life_state_eye::apply_look(&life.fixture.local.borrow(), &mut camera, 0.025, 0.01);
        crate::apply_mover_yaw_carry(&mut camera, carry_ground, &life.fixture.movers);
        carry_ground = crate::local_player_ground(&life.fixture.local.borrow());
        if allowed {
            turns_before_receipt += 1;
            assert!(camera.yaw > before.0 && camera.pitch > before.1);
        } else {
            let expected = *frozen.get_or_insert(before);
            near(camera.yaw, expected.0);
            near(camera.pitch, expected.1);
        }
        let mut command = neutral();
        command.movement.facing_yaw = camera.yaw;
        command.movement.facing_pitch = camera.pitch;
        life.fixture.aim_pitch = camera.pitch;
        samples.insert(life.next_tick, (camera.yaw, camera.pitch, allowed));
        let sent = (camera.yaw, camera.pitch);
        life.step(command);
        let adoptions = life.fixture.life_owner_adoptions[adopted..]
            .iter()
            .map(|&(_, adoption)| adoption)
            .collect::<Vec<_>>();
        adopted = life.fixture.life_owner_adoptions.len();
        crate::runtime_teleporters::apply_local_adoptions(
            &mut camera,
            &mut timing,
            &mut carry_ground,
            &mut arrival_sounds,
            &life.fixture.world.teleporters,
            &life.fixture.local.borrow(),
            Vec3::ZERO,
            &adoptions,
            None,
        );
        near(camera.yaw, sent.0);
        near(camera.pitch, sent.1);
    }
    assert!(
        turns_before_receipt > 0,
        "the owner keeps turning while down's record is in flight"
    );
    let frozen = frozen.expect("the host look:false stamp arrived through snapshot receive");
    assert!(life.fixture.receive.as_ref().unwrap().local_applies > 1);
    assert!(
        life.fixture.life_owner_adoptions.is_empty(),
        "ordinary life-state receipt has no teleport cut"
    );
    let mut accepted_turns = 0;
    let mut accepted_frozen = 0;
    for (_, resolved) in &life.fixture.resolved_commands {
        let Some(&(yaw, pitch, allowed)) = samples.get(&resolved.client_tick) else {
            continue;
        };
        near(resolved.command.movement.facing_yaw, yaw);
        near(resolved.command.movement.facing_pitch, pitch);
        if allowed {
            accepted_turns += 1;
        } else {
            accepted_frozen += 1;
        }
    }
    assert!(
        accepted_turns > 0,
        "host look:false never clamps commands sampled before receipt"
    );
    assert!(
        accepted_frozen > 0,
        "post-receipt frozen commands reach host resolution"
    );
    for (registry, pawn) in [
        (&life.fixture.host, life.fixture.host_actors.pawn),
        (&life.fixture.local, life.fixture.local_actors.pawn),
    ] {
        let registry = registry.borrow();
        let transform = registry.get_component::<Transform>(pawn).unwrap();
        let movement = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap();
        assert_eq!(player_capability(movement, Capability::Look), Some(false));
        near(transform.rotation.to_euler(glam::EulerRot::YXZ).0, frozen.0);
    }
    near(camera.yaw, frozen.0);
    near(camera.pitch, frozen.1);
    for input in life
        .fixture
        .sent
        .iter()
        .filter(|input| samples.contains_key(&input.client_tick))
    {
        let sample = samples[&input.client_tick];
        near(input.movement.facing_yaw, sample.0);
        near(input.movement.aim_pitch, sample.1);
    }
}
