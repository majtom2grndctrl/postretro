// Tests: M13 G2 descriptor-field bridge.

use super::super::*;
use super::common::*;

// --- M13 G2: bridge reads every new descriptor field (JS + Lua) ---------
//
// A serde-only field would round-trip through `descriptor.rs` yet SILENTLY
// DROP on the live authoring path, because the hand-written bridge — not
// serde — converts authored JS/Luau tables into descriptors. These tests
// author each new G2 field through the bridge and assert it ARRIVES on the
// typed descriptor, in BOTH runtimes.

#[test]
fn js_bridge_reads_all_g2_fields() {
    // A capture tree exercising: envelope accessibleName + role; a button with
    // selected/checked/bind predicates, styleRanges, disabled, labelledBy,
    // visibleWhen, role; a decorative image; and a polite/assertive announce.
    let src = r#"({
        anchor: "center", offset: [0.0, 0.0], captureMode: "capture",
        accessibleName: "Pause menu", role: "group",
        root: {
            kind: "vstack", gap: 0.0, padding: 0.0, align: "start",
            visibleWhen: { slot: "hud.menuOpen", equals: true },
            role: "tablist",
            children: [
                { kind: "button", id: "tab1", labelledBy: "tab1Label", onPress: "openStats",
                  selected: { slot: "hud.tab", equals: "stats" },
                  checked: { local: "on" },
                  bind: { slot: "hud.charge" },
                  styleRanges: { max: 100.0, entries: [ { color: "ok" } ] },
                  disabled: true, visibleWhen: { slot: "hud.show" }, role: "tab" },
                { kind: "slider", id: "vol", labelledBy: "volLabel", bind: { slot: "audio.master" },
                  min: 0.0, max: 1.0, step: 0.1, disabled: true },
                { kind: "image", asset: "ui/logo", decorative: true },
                { kind: "image", asset: "ui/portrait", label: "Hero" },
                { kind: "announce", text: "Saved" },
                { kind: "announce", text: "Alert", priority: "assertive" }
            ]
        }
    })"#;
    let tree = eval_js(src, |ctx, v| {
        anchored_tree_from_js_value(ctx, v).expect("g2 tree must convert")
    });

    assert_eq!(tree.accessible_name.as_deref(), Some("Pause menu"));
    assert_eq!(tree.role, Some(Role::Group));
    let Widget::VStack(root) = &tree.root else {
        panic!("root must be vstack");
    };
    assert_eq!(
        root.visible_when.as_ref().map(|p| &p.source),
        Some(&BindSource::Slot {
            slot: "hud.menuOpen".into()
        })
    );
    assert_eq!(
        root.visible_when.as_ref().and_then(|p| p.equals.clone()),
        Some(PredicateValue::Boolean(true))
    );
    assert_eq!(root.role, Some(Role::Tablist));

    let Widget::Button(b) = &root.children[0] else {
        panic!("child 0 must be button");
    };
    assert_eq!(b.label, None);
    assert_eq!(b.labelled_by.as_deref(), Some("tab1Label"));
    assert_eq!(
        b.selected.as_ref().and_then(|p| p.equals.clone()),
        Some(PredicateValue::String("stats".into()))
    );
    assert!(b.checked.is_some(), "checked predicate must arrive");
    assert!(b.bind.is_some(), "styleRanges bind predicate must arrive");
    assert!(b.style_ranges.is_some(), "styleRanges must arrive");
    assert!(b.disabled, "disabled must arrive");
    assert!(b.visible_when.is_some(), "visibleWhen must arrive");
    assert_eq!(b.role, Some(Role::Tab));

    let Widget::Slider(s) = &root.children[1] else {
        panic!("child 1 must be slider");
    };
    assert_eq!(s.labelled_by.as_deref(), Some("volLabel"));
    assert!(s.disabled, "slider disabled must arrive");

    let Widget::Image(deco) = &root.children[2] else {
        panic!("child 2 must be image");
    };
    assert!(deco.decorative && deco.label.is_none());
    let Widget::Image(named) = &root.children[3] else {
        panic!("child 3 must be image");
    };
    assert_eq!(named.label.as_deref(), Some("Hero"));
    assert!(!named.decorative);

    let Widget::Announce(polite) = &root.children[4] else {
        panic!("child 4 must be announce");
    };
    assert_eq!(polite.text, "Saved");
    assert_eq!(polite.priority, Priority::Polite);
    let Widget::Announce(assertive) = &root.children[5] else {
        panic!("child 5 must be announce");
    };
    assert_eq!(assertive.priority, Priority::Assertive);
}

#[test]
fn lua_bridge_reads_all_g2_fields() {
    // The Luau twin of `js_bridge_reads_all_g2_fields`: identical field set,
    // identical arrival assertions. A field read on one runtime but not the
    // other would diverge the behavioral-twin contract.
    let src = r#"return {
        anchor = "center", offset = {0.0, 0.0}, captureMode = "capture",
        accessibleName = "Pause menu", role = "group",
        root = {
            kind = "vstack", gap = 0.0, padding = 0.0, align = "start",
            visibleWhen = { slot = "hud.menuOpen", equals = true },
            role = "tablist",
            children = {
                { kind = "button", id = "tab1", labelledBy = "tab1Label", onPress = "openStats",
                  selected = { slot = "hud.tab", equals = "stats" },
                  checked = { ["local"] = "on" },
                  bind = { slot = "hud.charge" },
                  styleRanges = { max = 100.0, entries = { { color = "ok" } } },
                  disabled = true, visibleWhen = { slot = "hud.show" }, role = "tab" },
                { kind = "slider", id = "vol", labelledBy = "volLabel", bind = { slot = "audio.master" },
                  min = 0.0, max = 1.0, step = 0.1, disabled = true },
                { kind = "image", asset = "ui/logo", decorative = true },
                { kind = "image", asset = "ui/portrait", label = "Hero" },
                { kind = "announce", text = "Saved" },
                { kind = "announce", text = "Alert", priority = "assertive" }
            }
        }
    }"#;
    let tree = eval_lua(src, |v| {
        anchored_tree_from_lua_value(v).expect("g2 tree must convert")
    });

    assert_eq!(tree.accessible_name.as_deref(), Some("Pause menu"));
    assert_eq!(tree.role, Some(Role::Group));
    let Widget::VStack(root) = &tree.root else {
        panic!("root must be vstack");
    };
    assert_eq!(
        root.visible_when.as_ref().and_then(|p| p.equals.clone()),
        Some(PredicateValue::Boolean(true))
    );
    assert_eq!(root.role, Some(Role::Tablist));

    let Widget::Button(b) = &root.children[0] else {
        panic!("child 0 must be button");
    };
    assert_eq!(b.label, None);
    assert_eq!(b.labelled_by.as_deref(), Some("tab1Label"));
    assert_eq!(
        b.selected.as_ref().and_then(|p| p.equals.clone()),
        Some(PredicateValue::String("stats".into()))
    );
    assert!(b.checked.is_some());
    assert!(b.bind.is_some());
    assert!(b.style_ranges.is_some());
    assert!(b.disabled);
    assert!(b.visible_when.is_some());
    assert_eq!(b.role, Some(Role::Tab));

    let Widget::Slider(s) = &root.children[1] else {
        panic!("child 1 must be slider");
    };
    assert_eq!(s.labelled_by.as_deref(), Some("volLabel"));
    assert!(s.disabled);

    let Widget::Image(deco) = &root.children[2] else {
        panic!("child 2 must be image");
    };
    assert!(deco.decorative && deco.label.is_none());
    let Widget::Image(named) = &root.children[3] else {
        panic!("child 3 must be image");
    };
    assert_eq!(named.label.as_deref(), Some("Hero"));

    let Widget::Announce(polite) = &root.children[4] else {
        panic!("child 4 must be announce");
    };
    assert_eq!(polite.priority, Priority::Polite);
    let Widget::Announce(assertive) = &root.children[5] else {
        panic!("child 5 must be announce");
    };
    assert_eq!(assertive.priority, Priority::Assertive);
}

#[test]
fn js_bridge_enforces_g2_preconditions() {
    // Each precondition surfaces as a named load-time error (no panic).
    let cases: &[&str] = &[
        // Button with neither name.
        r#"({ anchor:"center", offset:[0.0,0.0], root:{ kind:"button", id:"a", onPress:"go" } })"#,
        // Button with both names.
        r#"({ anchor:"center", offset:[0.0,0.0], root:{ kind:"button", id:"a", label:"A", labelledBy:"x", onPress:"go" } })"#,
        // Image with neither name nor decorative.
        r#"({ anchor:"center", offset:[0.0,0.0], root:{ kind:"image", asset:"x" } })"#,
        // Image with both label and decorative.
        r#"({ anchor:"center", offset:[0.0,0.0], root:{ kind:"image", asset:"x", label:"A", decorative:true } })"#,
        // Announce missing text.
        r#"({ anchor:"center", offset:[0.0,0.0], root:{ kind:"announce", priority:"polite" } })"#,
        // Announce with an unknown priority.
        r#"({ anchor:"center", offset:[0.0,0.0], root:{ kind:"announce", text:"x", priority:"shouty" } })"#,
        // Predicate `equals` with an rgba/array comparand.
        r#"({ anchor:"center", offset:[0.0,0.0], root:{ kind:"text", content:"x", fontSize:12.0, color:[1.0,1.0,1.0,1.0], visibleWhen:{ slot:"a.b", equals:[1.0,0.0,0.0,1.0] } } })"#,
        // Unknown role.
        r#"({ anchor:"center", offset:[0.0,0.0], root:{ kind:"button", id:"a", label:"A", onPress:"go", role:"wizard" } })"#,
    ];
    for src in cases {
        let err = eval_js(src, |ctx, v| {
            anchored_tree_from_js_value(ctx, v).unwrap_err()
        });
        assert!(
            matches!(
                err,
                DescriptorError::InvalidShape { .. } | DescriptorError::MissingField { .. }
            ),
            "precondition must be a named load-time error (no panic), got {err:?} for {src}"
        );
    }
}

#[test]
fn js_bar_max_rejects_non_finite_literal_and_empty_state_ref() {
    let cases = [
        r#"({
            anchor:"center", offset:[0.0,0.0],
            root:{ kind:"bar", bind:{ slot:"player.health" }, max: 1/0 }
        })"#,
        r#"({
            anchor:"center", offset:[0.0,0.0],
            root:{ kind:"bar", bind:{ slot:"player.health" }, max: { slot:"" } }
        })"#,
    ];

    for src in cases {
        let err = eval_js(src, |ctx, v| {
            anchored_tree_from_js_value(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }
}

#[test]
fn lua_bar_max_rejects_non_finite_literal_and_empty_state_ref() {
    let cases = [
        r#"return {
            anchor = "center", offset = {0.0, 0.0},
            root = { kind = "bar", bind = { slot = "player.health" }, max = 1/0 }
        }"#,
        r#"return {
            anchor = "center", offset = {0.0, 0.0},
            root = { kind = "bar", bind = { slot = "player.health" }, max = { slot = "" } }
        }"#,
    ];

    for src in cases {
        let err = eval_lua(src, |v| anchored_tree_from_lua_value(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }
}

#[test]
fn lua_bridge_enforces_g2_preconditions() {
    // The Luau twin: identical preconditions surface the same named error.
    let cases: &[&str] = &[
        r#"return { anchor="center", offset={0.0,0.0}, root={ kind="button", id="a", onPress="go" } }"#,
        r#"return { anchor="center", offset={0.0,0.0}, root={ kind="button", id="a", label="A", labelledBy="x", onPress="go" } }"#,
        r#"return { anchor="center", offset={0.0,0.0}, root={ kind="image", asset="x" } }"#,
        r#"return { anchor="center", offset={0.0,0.0}, root={ kind="image", asset="x", label="A", decorative=true } }"#,
        r#"return { anchor="center", offset={0.0,0.0}, root={ kind="announce", priority="polite" } }"#,
        r#"return { anchor="center", offset={0.0,0.0}, root={ kind="announce", text="x", priority="shouty" } }"#,
        r#"return { anchor="center", offset={0.0,0.0}, root={ kind="text", content="x", fontSize=12.0, color={1.0,1.0,1.0,1.0}, visibleWhen={ slot="a.b", equals={1.0,0.0,0.0,1.0} } } }"#,
        r#"return { anchor="center", offset={0.0,0.0}, root={ kind="button", id="a", label="A", onPress="go", role="wizard" } }"#,
    ];
    for src in cases {
        let err = eval_lua(src, |v| anchored_tree_from_lua_value(v).unwrap_err());
        assert!(
            matches!(
                err,
                DescriptorError::InvalidShape { .. } | DescriptorError::MissingField { .. }
            ),
            "precondition must be a named load-time error (no panic), got {err:?} for {src}"
        );
    }
}
