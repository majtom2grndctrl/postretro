// Tests: ai component parsing.

use super::super::*;
use super::common::*;

// --- ai component (both parsers) ----------------------------------------
//
// Parse-time acceptance: range fields finite+positive, attackDamage
// non-negative+finite, and an unknown `states` key all abort at parse —
// twinned across QuickJS and Luau (identical outcome). Tested on a bare
// descriptor value with NO entity materialized. The logical-state →
// animation-state name mapping is validated at SPAWN (cross-component), not
// here; see `components::brain` tests.

/// A well-formed `components.ai` block (JS source body) with placeholder
/// overrides applied via string substitution for the negative-case tests.
fn js_ai(extra: &str) -> String {
    format!(
        r#"({{ components: {{ ai: {{
            detectionRange: 18, attackRange: 2.2, leashRange: 26,
            attackDamage: 8, attackCooldownMs: 1200, moveSpeed: 3.5,
            deathDespawnMs: 1500,
            states: {{ idle: "idle", alert: "walk", attack: "attack", death: "die" }}{extra}
        }} }} }})"#
    )
}

/// Luau twin of [`js_ai`].
fn lua_ai(extra: &str) -> String {
    format!(
        r#"return {{ components = {{ ai = {{
            detectionRange = 18, attackRange = 2.2, leashRange = 26,
            attackDamage = 8, attackCooldownMs = 1200, moveSpeed = 3.5,
            deathDespawnMs = 1500,
            states = {{ idle = "idle", alert = "walk", attack = "attack", death = "die" }}{extra}
        }} }} }}"#
    )
}

#[test]
fn js_entity_descriptor_parses_ai_block() {
    let d = eval_js(&js_ai(""), |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap()
    });
    let ai = d.ai.expect("ai parsed");
    assert_eq!(ai.detection_range, 18.0);
    assert_eq!(ai.attack_range, 2.2);
    assert_eq!(ai.leash_range, 26.0);
    assert_eq!(ai.attack_damage, 8.0);
    assert_eq!(ai.attack_cooldown_ms, 1200.0);
    assert_eq!(ai.move_speed, 3.5);
    assert_eq!(ai.death_despawn_ms, 1500.0);
    assert_eq!(ai.states.idle, "idle");
    assert_eq!(ai.states.alert, "walk");
    assert_eq!(ai.states.attack, "attack");
    assert_eq!(ai.states.death, "die");
}

#[test]
fn lua_entity_descriptor_parses_ai_block() {
    // Luau parity: a missing arm would silently drop `ai`. Assert the arm
    // exists and the same shape parses identically.
    let d = eval_lua(&lua_ai(""), |v| entity_descriptor_from_lua(v).unwrap());
    let ai = d.ai.expect("ai parsed by the Luau arm");
    assert_eq!(ai.detection_range, 18.0);
    assert_eq!(ai.states.death, "die");
}

#[test]
fn js_ai_non_positive_range_is_rejected() {
    // A zero range field aborts (must be finite AND strictly positive).
    let src = r#"({ components: { ai: {
        detectionRange: 0, attackRange: 2.2, leashRange: 26,
        attackDamage: 8, attackCooldownMs: 1200, moveSpeed: 3.5,
        deathDespawnMs: 1500,
        states: { idle: "idle", alert: "walk", attack: "attack", death: "die" }
    } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_ai_non_positive_range_is_rejected() {
    // Twin of the JS non-positive-range case: identical abort outcome.
    let src = r#"return { components = { ai = {
        detectionRange = 0, attackRange = 2.2, leashRange = 26,
        attackDamage = 8, attackCooldownMs = 1200, moveSpeed = 3.5,
        deathDespawnMs = 1500,
        states = { idle = "idle", alert = "walk", attack = "attack", death = "die" }
    } } }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_ai_non_finite_range_is_rejected() {
    let src = r#"({ components: { ai: {
        detectionRange: 18, attackRange: 2.2, leashRange: 1/0,
        attackDamage: 8, attackCooldownMs: 1200, moveSpeed: 3.5,
        deathDespawnMs: 1500,
        states: { idle: "idle", alert: "walk", attack: "attack", death: "die" }
    } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_ai_non_finite_range_is_rejected() {
    let src = r#"return { components = { ai = {
        detectionRange = 18, attackRange = 2.2, leashRange = 1/0,
        attackDamage = 8, attackCooldownMs = 1200, moveSpeed = 3.5,
        deathDespawnMs = 1500,
        states = { idle = "idle", alert = "walk", attack = "attack", death = "die" }
    } } }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_ai_negative_attack_damage_is_rejected() {
    // A negative attackDamage would HEAL the player through `apply_damage`'s
    // subtraction; aborted at parse.
    let src = r#"({ components: { ai: {
        detectionRange: 18, attackRange: 2.2, leashRange: 26,
        attackDamage: -1, attackCooldownMs: 1200, moveSpeed: 3.5,
        deathDespawnMs: 1500,
        states: { idle: "idle", alert: "walk", attack: "attack", death: "die" }
    } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_ai_negative_attack_damage_is_rejected() {
    let src = r#"return { components = { ai = {
        detectionRange = 18, attackRange = 2.2, leashRange = 26,
        attackDamage = -1, attackCooldownMs = 1200, moveSpeed = 3.5,
        deathDespawnMs = 1500,
        states = { idle = "idle", alert = "walk", attack = "attack", death = "die" }
    } } }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_ai_unknown_states_key_is_rejected() {
    // The four logical-state keys are a closed set: an extra key
    // (`deny_unknown_fields`) is a parse error.
    let src = r#"({ components: { ai: {
        detectionRange: 18, attackRange: 2.2, leashRange: 26,
        attackDamage: 8, attackCooldownMs: 1200, moveSpeed: 3.5,
        deathDespawnMs: 1500,
        states: { idle: "idle", alert: "walk", attack: "attack", death: "die", flee: "run" }
    } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_ai_unknown_states_key_is_rejected() {
    let src = r#"return { components = { ai = {
        detectionRange = 18, attackRange = 2.2, leashRange = 26,
        attackDamage = 8, attackCooldownMs = 1200, moveSpeed = 3.5,
        deathDespawnMs = 1500,
        states = { idle = "idle", alert = "walk", attack = "attack", death = "die", flee = "run" }
    } } }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_ai_missing_states_key_is_rejected() {
    // Every logical-state key is required (no serde default): a missing
    // `death` aborts. Twins with the Luau case below.
    let src = r#"({ components: { ai: {
        detectionRange: 18, attackRange: 2.2, leashRange: 26,
        attackDamage: 8, attackCooldownMs: 1200, moveSpeed: 3.5,
        deathDespawnMs: 1500,
        states: { idle: "idle", alert: "walk", attack: "attack" }
    } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_ai_missing_states_key_is_rejected() {
    let src = r#"return { components = { ai = {
        detectionRange = 18, attackRange = 2.2, leashRange = 26,
        attackDamage = 8, attackCooldownMs = 1200, moveSpeed = 3.5,
        deathDespawnMs = 1500,
        states = { idle = "idle", alert = "walk", attack = "attack" }
    } } }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}
