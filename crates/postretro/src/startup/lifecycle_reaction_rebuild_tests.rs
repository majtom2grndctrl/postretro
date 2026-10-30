//! Active reaction rebuild tests: system command bindings, staged recompose,
//! crossing filters, and audience and `on.player` gates.
//! See: context/lib/boot_sequence.md §3 · context/lib/scripting.md §2

use super::lifecycle_test_fixtures::*;
use super::*;

use postretro_entities::SystemReactionCommand;
use postretro_entities::{
    CrossingCondition, CrossingDescriptor, NamedReaction, PrimitiveDescriptor, ProgressDescriptor,
    ReactionDescriptor, TriggerActivation, TriggerCommand, TriggerEventDescriptor, TriggerFireMode,
    TriggerVolumeComponent,
};
use postretro_entities::{
    ScriptCtx, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue, Transform,
};

use postretro_scripting_core::reaction_dispatch::ProgressTracker;

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

fn scoped_global_progress(name: &str, tag: &str, fire: &str) -> postretro_entities::ScopedReaction {
    postretro_entities::ScopedReaction {
        reaction: progress_reaction(name, tag, 1.0, fire),
        levels: Vec::new(),
    }
}

fn scoped_global_set_state(name: &str, value: f32) -> postretro_entities::ScopedReaction {
    postretro_entities::ScopedReaction {
        reaction: NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "setState".to_string(),
                target: None,
                kind: None,
                tag: None,
                on_complete: None,
                args: serde_json::json!({ "slot": "trigger.flag", "value": value }),
            }),
        },
        levels: Vec::new(),
    }
}

fn scoped_global_crossing(slot: &str, fire: &str) -> postretro_entities::ScopedCrossing {
    postretro_entities::ScopedCrossing {
        crossing: CrossingDescriptor {
            slot: Some(slot.to_string()),
            condition: CrossingCondition::Below { threshold: 0.5 },
            max: 100.0,
            edge: None,
            fire: vec![fire.to_string()],
        },
        levels: Vec::new(),
    }
}

fn number_slot(value: f32) -> SlotRecord {
    let mut record = SlotRecord::new(SlotSchema {
        slot_type: SlotType::Number,
        default: None,
        range: None,
        persist: false,
        readonly: false,
        ownership: SlotOwnership::Mod,
        network: postretro_entities::ReplicationScope::None,
        per_owner: false,
        accumulate: None,
    });
    record.value = Some(SlotValue::Number(value));
    record
}

#[test]
fn app_system_command_drain_accumulates_bound_runtime_set_state_writes_in_queue_order() {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    ctx.slot_table
        .borrow_mut()
        .insert("puzzle.count".to_string(), number_slot(0.0))
        .expect("fixture slot should be vacant");
    let increment = serde_json::json!({
        "op": "add",
        "a": { "op": "input", "name": "puzzle.count" },
        "b": { "op": "const", "value": 1.0 }
    });
    ctx.data_registry.borrow_mut().populate_level(
        vec![NamedReaction {
            name: "increment".to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "setState".to_string(),
                target: None,
                kind: None,
                tag: None,
                on_complete: None,
                args: serde_json::json!({
                    "slot": "puzzle.count",
                    "value": increment.clone(),
                }),
            }),
        }],
        Vec::new(),
        &[],
    );
    app.rebuild_active_system_reaction_bindings();

    // Exercise App::dispatch_system_commands, the production app-side
    // drain. The session-owned table must evaluate both installed programs
    // in FIFO order so the second read observes the first write.
    for _ in 0..2 {
        ctx.system_commands.push(SystemReactionCommand::SetState {
            slot: "puzzle.count".to_string(),
            value: increment.clone(),
            dispatch_source: "test.levelLoad".to_string(),
            dispatch_values: Vec::new(),
        });
    }
    app.dispatch_system_commands();

    let count = match ctx
        .slot_table
        .borrow()
        .get("puzzle.count")
        .and_then(|record| record.value.as_ref())
    {
        Some(SlotValue::Number(value)) => *value,
        other => panic!("expected numeric puzzle count, got {other:?}"),
    };
    assert!(
        (count - 2.0).abs() <= 1.0e-5,
        "two self-referential IR writes must accumulate at the app drain: got {count}"
    );
}

#[test]
fn staged_commit_guard_does_not_recompose_when_no_level_is_installed() {
    let mut app = test_app();
    app.boot_state = BootState::Frontend;
    app.level = None;
    app.active_level_tags.clear();
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_global_reactions(vec![scoped_global_progress("waveDone", "wave1", "powerOn")]);
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_global_crossings(vec![scoped_global_crossing("test.health", "healthLow")]);

    if app.has_installed_level() {
        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .recompose_active_sets(&app.active_level_tags);
        app.rebuild_active_reaction_subscribers();
    }

    let ctx = script_ctx(&app);
    let registry = ctx.data_registry.borrow();
    assert!(
        registry.reactions.is_empty(),
        "unscoped globals must not repopulate active reactions after unload",
    );
    assert!(
        registry.crossings.is_empty(),
        "unscoped globals must not repopulate active crossings after unload",
    );
}

#[test]
fn staged_commit_rebuilds_active_subscribers_for_installed_raw_path_level() {
    let mut app = test_app();
    app.boot_state = BootState::Running;
    app.level = Some(level_world("raw_dev_level", 1));
    app.active_level_tags.clear();
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_global_reactions(vec![scoped_global_progress("waveDone", "wave1", "powerOn")]);
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_global_crossings(vec![scoped_global_crossing("test.health", "healthLow")]);
    let wave_member = {
        let ctx = script_ctx(&app);
        let mut entities = ctx.registry.borrow_mut();
        let id = entities.spawn(Transform::default());
        entities.set_tags(id, vec!["wave1".to_string()]).unwrap();
        id
    };
    script_ctx(&app)
        .slot_table
        .borrow_mut()
        .insert("test.health".to_string(), number_slot(75.0))
        .expect("test slot should be vacant");
    // Level install, in production order: the subscriber rebuild, then the
    // progress membership capture once the map placements exist. A
    // recompose keeps that membership and only re-subscribes.
    {
        let ctx = script_ctx(&app);
        ctx.data_registry
            .borrow_mut()
            .recompose_active_sets(&app.active_level_tags);
        let session = app.session.as_mut().expect("test app session installed");
        rebuild_reaction_subscribers(
            &mut session.progress_tracker,
            &mut session.crossing_detector,
            &ctx,
            SubscriberRebuild::LevelInstall,
        );
        assert!(
            capture_progress_membership(&mut session.progress_tracker, &ctx).is_empty(),
            "one living member is below `at: 1.0`"
        );
    }

    if app.has_installed_level() {
        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .recompose_active_sets(&app.active_level_tags);
        app.rebuild_active_reaction_subscribers();
    }
    assert!(
        app.session
            .as_ref()
            .expect("test app session installed")
            .pending_death_events
            .is_empty(),
        "a recompose over an unmet threshold queues no progress fire"
    );

    assert_eq!(
        app.session
            .as_mut()
            .expect("test app session installed")
            .progress_tracker
            .on_entity_killed(wave_member),
        vec!["powerOn".to_string()],
    );
    script_ctx(&app)
        .slot_table
        .borrow_mut()
        .get_mut("test.health")
        .expect("test slot should exist")
        .value = Some(SlotValue::Number(25.0));
    let ctx = script_ctx(&app);
    assert_eq!(
        app.session
            .as_mut()
            .expect("test app session installed")
            .crossing_detector
            .detect(&ctx.slot_table.borrow()),
        vec!["healthLow".to_string()],
    );
}

// Regression: staged mod-init reload omitted manifest trigger events, leaving
// script-authored bindings absent while brush KVP bindings were restored.
#[test]
fn staged_recomposition_rebuilds_brush_and_manifest_trigger_bindings() {
    let mut app = test_app();
    app.level = Some(level_world("trigger_reload_level", 1));
    let trigger = {
        let ctx = script_ctx(&app);
        let mut entities = ctx.registry.borrow_mut();
        let id = entities.spawn(Transform::default());
        entities
            .set_component(
                id,
                TriggerVolumeComponent::new(
                    TriggerActivation::Touch,
                    String::new(),
                    "plate_pressed".to_string(),
                    String::new(),
                    TriggerCommand::Start,
                    TriggerFireMode::Multiple,
                    0.0,
                    true,
                ),
            )
            .expect("trigger component attaches");
        entities
            .set_tags(id, vec!["trap".to_string()])
            .expect("trigger tags attach");
        id
    };
    script_ctx(&app)
        .slot_table
        .borrow_mut()
        .insert("trigger.flag".to_string(), number_slot(0.0))
        .expect("trigger fixture slot should be vacant");

    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_global_reactions(vec![
            scoped_global_set_state("plate_pressed", 1.0),
            scoped_global_set_state("script_pressed", 10.0),
        ]);
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_global_trigger_events(vec![TriggerEventDescriptor {
            tag: "trap".to_string(),
            event: "enter".to_string(),
            fire: vec!["script_pressed".to_string()],
            levels: Vec::new(),
        }]);
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .recompose_active_sets(&app.active_level_tags);
    app.rebuild_active_reaction_subscribers();
    app.rebuild_active_trigger_bindings();
    {
        let ctx = script_ctx(&app);
        let mut entities = ctx.registry.borrow_mut();
        let mut slots = ctx.slot_table.borrow_mut();
        assert!(
            app.trigger_bindings
                .execute(
                    trigger,
                    crate::trigger_system::TriggerEventEdge::Enter,
                    &mut entities,
                    &mut slots,
                    &crate::trigger_commands::TriggerFireContext::default(),
                )
                .residual()
                .is_none(),
            "the direct bindings have no app-side residual"
        );
    }
    assert_eq!(
        script_ctx(&app)
            .slot_table
            .borrow()
            .get("trigger.flag")
            .and_then(|record| record.value.clone()),
        Some(SlotValue::Number(10.0)),
        "brush KVP binding runs before the appended manifest binding",
    );

    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_global_reactions(vec![
            scoped_global_set_state("plate_pressed", 2.0),
            scoped_global_set_state("script_pressed", 20.0),
        ]);
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .recompose_active_sets(&app.active_level_tags);
    app.rebuild_active_reaction_subscribers();
    app.rebuild_active_trigger_bindings();
    {
        let ctx = script_ctx(&app);
        let mut entities = ctx.registry.borrow_mut();
        let mut slots = ctx.slot_table.borrow_mut();
        assert!(
            app.trigger_bindings
                .execute(
                    trigger,
                    crate::trigger_system::TriggerEventEdge::Enter,
                    &mut entities,
                    &mut slots,
                    &crate::trigger_commands::TriggerFireContext::default(),
                )
                .residual()
                .is_none(),
            "the replacement direct bindings have no app-side residual"
        );
    }
    assert_eq!(
        script_ctx(&app)
            .slot_table
            .borrow()
            .get("trigger.flag")
            .and_then(|record| record.value.clone()),
        Some(SlotValue::Number(20.0)),
        "the post-reload binding must retain both the KVP and manifest commands",
    );
}

// A mod hot reload recomposes the active sets and
// rebuilds trigger bindings through `rebuild_active_trigger_bindings`. A
// level member's `on("enter", …)` is retained level-local data keyed by
// its volume, so after the reload it still fires for that volume and never
// for a sibling carrying the same tag.
#[test]
fn staged_recomposition_keeps_a_level_member_enter_on_its_volume_only() {
    let mut app = test_app();
    app.level = Some(level_world("member_reload_level", 1));
    let spawn_plate = |app: &App| {
        let ctx = script_ctx(app);
        let mut entities = ctx.registry.borrow_mut();
        let id = entities.spawn(Transform::default());
        entities
            .set_component(
                id,
                TriggerVolumeComponent::new(
                    TriggerActivation::Touch,
                    String::new(),
                    String::new(),
                    String::new(),
                    TriggerCommand::Start,
                    TriggerFireMode::Multiple,
                    0.0,
                    true,
                ),
            )
            .expect("trigger component attaches");
        entities
            .set_tags(id, vec!["plate".to_string()])
            .expect("trigger tags attach");
        id
    };
    let plate = spawn_plate(&app);
    let sibling = spawn_plate(&app);
    script_ctx(&app)
        .slot_table
        .borrow_mut()
        .insert("trigger.flag".to_string(), number_slot(0.0))
        .expect("trigger fixture slot should be vacant");

    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .populate_level_with_trigger_events(
            vec![scoped_global_set_state("reveal", 7.0).reaction],
            Vec::new(),
            vec![postretro_entities::VolumeTriggerEventDescriptor {
                trigger: plate,
                event: "enter".to_string(),
                fire: vec!["reveal".to_string()],
            }],
            Vec::new(),
            &app.active_level_tags,
        );
    // The reload: the mod manifest's global rules change, then the staged
    // commit recomposes and rebinds.
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_global_reactions(vec![scoped_global_set_state("ambient", 1.0)]);
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .recompose_active_sets(&app.active_level_tags);
    app.rebuild_active_reaction_subscribers();
    app.rebuild_active_trigger_bindings();

    let fire_enter = |app: &App, trigger| {
        let ctx = script_ctx(app);
        let mut entities = ctx.registry.borrow_mut();
        let mut slots = ctx.slot_table.borrow_mut();
        app.trigger_bindings
            .execute(
                trigger,
                crate::trigger_system::TriggerEventEdge::Enter,
                &mut entities,
                &mut slots,
                &crate::trigger_commands::TriggerFireContext::default(),
            )
            .command_count()
    };
    let flag = |app: &App| {
        script_ctx(app)
            .slot_table
            .borrow()
            .get("trigger.flag")
            .and_then(|record| record.value.clone())
    };
    assert!(
        !app.trigger_bindings
            .bound_edges()
            .contains(&(sibling, crate::trigger_system::TriggerEventEdge::Enter)),
        "the sibling carrying the same tag gains no edge"
    );
    assert_eq!(fire_enter(&app, sibling), 0);
    assert_eq!(flag(&app), Some(SlotValue::Number(0.0)));
    assert_eq!(fire_enter(&app, plate), 1);
    assert_eq!(
        flag(&app),
        Some(SlotValue::Number(7.0)),
        "the member's reaction still runs on its volume after the reload"
    );
}

// Regression: filtering a trigger-scoped reaction from a crossing used to
// discard all of the crossing's compatible reactions.
#[test]
fn crossing_filter_preserves_compatible_reactions_in_original_order() {
    let mut app = test_app();
    script_ctx(&app)
        .slot_table
        .borrow_mut()
        .insert("test.health".to_string(), number_slot(75.0))
        .expect("test slot should be vacant");
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_global_reactions(vec![
            postretro_entities::ScopedReaction {
                reaction: NamedReaction {
                    name: "trigger_only".to_string(),
                    descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                        primitive: "applyDamage".to_string(),
                        target: Some("@activators".to_string()),
                        kind: None,
                        tag: None,
                        on_complete: None,
                        args: serde_json::json!({ "amount": 10.0 }),
                    }),
                },
                levels: Vec::new(),
            },
            scoped_global_set_state("ordinary", 1.0),
        ]);
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_global_crossings(vec![postretro_entities::ScopedCrossing {
            crossing: CrossingDescriptor {
                slot: Some("test.health".to_string()),
                condition: CrossingCondition::Below { threshold: 0.5 },
                max: 100.0,
                edge: None,
                fire: vec!["trigger_only".to_string(), "ordinary".to_string()],
            },
            levels: Vec::new(),
        }]);
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .recompose_active_sets(&[]);
    app.rebuild_active_reaction_subscribers();

    assert_eq!(
        script_ctx(&app).data_registry.borrow().crossings[0].fire,
        vec!["ordinary".to_string()],
    );
    script_ctx(&app)
        .slot_table
        .borrow_mut()
        .get_mut("test.health")
        .expect("test slot remains installed")
        .value = Some(SlotValue::Number(25.0));
    let ctx = script_ctx(&app);
    assert_eq!(
        app.session
            .as_mut()
            .expect("test app session installed")
            .crossing_detector
            .detect(&ctx.slot_table.borrow()),
        vec!["ordinary".to_string()],
    );
}

// `on.player` exists only in a player event's fire: a crossing loses the
// address, and a `levelLoad` reaction using it is reported at install.
#[test]
fn on_player_reactions_are_rejected_for_crossings_and_level_load() {
    use postretro_test_log_capture::LogCapture;
    let ctx = ScriptCtx::new();
    let on_player = |name: &str| NamedReaction {
        name: name.to_string(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: "applyDamage".to_string(),
            target: Some("@player".to_string()),
            kind: None,
            tag: None,
            on_complete: None,
            args: serde_json::json!({ "amount": 5.0 }),
        }),
    };
    ctx.data_registry.borrow_mut().populate_level(
        vec![on_player("scald"), on_player("levelLoad")],
        vec![CrossingDescriptor {
            slot: Some("player.health".to_string()),
            condition: CrossingCondition::Below { threshold: 0.5 },
            max: 100.0,
            edge: None,
            fire: vec!["scald".to_string()],
        }],
        &[],
    );
    let capture = LogCapture::start();
    rebuild_reaction_subscribers(
        &mut postretro_scripting_core::reaction_dispatch::ProgressTracker::new(),
        &mut postretro_scripting_core::state_crossings::CrossingDetector::new(),
        &ctx,
        SubscriberRebuild::LevelInstall,
    );
    capture.assert_logged_once(
        log::Level::Error,
        "crossing on `player.health` drops address `scald`: reaction `scald` uses `on.player`",
    );
    capture.assert_logged_once(
        log::Level::Error,
        "levelLoad: reaction `levelLoad` uses `on.player`",
    );
    assert!(
        ctx.data_registry.borrow().crossings.is_empty(),
        "the crossing kept no other address"
    );
    // The `@player`-only levelLoad is covered by the error above; the legacy
    // trigger-sentinel warning must not double-report it.
    assert!(
        !capture.records().iter().any(|record| {
            record.level == log::Level::Warn
                && record
                    .message
                    .contains("levelLoad references trigger-sentinel work")
        }),
        "legacy sentinel warning fired for an `@player`-only levelLoad"
    );
}

// Regression: any non-entity step read as trigger-sentinel work, so a body
// of system or group steps lost its crossing subscription and a
// `levelLoad` holding one warned.
#[test]
fn system_and_group_steps_are_not_trigger_sentinel_work() {
    use postretro_scripting_core::data_descriptors::{SequenceStep, SequenceTarget};
    use postretro_test_log_capture::LogCapture;
    let ctx = ScriptCtx::new();
    let body = |name: &str| NamedReaction {
        name: name.to_string(),
        descriptor: ReactionDescriptor::Sequence(vec![
            SequenceStep {
                id: SequenceTarget::System,
                primitive: "playSound".to_string(),
                args: serde_json::json!({ "sound": "x" }),
            },
            SequenceStep {
                id: SequenceTarget::Group(postretro_entities::GroupTarget {
                    kind: postretro_entities::GroupKind::Npc,
                    tag: None,
                }),
                primitive: "updateNpcState".to_string(),
                args: serde_json::json!({ "aggro": true }),
            },
        ]),
    };
    ctx.data_registry.borrow_mut().populate_level(
        vec![body("warn"), body("levelLoad")],
        vec![CrossingDescriptor {
            slot: Some("player.health".to_string()),
            condition: CrossingCondition::Below { threshold: 0.5 },
            max: 100.0,
            edge: None,
            fire: vec!["warn".to_string()],
        }],
        &[],
    );
    let capture = LogCapture::start();
    rebuild_reaction_subscribers(
        &mut postretro_scripting_core::reaction_dispatch::ProgressTracker::new(),
        &mut postretro_scripting_core::state_crossings::CrossingDetector::new(),
        &ctx,
        SubscriberRebuild::LevelInstall,
    );
    assert_eq!(
        ctx.data_registry.borrow().crossings[0].fire,
        ["warn"],
        "the crossing keeps its address"
    );
    capture.assert_not_logged(
        log::Level::Warn,
        "levelLoad references trigger-sentinel work",
    );
}

#[test]
fn audience_source_gates_reject_all_tokens_for_crossings_and_level_load() {
    use postretro_test_log_capture::LogCapture;
    for token in ["@activators", "@trigger", "@player"] {
        let ctx = ScriptCtx::new();
        let named = |name: &str| NamedReaction {
            name: name.into(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "playSound".into(),
                target: None,
                kind: None,
                tag: None,
                on_complete: None,
                args: serde_json::json!({"sound":"beep", "audience":token}),
            }),
        };
        ctx.data_registry.borrow_mut().populate_level(
            vec![named("audience"), named("levelLoad")],
            vec![CrossingDescriptor {
                slot: Some("player.health".into()),
                condition: CrossingCondition::Below { threshold: 0.5 },
                max: 100.0,
                edge: None,
                fire: vec!["audience".into()],
            }],
            &[],
        );
        let capture = LogCapture::start();
        rebuild_reaction_subscribers(
            &mut postretro_scripting_core::reaction_dispatch::ProgressTracker::new(),
            &mut postretro_scripting_core::state_crossings::CrossingDetector::new(),
            &ctx,
            SubscriberRebuild::LevelInstall,
        );
        assert!(ctx.data_registry.borrow().crossings.is_empty(), "{token}");
        assert!(
            matches!(&ctx.data_registry.borrow().reactions[1].descriptor,
                ReactionDescriptor::Sequence(steps) if steps.is_empty())
        );
        capture.assert_logged_once(
            log::Level::Error,
            "crossing on `player.health` drops address `audience`",
        );
        capture.assert_logged_once(
            log::Level::Error,
            "levelLoad: reaction `levelLoad` uses audience",
        );
    }
}

#[test]
fn runtime_audience_source_install_parity_for_crossings_and_level_load() {
    use postretro_test_log_capture::LogCapture;
    for audience in [
        Some("@activators"),
        Some("@trigger"),
        Some("@player"),
        Some("everyone"),
        None,
    ] {
        let mut args = serde_json::json!({ "sound": "beep" });
        if let Some(audience) = audience {
            args["audience"] = serde_json::json!(audience);
        }
        let fixture = serde_json::json!({
            "reactions": [
                { "name": "crossingCue", "primitive": "playSound", "args": args },
                { "name": "levelLoad", "primitive": "playSound", "args": args }
            ],
            "crossings": [{ "slot": "player.health", "below": 25, "max": 100, "fire": ["crossingCue"] }]
        });
        let ts = runtime_audience_manifest(&fixture, false);
        let luau = runtime_audience_manifest(&fixture, true);
        assert_eq!(ts.reactions.len(), 2, "fixture must parse: {audience:?}");
        assert_eq!(ts.crossings.len(), 1);
        assert_eq!(ts.reactions, luau.reactions);
        assert_eq!(ts.crossings, luau.crossings);
        let scoped = audience.is_some_and(|audience| audience.starts_with('@'));
        for manifest in [ts, luau] {
            let ctx = ScriptCtx::new();
            ctx.data_registry.borrow_mut().populate_level(
                manifest.reactions,
                manifest.crossings,
                &[],
            );
            let capture = LogCapture::start();
            rebuild_reaction_subscribers(
                &mut ProgressTracker::new(),
                &mut postretro_scripting_core::state_crossings::CrossingDetector::new(),
                &ctx,
                SubscriberRebuild::LevelInstall,
            );
            let data = ctx.data_registry.borrow();
            assert_eq!(data.crossings.is_empty(), scoped, "{audience:?}");
            assert_eq!(
                matches!(&data.reactions[1].descriptor, ReactionDescriptor::Sequence(steps) if steps.is_empty()),
                scoped
            );
            if scoped {
                capture.assert_logged_once(
                    log::Level::Error,
                    "crossing on `player.health` drops address `crossingCue`",
                );
                capture.assert_logged_once(
                    log::Level::Error,
                    "levelLoad: reaction `levelLoad` uses audience",
                );
            } else {
                capture.assert_not_logged(log::Level::Error, "uses audience");
            }
        }
    }
}
