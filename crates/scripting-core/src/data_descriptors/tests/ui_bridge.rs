// Tests: UI bridge: kinds, binds, local-state.

use super::super::*;
use super::common::*;

#[test]
fn js_bridge_converts_all_kinds_tree_and_reserializes_byte_identically() {
    let tree = eval_js(UI_ALL_KINDS_JS, |ctx, v| {
        anchored_tree_from_js_value(ctx, v).expect("well-formed tree must convert")
    });

    // Typed structure assertions.
    assert_eq!(tree.anchor, Anchor::Center);
    assert_eq!(tree.offset, [10.0, -20.0]);
    assert_eq!(tree.capture_mode, CaptureMode::Passthrough);
    let Widget::VStack(root) = &tree.root else {
        panic!("root must be a vstack, got {:?}", tree.root);
    };
    assert_eq!(root.children.len(), 4);
    assert!(matches!(root.children[0], Widget::Text(_)));
    assert!(matches!(root.children[1], Widget::Panel(_)));
    assert!(matches!(root.children[3], Widget::Grid(_)));

    // Byte-identical re-serialization through the descriptor's own serde impl.
    let reserialized = serde_json::to_string(&tree).expect("must serialize");
    assert_eq!(reserialized, UI_ALL_KINDS_WIRE);
}

#[test]
fn js_bridge_parses_local_state_scope_and_local_bind() {
    // M13 G1b, Task 5: the G1a bridge must read a container's `localState`
    // declaration (scope + cells) AND a descendant `{ local }` bind, and the
    // result must re-serialize byte-identically through the descriptor's serde.
    let src = r#"({
        anchor: "center", offset: [0.0, 0.0],
        root: {
            kind: "vstack", gap: 0.0, padding: 0.0, align: "start",
            localState: { scope: "counter", cells: { count: 0.0, flash: [1.0, 0.0, 0.0, 1.0] } },
            children: [
                { kind: "text", content: "0", fontSize: 18.0, color: [1.0, 1.0, 1.0, 1.0], bind: { local: "count" } }
            ]
        }
    })"#;
    let tree = eval_js(src, |ctx, v| {
        anchored_tree_from_js_value(ctx, v).expect("localState tree must convert")
    });
    let Widget::VStack(root) = &tree.root else {
        panic!("root must be a vstack");
    };
    let ls = root.local_state.as_ref().expect("localState present");
    assert_eq!(ls.scope, "counter");
    assert_eq!(ls.cells.len(), 2);
    let Widget::Text(t) = &root.children[0] else {
        panic!("child must be text");
    };
    assert_eq!(
        t.bind.as_ref().map(|b| &b.source),
        Some(&BindSource::Local {
            local: "count".into()
        })
    );
    let wire = r#"{"anchor":"center","offset":[0.0,0.0],"root":{"kind":"vstack","gap":0.0,"padding":0.0,"align":"start","localState":{"scope":"counter","cells":{"count":0.0,"flash":[1.0,0.0,0.0,1.0]}},"children":[{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"local":"count"}}]}}"#;
    assert_eq!(serde_json::to_string(&tree).unwrap(), wire);
}

#[test]
fn lua_bridge_parses_local_state_scope_and_local_bind() {
    // The Luau twin: the `["local"]` keyword-escaped bind key + `localState`.
    let src = r#"return {
        anchor = "center", offset = {0.0, 0.0},
        root = {
            kind = "vstack", gap = 0.0, padding = 0.0, align = "start",
            localState = { scope = "counter", cells = { count = 0.0 } },
            children = {
                { kind = "text", content = "0", fontSize = 18.0, color = {1.0, 1.0, 1.0, 1.0}, bind = { ["local"] = "count" } }
            }
        }
    }"#;
    let tree = eval_lua(src, |v| {
        anchored_tree_from_lua_value(v).expect("localState tree must convert")
    });
    let Widget::VStack(root) = &tree.root else {
        panic!("root must be a vstack");
    };
    assert_eq!(root.local_state.as_ref().unwrap().scope, "counter");
    let Widget::Text(t) = &root.children[0] else {
        panic!("child must be text");
    };
    assert_eq!(
        t.bind.as_ref().map(|b| &b.source),
        Some(&BindSource::Local {
            local: "count".into()
        })
    );
}

#[test]
fn presentation_fact_bind_requires_template_and_parses_there_in_both_runtimes() {
    let js_tree = r#"({
        anchor: "center", offset: [0, 0],
        root: { kind: "text", content: "0", fontSize: 18, color: [1,1,1,1],
          bind: { fact: "value", format: "{}" } }
    })"#;
    let js_error = eval_js(js_tree, |ctx, value| {
        anchored_tree_from_js_value(ctx, value).unwrap_err()
    });
    assert!(
        js_error
            .to_string()
            .contains("outside a presentation template")
    );

    let lua_tree = r#"return {
        anchor = "center", offset = {0, 0},
        root = { kind = "text", content = "0", fontSize = 18, color = {1,1,1,1},
          bind = { fact = "value", format = "{}" } }
    }"#;
    let lua_error = eval_lua(lua_tree, |value| {
        anchored_tree_from_lua_value(value).unwrap_err()
    });
    assert!(
        lua_error
            .to_string()
            .contains("outside a presentation template")
    );

    let js_template = r#"({
        id: "damageNumber", lifetimeMs: 750,
        root: { kind: "text", content: "0", fontSize: 18, color: [1,1,1,1],
          bind: { fact: "value", format: "{}" } },
        motion: { rise: 18, easing: "easeOut" },
        fade: { startMs: 500 }, spawnScatter: { radius: 0.25 }
    })"#;
    let js_template = eval_js(js_template, |ctx, value| {
        presentation_template_from_js(ctx, value).expect("JS template fact bind must convert")
    });

    let lua_template = r#"return {
        id = "damageNumber", lifetimeMs = 750,
        root = { kind = "text", content = "0", fontSize = 18, color = {1,1,1,1},
          bind = { fact = "value", format = "{}" } },
        motion = { rise = 18, easing = "easeOut" },
        fade = { startMs = 500 }, spawnScatter = { radius = 0.25 }
    }"#;
    let lua_template = eval_lua(lua_template, |value| {
        presentation_template_from_lua(value).expect("Luau template fact bind must convert")
    });

    assert_eq!(js_template, lua_template);
    let Widget::Text(text) = js_template.root else {
        panic!("root must be text");
    };
    assert_eq!(
        text.bind.as_ref().map(|bind| &bind.source),
        Some(&BindSource::Fact {
            fact: "value".into()
        })
    );
}

#[test]
fn bar_bridge_accepts_sizing_and_exit_fade_and_rejects_invalid_authored_shapes() {
    let valid_js = r#"({
        anchor: "center", offset: [0.0, 0.0],
        root: { kind: "bar", bind: { slot: "player.reloadProgress" }, max: 1.0,
          fill: [0,1,0,1], background: [0,0,0,1], width: 120.0, height: 24.0,
          visibleWhen: { slot: "player.reloadActive", equals: true },
          exitFade: { durationMs: 500.0 } }
    })"#;
    let tree = eval_js(valid_js, |ctx, value| {
        anchored_tree_from_js_value(ctx, value).unwrap()
    });
    let Widget::Bar(bar) = tree.root else {
        panic!("root must be a bar");
    };
    assert_eq!(bar.width, Some(120.0));
    assert_eq!(bar.height, Some(24.0));
    assert_eq!(
        bar.exit_fade.as_ref().map(|fade| fade.duration_ms),
        Some(500.0)
    );

    let valid_lua = r#"return {
        anchor = "center", offset = {0.0, 0.0},
        root = { kind = "bar", bind = { slot = "player.reloadProgress" }, max = 1.0,
          fill = {0,1,0,1}, background = {0,0,0,1}, width = 120.0, height = 24.0,
          visibleWhen = { slot = "player.reloadActive", equals = true },
          exitFade = { durationMs = 500.0 } }
    }"#;
    let tree = eval_lua(valid_lua, |value| {
        anchored_tree_from_lua_value(value).expect("well-formed Luau bar must convert")
    });
    let Widget::Bar(bar) = tree.root else {
        panic!("root must be a bar");
    };
    assert_eq!(bar.width, Some(120.0));
    assert_eq!(bar.height, Some(24.0));
    assert_eq!(
        bar.exit_fade.as_ref().map(|fade| fade.duration_ms),
        Some(500.0)
    );

    for invalid_js in [
        r#"({ anchor:"center", offset:[0,0], root:{kind:"bar",bind:{slot:"x"},max:1,fill:[0,1,0,1],background:[0,0,0,1],width:0} })"#,
        r#"({ anchor:"center", offset:[0,0], root:{kind:"bar",bind:{slot:"x"},max:1,fill:[0,1,0,1],background:[0,0,0,1],exitFade:{durationMs:500}} })"#,
        r#"({ anchor:"center", offset:[0,0], root:{kind:"bar",bind:{slot:"x"},max:1,fill:[0,1,0,1],background:[0,0,0,1],visibleWhen:{slot:"active"},exitFade:{durationMs:0}} })"#,
    ] {
        let error = eval_js(invalid_js, |ctx, value| {
            anchored_tree_from_js_value(ctx, value).unwrap_err()
        });
        assert!(matches!(error, DescriptorError::InvalidShape { .. }));
    }

    let invalid_lua = r#"return {
        anchor = "center", offset = {0, 0},
        root = { kind = "bar", bind = { slot = "x" }, max = 1,
          fill = {0,1,0,1}, background = {0,0,0,1}, exitFade = { durationMs = 500 } }
    }"#;
    let error = eval_lua(invalid_lua, |value| {
        anchored_tree_from_lua_value(value).unwrap_err()
    });
    assert!(matches!(error, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_bridge_rejects_a_bind_with_neither_slot_nor_local() {
    // A bind object must carry exactly one source key; neither is a shape error.
    let src = r#"({
        anchor: "center", offset: [0.0, 0.0],
        root: { kind: "text", content: "x", fontSize: 12.0, color: [1.0,1.0,1.0,1.0], bind: {} }
    })"#;
    let err = eval_js(src, |ctx, v| {
        anchored_tree_from_js_value(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_health_non_finite_zone_multiplier_is_rejected() {
    let src =
        r#"return { components = { health = { max = 50, zoneMultipliers = { head = 1/0 } } } }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}
