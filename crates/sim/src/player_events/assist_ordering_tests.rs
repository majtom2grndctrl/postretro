//! The assist sources against the real accumulator: completions recorded in
//! the fixed tick reach that tick's player-event pass (O1), a fire list's
//! store write reaches the next tick's pair evaluation (O41), and a target
//! revived in the pass reads no received progress afterwards (O50's host
//! half).
//! See: context/lib/scripting.md §12 (Player events, Assist)

use glam::Vec3;
use postretro_entities::{
    FactionRegistry, ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    Transform,
};
use postretro_foundation::{Assistable, NumberOrIr, UNDECLARED_LIFE_STATE};
use postretro_scripting_core::data_descriptors::{
    NamedReaction, PrimitiveDescriptor, ReactionDescriptor,
};
use postretro_scripting_core::player_slots::{PlayerSlot, player_slot_value};
use postretro_scripting_core::reaction_dispatch::ProgressTracker;
use serde_json::json;

use super::assist_sources_tests::{
    enter, install_assist_states, literal, movement, received, run_pass, state_id,
};
use super::tests::{World, input, number, on_player, player_event};
use super::*;
use crate::ai_host::AiTickResult;
use crate::collision::CollisionWorld;
use crate::kinematic_mover::MoverTickStateTable;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::sim::assist::{AssistCompletion, AssistState, increment, required_count};
use crate::sim::touch::TouchSystem;
use crate::sim::{RemoteEquipGate, RemotePawnCommand, SimCommand, TickEvents};

fn command(assist_held: bool) -> SimCommand {
    SimCommand {
        assist_held,
        input_tick: 0,
        movement: crate::movement::MovementInput {
            wish_dir: glam::Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            facing_pitch: 0.0,
            use_pressed: false,
            drop_pressed: false,
        },
        fire_button: crate::weapon::FireButtonState {
            pressed: false,
            active: false,
        },
        secondary_button: crate::weapon::FireButtonState {
            pressed: false,
            active: false,
        },
        activation: Default::default(),
        reload: false,
        firing_slot: 0,
        select_slot: None,
        use_pressed: false,
        drop_pressed: false,
    }
}

fn remote(pawn: EntityId, assist_held: bool) -> RemotePawnCommand {
    RemotePawnCommand {
        reload_press_tick: None,
        equip: RemoteEquipGate::default(),
        real_command: true,
        rejected_activation: None,
        pawn,
        owner_client_id: 2,
        weapon: None,
        shot_id: None,
        fire_tick: 0,
        client_tick: 0,
        aim_pitch: 0.0,
        command: command(assist_held),
        start_aim: None,
    }
}

/// One real fixed tick with the local pawn's `assist` level, the accumulator
/// running in it after the death sweep.
fn simulate(world: &World, state: &mut AssistState, assist_held: bool) -> TickEvents {
    let edges = std::collections::HashMap::new();
    crate::sim::simulate_tick_with_presentation_aim(
        world.script_ctx.registry.clone(),
        &CollisionWorld::new(),
        &HitZoneStore::new(),
        None,
        postretro_collision::EnvironmentView::level_only(0.0),
        false,
        0.0,
        (0.0, 0.0),
        &mut ProgressTracker::new(),
        |_, _, _, _| AiTickResult {
            events: Vec::new(),
            projectile_spawns: Vec::new(),
        },
        &[],
        &mut MoverTickStateTable::default(),
        &[],
        &command(assist_held),
        |_| crate::sim::PostMovementCommand {
            aim_origin: Vec3::ZERO,
            aim_direction: Vec3::NEG_Z,
        },
        1.0 / 60.0,
        &mut TouchSystem::default(),
        state,
        &[],
        0,
        &FactionRegistry::default(),
        &world.script_ctx.faction_sentiment,
        None,
        &edges,
        &edges,
        None,
        |_, _| {},
        |_| {},
        crate::sim::TimingGate::OFF,
    )
}

/// The accumulator alone, as the fixed tick runs it: every pawn in a declared
/// state advances its elapsed (the movement stage's clock), then the pass
/// runs with the local pawn's level and the remote commands.
fn accumulate(
    world: &World,
    state: &mut AssistState,
    pawns: &[EntityId],
    local_held: bool,
    remotes: &[RemotePawnCommand],
) -> Vec<AssistCompletion> {
    let mut registry = world.script_ctx.registry.borrow_mut();
    for &pawn in pawns {
        let mut component = registry
            .get_component::<postretro_foundation::PlayerMovementComponent>(pawn)
            .unwrap()
            .clone();
        if component.life_state != UNDECLARED_LIFE_STATE {
            component.life_state_elapsed_ticks += 1;
        }
        registry.set_component(pawn, component).unwrap();
    }
    state.run_host_pass(
        &mut registry,
        Some(&world.script_ctx.slot_table.borrow()),
        &command(local_held),
        remotes,
    )
}

fn place(world: &World, pawn: EntityId, x: f32) {
    let mut registry = world.script_ctx.registry.borrow_mut();
    let mut transform = *registry.get_component::<Transform>(pawn).unwrap();
    transform.position = Vec3::new(x, 0.0, 0.0);
    registry.set_component(pawn, transform).unwrap();
}

fn mark_local(world: &World, pawn: EntityId) {
    world
        .script_ctx
        .registry
        .borrow_mut()
        .mark_local_player_pawn(pawn)
        .unwrap();
}

/// `inLifeState("down")` and `lifeStateMs >= 30`: the bleed-out a revive must
/// beat. Two ticks in `down` reach it.
fn bleed_out_expires() -> IrNode {
    IrNode::And {
        a: Box::new(input("player.inLifeState:down")),
        b: Box::new(IrNode::Ge {
            a: Box::new(input("player.lifeStateMs")),
            b: Box::new(number(30.0)),
        }),
    }
}

/// O1: the assist completes in the tick bleed-out would expire, and that tick
/// is the first of several run in one frame with no frame-end drain between
/// them. The revive lands in the first tick's own pass, before its snapshot,
/// so the bleed-out condition reads `down` false and never fires. Without the
/// hold the same tick bleeds out.
#[test]
fn assist_ordering_o1_revive_lands_in_the_first_ticks_pass_before_bleed_out() {
    for hold in [true, false] {
        let mut world = World::new();
        let helper = world.spawn_player(Some(Seat(0)), 100.0);
        let target = world.spawn_player(Some(Seat(1)), 100.0);
        mark_local(&world, helper);
        install_assist_states(&world, &[helper, target], literal(1.5, 0.0));
        world.install(
            vec![
                on_player("revive", "setLifeState", json!({ "state": "alive" })),
                on_player("bleedOut", "setLifeState", json!({ "state": "blocked" })),
            ],
            vec![
                received(&["revive"]),
                player_event(PlayerEventEdge::Becomes, bleed_out_expires(), &["bleedOut"]),
            ],
        );
        let mut state = AssistState::default();
        state.rebind(&world.script_ctx);
        enter(&world, target, "down");

        // Frame 1: one tick, not yet holding; 17 ms in `down`.
        let events = simulate(&world, &mut state, false);
        assert!(events.assist_completions.is_empty());
        run_pass(&mut world, &events.assist_completions);
        assert_eq!(
            movement(&world, target).life_state,
            state_id(&world, "down")
        );

        // Frame 2: three ticks, no frame-end drain between them.
        for tick in 0..3 {
            let events = simulate(&world, &mut state, hold);
            assert_eq!(
                events.assist_completions.len(),
                usize::from(hold && tick == 0),
                "hold {hold}, tick {tick}: one completion, in the first tick"
            );
            run_pass(&mut world, &events.assist_completions);
            if tick == 0 {
                let expected = if hold { "alive" } else { "blocked" };
                assert_eq!(
                    movement(&world, target).life_state,
                    state_id(&world, expected),
                    "hold {hold}: decided in the first tick's own pass"
                );
            }
        }
        let expected = if hold { "alive" } else { "blocked" };
        assert_eq!(
            movement(&world, target).life_state,
            state_id(&world, expected)
        );
    }
}

fn declare_world_radius(world: &World, radius: f32) {
    world
        .script_ctx
        .slot_table
        .borrow_mut()
        .insert_namespace(
            "world",
            vec![(
                "radius".into(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(radius)),
                    range: None,
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: ReplicationScope::SharedGlobal,
                    per_owner: false,
                    accumulate: None,
                }),
            )],
        )
        .unwrap();
}

/// holdMs 100 is six ticks.
const SIX: u32 = 6;

/// O41: helper H1's completion fire list writes the global store value the
/// `radius` expression of H2's pair on T2 reads. H2's pass in that tick used
/// the value before the write; the next tick's pass evaluates with the
/// written one, keeping H2's progress while T2 stays in range and resetting
/// it (row 4) when the new radius puts T2 out of range.
#[test]
fn assist_ordering_o41_fire_list_store_write_is_read_by_the_next_ticks_pair_pass() {
    for (written_radius, stays_in_range) in [(3.5, true), (2.0, false)] {
        let mut world = World::new();
        let first_helper = world.spawn_player(Some(Seat(0)), 100.0);
        let first_target = world.spawn_player(Some(Seat(1)), 100.0);
        let second_helper = world.spawn_player(Some(Seat(2)), 100.0);
        let second_target = world.spawn_player(Some(Seat(3)), 100.0);
        mark_local(&world, first_helper);
        let pawns = [first_helper, first_target, second_helper, second_target];
        declare_world_radius(&world, 4.0);
        install_assist_states(
            &world,
            &pawns,
            Assistable {
                radius: NumberOrIr::Ir(input("world.radius")),
                hold_ms: NumberOrIr::Literal(100.0),
            },
        );
        for (pawn, x) in pawns.into_iter().zip([0.0, 1.0, 10.0, 13.0]) {
            place(&world, pawn, x);
        }
        world.install(
            vec![
                on_player("revive", "setLifeState", json!({ "state": "alive" })),
                NamedReaction {
                    name: "shrink".into(),
                    descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                        primitive: "setState".into(),
                        target: None,
                        kind: None,
                        tag: None,
                        on_complete: None,
                        args: json!({ "slot": "world.radius", "value": written_radius }),
                    }),
                },
            ],
            vec![received(&["revive", "shrink"])],
        );
        let mut state = AssistState::default();
        state.rebind(&world.script_ctx);
        enter(&world, first_target, "down");
        enter(&world, second_target, "down");
        assert_eq!(required_count(100.0), SIX);

        // H1 holds from tick 1, H2 from tick 3; H1 completes at tick 6.
        for tick in 1..=SIX {
            let completions = accumulate(
                &world,
                &mut state,
                &pawns,
                true,
                &[remote(second_helper, tick >= 3)],
            );
            assert_eq!(completions.len(), usize::from(tick == SIX), "tick {tick}");
            run_pass(&mut world, &completions);
        }
        let read = |state: &AssistState| {
            state
                .pair_values(second_helper, second_target)
                .expect("the pass evaluated H2's pair")
                .radius
        };
        assert_eq!(
            read(&state),
            4.0,
            "tick N's pass used the value before the write"
        );
        assert_eq!(
            state.helper(second_helper).unwrap().progress.progress,
            4 * increment(SIX)
        );

        accumulate(
            &world,
            &mut state,
            &pawns,
            true,
            &[remote(second_helper, true)],
        );
        assert_eq!(read(&state), written_radius, "tick N+1 reads the write");
        let expected = if stays_in_range {
            5 * increment(SIX)
        } else {
            0
        };
        assert_eq!(
            state.helper(second_helper).unwrap().progress.progress,
            expected,
            "radius {written_radius}"
        );
    }
}

/// O50 (host half): A completes on T while B still holds on T; the pass
/// revives T. From that pass on, T's `player.assistReceivedProgress` reads 0,
/// though the accumulator wrote B's progress into the field before the pass
/// and B's own progress resets only at the next pass.
#[test]
fn assist_ordering_o50_target_revived_in_the_pass_reads_no_received_progress() {
    let mut world = World::new();
    let first_helper = world.spawn_player(Some(Seat(0)), 100.0);
    let target = world.spawn_player(Some(Seat(1)), 100.0);
    let second_helper = world.spawn_player(Some(Seat(2)), 100.0);
    mark_local(&world, first_helper);
    let pawns = [first_helper, target, second_helper];
    install_assist_states(&world, &pawns, literal(1.5, 100.0));
    for (pawn, x) in [(first_helper, 0.0), (target, 1.0), (second_helper, 2.0)] {
        place(&world, pawn, x);
    }
    world.install(
        vec![on_player(
            "revive",
            "setLifeState",
            json!({ "state": "alive" }),
        )],
        vec![received(&["revive"])],
    );
    let mut state = AssistState::default();
    state.rebind(&world.script_ctx);
    enter(&world, target, "down");

    let received_progress = |world: &World| {
        player_slot_value(
            &world.script_ctx.registry.borrow(),
            PlayerSlot::AssistReceivedProgress,
            target,
        )
    };
    for tick in 1..=SIX {
        let completions = accumulate(
            &world,
            &mut state,
            &pawns,
            true,
            &[remote(second_helper, tick >= 3)],
        );
        if tick == SIX {
            assert_eq!(completions.len(), 1);
            assert_eq!(completions[0].helper, first_helper);
            assert!(
                movement(&world, target).assist_received_progress > 0.0,
                "the accumulator wrote B's progress before the pass"
            );
            assert!(matches!(
                received_progress(&world),
                Some(SlotValue::Number(value)) if value > 0.0
            ));
        }
        run_pass(&mut world, &completions);
    }
    assert_eq!(
        movement(&world, target).life_state,
        state_id(&world, "alive")
    );
    assert_eq!(
        received_progress(&world),
        Some(SlotValue::Number(0.0)),
        "a target revived in the pass reads 0 from that pass on"
    );
    assert_eq!(
        state.helper(second_helper).unwrap().progress.progress,
        4 * increment(SIX),
        "B's progress resets only at the next pass"
    );
    accumulate(
        &world,
        &mut state,
        &pawns,
        false,
        &[remote(second_helper, true)],
    );
    assert_eq!(state.helper(second_helper).unwrap().progress.progress, 0);
    assert_eq!(received_progress(&world), Some(SlotValue::Number(0.0)));
}
