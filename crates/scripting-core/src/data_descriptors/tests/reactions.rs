// Tests: JS/Luau reaction & crossing parsing.

use super::super::*;
use super::common::*;
use crate::ir::IrNode;

#[test]
fn js_manifest_parses_progress_and_primitive_reactions() {
    let src = r#"({
        reactions: [
            { name: "reactorWave1",
              progress: { tag: "reactorWave1Monsters", at: 1.0, fire: "wave1Complete" } },
            { name: "wave1Complete",
              primitive: "moveGeometry",
              tag: "reactorChambers",
              onComplete: "wave2Revealed" },
        ]
    })"#;
    let manifest = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());

    assert_eq!(manifest.reactions.len(), 2);
    assert_eq!(manifest.reactions[0].name, "reactorWave1");
    match &manifest.reactions[0].descriptor {
        ReactionDescriptor::Progress(p) => {
            assert_eq!(p.tag, "reactorWave1Monsters");
            assert!((p.at - 1.0).abs() < 1e-6);
            assert_eq!(p.fire, "wave1Complete");
        }
        other => panic!("expected progress, got {other:?}"),
    }
    match &manifest.reactions[1].descriptor {
        ReactionDescriptor::Primitive(p) => {
            assert_eq!(p.primitive, "moveGeometry");
            assert_eq!(p.tag.as_deref(), Some("reactorChambers"));
            assert_eq!(p.on_complete.as_deref(), Some("wave2Revealed"));
        }
        other => panic!("expected primitive, got {other:?}"),
    }
}

#[test]
fn js_primitive_without_on_complete_is_none() {
    let src = r#"({
        reactions: [{ name: "x", primitive: "moveGeometry", tag: "t" }]
    })"#;
    let m = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
    match &m.reactions[0].descriptor {
        ReactionDescriptor::Primitive(p) => assert!(p.on_complete.is_none()),
        other => panic!("expected primitive, got {other:?}"),
    }
}

#[test]
fn js_primitive_with_tag_parses_as_entity_targeted() {
    // An entity-targeted descriptor (with `tag`) still parses byte-identically:
    // `tag` round-trips as `Some`.
    let src = r#"({
        reactions: [{ name: "x", primitive: "setEmitterRate", tag: "smoke", args: { rate: 0.0 } }]
    })"#;
    let m = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
    match &m.reactions[0].descriptor {
        ReactionDescriptor::Primitive(p) => {
            assert_eq!(p.primitive, "setEmitterRate");
            assert_eq!(p.tag.as_deref(), Some("smoke"));
        }
        other => panic!("expected primitive, got {other:?}"),
    }
}

#[test]
fn js_primitive_without_tag_is_system_targeted() {
    // A system reaction omits `tag` entirely; it parses with `tag == None`.
    let src = r#"({
        reactions: [{ name: "lowHealth", primitive: "playSound", args: { sound: "alarm" } }]
    })"#;
    let m = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
    match &m.reactions[0].descriptor {
        ReactionDescriptor::Primitive(p) => {
            assert_eq!(p.primitive, "playSound");
            assert!(p.tag.is_none());
        }
        other => panic!("expected primitive, got {other:?}"),
    }
}

#[test]
fn js_spawner_primitive_requires_a_non_empty_tag() {
    for source in [
        r#"({ reactions: [{ name: "x", primitive: "spawnFromSpawner" }] })"#,
        r#"({ reactions: [{ name: "x", primitive: "spawnFromSpawner", tag: "" }] })"#,
        r#"({ reactions: [{ name: "x", primitive: "spawnFromSpawner", target: "@activators" }] })"#,
    ] {
        let manifest = eval_js(source, |ctx, value| {
            LevelManifest::from_js_value(ctx, value).unwrap()
        });
        assert!(manifest.reactions.is_empty());
    }
}

#[test]
fn js_malformed_reaction_is_skipped() {
    // A bad descriptor must not discard the rest of a valid setup manifest.
    let src = r#"({
        reactions: [{ name: "x", progress: { tag: "t", at: 0.5 } }]
    })"#;
    let manifest = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
    assert!(manifest.reactions.is_empty());
}

#[test]
fn js_reaction_without_name_is_skipped() {
    let src = r#"({
        reactions: [{ progress: { tag: "t", at: 0.5, fire: "f" } }]
    })"#;
    let manifest = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
    assert!(manifest.reactions.is_empty());
}

#[test]
fn js_unknown_shape_reaction_is_skipped() {
    let src = r#"({
        reactions: [{ name: "x", tag: "t" }]
    })"#;
    let manifest = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
    assert!(manifest.reactions.is_empty());
}

#[test]
fn js_empty_primitive_name_is_skipped() {
    let src = r#"({
        reactions: [{ name: "x", primitive: "", tag: "t" }]
    })"#;
    let manifest = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
    assert!(manifest.reactions.is_empty());
}

#[test]
fn js_at_out_of_range_high_is_skipped() {
    let src = r#"({
        reactions: [{ name: "x", progress: { tag: "t", at: 1.5, fire: "f" } }]
    })"#;
    let manifest = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
    assert!(manifest.reactions.is_empty());
}

#[test]
fn js_at_out_of_range_negative_is_skipped() {
    let src = r#"({
        reactions: [{ name: "x", progress: { tag: "t", at: -0.1, fire: "f" } }]
    })"#;
    let manifest = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
    assert!(manifest.reactions.is_empty());
}

#[test]
fn js_sequence_reaction_deserializes() {
    let src = r#"({
        reactions: [{
            name: "openVault",
            sequence: [
                { id: 65536, primitive: "moveGeometry", args: { duration: 1.5 } },
                { id: 131072, primitive: "playSound", args: { clip: "vault" } }
            ]
        }]
    })"#;
    let m = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
    match &m.reactions[0].descriptor {
        ReactionDescriptor::Sequence(steps) => {
            assert_eq!(steps.len(), 2);
            assert_eq!(
                steps[0].id,
                SequenceTarget::Entity(EntityId::from_raw(65536))
            );
            assert_eq!(steps[0].primitive, "moveGeometry");
            assert_eq!(steps[0].args["duration"].as_f64(), Some(1.5));
            assert_eq!(
                steps[1].id,
                SequenceTarget::Entity(EntityId::from_raw(131072))
            );
            assert_eq!(steps[1].primitive, "playSound");
            assert_eq!(steps[1].args["clip"], serde_json::json!("vault"));
        }
        other => panic!("expected sequence, got {other:?}"),
    }
}

#[test]
fn js_sequence_step_missing_args_defaults_to_null() {
    let src = r#"({
        reactions: [{
            name: "x",
            sequence: [{ id: 1, primitive: "ping" }]
        }]
    })"#;
    let m = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
    match &m.reactions[0].descriptor {
        ReactionDescriptor::Sequence(steps) => {
            assert_eq!(steps.len(), 1);
            assert!(steps[0].args.is_null());
        }
        other => panic!("expected sequence, got {other:?}"),
    }
}

// Regression: QuickJS accepted control sentinels with arbitrary primitive
// names, and an entity-targeted `wait`/`fire` reached the wrong dispatch arm.
#[test]
fn js_sequence_control_steps_require_canonical_target_primitive_pairs() {
    let cases = [
        (
            r#"({ name: "bad", sequence: [{ id: "@wait", primitive: "ping", args: {} }] })"#,
            "reaction `bad` sequence step 0: sentinel `@wait` requires primitive `wait`",
        ),
        (
            r#"({ name: "bad", sequence: [{ id: "@fire", primitive: "ping", args: {} }] })"#,
            "reaction `bad` sequence step 0: sentinel `@fire` requires primitive `fire`",
        ),
        (
            r#"({ name: "bad", sequence: [{ id: 65536, primitive: "wait", args: {} }] })"#,
            "reaction `bad` sequence step 0: control primitive `wait` requires sentinel `@wait`; it cannot be entity-targeted",
        ),
        (
            r#"({ name: "bad", sequence: [{ id: 65536, primitive: "fire", args: {} }] })"#,
            "reaction `bad` sequence step 0: control primitive `fire` requires sentinel `@fire`; it cannot be entity-targeted",
        ),
    ];

    for (source, expected) in cases {
        let error = eval_js(source, |ctx, value| {
            named_reaction_from_js(ctx, value).unwrap_err()
        });
        assert!(
            error.to_string().contains(expected),
            "unexpected QuickJS diagnostic for {source}: {error}",
        );
    }
}

#[test]
fn js_sequence_control_steps_accept_only_the_canonical_pairs() {
    let reaction = eval_js(
        r#"({ name: "ok", sequence: [
            { id: "@wait", primitive: "wait", args: { durationMs: 1 } },
            { id: "@fire", primitive: "fire", args: { event: "next" } }
        ] })"#,
        |ctx, value| named_reaction_from_js(ctx, value).unwrap(),
    );
    let ReactionDescriptor::Sequence(steps) = reaction.descriptor else {
        panic!("expected sequence");
    };
    assert!(matches!(steps[0].id, SequenceTarget::Wait));
    assert!(matches!(steps[1].id, SequenceTarget::Fire));
}

#[test]
fn js_empty_arrays_yield_empty_manifest() {
    let src = "({ reactions: [] })";
    let m = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
    assert!(m.reactions.is_empty());
}
#[test]
fn lua_manifest_parses_progress_and_primitive_reactions() {
    let src = r#"return {
        reactions = {
            { name = "reactorWave1",
              progress = { tag = "reactorWave1Monsters", at = 1.0, fire = "wave1Complete" } },
            { name = "wave1Complete",
              primitive = "moveGeometry",
              tag = "reactorChambers",
              onComplete = "wave2Revealed" },
        }
    }"#;
    let m = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());

    assert_eq!(m.reactions.len(), 2);
    match &m.reactions[0].descriptor {
        ReactionDescriptor::Progress(p) => {
            assert_eq!(p.tag, "reactorWave1Monsters");
            assert!((p.at - 1.0).abs() < 1e-6);
            assert_eq!(p.fire, "wave1Complete");
        }
        other => panic!("expected progress, got {other:?}"),
    }
    match &m.reactions[1].descriptor {
        ReactionDescriptor::Primitive(p) => {
            assert_eq!(p.primitive, "moveGeometry");
            assert_eq!(p.tag.as_deref(), Some("reactorChambers"));
            assert_eq!(p.on_complete.as_deref(), Some("wave2Revealed"));
        }
        other => panic!("expected primitive, got {other:?}"),
    }
}

#[test]
fn lua_primitive_without_on_complete_is_none() {
    let src = r#"return {
        reactions = { { name = "x", primitive = "moveGeometry", tag = "t" } }
    }"#;
    let m = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());
    match &m.reactions[0].descriptor {
        ReactionDescriptor::Primitive(p) => assert!(p.on_complete.is_none()),
        other => panic!("expected primitive, got {other:?}"),
    }
}

#[test]
fn lua_primitive_with_tag_parses_as_entity_targeted() {
    let src = r#"return {
        reactions = { { name = "x", primitive = "setEmitterRate", tag = "smoke", args = { rate = 0.0 } } }
    }"#;
    let m = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());
    match &m.reactions[0].descriptor {
        ReactionDescriptor::Primitive(p) => {
            assert_eq!(p.primitive, "setEmitterRate");
            assert_eq!(p.tag.as_deref(), Some("smoke"));
        }
        other => panic!("expected primitive, got {other:?}"),
    }
}

#[test]
fn lua_primitive_without_tag_is_system_targeted() {
    let src = r#"return {
        reactions = { { name = "lowHealth", primitive = "playSound", args = { sound = "alarm" } } }
    }"#;
    let m = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());
    match &m.reactions[0].descriptor {
        ReactionDescriptor::Primitive(p) => {
            assert_eq!(p.primitive, "playSound");
            assert!(p.tag.is_none());
        }
        other => panic!("expected primitive, got {other:?}"),
    }
}

#[test]
fn lua_spawner_primitive_requires_a_non_empty_tag() {
    for source in [
        r#"return { reactions = { { name = "x", primitive = "spawnFromSpawner" } } }"#,
        r#"return { reactions = { { name = "x", primitive = "spawnFromSpawner", tag = "" } } }"#,
        r#"return { reactions = { { name = "x", primitive = "spawnFromSpawner", target = "@activators" } } }"#,
    ] {
        let manifest = eval_lua(source, |value| {
            LevelManifest::from_lua_value(value).unwrap()
        });
        assert!(manifest.reactions.is_empty());
    }
}

#[test]
fn lua_malformed_reaction_is_skipped() {
    let src = r#"return {
        reactions = { { name = "x", progress = { tag = "t", at = 0.5 } } }
    }"#;
    let manifest = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());
    assert!(manifest.reactions.is_empty());
}

#[test]
fn lua_unknown_shape_reaction_is_skipped() {
    let src = r#"return {
        reactions = { { name = "x", tag = "t" } }
    }"#;
    let manifest = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());
    assert!(manifest.reactions.is_empty());
}

#[test]
fn lua_empty_primitive_name_is_skipped() {
    let src = r#"return {
        reactions = { { name = "x", primitive = "", tag = "t" } }
    }"#;
    let manifest = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());
    assert!(manifest.reactions.is_empty());
}

#[test]
fn lua_at_out_of_range_is_skipped() {
    let src = r#"return {
        reactions = { { name = "x", progress = { tag = "t", at = 1.5, fire = "f" } } }
    }"#;
    let manifest = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());
    assert!(manifest.reactions.is_empty());
}

#[test]
fn lua_sequence_reaction_deserializes() {
    let src = r#"return {
        reactions = {
            { name = "openVault",
              sequence = {
                  { id = 65536, primitive = "moveGeometry", args = { duration = 1.5 } },
                  { id = 131072, primitive = "playSound", args = { clip = "vault" } },
              } }
        }
    }"#;
    let m = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());
    match &m.reactions[0].descriptor {
        ReactionDescriptor::Sequence(steps) => {
            assert_eq!(steps.len(), 2);
            assert_eq!(
                steps[0].id,
                SequenceTarget::Entity(EntityId::from_raw(65536))
            );
            assert_eq!(steps[0].primitive, "moveGeometry");
            assert_eq!(steps[1].primitive, "playSound");
        }
        other => panic!("expected sequence, got {other:?}"),
    }
}

// Regression: Luau must enforce the same canonical control descriptor pairs as
// QuickJS, including rejecting entity-targeted control primitives.
#[test]
fn lua_sequence_control_steps_require_canonical_target_primitive_pairs() {
    let cases = [
        (
            r#"return { name = "bad", sequence = { { id = "@wait", primitive = "ping", args = {} } } }"#,
            "reaction `bad` sequence step 0: sentinel `@wait` requires primitive `wait`",
        ),
        (
            r#"return { name = "bad", sequence = { { id = "@fire", primitive = "ping", args = {} } } }"#,
            "reaction `bad` sequence step 0: sentinel `@fire` requires primitive `fire`",
        ),
        (
            r#"return { name = "bad", sequence = { { id = 65536, primitive = "wait", args = {} } } }"#,
            "reaction `bad` sequence step 0: control primitive `wait` requires sentinel `@wait`; it cannot be entity-targeted",
        ),
        (
            r#"return { name = "bad", sequence = { { id = 65536, primitive = "fire", args = {} } } }"#,
            "reaction `bad` sequence step 0: control primitive `fire` requires sentinel `@fire`; it cannot be entity-targeted",
        ),
    ];

    for (source, expected) in cases {
        let error = eval_lua(source, |value| named_reaction_from_lua(value).unwrap_err());
        assert!(
            error.to_string().contains(expected),
            "unexpected Luau diagnostic for {source}: {error}",
        );
    }
}

#[test]
fn lua_sequence_control_steps_accept_only_the_canonical_pairs() {
    let reaction = eval_lua(
        r#"return { name = "ok", sequence = {
            { id = "@wait", primitive = "wait", args = { durationMs = 1 } },
            { id = "@fire", primitive = "fire", args = { event = "next" } },
        } }"#,
        |value| named_reaction_from_lua(value).unwrap(),
    );
    let ReactionDescriptor::Sequence(steps) = reaction.descriptor else {
        panic!("expected sequence");
    };
    assert!(matches!(steps[0].id, SequenceTarget::Wait));
    assert!(matches!(steps[1].id, SequenceTarget::Fire));
}

#[test]
fn lua_reactions_reject_non_dense_tables() {
    // Regression: raw_len iteration silently accepted malformed reaction arrays.
    let cases = [
        (
            "return { reactions = { named = { name = \"x\", primitive = \"ping\" } } }",
            "map",
        ),
        (
            "return { reactions = { { name = \"x\", primitive = \"ping\" }, extra = { name = \"y\", primitive = \"pong\" } } }",
            "extra",
        ),
        (
            "return { reactions = { [2] = { name = \"x\", primitive = \"ping\" } } }",
            "hole",
        ),
        (
            "return { reactions = { [0] = { name = \"x\", primitive = \"ping\" } } }",
            "zero",
        ),
        (
            "return { reactions = { [1.5] = { name = \"x\", primitive = \"ping\" } } }",
            "float",
        ),
    ];

    for (source, label) in cases {
        let err = eval_lua(source, |v| LevelManifest::from_lua_value(v).unwrap_err());
        assert!(
            err.to_string().contains("dense array"),
            "{label} produced unexpected error: {err}"
        );
    }
}

#[test]
fn lua_malformed_sequences_skip_their_reaction() {
    // Regression: raw_len iteration silently accepted malformed sequence steps.
    let cases = [
        (
            "return { reactions = { { name = \"x\", sequence = { named = { id = 1, primitive = \"ping\" } } } } }",
            "map",
        ),
        (
            "return { reactions = { { name = \"x\", sequence = { { id = 1, primitive = \"ping\" }, extra = { id = 2, primitive = \"pong\" } } } } }",
            "extra",
        ),
        (
            "return { reactions = { { name = \"x\", sequence = { [2] = { id = 1, primitive = \"ping\" } } } } }",
            "hole",
        ),
        (
            "return { reactions = { { name = \"x\", sequence = { [0] = { id = 1, primitive = \"ping\" } } } } }",
            "zero",
        ),
        (
            "return { reactions = { { name = \"x\", sequence = { [1.5] = { id = 1, primitive = \"ping\" } } } } }",
            "float",
        ),
    ];

    for (source, label) in cases {
        let manifest = eval_lua(source, |v| LevelManifest::from_lua_value(v).unwrap());
        assert!(manifest.reactions.is_empty(), "{label} was not skipped");
    }
}

#[test]
fn lua_empty_arrays_yield_empty_manifest() {
    let src = "return { reactions = {} }";
    let m = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());
    assert!(m.reactions.is_empty());
}

#[test]
fn lua_crossings_accept_dense_arrays() {
    let src = r#"return {
        crossings = {
            { slot = "player.health", below = 25.0, max = 100.0, fire = { "lowHealth" } },
            { slot = "player.health", above = 80.0, max = 100.0, fire = {} },
        }
    }"#;
    let m = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());

    assert_eq!(m.crossings.len(), 2);
    assert_eq!(m.crossings[0].slot.as_deref(), Some("player.health"));
    assert_eq!(m.crossings[0].fire, vec!["lowHealth".to_string()]);
}

// A level script keys trigger events by volume. A tag-keyed
// entry is rejected with a warning naming the level script, while the
// volume-keyed entries beside it parse identically in both runtimes.
#[test]
fn level_trigger_events_keep_volume_entries_and_reject_tag_keyed_naming_the_level_script() {
    use postretro_test_log_capture::LogCapture;

    let capture = LogCapture::start();
    let js = eval_js(
        r#"({ triggerEvents: [
            { trigger: 65536, event: "enter", fire: ["zap", "once"] },
            { tag: "plate", event: "enter", fire: ["stale"] },
            { trigger: 131072, event: "exit", fire: ["leave"] },
            { trigger: 65536, event: "occupied", fire: ["bad"] },
            { trigger: 65536, tag: "plate", event: "enter", fire: ["both"] }
        ] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { triggerEvents = {
            { trigger = 65536, event = "enter", fire = { "zap", "once" } },
            { tag = "plate", event = "enter", fire = { "stale" } },
            { trigger = 131072, event = "exit", fire = { "leave" } },
            { trigger = 65536, event = "occupied", fire = { "bad" } },
            { trigger = 65536, tag = "plate", event = "enter", fire = { "both" } }
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js.trigger_events, lua.trigger_events);
    assert_eq!(
        js.trigger_events,
        vec![
            VolumeTriggerEventDescriptor {
                trigger: EntityId::from_raw(65536),
                event: "enter".into(),
                fire: vec!["zap".into(), "once".into()],
            },
            VolumeTriggerEventDescriptor {
                trigger: EntityId::from_raw(131072),
                event: "exit".into(),
                fire: vec!["leave".into()],
            },
        ]
    );
    let rejections: Vec<_> = capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level == log::Level::Warn
                && record.message.contains("level script `setupLevel`")
                && record.message.contains("keyed by tag `plate`")
        })
        .collect();
    assert_eq!(
        rejections.len(),
        2,
        "one rejection per runtime: {rejections:?}"
    );
    capture.assert_logged(log::Level::Warn, "carries both `trigger` and `tag`");
    // Both runtimes report each skipped entry with one identical message,
    // indexed from 0.
    for needle in [
        "triggerEvents[1] is keyed by tag `plate`",
        "triggerEvents[3] has unknown event `occupied`",
        "triggerEvents[4] is malformed",
    ] {
        assert_identical_warning_per_runtime(&capture, needle);
    }
}

/// Exactly one warning per runtime contains `needle`, and the two read the same.
fn assert_identical_warning_per_runtime(
    capture: &postretro_test_log_capture::LogCapture,
    needle: &str,
) {
    let messages: Vec<String> = capture
        .records()
        .into_iter()
        .filter(|record| record.level == log::Level::Warn && record.message.contains(needle))
        .map(|record| record.message)
        .collect();
    assert_eq!(
        messages.len(),
        2,
        "one `{needle}` per runtime: {messages:?}"
    );
    assert_eq!(
        messages[0], messages[1],
        "the runtimes' diagnostics diverged"
    );
}

// The mod manifest keys trigger events by tag. A volume-keyed
// entry is rejected with a warning naming the manifest, while the tag-keyed
// entries beside it keep their `levels` selector in both runtimes.
#[test]
fn mod_trigger_events_keep_tag_entries_and_reject_volume_keyed_naming_the_manifest() {
    use postretro_test_log_capture::LogCapture;

    let capture = LogCapture::start();
    let js = eval_js(
        r#"({ triggerEvents: [
            { tag: "plate", event: "enter", fire: ["zap"], levels: ["campaign"] },
            { trigger: 65536, event: "enter", fire: ["member"] },
            { tag: "door", event: "exit", fire: ["shut"] }
        ] })"#,
        |_ctx, value| {
            let obj = rquickjs::Object::from_value(value).unwrap();
            drain_mod_trigger_events_js(&obj, "mod manifest").unwrap()
        },
    );
    let lua = eval_lua(
        r#"return { triggerEvents = {
            { tag = "plate", event = "enter", fire = { "zap" }, levels = { "campaign" } },
            { trigger = 65536, event = "enter", fire = { "member" } },
            { tag = "door", event = "exit", fire = { "shut" } }
        } }"#,
        |value| {
            let LuaValue::Table(table) = value else {
                panic!("manifest is a table")
            };
            drain_mod_trigger_events_lua(&table, "mod manifest").unwrap()
        },
    );

    assert_eq!(js, lua);
    assert_eq!(
        js,
        vec![
            TriggerEventDescriptor {
                tag: "plate".into(),
                event: "enter".into(),
                fire: vec!["zap".into()],
                levels: vec!["campaign".into()],
            },
            TriggerEventDescriptor {
                tag: "door".into(),
                event: "exit".into(),
                fire: vec!["shut".into()],
                levels: Vec::new(),
            },
        ]
    );
    let rejections: Vec<_> = capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level == log::Level::Warn
                && record.message.contains("mod manifest")
                && record.message.contains("keyed by trigger volume")
        })
        .collect();
    assert_eq!(
        rejections.len(),
        2,
        "one rejection per runtime: {rejections:?}"
    );
    assert_identical_warning_per_runtime(&capture, "triggerEvents[1] is keyed by trigger volume");
}

// Regression: Luau rejected the whole `events` field for a sparse table while
// QuickJS skipped only the malformed slots and retained valid siblings.
#[test]
fn sparse_and_malformed_impact_events_keep_valid_siblings_in_both_vms() {
    let js = eval_js(
        r#"(() => {
            const events = [];
            events[0] = { kind: "impact", id: "salvage-first", filter: {}, policy: [] };
            events[2] = 42;
            events[3] = { kind: "impact", id: "salvage-last", filter: {}, policy: [] };
            return { events };
        })()"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { events = {
            [1] = { kind = "impact", id = "salvage-first", filter = {}, policy = {} },
            [3] = 42,
            [4] = { kind = "impact", id = "salvage-last", filter = {}, policy = {} },
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js.events, lua.events);
    assert_eq!(
        js.events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        ["salvage-first", "salvage-last"]
    );
}

#[test]
fn impact_event_ids_require_single_segment_portable_strings_in_both_vms() {
    let js = eval_js(
        r#"({ events: [
            { kind: "impact", id: "valid-id", filter: {}, policy: [] },
            { kind: "impact", id: "salvage:invalid", filter: {}, policy: [] },
            { kind: "impact", id: "bad space", filter: {}, policy: [] },
            { kind: "impact", id: "x".repeat(65), filter: {}, policy: [] }
        ] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { events = {
            { kind = "impact", id = "valid-id", filter = {}, policy = {} },
            { kind = "impact", id = "salvage:invalid", filter = {}, policy = {} },
            { kind = "impact", id = "bad space", filter = {}, policy = {} },
            { kind = "impact", id = string.rep("x", 65), filter = {}, policy = {} },
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js.events, lua.events);
    assert_eq!(js.events.len(), 1);
    assert_eq!(js.events[0].id, "valid-id");
}

// Regression: Luau's empty `do = {}` converted to an object and caused the
// engine to skip the complete policy, unlike JavaScript's empty array.
#[test]
fn empty_impact_group_and_valid_effect_parse_identically_in_both_vms() {
    let js = eval_js(
        r#"({ events: [{
            kind: "impact", id: "empty-group", filter: { tag: "crate" },
            policy: [
                { when: { op: "const", value: true }, do: [] },
                { primitive: "despawn", target: "@impact.target", args: {} }
            ]
        }] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { events = {{
            kind = "impact", id = "empty-group", filter = { tag = "crate" },
            policy = {
                { when = { op = "const", value = true }, ["do"] = {} },
                { primitive = "despawn", target = "@impact.target", args = {} },
            },
        }} }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js.events, lua.events);
    assert_eq!(js.events.len(), 1);
    assert_eq!(js.events[0].policy[0]["do"], serde_json::json!([]));
    assert_eq!(js.events[0].policy[1]["primitive"], "despawn");
}

#[test]
fn sparse_impact_policy_arrays_skip_the_event_in_both_vms() {
    let js = eval_js(
        r#"(() => {
            const policy = [];
            policy[1] = { primitive: "despawn", target: "@impact.target", args: {} };
            return { events: [
                { kind: "impact", id: "sparse", filter: { tag: "crate" }, policy },
                { kind: "impact", id: "valid", filter: { tag: "crate" }, policy: [] }
            ] };
        })()"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { events = {
            { kind = "impact", id = "sparse", filter = { tag = "crate" }, policy = {
                [2] = { primitive = "despawn", target = "@impact.target", args = {} },
            } },
            { kind = "impact", id = "valid", filter = { tag = "crate" }, policy = {} },
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js.events, lua.events);
    assert_eq!(js.events.len(), 1);
    assert_eq!(js.events[0].id, "valid");
}

#[test]
fn tagless_impact_overrides_are_skipped_in_both_vms() {
    let js = eval_js(
        r#"({ events: [
            { kind: "impact", id: "base", isOverride: true, filter: {}, policy: [] },
            { kind: "impact", id: "base", isOverride: true, filter: { tag: "elite" }, policy: [] }
        ] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { events = {
            { kind = "impact", id = "base", isOverride = true, filter = {}, policy = {} },
            { kind = "impact", id = "base", isOverride = true, filter = { tag = "elite" }, policy = {} },
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js.events, lua.events);
    assert_eq!(js.events.len(), 1);
    assert_eq!(js.events[0].filter_tag.as_deref(), Some("elite"));
}

#[test]
fn trigger_pool_manifests_parse_identically_across_vms() {
    let js = eval_js(
        r#"({ triggerPools: [
            { tag: "closet", arm: 2 },
            { tag: "ambush", armPercentage: 50, levels: ["campaign", "challenge"] }
        ] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { triggerPools = {
            { tag = "closet", arm = 2 },
            { tag = "ambush", armPercentage = 50, levels = { "campaign", "challenge" } }
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js.trigger_pools, lua.trigger_pools);
    assert_eq!(js.trigger_pools[0].arm, TriggerPoolArm::Count(2));
    assert_eq!(js.trigger_pools[1].arm, TriggerPoolArm::Percentage(50.0));
    assert_eq!(js.trigger_pools[1].levels, ["campaign", "challenge"]);
}

#[test]
fn sparse_trigger_pool_arrays_keep_valid_siblings_in_both_vms() {
    let js = eval_js(
        r#"({ triggerPools: [
            { tag: "first", arm: 1 },
            ,
            { tag: "third", arm: 2 }
        ] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { triggerPools = {
            [1] = { tag = "first", arm = 1 },
            [3] = { tag = "third", arm = 2 }
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js.trigger_pools, lua.trigger_pools);
    assert_eq!(
        js.trigger_pools
            .iter()
            .map(|pool| pool.tag.as_str())
            .collect::<Vec<_>>(),
        ["first", "third"]
    );
}

#[test]
fn oversized_sparse_trigger_pool_arrays_degrade_to_empty_in_both_vms() {
    // Regression: sparse arrays outside the 4,096-slot authoring contract must
    // fail as one field rather than driving a giant hole walk or allocation.
    let js = eval_js(
        r#"(() => {
            const triggerPools = [];
            triggerPools[0] = { tag: "first", arm: 1 };
            triggerPools[4294967294] = { tag: "last", arm: 1 };
            return { triggerPools };
        })()"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { triggerPools = {
            [1] = { tag = "first", arm = 1 },
            [4294967295] = { tag = "last", arm = 1 }
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js.trigger_pools, lua.trigger_pools);
    assert!(js.trigger_pools.is_empty());
}

#[test]
fn huge_nested_trigger_pool_levels_skip_bad_pool_and_keep_valid_sibling() {
    // Regression: a huge sparse JS `levels` array panicked before the sibling pool was parsed.
    let js = eval_js(
        r#"(() => {
            const levels = [];
            levels[4294967294] = "campaign";
            return { triggerPools: [
                { tag: "bad", arm: 1, levels },
                { tag: "good", arm: 2, levels: ["campaign"] }
            ] };
        })()"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { triggerPools = {
            { tag = "bad", arm = 1, levels = { [4294967295] = "campaign" } },
            { tag = "good", arm = 2, levels = { "campaign" } }
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js.trigger_pools, lua.trigger_pools);
    assert_eq!(js.trigger_pools.len(), 1);
    assert_eq!(js.trigger_pools[0].tag, "good");
}

#[test]
fn malformed_trigger_pool_containers_degrade_to_empty_in_both_vms() {
    let js = eval_js(
        r#"({ triggerPools: { first: { tag: "first", arm: 1 } } })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { triggerPools = { first = { tag = "first", arm = 1 } } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js.trigger_pools, lua.trigger_pools);
    assert!(js.trigger_pools.is_empty());
}

// Regression: a throwing `triggerPools` accessor aborted Luau manifest parsing.
#[test]
fn throwing_trigger_pool_container_getters_degrade_field_and_keep_manifest_siblings() {
    let js = eval_js(
        r#"(() => {
            const manifest = {
                reactions: [{ name: "good", primitive: "playSound" }],
                crossings: [{ slot: "test.value", above: 1, fire: ["good"] }],
                triggerEvents: [{ trigger: 65536, event: "enter", fire: ["good"] }]
            };
            Object.defineProperty(manifest, "triggerPools", {
                enumerable: true,
                get() { throw new Error("triggerPools accessor failed"); }
            });
            return manifest;
        })()"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"local manifest = {
            reactions = { { name = "good", primitive = "playSound" } },
            crossings = { { slot = "test.value", above = 1, fire = { "good" } } },
            triggerEvents = { { trigger = 65536, event = "enter", fire = { "good" } } }
        }
        return setmetatable(manifest, {
            __index = function(_, key)
                if key == "triggerPools" then
                    error("triggerPools accessor failed")
                end
                return nil
            end
        })"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js, lua);
    assert!(js.trigger_pools.is_empty());
    assert_eq!(js.reactions.len(), 1);
    assert_eq!(js.crossings.len(), 1);
    assert_eq!(js.trigger_events.len(), 1);
}

// Regression: a throwing indexed accessor discarded valid sparse JS siblings.
#[test]
fn js_throwing_trigger_pool_index_accessor_skips_entry_and_keeps_sparse_siblings() {
    let manifest = eval_js(
        r#"(() => {
            const triggerPools = [];
            triggerPools[0] = { tag: "first", arm: 1 };
            Object.defineProperty(triggerPools, "1", {
                enumerable: true,
                get() { throw new Error("trigger pool entry failed"); }
            });
            triggerPools[2] = { tag: "last", arm: 1 };
            return {
                reactions: [{ name: "good", primitive: "playSound" }],
                crossings: [{ slot: "test.value", above: 1, fire: ["good"] }],
                triggerEvents: [{ trigger: 65536, event: "enter", fire: ["good"] }],
                triggerPools
            };
        })()"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );

    assert_eq!(
        manifest
            .trigger_pools
            .iter()
            .map(|pool| pool.tag.as_str())
            .collect::<Vec<_>>(),
        ["first", "last"]
    );
    assert_eq!(manifest.reactions.len(), 1);
    assert_eq!(manifest.crossings.len(), 1);
    assert_eq!(manifest.trigger_events.len(), 1);
}

#[test]
fn over_limit_trigger_pool_containers_degrade_to_empty_in_both_vms() {
    let descriptor_count = MAX_TRIGGER_POOL_CONTAINER_ENTRIES + 1;
    let js_source = format!(
        r#"(() => {{
            const triggerPools = [];
            for (let i = 0; i < {descriptor_count}; i += 1) {{
                triggerPools.push({{ tag: "pool-" + i, arm: 1 }});
            }}
            return {{ triggerPools }};
        }})()"#,
    );
    let lua_source = format!(
        r#"local triggerPools = {{}}
        for i = 1, {descriptor_count} do
            triggerPools[i] = {{ tag = "pool-" .. i, arm = 1 }}
        end
        return {{ triggerPools = triggerPools }}"#,
    );

    let js = eval_js(&js_source, |ctx, value| {
        LevelManifest::from_js_value(ctx, value).unwrap()
    });
    let lua = eval_lua(&lua_source, |value| {
        LevelManifest::from_lua_value(value).unwrap()
    });

    assert_eq!(js.trigger_pools, lua.trigger_pools);
    assert!(js.trigger_pools.is_empty());
}

#[test]
fn luau_trigger_pool_slot_limit_ignores_metadata_properties() {
    // Regression: metadata was counted as a 4,097th array slot and discarded valid pools.
    let lua_source = format!(
        r#"local triggerPools = {{ metadata = "allowed" }}
        for i = 1, {} do
            triggerPools[i] = {{ tag = "pool-" .. i, arm = 1 }}
        end
        return {{ triggerPools = triggerPools }}"#,
        MAX_TRIGGER_POOL_CONTAINER_ENTRIES,
    );

    let manifest = eval_lua(&lua_source, |value| {
        LevelManifest::from_lua_value(value).unwrap()
    });

    assert_eq!(
        manifest.trigger_pools.len(),
        MAX_TRIGGER_POOL_CONTAINER_ENTRIES
    );
    assert_eq!(manifest.trigger_pools.first().unwrap().tag, "pool-1");
    assert_eq!(
        manifest.trigger_pools.last().unwrap().tag,
        format!("pool-{}", MAX_TRIGGER_POOL_CONTAINER_ENTRIES)
    );
}

#[test]
fn nullish_unused_trigger_pool_arm_form_is_ignored_in_both_vms() {
    let js = eval_js(
        r#"({ triggerPools: [
            { tag: "percentage", arm: undefined, armPercentage: 50 },
            { tag: "count", arm: 2, armPercentage: undefined }
        ] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { triggerPools = {
            { tag = "percentage", arm = nil, armPercentage = 50 },
            { tag = "count", arm = 2, armPercentage = nil }
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js.trigger_pools, lua.trigger_pools);
    assert_eq!(js.trigger_pools[0].arm, TriggerPoolArm::Percentage(50.0));
    assert_eq!(js.trigger_pools[1].arm, TriggerPoolArm::Count(2));
}

#[test]
fn trigger_pool_manifests_skip_malformed_entries_keep_first_duplicate_and_accept_zero_arms() {
    let js = eval_js(
        r#"({ triggerPools: [
            { tag: "count-zero", arm: 0 },
            { tag: "percentage-zero", armPercentage: 0, levels: ["campaign"] },
            42,
            { tag: "", arm: 1 },
            { arm: 1 },
            { tag: "neither" },
            { tag: "both", arm: 1, armPercentage: 50 },
            { tag: "negative-count", arm: -1 },
            { tag: "fractional-count", arm: 1.5 },
            { tag: "over-u32", arm: 4294967296 },
            { tag: "negative-percentage", armPercentage: -0.1 },
            { tag: "high-percentage", armPercentage: 100.1 },
            { tag: "nonfinite-percentage", armPercentage: NaN },
            { tag: "bad-levels", arm: 1, levels: ["campaign", 2] },
            { tag: "count-zero", arm: 5 }
        ] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { triggerPools = {
            { tag = "count-zero", arm = 0 },
            { tag = "percentage-zero", armPercentage = 0, levels = { "campaign" } },
            42,
            { tag = "", arm = 1 },
            { arm = 1 },
            { tag = "neither" },
            { tag = "both", arm = 1, armPercentage = 50 },
            { tag = "negative-count", arm = -1 },
            { tag = "fractional-count", arm = 1.5 },
            { tag = "over-u32", arm = 4294967296 },
            { tag = "negative-percentage", armPercentage = -0.1 },
            { tag = "high-percentage", armPercentage = 100.1 },
            { tag = "nonfinite-percentage", armPercentage = 0 / 0 },
            { tag = "bad-levels", arm = 1, levels = { "campaign", 2 } },
            { tag = "count-zero", arm = 5 }
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );
    let expected = vec![
        TriggerPoolDescriptor {
            tag: "count-zero".to_string(),
            arm: TriggerPoolArm::Count(0),
            levels: Vec::new(),
        },
        TriggerPoolDescriptor {
            tag: "percentage-zero".to_string(),
            arm: TriggerPoolArm::Percentage(0.0),
            levels: vec!["campaign".to_string()],
        },
    ];

    assert_eq!(js.trigger_pools, lua.trigger_pools);
    assert_eq!(js.trigger_pools, expected);
}

#[test]
fn malformed_reactions_do_not_discard_valid_manifest_siblings_in_either_vm() {
    let cases = [
        (
            r#"({ reactions: [{ name: "bad", primitive: "applyDamage", target: "@trigger", args: { amount: 5 } }, { name: "good", primitive: "playSound" }], crossings: [{ slot: "test.value", above: 1, fire: ["good"] }], triggerEvents: [{ trigger: 65536, event: "enter", fire: ["good"] }], uiTrees: [{ name: "good", tree: { anchor: "top", offset: [0, 0], root: { kind: "spacer", flexGrow: 1 } } }] })"#,
            r#"return { reactions = { { name = "bad", primitive = "applyDamage", target = "@trigger", args = { amount = 5 } }, { name = "good", primitive = "playSound" } }, crossings = { { slot = "test.value", above = 1, fire = { "good" } } }, triggerEvents = { { trigger = 65536, event = "enter", fire = { "good" } } }, uiTrees = { { name = "good", tree = { anchor = "top", offset = { 0, 0 }, root = { kind = "spacer", flexGrow = 1 } } } } }"#,
        ),
        (
            r#"({ reactions: [{ name: "bad", primitive: "applyDamage", target: "@unknown", args: { amount: 5 } }, { name: "good", primitive: "playSound" }], crossings: [{ slot: "test.value", above: 1, fire: ["good"] }], triggerEvents: [{ trigger: 65536, event: "enter", fire: ["good"] }], uiTrees: [{ name: "good", tree: { anchor: "top", offset: [0, 0], root: { kind: "spacer", flexGrow: 1 } } }] })"#,
            r#"return { reactions = { { name = "bad", primitive = "applyDamage", target = "@unknown", args = { amount = 5 } }, { name = "good", primitive = "playSound" } }, crossings = { { slot = "test.value", above = 1, fire = { "good" } } }, triggerEvents = { { trigger = 65536, event = "enter", fire = { "good" } } }, uiTrees = { { name = "good", tree = { anchor = "top", offset = { 0, 0 }, root = { kind = "spacer", flexGrow = 1 } } } } }"#,
        ),
        (
            r#"({ reactions: [{ name: "bad", primitive: "applyDamage", target: "@activators", tag: "enemy", args: { amount: 5 } }, { name: "good", primitive: "playSound" }], crossings: [{ slot: "test.value", above: 1, fire: ["good"] }], triggerEvents: [{ trigger: 65536, event: "enter", fire: ["good"] }], uiTrees: [{ name: "good", tree: { anchor: "top", offset: [0, 0], root: { kind: "spacer", flexGrow: 1 } } }] })"#,
            r#"return { reactions = { { name = "bad", primitive = "applyDamage", target = "@activators", tag = "enemy", args = { amount = 5 } }, { name = "good", primitive = "playSound" } }, crossings = { { slot = "test.value", above = 1, fire = { "good" } } }, triggerEvents = { { trigger = 65536, event = "enter", fire = { "good" } } }, uiTrees = { { name = "good", tree = { anchor = "top", offset = { 0, 0 }, root = { kind = "spacer", flexGrow = 1 } } } } }"#,
        ),
        (
            r#"({ reactions: [{ name: "bad", sequence: [{ id: "@occupancy", primitive: "armTrigger", args: {} }] }, { name: "good", primitive: "playSound" }], crossings: [{ slot: "test.value", above: 1, fire: ["good"] }], triggerEvents: [{ trigger: 65536, event: "enter", fire: ["good"] }], uiTrees: [{ name: "good", tree: { anchor: "top", offset: [0, 0], root: { kind: "spacer", flexGrow: 1 } } }] })"#,
            r#"return { reactions = { { name = "bad", sequence = { { id = "@occupancy", primitive = "armTrigger", args = {} } } }, { name = "good", primitive = "playSound" } }, crossings = { { slot = "test.value", above = 1, fire = { "good" } } }, triggerEvents = { { trigger = 65536, event = "enter", fire = { "good" } } }, uiTrees = { { name = "good", tree = { anchor = "top", offset = { 0, 0 }, root = { kind = "spacer", flexGrow = 1 } } } } }"#,
        ),
    ];

    for (js_source, lua_source) in cases {
        let js = eval_js(js_source, |ctx, value| {
            LevelManifest::from_js_value(ctx, value).unwrap()
        });
        let lua = eval_lua(lua_source, |value| {
            LevelManifest::from_lua_value(value).unwrap()
        });
        assert_eq!(js, lua, "both runtimes must retain the same valid siblings");
        assert_eq!(
            js.reactions
                .iter()
                .map(|reaction| reaction.name.as_str())
                .collect::<Vec<_>>(),
            ["good"]
        );
        assert_eq!(js.crossings.len(), 1);
        assert_eq!(js.trigger_events.len(), 1);
        assert_eq!(js.ui_trees.len(), 1);
    }
}

#[test]
fn grant_reactions_accept_tag_and_activator_forms_identically_in_both_vms() {
    let js = eval_js(
        r#"({ reactions: [
            { name: "health", primitive: "grantHealth", tag: "player", args: { amount: 12.5 } },
            { name: "ammo", primitive: "grantAmmo", target: "@activators", args: { type: "bullets.light", amount: 8 } }
        ] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { reactions = {
            { name = "health", primitive = "grantHealth", tag = "player", args = { amount = 12.5 } },
            { name = "ammo", primitive = "grantAmmo", target = "@activators", args = { type = "bullets.light", amount = 8 } },
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js, lua);
    let [health, ammo] = js.reactions.as_slice() else {
        panic!("both valid grant reactions must load");
    };
    let ReactionDescriptor::Primitive(health) = &health.descriptor else {
        panic!("health grant must remain primitive");
    };
    assert_eq!(health.tag.as_deref(), Some("player"));
    let ReactionDescriptor::Primitive(ammo) = &ammo.descriptor else {
        panic!("ammo grant must remain primitive");
    };
    assert_eq!(ammo.target.as_deref(), Some("@activators"));
}

#[test]
fn malformed_grant_reactions_reject_the_whole_manifest_identically_in_both_vms() {
    let cases = [
        (
            r#"({ reactions: [{ name: "missingTarget", primitive: "grantHealth", args: { amount: 1 } }, { name: "good", primitive: "playSound" }] })"#,
            r#"return { reactions = { { name = "missingTarget", primitive = "grantHealth", args = { amount = 1 } }, { name = "good", primitive = "playSound" } } }"#,
            "requires exactly one",
        ),
        (
            r#"({ reactions: [{ name: "emptyTag", primitive: "grantHealth", tag: "", args: { amount: 1 } }, { name: "good", primitive: "playSound" }] })"#,
            r#"return { reactions = { { name = "emptyTag", primitive = "grantHealth", tag = "", args = { amount = 1 } }, { name = "good", primitive = "playSound" } } }"#,
            "non-empty `tag`",
        ),
        (
            r#"({ reactions: [{ name: "runtimeAmount", primitive: "grantHealth", tag: "player", args: { amount: { op: "const", value: 1 } } }] })"#,
            r#"return { reactions = { { name = "runtimeAmount", primitive = "grantHealth", tag = "player", args = { amount = { op = "const", value = 1 } } } } }"#,
            "args.amount",
        ),
        (
            r#"({ reactions: [{ name: "f64OnlyAmount", primitive: "grantAmmo", tag: "player", args: { type: "bullets.light", amount: 1e100 } }, { name: "good", primitive: "playSound" }] })"#,
            r#"return { reactions = { { name = "f64OnlyAmount", primitive = "grantAmmo", tag = "player", args = { type = "bullets.light", amount = 1e100 } }, { name = "good", primitive = "playSound" } } }"#,
            "representable as f32",
        ),
        (
            r#"({ reactions: [{ name: "badPool", primitive: "grantAmmo", tag: "player", args: { type: "bad pool", amount: 1 } }] })"#,
            r#"return { reactions = { { name = "badPool", primitive = "grantAmmo", tag = "player", args = { type = "bad pool", amount = 1 } } } }"#,
            "grantAmmo.type",
        ),
        (
            r#"({ reactions: [{ name: "both", primitive: "grantAmmo", tag: "player", target: "@activators", args: { type: "bullets.light", amount: 1 } }] })"#,
            r#"return { reactions = { { name = "both", primitive = "grantAmmo", tag = "player", target = "@activators", args = { type = "bullets.light", amount = 1 } } } }"#,
            "both `target` and `tag`",
        ),
    ];

    for (js_source, lua_source, expected) in cases {
        let js_error = eval_js(js_source, |ctx, value| {
            LevelManifest::from_js_value(ctx, value).unwrap_err()
        });
        let lua_error = eval_lua(lua_source, |value| {
            LevelManifest::from_lua_value(value).unwrap_err()
        });
        assert!(js_error.to_string().contains(expected), "{js_error}");
        assert!(lua_error.to_string().contains(expected), "{lua_error}");
    }
}

#[test]
fn non_string_crossing_edges_degrade_identically_in_both_vms() {
    // Regression: VM field readers rejected these descriptors before shared
    // edge normalization could warn and preserve shipped single-edge behavior.
    let js = eval_js(
        r#"({ crossings: [{ slot: "test.value", above: 1, edge: 42, fire: ["go"] }] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { crossings = { { slot = "test.value", above = 1, edge = 42, fire = { "go" } } } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js.crossings, lua.crossings);
    assert_eq!(js.crossings[0].edge, None);
}

#[test]
fn js_predicate_crossing_uses_predicate_as_the_wire_discriminant() {
    let src = r#"({
        crossings: [{
            // `slot` is deliberately present: predicate presence must select
            // the IR form rather than attempting threshold validation.
            slot: "ignored.by.predicate",
            predicate: {
                op: "ge",
                a: { op: "input", name: "test.a" },
                b: { op: "const", value: 2 }
            },
            fire: ["ready"]
        }]
    })"#;
    let manifest = eval_js(src, |ctx, value| {
        LevelManifest::from_js_value(ctx, value).unwrap()
    });

    let crossing = &manifest.crossings[0];
    assert!(crossing.slot.is_none());
    assert!(matches!(
        crossing.condition,
        CrossingCondition::Ir(IrNode::Ge { .. })
    ));
    assert_eq!(crossing.fire, vec!["ready".to_string()]);
}

#[test]
fn lua_predicate_crossing_uses_predicate_as_the_wire_discriminant() {
    let src = r#"return {
        crossings = {{
            slot = "ignored.by.predicate",
            predicate = {
                op = "ge",
                a = { op = "input", name = "test.a" },
                b = { op = "const", value = 2 },
            },
            fire = { "ready" },
        }}
    }"#;
    let manifest = eval_lua(src, |value| LevelManifest::from_lua_value(value).unwrap());

    let crossing = &manifest.crossings[0];
    assert!(crossing.slot.is_none());
    assert!(matches!(
        crossing.condition,
        CrossingCondition::Ir(IrNode::Ge { .. })
    ));
    assert_eq!(crossing.fire, vec!["ready".to_string()]);
}

#[test]
fn lua_crossings_reject_non_dense_tables() {
    // Regression: raw_len iteration silently dropped map-shaped and sparse
    // crossing watcher declarations.
    let cases = [
        (
            "return { crossings = { named = { slot = \"s\", below = 1, fire = {} } } }",
            "map",
        ),
        (
            "return { crossings = { { slot = \"s\", below = 1, fire = {} }, extra = {} } }",
            "extra",
        ),
        (
            "return { crossings = { [2] = { slot = \"s\", below = 1, fire = {} } } }",
            "hole",
        ),
        (
            "return { crossings = { [0] = { slot = \"s\", below = 1, fire = {} } } }",
            "zero",
        ),
        (
            "return { crossings = { [1.5] = { slot = \"s\", below = 1, fire = {} } } }",
            "float",
        ),
    ];

    for (source, label) in cases {
        let err = eval_lua(source, |v| LevelManifest::from_lua_value(v).unwrap_err());
        assert!(
            err.to_string().contains("dense array"),
            "{label} produced unexpected error: {err}"
        );
    }
}

#[test]
fn lua_crossing_fire_rejects_non_dense_tables() {
    // Regression: raw_len iteration silently dropped malformed fire entries.
    let cases = [
        (
            "return { crossings = { { slot = \"s\", below = 1, fire = { named = \"event\" } } } }",
            "map",
        ),
        (
            "return { crossings = { { slot = \"s\", below = 1, fire = { \"event\", extra = \"other\" } } } }",
            "extra",
        ),
        (
            "return { crossings = { { slot = \"s\", below = 1, fire = { [2] = \"event\" } } } }",
            "hole",
        ),
        (
            "return { crossings = { { slot = \"s\", below = 1, fire = { [0] = \"event\" } } } }",
            "zero",
        ),
        (
            "return { crossings = { { slot = \"s\", below = 1, fire = { [1.5] = \"event\" } } } }",
            "float",
        ),
    ];

    for (source, label) in cases {
        let err = eval_lua(source, |v| LevelManifest::from_lua_value(v).unwrap_err());
        assert!(
            err.to_string().contains("dense array"),
            "{label} produced unexpected error: {err}"
        );
    }
}

// A group entry parses to the same descriptor in both runtimes. A sequence
// entry `{ primitive, kind, tag?, args }` becomes a group step; a primitive
// descriptor keeps its optional tag beside the kind; a kindless descriptor is
// unchanged raw data.
#[test]
fn group_entries_parse_identically_in_both_vms() {
    let js = eval_js(
        r#"({ reactions: [
            { name: "closet", sequence: [
                { kind: "npc", tag: "x", primitive: "applyDamage", args: { amount: 5 } },
                { id: "@wait", primitive: "wait", args: { durationMs: 800 } },
                { kind: "player", primitive: "grantHealth", args: { amount: 10 } },
            ] },
            { name: "resupply", primitive: "grantHealth", kind: "player", args: { amount: 10 } },
            { name: "raw", primitive: "applyDamage", tag: "x", args: { amount: 5 } },
        ] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { reactions = {
            { name = "closet", sequence = {
                { kind = "npc", tag = "x", primitive = "applyDamage", args = { amount = 5 } },
                { id = "@wait", primitive = "wait", args = { durationMs = 800 } },
                { kind = "player", primitive = "grantHealth", args = { amount = 10 } },
            } },
            { name = "resupply", primitive = "grantHealth", kind = "player", args = { amount = 10 } },
            { name = "raw", primitive = "applyDamage", tag = "x", args = { amount = 5 } },
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );
    assert_eq!(js.reactions, lua.reactions);
    assert_eq!(js.reactions.len(), 3);

    let ReactionDescriptor::Sequence(steps) = &js.reactions[0].descriptor else {
        panic!("expected sequence");
    };
    assert_eq!(
        steps[0].id,
        SequenceTarget::Group(GroupTarget {
            kind: GroupKind::Npc,
            tag: Some("x".to_string()),
        })
    );
    assert_eq!(steps[1].id, SequenceTarget::Wait);
    assert_eq!(
        steps[2].id,
        SequenceTarget::Group(GroupTarget {
            kind: GroupKind::Player,
            tag: None,
        })
    );

    let ReactionDescriptor::Primitive(resupply) = &js.reactions[1].descriptor else {
        panic!("expected primitive");
    };
    assert_eq!(resupply.kind, Some(GroupKind::Player));
    assert_eq!(resupply.tag, None);

    let ReactionDescriptor::Primitive(raw) = &js.reactions[2].descriptor else {
        panic!("expected primitive");
    };
    assert_eq!(
        raw.kind, None,
        "a kindless descriptor stays on the raw tag path"
    );
    assert_eq!(raw.tag.as_deref(), Some("x"));
}

// Both runtimes reject, naming the reaction, an entry or primitive
// descriptor that carries both `id` and `kind`, or a `kind` other than
// `npc`/`player`. The manifest drain skips the reaction and warns with that name.
#[test]
fn group_kind_rejections_name_the_reaction_in_both_vms() {
    let cases = [
        (
            r#"({ name: "bad", sequence: [{ id: 65536, kind: "npc", primitive: "applyDamage", args: { amount: 1 } }] })"#,
            r#"return { name = "bad", sequence = { { id = 65536, kind = "npc", primitive = "applyDamage", args = { amount = 1 } } } }"#,
            "reaction `bad` sequence step",
            "cannot carry both `id` and `kind`",
        ),
        (
            r#"({ name: "bad", sequence: [{ id: "@activators", kind: "player", primitive: "grantHealth", args: { amount: 1 } }] })"#,
            r#"return { name = "bad", sequence = { { id = "@activators", kind = "player", primitive = "grantHealth", args = { amount = 1 } } } }"#,
            "reaction `bad` sequence step",
            "cannot carry both `id` and `kind`",
        ),
        (
            r#"({ name: "bad", primitive: "applyDamage", id: 65536, kind: "npc", args: { amount: 1 } })"#,
            r#"return { name = "bad", primitive = "applyDamage", id = 65536, kind = "npc", args = { amount = 1 } }"#,
            "reaction `bad` primitive",
            "cannot carry both `id` and `kind`",
        ),
        (
            r#"({ name: "bad", sequence: [{ kind: "enemy", primitive: "applyDamage", args: { amount: 1 } }] })"#,
            r#"return { name = "bad", sequence = { { kind = "enemy", primitive = "applyDamage", args = { amount = 1 } } } }"#,
            "reaction `bad` sequence step",
            "`kind` must be \"npc\" or \"player\", got \"enemy\"",
        ),
        (
            r#"({ name: "bad", primitive: "applyDamage", kind: "enemy", tag: "x", args: { amount: 1 } })"#,
            r#"return { name = "bad", primitive = "applyDamage", kind = "enemy", tag = "x", args = { amount = 1 } }"#,
            "reaction `bad` primitive",
            "`kind` must be \"npc\" or \"player\", got \"enemy\"",
        ),
        (
            r#"({ name: "bad", primitive: "applyDamage", kind: 1, args: { amount: 1 } })"#,
            r#"return { name = "bad", primitive = "applyDamage", kind = 1, args = { amount = 1 } }"#,
            "reaction `bad` primitive",
            "`kind` must be \"npc\" or \"player\"",
        ),
        (
            r#"({ name: "bad", primitive: "grantHealth", kind: "player", target: "@activators", args: { amount: 1 } })"#,
            r#"return { name = "bad", primitive = "grantHealth", kind = "player", target = "@activators", args = { amount = 1 } }"#,
            "reaction `bad` primitive",
            "cannot carry both `target` and `kind`",
        ),
    ];

    for (js_source, lua_source, site, reason) in cases {
        let js_error = eval_js(js_source, |ctx, value| {
            named_reaction_from_js(ctx, value).unwrap_err()
        });
        let lua_error = eval_lua(lua_source, |value| {
            named_reaction_from_lua(value).unwrap_err()
        });
        for error in [js_error.to_string(), lua_error.to_string()] {
            assert!(
                error.contains(site) && error.contains(reason),
                "unexpected diagnostic for {js_source}: {error}"
            );
        }
        // Identical apart from a non-string value's VM type name.
        if !js_error.to_string().contains(", got a ") {
            assert_eq!(js_error.to_string(), lua_error.to_string());
        }
    }

    // At the manifest drain the rejection skips only that reaction, and the
    // skip warning names it.
    let capture = postretro_test_log_capture::LogCapture::start();
    let manifest = eval_js(
        r#"({ reactions: [
            { name: "badGroup", sequence: [{ kind: "enemy", primitive: "applyDamage", args: { amount: 1 } }] },
            { name: "ok", sequence: [{ kind: "npc", primitive: "applyDamage", args: { amount: 1 } }] },
        ] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    assert_eq!(manifest.reactions.len(), 1);
    assert_eq!(manifest.reactions[0].name, "ok");
    capture.assert_logged_once(log::Level::Warn, "reaction `badGroup` sequence step 0");
}

// A subject-token command `{ primitive, target, args }` is one descriptor in
// both positions: a reaction body keeps `target`, and a sequence entry lowers
// it to the same token arm the raw `id` sentinel reaches. Both runtimes agree.
#[test]
fn subject_token_entries_parse_identically_in_both_vms() {
    let js = eval_js(
        r#"({ reactions: [
            { name: "ambush", sequence: [
                { primitive: "grantHealth", target: "@activators", args: { amount: 10 } },
                { primitive: "disarmTrigger", target: "@trigger", args: {} },
                { id: "@wait", primitive: "wait", args: { durationMs: 800 } },
            ] },
            { name: "rearm", primitive: "armTrigger", target: "@trigger", args: {} },
            { name: "raw", sequence: [{ id: "@trigger", primitive: "disarmTrigger", args: {} }] },
        ] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { reactions = {
            { name = "ambush", sequence = {
                { primitive = "grantHealth", target = "@activators", args = { amount = 10 } },
                { primitive = "disarmTrigger", target = "@trigger", args = {} },
                { id = "@wait", primitive = "wait", args = { durationMs = 800 } },
            } },
            { name = "rearm", primitive = "armTrigger", target = "@trigger", args = {} },
            { name = "raw", sequence = { { id = "@trigger", primitive = "disarmTrigger", args = {} } } },
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );
    assert_eq!(js.reactions, lua.reactions);
    assert_eq!(js.reactions.len(), 3);

    let ReactionDescriptor::Sequence(steps) = &js.reactions[0].descriptor else {
        panic!("expected sequence");
    };
    let targets: Vec<_> = steps.iter().map(|step| step.id.clone()).collect();
    assert_eq!(
        targets,
        vec![
            SequenceTarget::Activators,
            SequenceTarget::FiredTrigger,
            SequenceTarget::Wait,
        ]
    );

    let ReactionDescriptor::Primitive(rearm) = &js.reactions[1].descriptor else {
        panic!("a token command body stays a primitive");
    };
    assert_eq!(rearm.target.as_deref(), Some("@trigger"));
    assert_eq!(rearm.kind, None);
    assert_eq!(rearm.tag, None);

    let ReactionDescriptor::Sequence(raw) = &js.reactions[2].descriptor else {
        panic!("expected sequence");
    };
    assert_eq!(
        raw[0], steps[1],
        "the raw `id` sentinel and the `target` entry are one step"
    );
}

// Both runtimes reject, naming the reaction, a subject-token entry or body that
// carries `target` beside `id`, `kind` or `tag`, an unknown sentinel, or a verb
// its subject lacks. The manifest drain skips only that reaction.
#[test]
fn subject_token_rejections_name_the_reaction_in_both_vms() {
    let cases = [
        (
            r#"({ name: "bad", sequence: [{ id: 65536, target: "@activators", primitive: "grantHealth", args: { amount: 1 } }] })"#,
            r#"return { name = "bad", sequence = { { id = 65536, target = "@activators", primitive = "grantHealth", args = { amount = 1 } } } }"#,
            "reaction `bad` sequence step",
            "cannot carry both `target` and `id`",
        ),
        (
            r#"({ name: "bad", sequence: [{ id: "@trigger", target: "@trigger", primitive: "armTrigger", args: {} }] })"#,
            r#"return { name = "bad", sequence = { { id = "@trigger", target = "@trigger", primitive = "armTrigger", args = {} } } }"#,
            "reaction `bad` sequence step",
            "cannot carry both `target` and `id`",
        ),
        (
            r#"({ name: "bad", sequence: [{ kind: "player", target: "@activators", primitive: "grantHealth", args: { amount: 1 } }] })"#,
            r#"return { name = "bad", sequence = { { kind = "player", target = "@activators", primitive = "grantHealth", args = { amount = 1 } } } }"#,
            "reaction `bad` sequence step",
            "cannot carry both `target` and `kind`",
        ),
        (
            r#"({ name: "bad", sequence: [{ tag: "x", target: "@activators", primitive: "grantHealth", args: { amount: 1 } }] })"#,
            r#"return { name = "bad", sequence = { { tag = "x", target = "@activators", primitive = "grantHealth", args = { amount = 1 } } } }"#,
            "reaction `bad` sequence step",
            "cannot carry both `target` and `tag`",
        ),
        (
            r#"({ name: "bad", sequence: [{ target: "@wait", primitive: "wait", args: { durationMs: 1 } }] })"#,
            r#"return { name = "bad", sequence = { { target = "@wait", primitive = "wait", args = { durationMs = 1 } } } }"#,
            "reaction `bad` sequence step",
            "`target` must be \"@activators\" or \"@trigger\", got \"@wait\"",
        ),
        (
            r#"({ name: "bad", sequence: [{ target: 7, primitive: "grantHealth", args: { amount: 1 } }] })"#,
            r#"return { name = "bad", sequence = { { target = 7, primitive = "grantHealth", args = { amount = 1 } } } }"#,
            "reaction `bad` sequence step",
            "`target` must be \"@activators\" or \"@trigger\", got a",
        ),
        (
            r#"({ name: "bad", primitive: "applyDamage", target: "@everyone", args: { amount: 1 } })"#,
            r#"return { name = "bad", primitive = "applyDamage", target = "@everyone", args = { amount = 1 } }"#,
            "reaction `bad` primitive",
            "`target` must be \"@activators\" or \"@trigger\", got \"@everyone\"",
        ),
        (
            r#"({ name: "bad", sequence: [{ target: "@activators", primitive: "armTrigger", args: {} }] })"#,
            r#"return { name = "bad", sequence = { { target = "@activators", primitive = "armTrigger", args = {} } } }"#,
            "reaction `bad` sequence step",
            "takes `@trigger`",
        ),
        (
            r#"({ name: "bad", primitive: "applyDamage", target: "@trigger", args: { amount: 1 } })"#,
            r#"return { name = "bad", primitive = "applyDamage", target = "@trigger", args = { amount = 1 } }"#,
            "reaction `bad` primitive",
            "carries only `armTrigger` and `disarmTrigger`",
        ),
        (
            r#"({ name: "bad", sequence: [{ id: "@trigger", primitive: "moverStart", args: {} }] })"#,
            r#"return { name = "bad", sequence = { { id = "@trigger", primitive = "moverStart", args = {} } } }"#,
            "reaction `bad` sequence step",
            "carries only `armTrigger` and `disarmTrigger`",
        ),
    ];

    for (js_source, lua_source, site, reason) in cases {
        let js_error = eval_js(js_source, |ctx, value| {
            named_reaction_from_js(ctx, value).unwrap_err()
        });
        let lua_error = eval_lua(lua_source, |value| {
            named_reaction_from_lua(value).unwrap_err()
        });
        for error in [js_error.to_string(), lua_error.to_string()] {
            assert!(
                error.contains(site) && error.contains(reason),
                "unexpected diagnostic for {js_source}: {error}"
            );
        }
        // Identical apart from a non-string value's VM type name.
        if !js_error.to_string().contains(", got a ") {
            assert_eq!(js_error.to_string(), lua_error.to_string());
        }
    }

    let capture = postretro_test_log_capture::LogCapture::start();
    let manifest = eval_js(
        r#"({ reactions: [
            { name: "badToken", sequence: [{ id: 65536, target: "@activators", primitive: "grantHealth", args: { amount: 1 } }] },
            { name: "ok", sequence: [{ target: "@activators", primitive: "grantHealth", args: { amount: 1 } }] },
        ] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    assert_eq!(manifest.reactions.len(), 1);
    assert_eq!(manifest.reactions[0].name, "ok");
    capture.assert_logged_once(log::Level::Warn, "reaction `badToken` sequence step 0");
}

/// Parse one reaction in both runtimes, expect both to reject it, and return
/// the shared diagnostic after asserting the two are identical.
fn identical_rejection(js_source: &str, lua_source: &str) -> String {
    let js_error = eval_js(js_source, |ctx, value| {
        named_reaction_from_js(ctx, value).unwrap_err()
    })
    .to_string();
    let lua_error = eval_lua(lua_source, |value| {
        named_reaction_from_lua(value).unwrap_err()
    })
    .to_string();
    assert_eq!(js_error, lua_error, "the runtimes' diagnostics diverged");
    js_error
}

// A group or subject-token sequence step carries the same grant payload as a
// reaction body, so both runtimes run the same load-time payload check on it
// and name the reaction and step. The grant handlers rely on that check.
#[test]
fn sequence_step_grant_payloads_are_validated_like_bodies_in_both_vms() {
    let cases = [
        (
            r#"({ name: "bad", sequence: [{ kind: "player", primitive: "grantAmmo", args: { type: "bad key!", amount: 1 } }] })"#,
            r#"return { name = "bad", sequence = { { kind = "player", primitive = "grantAmmo", args = { type = "bad key!", amount = 1 } } } }"#,
            "reaction `bad` sequence step 0: `grantAmmo.type` must match",
        ),
        (
            r#"({ name: "bad", sequence: [{ kind: "player", primitive: "grantAmmo", args: { type: "shells", amount: 1e40 } }] })"#,
            r#"return { name = "bad", sequence = { { kind = "player", primitive = "grantAmmo", args = { type = "shells", amount = 1e40 } } } }"#,
            "reaction `bad` sequence step 0: `grantAmmo` `args.amount` must be a finite number representable as f32",
        ),
        (
            r#"({ name: "bad", sequence: [{ target: "@activators", primitive: "grantHealth", args: { amount: "lots" } }] })"#,
            r#"return { name = "bad", sequence = { { target = "@activators", primitive = "grantHealth", args = { amount = "lots" } } } }"#,
            "reaction `bad` sequence step 0: `grantHealth` `args.amount` must be a finite number",
        ),
        (
            r#"({ name: "bad", sequence: [{ target: "@activators", primitive: "addSlot", args: { delta: 1 } }] })"#,
            r#"return { name = "bad", sequence = { { target = "@activators", primitive = "addSlot", args = { delta = 1 } } } }"#,
            "reaction `bad` sequence step 0: `addSlot` `args.slot` must be a string",
        ),
        (
            r#"({ name: "bad", sequence: [{ id: "@wait", primitive: "wait", args: { durationMs: 1 } }, { kind: "player", primitive: "addSlot", args: { slot: "xp", delta: 1e40 } }] })"#,
            r#"return { name = "bad", sequence = { { id = "@wait", primitive = "wait", args = { durationMs = 1 } }, { kind = "player", primitive = "addSlot", args = { slot = "xp", delta = 1e40 } } } }"#,
            "reaction `bad` sequence step 1: `addSlot` `args.delta` must be a finite number representable as f32",
        ),
        (
            r#"({ name: "bad", sequence: [{ kind: "player", primitive: "grantHealth" }] })"#,
            r#"return { name = "bad", sequence = { { kind = "player", primitive = "grantHealth" } } }"#,
            "reaction `bad` sequence step 0: `grantHealth` `args` must be an object",
        ),
    ];
    for (js_source, lua_source, expected) in cases {
        let error = identical_rejection(js_source, lua_source);
        assert!(error.contains(expected), "unexpected diagnostic: {error}");
    }

    // A failing step skips only its reaction, with a warning naming it; unlike
    // a malformed grant body, it does not reject the whole manifest.
    let capture = postretro_test_log_capture::LogCapture::start();
    let js = eval_js(
        r#"({ reactions: [
            { name: "badAmmo", sequence: [{ kind: "player", primitive: "grantAmmo", args: { type: "bad key!", amount: 1 } }] },
            { name: "ok", sequence: [{ kind: "player", primitive: "grantAmmo", args: { type: "shells", amount: 8 } }] },
        ] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { reactions = {
            { name = "badAmmo", sequence = { { kind = "player", primitive = "grantAmmo", args = { type = "bad key!", amount = 1 } } } },
            { name = "ok", sequence = { { kind = "player", primitive = "grantAmmo", args = { type = "shells", amount = 8 } } } },
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );
    assert_eq!(js.reactions, lua.reactions);
    assert_eq!(js.reactions.len(), 1);
    assert_eq!(js.reactions[0].name, "ok");
    assert_identical_warning_per_runtime(
        &capture,
        "reactions[0] is malformed and was skipped: invalid sequence step: reaction `badAmmo` sequence step 0",
    );
}

// `spawnFromSpawner` addresses spawners, never a group: a `kind` beside it is
// rejected in both runtimes, as a body and as a sequence entry, naming the
// reaction. Raw-only — the SDK has no group verb that emits it.
#[test]
fn spawn_from_spawner_with_a_group_kind_is_rejected_in_both_vms() {
    let cases = [
        (
            r#"({ name: "bad", primitive: "spawnFromSpawner", kind: "npc", tag: "closet" })"#,
            r#"return { name = "bad", primitive = "spawnFromSpawner", kind = "npc", tag = "closet" }"#,
            "reaction `bad` primitive: `spawnFromSpawner` addresses spawners, so it cannot carry a group `kind`",
        ),
        (
            r#"({ name: "bad", sequence: [{ kind: "npc", tag: "closet", primitive: "spawnFromSpawner" }] })"#,
            r#"return { name = "bad", sequence = { { kind = "npc", tag = "closet", primitive = "spawnFromSpawner" } } }"#,
            "reaction `bad` sequence step 0: `spawnFromSpawner` addresses spawners, so it cannot carry a group `kind`",
        ),
    ];
    for (js_source, lua_source, expected) in cases {
        let error = identical_rejection(js_source, lua_source);
        assert!(error.contains(expected), "unexpected diagnostic: {error}");
    }
}

// Sequence-step diagnostics count steps from 0 in both runtimes, so a Luau
// author and a TS author read the same index for the same entry.
#[test]
fn sequence_step_diagnostics_count_from_zero_in_both_vms() {
    let cases = [
        (
            r#"({ name: "bad", sequence: [{ id: "@wait", primitive: "wait", args: { durationMs: 1 } }, { id: "@bogus", primitive: "wait", args: {} }] })"#,
            r#"return { name = "bad", sequence = { { id = "@wait", primitive = "wait", args = { durationMs = 1 } }, { id = "@bogus", primitive = "wait", args = {} } } }"#,
            "reaction `bad` sequence step 1: illegal sentinel `@bogus`",
        ),
        (
            r#"({ name: "bad", sequence: [{ id: "@wait", primitive: "wait", args: { durationMs: 1 } }, { id: "@wait", primitive: "fire", args: {} }] })"#,
            r#"return { name = "bad", sequence = { { id = "@wait", primitive = "wait", args = { durationMs = 1 } }, { id = "@wait", primitive = "fire", args = {} } } }"#,
            "reaction `bad` sequence step 1: sentinel `@wait` requires primitive `wait`, got `fire`",
        ),
        (
            r#"({ name: "bad", sequence: [{ id: "@wait", primitive: "wait", args: { durationMs: 1 } }, { kind: "enemy", primitive: "applyDamage", args: { amount: 1 } }] })"#,
            r#"return { name = "bad", sequence = { { id = "@wait", primitive = "wait", args = { durationMs = 1 } }, { kind = "enemy", primitive = "applyDamage", args = { amount = 1 } } } }"#,
            "reaction `bad` sequence step 1: `kind` must be",
        ),
    ];
    for (js_source, lua_source, expected) in cases {
        let error = identical_rejection(js_source, lua_source);
        assert!(error.contains(expected), "unexpected diagnostic: {error}");
    }
}
