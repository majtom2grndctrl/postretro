// What a player event's fire list may hold, and what `on.player` and
// `byPlayer(on.player)` read and target when it fires.
// See: context/lib/scripting.md §12 (Player events)

use postretro_entities::{
    ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
};
use postretro_foundation::{IrNode, Seat};
use postretro_scripting_core::data_descriptors::{
    NamedReaction, PlayerEventEdge, PrimitiveDescriptor, ReactionDescriptor, SequenceStep,
    SequenceTarget,
};
use postretro_test_log_capture::LogCapture;
use serde_json::json;

use super::tests::{World, input, lt, number, on_player, play_sound, player_event};

const LAST_HP: &str = "leveling.lastHp";
const LOCAL_NOTE: &str = "hud.note";

fn declare(world: &World, name: &str, network: ReplicationScope, per_owner: bool) {
    world
        .script_ctx
        .slot_table
        .borrow_mut()
        .insert(
            name.to_string(),
            SlotRecord::new(SlotSchema {
                slot_type: SlotType::Number,
                default: Some(SlotValue::Number(0.0)),
                range: None,
                persist: false,
                readonly: false,
                ownership: SlotOwnership::Mod,
                network,
                per_owner,
                accumulate: None,
            }),
        )
        .unwrap();
}

fn global(world: &World, name: &str) -> f32 {
    match world
        .script_ctx
        .slot_table
        .borrow()
        .get(name)
        .unwrap()
        .value
    {
        Some(SlotValue::Number(value)) => value,
        ref other => panic!("{name} is {other:?}"),
    }
}

fn system(name: &str, primitive: &str, args: serde_json::Value) -> NamedReaction {
    NamedReaction {
        name: name.to_string(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: primitive.to_string(),
            target: None,
            kind: None,
            tag: None,
            on_complete: None,
            args,
        }),
    }
}

fn with_on_complete(mut reaction: NamedReaction, next: &str) -> NamedReaction {
    if let ReactionDescriptor::Primitive(primitive) = &mut reaction.descriptor {
        primitive.on_complete = Some(next.to_string());
    }
    reaction
}

fn sequence(name: &str, steps: Vec<SequenceStep>) -> NamedReaction {
    NamedReaction {
        name: name.to_string(),
        descriptor: ReactionDescriptor::Sequence(steps),
    }
}

fn fire_step(event: &str) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Fire,
        primitive: "fire".to_string(),
        args: json!({ "event": event }),
    }
}

fn record_hp(owner: Option<&str>) -> NamedReaction {
    let mut value = json!({ "op": "input", "name": "player.health" });
    if let Some(owner) = owner {
        value["owner"] = json!(owner);
    }
    system(
        "recordHp",
        "setState",
        json!({ "slot": LAST_HP, "value": value }),
    )
}

fn low_health() -> IrNode {
    lt(input("player.health"), number(50.0))
}

fn errors_containing(capture: &LogCapture, needle: &str) -> Vec<String> {
    capture
        .records()
        .into_iter()
        .filter(|record| record.level == log::Level::Error && record.message.contains(needle))
        .map(|record| record.message)
        .collect()
}

#[test]
fn by_player_reads_the_event_players_value_and_a_plain_read_is_rejected() {
    let mut world = World::new();
    world.spawn_player(None, 90.0);
    let remote = world.spawn_player(Some(Seat(2)), 90.0);
    declare(&world, LAST_HP, ReplicationScope::SharedGlobal, false);
    // The host's local HUD projection says 90; the event player is at 30.
    world
        .script_ctx
        .slot_table
        .borrow_mut()
        .get_mut("player.health")
        .unwrap()
        .write_value(Some(SlotValue::Number(90.0)));
    world.install(
        vec![record_hp(Some("@player"))],
        vec![player_event(
            PlayerEventEdge::Becomes,
            low_health(),
            &["recordHp"],
        )],
    );
    world.set_health(remote, 30.0);
    world.tick();
    assert_eq!(
        global(&world, LAST_HP),
        30.0,
        "byPlayer(on.player) reads the crossing player, not the host"
    );

    let capture = LogCapture::start();
    let mut world = World::new();
    world.spawn_player(Some(Seat(2)), 30.0);
    declare(&world, LAST_HP, ReplicationScope::SharedGlobal, false);
    world.install(
        vec![record_hp(None)],
        vec![player_event(
            PlayerEventEdge::Becomes,
            low_health(),
            &["recordHp"],
        )],
    );
    world.tick();
    assert_eq!(global(&world, LAST_HP), 0.0, "the plain read never runs");
    let errors = errors_containing(&capture, "drops address `recordHp`");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("reaction `recordHp`")
            && errors[0].contains("`player.health`")
            && errors[0].contains("byPlayer(on.player)"),
        "the error names the reaction, the slot and the fix: {}",
        errors[0]
    );
}

#[test]
fn a_mixed_address_is_dropped_whole_and_its_reactions_stay_for_other_sources() {
    let mut world = World::new();
    world.spawn_player(Some(Seat(1)), 30.0);
    declare(&world, LAST_HP, ReplicationScope::SharedGlobal, false);
    let capture = LogCapture::start();
    let mut chime = play_sound("recordHp", "chime");
    chime.name = "recordHp".to_string();
    world.install(
        vec![chime, record_hp(None), play_sound("bleed", "bleed")],
        vec![player_event(
            PlayerEventEdge::Becomes,
            low_health(),
            &["recordHp", "bleed"],
        )],
    );
    world.tick();
    assert_eq!(
        world.take_residual_reactions(),
        vec!["bleed".to_string()],
        "the sound sharing the address drops with it; the next address still fires"
    );
    assert_eq!(
        errors_containing(&capture, "drops address `recordHp`").len(),
        1
    );
    assert_eq!(
        world
            .script_ctx
            .data_registry
            .borrow()
            .reactions
            .iter()
            .filter(|reaction| reaction.name == "recordHp")
            .count(),
        2,
        "rejection strips a subscription, never the reaction"
    );
}

#[test]
fn machine_local_effects_are_rejected_and_a_shared_slot_write_lands_on_the_host() {
    let mut world = World::new();
    world.spawn_player(Some(Seat(1)), 30.0);
    declare(&world, LAST_HP, ReplicationScope::SharedGlobal, false);
    declare(&world, LOCAL_NOTE, ReplicationScope::None, false);
    let capture = LogCapture::start();
    world.install(
        vec![
            system("dialog", "showDialog", json!({ "tree": "levelUp" })),
            system(
                "note",
                "setState",
                json!({ "slot": LOCAL_NOTE, "value": 1.0 }),
            ),
            system("mark", "setState", json!({ "slot": LAST_HP, "value": 7.0 })),
        ],
        vec![player_event(
            PlayerEventEdge::Becomes,
            low_health(),
            &["dialog", "note", "mark"],
        )],
    );
    world.tick();
    assert_eq!(
        global(&world, LAST_HP),
        7.0,
        "a shared slot write installs and lands in-tick"
    );
    assert!(
        !errors_containing(
            &capture,
            "drops address `dialog`: reaction `dialog` `showDialog` is machine-local"
        )
        .is_empty()
    );
    assert!(
        !errors_containing(&capture, "drops address `note`: reaction `note` `setState` writes `hud.note`, which does not replicate").is_empty()
    );
    assert!(errors_containing(&capture, "drops address `mark`").is_empty());
}

#[test]
fn context_free_routes_reaching_presentation_are_rejected_at_any_depth() {
    let mut world = World::new();
    world.spawn_player(Some(Seat(1)), 30.0);
    declare(&world, LAST_HP, ReplicationScope::SharedGlobal, false);
    let capture = LogCapture::start();
    world.install(
        vec![
            // fire → fire → a presentation reaction.
            sequence("relay", vec![fire_step("relay2")]),
            sequence("relay2", vec![fire_step("fanfare")]),
            play_sound("fanfare", "level_up"),
            // onComplete → a plain per-player read.
            with_on_complete(
                system("mark", "setState", json!({ "slot": LAST_HP, "value": 1.0 })),
                "recordHp",
            ),
            record_hp(None),
            // fire after a wait → a presentation reaction.
            sequence(
                "later",
                vec![
                    SequenceStep {
                        id: SequenceTarget::Wait,
                        primitive: "wait".to_string(),
                        args: json!({ "durationMs": 100.0 }),
                    },
                    fire_step("fanfare"),
                ],
            ),
            // fire → a clean reaction.
            sequence("clean", vec![fire_step("tally")]),
            system(
                "tally",
                "setState",
                json!({ "slot": LAST_HP, "value": 2.0 }),
            ),
        ],
        vec![player_event(
            PlayerEventEdge::Becomes,
            low_health(),
            &["relay", "mark", "later", "clean", "fanfare"],
        )],
    );
    for (address, reached) in [
        ("relay", "fanfare"),
        ("mark", "recordHp"),
        ("later", "fanfare"),
    ] {
        let errors = errors_containing(&capture, &format!("drops address `{address}`"));
        assert_eq!(errors.len(), 1, "`{address}`: {errors:?}");
        assert!(
            errors[0].contains(&format!("reaction `{address}`"))
                && errors[0].contains(&format!("reaches reaction `{reached}`")),
            "the error names both reactions: {}",
            errors[0]
        );
    }
    assert!(errors_containing(&capture, "drops address `clean`").is_empty());
    assert!(
        errors_containing(&capture, "drops address `fanfare`").is_empty(),
        "the presentation reaction listed directly installs"
    );
    world.tick();
    assert_eq!(
        world.take_residual_reactions(),
        vec!["->tally".to_string(), "fanfare".to_string()],
        "the clean route and the direct presentation both run"
    );
}

#[test]
fn an_on_player_command_whose_pawn_is_gone_before_it_applies_warn_skips_and_siblings_apply() {
    let mut world = World::new();
    let pawn = world.spawn_player(Some(Seat(1)), 30.0);
    let other = world.spawn_player(Some(Seat(2)), 30.0);
    declare(&world, LAST_HP, ReplicationScope::SharedGlobal, false);
    world.install(
        vec![
            on_player("scald", "applyDamage", json!({ "amount": 5.0 })),
            record_hp(Some("@player")),
            play_sound("hiss", "scald"),
        ],
        vec![player_event(
            PlayerEventEdge::Becomes,
            low_health(),
            &["scald", "recordHp", "hiss"],
        )],
    );
    // Evaluate, then lose the first player's pawn before the fires apply.
    {
        let registry = world.script_ctx.registry.borrow();
        let slot_table = world.script_ctx.slot_table.borrow();
        world.table.evaluate(&registry, &slot_table);
    }
    world
        .script_ctx
        .registry
        .borrow_mut()
        .despawn(pawn)
        .unwrap();
    let capture = LogCapture::start();
    world.table.apply(&world.script_ctx, &mut world.residuals);

    capture.assert_logged(log::Level::Warn, "player event target");
    capture.assert_logged(log::Level::Warn, "has no source for; skipping");
    assert_eq!(world.health(other), 25.0, "the sibling fire still applies");
    // In-tick commands apply in listed order, so the second player's read sees
    // the damage listed before it. The gone pawn's read wrote nothing.
    assert_eq!(
        global(&world, LAST_HP),
        25.0,
        "the second player's read lands; the gone pawn's is skipped, never written as 0"
    );
    assert_eq!(
        world.take_residual_reactions(),
        vec!["hiss".to_string(), "hiss".to_string()],
        "presentation still reaches both players"
    );
}

#[test]
fn an_on_player_step_before_a_wait_lands_on_the_event_player_in_tick() {
    let mut world = World::new();
    let pawn = world.spawn_player(Some(Seat(1)), 30.0);
    let bystander = world.spawn_player(Some(Seat(2)), 80.0);
    world.install(
        vec![sequence(
            "burn",
            vec![
                SequenceStep {
                    id: SequenceTarget::EventPlayer,
                    primitive: "applyDamage".to_string(),
                    args: json!({ "amount": 5.0 }),
                },
                SequenceStep {
                    id: SequenceTarget::Wait,
                    primitive: "wait".to_string(),
                    args: json!({ "durationMs": 100.0 }),
                },
            ],
        )],
        vec![player_event(
            PlayerEventEdge::Becomes,
            low_health(),
            &["burn"],
        )],
    );
    world.tick();
    assert_eq!(world.health(pawn), 25.0);
    assert_eq!(world.health(bystander), 80.0);
}

#[test]
fn a_consequence_and_two_presentations_run_each_on_its_own_path_in_listed_order() {
    let mut world = World::new();
    let pawn = world.spawn_player(Some(Seat(1)), 30.0);
    world.install(
        vec![
            on_player("credit", "grantHealth", json!({ "amount": 10.0 })),
            play_sound("fanfare", "level_up"),
            system(
                "flash",
                "flashScreen",
                json!({ "color": [1.0, 0.9, 0.3, 0.4], "durationMs": 300.0 }),
            ),
        ],
        vec![player_event(
            PlayerEventEdge::Becomes,
            low_health(),
            &["credit", "fanfare", "flash"],
        )],
    );
    world.tick();
    assert_eq!(
        world.health(pawn),
        40.0,
        "the credit applies on the event's tick"
    );
    assert_eq!(
        world.take_residual_reactions(),
        vec!["fanfare".to_string(), "flash".to_string()],
        "presentation waits for the frame drain in listed order"
    );
}

#[test]
fn a_reaction_using_on_player_is_not_bound_for_a_trigger() {
    use postretro_entities::{
        TriggerActivation, TriggerCommand, TriggerFireMode, TriggerVolumeComponent,
    };
    let mut world = World::new();
    let trigger = {
        let mut registry = world.script_ctx.registry.borrow_mut();
        let trigger = registry.spawn(postretro_entities::Transform::default());
        registry
            .set_component(
                trigger,
                TriggerVolumeComponent::new(
                    TriggerActivation::Touch,
                    String::new(),
                    "scald".to_string(),
                    String::new(),
                    TriggerCommand::Start,
                    TriggerFireMode::Multiple,
                    0.0,
                    true,
                ),
            )
            .unwrap();
        trigger
    };
    world.install(
        vec![on_player("scald", "applyDamage", json!({ "amount": 5.0 }))],
        Vec::new(),
    );
    let capture = LogCapture::start();
    let bindings = {
        let registry = world.script_ctx.registry.borrow();
        let data_registry = world.script_ctx.data_registry.borrow();
        crate::trigger_bindings::TriggerBindingTable::build_with_script_ctx(
            &registry,
            &data_registry,
            &world.script_ctx,
        )
    };
    capture.assert_logged(
        log::Level::Error,
        "reaction `scald` uses `on.player`, which only a player event publishes",
    );
    assert!(
        !bindings
            .bound_edges()
            .contains(&(trigger, crate::trigger_system::TriggerEventEdge::Enter)),
        "the trigger binds nothing for it"
    );
}

#[test]
fn a_reaction_losing_its_player_event_subscription_still_fires_under_a_crossing_for_the_local_player()
 {
    use postretro_entities::reactions::system_commands::SystemReactionCommand;
    use postretro_scripting_core::data_descriptors::build_crossing;
    use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;
    use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
    use postretro_scripting_core::state_crossings::CrossingDetector;

    use crate::scripting::reactions::dispatch_state_crossings_with_sequences;
    use crate::scripting::reactions::system_commands::{
        SystemReactionRegistry, register_system_reaction_primitives,
    };
    use crate::scripting_systems::system_reactions::{
        SystemReactionIrBindings, SystemReactionIrDispatch,
    };

    let mut world = World::new();
    world.spawn_player(None, 90.0);
    world.spawn_player(Some(Seat(2)), 20.0);
    declare(&world, LAST_HP, ReplicationScope::SharedGlobal, false);
    let set_local_hud_health = |world: &World, value: f32| {
        world
            .script_ctx
            .slot_table
            .borrow_mut()
            .get_mut("player.health")
            .unwrap()
            .write_value(Some(SlotValue::Number(value)));
    };
    set_local_hud_health(&world, 90.0);
    {
        let mut data = world.script_ctx.data_registry.borrow_mut();
        data.clear();
        data.set_level_reactions(vec![record_hp(None)]);
        data.set_level_player_events(vec![player_event(
            PlayerEventEdge::Becomes,
            low_health(),
            &["recordHp"],
        )]);
        data.set_level_crossings(vec![
            build_crossing(
                "player.health".to_string(),
                Some(50.0),
                None,
                None,
                None,
                vec!["recordHp".to_string()],
            )
            .unwrap(),
        ]);
        data.recompose(&[]);
    }
    let capture = LogCapture::start();
    world.table = super::PlayerEventTable::build(
        &world.script_ctx,
        Default::default(),
        Default::default(),
        None,
    );
    let errors = errors_containing(&capture, "drops address `recordHp`");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("still runs under every other source"));
    world.tick();
    assert_eq!(
        global(&world, LAST_HP),
        0.0,
        "the player event never runs it"
    );

    let mut detector = CrossingDetector::new();
    let mut bindings = SystemReactionIrBindings::default();
    {
        let data = world.script_ctx.data_registry.borrow();
        detector.initialize(
            &data,
            &world.script_ctx.slot_table.borrow(),
            &world.script_ctx,
        );
        bindings.rebuild(&data, &world.script_ctx);
    }
    // The local player's own health drops; the crossing reads that machine's view.
    set_local_hud_health(&world, 30.0);
    let mut system = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut system);
    dispatch_state_crossings_with_sequences(
        &mut detector,
        &world.script_ctx.slot_table.borrow(),
        &world.script_ctx.data_registry.borrow(),
        &SequencedPrimitiveRegistry::new(),
        &ReactionPrimitiveRegistry::new(),
        &system,
        &world.script_ctx,
    );
    let commands = world.script_ctx.system_commands.take();
    let [
        SystemReactionCommand::SetState {
            slot,
            value,
            dispatch_source,
            dispatch_values,
        },
    ] = commands.as_slice()
    else {
        panic!("the crossing fires recordHp once: {commands:?}");
    };
    assert_eq!(
        bindings.dispatch(
            slot,
            value,
            dispatch_source,
            dispatch_values,
            &world.script_ctx
        ),
        SystemReactionIrDispatch::Evaluated
    );
    assert_eq!(
        global(&world, LAST_HP),
        30.0,
        "the plain read means this machine's own player"
    );
}

#[test]
fn a_plain_engine_read_outside_a_condition_still_means_the_local_player() {
    use postretro_foundation::{BakedIr, CURRENT_IR_VERSION, IrValue, bind, eval_value};
    use postretro_scripting_core::ir_player_scope::PlayerConditionScope;
    use postretro_scripting_core::ir_scopes::StoreScope;

    use crate::scripting_systems::ui_proxy::PlayerHudStatePublisher;

    let world = World::new();
    world.spawn_player(None, 80.0);
    let remote = world.spawn_player(Some(Seat(2)), 40.0);
    // The host HUD publisher projects its own pawn into the slot table, which
    // HUD binds, `bindState`, local crossings and impact-policy ambient reads
    // all read.
    PlayerHudStatePublisher::new(world.script_ctx.clone())
        .publish_health_from_registry(&world.script_ctx.registry.borrow());
    let baked = BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root: input("player.health"),
    };
    let store = StoreScope::script(world.script_ctx.clone());
    let ambient = bind(&baked, &store).expect("a plain engine read binds ambiently");
    assert_eq!(eval_value(&ambient, &store), IrValue::Number(80.0));

    // Only a player-event condition binds the same read to the evaluated player.
    let condition = PlayerConditionScope::new(world.script_ctx.clone());
    let program = bind(&baked, &condition).expect("binds in a condition");
    condition.seed_player(
        &world.script_ctx.registry.borrow(),
        &world.script_ctx.slot_table.borrow(),
        remote,
        Some(Seat(2)),
    );
    assert_eq!(eval_value(&program, &condition), IrValue::Number(40.0));
}

fn record_input(name: &str, input_name: &str) -> NamedReaction {
    system(
        name,
        "setState",
        json!({ "slot": LAST_HP, "value": { "op": "input", "name": input_name } }),
    )
}

#[test]
fn a_condition_reading_a_local_only_player_slot_is_not_installed() {
    let mut world = World::new();
    let pawn = world.spawn_player(Some(Seat(2)), 30.0);
    let capture = LogCapture::start();
    world.install(
        vec![on_player("scald", "applyDamage", json!({ "amount": 5.0 }))],
        vec![player_event(
            PlayerEventEdge::Becomes,
            lt(input("player.spread"), number(1.0)),
            &["scald"],
        )],
    );
    capture.assert_logged(
        log::Level::Error,
        "player event condition reads `player.spread`, which each machine publishes for its own player",
    );
    let errors = errors_containing(&capture, "condition failed to bind");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("setupLevel().playerEvents[0]") && errors[0].contains("`player.spread`"),
        "the bind error names the event and the slot: {}",
        errors[0]
    );
    world.tick();
    assert_eq!(
        world.health(pawn),
        30.0,
        "the event never evaluates the host's spread for this player"
    );
}

#[test]
fn a_fired_reaction_reading_a_local_only_player_slot_is_rejected_without_a_by_player_hint() {
    let mut world = World::new();
    world.spawn_player(Some(Seat(2)), 30.0);
    declare(&world, LAST_HP, ReplicationScope::SharedGlobal, false);
    let capture = LogCapture::start();
    world.install(
        vec![
            record_input("recordSpread", "player.spread"),
            record_input("recordSwitching", "player.weapon.switching"),
            // Reached through `fire`, the same read is rejected naming both.
            sequence("relay", vec![fire_step("recordSpread")]),
        ],
        vec![player_event(
            PlayerEventEdge::Becomes,
            low_health(),
            &["recordSpread", "recordSwitching", "relay"],
        )],
    );
    for (address, slot) in [
        ("recordSpread", "player.spread"),
        ("recordSwitching", "player.weapon.switching"),
    ] {
        let errors = errors_containing(&capture, &format!("drops address `{address}`"));
        assert_eq!(errors.len(), 1, "`{address}`: {errors:?}");
        assert!(
            errors[0].contains(&format!(
                "reads `{slot}`, which each machine publishes for its own player; a player event cannot read it for the event's player"
            )) && !errors[0].contains("byPlayer"),
            "a local-only slot has no `byPlayer` fix to point to: {}",
            errors[0]
        );
    }
    let errors = errors_containing(&capture, "drops address `relay`");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("reaches reaction `recordSpread`")
            && errors[0].contains("reads `player.spread`"),
        "{}",
        errors[0]
    );
    world.tick();
    assert_eq!(global(&world, LAST_HP), 0.0, "no rejected read runs");
}

#[test]
fn a_reaction_reading_on_occupancy_is_rejected_directly_and_through_a_route() {
    let mut world = World::new();
    world.spawn_player(Some(Seat(1)), 30.0);
    declare(&world, LAST_HP, ReplicationScope::SharedGlobal, false);
    let capture = LogCapture::start();
    world.install(
        vec![
            record_input("count", "@occupancy"),
            record_input("countLater", "@occupancy"),
            with_on_complete(
                system("mark", "setState", json!({ "slot": LAST_HP, "value": 1.0 })),
                "countLater",
            ),
        ],
        vec![player_event(
            PlayerEventEdge::Becomes,
            low_health(),
            &["count", "mark"],
        )],
    );
    let errors = errors_containing(&capture, "drops address `count`");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0]
            .contains("reaction `count` uses `on.occupancy`, which only trigger events publish"),
        "{}",
        errors[0]
    );
    let errors = errors_containing(&capture, "drops address `mark`");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("reaches reaction `countLater`")
            && errors[0].contains("uses `on.occupancy`"),
        "{}",
        errors[0]
    );
    world.tick();
    assert_eq!(
        global(&world, LAST_HP),
        0.0,
        "occupancy never silently reads 0 into the slot"
    );
}

#[test]
fn an_interruptible_wait_is_rejected_and_a_plain_wait_installs() {
    let wait = |interruptible: bool| SequenceStep {
        id: SequenceTarget::Wait,
        primitive: "wait".to_string(),
        args: json!({ "durationMs": 100.0, "interruptible": interruptible }),
    };
    let mut world = World::new();
    world.spawn_player(Some(Seat(1)), 30.0);
    let capture = LogCapture::start();
    world.install(
        vec![
            sequence("hold", vec![wait(true)]),
            sequence("pause", vec![wait(false)]),
        ],
        vec![player_event(
            PlayerEventEdge::Becomes,
            low_health(),
            &["hold", "pause"],
        )],
    );
    let errors = errors_containing(&capture, "drops address `hold`");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains(
            "reaction `hold` uses an interruptible `wait`, which only a trigger's exit cancels"
        ),
        "{}",
        errors[0]
    );
    assert!(errors_containing(&capture, "drops address `pause`").is_empty());
}

#[test]
fn player_event_audiences_reject_trigger_tokens_in_primitives_and_raw_sequence_args() {
    let mut world = World::new();
    world.spawn_player(Some(Seat(1)), 30.0);
    let capture = LogCapture::start();
    let mut reactions = Vec::new();
    let mut fire = Vec::new();
    for (label, token) in [("activators", "@activators"), ("trigger", "@trigger")] {
        for raw_sequence in [false, true] {
            let name = format!(
                "{label}.{}",
                if raw_sequence {
                    "sequence"
                } else {
                    "primitive"
                }
            );
            let args = json!({ "sound": "alarm", "audience": token });
            reactions.push(if raw_sequence {
                sequence(
                    &name,
                    vec![SequenceStep {
                        id: SequenceTarget::Entity(world.spawn_unbound(100.0)),
                        primitive: "playSound".to_string(),
                        args,
                    }],
                )
            } else {
                system(&name, "playSound", args)
            });
            fire.push(name);
        }
    }
    reactions.extend([
        system(
            "subject",
            "playSound",
            json!({ "sound": "subject", "audience": "@player" }),
        ),
        system(
            "everyone",
            "playSound",
            json!({ "sound": "team", "audience": "everyone" }),
        ),
        play_sound("default", "default"),
    ]);
    let wrong = fire.clone();
    reactions.push(sequence("relayEveryone", vec![fire_step("everyone")]));
    fire.push("relayEveryone".to_string());
    fire.extend([
        "subject".to_string(),
        "everyone".to_string(),
        "default".to_string(),
    ]);
    let addresses: Vec<_> = fire.iter().map(String::as_str).collect();
    world.install(
        reactions,
        vec![player_event(
            PlayerEventEdge::Becomes,
            low_health(),
            &addresses,
        )],
    );
    for name in wrong {
        let errors = errors_containing(&capture, &format!("drops address `{name}`"));
        assert_eq!(errors.len(), 1, "{name}: {errors:?}");
        let token = if name.starts_with("activators") {
            "on.activators"
        } else {
            "on.trigger"
        };
        assert!(
            errors[0].contains("setupLevel().playerEvents[0]")
                && errors[0].contains(&format!("reaction `{name}` uses `{token}`"))
                && errors[0].contains("only trigger events publish"),
            "{}",
            errors[0]
        );
    }
    let relay_errors = errors_containing(&capture, "drops address `relayEveryone`");
    assert_eq!(relay_errors.len(), 1);
    assert!(
        relay_errors[0].contains("reaches reaction `everyone`")
            && relay_errors[0].contains("presents `playSound`"),
        "audience everyone retains the existing prohibition on context-free player-event presentation"
    );
    for name in ["subject", "everyone", "default"] {
        assert!(errors_containing(&capture, &format!("drops address `{name}`")).is_empty());
    }
    world.tick();
    assert_eq!(
        world.take_residual_reactions(),
        vec!["subject", "everyone", "default"]
    );
}

#[test]
fn trigger_audiences_reject_player_tokens_in_primitives_and_raw_sequence_args() {
    use crate::trigger_bindings::TriggerBindingTable;
    use crate::trigger_system::TriggerEventEdge;
    use postretro_entities::{
        TriggerActivation, TriggerCommand, TriggerFireMode, TriggerVolumeComponent,
    };

    for (audience, raw_sequence, should_bind) in [
        ("@player", false, false),
        ("@player", true, false),
        ("@activators", false, true),
        ("@trigger", false, true),
        ("everyone", false, true),
    ] {
        let mut world = World::new();
        let trigger = {
            let mut registry = world.script_ctx.registry.borrow_mut();
            let trigger = registry.spawn(postretro_entities::Transform::default());
            registry
                .set_component(
                    trigger,
                    TriggerVolumeComponent::new(
                        TriggerActivation::Touch,
                        String::new(),
                        "cue".to_string(),
                        String::new(),
                        TriggerCommand::Start,
                        TriggerFireMode::Multiple,
                        0.0,
                        true,
                    ),
                )
                .unwrap();
            trigger
        };
        let reaction = if raw_sequence {
            sequence(
                "cue",
                vec![SequenceStep {
                    id: SequenceTarget::Entity(trigger),
                    primitive: "setEmitterRate".to_string(),
                    args: json!({ "rate": 1.0, "audience": audience }),
                }],
            )
        } else {
            system(
                "cue",
                "playSound",
                json!({ "sound": "alarm", "audience": audience }),
            )
        };
        world.install(vec![reaction], vec![]);
        let capture = LogCapture::start();
        let bindings = TriggerBindingTable::build_with_script_ctx(
            &world.script_ctx.registry.borrow(),
            &world.script_ctx.data_registry.borrow(),
            &world.script_ctx,
        );
        assert_eq!(
            bindings
                .bound_edges()
                .contains(&(trigger, TriggerEventEdge::Enter)),
            should_bind,
            "audience={audience}, raw_sequence={raw_sequence}"
        );
        let errors = errors_containing(&capture, "reaction `cue` uses `on.player`");
        if should_bind {
            assert!(errors.is_empty(), "{errors:?}");
        } else {
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert!(
                errors[0].contains("only a player event publishes")
                    && errors[0].contains(&trigger.to_string())
            );
        }
    }
}

#[test]
fn reachable_machine_local_reactions_walks_duplicate_bodies_cycles_and_post_wait_routes() {
    use super::reachable_machine_local_reactions;
    let mut data = postretro_scripting_core::data_registry::DataRegistry::new();
    let wait = || SequenceStep {
        id: SequenceTarget::Wait,
        primitive: "wait".to_string(),
        args: json!({ "durationMs": 100.0 }),
    };
    data.populate_level(
        vec![
            with_on_complete(
                system("root", "showDialog", json!({ "tree": "dialog" })),
                "relay",
            ),
            sequence("root", vec![wait(), fire_step("postWait")]),
            sequence(
                "postWait",
                vec![
                    wait(),
                    SequenceStep {
                        id: SequenceTarget::Entity(postretro_entities::EntityId::from_raw(1)),
                        primitive: "closeDialog".to_string(),
                        args: json!({}),
                    },
                    fire_step("relay"),
                ],
            ),
            sequence("relay", vec![fire_step("root")]),
            with_on_complete(play_sound("relay", "alarm"), "text"),
            with_on_complete(
                system(
                    "text",
                    "appendText",
                    json!({ "slot": "note", "text": "alarm" }),
                ),
                "postWait",
            ),
            system("text", "clearText", json!({ "slot": "note" })),
            system("unrelated", "openMenu", json!({ "tree": "other" })),
            system(
                "localSlot",
                "setState",
                json!({ "slot": "local", "value": 1.0 }),
            ),
        ],
        vec![],
        &[],
    );
    assert_eq!(
        reachable_machine_local_reactions("root", &data),
        vec!["root", "postWait", "text"],
        "each reached address warns once, including steps after wait and routes in every same-named body"
    );
    assert!(
        reachable_machine_local_reactions("localSlot", &data).is_empty(),
        "the UI warning class is MachineLocal; slot replication has its own validation"
    );
    assert!(reachable_machine_local_reactions("missing", &data).is_empty());
}
