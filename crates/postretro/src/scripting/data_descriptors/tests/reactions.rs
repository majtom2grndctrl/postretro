// Tests: JS/Luau reaction & crossing parsing.

use super::super::*;
use super::common::*;

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
fn js_missing_required_field_reports_missing_field() {
    // progress missing `fire`
    let src = r#"({
        reactions: [{ name: "x", progress: { tag: "t", at: 0.5 } }]
    })"#;
    let err = eval_js(src, |ctx, v| {
        LevelManifest::from_js_value(ctx, v).unwrap_err()
    });
    assert_eq!(err, DescriptorError::MissingField { field: "fire" });
}

#[test]
fn js_missing_name_field_reports_missing_field() {
    let src = r#"({
        reactions: [{ progress: { tag: "t", at: 0.5, fire: "f" } }]
    })"#;
    let err = eval_js(src, |ctx, v| {
        LevelManifest::from_js_value(ctx, v).unwrap_err()
    });
    assert_eq!(err, DescriptorError::MissingField { field: "name" });
}

#[test]
fn js_unknown_shape_reaction_is_rejected() {
    let src = r#"({
        reactions: [{ name: "x", tag: "t" }]
    })"#;
    let err = eval_js(src, |ctx, v| {
        LevelManifest::from_js_value(ctx, v).unwrap_err()
    });
    assert_eq!(err, DescriptorError::UnknownShape);
}

#[test]
fn js_empty_primitive_name_is_rejected() {
    let src = r#"({
        reactions: [{ name: "x", primitive: "", tag: "t" }]
    })"#;
    let err = eval_js(src, |ctx, v| {
        LevelManifest::from_js_value(ctx, v).unwrap_err()
    });
    assert_eq!(err, DescriptorError::EmptyPrimitiveName);
}

#[test]
fn js_at_out_of_range_high_is_rejected() {
    let src = r#"({
        reactions: [{ name: "x", progress: { tag: "t", at: 1.5, fire: "f" } }]
    })"#;
    let err = eval_js(src, |ctx, v| {
        LevelManifest::from_js_value(ctx, v).unwrap_err()
    });
    assert_eq!(err, DescriptorError::AtThresholdOutOfRange { value: 1.5 });
}

#[test]
fn js_at_out_of_range_negative_is_rejected() {
    let src = r#"({
        reactions: [{ name: "x", progress: { tag: "t", at: -0.1, fire: "f" } }]
    })"#;
    let err = eval_js(src, |ctx, v| {
        LevelManifest::from_js_value(ctx, v).unwrap_err()
    });
    match err {
        DescriptorError::AtThresholdOutOfRange { value } => {
            assert!((value + 0.1).abs() < 1e-6);
        }
        other => panic!("expected AtThresholdOutOfRange, got {other:?}"),
    }
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
            assert_eq!(steps[0].id.to_raw(), 65536);
            assert_eq!(steps[0].primitive, "moveGeometry");
            assert_eq!(steps[0].args["duration"].as_f64(), Some(1.5));
            assert_eq!(steps[1].id.to_raw(), 131072);
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
fn lua_missing_required_field_reports_missing_field() {
    let src = r#"return {
        reactions = { { name = "x", progress = { tag = "t", at = 0.5 } } }
    }"#;
    let err = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap_err());
    assert_eq!(err, DescriptorError::MissingField { field: "fire" });
}

#[test]
fn lua_unknown_shape_reaction_is_rejected() {
    let src = r#"return {
        reactions = { { name = "x", tag = "t" } }
    }"#;
    let err = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap_err());
    assert_eq!(err, DescriptorError::UnknownShape);
}

#[test]
fn lua_empty_primitive_name_is_rejected() {
    let src = r#"return {
        reactions = { { name = "x", primitive = "", tag = "t" } }
    }"#;
    let err = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap_err());
    assert_eq!(err, DescriptorError::EmptyPrimitiveName);
}

#[test]
fn lua_at_out_of_range_is_rejected() {
    let src = r#"return {
        reactions = { { name = "x", progress = { tag = "t", at = 1.5, fire = "f" } } }
    }"#;
    let err = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap_err());
    assert_eq!(err, DescriptorError::AtThresholdOutOfRange { value: 1.5 });
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
            assert_eq!(steps[0].id.to_raw(), 65536);
            assert_eq!(steps[0].primitive, "moveGeometry");
            assert_eq!(steps[1].primitive, "playSound");
        }
        other => panic!("expected sequence, got {other:?}"),
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
    assert_eq!(m.crossings[0].slot, "player.health");
    assert_eq!(m.crossings[0].fire, vec!["lowHealth".to_string()]);
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
