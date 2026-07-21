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

#[test]
fn trigger_event_manifests_parse_identically_and_drop_unknown_events() {
    let js = eval_js(
        r#"({ triggerEvents: [
            { tag: "plate", event: "enter", fire: ["zap", "once"], levels: ["campaign"] },
            { tag: "plate", event: "occupied", fire: ["bad"] }
        ] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { triggerEvents = {
            { tag = "plate", event = "enter", fire = { "zap", "once" }, levels = { "campaign" } },
            { tag = "plate", event = "occupied", fire = { "bad" } }
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js.trigger_events, lua.trigger_events);
    assert_eq!(js.trigger_events.len(), 1);
    assert_eq!(js.trigger_events[0].fire, ["zap", "once"]);
}

// Regression: Luau rejected the whole `events` field for a sparse table while
// QuickJS skipped only the malformed slots and retained valid siblings.
#[test]
fn sparse_and_malformed_impact_events_keep_valid_siblings_in_both_vms() {
    let js = eval_js(
        r#"(() => {
            const events = [];
            events[0] = { kind: "impact", id: "salvage:first", filter: {}, policy: [] };
            events[2] = 42;
            events[3] = { kind: "impact", id: "salvage:last", filter: {}, policy: [] };
            return { events };
        })()"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { events = {
            [1] = { kind = "impact", id = "salvage:first", filter = {}, policy = {} },
            [3] = 42,
            [4] = { kind = "impact", id = "salvage:last", filter = {}, policy = {} },
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js.events, lua.events);
    assert_eq!(
        js.events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        ["salvage:first", "salvage:last"]
    );
}

#[test]
fn impact_event_ids_require_namespaced_portable_strings_in_both_vms() {
    let js = eval_js(
        r#"({ events: [
            { kind: "impact", id: "salvage:valid-id", filter: {}, policy: [] },
            { kind: "impact", id: "not-namespaced", filter: {}, policy: [] },
            { kind: "impact", id: ":missing", filter: {}, policy: [] },
            { kind: "impact", id: "bad space:id", filter: {}, policy: [] }
        ] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { events = {
            { kind = "impact", id = "salvage:valid-id", filter = {}, policy = {} },
            { kind = "impact", id = "not-namespaced", filter = {}, policy = {} },
            { kind = "impact", id = ":missing", filter = {}, policy = {} },
            { kind = "impact", id = "bad space:id", filter = {}, policy = {} },
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js.events, lua.events);
    assert_eq!(js.events.len(), 1);
    assert_eq!(js.events[0].id, "salvage:valid-id");
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
                triggerEvents: [{ tag: "plate", event: "enter", fire: ["good"] }]
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
            triggerEvents = { { tag = "plate", event = "enter", fire = { "good" } } }
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
                triggerEvents: [{ tag: "plate", event: "enter", fire: ["good"] }],
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
            r#"({ reactions: [{ name: "bad", primitive: "applyDamage", target: "@trigger", args: { amount: 5 } }, { name: "good", primitive: "playSound" }], crossings: [{ slot: "test.value", above: 1, fire: ["good"] }], triggerEvents: [{ tag: "plate", event: "enter", fire: ["good"] }], uiTrees: [{ name: "good", tree: { anchor: "top", offset: [0, 0], root: { kind: "spacer", flexGrow: 1 } } }] })"#,
            r#"return { reactions = { { name = "bad", primitive = "applyDamage", target = "@trigger", args = { amount = 5 } }, { name = "good", primitive = "playSound" } }, crossings = { { slot = "test.value", above = 1, fire = { "good" } } }, triggerEvents = { { tag = "plate", event = "enter", fire = { "good" } } }, uiTrees = { { name = "good", tree = { anchor = "top", offset = { 0, 0 }, root = { kind = "spacer", flexGrow = 1 } } } } }"#,
        ),
        (
            r#"({ reactions: [{ name: "bad", primitive: "applyDamage", target: "@unknown", args: { amount: 5 } }, { name: "good", primitive: "playSound" }], crossings: [{ slot: "test.value", above: 1, fire: ["good"] }], triggerEvents: [{ tag: "plate", event: "enter", fire: ["good"] }], uiTrees: [{ name: "good", tree: { anchor: "top", offset: [0, 0], root: { kind: "spacer", flexGrow: 1 } } }] })"#,
            r#"return { reactions = { { name = "bad", primitive = "applyDamage", target = "@unknown", args = { amount = 5 } }, { name = "good", primitive = "playSound" } }, crossings = { { slot = "test.value", above = 1, fire = { "good" } } }, triggerEvents = { { tag = "plate", event = "enter", fire = { "good" } } }, uiTrees = { { name = "good", tree = { anchor = "top", offset = { 0, 0 }, root = { kind = "spacer", flexGrow = 1 } } } } }"#,
        ),
        (
            r#"({ reactions: [{ name: "bad", primitive: "applyDamage", target: "@activators", tag: "enemy", args: { amount: 5 } }, { name: "good", primitive: "playSound" }], crossings: [{ slot: "test.value", above: 1, fire: ["good"] }], triggerEvents: [{ tag: "plate", event: "enter", fire: ["good"] }], uiTrees: [{ name: "good", tree: { anchor: "top", offset: [0, 0], root: { kind: "spacer", flexGrow: 1 } } }] })"#,
            r#"return { reactions = { { name = "bad", primitive = "applyDamage", target = "@activators", tag = "enemy", args = { amount = 5 } }, { name = "good", primitive = "playSound" } }, crossings = { { slot = "test.value", above = 1, fire = { "good" } } }, triggerEvents = { { tag = "plate", event = "enter", fire = { "good" } } }, uiTrees = { { name = "good", tree = { anchor = "top", offset = { 0, 0 }, root = { kind = "spacer", flexGrow = 1 } } } } }"#,
        ),
        (
            r#"({ reactions: [{ name: "bad", sequence: [{ id: "@occupancy", primitive: "armTrigger", args: {} }] }, { name: "good", primitive: "playSound" }], crossings: [{ slot: "test.value", above: 1, fire: ["good"] }], triggerEvents: [{ tag: "plate", event: "enter", fire: ["good"] }], uiTrees: [{ name: "good", tree: { anchor: "top", offset: [0, 0], root: { kind: "spacer", flexGrow: 1 } } }] })"#,
            r#"return { reactions = { { name = "bad", sequence = { { id = "@occupancy", primitive = "armTrigger", args = {} } } }, { name = "good", primitive = "playSound" } }, crossings = { { slot = "test.value", above = 1, fire = { "good" } } }, triggerEvents = { { tag = "plate", event = "enter", fire = { "good" } } }, uiTrees = { { name = "good", tree = { anchor = "top", offset = { 0, 0 }, root = { kind = "spacer", flexGrow = 1 } } } } }"#,
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
fn luau_trigger_target_tokens_preserve_wrong_builder_tokens_for_validation() {
    const DATA_SCRIPT_LUAU: &str = include_str!("../../../../../sdk/lib/data_script.luau");

    let lua = mlua::Lua::new();
    let sdk: mlua::Table = lua
        .load(DATA_SCRIPT_LUAU)
        .set_name("data_script.luau")
        .eval()
        .expect("data-script SDK must load");
    lua.globals().set("Postretro", sdk).unwrap();
    let value: LuaValue = lua
        .load(
            r#"
            return { reactions = {
                Postretro.defineReaction("wrongDamage", function(on)
                    return Postretro.damage(on.trigger, 5)
                end),
                Postretro.defineReaction("rightDamage", function(on)
                    return Postretro.damage(on.activators, 5)
                end),
                Postretro.defineReaction("wrongArm", function(on)
                    return { sequence = Postretro.armTrigger(on.activators) }
                end),
            } }
            "#,
        )
        .eval()
        .expect("Luau SDK must build descriptors");

    let manifest = LevelManifest::from_lua_value(value).expect("malformed siblings degrade");
    assert_eq!(
        manifest
            .reactions
            .iter()
            .map(|reaction| reaction.name.as_str())
            .collect::<Vec<_>>(),
        ["rightDamage"],
        "wrong opaque target tokens must reach the descriptor validator instead of lowering as valid targets"
    );
    let ReactionDescriptor::Primitive(primitive) = &manifest.reactions[0].descriptor else {
        panic!("remaining descriptor must be the valid damage reaction");
    };
    assert_eq!(primitive.target.as_deref(), Some("@activators"));
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
