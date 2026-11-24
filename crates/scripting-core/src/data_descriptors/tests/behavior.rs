// Tests: recursive `components.behavior` parsing through both production VMs.

use super::super::*;
use super::common::*;
use postretro_foundation::data_descriptors::{
    BehaviorLayerDescriptor, BehaviorSelectorEntry, MAX_BEHAVIOR_NESTING_DEPTH, MotionVerb,
};

const JS_GUARD: &str = r#"{ op: "input", name: "@brain.hasTarget" }"#;
const LUA_GUARD: &str = r#"{ op = "input", name = "@brain.hasTarget" }"#;

// Attack handles as `defineAttack` builds them; these raw VMs carry no SDK.
// `kick` is declared but never listed, for the unlisted-reference rejection.
const JS_ATTACK_HANDLES: &str = r#"const jab = { kind: "attack", name: "jab", damage: 4, maxRange: 2, cooldownMs: 500 };
const slam = { kind: "attack", name: "slam", damage: 9, maxRange: 3, cooldownMs: 1500 };
const kick = { kind: "attack", name: "kick", damage: 1, maxRange: 1, cooldownMs: 1 };"#;
const LUA_ATTACK_HANDLES: &str = r#"local jab = { kind = "attack", name = "jab", damage = 4, maxRange = 2, cooldownMs = 500 }
local slam = { kind = "attack", name = "slam", damage = 9, maxRange = 3, cooldownMs = 1500 }
local kick = { kind = "attack", name = "kick", damage = 1, maxRange = 1, cooldownMs = 1 }"#;

fn js_behavior(root_extra: &str) -> String {
    format!(
        r#"{JS_ATTACK_HANDLES}
        ({{ components: {{ behavior: {{
            initial: "idle",
            moveSpeed: 3,
            activities: {{
                idle: {{ animation: "idle", motion: "hold" }},
                engage: {{ animation: "walk", layers: {{
                    move: [{{ when: {JS_GUARD}, motion: "hold" }}, "chaseTarget"],
                    presentation: [{{ when: {JS_GUARD} }}, "hold"],
                    offense: {{
                        initial: "windup",
                        activities: {{
                            windup: {{ animation: "windup" }},
                            commit: {{ animation: "slam", action: {{ attack: slam }} }}
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
        r#"{LUA_ATTACK_HANDLES}
        return {{ components = {{ behavior = {{
            initial = "idle",
            moveSpeed = 3,
            activities = {{
                idle = {{ animation = "idle", motion = "hold" }},
                engage = {{ animation = "walk", layers = {{
                    move = {{ {{ when = {LUA_GUARD}, motion = "hold" }}, "chaseTarget" }},
                    presentation = {{ {{ when = {LUA_GUARD} }}, "hold" }},
                    offense = {{
                        initial = "windup",
                        activities = {{
                            windup = {{ animation = "windup" }},
                            commit = {{ animation = "slam", action = {{ attack = slam }} }}
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
    assert!(matches!(
        engage.layers["presentation"],
        BehaviorLayerDescriptor::Selector(_)
    ));
}

#[test]
fn retaliation_tuning_parses_identically_in_both_runtimes() {
    let js = eval_js(
        &js_behavior(", retaliation: { windowMs: 750, damageWeight: 2, recencyWeight: 0.25 }"),
        |ctx, value| entity_descriptor_from_js(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        &lua_behavior(", retaliation = { windowMs = 750, damageWeight = 2, recencyWeight = 0.25 }"),
        |value| entity_descriptor_from_lua(value).unwrap(),
    );
    let (js, lua) = (js.behavior.unwrap(), lua.behavior.unwrap());

    assert_eq!(js, lua);
    assert_eq!(js.retaliation().window_ms, 750.0);
    assert_eq!(js.retaliation().damage_weight, 2.0);
    assert_eq!(js.retaliation().recency_weight, 0.25);
}

// AC 5 (P31): a non-finite or negative `fillMs` or `drainMs` rejects at load
// in both runtimes, naming the field; 0 loads.
#[test]
fn awareness_rejects_non_finite_or_negative_in_both_runtimes() {
    for (field, js_value, lua_value) in [
        ("fillMs", "-1", "-1"),
        ("drainMs", "-0.5", "-0.5"),
        ("fillMs", "Infinity", "math.huge"),
        ("drainMs", "-Infinity", "-math.huge"),
        ("fillMs", "NaN", "0/0"),
    ] {
        let js = js_error(&js_behavior(&format!(
            ", awareness: {{ {field}: {js_value} }}"
        )));
        let lua = lua_error(&lua_behavior(&format!(
            ", awareness = {{ {field} = {lua_value} }}"
        )));
        for error in [&js, &lua] {
            assert!(
                error.contains(&format!("awareness.{field}")) || error.contains(field),
                "{field} = {js_value}: {error}"
            );
        }
    }
    let js = eval_js(
        &js_behavior(", awareness: { fillMs: 0, drainMs: 0 }"),
        |ctx, value| entity_descriptor_from_js(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        &lua_behavior(", awareness = { fillMs = 0, drainMs = 0 }"),
        |value| entity_descriptor_from_lua(value).unwrap(),
    );
    let (js, lua) = (js.behavior.unwrap(), lua.behavior.unwrap());
    assert_eq!(js, lua);
    assert_eq!(js.awareness().fill_ms, 0.0);
    assert_eq!(js.awareness().drain_ms, 0.0);
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
fn both_runtimes_reject_actions_on_move_to_last_known() {
    let js = js_error(&js_behavior(
        ", initial: \"investigate\", activities: { investigate: { animation: \"walk\", motion: \"moveToLastKnown\", action: { attack: slam } } }, transitions: {}",
    ));
    let lua = lua_error(&lua_behavior(
        ", initial = \"investigate\", activities = { investigate = { animation = \"walk\", motion = \"moveToLastKnown\", action = { attack = slam } } }, transitions = {}",
    ));
    for error in [&js, &lua] {
        assert!(
            error.contains("components.behavior.activities.investigate.action"),
            "{error}"
        );
        assert!(error.contains("position-goal"), "{error}");
        assert!(error.contains("non-engaged"), "{error}");
    }
}

// Regression: independently valid move and offense selectors could combine a
// position goal with an attack inside one composite activity.
#[test]
fn both_runtimes_reject_composite_position_goals_with_offense_actions() {
    for (motion, js_patrol, lua_patrol) in [
        ("moveToAnchor", "", ""),
        ("moveToLastKnown", "", ""),
        (
            "patrol",
            ", patrol: { points: [[0, 0]], mode: \"loop\" }",
            ", patrol = { points = { { 0, 0 } }, mode = \"loop\" }",
        ),
    ] {
        let js = js_error(&js_behavior(&format!(
            r#", initial: "engage", activities: {{ engage: {{ animation: "walk", layers: {{ move: ["{motion}"], offense: [{{ action: {{ attack: slam }} }}] }} }} }}, transitions: {{}}{js_patrol}"#,
        )));
        let lua = lua_error(&lua_behavior(&format!(
            r#", initial = "engage", activities = {{ engage = {{ animation = "walk", layers = {{ move = {{ "{motion}" }}, offense = {{ {{ action = {{ attack = slam }} }} }} }} }} }}, transitions = {{}}{lua_patrol}"#,
        )));
        for error in [&js, &lua] {
            assert!(
                error.contains("components.behavior.activities.engage.layers.move[0]"),
                "{motion}: {error}"
            );
            assert!(
                error.contains("components.behavior.activities.engage.layers.offense[0].action"),
                "{motion}: {error}"
            );
            assert!(error.contains("position-goal"), "{motion}: {error}");
            assert!(error.contains("non-engaged"), "{motion}: {error}");
        }
    }
}

// Regression: a parent offense selector could combine with a position goal
// supplied by its nested graph descendant.
#[test]
fn both_runtimes_reject_nested_graph_position_goals_with_parent_actions() {
    let js = js_error(&js_behavior(
        r#", initial: "engage", activities: { engage: { animation: "walk", layers: { offense: [{ action: { attack: slam } }], phase: { initial: "investigate", activities: { investigate: { animation: "walk", motion: "moveToLastKnown" } }, transitions: {} } } } }, transitions: {}"#,
    ));
    let lua = lua_error(&lua_behavior(
        r#", initial = "engage", activities = { engage = { animation = "walk", layers = { offense = { { action = { attack = slam } } }, phase = { initial = "investigate", activities = { investigate = { animation = "walk", motion = "moveToLastKnown" } }, transitions = {} } } } }, transitions = {}"#,
    ));
    for error in [&js, &lua] {
        assert!(
            error.contains(
                "components.behavior.activities.engage.layers.phase.activities.investigate.motion"
            ),
            "{error}"
        );
        assert!(
            error.contains("components.behavior.activities.engage.layers.offense[0].action"),
            "{error}"
        );
        assert!(error.contains("position-goal"), "{error}");
        assert!(error.contains("non-engaged"), "{error}");
    }
}

// Regression: nested graph activities are mutually exclusive, so an
// investigate position goal and an attack sibling must not be merged into one
// impossible active path.
#[test]
fn both_runtimes_allow_mutually_exclusive_nested_position_goal_and_action() {
    let js = js_behavior(
        r#", initial: "engage", activities: { engage: { animation: "walk", layers: { phase: { initial: "investigate", activities: { investigate: { animation: "walk", motion: "moveToLastKnown" }, attack: { animation: "slam", action: { attack: slam } } }, transitions: { investigate: [{ when: { op: "input", name: "@brain.hasTarget" }, to: "attack" }] } } } } }, transitions: {}"#,
    );
    let lua = lua_behavior(
        r#", initial = "engage", activities = { engage = { animation = "walk", layers = { phase = { initial = "investigate", activities = { investigate = { animation = "walk", motion = "moveToLastKnown" }, attack = { animation = "slam", action = { attack = slam } } }, transitions = { investigate = { { when = { op = "input", name = "@brain.hasTarget" }, to = "attack" } } } } } } }, transitions = {}"#,
    );

    assert!(
        eval_js(&js, entity_descriptor_from_js).is_ok(),
        "QuickJS must retain mutually exclusive nested activities"
    );
    assert!(
        eval_lua(&lua, entity_descriptor_from_lua).is_ok(),
        "Luau must retain mutually exclusive nested activities"
    );
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
fn both_runtimes_reject_unknown_brain_guards_with_authored_paths() {
    let js = js_error(&js_behavior(
        ", transitions: { idle: [{ when: { op: \"input\", name: \"@brain.notAnInput\" }, to: \"engage\" }] }",
    ));
    let lua = lua_error(&lua_behavior(
        ", transitions = { idle = { { when = { op = \"input\", name = \"@brain.notAnInput\" }, to = \"engage\" } } }",
    ));
    for error in [&js, &lua] {
        assert!(
            error.contains("components.behavior.transitions.idle[0].when"),
            "{error}"
        );
        assert!(error.contains("@brain.notAnInput"), "{error}");
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
    assert!(eval_js(&js_at_cap, entity_descriptor_from_js).is_ok());
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
    assert!(eval_js(&js_behavior(""), entity_descriptor_from_js).is_ok());
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

const REFERENCE_ENEMY_TS_SRC: &str =
    include_str!("../../../../../content/dev/scripts/reference-enemy.ts");
const REFERENCE_ENEMY_LUAU_SRC: &str =
    include_str!("../../../../../content/dev/scripts/reference-enemy.luau");
const REFERENCE_ENTITIES_TS_SRC: &str =
    include_str!("../../../../../sdk/behaviors/reference/entities.ts");
const REFERENCE_ENTITIES_LUAU_SRC: &str =
    include_str!("../../../../../sdk/behaviors/reference/entities.luau");

pub(super) fn shipped_reference_descriptor_from_typescript(
    source: &str,
    export_name: &str,
    module_name: &str,
) -> EntityTypeDescriptor {
    static NEXT_FIXTURE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let fixture_id = NEXT_FIXTURE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "postretro-reference-enemy-{}-{fixture_id}",
        std::process::id(),
    ));
    std::fs::create_dir_all(&directory).expect("create reference-enemy fixture directory");
    // The dev enemy imports the vocabulary modules beside it.
    for module in [
        "defenses.ts",
        "damage-types.ts",
        "attack-tokens.ts",
        "enemy-rifle.ts",
        "arm-cannon.ts",
    ] {
        std::fs::write(
            directory.join(module),
            std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../content/dev/scripts")
                    .join(module),
            )
            .expect("read the dev vocabulary module"),
        )
        .expect("write the dev vocabulary module");
    }
    let entry = directory.join("entities.ts");
    std::fs::write(
        &entry,
        format!("{source}\nglobalThis.__referenceEntity = {export_name};"),
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
        let _: JsValue = crate::quickjs::run_script(&ctx, &bundled, module_name)
            .expect("the shipped reference entities module evaluates");
        let value: JsValue =
            crate::quickjs::run_script(&ctx, "globalThis.__referenceEntity", "read")
                .expect("the module exported the requested reference descriptor");
        entity_descriptor_from_js(&ctx, value).expect("the shipped TS reference descriptor parses")
    })
}

pub(super) fn shipped_reference_descriptor_from_luau(
    source: &str,
    export_name: &str,
    module_name: &str,
) -> EntityTypeDescriptor {
    // Rooted at the dev mod: a relative `require` resolves against the mod
    // root, so the dev enemy requires `./scripts/defenses.luau`.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
    let lua = crate::luau::build_lua_state(&[], None, Some(&root)).expect("mod-rooted luau state");
    let source = format!("local M = (function()\n{source}\nend)()\nreturn M.{export_name}");
    let value: LuaValue = lua
        .load(&source)
        .set_name(module_name)
        .eval()
        .expect("the shipped reference entities module evaluates");
    entity_descriptor_from_lua(value).expect("the shipped Luau reference descriptor parses")
}

fn shipped_reference_enemy_from_typescript() -> EntityTypeDescriptor {
    shipped_reference_descriptor_from_typescript(
        REFERENCE_ENEMY_TS_SRC,
        "referenceEnemyEntity",
        "content/dev/scripts/reference-enemy.ts",
    )
}

fn shipped_reference_enemy_from_luau() -> EntityTypeDescriptor {
    shipped_reference_descriptor_from_luau(
        REFERENCE_ENEMY_LUAU_SRC,
        "referenceEnemyEntity",
        "content/dev/scripts/reference-enemy.luau",
    )
}

#[test]
fn the_shipped_reference_enemy_descriptor_is_identical_in_both_authorings() {
    assert_eq!(
        shipped_reference_enemy_from_typescript(),
        shipped_reference_enemy_from_luau(),
        "the two authored shipped reference enemy descriptors must remain identical"
    );
}

const STANCE_SURFACE_TS_SRC: &str =
    include_str!("../../../../../content/dev/scripts/stance-surface.ts");
const STANCE_SURFACE_LUAU_SRC: &str =
    include_str!("../../../../../content/dev/scripts/stance-surface.luau");

// S1: `defineStance`, `defineAttack` and the stances brief's Scripting surface
// example, shipped as `content/dev` content, lower to identical runtime
// descriptors from TypeScript and Luau, its stances expanded in the offense
// level and its struck attacks registered.
#[test]
fn the_stance_surface_example_lowers_identically_in_both_authorings() {
    let typescript = shipped_reference_descriptor_from_typescript(
        STANCE_SURFACE_TS_SRC,
        "stanceSurfaceEntity",
        "content/dev/scripts/stance-surface.ts",
    );
    let luau = shipped_reference_descriptor_from_luau(
        STANCE_SURFACE_LUAU_SRC,
        "stanceSurfaceEntity",
        "content/dev/scripts/stance-surface.luau",
    );
    assert_eq!(typescript, luau, "the two authorings lower alike");

    let behavior = typescript.behavior.expect("the example carries a behavior");
    assert_eq!(
        behavior.attacks.keys().collect::<Vec<_>>(),
        ["charge", "lunge", "slam", "sweep"],
        "every attack its rows strike"
    );
    let BehaviorLayerDescriptor::Graph(offense) =
        &behavior.envelope.activities["engage"].layers["offense"]
    else {
        panic!("the offense layer is a graph");
    };
    assert_eq!(
        offense.activities.keys().collect::<Vec<_>>(),
        [
            "approach",
            "bellow",
            "charge_bellow",
            "charge_recover",
            "charge_release",
            "coil",
            "flinch",
            "lunge_coil",
            "lunge_recover",
            "lunge_release",
            "overhead",
            "slam_overhead",
            "slam_recover",
            "slam_release",
            "sweep_overhead",
            "sweep_recover",
            "sweep_release",
        ]
    );
    let rest_rows: Vec<(&str, Option<&str>)> = offense.transitions["overhead"]
        .iter()
        .map(|row| (row.to.as_str(), row.queue.as_deref()))
        .collect();
    assert_eq!(
        rest_rows,
        [
            ("flinch", None),
            ("slam_overhead", Some("slam")),
            ("sweep_overhead", Some("sweep")),
            ("approach", None),
        ],
        "the break row first, then the authored rows, strikes queued"
    );
    assert_eq!(offense.activities["charge_release"].speed, Some(2.0));
    assert_eq!(
        offense.activities["charge_release"].motion,
        Some(MotionVerb::ChaseTarget)
    );
}

#[test]
fn shipped_pose_fixture_is_a_recursive_graph_with_valid_mesh_animation_states() {
    let typescript = shipped_reference_descriptor_from_typescript(
        REFERENCE_ENTITIES_TS_SRC,
        "poseFixtureEnemyEntity",
        "sdk/behaviors/reference/entities.ts",
    );
    let luau = shipped_reference_descriptor_from_luau(
        REFERENCE_ENTITIES_LUAU_SRC,
        "poseFixtureEnemyEntity",
        "sdk/behaviors/reference/entities.luau",
    );
    assert_eq!(
        typescript.behavior, luau.behavior,
        "the pose graphs stay in lockstep"
    );
    assert_eq!(
        typescript.mesh, luau.mesh,
        "the pose meshes stay in lockstep"
    );
}

// Doc1: `sdk/behaviors/reference` carries typed attacks and a shared stance
// that load in both authorings, through the one lowering, to identical
// descriptors: the exhaustive `attacks` list keeps both strikes, and each
// strike gets its own hold in the shared pose.
#[test]
fn shipped_stance_reference_enemy_lowers_identically_in_both_authorings() {
    let typescript = shipped_reference_descriptor_from_typescript(
        REFERENCE_ENTITIES_TS_SRC,
        "stanceReferenceEnemyEntity",
        "sdk/behaviors/reference/entities.ts",
    );
    let luau = shipped_reference_descriptor_from_luau(
        REFERENCE_ENTITIES_LUAU_SRC,
        "stanceReferenceEnemyEntity",
        "sdk/behaviors/reference/entities.luau",
    );
    assert_eq!(typescript, luau, "the two authorings lower alike");

    let behavior = typescript
        .behavior
        .expect("the reference carries a behavior");
    assert_eq!(
        behavior.attacks.keys().collect::<Vec<_>>(),
        ["chop", "thrust"],
        "the listed attacks, by name"
    );
    let activities = &behavior.envelope.activities;
    assert_eq!(
        activities.keys().collect::<Vec<_>>(),
        [
            "approach",
            "chop_guard",
            "chop_recover",
            "chop_release",
            "guard",
            "idle",
            "stagger",
            "thrust_guard",
            "thrust_recover",
            "thrust_release",
        ]
    );
    for (hold, attack) in [("chop_guard", "chop"), ("thrust_guard", "thrust")] {
        assert_eq!(activities[hold].pose.as_deref(), Some("guard"), "{hold}");
        assert_eq!(
            activities[hold].animation.as_deref(),
            Some("guard"),
            "{hold}"
        );
        assert_eq!(
            activities[hold].attack_turn.as_deref(),
            Some(attack),
            "{hold}"
        );
    }
    assert_eq!(activities["guard"].pose.as_deref(), Some("guard"));
    assert_eq!(
        activities["guard"].attack_turn, None,
        "a rest holds no turn"
    );
    let rest_rows: Vec<(&str, Option<&str>)> = behavior.envelope.transitions["guard"]
        .iter()
        .map(|row| (row.to.as_str(), row.queue.as_deref()))
        .collect();
    assert_eq!(
        rest_rows,
        [
            ("stagger", None),
            ("chop_guard", Some("chop")),
            ("thrust_guard", Some("thrust")),
            ("approach", None),
        ],
        "the break row first, then the authored rows, strikes queued"
    );
}

// Engaged motion surface (E10): `strafe`, `reposition`, `facing`, and
// `attackTurn` marks, through both production VMs.

fn js_activities(activities: &str) -> String {
    js_behavior(&format!(
        ", initial: \"engage\", attacks: [jab, slam], activities: {{ {activities} }}, transitions: {{}}"
    ))
}

fn lua_activities(activities: &str) -> String {
    lua_behavior(&format!(
        ", initial = \"engage\", attacks = {{ jab, slam }}, activities = {{ {activities} }}, transitions = {{}}"
    ))
}

#[test]
fn engaged_motion_surface_parses_identically_in_both_runtimes() {
    use postretro_foundation::data_descriptors::FacingPolicy;

    let js = eval_js(
        &js_activities(&format!(
            r#"engage: {{ animation: "walk", layers: {{ offense: {{
                initial: "wait",
                activities: {{
                    wait: {{ animation: "walk", motion: "strafe" }},
                    burst: {{ animation: "idle_aiming", motion: "hold", action: {{ attack: slam }}, attackTurn: slam }},
                    move: {{ animation: "run", motion: "reposition", facing: "travel", reposition: {{ arc: [60, 120] }} }},
                    far: {{ animation: "run", motion: "reposition", facing: "target", reposition: {{ minTravel: 8 }} }}
                }},
                transitions: {{
                    wait: [{{ when: {JS_GUARD}, to: "burst" }}],
                    burst: [{{ when: {JS_GUARD}, to: "move" }}],
                    move: [{{ when: {JS_GUARD}, to: "far" }}]
                }}
            }} }} }}"#
        )),
        |ctx, value| entity_descriptor_from_js(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        &lua_activities(&format!(
            r#"engage = {{ animation = "walk", layers = {{ offense = {{
                initial = "wait",
                activities = {{
                    wait = {{ animation = "walk", motion = "strafe" }},
                    burst = {{ animation = "idle_aiming", motion = "hold", action = {{ attack = slam }}, attackTurn = slam }},
                    move = {{ animation = "run", motion = "reposition", facing = "travel", reposition = {{ arc = {{ 60, 120 }} }} }},
                    far = {{ animation = "run", motion = "reposition", facing = "target", reposition = {{ minTravel = 8 }} }}
                }},
                transitions = {{
                    wait = {{ {{ when = {LUA_GUARD}, to = "burst" }} }},
                    burst = {{ {{ when = {LUA_GUARD}, to = "move" }} }},
                    move = {{ {{ when = {LUA_GUARD}, to = "far" }} }}
                }}
            }} }} }}"#
        )),
        |value| entity_descriptor_from_lua(value).unwrap(),
    );
    let (js, lua) = (js.behavior.unwrap(), lua.behavior.unwrap());
    assert_eq!(js, lua);

    let BehaviorLayerDescriptor::Graph(offense) =
        &js.envelope.activities["engage"].layers["offense"]
    else {
        panic!("offense is a nested graph");
    };
    let activity = |name: &str| &offense.activities[name];
    assert_eq!(activity("wait").motion, Some(MotionVerb::Strafe));
    assert_eq!(activity("wait").facing, None);
    assert_eq!(activity("burst").attack_turn.as_deref(), Some("slam"));
    let moving = activity("move");
    assert_eq!(moving.motion, Some(MotionVerb::Reposition));
    assert_eq!(moving.facing, Some(FacingPolicy::Travel));
    let params = moving.reposition.expect("move authors reposition bounds");
    assert_eq!(params.arc_deg, [60.0, 120.0]);
    assert_eq!(
        params.min_travel_m, None,
        "omitted minTravel resolves at runtime"
    );
    let far = activity("far");
    assert_eq!(far.facing, Some(FacingPolicy::Target));
    let params = far.reposition.expect("far authors reposition bounds");
    assert_eq!(
        params.arc_deg,
        [0.0, 180.0],
        "omitted arc defaults to [0, 180]"
    );
    assert_eq!(params.min_travel_m, Some(8.0));
}

#[test]
fn both_runtimes_reject_invalid_reposition_params_with_paths() {
    const ENGAGE: &str = "components.behavior.activities.engage";
    for (js_activity, lua_activity, path, needle) in [
        (
            r#"engage: { animation: "run", motion: "reposition", reposition: { arc: [-10, 90] } }"#,
            r#"engage = { animation = "run", motion = "reposition", reposition = { arc = { -10, 90 } } }"#,
            format!("{ENGAGE}.reposition.arc"),
            "within 0 to 180",
        ),
        (
            r#"engage: { animation: "run", motion: "reposition", reposition: { arc: [0, 190] } }"#,
            r#"engage = { animation = "run", motion = "reposition", reposition = { arc = { 0, 190 } } }"#,
            format!("{ENGAGE}.reposition.arc"),
            "within 0 to 180",
        ),
        (
            r#"engage: { animation: "run", motion: "reposition", reposition: { arc: [120, 60] } }"#,
            r#"engage = { animation = "run", motion = "reposition", reposition = { arc = { 120, 60 } } }"#,
            format!("{ENGAGE}.reposition.arc"),
            "must not exceed",
        ),
        (
            r#"engage: { animation: "run", motion: "reposition", reposition: { arc: [50, 80] } }"#,
            r#"engage = { animation = "run", motion = "reposition", reposition = { arc = { 50, 80 } } }"#,
            format!("{ENGAGE}.reposition.arc"),
            "no multiple of 45",
        ),
        (
            r#"engage: { animation: "run", motion: "reposition", reposition: { arc: [0, Infinity] } }"#,
            r#"engage = { animation = "run", motion = "reposition", reposition = { arc = { 0, math.huge } } }"#,
            "activities.engage.reposition.arc[1]".to_string(),
            "non-finite",
        ),
        (
            r#"engage: { animation: "run", motion: "reposition", reposition: { minTravel: -1 } }"#,
            r#"engage = { animation = "run", motion = "reposition", reposition = { minTravel = -1 } }"#,
            format!("{ENGAGE}.reposition.minTravel"),
            ">= 0.0",
        ),
        (
            r#"engage: { animation: "run", motion: "reposition", reposition: { minTravel: NaN } }"#,
            r#"engage = { animation = "run", motion = "reposition", reposition = { minTravel = 0 / 0 } }"#,
            "activities.engage.reposition.minTravel".to_string(),
            "non-finite",
        ),
        (
            r#"engage: { animation: "walk", motion: "strafe", reposition: {} }"#,
            r#"engage = { animation = "walk", motion = "strafe", reposition = {} }"#,
            format!("{ENGAGE}.reposition"),
            "\"reposition\"",
        ),
        (
            r#"engage: { reposition: { arc: [0, 90] }, layers: { move: ["reposition"] } }"#,
            r#"engage = { reposition = { arc = { 0, 90 } }, layers = { move = { "reposition" } } }"#,
            format!("{ENGAGE}.reposition"),
            "\"reposition\"",
        ),
    ] {
        let js = js_error(&js_activities(js_activity));
        let lua = lua_error(&lua_activities(lua_activity));
        for error in [&js, &lua] {
            assert!(error.contains(&path), "{js_activity}: {error}");
            assert!(error.contains(needle), "{js_activity}: {error}");
        }
    }
}

// Per-activity `speed` (E10 stances): it crosses both production VMs on a
// composite and a leaf, and a zero, negative or non-finite value rejects load
// with the activity's path (AC S26).
#[test]
fn both_runtimes_carry_activity_speed_and_reject_invalid_values_with_paths() {
    let js = eval_js(
        &js_activities(
            r#"engage: { speed: 1.5, layers: { offense: {
                initial: "charge",
                activities: { charge: { animation: "run", motion: "chaseTarget", speed: 2 } },
                transitions: {}
            } } }"#,
        ),
        |ctx, value| entity_descriptor_from_js(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        &lua_activities(
            r#"engage = { speed = 1.5, layers = { offense = {
                initial = "charge",
                activities = { charge = { animation = "run", motion = "chaseTarget", speed = 2 } },
                transitions = {}
            } } }"#,
        ),
        |value| entity_descriptor_from_lua(value).unwrap(),
    );
    let (js, lua) = (js.behavior.unwrap(), lua.behavior.unwrap());
    assert_eq!(js, lua);
    let engage = &js.envelope.activities["engage"];
    assert_eq!(engage.speed, Some(1.5));
    let BehaviorLayerDescriptor::Graph(offense) = &engage.layers["offense"] else {
        panic!("offense is a nested graph");
    };
    assert_eq!(offense.activities["charge"].speed, Some(2.0));

    const ENGAGE: &str = "components.behavior.activities.engage";
    for (js_activity, lua_activity, path, needle) in [
        (
            r#"engage: { animation: "run", motion: "chaseTarget", speed: 0 }"#,
            r#"engage = { animation = "run", motion = "chaseTarget", speed = 0 }"#,
            format!("{ENGAGE}.speed"),
            "> 0.0",
        ),
        (
            r#"engage: { animation: "run", motion: "chaseTarget", speed: -2 }"#,
            r#"engage = { animation = "run", motion = "chaseTarget", speed = -2 }"#,
            format!("{ENGAGE}.speed"),
            "> 0.0",
        ),
        (
            r#"engage: { animation: "run", motion: "chaseTarget", speed: Infinity }"#,
            r#"engage = { animation = "run", motion = "chaseTarget", speed = math.huge }"#,
            "activities.engage.speed".to_string(),
            "non-finite",
        ),
        (
            r#"engage: { animation: "run", motion: "chaseTarget", speed: NaN }"#,
            r#"engage = { animation = "run", motion = "chaseTarget", speed = 0 / 0 }"#,
            "activities.engage.speed".to_string(),
            "non-finite",
        ),
    ] {
        let js = js_error(&js_activities(js_activity));
        let lua = lua_error(&lua_activities(lua_activity));
        for error in [&js, &lua] {
            assert!(error.contains(&path), "{js_activity}: {error}");
            assert!(error.contains(needle), "{js_activity}: {error}");
        }
    }
}

#[test]
fn both_runtimes_reject_disagreeing_attack_turn_marks_with_paths() {
    const ENGAGE: &str = "components.behavior.activities.engage";
    let guard_js = JS_GUARD;
    let guard_lua = LUA_GUARD;
    for (js_activity, lua_activity, path) in [
        (
            r#"engage: { animation: "idle", attackTurn: kick }"#.to_string(),
            r#"engage = { animation = "idle", attackTurn = kick }"#.to_string(),
            format!("{ENGAGE}.attackTurn"),
        ),
        (
            r#"engage: { attackTurn: slam, layers: { offense: { initial: "jab", activities: { jab: { animation: "jab", attackTurn: jab } }, transitions: {} } } }"#.to_string(),
            r#"engage = { attackTurn = slam, layers = { offense = { initial = "jab", activities = { jab = { animation = "jab", attackTurn = jab } }, transitions = {} } } }"#.to_string(),
            format!("{ENGAGE}.layers.offense.activities.jab.attackTurn"),
        ),
        (
            r#"engage: { animation: "jab", attackTurn: slam, action: { attack: jab } }"#.to_string(),
            r#"engage = { animation = "jab", attackTurn = slam, action = { attack = jab } }"#.to_string(),
            format!("{ENGAGE}.action.attack"),
        ),
        (
            format!(r#"engage: {{ attackTurn: slam, layers: {{ offense: [{{ when: {guard_js}, action: {{ attack: jab }} }}] }} }}"#),
            format!(r#"engage = {{ attackTurn = slam, layers = {{ offense = {{ {{ when = {guard_lua}, action = {{ attack = jab }} }} }} }} }}"#),
            format!("{ENGAGE}.layers.offense[0].action.attack"),
        ),
    ] {
        let js = js_error(&js_activities(&js_activity));
        let lua = lua_error(&lua_activities(&lua_activity));
        assert_eq!(js, lua, "both runtimes reject {js_activity} identically");
        assert!(js.contains(&path), "{js_activity}: {js}");
    }
}
