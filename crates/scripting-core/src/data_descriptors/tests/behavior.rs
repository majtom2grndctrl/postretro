// Tests: recursive `components.behavior` parsing through both production VMs.

use super::super::*;
use super::common::*;
use postretro_foundation::data_descriptors::{
    BehaviorLayerDescriptor, BehaviorSelectorEntry, MAX_BEHAVIOR_NESTING_DEPTH, MotionVerb,
};

const JS_GUARD: &str = r#"{ op: "input", name: "@brain.hasTarget" }"#;
const LUA_GUARD: &str = r#"{ op = "input", name = "@brain.hasTarget" }"#;

fn js_behavior(root_extra: &str) -> String {
    format!(
        r#"({{ components: {{ behavior: {{
            initial: "idle",
            moveSpeed: 3,
            attacks: {{ slam: {{ damage: 8, maxRange: 2, cooldownMs: 1200 }} }},
            activities: {{
                idle: {{ animation: "idle", motion: "hold" }},
                engage: {{ animation: "walk", layers: {{
                    move: [{{ when: {JS_GUARD}, motion: "hold" }}, "chaseTarget"],
                    offense: {{
                        initial: "windup",
                        activities: {{
                            windup: {{ animation: "windup" }},
                            commit: {{ animation: "slam", action: {{ attack: "slam" }} }}
                        }},
                        transitions: {{ windup: [{{ when: {JS_GUARD}, to: "commit" }}], "*": [] }}
                    }}
                }} }}
            }},
            transitions: {{ idle: [{{ when: {JS_GUARD}, to: "engage" }}], "*": [] }}
            {root_extra}
        }} }} }})"#
    )
}

fn lua_behavior(root_extra: &str) -> String {
    format!(
        r#"return {{ components = {{ behavior = {{
            initial = "idle",
            moveSpeed = 3,
            attacks = {{ slam = {{ damage = 8, maxRange = 2, cooldownMs = 1200 }} }},
            activities = {{
                idle = {{ animation = "idle", motion = "hold" }},
                engage = {{ animation = "walk", layers = {{
                    move = {{ {{ when = {LUA_GUARD}, motion = "hold" }}, "chaseTarget" }},
                    offense = {{
                        initial = "windup",
                        activities = {{
                            windup = {{ animation = "windup" }},
                            commit = {{ animation = "slam", action = {{ attack = "slam" }} }}
                        }},
                        transitions = {{ windup = {{ {{ when = {LUA_GUARD}, to = "commit" }} }}, ["*"] = {{}} }}
                    }}
                }} }}
            }},
            transitions = {{ idle = {{ {{ when = {LUA_GUARD}, to = "engage" }} }}, ["*"] = {{}} }}
            {root_extra}
        }} }} }}"#
    )
}

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
fn recursive_descriptor_parses_identically_in_both_runtimes() {
    let js = eval_js(&js_behavior(""), |ctx, value| {
        entity_descriptor_from_js(ctx, value).unwrap()
    });
    let lua = eval_lua(&lua_behavior(""), |value| {
        entity_descriptor_from_lua(value).unwrap()
    });
    let (js, lua) = (js.behavior.unwrap(), lua.behavior.unwrap());
    assert_eq!(js, lua);
    assert_eq!(js.envelope.initial, "idle");
    assert_eq!(js.envelope.activities.len(), 2);
    assert_eq!(js.move_speed, 3.0);
    assert_eq!(js.attacks.len(), 1);
    let engage = &js.envelope.activities["engage"];
    assert!(matches!(
        engage.layers["offense"],
        BehaviorLayerDescriptor::Graph(_)
    ));
    let BehaviorLayerDescriptor::Selector(entries) = &engage.layers["move"] else {
        panic!("move is a selector");
    };
    assert!(matches!(
        entries.last(),
        Some(BehaviorSelectorEntry::Motion(MotionVerb::ChaseTarget))
    ));
}

#[test]
fn both_runtimes_reject_flat_states_and_interrupts() {
    for (js_extra, lua_extra) in [
        (", states: {}", ", states = {}"),
        (", interrupts: []", ", interrupts = {}"),
    ] {
        let js = js_error(&js_behavior(js_extra));
        let lua = lua_error(&lua_behavior(lua_extra));
        for error in [&js, &lua] {
            assert!(error.contains("components.behavior"), "{error}");
            assert!(error.contains("unknown field"), "{error}");
        }
    }
}

#[test]
fn both_runtimes_reject_inline_activity_transitions() {
    let js = js_error(&js_behavior(
        ", activities: { idle: { animation: \"idle\", transitions: [] } }",
    ));
    let lua = lua_error(&lua_behavior(
        ", activities = { idle = { animation = \"idle\", transitions = {} } }",
    ));
    for error in [&js, &lua] {
        assert!(error.contains("components.behavior"), "{error}");
        assert!(error.contains("unknown field `transitions`"), "{error}");
    }
}

#[test]
fn both_runtimes_reject_cross_level_and_unknown_targets_with_paths() {
    let js = js_error(&js_behavior(
        ", transitions: { idle: [{ when: { op: \"const\", value: true }, to: \"windup\" }] }",
    ));
    let lua = lua_error(&lua_behavior(
        ", transitions = { idle = { { when = { op = \"const\", value = true }, to = \"windup\" } } }",
    ));
    for error in [&js, &lua] {
        assert!(
            error.contains("components.behavior.transitions.idle[0].to"),
            "{error}"
        );
        assert!(error.contains("windup"), "{error}");
    }
}

#[test]
fn both_runtimes_reject_scope_all_self_targets_with_paths() {
    let js = js_error(&js_behavior(
        ", transitions: { \"*\": [{ when: { op: \"const\", value: true }, to: \"*\" }] }",
    ));
    let lua = lua_error(&lua_behavior(
        ", transitions = { [\"*\"] = { { when = { op = \"const\", value = true }, to = \"*\" } } }",
    ));
    for error in [&js, &lua] {
        assert!(
            error.contains("components.behavior.transitions.*[0].to"),
            "{error}"
        );
        assert!(error.contains("scope-all key"), "{error}");
    }
}

#[test]
fn both_runtimes_reject_an_empty_activity_map_and_missing_move_fallback() {
    let js_empty = js_error(
        r#"({ components: { behavior: {
        initial: "idle", moveSpeed: 3, activities: {}, transitions: {}
    } } })"#,
    );
    let lua_empty = lua_error(
        r#"return { components = { behavior = {
        initial = "idle", moveSpeed = 3, activities = {}, transitions = {}
    } } }"#,
    );
    for error in [&js_empty, &lua_empty] {
        assert!(error.contains("components.behavior.activities"), "{error}");
        assert!(error.contains("at least one activity"), "{error}");
    }

    let js_fallback = js_error(&js_behavior(
        ", activities: { idle: { animation: \"idle\", motion: \"hold\" }, engage: { animation: \"walk\", layers: { move: [{ when: { op: \"const\", value: true }, motion: \"hold\" }] } } }",
    ));
    let lua_fallback = lua_error(&lua_behavior(
        ", activities = { idle = { animation = \"idle\", motion = \"hold\" }, engage = { animation = \"walk\", layers = { move = { { when = { op = \"const\", value = true }, motion = \"hold\" } } } } }",
    ));
    for error in [&js_fallback, &lua_fallback] {
        assert!(
            error.contains("components.behavior.activities.engage.layers.move"),
            "{error}"
        );
        assert!(error.contains("fallback"), "{error}");
    }
}

#[test]
fn both_runtimes_enforce_the_shared_nesting_cap() {
    fn js_envelope(depth: usize) -> String {
        if depth == 1 {
            r#"{ initial: "leaf", activities: { leaf: { animation: "leaf" } }, transitions: {} }"#
                .to_string()
        } else {
            format!(
                r#"{{ initial: "node", activities: {{ node: {{ layers: {{ offense: {} }} }} }}, transitions: {{}} }}"#,
                js_envelope(depth - 1)
            )
        }
    }
    fn lua_envelope(depth: usize) -> String {
        if depth == 1 {
            r#"{ initial = "leaf", activities = { leaf = { animation = "leaf" } }, transitions = {} }"#.to_string()
        } else {
            format!(
                r#"{{ initial = "node", activities = {{ node = {{ layers = {{ offense = {} }} }} }}, transitions = {{}} }}"#,
                lua_envelope(depth - 1)
            )
        }
    }
    let js_at_cap = format!(
        "({{ components: {{ behavior: {} }} }})",
        js_envelope(MAX_BEHAVIOR_NESTING_DEPTH).replacen('{', "{ moveSpeed: 3,", 1)
    );
    let lua_at_cap = format!(
        "return {{ components = {{ behavior = {} }} }}",
        lua_envelope(MAX_BEHAVIOR_NESTING_DEPTH).replacen('{', "{ moveSpeed = 3,", 1)
    );
    assert!(
        eval_js(&js_at_cap, |ctx, value| entity_descriptor_from_js(
            ctx, value
        ))
        .is_ok()
    );
    assert!(eval_lua(&lua_at_cap, entity_descriptor_from_lua).is_ok());

    let js_too_deep = format!(
        "({{ components: {{ behavior: {} }} }})",
        js_envelope(MAX_BEHAVIOR_NESTING_DEPTH + 1).replacen('{', "{ moveSpeed: 3,", 1)
    );
    let lua_too_deep = format!(
        "return {{ components = {{ behavior = {} }} }}",
        lua_envelope(MAX_BEHAVIOR_NESTING_DEPTH + 1).replacen('{', "{ moveSpeed = 3,", 1)
    );
    for error in [js_error(&js_too_deep), lua_error(&lua_too_deep)] {
        assert!(error.contains("MAX_BEHAVIOR_NESTING_DEPTH"), "{error}");
    }
}

#[test]
fn both_runtimes_allow_selectors_but_reject_two_stateful_layers_with_paths() {
    // `js_behavior` / `lua_behavior` are the positive fixture: one nested
    // offense graph plus a move selector must remain legal.
    assert!(
        eval_js(&js_behavior(""), |ctx, value| entity_descriptor_from_js(
            ctx, value
        ))
        .is_ok()
    );
    assert!(eval_lua(&lua_behavior(""), entity_descriptor_from_lua).is_ok());

    let js = js_error(
        r#"({ components: { behavior: {
            initial: "engage", moveSpeed: 3,
            activities: { engage: { animation: "walk", layers: {
                move: ["hold"],
                offense: { initial: "windup", activities: { windup: { animation: "windup" } }, transitions: {} },
                stance: { initial: "ready", activities: { ready: { animation: "ready" } }, transitions: {} }
            } } },
            transitions: {}
        } } })"#,
    );
    let lua = lua_error(
        r#"return { components = { behavior = {
            initial = "engage", moveSpeed = 3,
            activities = { engage = { animation = "walk", layers = {
                move = { "hold" },
                offense = { initial = "windup", activities = { windup = { animation = "windup" } }, transitions = {} },
                stance = { initial = "ready", activities = { ready = { animation = "ready" } }, transitions = {} }
            } } },
            transitions = {}
        } } }"#,
    );
    for error in [&js, &lua] {
        assert!(
            error.contains("components.behavior.activities.engage.layers"),
            "{error}"
        );
        assert!(error.contains("at most one nested-graph"), "{error}");
    }
}

// Task 5 re-authors these still-flat shipped fixtures. Keep their production
// parse paths alive, but park the oracles until then rather than teaching the
// recursive parser to accept the retired shape.
const REFERENCE_ENTITIES_TS_SRC: &str =
    include_str!("../../../../../sdk/behaviors/reference/entities.ts");
const REFERENCE_ENTITIES_LUAU_SRC: &str =
    include_str!("../../../../../sdk/behaviors/reference/entities.luau");

fn shipped_reference_descriptor_from_typescript(export_name: &str) -> EntityTypeDescriptor {
    static NEXT_FIXTURE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let fixture_id = NEXT_FIXTURE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "postretro-reference-enemy-{}-{fixture_id}",
        std::process::id(),
    ));
    std::fs::create_dir_all(&directory).expect("create reference-enemy fixture directory");
    let entry = directory.join("entities.ts");
    std::fs::write(
        &entry,
        format!("{REFERENCE_ENTITIES_TS_SRC}\nglobalThis.__referenceEntity = {export_name};"),
    )
    .expect("write reference-enemy fixture");
    let entry = std::fs::canonicalize(&entry).expect("canonicalize reference-enemy fixture");
    let bundled = postretro_script_compiler::bundle_entry(&entry)
        .expect("the shipped reference entities module bundles through scripts-build");
    let _ = std::fs::remove_dir_all(&directory);

    let registry = crate::primitives_registry::PrimitiveRegistry::new();
    let subsystem =
        crate::quickjs::QuickJsSubsystem::new(&registry, &crate::quickjs::QuickJsConfig::default())
            .expect("quickjs definition context");
    subsystem.definition_ctx().with(|ctx| {
        let _: JsValue = crate::quickjs::run_script(&ctx, &bundled, "entities.ts")
            .expect("the shipped reference entities module evaluates");
        let value: JsValue =
            crate::quickjs::run_script(&ctx, "globalThis.__referenceEntity", "read")
                .expect("the module exported the requested reference descriptor");
        entity_descriptor_from_js(&ctx, value).expect("the shipped TS reference descriptor parses")
    })
}

fn shipped_reference_descriptor_from_luau(export_name: &str) -> EntityTypeDescriptor {
    let lua = crate::luau::build_lua_state(
        &[],
        None,
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))),
    )
    .expect("mod-rooted luau state");
    let source = format!(
        "local M = (function()\n{REFERENCE_ENTITIES_LUAU_SRC}\nend)()\nreturn M.{export_name}"
    );
    let value: LuaValue = lua
        .load(&source)
        .set_name("sdk/behaviors/reference/entities.luau")
        .eval()
        .expect("the shipped reference entities module evaluates");
    entity_descriptor_from_lua(value).expect("the shipped Luau reference descriptor parses")
}

fn shipped_reference_enemy_from_typescript() -> EntityTypeDescriptor {
    shipped_reference_descriptor_from_typescript("referenceEnemyEntity")
}

fn shipped_reference_enemy_from_luau() -> EntityTypeDescriptor {
    shipped_reference_descriptor_from_luau("referenceEnemyEntity")
}

#[test]
#[ignore = "pending statecharts re-author, Task 5"]
fn the_shipped_reference_enemy_descriptor_is_identical_in_both_authorings() {
    assert_eq!(
        shipped_reference_enemy_from_typescript(),
        shipped_reference_enemy_from_luau(),
        "the two authored shipped reference enemy descriptors must remain identical"
    );
}

#[test]
#[ignore = "pending statecharts re-author, Task 5"]
fn shipped_pose_fixture_is_a_direct_graph_with_valid_mesh_animation_states() {
    let typescript = shipped_reference_descriptor_from_typescript("poseFixtureEnemyEntity");
    let luau = shipped_reference_descriptor_from_luau("poseFixtureEnemyEntity");
    assert_eq!(
        typescript.behavior, luau.behavior,
        "the pose graphs stay in lockstep"
    );
    assert_eq!(
        typescript.mesh, luau.mesh,
        "the pose meshes stay in lockstep"
    );
}
