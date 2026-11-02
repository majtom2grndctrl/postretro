use super::*;
use crate::player_events::tests::{World, input, lt, number, on_player, player_event};
use postretro_entities::data_descriptors::{EntityTypeDescriptor, LifeStatesDescriptor};
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_foundation::{
    Capability, LifeStateProfile, PlayerMovementComponent, PlayerMovementDescriptor, Seat,
    player_capability,
};
use postretro_scripting_core::data_descriptors::PlayerEventEdge;
use serde_json::json;

pub(crate) fn install_states(world: &World, pawns: &[EntityId]) {
    let movement = crate::trigger_bindings::group_tick_tests::movement();
    let descriptor = EntityTypeDescriptor {
        canonical_name: Some("player".into()),
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
        movement: Some(PlayerMovementDescriptor {
            capsule: movement.capsule,
            ground: movement.ground_params,
            air: movement.air,
            fall: movement.fall,
            dash: movement.dash,
            crouch: movement.crouch,
            slide: movement.slide,
            swim: movement.swim,
            vault: movement.vault,
            view_feel: movement.view_feel,
            sounds: movement.sounds,
            knockback: movement.knockback,
            forgiveness: None,
            stuck_stop_enabled: movement.stuck_stop_enabled,
            stuck_stop_threshold: movement.stuck_stop_threshold,
        }),
        life_states: Some(LifeStatesDescriptor {
            initial: "alive".into(),
            states: [
                ("alive".into(), LifeStateProfile::default()),
                ("down".into(), LifeStateProfile::default()),
                (
                    "blocked".into(),
                    LifeStateProfile {
                        fire: false,
                        ..Default::default()
                    },
                ),
            ]
            .into(),
            rejoin: Default::default(),
            state_ids: Default::default(),
            table_revision: 0,
        }),
    };
    world
        .script_ctx
        .data_registry
        .borrow_mut()
        .replace_entity_types(vec![descriptor]);
    let mut registry = world.script_ctx.registry.borrow_mut();
    for &pawn in pawns {
        registry
            .set_component(
                pawn,
                DescriptorProvenance {
                    canonical_name: "player".into(),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::PlayerSpawn,
                },
            )
            .unwrap();
    }
    set_life_state(&world.script_ctx, &mut registry, pawns, "alive");
}

#[test]
fn life_state_player_event_applies_only_to_subject_and_same_state_preserves_elapsed() {
    let mut world = World::new();
    let first = world.spawn_player(Some(Seat(0)), 0.0);
    let second = world.spawn_player(Some(Seat(1)), 100.0);
    install_states(&world, &[first, second]);
    world.install(
        vec![on_player("down", "setLifeState", json!({"state":"down"}))],
        vec![player_event(
            PlayerEventEdge::Becomes,
            lt(input("player.health"), number(1.0)),
            &["down"],
        )],
    );
    world.tick();
    let data = world.script_ctx.data_registry.borrow();
    let down = data
        .life_state_names
        .iter()
        .position(|name| name == "down")
        .unwrap() as u8;
    let alive = data
        .life_state_names
        .iter()
        .position(|name| name == "alive")
        .unwrap() as u8;
    drop(data);
    let mut registry = world.script_ctx.registry.borrow_mut();
    let first_state = registry
        .get_component::<PlayerMovementComponent>(first)
        .unwrap();
    assert_eq!(first_state.life_state, down);
    assert_eq!(first_state.life_state_elapsed_ticks, 0);
    assert_eq!(life_ms(&registry, first), 0.0);
    assert_eq!(
        registry
            .get_component::<PlayerMovementComponent>(second)
            .unwrap()
            .life_state,
        alive
    );
    let ComponentValue::PlayerMovement(first_state) = registry
        .get_component_value_mut(first, ComponentKind::PlayerMovement)
        .unwrap()
    else {
        unreachable!()
    };
    first_state.life_state_elapsed_ticks = 90;
    set_life_state(&world.script_ctx, &mut registry, &[first], "down");
    assert_eq!(
        registry
            .get_component::<PlayerMovementComponent>(first)
            .unwrap()
            .life_state_elapsed_ticks,
        90
    );
}

#[test]
fn life_state_fire_profile_replaces_health_gate_in_both_directions() {
    let world = World::new();
    let pawn = world.spawn_player(Some(Seat(0)), 0.0);
    install_states(&world, &[pawn]);
    let mut registry = world.script_ctx.registry.borrow_mut();
    set_life_state(&world.script_ctx, &mut registry, &[pawn], "down");
    assert!(crate::weapon::pawn_gate::pawn_can_fire(&registry, pawn));
    crate::scripting_systems::health::sweep_deaths(&mut registry);
    assert!(crate::weapon::pawn_gate::pawn_can_fire(&registry, pawn));
    set_life_state(&world.script_ctx, &mut registry, &[pawn], "blocked");
    assert!(!crate::weapon::pawn_gate::pawn_can_fire(&registry, pawn));
    assert_eq!(
        player_capability(
            registry
                .get_component::<PlayerMovementComponent>(pawn)
                .unwrap(),
            Capability::Fire
        ),
        Some(false)
    );
    postretro_foundation::clear_life_state(
        match registry
            .get_component_value_mut(pawn, ComponentKind::PlayerMovement)
            .unwrap()
        {
            ComponentValue::PlayerMovement(movement) => movement,
            _ => unreachable!(),
        },
        0,
    );
    assert!(
        !crate::weapon::pawn_gate::pawn_can_fire(&registry, pawn),
        "undeclared keeps zero-HP refusal"
    );
}

#[test]
fn life_state_clock_advances_for_remote_pawn_without_commands() {
    let world = World::new();
    let pawn = world.spawn_player(Some(Seat(1)), 0.0);
    install_states(&world, &[pawn]);
    let mut registry = world.script_ctx.registry.borrow_mut();
    assert_eq!(life_ms(&registry, pawn), 0.0);
    advance_unstepped_life_state_time(&mut registry, &[], None);
    assert_eq!(life_ms(&registry, pawn), 16.0);
    assert_eq!(
        registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .life_state_elapsed_ticks,
        1
    );
}

#[test]
fn life_state_runtime_missing_profile_warns_and_preserves_other_pawns() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let world = World::new();
    let first = world.spawn_player(Some(Seat(0)), 0.0);
    let second = world.spawn_player(Some(Seat(1)), 0.0);
    let undeclared = world.spawn_player(Some(Seat(2)), 0.0);
    install_states(&world, &[first, second]);
    let original = world.script_ctx.data_registry.borrow().entities[0].clone();
    let mut other = original.clone();
    other.canonical_name = Some("other".into());
    other
        .life_states
        .as_mut()
        .unwrap()
        .states
        .retain(|name, _| name == "alive");
    world
        .script_ctx
        .data_registry
        .borrow_mut()
        .replace_entity_types(vec![original, other]);
    let mut registry = world.script_ctx.registry.borrow_mut();
    let mut provenance = registry
        .get_component::<DescriptorProvenance>(second)
        .unwrap()
        .clone();
    provenance.canonical_name = "other".into();
    registry.set_component(second, provenance).unwrap();
    let before = registry
        .get_component::<PlayerMovementComponent>(second)
        .unwrap()
        .life_state;
    set_life_state(
        &world.script_ctx,
        &mut registry,
        &[first, second, undeclared],
        "down",
    );
    assert_ne!(
        registry
            .get_component::<PlayerMovementComponent>(first)
            .unwrap()
            .life_state,
        before
    );
    assert_eq!(
        registry
            .get_component::<PlayerMovementComponent>(second)
            .unwrap()
            .life_state,
        before
    );
    assert_eq!(
        registry
            .get_component::<PlayerMovementComponent>(undeclared)
            .unwrap()
            .life_state,
        postretro_foundation::UNDECLARED_LIFE_STATE
    );
    let warnings: Vec<_> = capture
        .records()
        .into_iter()
        .filter(|record| record.level == log::Level::Warn)
        .collect();
    assert_eq!(warnings.len(), 2);
    assert!(
        warnings
            .iter()
            .all(|record| record.message.contains("down") && record.message.contains("skipping"))
    );
}

#[test]
fn life_state_o15_trigger_write_reads_zero_before_next_host_tick() {
    use crate::trigger_commands::{BoundTarget, BoundTriggerCommand, TriggerFireContext};
    let world = World::new();
    let pawn = world.spawn_player(Some(Seat(1)), 0.0);
    install_states(&world, &[pawn]);
    let state_id = world
        .script_ctx
        .data_registry
        .borrow()
        .life_state_names
        .iter()
        .position(|name| name == "down")
        .unwrap() as u8;
    let command = BoundTriggerCommand::SetLifeState {
        target: BoundTarget::Activators,
        state_id,
    };
    let mut scope =
        postretro_scripting_core::ir_scopes::DispatchScope::script(world.script_ctx.clone(), &[]);
    let mut registry = world.script_ctx.registry.borrow_mut();
    command.execute_with_script_ctx(
        &mut registry,
        &world.script_ctx,
        &mut scope,
        &crate::mover_commands::MoverCommandDiagnostics::default(),
        &crate::spawner::SpawnContext::default(),
        &TriggerFireContext {
            activator: Some(pawn),
            ..Default::default()
        },
    );
    assert_eq!(
        registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .life_state_elapsed_ticks,
        0
    );
    assert_eq!(life_ms(&registry, pawn), 0.0);
    advance_unstepped_life_state_time(&mut registry, &[], None);
    assert_eq!(life_ms(&registry, pawn), 16.0);
    assert_eq!(
        registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .life_state_elapsed_ticks,
        1
    );
}

#[test]
fn life_state_o39_wait_tail_enters_at_drain_and_reload_drops_pending_tail() {
    use crate::scripting_systems::reaction_scheduler::{
        ReactionScheduler, register_reaction_control_primitives,
    };
    use postretro_entities::{
        GroupKind, GroupTarget, NamedReaction, ReactionDescriptor, SequenceStep, SequenceTarget,
    };
    use postretro_scripting_core::reaction_dispatch::fire_named_event_with_sequences;
    use postretro_scripting_core::reaction_registry::SystemReactionRegistry;
    let mut world = World::new();
    let pawn = world.spawn_player(Some(Seat(1)), 0.0);
    install_states(&world, &[pawn]);
    world.install(
        vec![NamedReaction {
            name: "late".into(),
            descriptor: ReactionDescriptor::Sequence(vec![
                SequenceStep {
                    id: SequenceTarget::Wait,
                    primitive: "wait".into(),
                    args: json!({"durationMs":1}),
                },
                SequenceStep {
                    id: SequenceTarget::Group(GroupTarget {
                        kind: GroupKind::Player,
                        tag: None,
                    }),
                    primitive: "setLifeState".into(),
                    args: json!({"state":"down"}),
                },
            ]),
        }],
        vec![],
    );
    let scheduler = ReactionScheduler::default();
    scheduler.set_role(crate::scripting_systems::reaction_scheduler::SchedulerRole::Host);
    let mut sequences = SequencedPrimitiveRegistry::new();
    register_reaction_control_primitives(&mut sequences, scheduler.clone());
    register_sequenced_life_state_primitives(&mut sequences, world.script_ctx.clone());
    let mut reactions = ReactionPrimitiveRegistry::new();
    register_life_state_reaction_primitives(&mut reactions, world.script_ctx.clone());
    let systems = SystemReactionRegistry::new();
    let fire = || {
        let _ = fire_named_event_with_sequences(
            "late",
            &world.script_ctx.data_registry.borrow(),
            &sequences,
            &reactions,
            &systems,
            &world.script_ctx,
            None,
        );
    };
    fire();
    assert_eq!(scheduler.pending_len(), 1);
    scheduler.begin_frame();
    scheduler.evaluate(&[]);
    scheduler.drain_landings(
        &world.script_ctx.data_registry.borrow(),
        &sequences,
        &reactions,
        &systems,
        &world.script_ctx,
    );
    let movement = world
        .script_ctx
        .registry
        .borrow()
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .clone();
    assert_eq!(
        world.script_ctx.data_registry.borrow().life_state_names[usize::from(movement.life_state)],
        "down"
    );
    assert_eq!(movement.life_state_elapsed_ticks, 0);
    assert_eq!(life_ms(&world.script_ctx.registry.borrow(), pawn), 0.0);
    advance_unstepped_life_state_time(&mut world.script_ctx.registry.borrow_mut(), &[], None);
    assert_eq!(life_ms(&world.script_ctx.registry.borrow(), pawn), 16.0);
    assert_eq!(
        world
            .script_ctx
            .registry
            .borrow()
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .life_state_elapsed_ticks,
        1
    );
    set_life_state(
        &world.script_ctx,
        &mut world.script_ctx.registry.borrow_mut(),
        &[pawn],
        "alive",
    );
    fire();
    scheduler.clear(); // The production hot-reload recompose discards parked instances.
    scheduler.begin_frame();
    scheduler.evaluate(&[]);
    scheduler.drain_landings(
        &world.script_ctx.data_registry.borrow(),
        &sequences,
        &reactions,
        &systems,
        &world.script_ctx,
    );
    let registry = world.script_ctx.registry.borrow();
    let movement = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap();
    assert_eq!(
        world.script_ctx.data_registry.borrow().life_state_names[usize::from(movement.life_state)],
        "alive"
    );
}

fn life_ms(registry: &EntityRegistry, pawn: EntityId) -> f32 {
    match postretro_scripting_core::player_slots::player_slot_value(
        registry,
        postretro_scripting_core::player_slots::PlayerSlot::LifeStateMs,
        pawn,
    ) {
        Some(postretro_entities::SlotValue::Number(value)) => value,
        other => panic!("missing life elapsed fact: {other:?}"),
    }
}

#[test]
fn life_state_o15_player_event_first_next_pass_reads_16_and_threshold_first_at_900() {
    let mut world = World::new();
    let pawn = world.spawn_player(Some(Seat(0)), 0.0);
    install_states(&world, &[pawn]);
    world.install(
        vec![on_player("down", "setLifeState", json!({"state":"down"}))],
        vec![player_event(
            PlayerEventEdge::Becomes,
            lt(input("player.health"), number(1.0)),
            &["down"],
        )],
    );
    world.tick();
    assert_eq!(life_ms(&world.script_ctx.registry.borrow(), pawn), 0.0);
    for tick in 1..=900 {
        advance_unstepped_life_state_time(&mut world.script_ctx.registry.borrow_mut(), &[], None);
        let ms = life_ms(&world.script_ctx.registry.borrow(), pawn);
        if tick == 1 {
            assert_eq!(ms, 16.0);
        }
        assert_eq!(ms >= 15_000.0, tick == 900, "tick {tick}, {ms}ms");
        world.tick();
    }
}

#[test]
fn life_state_o27_short_state_is_seen_for_exactly_one_host_pass() {
    let mut world = World::new();
    let pawn = world.spawn_player(Some(Seat(0)), 100.0);
    install_states(&world, &[pawn]);
    world.install(
        vec![on_player("leave", "setLifeState", json!({"state":"alive"}))],
        vec![player_event(
            PlayerEventEdge::Becomes,
            input("player.inLifeState:down"),
            &["leave"],
        )],
    );
    set_life_state(
        &world.script_ctx,
        &mut world.script_ctx.registry.borrow_mut(),
        &[pawn],
        "down",
    );
    let before = world
        .script_ctx
        .registry
        .borrow()
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .life_state;
    world.tick();
    let after = world
        .script_ctx
        .registry
        .borrow()
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .life_state;
    assert_ne!(before, after);
    world.tick();
    assert_eq!(
        world
            .script_ctx
            .registry
            .borrow()
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .life_state,
        after
    );
}
