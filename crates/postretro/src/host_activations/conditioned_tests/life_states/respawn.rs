//! Respawn crosses conditioned admission, owning reconciliation and observer presentation.
use super::*;
use postretro_entities::{RespawnPlacement, RespawnPlacements};
use postretro_foundation::{NO_TELEPORTER, enter_life_state};
use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;
use std::f32::consts::FRAC_PI_2;

const ARRIVAL: Vec3 = Vec3::new(40.0, 0.0, 0.0);

fn enable_respawn(life: &mut LifeFixture, yaw: f32) {
    let capsule = life
        .ctx
        .registry
        .borrow()
        .get_component::<PlayerMovementComponent>(life.fixture.host_actors.pawn)
        .unwrap()
        .capsule
        .clone();
    *life.ctx.respawn_placements.borrow_mut() = RespawnPlacements {
        seats: BTreeMap::from([(Seat(1), 0)]),
        map_order: vec![RespawnPlacement {
            map_index: 0,
            foot_anchor: ARRIVAL - Vec3::Y * (capsule.radius + capsule.half_height),
            yaw,
        }],
        first_player: Some(0),
    };
    life.fixture.respawn_ctx = Some(life.ctx.clone());
}

fn request(ctx: &ScriptCtx, pawn: EntityId) {
    let mut reactions = ReactionPrimitiveRegistry::new();
    postretro_sim::respawn_commands::register_respawn_reaction_primitives(
        &mut reactions,
        ctx.clone(),
    );
    assert!(
        reactions
            .dispatch(
                "respawn",
                &mut ctx.registry.borrow_mut(),
                &[pawn],
                &json!({"state":"alive"})
            )
            .unwrap()
    );
}

fn respawn_before_command_100(fixture: &mut Fixture) {
    if fixture.queues.resolved_cursor_for_test(CLIENT) != Some(99)
        || !fixture.queues.has_pending_tick_for_test(CLIENT, 100)
    {
        return;
    }
    let ctx = fixture.respawn_ctx.as_ref().unwrap().clone();
    {
        let mut registry = fixture.host.borrow_mut();
        postretro_sim::life_state_commands::set_life_state(
            &ctx,
            &mut registry,
            &[fixture.host_actors.pawn],
            "down",
        );
        let ComponentValue::PlayerMovement(pawn) = registry
            .get_component_value_mut(fixture.host_actors.pawn, ComponentKind::PlayerMovement)
            .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(player_capability(pawn, Capability::Fire), Some(false));
        pawn.velocity = Vec3::new(9.0, 0.0, 5.0);
        pawn.knockback_velocity = Vec3::new(2.0, 0.0, 3.0);
        pawn.push_velocity = Vec3::new(4.0, 0.0, 1.0);
    }
    request(&ctx, fixture.host_actors.pawn);
    fixture.before_switch_resolution = None;
}

#[test]
fn respawn_conditioned_o33_admits_start_in_new_state_and_turns_its_aim_before_one_movement_step() {
    let mut life = LifeFixture::new(descriptor(true, false, 1, "press"), false);
    enable_respawn(&mut life, FRAC_PI_2);
    life.fixture.before_switch_resolution = Some(respawn_before_command_100);
    let mut witnessed = false;
    while life.next_tick < 160 {
        let mut command = if life.next_tick == 100 {
            held(ActivationLane::Primary, true)
        } else {
            neutral()
        };
        command.movement.wish_dir = Vec2::new(0.0, 1.0);
        let waiting = life.fixture.before_switch_resolution.is_some();
        life.step(command);
        if waiting && life.fixture.before_switch_resolution.is_none() {
            witnessed = true;
            let (host_tick, resolved) = life.fixture.resolved_commands.last().unwrap();
            assert_eq!(resolved.client_tick, 100);
            assert_eq!(resolved.source, netcode::ResolutionSource::Real);
            near(resolved.command.movement.facing_yaw, FRAC_PI_2);
            let registry = life.ctx.registry.borrow();
            let pawn = registry
                .get_component::<PlayerMovementComponent>(life.fixture.host_actors.pawn)
                .unwrap();
            assert_eq!(pawn.life_state, life.alive_id);
            assert_eq!(player_capability(pawn, Capability::Fire), Some(true));
            assert_eq!(pawn.life_state_elapsed_ticks, 1);
            assert_eq!(pawn.teleport.generation, 1);
            assert_eq!(pawn.teleport.last_teleporter, NO_TELEPORTER);
            assert_eq!(pawn.knockback_velocity, Vec3::ZERO);
            assert_eq!(pawn.push_velocity, Vec3::ZERO);
            near(
                registry
                    .get_component::<Transform>(life.fixture.host_actors.pawn)
                    .unwrap()
                    .rotation
                    .to_euler(glam::EulerRot::YXZ)
                    .0,
                FRAC_PI_2,
            );
            // Compare with one production movement step from the descriptor's rest state.
            let descriptor = movement_descriptor();
            let mut expected = PlayerMovementComponent::from_descriptor(&descriptor);
            let data = life.ctx.data_registry.borrow();
            let states = data
                .entities
                .iter()
                .find_map(|entity| entity.life_states.as_ref())
                .unwrap();
            let (id, profile) = states.initial_state();
            enter_life_state(&mut expected, id, profile, states.table_revision);
            let (position, _, _) = postretro_physics::movement::tick_with_immersion_and_armed(
                &mut expected,
                &resolved.command.movement,
                &life.fixture.world,
                postretro_collision::EnvironmentView::level_only(0.0),
                DT,
                ARRIVAL,
                |_| false,
            );
            assert!(
                pawn.velocity.distance(expected.velocity) < 1e-5,
                "banked velocity survived or movement ran more than once"
            );
            assert!(
                registry
                    .get_component::<Transform>(life.fixture.host_actors.pawn)
                    .unwrap()
                    .position
                    .distance(position)
                    < 1e-5
            );
            let shot = life
                .fixture
                .authorized
                .iter()
                .find(|shot| shot.fire_tick == *host_tick)
                .expect("the same tick admits and authorizes the delivered start");
            let direction = shot
                .projectile_direction
                .expect("projectile authority freezes the turned start aim");
            assert!(
                direction.distance(Vec3::NEG_X) < 1e-4,
                "start aim stayed in its pre-respawn frame: {direction:?}"
            );
            assert!(
                shot.fire_origin.distance(ARRIVAL) < 2.0,
                "the shot originates at the arrival, not the old position"
            );
        }
    }
    assert!(
        witnessed,
        "the real conditioned command queue must reach the exact boundary"
    );
    assert_eq!(life.fixture.authorized.len(), 1);
    assert!(!life.fixture.outcomes.iter().any(|outcome| matches!(outcome, wire::ActivationOutcome::InitiationRejected { token, .. } if token.start_tick == 100)));
    assert!(
        life.fixture
            .life_owner_adoptions
            .iter()
            .any(|(_, adoption)| adoption.identity_changed
                && adoption.camera_cut
                && adoption.cue_teleporter.is_none()),
        "the owner receives the respawn through the teleport reconcile class"
    );
    life.assert_other_player_untouched();
}

fn install_teleporter(fixture: &mut Fixture) {
    use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
    use postretro_level_format::teleporters::{TeleportMode, TeleporterRecord, TeleportersSection};
    let mut planes = Vec::new();
    for axis in 0..3 {
        let mut normal = Vec3::ZERO;
        normal[axis] = 1.0;
        planes.push(EnvironmentPlane {
            normal: normal.to_array(),
            distance: 1.0,
        });
        planes.push(EnvironmentPlane {
            normal: (-normal).to_array(),
            distance: 1.0,
        });
    }
    let section = TeleportersSection {
        teleporters: vec![TeleporterRecord {
            source_entity: 1,
            mode: TeleportMode::Classic,
            enabled_on_spawn: true,
            brushes: vec![EnvironmentBrush {
                aabb_min: [-1.0; 3],
                aabb_max: [1.0; 3],
                planes,
            }],
            destination_origin: [10.0, 0.0, 0.0],
            destination_yaw: 0.0,
            landmark_origin: [0.0; 3],
            landmark_yaw: 0.0,
            arrival_sound: "fixture/teleporter".into(),
            on_teleport: String::new(),
        }],
    };
    let section = TeleportersSection::from_bytes(&section.to_bytes()).unwrap();
    fixture.world.teleporters = postretro_physics::teleporters::Teleporters::from_section(&section);
}

// This scenario has no player-event write to publish. The parent life fixture
// adds a second post-event snapshot; use the ordinary fixture's one publication.
fn step_snapshot_tick(life: &mut LifeFixture) {
    life.fixture.step(life.next_tick, neutral());
    life.next_tick += 1;
}

#[test]
fn respawn_conditioned_o10_o28_coalesced_observer_cuts_once_without_replaying_last_teleporter_cue()
{
    let mut life = LifeFixture::new(descriptor(false, false, 1, "press"), false);
    enable_respawn(&mut life, 0.0);
    let link = LinkConfig {
        delay: 75,
        ..LinkConfig::perfect()
    };
    life.fixture
        .attach_conditioned_observer(link, ScriptCtx::new().slot_table);
    for _ in 0..40 {
        life.step(neutral());
    }
    // The teleporter runs on an odd (unsent) snapshot tick; respawn is next/even.
    while life.fixture.tick.is_multiple_of(2) {
        life.step(neutral());
    }
    let teleport_tick = life.fixture.tick;
    assert!(
        life.fixture
            .switch_receive
            .as_ref()
            .unwrap()
            .observer
            .replication
            .latest_server_tick()
            .is_some()
    );
    life.fixture.life_owner_adoptions.clear();
    life.fixture.life_observed.clear();
    let mut host_poses = netcode::RemoteInterpolationBuffer::default();
    netcode::host_record_client_pawn_poses(
        &mut host_poses,
        &life.fixture.owners,
        &life.fixture.allocator,
        &life.ctx.registry.borrow(),
        teleport_tick - 1,
    );
    assert!(
        netcode::host_take_client_pawn_teleports(
            &mut host_poses,
            &life.fixture.owners,
            &life.fixture.allocator
        )
        .is_empty()
    );
    install_teleporter(&mut life.fixture);
    step_snapshot_tick(&mut life);
    assert_eq!(
        life.ctx
            .registry
            .borrow()
            .get_component::<PlayerMovementComponent>(life.fixture.host_actors.pawn)
            .unwrap()
            .teleport
            .generation,
        1
    );
    netcode::host_record_client_pawn_poses(
        &mut host_poses,
        &life.fixture.owners,
        &life.fixture.allocator,
        &life.ctx.registry.borrow(),
        teleport_tick,
    );
    let cues = netcode::host_take_client_pawn_teleports(
        &mut host_poses,
        &life.fixture.owners,
        &life.fixture.allocator,
    );
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].last_teleporter, 0);
    let mut sounds = Vec::new();
    crate::sound_events::observed_arrival_sounds(
        &life.fixture.world.teleporters,
        &cues,
        &mut sounds,
    );
    assert_eq!(
        sounds.len(),
        1,
        "the host's per-tick observer hears the real teleporter at T"
    );
    assert_eq!(
        life.fixture.life_forward_teleports.len(),
        1,
        "only the owning client's forward prediction reports its teleporter use"
    );
    let predicted = life.fixture.life_forward_teleports[0];
    let owner_sound = crate::sound_events::arrival_sound(
        &life.fixture.world.teleporters,
        predicted.teleporter,
        predicted.arrival,
    )
    .expect("forward prediction alone supplies the owner's T cue");
    assert_eq!(owner_sound.sound, "fixture/teleporter");
    request(&life.ctx, life.fixture.host_actors.pawn);
    step_snapshot_tick(&mut life);
    let respawn_tick = teleport_tick + 1;
    let registry = life.ctx.registry.borrow();
    let teleport = &registry
        .get_component::<PlayerMovementComponent>(life.fixture.host_actors.pawn)
        .unwrap()
        .teleport;
    assert_eq!(
        (teleport.generation, teleport.last_teleporter),
        (2, NO_TELEPORTER)
    );
    assert!(
        registry
            .get_component::<Transform>(life.fixture.host_actors.pawn)
            .unwrap()
            .position
            .distance(ARRIVAL)
            < 1e-5
    );
    netcode::host_record_client_pawn_poses(
        &mut host_poses,
        &life.fixture.owners,
        &life.fixture.allocator,
        &registry,
        respawn_tick,
    );
    drop(registry);
    let cues = netcode::host_take_client_pawn_teleports(
        &mut host_poses,
        &life.fixture.owners,
        &life.fixture.allocator,
    );
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].last_teleporter, NO_TELEPORTER);
    sounds.clear();
    crate::sound_events::observed_arrival_sounds(
        &life.fixture.world.teleporters,
        &cues,
        &mut sounds,
    );
    assert!(sounds.is_empty(), "the host sees the respawn cut silently");
    for _ in 0..30 {
        if !life.fixture.life_observed.is_empty()
            && life
                .fixture
                .life_owner_adoptions
                .iter()
                .any(|(_, adoption)| adoption.identity_changed)
        {
            break;
        }
        step_snapshot_tick(&mut life);
    }
    let cuts: Vec<_> = life
        .fixture
        .life_owner_adoptions
        .iter()
        .filter(|(_, adoption)| adoption.identity_changed)
        .collect();
    assert_eq!(
        cuts.len(),
        1,
        "one coalesced authoritative arrival follows the owner's forward-predicted teleporter"
    );
    assert!(cuts[0].1.camera_cut);
    assert_eq!(cuts[0].1.cue_teleporter, None);
    assert_eq!(
        life.fixture
            .local
            .borrow()
            .get_component::<PlayerMovementComponent>(life.fixture.local_actors.pawn)
            .unwrap()
            .teleport
            .generation,
        2
    );

    assert_eq!(
        life.fixture.life_observed.len(),
        1,
        "the observer receives one coalesced generation change"
    );
    let cue = life.fixture.life_observed[0].1;
    assert_eq!((cue.generation, cue.last_teleporter), (2, NO_TELEPORTER));
    assert!(cue.arrival.distance(ARRIVAL) < 1e-5);
    crate::sound_events::observed_arrival_sounds(
        &life.fixture.world.teleporters,
        &[cue],
        &mut sounds,
    );
    assert!(
        sounds.is_empty(),
        "the coalesced observer must not replay the earlier teleporter sound"
    );
    let network = wire::NetworkId(life.fixture.pawn_network());
    let observer = &mut life.fixture.switch_receive.as_mut().unwrap().observer;
    let entity = observer
        .replication
        .entity_network_ids()
        .into_iter()
        .find_map(|(entity, id)| (id == network).then_some(entity))
        .unwrap();
    netcode::client_sample_interpolation_at(
        &mut observer.registry,
        &mut observer.replication,
        f64::from(respawn_tick) - 0.5,
        0.0,
    );
    assert!(
        observer
            .registry
            .get_component::<Transform>(entity)
            .unwrap()
            .position
            .length()
            < 1e-5,
        "the interval before the arrival holds the old pose instead of blending across the map"
    );
    netcode::client_sample_interpolation_at(
        &mut observer.registry,
        &mut observer.replication,
        f64::from(respawn_tick),
        0.0,
    );
    assert!(
        observer
            .registry
            .get_component::<Transform>(entity)
            .unwrap()
            .position
            .distance(ARRIVAL)
            < 1e-5,
        "the arrival sample cuts to the respawn pose"
    );
}
