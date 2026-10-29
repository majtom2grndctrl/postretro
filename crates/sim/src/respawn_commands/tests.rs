//! Respawn verbs cross fixed-tick and deferred dispatch into one pending queue.
use super::*;
use crate::life_state_commands::set_life_state;
use crate::player_events::tests::{World, input, lt, number, on_player, player_event};
use postretro_entities::data_descriptors::{EntityTypeDescriptor, LifeStatesDescriptor};
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{NamedReaction, ReactionDescriptor, SequenceStep, SequenceTarget};
use postretro_foundation::{LifeStateProfile, PlayerMovementDescriptor, Seat};
use postretro_scripting_core::data_descriptors::PlayerEventEdge;
use postretro_test_log_capture::LogCapture;
use serde_json::json;

fn install_states(world: &World, pawns: &[EntityId]) {
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
fn respawn_player_event_coalesces_last_request_and_preserves_immediate_state_write() {
    let mut world = World::new();
    let pawn = world.spawn_player(Some(Seat(0)), 0.0);
    let other = world.spawn_player(Some(Seat(1)), 100.0);
    install_states(&world, &[pawn, other]);
    let before_position = world
        .script_ctx
        .registry
        .borrow()
        .get_component::<postretro_entities::Transform>(pawn)
        .unwrap()
        .position;
    world.install(
        vec![
            on_player("cycle", "respawn", json!({"health":25,"state":"down"})),
            on_player("cycle", "setLifeState", json!({"state":"down"})),
            on_player("cycle", "respawn", json!({"health":40,"state":"alive"})),
        ],
        vec![player_event(
            PlayerEventEdge::Becomes,
            lt(input("player.health"), number(1.0)),
            &["cycle"],
        )],
    );
    world.tick();
    let pending = world.script_ctx.pending_respawns.borrow();
    assert_eq!(pending.len(), 1);
    let request = pending.get(&pawn).unwrap();
    assert_eq!(request.pawn, pawn);
    assert_eq!(request.health, Some(40.0));
    assert_eq!(request.state.as_deref(), Some("alive"));
    assert!(!pending.contains_key(&other));
    drop(pending);
    let registry = world.script_ctx.registry.borrow();
    let movement = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap();
    assert_eq!(
        world.script_ctx.data_registry.borrow().life_state_names[usize::from(movement.life_state)],
        "down"
    );
    assert_eq!(world.health(pawn), 0.0);
    assert_eq!(world.health(other), 100.0);
    assert_eq!(
        registry
            .get_component::<postretro_entities::Transform>(pawn)
            .unwrap()
            .position,
        before_position
    );
    assert!(
        world.residuals.is_empty(),
        "respawn must bind into the fixed-tick command list"
    );
}

#[test]
fn respawn_named_targets_warn_skip_missing_descriptor_state_and_nonplayers() {
    let world = World::new();
    let pawn = world.spawn_player(Some(Seat(0)), 0.0);
    let other = world.spawn_player(Some(Seat(1)), 0.0);
    let undeclared = world.spawn_player(Some(Seat(2)), 0.0);
    let nonplayer = world.spawn_unbound(0.0);
    install_states(&world, &[pawn, other]);
    let original = world.script_ctx.data_registry.borrow().entities[0].clone();
    let mut alternate = original.clone();
    alternate.canonical_name = Some("alternate".into());
    alternate
        .life_states
        .as_mut()
        .unwrap()
        .states
        .retain(|name, _| name == "alive");
    world
        .script_ctx
        .data_registry
        .borrow_mut()
        .replace_entity_types(vec![original, alternate]);
    let mut registry = world.script_ctx.registry.borrow_mut();
    let mut provenance = registry
        .get_component::<DescriptorProvenance>(other)
        .unwrap()
        .clone();
    provenance.canonical_name = "alternate".into();
    registry.set_component(other, provenance).unwrap();
    let capture = LogCapture::start();
    enqueue_respawns(
        &world.script_ctx,
        &registry,
        &[pawn, other, undeclared, nonplayer],
        Some(25.0),
        Some("down"),
    );
    let pending = world.script_ctx.pending_respawns.borrow();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending.get(&pawn).unwrap().state.as_deref(), Some("down"));
    let warnings: Vec<_> = capture
        .records()
        .into_iter()
        .filter(|record| record.level == log::Level::Warn)
        .collect();
    assert_eq!(warnings.len(), 3);
    assert_eq!(
        warnings
            .iter()
            .filter(|record| record.message.contains("does not declare state `down`"))
            .count(),
        2
    );
    assert_eq!(
        warnings
            .iter()
            .filter(|record| record.message.contains("not a live player pawn"))
            .count(),
        1
    );
}

#[test]
fn respawn_default_allows_undeclared_players_but_refuses_clients_and_nonplayers() {
    let world = World::new();
    let pawn = world.spawn_player(None, 0.0);
    let nonplayer = world.spawn_unbound(0.0);
    let capture = LogCapture::start();
    enqueue_respawns(
        &world.script_ctx,
        &world.script_ctx.registry.borrow(),
        &[pawn, nonplayer],
        None,
        None,
    );
    let pending = world.script_ctx.pending_respawns.borrow();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending.get(&pawn).unwrap().state, None);
    assert_eq!(pending.get(&pawn).unwrap().health, None);
    drop(pending);
    world.script_ctx.pending_respawns.borrow_mut().clear();
    world.script_ctx.owner_slot_writes_enabled.set(false);
    enqueue_respawns(
        &world.script_ctx,
        &world.script_ctx.registry.borrow(),
        &[pawn, nonplayer],
        Some(25.0),
        None,
    );
    assert!(world.script_ctx.pending_respawns.borrow().is_empty());
    assert_eq!(
        capture
            .records()
            .iter()
            .filter(|record| record.level == log::Level::Warn)
            .count(),
        1
    );
}

#[test]
fn respawn_queued_name_survives_table_relower_and_removal_until_drain() {
    let world = World::new();
    let pawn = world.spawn_player(Some(Seat(0)), 0.0);
    install_states(&world, &[pawn]);
    enqueue_respawns(
        &world.script_ctx,
        &world.script_ctx.registry.borrow(),
        &[pawn],
        None,
        Some("down"),
    );
    let mut descriptor = world.script_ctx.data_registry.borrow().entities[0].clone();
    let old_id = descriptor
        .life_states
        .as_ref()
        .unwrap()
        .state("down")
        .unwrap()
        .0;
    descriptor
        .life_states
        .as_mut()
        .unwrap()
        .states
        .insert("aaa".into(), LifeStateProfile::default());
    world
        .script_ctx
        .data_registry
        .borrow_mut()
        .replace_entity_types(vec![descriptor]);
    let mut descriptor = world.script_ctx.data_registry.borrow().entities[0].clone();
    assert_ne!(
        descriptor
            .life_states
            .as_ref()
            .unwrap()
            .state("down")
            .unwrap()
            .0,
        old_id
    );
    assert_eq!(
        world.script_ctx.pending_respawns.borrow()[&pawn]
            .state
            .as_deref(),
        Some("down")
    );
    descriptor
        .life_states
        .as_mut()
        .unwrap()
        .states
        .remove("down");
    world
        .script_ctx
        .data_registry
        .borrow_mut()
        .replace_entity_types(vec![descriptor]);
    assert_eq!(
        world.script_ctx.pending_respawns.borrow()[&pawn]
            .state
            .as_deref(),
        Some("down")
    );
}

#[test]
fn respawn_wait_tail_uses_captured_context_and_both_runtime_registries() {
    use crate::scripting_systems::reaction_scheduler::{
        ReactionScheduler, register_reaction_control_primitives,
    };
    use postretro_entities::{GroupKind, GroupTarget};
    use postretro_scripting_core::reaction_dispatch::fire_named_event_with_sequences;
    use postretro_scripting_core::reaction_registry::SystemReactionRegistry;
    let mut world = World::new();
    let first = world.spawn_player(Some(Seat(0)), 0.0);
    let second = world.spawn_player(Some(Seat(1)), 0.0);
    install_states(&world, &[first, second]);
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
                    primitive: "respawn".into(),
                    args: json!({"health":25,"state":"down"}),
                },
            ]),
        }],
        vec![],
    );
    let scheduler = ReactionScheduler::default();
    scheduler.set_enabled(true);
    let mut sequences = SequencedPrimitiveRegistry::new();
    register_reaction_control_primitives(&mut sequences, scheduler.clone());
    register_sequenced_respawn_primitives(&mut sequences, world.script_ctx.clone());
    let mut reactions = ReactionPrimitiveRegistry::new();
    register_respawn_reaction_primitives(&mut reactions, world.script_ctx.clone());
    let systems = SystemReactionRegistry::new();
    let _ = fire_named_event_with_sequences(
        "late",
        &world.script_ctx.data_registry.borrow(),
        &sequences,
        &reactions,
        &systems,
        &world.script_ctx,
        None,
    );
    assert_eq!(scheduler.pending_len(), 1);
    assert!(world.script_ctx.pending_respawns.borrow().is_empty());
    scheduler.begin_frame();
    scheduler.evaluate(&[]);
    scheduler.drain_landings(
        &world.script_ctx.data_registry.borrow(),
        &sequences,
        &reactions,
        &systems,
        &world.script_ctx,
    );
    assert_eq!(world.script_ctx.pending_respawns.borrow().len(), 2);
    for pawn in [first, second] {
        let pending = world.script_ctx.pending_respawns.borrow();
        assert_eq!(pending[&pawn].health, Some(25.0));
        assert_eq!(pending[&pawn].state.as_deref(), Some("down"));
    }
    // The direct entity sequence arm must reach the same captured queue too.
    sequences.get("respawn").unwrap()(first, &json!({"health":1e40})).unwrap();
    let pending = world.script_ctx.pending_respawns.borrow();
    assert!(pending[&first].health.unwrap().is_infinite());
    assert_eq!(pending[&first].state, None);
    assert_eq!(pending[&second].health, Some(25.0));
    assert_eq!(world.health(first), 0.0);
    assert_eq!(world.health(second), 0.0);
}
