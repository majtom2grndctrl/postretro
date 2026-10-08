use super::*;
use crate::data_descriptors::{
    EntityTypeDescriptor, NamedReaction, PrimitiveDescriptor, ProgressDescriptor,
    ReactionDescriptor,
};
use crate::registry::{EntityRegistry, Transform};
use crate::slot_table::{
    NumericRange, ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
};
use log::Level;
use postretro_foundation::Seat;
use postretro_test_log_capture::LogCapture;

fn per_owner_number_slot(value: f32) -> SlotRecord {
    SlotRecord::new(SlotSchema {
        slot_type: SlotType::Number,
        default: Some(SlotValue::Number(value)),
        range: Some(NumericRange {
            min: -10_000.0,
            max: 10_000.0,
        }),
        persist: false,
        readonly: false,
        ownership: SlotOwnership::Mod,
        network: ReplicationScope::None,
        per_owner: true,
        accumulate: None,
    })
}

fn progress_reaction(name: &str, tag: &str, at: f32, fire: &str) -> NamedReaction {
    NamedReaction {
        name: name.to_string(),
        descriptor: ReactionDescriptor::Progress(ProgressDescriptor {
            tag: tag.to_string(),
            at,
            fire: fire.to_string(),
        }),
    }
}

fn primitive_reaction(
    name: &str,
    primitive: &str,
    tag: &str,
    on_complete: Option<&str>,
) -> NamedReaction {
    NamedReaction {
        name: name.to_string(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: primitive.to_string(),
            target: None,
            kind: None,
            tag: Some(tag.to_string()),
            on_complete: on_complete.map(|s| s.to_string()),
            args: serde_json::Value::Object(Default::default()),
        }),
    }
}

fn spawn_with_tags(reg: &mut EntityRegistry, tags: &[&str]) -> EntityId {
    let id = reg.spawn(Transform::default());
    let owned: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
    reg.set_tags(id, owned).unwrap();
    id
}

#[cfg(debug_assertions)]
#[test]
fn update_npc_state_stays_in_trigger_consequential_mirror() {
    assert!(is_trigger_consequential_primitive("updateNpcState"));
}

#[cfg(debug_assertions)]
#[test]
fn spawn_from_spawner_stays_in_trigger_consequential_mirror() {
    assert!(is_trigger_consequential_primitive("spawnFromSpawner"));
}

#[test]
fn progress_threshold_fires_when_all_dead_at_full_ratio() {
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![progress_reaction("waveDone", "wave1", 1.0, "powerOn")],
        Vec::new(),
        &[],
    );

    let mut entities = EntityRegistry::new();
    let member = spawn_with_tags(&mut entities, &["wave1"]);

    let mut tracker = ProgressTracker::new();
    tracker.initialize(&data, &entities);

    let fired = tracker.on_entity_killed(member);
    assert_eq!(fired, vec!["powerOn".to_string()]);
}

#[test]
fn progress_does_not_fire_before_threshold() {
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![progress_reaction("waveDone", "wave1", 1.0, "powerOn")],
        Vec::new(),
        &[],
    );

    let mut entities = EntityRegistry::new();
    let first = spawn_with_tags(&mut entities, &["wave1"]);
    let second = spawn_with_tags(&mut entities, &["wave1"]);

    let mut tracker = ProgressTracker::new();
    tracker.initialize(&data, &entities);

    let fired = tracker.on_entity_killed(first);
    assert!(fired.is_empty());
    assert!(
        tracker.on_entity_killed(first).is_empty(),
        "a repeated report for one member never counts twice"
    );

    let fired = tracker.on_entity_killed(second);
    assert_eq!(fired, vec!["powerOn".to_string()]);
}

#[test]
fn progress_fires_at_partial_ratio_when_at_below_one() {
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![progress_reaction("half", "wave1", 0.5, "midwave")],
        Vec::new(),
        &[],
    );

    let mut entities = EntityRegistry::new();
    let members: Vec<EntityId> = (0..4)
        .map(|_| spawn_with_tags(&mut entities, &["wave1"]))
        .collect();

    let mut tracker = ProgressTracker::new();
    tracker.initialize(&data, &entities);

    assert!(tracker.on_entity_killed(members[0]).is_empty());
    let fired = tracker.on_entity_killed(members[1]);
    assert_eq!(fired, vec!["midwave".to_string()]);
    assert!(tracker.on_entity_killed(members[2]).is_empty());
}

#[test]
fn multi_tag_entity_decrements_both_buckets_independently() {
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![
            progress_reaction("waveDone", "wave1", 1.0, "powerOn"),
            progress_reaction("reactorDown", "reactorMonster", 1.0, "reactorOff"),
        ],
        Vec::new(),
        &[],
    );

    let mut entities = EntityRegistry::new();
    let member = spawn_with_tags(&mut entities, &["wave1", "reactorMonster"]);

    let mut tracker = ProgressTracker::new();
    tracker.initialize(&data, &entities);

    assert_eq!(tracker.subscription_count("wave1"), 1);
    assert_eq!(tracker.subscription_count("reactorMonster"), 1);

    let fired = tracker.on_entity_killed(member);
    assert!(fired.contains(&"powerOn".to_string()));
    assert!(fired.contains(&"reactorOff".to_string()));
    assert_eq!(fired.len(), 2);
}

#[test]
fn multi_tag_entity_fires_both_subscriptions() {
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![
            progress_reaction("waveDone", "wave1", 0.5, "powerOn"),
            progress_reaction("reactorDown", "reactorMonster", 0.5, "reactorOff"),
        ],
        Vec::new(),
        &[],
    );

    let mut entities = EntityRegistry::new();
    let member = spawn_with_tags(&mut entities, &["wave1", "reactorMonster"]);

    let mut tracker = ProgressTracker::new();
    tracker.initialize(&data, &entities);

    let fired = tracker.on_entity_killed(member);
    assert!(fired.contains(&"powerOn".to_string()));
    assert!(fired.contains(&"reactorOff".to_string()));
    assert_eq!(fired.len(), 2);
}

#[test]
fn killing_a_non_member_is_a_no_op() {
    let mut entities = EntityRegistry::new();
    let ghost = spawn_with_tags(&mut entities, &["ghosts"]);
    let mut tracker = ProgressTracker::new();
    let fired = tracker.on_entity_killed(ghost);
    assert!(fired.is_empty());
}

#[test]
fn clear_drops_all_subscriptions() {
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![progress_reaction("waveDone", "wave1", 1.0, "powerOn")],
        Vec::new(),
        &[],
    );
    let mut entities = EntityRegistry::new();
    let member = spawn_with_tags(&mut entities, &["wave1"]);

    let mut tracker = ProgressTracker::new();
    tracker.initialize(&data, &entities);
    assert_eq!(tracker.subscription_count("wave1"), 1);

    tracker.clear();
    assert_eq!(tracker.subscription_count("wave1"), 0);
    assert!(tracker.on_entity_killed(member).is_empty());
}

#[test]
fn progress_with_zero_total_never_fires() {
    // `total == 0` at init: no division-by-zero and threshold never fires.
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![progress_reaction("waveDone", "ghosts", 1.0, "spooky")],
        Vec::new(),
        &[],
    );
    let mut entities = EntityRegistry::new();

    let mut tracker = ProgressTracker::new();
    tracker.initialize(&data, &entities);
    // Tagged after install: not a member, so the zero total stays inert.
    let late = spawn_with_tags(&mut entities, &["ghosts"]);
    let fired = tracker.on_entity_killed(late);
    assert!(fired.is_empty());
}

// S2: membership is the id set carrying the tag at install. An entity tagged
// later — a spawner's output carries its `spawned_tags` — neither counts nor
// raises the total, and a mod hot reload recompose keeps both the set and the
// tally.
#[test]
fn progress_recompose_keeps_install_membership_and_kill_tally() {
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![progress_reaction("waveDone", "wave1", 1.0, "powerOn")],
        Vec::new(),
        &[],
    );
    let mut entities = EntityRegistry::new();
    let first = spawn_with_tags(&mut entities, &["wave1"]);
    let second = spawn_with_tags(&mut entities, &["wave1"]);

    let mut tracker = ProgressTracker::new();
    tracker.initialize(&data, &entities);
    assert!(tracker.on_entity_killed(first).is_empty());

    let late = spawn_with_tags(&mut entities, &["wave1"]);
    tracker.recompose(&data, &entities);
    assert!(
        tracker.on_entity_killed(late).is_empty(),
        "an entity tagged after install never joins the set"
    );
    assert_eq!(
        tracker.on_entity_killed(second),
        vec!["powerOn".to_string()],
        "the pre-recompose kill still counts and the total stays two"
    );
}

#[test]
fn progress_recompose_keeps_a_fired_subscription_fired() {
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![progress_reaction("half", "wave1", 0.5, "midwave")],
        Vec::new(),
        &[],
    );
    let mut entities = EntityRegistry::new();
    let first = spawn_with_tags(&mut entities, &["wave1"]);
    let second = spawn_with_tags(&mut entities, &["wave1"]);

    let mut tracker = ProgressTracker::new();
    tracker.initialize(&data, &entities);
    assert_eq!(tracker.on_entity_killed(first), vec!["midwave".to_string()]);

    tracker.recompose(&data, &entities);
    assert!(
        tracker.on_entity_killed(second).is_empty(),
        "a recompose does not re-arm a fired progress"
    );
}

#[test]
fn resolve_entity_type_finds_registered_classname() {
    let mut data = DataRegistry::new();
    data.upsert_entity_type(EntityTypeDescriptor {
        faction: None,
        tolerance: None,
        canonical_name: Some("grunt".to_string()),
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: None,
        touchable: None,
        mesh: None,
        health: None,
        behavior: None,
    });

    let resolved = resolve_entity_type("grunt", &data);
    assert_eq!(
        resolved,
        Some(&EntityTypeDescriptor {
            faction: None,
            tolerance: None,
            canonical_name: Some("grunt".to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        })
    );
}

#[test]
fn resolve_entity_type_returns_none_for_missing_classname() {
    let data = DataRegistry::new();
    assert!(resolve_entity_type("grunt", &data).is_none());
}

#[test]
fn fire_named_event_on_primitive_returns_on_complete_chain() {
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![primitive_reaction(
            "wave1Complete",
            "moveGeometry",
            "reactorChambers",
            Some("wave2Revealed"),
        )],
        Vec::new(),
        &[],
    );

    let chained = fire_named_event("wave1Complete", &data);
    assert_eq!(chained, vec!["wave2Revealed".to_string()]);
}

#[test]
fn fire_named_event_on_primitive_without_on_complete_returns_empty() {
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![primitive_reaction(
            "wave2Revealed",
            "activateGroup",
            "reactorWave2Monsters",
            None,
        )],
        Vec::new(),
        &[],
    );

    let chained = fire_named_event("wave2Revealed", &data);
    assert!(chained.is_empty());
}

#[test]
fn fire_named_event_on_progress_is_a_noop() {
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![progress_reaction("waveDone", "wave1", 1.0, "powerOn")],
        Vec::new(),
        &[],
    );
    let chained = fire_named_event("waveDone", &data);
    assert!(chained.is_empty());
}

#[test]
fn fire_named_event_unknown_name_returns_empty() {
    let data = DataRegistry::new();
    let chained = fire_named_event("nothingHere", &data);
    assert!(chained.is_empty());
}

#[test]
fn add_slot_tag_target_resolves_owner_seat_for_level_load_and_crossing_dispatch() {
    use crate::reaction_registry::SystemReactionCommand;

    let script_ctx = ScriptCtx::new();
    script_ctx
        .slot_table
        .borrow_mut()
        .insert("currency.xp".into(), per_owner_number_slot(0.0))
        .expect("new owner slot");
    let pawn = script_ctx.registry.borrow_mut().spawn(Transform::default());
    {
        let mut registry = script_ctx.registry.borrow_mut();
        registry
            .set_tags(pawn, vec!["players".to_string()])
            .unwrap();
        registry.bind_pawn_seat(pawn, Seat(4));
    }
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![NamedReaction {
            name: "levelLoadOrCrossing".to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "addSlot".to_string(),
                target: None,
                kind: None,
                tag: Some("players".to_string()),
                on_complete: None,
                args: serde_json::json!({ "slot": "currency.xp", "delta": 3.0 }),
            }),
        }],
        Vec::new(),
        &[],
    );

    fire_named_event_with_sequences(
        "levelLoadOrCrossing",
        &data,
        &SequencedPrimitiveRegistry::new(),
        &ReactionPrimitiveRegistry::new(),
        &SystemReactionRegistry::new(),
        &script_ctx,
        None,
    );

    assert_eq!(
        script_ctx.system_commands.take(),
        vec![SystemReactionCommand::AddOwnerSlot {
            slot: "currency.xp".to_string(),
            seats: vec![Seat(4)],
            delta: 3.0,
        }],
        "the same tag resolver is used by level-load and crossing named dispatches"
    );
}

#[test]
fn add_slot_zero_recipients_and_client_dispatch_are_silent_no_ops() {
    let script_ctx = ScriptCtx::new();
    script_ctx
        .slot_table
        .borrow_mut()
        .insert("currency.xp".into(), per_owner_number_slot(0.0))
        .expect("new owner slot");
    let descriptor = PrimitiveDescriptor {
        primitive: "addSlot".to_string(),
        target: None,
        kind: None,
        tag: Some("no-pawns".to_string()),
        on_complete: None,
        args: serde_json::json!({ "slot": "currency.xp", "delta": 1.0 }),
    };
    let logs = LogCapture::start();
    dispatch_primitive(
        &descriptor,
        &ReactionPrimitiveRegistry::new(),
        &SystemReactionRegistry::new(),
        &script_ctx,
    );
    assert!(script_ctx.system_commands.is_empty());
    assert!(logs.records().is_empty(), "zero recipients must not warn");
    drop(logs);

    script_ctx.owner_slot_writes_enabled.set(false);
    let pawn = script_ctx.registry.borrow_mut().spawn(Transform::default());
    script_ctx
        .registry
        .borrow_mut()
        .set_tags(pawn, vec!["no-pawns".to_string()])
        .unwrap();
    let logs = LogCapture::start();
    dispatch_primitive(
        &descriptor,
        &ReactionPrimitiveRegistry::new(),
        &SystemReactionRegistry::new(),
        &script_ctx,
    );
    assert!(script_ctx.system_commands.is_empty());
    assert!(logs.records().is_empty(), "client addSlot must be silent");
}

// Regression: an invalid named addSlot escaped validation when its tag had no matches.
#[test]
fn named_add_slot_validates_global_slot_without_recipients_and_runs_sibling_effect() {
    let script_ctx = ScriptCtx::new();
    let mut global_slot = per_owner_number_slot(0.0);
    global_slot.schema.per_owner = false;
    script_ctx
        .slot_table
        .borrow_mut()
        .insert("currency.teamKills".into(), global_slot)
        .expect("new global slot");

    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let calls_for_handler = std::sync::Arc::clone(&calls);
    let mut system_registry = SystemReactionRegistry::new();
    system_registry.register("record", move |args, _queue| {
        calls_for_handler.lock().unwrap().push(
            args.get("label")
                .and_then(serde_json::Value::as_str)
                .unwrap()
                .to_string(),
        );
        Ok(())
    });

    let mut data = DataRegistry::new();
    data.populate_level(
        vec![
            NamedReaction {
                name: "levelLoadOrCrossing".to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "addSlot".to_string(),
                    target: None,
                    kind: None,
                    tag: Some("no-pawns".to_string()),
                    on_complete: None,
                    args: serde_json::json!({
                        "slot": "currency.teamKills",
                        "delta": 1.0
                    }),
                }),
            },
            NamedReaction {
                name: "levelLoadOrCrossing".to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "record".to_string(),
                    target: None,
                    kind: None,
                    tag: None,
                    on_complete: None,
                    args: serde_json::json!({ "label": "sibling" }),
                }),
            },
        ],
        Vec::new(),
        &[],
    );

    let logs = LogCapture::start();
    fire_named_event_with_sequences(
        "levelLoadOrCrossing",
        &data,
        &SequencedPrimitiveRegistry::new(),
        &ReactionPrimitiveRegistry::new(),
        &system_registry,
        &script_ctx,
        None,
    );

    logs.assert_logged_once(
        Level::Warn,
        "[Scripting] addSlot requires per-owner slot `currency.teamKills`; reaction had no effect",
    );
    assert_eq!(calls.lock().unwrap().as_slice(), ["sibling".to_string()]);
}

#[test]
fn named_add_slot_with_valid_descriptor_and_zero_recipients_is_silent_and_runs_sibling_effect() {
    let script_ctx = ScriptCtx::new();
    script_ctx
        .slot_table
        .borrow_mut()
        .insert("currency.xp".into(), per_owner_number_slot(0.0))
        .expect("new owner slot");

    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let calls_for_handler = std::sync::Arc::clone(&calls);
    let mut system_registry = SystemReactionRegistry::new();
    system_registry.register("record", move |args, _queue| {
        calls_for_handler.lock().unwrap().push(
            args.get("label")
                .and_then(serde_json::Value::as_str)
                .unwrap()
                .to_string(),
        );
        Ok(())
    });

    let mut data = DataRegistry::new();
    data.populate_level(
        vec![
            NamedReaction {
                name: "levelLoadOrCrossing".to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "addSlot".to_string(),
                    target: None,
                    kind: None,
                    tag: Some("no-pawns".to_string()),
                    on_complete: None,
                    args: serde_json::json!({ "slot": "currency.xp", "delta": 1.0 }),
                }),
            },
            NamedReaction {
                name: "levelLoadOrCrossing".to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "record".to_string(),
                    target: None,
                    kind: None,
                    tag: None,
                    on_complete: None,
                    args: serde_json::json!({ "label": "sibling" }),
                }),
            },
        ],
        Vec::new(),
        &[],
    );

    let logs = LogCapture::start();
    fire_named_event_with_sequences(
        "levelLoadOrCrossing",
        &data,
        &SequencedPrimitiveRegistry::new(),
        &ReactionPrimitiveRegistry::new(),
        &system_registry,
        &script_ctx,
        None,
    );

    assert!(
        logs.records()
            .iter()
            .all(|record| record.level != Level::Warn),
        "valid zero-recipient addSlot must not warn"
    );
    assert_eq!(calls.lock().unwrap().as_slice(), ["sibling".to_string()]);
}

// A trigger-bound Progress reaction means "the tracker watches this tag", not
// "fire the target now". Firing it from the residual would double-fire the
// target — once on the trigger with zero kills, once again at the threshold.
#[test]
fn prepartitioned_progress_is_a_noop_and_yields_no_follow_up() {
    let script_ctx = ScriptCtx::new();
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let calls_for_handler = Arc::clone(&calls);
    let mut system_registry = SystemReactionRegistry::new();
    system_registry.register("record", move |args, _queue| {
        calls_for_handler.lock().unwrap().push(
            args.get("label")
                .and_then(serde_json::Value::as_str)
                .unwrap()
                .to_string(),
        );
        Ok(())
    });

    let progress = ProgressDescriptor {
        tag: "wave".into(),
        at: 1.0,
        fire: "release".into(),
    };
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![NamedReaction {
            name: "release".into(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "record".into(),
                target: None,
                kind: None,
                tag: None,
                on_complete: None,
                args: serde_json::json!({ "label": "release" }),
            }),
        }],
        Vec::new(),
        &[],
    );

    let sequence_registry = SequencedPrimitiveRegistry::new();
    let reaction_registry = ReactionPrimitiveRegistry::new();
    let follow_ups = fire_prepartitioned_reactions_with_sequences(
        &[PrepartitionedReactionStep::Descriptor(
            String::new(),
            0,
            ReactionDescriptor::Progress(progress),
        )],
        &sequence_registry,
        &reaction_registry,
        &system_registry,
        &script_ctx,
        ResidualOrigin::TriggerBinding,
    );
    assert!(
        follow_ups.is_empty(),
        "a prepartitioned Progress descriptor must not queue its fire target",
    );

    dispatch_deferred_named_events_with_sequences(
        follow_ups,
        &data,
        &sequence_registry,
        &reaction_registry,
        &system_registry,
        &script_ctx,
    );
    assert!(
        calls.lock().unwrap().is_empty(),
        "the Progress target must not execute on the app drain",
    );
}

// Companion to the test above: no-oping the residual must not break the real
// progress path — the tracker still owns and fires the target at threshold.
#[test]
fn progress_tracker_still_fires_its_target_at_threshold() {
    let script_ctx = ScriptCtx::new();
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let calls_for_handler = Arc::clone(&calls);
    let mut system_registry = SystemReactionRegistry::new();
    system_registry.register("record", move |args, _queue| {
        calls_for_handler.lock().unwrap().push(
            args.get("label")
                .and_then(serde_json::Value::as_str)
                .unwrap()
                .to_string(),
        );
        Ok(())
    });

    let mut data = DataRegistry::new();
    data.populate_level(
        vec![
            progress_reaction("waveDone", "wave", 1.0, "release"),
            NamedReaction {
                name: "release".into(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "record".into(),
                    target: None,
                    kind: None,
                    tag: None,
                    on_complete: None,
                    args: serde_json::json!({ "label": "release" }),
                }),
            },
        ],
        Vec::new(),
        &[],
    );

    let mut entities = EntityRegistry::new();
    let first = spawn_with_tags(&mut entities, &["wave"]);
    let second = spawn_with_tags(&mut entities, &["wave"]);

    let mut tracker = ProgressTracker::new();
    tracker.initialize(&data, &entities);

    let sequence_registry = SequencedPrimitiveRegistry::new();
    let reaction_registry = ReactionPrimitiveRegistry::new();

    let fired = tracker.on_entity_killed(first);
    assert!(fired.is_empty(), "half the wave is not the threshold");

    let fired = tracker.on_entity_killed(second);
    assert_eq!(fired, vec!["release".to_string()]);

    dispatch_deferred_named_events_with_sequences(
        fired,
        &data,
        &sequence_registry,
        &reaction_registry,
        &system_registry,
        &script_ctx,
    );
    assert_eq!(calls.lock().unwrap().as_slice(), ["release".to_string()]);
}

#[test]
fn deferred_named_events_are_breadth_first_and_batch_hop_bounded() {
    let script_ctx = ScriptCtx::new();
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let calls_for_handler = Arc::clone(&calls);
    let mut system_registry = SystemReactionRegistry::new();
    system_registry.register("record", move |args, _queue| {
        calls_for_handler.lock().unwrap().push(
            args.get("label")
                .and_then(serde_json::Value::as_str)
                .unwrap()
                .to_string(),
        );
        Ok(())
    });

    let named = |name: &str, on_complete: Option<&str>| NamedReaction {
        name: name.into(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: "record".into(),
            target: None,
            kind: None,
            tag: None,
            on_complete: on_complete.map(str::to_string),
            args: serde_json::json!({ "label": name }),
        }),
    };
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![
            named("first", Some("after_first")),
            named("second", Some("after_second")),
            named("after_first", Some("first")),
            named("after_second", None),
        ],
        Vec::new(),
        &[],
    );

    let sequence_registry = SequencedPrimitiveRegistry::new();
    let reaction_registry = ReactionPrimitiveRegistry::new();
    let dispatched = dispatch_deferred_named_events_with_sequences_up_to(
        ["first".to_string(), "second".to_string()],
        &data,
        &sequence_registry,
        &reaction_registry,
        &system_registry,
        &script_ctx,
        3,
    );

    assert_eq!(dispatched, 3);
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        [
            "first".to_string(),
            "second".to_string(),
            "after_first".to_string(),
        ],
        "the shared batch cap applies after FIFO drains both roots, then the first follow-up"
    );
}

use crate::ctx::ScriptCtx;
use crate::data_descriptors::SequenceStep;
use crate::data_registry::ScopedReaction;
use crate::registry::EntityId;
use crate::sequence::{SequenceError, SequencedPrimitiveRegistry};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

fn sequence_reaction(name: &str, steps: Vec<SequenceStep>) -> NamedReaction {
    NamedReaction {
        name: name.to_string(),
        descriptor: ReactionDescriptor::Sequence(steps),
    }
}

// A system reaction (no `tag`) fired through the SAME `fire_named_event`
// path an entity event uses resolves through the shared vocabulary and
// enqueues a typed command onto the queue — one namespace, two arms.
#[test]
fn system_reaction_fired_by_named_event_enqueues_command() {
    use crate::reaction_registry::SystemReactionCommand;

    let script_ctx = ScriptCtx::new();

    let mut data = DataRegistry::new();
    data.populate_level(
        vec![NamedReaction {
            name: "lowHealth".to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "playSound".to_string(),
                target: None,
                // No tag ⇒ system-targeted.
                kind: None,
                tag: None,
                on_complete: None,
                args: serde_json::json!({ "sound": "alarm", "bus": "sfx" }),
            }),
        }],
        Vec::new(),
        &[],
    );

    let seq_reg = SequencedPrimitiveRegistry::new();
    let reaction_reg = ReactionPrimitiveRegistry::new();
    let mut system_reg = SystemReactionRegistry::new();
    system_reg.register("playSound", |args, queue| {
        let sound = args
            .get("sound")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let bus = args
            .get("bus")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        queue.push(SystemReactionCommand::PlaySound {
            sound,
            bus,
            at: None,
        });
        Ok(())
    });

    assert!(script_ctx.system_commands.is_empty());
    fire_named_event_with_sequences(
        "lowHealth",
        &data,
        &seq_reg,
        &reaction_reg,
        &system_reg,
        &script_ctx,
        None,
    );

    assert_eq!(
        script_ctx.system_commands.take(),
        vec![SystemReactionCommand::PlaySound {
            sound: "alarm".to_string(),
            bus: Some("sfx".to_string()),
            at: None,
        }]
    );
}

// O62: a resumed tail is everything AFTER a wait, so it may legitimately
// contain a consequential step the binder deferred. `ResidualOrigin::ResumedTail`
// exempts it from the residual consequential-primitive guard — no panic even in
// a debug build.
#[test]
fn resumed_tail_with_consequential_step_does_not_trip_residual_guard() {
    let script_ctx = ScriptCtx::new();
    let tail = vec![
        SequenceStep {
            id: EntityId::from_raw(1).into(),
            primitive: "moverStart".into(),
            args: serde_json::json!({}),
        },
        SequenceStep {
            id: postretro_entities::SequenceTarget::Fire,
            primitive: "fire".into(),
            args: serde_json::json!({ "event": "release" }),
        },
    ];
    let follow_ups = fire_prepartitioned_reactions_with_sequences(
        &[PrepartitionedReactionStep::Descriptor(
            "closet.timedReveal".into(),
            0,
            ReactionDescriptor::Sequence(tail),
        )],
        &SequencedPrimitiveRegistry::new(),
        &ReactionPrimitiveRegistry::new(),
        &SystemReactionRegistry::new(),
        &script_ctx,
        ResidualOrigin::ResumedTail,
    );
    assert_eq!(
        follow_ups,
        vec!["release".to_string()],
        "the tail's fire name is collected and the consequential step does not panic",
    );
}

// Companion to the row above: the SAME consequential tail draining as a
// `TriggerBinding` residual DOES trip the guard, so the exemption is keyed on
// the caller, not on the steps. Debug-only (the guard is `debug_assert!`).
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "consequential sequence step")]
fn trigger_binding_residual_with_consequential_sequence_step_trips_guard() {
    let script_ctx = ScriptCtx::new();
    let steps = vec![SequenceStep {
        id: EntityId::from_raw(1).into(),
        primitive: "moverStart".into(),
        args: serde_json::json!({}),
    }];
    let _ = fire_prepartitioned_reactions_with_sequences(
        &[PrepartitionedReactionStep::Descriptor(
            String::new(),
            0,
            ReactionDescriptor::Sequence(steps),
        )],
        &SequencedPrimitiveRegistry::new(),
        &ReactionPrimitiveRegistry::new(),
        &SystemReactionRegistry::new(),
        &script_ctx,
        ResidualOrigin::TriggerBinding,
    );
}

// Regression: a trigger-bound wait-tail residual `Sequence([wait, moverStart,
// fire])` panicked in debug — the guard inspected every step and tripped on the
// post-wait `moverStart`, even though `dispatch_sequence` breaks at the leading
// wait and never runs it synchronously. The guard now inspects only the pre-wait
// prefix, so a consequential step after the wait is legitimately deferred.
#[cfg(debug_assertions)]
#[test]
fn trigger_binding_wait_tail_with_post_wait_consequential_step_does_not_panic() {
    let script_ctx = ScriptCtx::new();
    let steps = vec![
        SequenceStep {
            id: postretro_entities::SequenceTarget::Wait,
            primitive: "wait".into(),
            args: serde_json::json!({ "durationMs": 800 }),
        },
        SequenceStep {
            id: EntityId::from_raw(1).into(),
            primitive: "moverStart".into(),
            args: serde_json::json!({}),
        },
        SequenceStep {
            id: postretro_entities::SequenceTarget::Fire,
            primitive: "fire".into(),
            args: serde_json::json!({ "event": "release" }),
        },
    ];
    // No `wait` control handler is registered, so `dispatch_sequence` logs and
    // breaks at the leading wait; the point is that the residual guard does not
    // trip on the deferred post-wait `moverStart`.
    let follow_ups = fire_prepartitioned_reactions_with_sequences(
        &[PrepartitionedReactionStep::Descriptor(
            "closet.timedReveal".into(),
            0,
            ReactionDescriptor::Sequence(steps),
        )],
        &SequencedPrimitiveRegistry::new(),
        &ReactionPrimitiveRegistry::new(),
        &SystemReactionRegistry::new(),
        &script_ctx,
        ResidualOrigin::TriggerBinding,
    );
    assert!(
        follow_ups.is_empty(),
        "the drain breaks at the leading wait; the post-wait fire never runs synchronously",
    );
}

#[test]
fn sequence_dispatch_runs_each_step_in_order() {
    let script_ctx = ScriptCtx::new();
    let id_a = script_ctx.registry.borrow_mut().spawn(Transform::default());
    let id_b = script_ctx.registry.borrow_mut().spawn(Transform::default());

    let calls: Arc<std::sync::Mutex<Vec<(u32, i64)>>> = Arc::new(std::sync::Mutex::new(Vec::new()));

    let mut seq_reg = SequencedPrimitiveRegistry::new();
    let calls_cl = Arc::clone(&calls);
    seq_reg.register("noteValue", move |id, args| {
        let v = args["v"].as_i64().unwrap_or(-1);
        calls_cl.lock().unwrap().push((id.to_raw(), v));
        Ok(())
    });

    let mut data = DataRegistry::new();
    data.populate_level(
        vec![sequence_reaction(
            "go",
            vec![
                SequenceStep {
                    id: id_a.into(),
                    primitive: "noteValue".into(),
                    args: serde_json::json!({ "v": 1 }),
                },
                SequenceStep {
                    id: id_b.into(),
                    primitive: "noteValue".into(),
                    args: serde_json::json!({ "v": 2 }),
                },
            ],
        )],
        Vec::new(),
        &[],
    );

    let reaction_reg = ReactionPrimitiveRegistry::new();
    let system_reg = SystemReactionRegistry::new();
    let chained = fire_named_event_with_sequences(
        "go",
        &data,
        &seq_reg,
        &reaction_reg,
        &system_reg,
        &script_ctx,
        None,
    );
    assert!(chained.is_empty());
    let observed = calls.lock().unwrap().clone();
    assert_eq!(observed, vec![(id_a.to_raw(), 1), (id_b.to_raw(), 2)]);
}

// AC 10: a NAMED (non-trigger) dispatch has no fire context, so a primitive
// carrying a sentinel `target` cannot resolve — it warns and is skipped,
// while a sibling sentinel-free reaction on the same event name still runs.
#[test]
fn named_dispatch_skips_sentinel_target_primitive_but_runs_sentinel_free_command() {
    use crate::reaction_registry::SystemReactionCommand;

    let script_ctx = ScriptCtx::new();
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![
            // Sentinel target with no trigger fire context: must warn-skip.
            NamedReaction {
                name: "onPress".to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "applyDamage".to_string(),
                    target: Some("@activators".to_string()),
                    kind: None,
                    tag: None,
                    on_complete: None,
                    args: serde_json::json!({ "amount": 25 }),
                }),
            },
            // Sentinel-free system reaction on the same event name: must run.
            NamedReaction {
                name: "onPress".to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "playSound".to_string(),
                    target: None,
                    kind: None,
                    tag: None,
                    on_complete: None,
                    args: serde_json::json!({ "sound": "beep", "bus": "sfx" }),
                }),
            },
        ],
        Vec::new(),
        &[],
    );

    let seq_reg = SequencedPrimitiveRegistry::new();
    let reaction_reg = ReactionPrimitiveRegistry::new();
    let mut system_reg = SystemReactionRegistry::new();
    system_reg.register("playSound", |args, queue| {
        let sound = args
            .get("sound")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let bus = args
            .get("bus")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        queue.push(SystemReactionCommand::PlaySound {
            sound,
            bus,
            at: None,
        });
        Ok(())
    });

    fire_named_event_with_sequences(
        "onPress",
        &data,
        &seq_reg,
        &reaction_reg,
        &system_reg,
        &script_ctx,
        None,
    );

    assert_eq!(
        script_ctx.system_commands.take(),
        vec![SystemReactionCommand::PlaySound {
            sound: "beep".to_string(),
            bus: Some("sfx".to_string()),
            at: None,
        }],
        "the sentinel-target primitive is skipped; only the sentinel-free command runs",
    );
}

// AC 10: the symmetric sequence path — a named dispatch's sequence step
// carrying a sentinel `id` has no fire context to resolve, so it warns and
// is skipped, while the sequence's entity-targeted step still executes.
#[test]
fn named_dispatch_skips_sentinel_sequence_step_but_runs_entity_step() {
    let script_ctx = ScriptCtx::new();
    let id_entity = script_ctx.registry.borrow_mut().spawn(Transform::default());

    let calls: Arc<std::sync::Mutex<Vec<(u32, i64)>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let calls_cl = Arc::clone(&calls);
    let mut seq_reg = SequencedPrimitiveRegistry::new();
    seq_reg.register("noteValue", move |id, args| {
        let v = args["v"].as_i64().unwrap_or(-1);
        calls_cl.lock().unwrap().push((id.to_raw(), v));
        Ok(())
    });

    let mut data = DataRegistry::new();
    data.populate_level(
        vec![sequence_reaction(
            "onComplete",
            vec![
                SequenceStep {
                    id: postretro_entities::SequenceTarget::Activators,
                    primitive: "noteValue".into(),
                    args: serde_json::json!({ "v": 1 }),
                },
                SequenceStep {
                    id: id_entity.into(),
                    primitive: "noteValue".into(),
                    args: serde_json::json!({ "v": 2 }),
                },
            ],
        )],
        Vec::new(),
        &[],
    );

    let reaction_reg = ReactionPrimitiveRegistry::new();
    let system_reg = SystemReactionRegistry::new();
    fire_named_event_with_sequences(
        "onComplete",
        &data,
        &seq_reg,
        &reaction_reg,
        &system_reg,
        &script_ctx,
        None,
    );

    assert_eq!(
        calls.lock().unwrap().clone(),
        vec![(id_entity.to_raw(), 2)],
        "the sentinel step is skipped; the entity-targeted step still runs",
    );
}

#[test]
fn sequence_dispatch_skips_stale_entity_and_continues() {
    let script_ctx = ScriptCtx::new();
    let id_a = script_ctx.registry.borrow_mut().spawn(Transform::default());
    let id_b = script_ctx.registry.borrow_mut().spawn(Transform::default());

    // Stale ID: reuse a slot that was despawned (mismatched generation).
    let id_dead = script_ctx.registry.borrow_mut().spawn(Transform::default());
    script_ctx.registry.borrow_mut().despawn(id_dead).unwrap();
    assert!(!script_ctx.registry.borrow().exists(id_dead));

    let count = Arc::new(AtomicU32::new(0));
    let count_cl = Arc::clone(&count);

    let mut seq_reg = SequencedPrimitiveRegistry::new();
    seq_reg.register("tick", move |_id, _args| {
        count_cl.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });

    let mut data = DataRegistry::new();
    data.populate_level(
        vec![sequence_reaction(
            "go",
            vec![
                SequenceStep {
                    id: id_a.into(),
                    primitive: "tick".into(),
                    args: serde_json::Value::Null,
                },
                SequenceStep {
                    id: id_dead.into(),
                    primitive: "tick".into(),
                    args: serde_json::Value::Null,
                },
                SequenceStep {
                    id: id_b.into(),
                    primitive: "tick".into(),
                    args: serde_json::Value::Null,
                },
            ],
        )],
        Vec::new(),
        &[],
    );

    let reaction_reg = ReactionPrimitiveRegistry::new();
    let system_reg = SystemReactionRegistry::new();
    fire_named_event_with_sequences(
        "go",
        &data,
        &seq_reg,
        &reaction_reg,
        &system_reg,
        &script_ctx,
        None,
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[test]
fn sequence_dispatch_continues_after_handler_error() {
    let script_ctx = ScriptCtx::new();
    let id_a = script_ctx.registry.borrow_mut().spawn(Transform::default());
    let id_b = script_ctx.registry.borrow_mut().spawn(Transform::default());

    let count = Arc::new(AtomicU32::new(0));
    let count_cl = Arc::clone(&count);

    let mut seq_reg = SequencedPrimitiveRegistry::new();
    seq_reg.register("ok", move |_id, _args| {
        count_cl.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    seq_reg.register("boom", |_id, _args| {
        Err(SequenceError::ExecutionFailed {
            reason: "intentional".into(),
        })
    });

    let mut data = DataRegistry::new();
    data.populate_level(
        vec![sequence_reaction(
            "go",
            vec![
                SequenceStep {
                    id: id_a.into(),
                    primitive: "boom".into(),
                    args: serde_json::Value::Null,
                },
                SequenceStep {
                    id: id_b.into(),
                    primitive: "ok".into(),
                    args: serde_json::Value::Null,
                },
            ],
        )],
        Vec::new(),
        &[],
    );

    let reaction_reg = ReactionPrimitiveRegistry::new();
    let system_reg = SystemReactionRegistry::new();
    fire_named_event_with_sequences(
        "go",
        &data,
        &seq_reg,
        &reaction_reg,
        &system_reg,
        &script_ctx,
        None,
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn validate_sequence_primitives_drops_reaction_with_unknown_primitive() {
    let mut seq_reg = SequencedPrimitiveRegistry::new();
    seq_reg.register("known", |_id, _args| Ok(()));

    let bogus_id = EntityId::from_raw(0x0001_0000);
    let reactions = vec![
        sequence_reaction(
            "valid",
            vec![SequenceStep {
                id: bogus_id.into(),
                primitive: "known".into(),
                args: serde_json::Value::Null,
            }],
        ),
        sequence_reaction(
            "invalid",
            vec![
                SequenceStep {
                    id: bogus_id.into(),
                    primitive: "known".into(),
                    args: serde_json::Value::Null,
                },
                SequenceStep {
                    id: bogus_id.into(),
                    primitive: "ghost".into(),
                    args: serde_json::Value::Null,
                },
            ],
        ),
    ];

    let surviving = validate_sequence_primitives(reactions, &seq_reg);
    assert_eq!(surviving.len(), 1);
    assert_eq!(surviving[0].name, "valid");
}

#[test]
fn validate_sequence_primitives_drops_reaction_when_bad_step_is_at_index_0() {
    let mut seq_reg = SequencedPrimitiveRegistry::new();
    seq_reg.register("known", |_id, _args| Ok(()));

    let bogus_id = EntityId::from_raw(0x0001_0000);
    let reactions = vec![
        sequence_reaction(
            "valid",
            vec![SequenceStep {
                id: bogus_id.into(),
                primitive: "known".into(),
                args: serde_json::Value::Null,
            }],
        ),
        sequence_reaction(
            "invalid_at_zero",
            vec![
                SequenceStep {
                    id: bogus_id.into(),
                    primitive: "ghost".into(),
                    args: serde_json::Value::Null,
                },
                SequenceStep {
                    id: bogus_id.into(),
                    primitive: "known".into(),
                    args: serde_json::Value::Null,
                },
            ],
        ),
    ];

    let surviving = validate_sequence_primitives(reactions, &seq_reg);
    assert_eq!(surviving.len(), 1);
    assert_eq!(surviving[0].name, "valid");
}

#[test]
fn validate_scoped_sequence_primitives_drops_invalid_sequences_and_preserves_levels() {
    let mut seq_reg = SequencedPrimitiveRegistry::new();
    seq_reg.register("known", |_id, _args| Ok(()));

    let bogus_id = EntityId::from_raw(0x0001_0000);
    let reactions = vec![
        ScopedReaction {
            reaction: primitive_reaction("non_sequence", "moveGeometry", "reactor", None),
            levels: vec!["campaign".to_string()],
        },
        ScopedReaction {
            reaction: sequence_reaction(
                "valid_sequence",
                vec![SequenceStep {
                    id: bogus_id.into(),
                    primitive: "known".into(),
                    args: serde_json::Value::Null,
                }],
            ),
            levels: vec!["campaign".to_string(), "boss".to_string()],
        },
        ScopedReaction {
            reaction: sequence_reaction(
                "invalid_sequence",
                vec![SequenceStep {
                    id: bogus_id.into(),
                    primitive: "ghost".into(),
                    args: serde_json::Value::Null,
                }],
            ),
            levels: vec!["campaign".to_string()],
        },
    ];

    let surviving = validate_scoped_sequence_primitives(reactions, &seq_reg);

    assert_eq!(surviving.len(), 2);
    assert_eq!(surviving[0].reaction.name, "non_sequence");
    assert_eq!(surviving[0].levels, vec!["campaign"]);
    assert_eq!(surviving[1].reaction.name, "valid_sequence");
    assert_eq!(surviving[1].levels, vec!["campaign", "boss"]);
}

// O35: a reaction containing `@wait`/`@fire` control steps survives
// `setupLevel` validation (`wait`/`fire` are registered names), while a
// sequence naming an unknown *action* primitive is still dropped.
#[test]
fn wait_and_fire_survive_sequence_validation_but_unknown_action_is_dropped() {
    let mut seq_reg = SequencedPrimitiveRegistry::new();
    // Inert admission entries, as the binary registers them.
    seq_reg.register("wait", |_id, _args| Ok(()));
    seq_reg.register("fire", |_id, _args| Ok(()));
    seq_reg.register("setLightAnimation", |_id, _args| Ok(()));

    let bogus_id = EntityId::from_raw(0x0001_0000);
    let reactions = vec![
        sequence_reaction(
            "timedReveal",
            vec![
                SequenceStep {
                    id: postretro_entities::SequenceTarget::Fire,
                    primitive: "fire".into(),
                    args: serde_json::json!({ "event": "raiseAlarm" }),
                },
                SequenceStep {
                    id: postretro_entities::SequenceTarget::Wait,
                    primitive: "wait".into(),
                    args: serde_json::json!({ "durationMs": 800, "interruptible": true }),
                },
                SequenceStep {
                    id: bogus_id.into(),
                    primitive: "setLightAnimation".into(),
                    args: serde_json::Value::Null,
                },
            ],
        ),
        sequence_reaction(
            "bogusAction",
            vec![SequenceStep {
                id: bogus_id.into(),
                primitive: "notARegisteredPrimitive".into(),
                args: serde_json::Value::Null,
            }],
        ),
    ];

    let surviving = validate_sequence_primitives(reactions, &seq_reg);
    assert_eq!(
        surviving.len(),
        1,
        "only the unknown-action reaction is dropped"
    );
    assert_eq!(surviving[0].name, "timedReveal");
}

// O34: firing a named body that contains a wait runs only up to that wait,
// including at hop depth >= 1 inside a deferred batch. `S = [x, fire(R)]`,
// `R = [alarm, wait(800), moverStart]`; firing S dispatches R via the
// deferred hop, R runs `alarm`, hits the `@wait` arm, enrolls the tail, and
// breaks — `moverStart` never runs in the same drain.
#[test]
fn name_fired_body_runs_only_up_to_its_wait_across_a_deferred_hop() {
    let script_ctx = ScriptCtx::new();
    let x = script_ctx.registry.borrow_mut().spawn(Transform::default());
    let alarm = script_ctx.registry.borrow_mut().spawn(Transform::default());
    let mover = script_ctx.registry.borrow_mut().spawn(Transform::default());

    let ran: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let enrolled_tail_len: Arc<std::sync::Mutex<Option<usize>>> =
        Arc::new(std::sync::Mutex::new(None));

    let mut seq_reg = SequencedPrimitiveRegistry::new();
    seq_reg.register("wait", |_id, _args| Ok(()));
    seq_reg.register("fire", |_id, _args| Ok(()));
    let ran_note = Arc::clone(&ran);
    seq_reg.register("note", move |_id, args| {
        let label = args
            .get("label")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        ran_note.lock().unwrap().push(label);
        Ok(())
    });
    let captured = Arc::clone(&enrolled_tail_len);
    seq_reg.register_control("wait", move |_address, _ordinal, tail, _args| {
        // Model enrollment: record the tail length; do NOT run it.
        *captured.lock().unwrap() = Some(tail.len());
    });

    let note = |label: &str, id: EntityId| SequenceStep {
        id: id.into(),
        primitive: "note".into(),
        args: serde_json::json!({ "label": label }),
    };
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![
            sequence_reaction(
                "S",
                vec![
                    note("x", x),
                    SequenceStep {
                        id: postretro_entities::SequenceTarget::Fire,
                        primitive: "fire".into(),
                        args: serde_json::json!({ "event": "R" }),
                    },
                ],
            ),
            sequence_reaction(
                "R",
                vec![
                    note("alarm", alarm),
                    SequenceStep {
                        id: postretro_entities::SequenceTarget::Wait,
                        primitive: "wait".into(),
                        args: serde_json::json!({ "durationMs": 800 }),
                    },
                    note("moverStart", mover),
                ],
            ),
        ],
        Vec::new(),
        &[],
    );

    let reaction_reg = ReactionPrimitiveRegistry::new();
    let system_reg = SystemReactionRegistry::new();
    // Fire S: it runs `x`, then its `fire(R)` step is collected as a chained
    // name and dispatched through the deferred batch (hop depth 1).
    let chained = fire_named_event_with_sequences(
        "S",
        &data,
        &seq_reg,
        &reaction_reg,
        &system_reg,
        &script_ctx,
        None,
    );
    assert_eq!(chained, vec!["R".to_string()], "the fire step collects R");
    dispatch_deferred_named_events_with_sequences(
        chained,
        &data,
        &seq_reg,
        &reaction_reg,
        &system_reg,
        &script_ctx,
    );

    assert_eq!(
        ran.lock().unwrap().as_slice(),
        ["x".to_string(), "alarm".to_string()],
        "the body runs only up to the wait; moverStart never runs in this drain"
    );
    assert_eq!(
        *enrolled_tail_len.lock().unwrap(),
        Some(1),
        "the enrolled tail is exactly [moverStart]"
    );
}
