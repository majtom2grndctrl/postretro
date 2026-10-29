use super::*;
use log::Level;
use postretro_entities::{
    FactionDescriptor, FactionRegistry, FactionSentimentDescriptor, NumericRange, SlotOwnership,
    SlotRecord, SlotSchema, SlotType, SlotValue,
};
use postretro_foundation::IrValue;
use postretro_scripting_core::data_descriptors::{
    NamedReaction, PrimitiveDescriptor, ReactionDescriptor,
};
use postretro_scripting_core::data_registry::DataRegistry;
use postretro_test_log_capture::LogCapture;

fn number_slot(default: f32, max: f32, readonly: bool) -> SlotRecord {
    SlotRecord::new(SlotSchema {
        slot_type: SlotType::Number,
        default: Some(SlotValue::Number(default)),
        range: Some(NumericRange { min: 0.0, max }),
        persist: false,
        readonly,
        ownership: if readonly {
            SlotOwnership::Engine
        } else {
            SlotOwnership::Mod
        },
        network: Default::default(),
        per_owner: false,
        accumulate: None,
    })
}

fn insert_number(ctx: &ScriptCtx, name: &str, default: f32, max: f32, readonly: bool) {
    ctx.slot_table
        .borrow_mut()
        .insert(name.to_string(), number_slot(default, max, readonly))
        .expect("fixture slot should be vacant");
}

fn insert_per_owner_number(ctx: &ScriptCtx, name: &str, default: f32) {
    let mut record = number_slot(default, 100.0, false);
    record.schema.per_owner = true;
    ctx.slot_table
        .borrow_mut()
        .insert(name.to_string(), record)
        .expect("fixture per-owner slot should be vacant");
}

fn set_state_reaction(name: &str, slot: &str, value: serde_json::Value) -> NamedReaction {
    NamedReaction {
        name: name.to_string(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: "setState".to_string(),
            target: None,
            kind: None,
            tag: None,
            on_complete: None,
            args: serde_json::json!({ "slot": slot, "value": value }),
        }),
    }
}

fn active_reactions(reactions: Vec<NamedReaction>) -> DataRegistry {
    let mut data = DataRegistry::new();
    data.populate_level(reactions, Vec::new(), &[]);
    data
}

fn number_value(ctx: &ScriptCtx, name: &str) -> f32 {
    match ctx
        .slot_table
        .borrow()
        .get(name)
        .and_then(|record| record.value.as_ref())
    {
        Some(SlotValue::Number(value)) => *value,
        other => panic!("expected number for `{name}`, got {other:?}"),
    }
}

fn assert_number_approx_eq(actual: f32, expected: f32, message: &str) {
    assert!(
        (actual - expected).abs() <= 1.0e-5,
        "{message}: expected {expected}, got {actual}"
    );
}

fn sentiment_context() -> ScriptCtx {
    let ctx = ScriptCtx::new();
    let factions = FactionRegistry::from_descriptors(vec![
        FactionDescriptor {
            name: "cabal".to_string(),
        },
        FactionDescriptor {
            name: "resistance".to_string(),
        },
    ])
    .expect("fixture faction declarations are valid")
    .with_sentiments([FactionSentimentDescriptor {
        from_faction: "cabal".to_string(),
        to_faction: "resistance".to_string(),
        sentiment: 0.25,
        tolerance: 3.0,
        decay: None,
    }])
    .expect("fixture faction relationship is valid");
    ctx.data_registry
        .borrow_mut()
        .replace_factions(factions, &mut ctx.faction_sentiment.borrow_mut());
    ctx
}

#[test]
fn runtime_set_state_uses_derived_value_then_target_range_validation() {
    let ctx = ScriptCtx::new();
    insert_number(&ctx, "puzzle.a", 17.0, 100.0, false);
    insert_number(&ctx, "puzzle.b", 3.0, 100.0, false);
    insert_number(&ctx, "puzzle.target", 0.0, 10.0, false);
    let value = serde_json::json!({
        "op": "clamp",
        "x": {
            "op": "sub",
            "a": { "op": "input", "name": "puzzle.a" },
            "b": { "op": "input", "name": "puzzle.b" }
        },
        "lo": { "op": "const", "value": 0.0 },
        "hi": { "op": "const", "value": 100.0 }
    });
    let data = active_reactions(vec![set_state_reaction(
        "derive",
        "puzzle.target",
        value.clone(),
    )]);
    let mut bindings = SystemReactionIrBindings::default();
    bindings.rebuild(&data, &ctx);

    assert_eq!(
        bindings.dispatch("puzzle.target", &value, "named:derive", &[], &ctx),
        SystemReactionIrDispatch::Evaluated
    );
    assert_number_approx_eq(
        number_value(&ctx, "puzzle.target"),
        10.0,
        "the IR produces 14, then the target slot clamps to its [0, 10] range",
    );
}

#[test]
fn runtime_set_state_rejects_readonly_and_non_projectable_targets_at_bind() {
    let ctx = ScriptCtx::new();
    insert_number(&ctx, "puzzle.readonly", 4.0, 100.0, true);
    ctx.slot_table
        .borrow_mut()
        .insert(
            "puzzle.label".to_string(),
            SlotRecord::new(SlotSchema {
                slot_type: SlotType::String,
                default: Some(SlotValue::String("unchanged".to_string())),
                range: None,
                persist: false,
                readonly: false,
                ownership: SlotOwnership::Mod,
                network: Default::default(),
                per_owner: false,
                accumulate: None,
            }),
        )
        .expect("fixture label should be vacant");
    let value = serde_json::json!({ "op": "const", "value": 9.0 });
    let data = active_reactions(vec![
        set_state_reaction("readonly", "puzzle.readonly", value.clone()),
        set_state_reaction("string", "puzzle.label", value.clone()),
    ]);
    let mut bindings = SystemReactionIrBindings::default();
    bindings.rebuild(&data, &ctx);

    assert_eq!(
        bindings.dispatch("puzzle.readonly", &value, "named:readonly", &[], &ctx),
        SystemReactionIrDispatch::Rejected
    );
    assert_eq!(
        bindings.dispatch("puzzle.label", &value, "named:string", &[], &ctx),
        SystemReactionIrDispatch::Rejected
    );
    assert_eq!(
        bindings.dispatch("puzzle.unknown", &value, "named:unknown", &[], &ctx),
        SystemReactionIrDispatch::Unknown,
        "only commands represented at install are known rejections"
    );
    assert_number_approx_eq(
        number_value(&ctx, "puzzle.readonly"),
        4.0,
        "rejected runtime writes must leave the target unchanged",
    );
    assert!(matches!(
        ctx.slot_table
            .borrow()
            .get("puzzle.label")
            .and_then(|record| record.value.as_ref()),
        Some(SlotValue::String(value)) if value == "unchanged"
    ));
}

#[test]
fn system_set_state_rejects_per_owner_literal_and_ir_while_global_sibling_binds() {
    let ctx = ScriptCtx::new();
    insert_per_owner_number(&ctx, "currency.xp", 7.0);
    insert_number(&ctx, "currency.team", 0.0, 100.0, false);
    let literal = serde_json::json!(99.0);
    let runtime = serde_json::json!({ "op": "const", "value": 88.0 });
    let global = serde_json::json!({ "op": "const", "value": 3.0 });
    let data = active_reactions(vec![
        set_state_reaction("badLiteral", "currency.xp", literal.clone()),
        set_state_reaction("badRuntime", "currency.xp", runtime.clone()),
        set_state_reaction("goodGlobal", "currency.team", global.clone()),
    ]);
    let capture = LogCapture::start();
    let mut bindings = SystemReactionIrBindings::default();
    bindings.rebuild(&data, &ctx);

    capture.assert_logged_once(
        Level::Warn,
        "setState reaction `badLiteral` rejects per-owner slot `currency.xp`",
    );
    capture.assert_logged_once(
        Level::Warn,
        "setState reaction `badRuntime` rejects per-owner slot `currency.xp`",
    );
    assert!(bindings.rejects_literal("currency.xp", &literal));
    assert_eq!(
        bindings.dispatch("currency.xp", &runtime, "named:badRuntime", &[], &ctx),
        SystemReactionIrDispatch::Rejected,
    );
    assert_eq!(
        bindings.dispatch("currency.team", &global, "named:goodGlobal", &[], &ctx),
        SystemReactionIrDispatch::Evaluated,
    );
    assert_number_approx_eq(
        number_value(&ctx, "currency.xp"),
        7.0,
        "rejected setState paths leave the scalar projection unchanged",
    );
    assert_number_approx_eq(
        number_value(&ctx, "currency.team"),
        3.0,
        "a rejected per-owner reaction does not block a global sibling",
    );
}

#[test]
fn an_app_side_event_player_set_state_warns_once_and_skips() {
    let ctx = ScriptCtx::new();
    insert_number(&ctx, "leveling.lastHp", 7.0, 100.0, false);
    let value = serde_json::json!({ "op": "input", "name": "player.health", "owner": "@player" });
    let data = active_reactions(vec![set_state_reaction(
        "recordHp",
        "leveling.lastHp",
        value.clone(),
    )]);
    let mut bindings = SystemReactionIrBindings::default();
    bindings.rebuild(&data, &ctx);

    let capture = LogCapture::start();
    for _ in 0..2 {
        assert_eq!(
            bindings.dispatch("leveling.lastHp", &value, "ui:levelUp", &[], &ctx),
            SystemReactionIrDispatch::Rejected,
            "no app-drain source publishes the event player",
        );
    }
    capture.assert_logged_once(
        Level::Warn,
        "setState reaction `recordHp` reads `byPlayer(on.player)`, which only a player event publishes; skipped",
    );
    assert_number_approx_eq(
        number_value(&ctx, "leveling.lastHp"),
        7.0,
        "the skipped write leaves the slot unchanged",
    );
}

#[test]
fn on_complete_named_dispatch_rejects_contextless_occupancy_without_slot_mutation() {
    let ctx = ScriptCtx::new();
    insert_number(&ctx, "trap.observedOccupancy", 7.0, 16.0, false);
    let value = serde_json::json!({ "op": "input", "name": "@occupancy" });
    let data = active_reactions(vec![set_state_reaction(
        "afterPresentation",
        "trap.observedOccupancy",
        value.clone(),
    )]);
    let mut bindings = SystemReactionIrBindings::default();
    bindings.rebuild(&data, &ctx);

    assert_eq!(
        bindings.dispatch(
            "trap.observedOccupancy",
            &value,
            "named:afterPresentation",
            &[],
            &ctx,
        ),
        SystemReactionIrDispatch::Rejected,
        "an onComplete/name-dispatch source publishes no trigger DispatchScope",
    );
    assert_number_approx_eq(
        number_value(&ctx, "trap.observedOccupancy"),
        7.0,
        "contextless @occupancy must not mutate the destination slot",
    );
    assert!(
        ctx.system_commands.is_empty(),
        "a rejected contextless value must not leave a command side effect",
    );
}

#[test]
fn dispatch_inputs_require_source_membership_and_seed_without_stale_reads() {
    let ctx = ScriptCtx::new();
    insert_number(&ctx, "puzzle.target", 7.0, 10.0, false);
    let value = serde_json::json!({
        "op": "select",
        "cond": { "op": "input", "name": "@rising" },
        "a": { "op": "const", "value": 1.0 },
        "b": { "op": "const", "value": 0.0 }
    });
    let data = active_reactions(vec![set_state_reaction(
        "direction",
        "puzzle.target",
        value.clone(),
    )]);
    let mut bindings = SystemReactionIrBindings::default();
    bindings.rebuild(&data, &ctx);

    assert_eq!(
        bindings.dispatch("puzzle.target", &value, "named:empty", &[], &ctx),
        SystemReactionIrDispatch::Rejected,
        "a source publishing no dispatch values must skip before eval"
    );
    assert_number_approx_eq(
        number_value(&ctx, "puzzle.target"),
        7.0,
        "skip leaves state untouched",
    );

    assert_eq!(
        bindings.dispatch(
            "puzzle.target",
            &value,
            "crossing:0",
            &[("@rising".to_string(), IrValue::Bool(true))],
            &ctx,
        ),
        SystemReactionIrDispatch::Evaluated
    );
    assert_number_approx_eq(
        number_value(&ctx, "puzzle.target"),
        1.0,
        "rising seed is observed",
    );

    assert_eq!(
        bindings.dispatch(
            "puzzle.target",
            &value,
            "crossing:0",
            &[("@rising".to_string(), IrValue::Bool(false))],
            &ctx,
        ),
        SystemReactionIrDispatch::Evaluated
    );
    assert_number_approx_eq(
        number_value(&ctx, "puzzle.target"),
        0.0,
        "falling seed replaces prior value",
    );
}

#[test]
fn missing_dispatch_input_warnings_key_by_program_and_canonical_source() {
    let ctx = ScriptCtx::new();
    insert_number(&ctx, "puzzle.target", 7.0, 10.0, false);
    let value = serde_json::json!({
        "op": "select",
        "cond": { "op": "input", "name": "@rising" },
        "a": { "op": "const", "value": 1.0 },
        "b": { "op": "const", "value": 0.0 }
    });
    let data = active_reactions(vec![set_state_reaction(
        "direction",
        "puzzle.target",
        value.clone(),
    )]);
    let mut bindings = SystemReactionIrBindings::default();
    bindings.rebuild(&data, &ctx);

    for source in ["named:levelLoad", "named:doorOpened", "crossing:0"] {
        assert_eq!(
            bindings.dispatch("puzzle.target", &value, source, &[], &ctx),
            SystemReactionIrDispatch::Rejected
        );
    }
    assert_eq!(bindings.warned_missing_inputs.borrow().len(), 3);

    // Published-input ordering is membership data, not source identity.
    // Repeating one source with order variants must retain one warning key.
    for values in [
        vec![
            ("@unused_b".to_string(), IrValue::Bool(false)),
            ("@unused_a".to_string(), IrValue::Bool(false)),
        ],
        vec![
            ("@unused_a".to_string(), IrValue::Bool(false)),
            ("@unused_b".to_string(), IrValue::Bool(false)),
        ],
    ] {
        assert_eq!(
            bindings.dispatch("puzzle.target", &value, "named:doorOpened", &values, &ctx),
            SystemReactionIrDispatch::Rejected
        );
    }
    assert_eq!(bindings.warned_missing_inputs.borrow().len(), 3);
}

#[test]
fn registers_exactly_the_classified_system_reaction_kinds() {
    use postretro_entities::reactions::system_commands::SystemReactionKind;
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    for name in r.names() {
        assert!(
            SystemReactionKind::from_primitive_name(name).is_some(),
            "system reaction `{name}` is registered but not classified"
        );
    }
    for kind in SystemReactionKind::ALL {
        assert!(
            r.contains(kind.primitive_name()),
            "classified system reaction `{}` is not registered",
            kind.primitive_name()
        );
    }
    // Defensive: system reactions are a distinct arm; entity primitives
    // are NOT registered here.
    assert!(!r.contains("setEmitterRate"));
}

#[test]
fn play_sound_dispatch_enqueues_command_with_bus() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "sound": "alarm", "bus": "sfx" });
    assert!(r.dispatch("playSound", &args, &queue).unwrap());

    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::PlaySound {
            audience: None,
            sound: "alarm".to_string(),
            bus: Some("sfx".to_string()),
            at: None,
        }]
    );
}

#[test]
fn play_sound_dispatch_defaults_absent_bus_to_none() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "sound": "alarm" });
    assert!(r.dispatch("playSound", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::PlaySound {
            audience: None,
            sound: "alarm".to_string(),
            bus: None,
            at: None,
        }]
    );
}

#[test]
fn rumble_dispatch_enqueues_command() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "strong": 0.8, "durationMs": 200.0 });
    assert!(r.dispatch("rumble", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::Rumble {
            audience: None,
            strong: 0.8,
            weak: None,
            duration_ms: 200.0,
        }]
    );
}

#[test]
fn vignette_dispatch_enqueues_command_with_color() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args =
        serde_json::json!({ "color": [0.1, 0.0, 0.2], "strength": 0.8, "durationMs": 300.0 });
    assert!(r.dispatch("vignette", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::Vignette {
            audience: None,
            color: Some([0.1, 0.0, 0.2]),
            strength: 0.8,
            duration_ms: 300.0,
        }]
    );
}

#[test]
fn vignette_dispatch_defaults_absent_color_to_none() {
    // The command carries `None`; the drain applies the black default when it
    // maps onto `VignetteDecay::start`.
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "strength": 0.5, "durationMs": 200.0 });
    assert!(r.dispatch("vignette", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::Vignette {
            audience: None,
            color: None,
            strength: 0.5,
            duration_ms: 200.0,
        }]
    );
}

#[test]
fn screen_shake_dispatch_enqueues_command_with_frequency() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "amplitude": 12.0, "durationMs": 250.0, "frequency": 24.0 });
    assert!(r.dispatch("screenShake", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::ScreenShake {
            audience: None,
            amplitude: 12.0,
            duration_ms: 250.0,
            frequency: Some(24.0),
        }]
    );
}

#[test]
fn screen_shake_dispatch_passes_absent_frequency_through_as_none() {
    // The omitted-frequency default is the DRIVER's job (18 Hz); the
    // deserializer carries `None` through unchanged.
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "amplitude": 8.0, "durationMs": 150.0 });
    assert!(r.dispatch("screenShake", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::ScreenShake {
            audience: None,
            amplitude: 8.0,
            duration_ms: 150.0,
            frequency: None,
        }]
    );
}

#[test]
fn show_dialog_dispatch_enqueues_push_tree_with_on_commit() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "tree": "pauseMenu", "onCommit": "resumeGame" });
    assert!(r.dispatch("showDialog", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::PushTree {
            tree: "pauseMenu".to_string(),
            on_commit: Some("resumeGame".to_string()),
        }]
    );
}

#[test]
fn show_dialog_defaults_absent_on_commit_to_none() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "tree": "hint" });
    assert!(r.dispatch("showDialog", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::PushTree {
            tree: "hint".to_string(),
            on_commit: None,
        }]
    );
}

#[test]
fn open_menu_dispatch_enqueues_push_tree_without_on_commit() {
    // `openMenu` is a v1 alias of `showDialog` that never carries onCommit —
    // a menu has no commit payload, so the alias drops any onCommit key.
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "tree": "mainMenu" });
    assert!(r.dispatch("openMenu", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::PushTree {
            tree: "mainMenu".to_string(),
            on_commit: None,
        }]
    );
}

#[test]
fn close_dialog_dispatch_enqueues_pop_tree() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    assert!(
        r.dispatch("closeDialog", &serde_json::json!({}), &queue)
            .unwrap()
    );
    assert_eq!(queue.take(), vec![SystemReactionCommand::PopTree]);
}

#[test]
fn load_level_dispatch_reads_map_key() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "map": "e1m1" });
    assert!(r.dispatch("loadLevel", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::LoadLevel {
            map: "e1m1".to_string(),
        }]
    );
}

#[test]
fn restart_level_dispatch_enqueues_argumentless_command() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    assert!(
        r.dispatch("restartLevel", &serde_json::json!({}), &queue)
            .unwrap()
    );
    assert_eq!(queue.take(), vec![SystemReactionCommand::RestartLevel]);
}

#[test]
fn return_to_frontend_dispatch_enqueues_argumentless_command() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    assert!(
        r.dispatch("returnToFrontend", &serde_json::json!({}), &queue)
            .unwrap()
    );
    assert_eq!(queue.take(), vec![SystemReactionCommand::ReturnToFrontend]);
}

#[test]
fn set_state_dispatch_enqueues_command_with_slot_and_value() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "slot": "audio.master", "value": 0.5 });
    assert!(r.dispatch("setState", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::SetState {
            slot: "audio.master".to_string(),
            value: serde_json::json!(0.5),
            dispatch_source: String::new(),
            dispatch_values: Vec::new(),
        }]
    );
}

#[test]
fn sentiment_reactions_deserialize_camel_case_args_and_queue_typed_commands() {
    let mut registry = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut registry);
    let queue = SystemCommandQueue::new();

    assert!(
        registry
            .dispatch(
                "setSentiment",
                &serde_json::json!({ "from": "cabal", "to": "resistance", "value": -0.75 }),
                &queue,
            )
            .expect("setSentiment args are valid")
    );
    assert!(
        registry
            .dispatch(
                "adjustSentiment",
                &serde_json::json!({ "from": "resistance", "to": "cabal", "delta": 0.5 }),
                &queue,
            )
            .expect("adjustSentiment args are valid")
    );

    let commands = queue.take();
    let [
        SystemReactionCommand::SetSentiment { from, to, value },
        SystemReactionCommand::AdjustSentiment {
            from: reverse_from,
            to: reverse_to,
            delta,
        },
    ] = commands.as_slice()
    else {
        panic!("sentiment reactions must queue set then adjust commands");
    };
    assert_eq!(from, "cabal");
    assert_eq!(to, "resistance");
    assert_eq!(reverse_from, "resistance");
    assert_eq!(reverse_to, "cabal");
    assert_number_approx_eq(*value, -0.75, "setSentiment value must round-trip");
    assert_number_approx_eq(*delta, 0.5, "adjustSentiment delta must round-trip");
}

#[test]
fn sentiment_drain_writes_only_directional_live_overlay_and_adjusts_from_live_value() {
    let ctx = sentiment_context();

    apply_set_sentiment(&ctx, "cabal", "resistance", -0.75)
        .expect("known factions and finite value apply");
    assert_number_approx_eq(
        ctx.faction_sentiment
            .borrow()
            .get(2.0, 3.0)
            .expect("directional live overlay entry is present"),
        -0.75,
        "set sentiment writes the requested live value",
    );
    assert_eq!(
        ctx.faction_sentiment.borrow().get(3.0, 2.0),
        None,
        "a directional write must not create the reverse pair"
    );
    assert_number_approx_eq(
        ctx.data_registry.borrow().factions.sentiment(2.0, 3.0),
        0.25,
        "the authored baseline remains immutable",
    );

    apply_adjust_sentiment(&ctx, "cabal", "resistance", 0.5)
        .expect("adjustment applies from the live overlay value");
    assert_number_approx_eq(
        ctx.faction_sentiment
            .borrow()
            .get(2.0, 3.0)
            .expect("directional live overlay entry remains present"),
        -0.25,
        "adjustment starts from the live overlay value",
    );

    let reverse_baseline = ctx.data_registry.borrow().factions.sentiment(3.0, 2.0);
    apply_adjust_sentiment(&ctx, "resistance", "cabal", -0.5)
        .expect("an unlisted directional pair starts from its authored baseline");
    assert_number_approx_eq(
        ctx.faction_sentiment
            .borrow()
            .get(3.0, 2.0)
            .expect("reverse live overlay entry is present"),
        reverse_baseline - 0.5,
        "an unlisted reverse pair gets its own live overlay entry",
    );

    apply_set_sentiment(&ctx, "cabal", "resistance", 0.25).expect("returning to baseline is valid");
    assert_eq!(
        ctx.faction_sentiment.borrow().get(2.0, 3.0),
        None,
        "a pair at authored baseline must be sparse again"
    );
}

#[test]
fn sentiment_drain_rejects_unknown_faction_names_without_mutating_overlay() {
    let ctx = sentiment_context();

    let error = apply_adjust_sentiment(&ctx, "unknown", "resistance", -1.0)
        .expect_err("unknown faction names must degrade to a no-op");
    assert!(error.contains("unknown from faction `unknown`"));
    assert!(ctx.faction_sentiment.borrow().iter().next().is_none());
}

#[test]
fn sentiment_drain_prevents_finite_adjustment_overflow_without_clamping() {
    let ctx = sentiment_context();
    apply_set_sentiment(&ctx, "cabal", "resistance", f32::MAX)
        .expect("a finite sentiment is accepted without a numeric clamp");

    let error = apply_adjust_sentiment(&ctx, "cabal", "resistance", f32::MAX)
        .expect_err("the frame-end writer reports its skipped overflow to the app drain");
    assert!(
        error.contains("sentiment value and baseline must be finite"),
        "the finite addition overflow must surface as a rejected non-finite result: {error}"
    );
    assert_number_approx_eq(
        ctx.faction_sentiment
            .borrow()
            .get(2.0, 3.0)
            .expect("live overlay entry remains present after overflow"),
        f32::MAX,
        "a finite adjustment that would overflow cannot commit non-finite live state",
    );
}

#[test]
fn connected_client_sentiment_reaction_validates_but_does_not_write_overlay() {
    let ctx = sentiment_context();
    ctx.owner_slot_writes_enabled.set(false);

    apply_set_sentiment(&ctx, "cabal", "resistance", -0.75)
        .expect("a client-local reaction remains a valid evaluated command");
    assert!(
        ctx.faction_sentiment.borrow().iter().next().is_none(),
        "the connected-client authority gate suppresses the local overlay write"
    );
}

#[test]
fn set_state_stamps_active_fire_context_without_widening_handler() {
    let mut registry = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut registry);
    let queue = SystemCommandQueue::new();
    queue.replace_fire_context(postretro_entities::SystemCommandFireContext {
        source: "crossing:7".to_string(),
        values: vec![("@rising".to_string(), IrValue::Bool(true))],
        emitter: None,
        origin: postretro_entities::FireOrigin::EveryMachine(
            postretro_entities::MachineSource::HostMirrored,
        ),
    });

    registry
        .dispatch(
            "setState",
            &serde_json::json!({ "slot": "puzzle.direction", "value": { "op": "input", "name": "@rising" } }),
            &queue,
        )
        .unwrap();

    assert!(matches!(
        queue.take().as_slice(),
        [SystemReactionCommand::SetState { dispatch_source, dispatch_values, .. }]
            if dispatch_source == "crossing:7"
                && dispatch_values == &vec![("@rising".to_string(), IrValue::Bool(true))]
    ));
}

#[test]
fn literal_set_state_keeps_the_existing_json_write_path() {
    let ctx = ScriptCtx::new();
    insert_number(&ctx, "puzzle.literal", 0.0, 10.0, false);
    let mut registry = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut registry);
    let queue = SystemCommandQueue::new();

    registry
        .dispatch(
            "setState",
            &serde_json::json!({ "slot": "puzzle.literal", "value": 5 }),
            &queue,
        )
        .expect("literal setState must dispatch");
    let command = queue.take().pop().expect("literal command must queue");
    let SystemReactionCommand::SetState { slot, value, .. } = command else {
        panic!("literal setState must retain its system command shape");
    };
    assert!(!is_ir_node(&value), "a literal must not enter the IR path");
    crate::scripting::primitives::store::write_state_slot_json(&ctx, &slot, &value)
        .expect("literal setState must retain the shipped JSON write path");

    assert_number_approx_eq(
        number_value(&ctx, "puzzle.literal"),
        5.0,
        "literal setState must retain the shipped JSON write path",
    );
}

fn anchored_play_sound() -> serde_json::Value {
    serde_json::json!({ "sound": "sfx/thud", "at": postretro_entities::EMITTER_AT_TOKEN })
}

fn fire_context(
    source: &str,
    emitter: Option<postretro_entities::Emitter>,
) -> postretro_entities::SystemCommandFireContext {
    postretro_entities::SystemCommandFireContext {
        source: source.to_string(),
        values: Vec::new(),
        emitter,
        origin: postretro_entities::FireOrigin::EveryMachine(
            postretro_entities::MachineSource::HostMirrored,
        ),
    }
}

// A named gameplay event publishes its emitter; `at: on.emitter` plays there.
#[test]
fn play_sound_at_emitter_takes_the_firing_sources_emitter() {
    let mut registry = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut registry);
    let queue = SystemCommandQueue::new();
    let emitter = postretro_entities::Emitter::Entity {
        id: postretro_entities::EntityId::from_raw(4),
        origin: glam::Vec3::new(1.0, 2.0, 3.0),
    };
    queue.replace_fire_context(fire_context("named:activate", Some(emitter.clone())));

    registry
        .dispatch("playSound", &anchored_play_sound(), &queue)
        .expect("anchored playSound dispatches");
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::PlaySound {
            audience: None,
            sound: "sfx/thud".to_string(),
            bus: None,
            at: Some(emitter),
        }],
    );
}

// Pins P7: level load, crossings, triggers, deaths and follow-ups publish no
// emitter. The reaction is skipped with one warning per source; it never
// plays dry.
#[test]
fn play_sound_at_emitter_without_one_skips_and_warns_once_per_source() {
    let mut registry = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut registry);
    let queue = SystemCommandQueue::new();
    let capture = LogCapture::start();

    for source in ["named:levelLoad", "named:levelLoad", "crossing:3"] {
        queue.replace_fire_context(fire_context(source, None));
        registry
            .dispatch("playSound", &anchored_play_sound(), &queue)
            .expect("a skipped playSound is not an error");
    }
    // This fixture explicitly resets to the default context, which has no emitter.
    queue.replace_fire_context(postretro_entities::SystemCommandFireContext::default());
    registry
        .dispatch("playSound", &anchored_play_sound(), &queue)
        .expect("a skipped playSound is not an error");

    assert!(
        queue.take().is_empty(),
        "nothing plays, not even unpositioned"
    );
    capture.assert_logged_once(
        log::Level::Warn,
        "playSound `sfx/thud` reads `on.emitter`, but source `named:levelLoad` publishes no emitter",
    );
    capture.assert_logged_once(log::Level::Warn, "source `crossing:3` publishes no emitter");
}

#[test]
fn play_sound_rejects_an_at_other_than_the_emitter_token() {
    let mut registry = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut registry);
    let queue = SystemCommandQueue::new();
    let error = registry
        .dispatch(
            "playSound",
            &serde_json::json!({ "sound": "sfx/thud", "at": "@activators" }),
            &queue,
        )
        .expect_err("only the emitter token is an anchor");
    assert!(error.to_string().contains("`at` must be `on.emitter`"));
    assert!(queue.take().is_empty());
}

#[test]
fn set_state_carries_arbitrary_json_value_shapes() {
    // The command is type-agnostic at the queue layer; the drain's write path
    // coerces to the slot's declared type. A string, bool, and array all ride.
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();
    r.dispatch(
        "setState",
        &serde_json::json!({ "slot": "ui.label", "value": "hi" }),
        &queue,
    )
    .unwrap();
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::SetState {
            slot: "ui.label".to_string(),
            value: serde_json::json!("hi"),
            dispatch_source: String::new(),
            dispatch_values: Vec::new(),
        }]
    );
}

#[test]
fn cell_write_dispatch_enqueues_command_with_scope_cell_and_value() {
    // The `ui.createLocalState().set(v)` path: distinct from `setState`, it
    // carries a scope id + cell name (NOT a slot) and rides into the
    // presentation-cell store, never the slot table.
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "scope": "counter", "cell": "count", "value": 5 });
    assert!(r.dispatch("cellWrite", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::CellWrite {
            scope: "counter".to_string(),
            cell: "count".to_string(),
            value: serde_json::json!(5),
        }]
    );
}

#[test]
fn append_text_dispatch_enqueues_command_with_slot_and_text() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "slot": "ui.textEntry", "text": "ab" });
    assert!(r.dispatch("appendText", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::AppendText {
            slot: "ui.textEntry".to_string(),
            text: "ab".to_string(),
        }]
    );
}

#[test]
fn backspace_text_dispatch_enqueues_command_with_slot() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "slot": "ui.textEntry" });
    assert!(r.dispatch("backspaceText", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::BackspaceText {
            slot: "ui.textEntry".to_string(),
        }]
    );
}

#[test]
fn clear_text_dispatch_enqueues_command_with_slot() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "slot": "ui.textEntry" });
    assert!(r.dispatch("clearText", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::ClearText {
            slot: "ui.textEntry".to_string(),
        }]
    );
}

#[test]
fn dispatch_unknown_name_returns_false() {
    let r = SystemReactionRegistry::new();
    let queue = SystemCommandQueue::new();
    assert!(
        !r.dispatch("noSuchReaction", &serde_json::Value::Null, &queue)
            .unwrap()
    );
    assert!(queue.is_empty());
}

#[test]
fn invalid_args_surface_as_invalid_argument() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    // `playSound` requires `sound`; an empty object fails deserialization.
    let err = r
        .dispatch("playSound", &serde_json::json!({}), &queue)
        .unwrap_err();
    assert!(matches!(err, ReactionError::InvalidArgument { .. }));
    assert!(queue.is_empty());
}

#[test]
fn take_leaves_queue_empty() {
    let queue = SystemCommandQueue::new();
    queue.push(SystemReactionCommand::PopTree);
    assert!(!queue.is_empty());
    let _ = queue.take();
    assert!(queue.is_empty());
}

// --- vignette durationMs validation -------------------------------------
//
// The nan/inf tests below assert that the bad arg IS rejected — that is the
// guarantee under test. The rejection layer differs by arg source: the JSON
// bridge (serde_json::Number::from_f64) maps non-finite f32 to null, so
// NaN/Infinity arrive as null and are refused at deserialize ("invalid type:
// null, expected f32") before the is_finite() guard runs. The is_finite()
// arm is defense-in-depth for any future non-JSON caller. The tests remain
// valuable as rejection guarantees regardless of which layer fires.

#[test]
fn vignette_nan_duration_ms_rejected_with_invalid_argument() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "strength": 0.8, "durationMs": f32::NAN });
    let err = r.dispatch("vignette", &args, &queue).unwrap_err();
    assert!(matches!(err, ReactionError::InvalidArgument { .. }));
    assert!(queue.is_empty());
}

#[test]
fn vignette_inf_duration_ms_rejected_with_invalid_argument() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "strength": 0.5, "durationMs": f32::INFINITY });
    let err = r.dispatch("vignette", &args, &queue).unwrap_err();
    assert!(matches!(err, ReactionError::InvalidArgument { .. }));
    assert!(queue.is_empty());
}

#[test]
fn vignette_zero_duration_ms_rejected_with_invalid_argument() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "strength": 0.5, "durationMs": 0.0 });
    let err = r.dispatch("vignette", &args, &queue).unwrap_err();
    assert!(matches!(err, ReactionError::InvalidArgument { .. }));
    assert!(queue.is_empty());
}

#[test]
fn vignette_negative_duration_ms_rejected_with_invalid_argument() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "strength": 0.5, "durationMs": -100.0 });
    let err = r.dispatch("vignette", &args, &queue).unwrap_err();
    assert!(matches!(err, ReactionError::InvalidArgument { .. }));
    assert!(queue.is_empty());
}

#[test]
fn vignette_valid_args_enqueues_command() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "strength": 0.7, "durationMs": 400.0 });
    assert!(r.dispatch("vignette", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::Vignette {
            audience: None,
            color: None,
            strength: 0.7,
            duration_ms: 400.0,
        }]
    );
}

// --- screenShake durationMs, frequency, and amplitude validation ---------
//
// The nan/inf tests below assert that the bad arg IS rejected — that is the
// guarantee under test. The rejection layer differs by arg source: the JSON
// bridge (serde_json::Number::from_f64) maps non-finite f32 to null, so
// NaN/Infinity arrive as null and are refused at deserialize ("invalid type:
// null, expected f32") before the is_finite() guard runs. The is_finite()
// arm is defense-in-depth for any future non-JSON caller. The tests remain
// valuable as rejection guarantees regardless of which layer fires.

#[test]
fn screen_shake_nan_duration_ms_rejected_with_invalid_argument() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "amplitude": 10.0, "durationMs": f32::NAN });
    let err = r.dispatch("screenShake", &args, &queue).unwrap_err();
    assert!(matches!(err, ReactionError::InvalidArgument { .. }));
    assert!(queue.is_empty());
}

#[test]
fn screen_shake_inf_duration_ms_rejected_with_invalid_argument() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "amplitude": 10.0, "durationMs": f32::INFINITY });
    let err = r.dispatch("screenShake", &args, &queue).unwrap_err();
    assert!(matches!(err, ReactionError::InvalidArgument { .. }));
    assert!(queue.is_empty());
}

#[test]
fn screen_shake_zero_duration_ms_rejected_with_invalid_argument() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "amplitude": 10.0, "durationMs": 0.0 });
    let err = r.dispatch("screenShake", &args, &queue).unwrap_err();
    assert!(matches!(err, ReactionError::InvalidArgument { .. }));
    assert!(queue.is_empty());
}

#[test]
fn screen_shake_negative_duration_ms_rejected_with_invalid_argument() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "amplitude": 10.0, "durationMs": -50.0 });
    let err = r.dispatch("screenShake", &args, &queue).unwrap_err();
    assert!(matches!(err, ReactionError::InvalidArgument { .. }));
    assert!(queue.is_empty());
}

#[test]
fn screen_shake_negative_frequency_rejected_with_invalid_argument() {
    // JSON cannot represent f32::INFINITY (serde_json::json! maps it to null,
    // which deserializes to None — the absent-frequency path — rather than a
    // non-finite value). A negative frequency is the JSON-reachable equivalent:
    // it hits the `freq <= 0.0` guard and is a realistic authoring error.
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "amplitude": 10.0, "durationMs": 200.0, "frequency": -1.0 });
    let err = r.dispatch("screenShake", &args, &queue).unwrap_err();
    assert!(matches!(err, ReactionError::InvalidArgument { .. }));
    assert!(queue.is_empty());
}

#[test]
fn screen_shake_zero_frequency_rejected_with_invalid_argument() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "amplitude": 10.0, "durationMs": 200.0, "frequency": 0.0 });
    let err = r.dispatch("screenShake", &args, &queue).unwrap_err();
    assert!(matches!(err, ReactionError::InvalidArgument { .. }));
    assert!(queue.is_empty());
}

#[test]
fn screen_shake_valid_args_enqueues_command() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "amplitude": 6.0, "durationMs": 300.0, "frequency": 20.0 });
    assert!(r.dispatch("screenShake", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::ScreenShake {
            audience: None,
            amplitude: 6.0,
            duration_ms: 300.0,
            frequency: Some(20.0),
        }]
    );
}

#[test]
fn screen_shake_negative_amplitude_rejected_with_invalid_argument() {
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "amplitude": -1.0, "durationMs": 200.0 });
    let err = r.dispatch("screenShake", &args, &queue).unwrap_err();
    assert!(matches!(err, ReactionError::InvalidArgument { .. }));
    assert!(queue.is_empty());
}

#[test]
fn screen_shake_amplitude_above_max_rejected_with_invalid_argument() {
    // Amplitude above MAX_SHAKE_AMPLITUDE_PX (1280.0 px) produces a UV
    // offset > 1.0 that causes whole-frame ClampToEdge edge-smear.
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "amplitude": 1281.0, "durationMs": 200.0 });
    let err = r.dispatch("screenShake", &args, &queue).unwrap_err();
    assert!(matches!(err, ReactionError::InvalidArgument { .. }));
    assert!(queue.is_empty());
}

#[test]
fn screen_shake_zero_amplitude_accepted_enqueues_command() {
    // amplitude == 0.0 is a valid no-op shake (zero displacement).
    let mut r = SystemReactionRegistry::new();
    register_system_reaction_primitives(&mut r);
    let queue = SystemCommandQueue::new();

    let args = serde_json::json!({ "amplitude": 0.0, "durationMs": 200.0 });
    assert!(r.dispatch("screenShake", &args, &queue).unwrap());
    assert_eq!(
        queue.take(),
        vec![SystemReactionCommand::ScreenShake {
            audience: None,
            amplitude: 0.0,
            duration_ms: 200.0,
            frequency: None,
        }]
    );
}
