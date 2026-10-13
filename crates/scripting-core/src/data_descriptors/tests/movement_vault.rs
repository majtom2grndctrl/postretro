// Tests: `movement.vault` parsing and validation in both runtimes.

use super::super::*;
use super::common::*;
use postretro_foundation::{VaultEntry, VaultParams, VaultTrigger};
use postretro_test_log_capture::LogCapture;

/// A player descriptor whose `movement.vault` value is `vault_body`, in JS or
/// Luau syntax. `stepHeight` is 0.3.
fn js_movement_with_vault(vault_body: &str) -> String {
    format!(
        r#"({{
            canonicalName: "player",
            components: {{
                movement: {{
                    capsule: {{ radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 }},
                    ground: {{ speed: {{ walk: 7.0, run: 11.0, crouch: 3.0 }}, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 }},
                    air: {{ forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 }},
                    fall: {{ terminalVelocity: 40.0 }},
                    vault: {vault_body}
                }}
            }}
        }})"#
    )
}

fn lua_movement_with_vault(vault_body: &str) -> String {
    format!(
        r#"return {{
            canonicalName = "player",
            components = {{
                movement = {{
                    capsule = {{ radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 }},
                    ground = {{ speed = {{ walk = 7.0, run = 11.0, crouch = 3.0 }}, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 }},
                    air = {{ forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 }},
                    fall = {{ terminalVelocity = 40.0 }},
                    vault = {vault_body}
                }}
            }}
        }}"#
    )
}

/// The required fields, one `(key, value)` per field, JS syntax.
const VAULT: [(&str, &str); 3] = [
    ("maxHeight", "1.6"),
    ("swimMaxHeight", "0.9"),
    ("durationMs", "320"),
];

/// A vault body in JS syntax: the required fields with `overrides` applied,
/// then `extra` fields verbatim.
fn js_vault(overrides: &[(&str, &str)], extra: &str) -> String {
    let mut fields: Vec<String> = VAULT
        .iter()
        .filter(|(key, _)| !overrides.iter().any(|(k, v)| k == key && *v == "<absent>"))
        .map(|(key, value)| {
            let value = overrides
                .iter()
                .find(|(k, _)| k == key)
                .map_or(*value, |(_, v)| *v);
            format!("{key}: {value}")
        })
        .collect();
    if !extra.is_empty() {
        fields.push(extra.to_string());
    }
    format!("{{ {} }}", fields.join(", "))
}

/// The same body in Luau syntax.
fn lua_vault(js_body: &str) -> String {
    js_body.replace(": ", " = ").replace("null", "nil")
}

fn parse_both(js_body: &str) -> [(Option<VaultParams>, Vec<String>); 2] {
    let lua_body = lua_vault(js_body);
    let js = {
        let capture = LogCapture::start();
        let descriptor = eval_js(&js_movement_with_vault(js_body), |ctx, v| {
            entity_descriptor_from_js(ctx, v).expect("the descriptor loads")
        });
        (
            descriptor.movement.expect("movement present").vault,
            vault_warnings(&capture),
        )
    };
    let lua = {
        let capture = LogCapture::start();
        let descriptor = eval_lua(&lua_movement_with_vault(&lua_body), |v| {
            entity_descriptor_from_lua(v).expect("the descriptor loads")
        });
        (
            descriptor.movement.expect("movement present").vault,
            vault_warnings(&capture),
        )
    };
    [js, lua]
}

fn vault_warnings(capture: &LogCapture) -> Vec<String> {
    capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level == log::Level::Warn && record.message.contains("movement.vault")
        })
        .map(|record| record.message)
        .collect()
}

/// Assert both runtimes dropped the block with one identical warning naming
/// `needle`, while the rest of the descriptor loaded.
fn assert_dropped_naming(js_body: &str, needle: &str) {
    let [(js, js_warnings), (lua, lua_warnings)] = parse_both(js_body);
    assert_eq!(js, None, "QuickJS drops the block ({needle})");
    assert_eq!(lua, None, "Luau drops the block ({needle})");
    assert_eq!(js_warnings.len(), 1, "one QuickJS warning: {js_warnings:?}");
    assert_eq!(
        js_warnings, lua_warnings,
        "the runtimes' diagnostics diverged"
    );
    let warning = &js_warnings[0];
    assert!(warning.contains(needle), "names `{needle}`: {warning}");
    assert!(
        warning.contains("the movement.vault block was dropped, so the player cannot vault"),
        "{warning}"
    );
}

#[test]
fn a_vault_block_parses_identically_with_defaults_in_both_runtimes() {
    let [(js, js_warnings), (lua, lua_warnings)] = parse_both(&js_vault(&[], ""));
    let expected = VaultParams {
        max_height: 1.6,
        swim_max_height: 0.9,
        duration_ms: 320.0,
        from: VaultEntry {
            air: Some(VaultTrigger::Auto),
            ground: None,
            swim: Some(VaultTrigger::Auto),
        },
        allow_fire: false,
    };
    assert_eq!(js, Some(expected.clone()));
    assert_eq!(lua, Some(expected));
    assert!(js_warnings.is_empty() && lua_warnings.is_empty());
}

#[test]
fn a_present_from_replaces_the_default_whole_and_allow_fire_reads() {
    let body = js_vault(
        &[],
        r#"from: { ground: "jump", air: null }, allowFire: true"#,
    );
    let [(js, _), (lua, _)] = parse_both(&body);
    let vault = js.expect("the block loads");
    assert_eq!(
        vault.from,
        VaultEntry {
            air: None,
            ground: Some(VaultTrigger::Jump),
            swim: None,
        }
    );
    assert!(vault.allow_fire);
    assert_eq!(lua, Some(vault));
}

#[test]
fn a_movement_without_a_vault_block_never_vaults() {
    let js = eval_js(JS_PLAYER_MOVEMENT, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap()
    });
    assert_eq!(js.movement.unwrap().vault, None);
}

#[test]
fn each_invalid_vault_field_drops_the_block_naming_it() {
    for (body, needle) in [
        (
            js_vault(&[("maxHeight", "<absent>")], ""),
            "`movement.vault.maxHeight` is required",
        ),
        (
            js_vault(&[("swimMaxHeight", "\"high\"")], ""),
            "`movement.vault.swimMaxHeight` must be a number",
        ),
        (
            js_vault(&[("durationMs", "0")], ""),
            "movement.vault.durationMs",
        ),
        (
            js_vault(&[("maxHeight", "0.25")], ""),
            "`movement.vault.maxHeight` must exceed `movement.ground.stepHeight`",
        ),
        (
            js_vault(&[("swimMaxHeight", "-1")], ""),
            "movement.vault.swimMaxHeight",
        ),
        (
            js_vault(&[], "allowFire: 1"),
            "`movement.vault.allowFire` must be a boolean",
        ),
        (
            js_vault(&[], "from: 4"),
            "`movement.vault.from` must be an object",
        ),
        (
            js_vault(&[], r#"from: { air: "climb" }"#),
            r#"`movement.vault.from.air` must be "auto" or "jump""#,
        ),
        (
            js_vault(&[], r#"from: { wall: "auto" }"#),
            "`movement.vault.from.wall` is not a vault context",
        ),
        ("4".to_string(), "`movement.vault` must be an object"),
    ] {
        assert_dropped_naming(&body, needle);
    }
}
