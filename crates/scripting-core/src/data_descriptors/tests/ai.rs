// Tests: retired AI component migration boundary.

use super::super::*;
use super::common::*;

fn js_error(source: &str) -> String {
    eval_js(source, |ctx, value| {
        entity_descriptor_from_js(ctx, value).unwrap_err()
    })
    .to_string()
}

fn lua_error(source: &str) -> String {
    eval_lua(source, |value| {
        entity_descriptor_from_lua(value).unwrap_err()
    })
    .to_string()
}

#[test]
fn js_own_legacy_ai_key_rejects_every_representable_value() {
    for value in ["null", "undefined", "false", "{}"] {
        let error = js_error(&format!("({{ components: {{ ai: {value} }} }})"));
        assert!(error.contains("components.ai"), "unexpected error: {error}");
        assert!(
            error.contains("components.behavior"),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn js_inherited_legacy_ai_key_is_not_authored_descriptor_content() {
    let descriptor = eval_js(
        "({ components: Object.create({ ai: { detectionRange: 16 } }) })",
        |ctx, value| entity_descriptor_from_js(ctx, value).unwrap(),
    );
    assert!(descriptor.behavior.is_none());
}

#[test]
fn luau_non_nil_legacy_ai_key_rejects() {
    for value in ["false", "0", "\"legacy\"", "{}"] {
        let error = lua_error(&format!("return {{ components = {{ ai = {value} }} }}"));
        assert!(error.contains("components.ai"), "unexpected error: {error}");
        assert!(
            error.contains("components.behavior"),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn luau_nil_legacy_ai_key_is_ordinary_absence() {
    let descriptor = eval_lua("return { components = { ai = nil } }", |value| {
        entity_descriptor_from_lua(value).unwrap()
    });
    assert!(descriptor.behavior.is_none());
}
