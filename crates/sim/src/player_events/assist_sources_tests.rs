//! The discrete `assistReceived` / `assistGiven` sources at the head of the
//! player-event pass: order, `on.player` binding, the per-completion re-check
//! and install-time dedupe. Completions are recorded from the live pawns and
//! the state changed directly between record and apply, standing in for the
//! remote-hit flush that runs between the accumulator and the pass.
//! See: context/lib/scripting.md §12 (Player events, Assist)

use postretro_foundation::{Assistable, LifeStateProfile, NumberOrIr, PlayerMovementComponent};
use postretro_scripting_core::data_descriptors::{PlayerEventDescriptor, PlayerEventTrigger};
use postretro_test_log_capture::LogCapture;
use serde_json::json;

use super::tests::{World, input, on_player, play_sound, player_event};
use super::*;
use crate::life_state_commands::set_life_state;
use crate::sim::assist::AssistCompletion;

pub(super) fn literal(radius: f32, hold_ms: f32) -> Assistable {
    Assistable {
        radius: NumberOrIr::Literal(radius),
        hold_ms: NumberOrIr::Literal(hold_ms),
    }
}

pub(super) fn discrete(trigger: PlayerEventTrigger, fire: &[&str]) -> PlayerEventDescriptor {
    PlayerEventDescriptor {
        trigger,
        fire: fire.iter().map(|name| name.to_string()).collect(),
        levels: Vec::new(),
        authored_index: 0,
    }
}

pub(super) fn received(fire: &[&str]) -> PlayerEventDescriptor {
    discrete(PlayerEventTrigger::AssistReceived, fire)
}

pub(super) fn given(fire: &[&str]) -> PlayerEventDescriptor {
    discrete(PlayerEventTrigger::AssistGiven, fire)
}

/// Spec A's `alive` / `blocked` / `down` states, with `down` refusing `use`
/// and declaring `down_assist`, plus `wounded`, which permits `use` and is
/// assistable. Every pawn enters `alive`.
pub(super) fn install_assist_states(world: &World, pawns: &[EntityId], down_assist: Assistable) {
    crate::life_state_commands::tests::install_states(world, pawns);
    {
        let mut data = world.script_ctx.data_registry.borrow_mut();
        let mut descriptor = data.entities[0].clone();
        let block = descriptor
            .life_states
            .as_mut()
            .expect("install_states declares life states");
        let down = block.states.get_mut("down").expect("`down` is declared");
        down.use_enabled = false;
        down.assistable = Some(down_assist);
        block.states.insert(
            "wounded".into(),
            LifeStateProfile {
                assistable: Some(literal(1.5, 0.0)),
                ..Default::default()
            },
        );
        data.replace_entity_types(vec![descriptor]);
    }
    for &pawn in pawns {
        enter(world, pawn, "alive");
    }
}

pub(super) fn enter(world: &World, pawn: EntityId, state: &str) {
    set_life_state(
        &world.script_ctx,
        &mut world.script_ctx.registry.borrow_mut(),
        &[pawn],
        state,
    );
}

pub(super) fn movement(world: &World, pawn: EntityId) -> PlayerMovementComponent {
    world
        .script_ctx
        .registry
        .borrow()
        .get_component::<PlayerMovementComponent>(pawn)
        .expect("pawn has movement")
        .clone()
}

pub(super) fn edit(
    world: &World,
    pawn: EntityId,
    change: impl FnOnce(&mut PlayerMovementComponent),
) {
    let mut component = movement(world, pawn);
    change(&mut component);
    world
        .script_ctx
        .registry
        .borrow_mut()
        .set_component(pawn, component)
        .unwrap();
}

pub(super) fn state_id(world: &World, name: &str) -> u8 {
    world.script_ctx.data_registry.borrow().entities[0]
        .life_states
        .as_ref()
        .and_then(|block| block.state(name))
        .map(|(id, _)| id)
        .expect("state is declared")
}

/// The completion the accumulator would record now for `helper` on `target`.
pub(super) fn record(world: &World, helper: EntityId, target: EntityId) -> AssistCompletion {
    let target_movement = movement(world, target);
    AssistCompletion {
        helper,
        target,
        life_state: target_movement.life_state,
        life_state_entry: target_movement.life_state_entry,
    }
}

pub(super) fn run_pass(world: &mut World, completions: &[AssistCompletion]) {
    world
        .table
        .run_tick(&world.script_ctx, completions, &mut world.residuals);
}

/// This tick's frame-end residuals as (reaction, pawn), in drain order, then
/// clear them.
pub(super) fn take_fires(world: &mut World) -> Vec<(String, EntityId)> {
    let fires = world
        .residuals
        .iter()
        .flat_map(|residual| {
            world
                .table
                .residual(residual.handle)
                .expect("residual handle resolves")
                .iter()
                .map(move |step| match step {
                    PrepartitionedReactionStep::Descriptor(name, _, _) => {
                        (name.clone(), residual.pawn)
                    }
                    PrepartitionedReactionStep::DeferredEvent(name) => {
                        (format!("->{name}"), residual.pawn)
                    }
                })
        })
        .collect();
    world.residuals.clear();
    fires
}

fn fired(pairs: &[(&str, EntityId)]) -> Vec<(String, EntityId)> {
    pairs
        .iter()
        .map(|(name, pawn)| (name.to_string(), *pawn))
        .collect()
}

/// Helper seat 0, target seat 1 in `down`, both at 50 health.
fn helper_and_downed_target() -> (World, EntityId, EntityId) {
    let world = World::new();
    let helper = world.spawn_player(Some(Seat(0)), 50.0);
    let target = world.spawn_player(Some(Seat(1)), 50.0);
    install_assist_states(&world, &[helper, target], literal(1.5, 0.0));
    enter(&world, target, "down");
    (world, helper, target)
}

/// AC4: an applied completion fires every `assistReceived` subscription with
/// `on.player` bound to the target, then every `assistGiven` subscription
/// with it bound to the helper, each in composed order (mod-global entries
/// before the level's), whatever the authored interleaving.
#[test]
fn assist_source_ac4_received_then_given_in_composed_order_on_target_then_helper() {
    let (mut world, helper, target) = helper_and_downed_target();
    world.install_composed(
        vec![
            play_sound("given.global", "a"),
            play_sound("received.global", "b"),
            play_sound("received.level", "c"),
            play_sound("given.level", "d"),
            on_player("heal", "grantHealth", json!({ "amount": 10.0 })),
            on_player("bonus", "grantHealth", json!({ "amount": 5.0 })),
        ],
        vec![
            given(&["given.global", "bonus"]),
            received(&["received.global", "heal"]),
        ],
        vec![received(&["received.level"]), given(&["given.level"])],
        &[],
    );
    let completion = record(&world, helper, target);
    run_pass(&mut world, &[completion]);
    assert_eq!(
        take_fires(&mut world),
        fired(&[
            ("received.global", target),
            ("received.level", target),
            ("given.global", helper),
            ("given.level", helper),
        ])
    );
    assert_eq!(world.health(target), 60.0, "`on.player` is the target");
    assert_eq!(world.health(helper), 55.0, "`on.player` is the helper");

    run_pass(&mut world, &[]);
    assert!(
        take_fires(&mut world).is_empty(),
        "a discrete source fires only from a completion"
    );
}

/// AC4 / I3: the completion itself changes no life state or health; only the
/// authored fire lists do. With no subscription, an applied completion leaves
/// the target exactly as it was.
#[test]
fn assist_source_ac4_completion_itself_changes_no_life_state_or_health() {
    let (mut world, helper, target) = helper_and_downed_target();
    world.install(
        vec![play_sound("cue", "ding")],
        vec![player_event(
            PlayerEventEdge::Becomes,
            input("player.inLifeState:wounded"),
            &["cue"],
        )],
    );
    let before = movement(&world, target);
    let completion = record(&world, helper, target);
    run_pass(&mut world, &[completion]);
    let after = movement(&world, target);
    assert_eq!(after.life_state, before.life_state);
    assert_eq!(
        after.life_state_elapsed_ticks,
        before.life_state_elapsed_ticks
    );
    assert_eq!(world.health(target), 50.0);
    assert_eq!(world.health(helper), 50.0);
    assert!(take_fires(&mut world).is_empty());
}

/// O23: completions on different targets apply in `players()` order of the
/// target, whatever order the accumulator recorded them in (helper order).
#[test]
fn assist_source_o23_completions_apply_in_target_order() {
    let mut world = World::new();
    let first_helper = world.spawn_player(Some(Seat(0)), 50.0);
    let first_target = world.spawn_player(Some(Seat(1)), 50.0);
    let second_helper = world.spawn_player(Some(Seat(2)), 50.0);
    let second_target = world.spawn_player(Some(Seat(3)), 50.0);
    let pawns = [first_helper, first_target, second_helper, second_target];
    install_assist_states(&world, &pawns, literal(1.5, 0.0));
    enter(&world, first_target, "down");
    enter(&world, second_target, "down");
    world.install(
        vec![play_sound("received", "a"), play_sound("given", "b")],
        vec![received(&["received"]), given(&["given"])],
    );
    // Helper order: the seat-0 helper reached the seat-3 target.
    let completions = [
        record(&world, first_helper, second_target),
        record(&world, second_helper, first_target),
    ];
    run_pass(&mut world, &completions);
    assert_eq!(
        take_fires(&mut world),
        fired(&[
            ("received", first_target),
            ("given", second_helper),
            ("received", second_target),
            ("given", first_helper),
        ])
    );
}

/// O23 / I1: each completion is re-checked just before its own application,
/// after every earlier one's fire lists ran. C1's `assistGiven` list stands
/// its helper up, and that helper is C2's target, so C2 drops.
#[test]
fn assist_source_o23_each_completion_is_rechecked_after_the_earlier_ones_fire() {
    for earlier_applies in [true, false] {
        let mut world = World::new();
        let medic = world.spawn_player(Some(Seat(0)), 50.0);
        let downed = world.spawn_player(Some(Seat(1)), 50.0);
        let wounded = world.spawn_player(Some(Seat(2)), 50.0);
        install_assist_states(&world, &[medic, downed, wounded], literal(1.5, 0.0));
        enter(&world, downed, "down");
        enter(&world, wounded, "wounded");
        world.install(
            vec![
                play_sound("received", "a"),
                play_sound("given", "b"),
                on_player("stand", "setLifeState", json!({ "state": "alive" })),
            ],
            vec![received(&["received"]), given(&["given", "stand"])],
        );
        // C2 (medic → wounded) was recorded first, in helper order; C1
        // (wounded → downed) applies first, in target order.
        let mut completions = vec![record(&world, medic, wounded)];
        if earlier_applies {
            completions.push(record(&world, wounded, downed));
        }
        run_pass(&mut world, &completions);
        let expected = if earlier_applies {
            fired(&[("received", downed), ("given", wounded)])
        } else {
            fired(&[("received", wounded), ("given", medic)])
        };
        assert_eq!(
            take_fires(&mut world),
            expected,
            "earlier completion applied: {earlier_applies}"
        );
    }
}

/// O24: with no completions the pass is the edge pass alone; a table with
/// discrete subscriptions fires only its edges.
#[test]
fn assist_source_o24_no_completions_runs_the_edge_pass_unchanged() {
    let (mut world, helper, target) = helper_and_downed_target();
    world.install(
        vec![
            play_sound("downed", "a"),
            play_sound("received", "b"),
            play_sound("given", "c"),
        ],
        vec![
            received(&["received"]),
            player_event(
                PlayerEventEdge::Becomes,
                input("player.inLifeState:down"),
                &["downed"],
            ),
            given(&["given"]),
        ],
    );
    run_pass(&mut world, &[]);
    assert_eq!(take_fires(&mut world), fired(&[("downed", target)]));
    run_pass(&mut world, &[]);
    assert!(take_fires(&mut world).is_empty());
    assert_eq!(world.health(helper), 50.0);
}

/// O33: a completion whose target left its recorded state before the pass is
/// dropped, even into another assistable state; so is one whose state no
/// longer declares `assistable` (a reload restamp) or whose pawn is gone. No
/// assist source fires for it.
#[test]
fn assist_source_o33_recheck_drops_a_completion_whose_target_changed() {
    for case in [
        "another assistable state",
        "unassistable state",
        "restamp",
        "despawned",
    ] {
        let (mut world, helper, target) = helper_and_downed_target();
        world.install(
            vec![play_sound("received", "a"), play_sound("given", "b")],
            vec![received(&["received"]), given(&["given"])],
        );
        let completion = record(&world, helper, target);
        match case {
            "another assistable state" => enter(&world, target, "wounded"),
            "unassistable state" => enter(&world, target, "alive"),
            "restamp" => edit(&world, target, |movement| movement.assistable = false),
            _ => {
                world
                    .script_ctx
                    .registry
                    .borrow_mut()
                    .despawn(target)
                    .unwrap();
            }
        }
        run_pass(&mut world, &[completion]);
        assert!(take_fires(&mut world).is_empty(), "{case}: no source fires");
    }
}

/// O34: a leave and re-entry of the recorded state between record and apply
/// moves the target's entry count on and drops the completion, whatever its
/// elapsed at record: one recorded at elapsed zero, on a target that entered
/// its state that tick, drops too. Without the re-entry it applies.
#[test]
fn assist_source_o34_leave_and_reentry_drops_even_at_zero_elapsed() {
    for (recorded_elapsed, reenters) in [(3, true), (0, true), (0, false)] {
        let (mut world, helper, target) = helper_and_downed_target();
        world.install(
            vec![play_sound("received", "a")],
            vec![received(&["received"])],
        );
        edit(&world, target, |movement| {
            movement.life_state_elapsed_ticks = recorded_elapsed;
        });
        let completion = record(&world, helper, target);
        if reenters {
            enter(&world, target, "alive");
            enter(&world, target, "down");
        }
        let now = movement(&world, target);
        assert_eq!(now.life_state, state_id(&world, "down"));
        assert_eq!(
            now.life_state_elapsed_ticks,
            if reenters { 0 } else { recorded_elapsed }
        );
        run_pass(&mut world, &[completion]);
        let fires = take_fires(&mut world);
        if reenters {
            assert!(
                fires.is_empty(),
                "elapsed {recorded_elapsed}: the re-entry drops the completion"
            );
        } else {
            assert_eq!(fires, fired(&[("received", target)]));
        }
    }
}

/// O38: the helper is not re-checked. A helper downed after the record still
/// earns its `assistGiven`, and its down edge then fires in the same pass.
#[test]
fn assist_source_o38_helper_downed_after_the_record_still_earns_given() {
    let (mut world, helper, target) = helper_and_downed_target();
    world.install(
        vec![
            on_player("revive", "setLifeState", json!({ "state": "alive" })),
            play_sound("received", "a"),
            play_sound("given", "b"),
            play_sound("downed", "c"),
        ],
        vec![
            received(&["revive", "received"]),
            given(&["given"]),
            player_event(
                PlayerEventEdge::Becomes,
                input("player.inLifeState:down"),
                &["downed"],
            ),
        ],
    );
    run_pass(&mut world, &[]);
    assert_eq!(take_fires(&mut world), fired(&[("downed", target)]));

    let completion = record(&world, helper, target);
    enter(&world, helper, "down");
    run_pass(&mut world, &[completion]);
    assert_eq!(
        take_fires(&mut world),
        fired(&[("received", target), ("given", helper), ("downed", helper)])
    );
    assert_eq!(
        movement(&world, target).life_state,
        state_id(&world, "alive")
    );
    assert_eq!(
        movement(&world, helper).life_state,
        state_id(&world, "down")
    );
}

/// Dedupe for discrete entries keys on (kind, address): one reaction bound
/// twice under `assistReceived()` binds once with one warning naming both
/// entries; the same address under `assistGiven()` is a different key.
#[test]
fn assist_source_duplicate_binding_binds_once_with_one_warning() {
    let (mut world, helper, target) = helper_and_downed_target();
    let capture = LogCapture::start();
    world.install_composed(
        vec![play_sound("cue", "a")],
        vec![received(&["cue"])],
        vec![
            PlayerEventDescriptor {
                authored_index: 1,
                ..received(&["cue"])
            },
            given(&["cue"]),
        ],
        &[],
    );
    let warnings: Vec<String> = capture
        .records()
        .into_iter()
        .filter(|record| record.message.contains("is bound by both"))
        .map(|record| record.message)
        .collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("ModManifest.playerEvents[0]")
            && warnings[0].contains("setupLevel().playerEvents[1]")
            && warnings[0].contains("`assistReceived()`"),
        "{}",
        warnings[0]
    );
    let completion = record(&world, helper, target);
    run_pass(&mut world, &[completion]);
    assert_eq!(
        take_fires(&mut world),
        fired(&[("cue", target), ("cue", helper)])
    );
}

/// Discrete entries keep no edge memory, and a recompose that keeps them
/// beside an edge entry still carries that edge's memory.
#[test]
fn assist_source_discrete_entries_keep_no_edge_memory_across_a_recompose() {
    let (mut world, _, target) = helper_and_downed_target();
    let events = vec![
        received(&["cue"]),
        player_event(
            PlayerEventEdge::Becomes,
            input("player.inLifeState:down"),
            &["cue"],
        ),
        given(&["cue"]),
    ];
    world.install(vec![play_sound("cue", "a")], events);
    run_pass(&mut world, &[]);
    assert_eq!(take_fires(&mut world), fired(&[("cue", target)]));
    assert_eq!(
        world.table.edge_memory_len(),
        2,
        "one entry per observed player"
    );

    let rebuilt = PlayerEventTable::build(
        &world.script_ctx,
        crate::mover_commands::MoverCommandDiagnostics::default(),
        crate::spawner::SpawnContext::default(),
        Some(&world.table),
    );
    world.table = rebuilt;
    assert_eq!(world.table.edge_memory_len(), 2);
    run_pass(&mut world, &[]);
    assert!(
        take_fires(&mut world).is_empty(),
        "the kept edge memory re-fires nothing"
    );
}
