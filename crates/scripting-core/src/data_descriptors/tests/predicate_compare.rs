// Tests: numeric `compare` predicates through both hand-written bridges.

use super::super::*;
use super::common::*;

/// A text root carrying `visibleWhen = <predicate>`, in each runtime's syntax.
fn js_tree(predicate: &str) -> String {
    format!(
        r#"({{ anchor:"center", offset:[0.0,0.0], root:{{ kind:"vstack", gap:0.0, padding:0.0, align:"start", children:[ {{ kind:"text", content:"x", fontSize:12.0, color:[1.0,1.0,1.0,1.0], visibleWhen:{predicate} }} ] }} }})"#
    )
}

fn lua_tree(predicate: &str) -> String {
    format!(
        r#"return {{ anchor="center", offset={{0.0,0.0}}, root={{ kind="vstack", gap=0.0, padding=0.0, align="start", children={{ {{ kind="text", content="x", fontSize=12.0, color={{1.0,1.0,1.0,1.0}}, visibleWhen={predicate} }} }} }} }}"#
    )
}

fn js_result(predicate: &str) -> Result<AnchoredTree, DescriptorError> {
    eval_js(&js_tree(predicate), |ctx, v| {
        anchored_tree_from_js_value(ctx, v)
    })
}

fn lua_result(predicate: &str) -> Result<AnchoredTree, DescriptorError> {
    eval_lua(&lua_tree(predicate), anchored_tree_from_lua_value)
}

fn reason(result: Result<AnchoredTree, DescriptorError>) -> String {
    match result {
        Err(DescriptorError::InvalidShape { reason }) => reason,
        other => panic!("expected a named InvalidShape load error, got {other:?}"),
    }
}

#[test]
fn both_bridges_read_compare_onto_the_same_descriptor_and_wire() {
    let from_js =
        js_result(r#"{ slot:"player.assistProgress", compare:{ op:"greaterThan", value:0 } }"#)
            .expect("JS compare predicate parses");
    let from_lua =
        lua_result(r#"{ slot="player.assistProgress", compare={ op="greaterThan", value=0 } }"#)
            .expect("Luau compare predicate parses");
    assert_eq!(from_js, from_lua, "the runtimes are behavioral twins");
    let Widget::VStack(root) = &from_js.root else {
        panic!("root is a vstack");
    };
    let Widget::Text(text) = &root.children[0] else {
        panic!("child is text");
    };
    let predicate = text.visible_when.as_ref().expect("visibleWhen arrives");
    assert_eq!(
        predicate.compare,
        Some(PredicateCompare {
            op: CompareOp::GreaterThan,
            value: 0.0,
        })
    );
    assert_eq!(predicate.equals, None);
    let wire = serde_json::to_value(predicate).unwrap();
    assert_eq!(
        wire,
        serde_json::json!({"slot":"player.assistProgress","compare":{"op":"greaterThan","value":0.0}})
    );
    let back: Predicate = serde_json::from_value(wire).unwrap();
    assert_eq!(&back, predicate, "the wire round-trips");
}

#[test]
fn both_bridges_reject_bad_compares_with_identical_key_path_errors() {
    let path = "UI tree `root`.children[0].visibleWhen";
    let cases: &[(&str, &str, String)] = &[
        (
            r#"{ slot:"player.lifeState", compare:{ op:"greaterThan", value:0 } }"#,
            r#"{ slot="player.lifeState", compare={ op="greaterThan", value=0 } }"#,
            format!(
                "{path}.compare: slot `player.lifeState` is a String slot; a numeric comparison needs a Number slot"
            ),
        ),
        (
            r#"{ slot:"player.reloadActive", compare:{ op:"atLeast", value:1 } }"#,
            r#"{ slot="player.reloadActive", compare={ op="atLeast", value=1 } }"#,
            format!(
                "{path}.compare: slot `player.reloadActive` is a Boolean slot; a numeric comparison needs a Number slot"
            ),
        ),
        (
            r#"{ slot:"player.assistProgress", compare:{ op:"greaterThan", value:1/0 } }"#,
            r#"{ slot="player.assistProgress", compare={ op="greaterThan", value=1/0 } }"#,
            format!("{path}.compare.value must be a finite number"),
        ),
        (
            r#"{ slot:"player.assistProgress", compare:{ op:"greaterThan", value:0/0 } }"#,
            r#"{ slot="player.assistProgress", compare={ op="greaterThan", value=0/0 } }"#,
            format!("{path}.compare.value must be a finite number"),
        ),
        (
            r#"{ slot:"player.assistProgress", equals:0, compare:{ op:"greaterThan", value:0 } }"#,
            r#"{ slot="player.assistProgress", equals=0, compare={ op="greaterThan", value=0 } }"#,
            format!("{path}: `equals` and `compare` are mutually exclusive"),
        ),
        (
            r#"{ slot:"player.assistProgress", compare:{ op:"gt", value:0 } }"#,
            r#"{ slot="player.assistProgress", compare={ op="gt", value=0 } }"#,
            "`visibleWhen.compare.op` must be one of `greaterThan`, `lessThan`, `atLeast`, `atMost`"
                .to_string(),
        ),
        (
            r#"{ slot:"player.assistProgress", compare:{ op:"greaterThan", value:"0" } }"#,
            r#"{ slot="player.assistProgress", compare={ op="greaterThan", value="0" } }"#,
            "`visibleWhen.compare.value` must be a number".to_string(),
        ),
        (
            r#"{ slot:"player.assistProgress", compare:0 }"#,
            r#"{ slot="player.assistProgress", compare=0 }"#,
            "`visibleWhen.compare` must be an `{ op, value }` object".to_string(),
        ),
    ];
    for (js, lua, expected) in cases {
        let js_reason = reason(js_result(js));
        let lua_reason = reason(lua_result(lua));
        assert_eq!(&js_reason, expected, "JS: {js}");
        assert_eq!(js_reason, lua_reason, "runtimes diverge for {js}");
    }
}

#[test]
fn value_text_conditions_accept_compare_and_validate_it() {
    let js = r#"({ anchor:"center", offset:[0.0,0.0], root:{ kind:"button", id:"a", label:"A", onPress:"go", valueText:[ { when:[ { slot:"player.lifeState", compare:{ op:"atMost", value:0 } } ], text:"X" } ] } })"#;
    let lua = r#"return { anchor="center", offset={0.0,0.0}, root={ kind="button", id="a", label="A", onPress="go", valueText={ { when={ { slot="player.lifeState", compare={ op="atMost", value=0 } } }, text="X" } } } }"#;
    let expected = "UI tree `root`.valueText[0].when[0].compare: slot `player.lifeState` is a String slot; a numeric comparison needs a Number slot";
    assert_eq!(reason(eval_js(js, anchored_tree_from_js_value)), expected);
    assert_eq!(
        reason(eval_lua(lua, anchored_tree_from_lua_value)),
        expected
    );
}
