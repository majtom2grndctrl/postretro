// Tests: behavior-graph component parsing and the SDK guard-input helpers.

use super::super::*;
use super::common::*;

// --- components.behavior (both parsers) ----------------------------------
//
// Both runtimes convert the authored block to JSON and funnel it through the
// same `BehaviorGraphDescriptor` serde + `validate` chokepoint, so acceptance
// and every rejection below is identical by construction. Tested on a bare
// descriptor value with NO entity materialized: the state → mesh
// animation-state mapping is cross-component and stays a SPAWN-time check, as
// it is for `components.ai`.

/// Guard source shared by the JS fixtures: `targetDistance <= 16`.
const JS_NEAR_GUARD: &str = r#"{ op: "le", a: { op: "input", name: "@brain.targetDistance" }, b: { op: "const", value: 16 } }"#;
/// Luau twin of [`JS_NEAR_GUARD`].
const LUA_NEAR_GUARD: &str = r#"{ op = "le", a = { op = "input", name = "@brain.targetDistance" }, b = { op = "const", value = 16 } }"#;

/// A well-formed `components.behavior` block (JS source body). `states_extra`
/// splices additional state entries; `guard` overrides the idle→chase guard.
fn js_behavior(guard: &str, states_extra: &str) -> String {
    format!(
        r#"({{ components: {{ behavior: {{
            initial: "idle",
            moveSpeed: 3,
            attack: {{ damage: 8, range: 2, cooldownMs: 1200 }},
            states: {{
                idle: {{ animation: "idle", motion: "hold",
                        transitions: [{{ to: "chase", when: {guard} }}] }},
                chase: {{ animation: "walk", motion: "chaseTarget" }}{states_extra}
            }}
        }} }} }})"#
    )
}

/// Luau twin of [`js_behavior`].
fn lua_behavior(guard: &str, states_extra: &str) -> String {
    format!(
        r#"return {{ components = {{ behavior = {{
            initial = "idle",
            moveSpeed = 3,
            attack = {{ damage = 8, range = 2, cooldownMs = 1200 }},
            states = {{
                idle = {{ animation = "idle", motion = "hold",
                         transitions = {{ {{ to = "chase", when = {guard} }} }} }},
                chase = {{ animation = "walk", motion = "chaseTarget" }}{states_extra}
            }}
        }} }} }}"#
    )
}

fn js_error(src: &str) -> String {
    eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err()).to_string()
}

fn lua_error(src: &str) -> String {
    eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err()).to_string()
}

#[test]
fn js_entity_descriptor_parses_a_behavior_graph() {
    let d = eval_js(&js_behavior(JS_NEAR_GUARD, ""), |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap()
    });
    let graph = d.behavior.expect("behavior parsed");
    assert_eq!(graph.initial, "idle");
    assert_eq!(graph.move_speed, 3.0);
    assert_eq!(graph.states.len(), 2);
    assert_eq!(graph.states["chase"].motion, MotionVerb::ChaseTarget);
    assert_eq!(graph.states["idle"].transitions.len(), 1);
    assert_eq!(graph.states["idle"].transitions[0].to, "chase");
    assert_eq!(
        graph.attack.expect("attack block").cooldown_ms,
        1200.0,
        "attack tuning survives the bridge"
    );
    // Absent `deathDespawnMs` resolves through the shared default.
    assert_eq!(graph.death_despawn_ms, None);
    assert_eq!(
        graph.death_despawn_ms(),
        BehaviorGraphDescriptor::DEFAULT_DEATH_DESPAWN_MS
    );
    assert!(d.ai.is_none());
}

#[test]
fn lua_entity_descriptor_parses_a_behavior_graph() {
    // Luau parity: a missing arm would silently drop `behavior`. Assert the arm
    // exists and the same shape parses identically.
    let d = eval_lua(&lua_behavior(LUA_NEAR_GUARD, ""), |v| {
        entity_descriptor_from_lua(v).unwrap()
    });
    let graph = d.behavior.expect("behavior parsed by the Luau arm");
    assert_eq!(graph.initial, "idle");
    assert_eq!(graph.move_speed, 3.0);
    assert_eq!(graph.states["chase"].motion, MotionVerb::ChaseTarget);
    assert_eq!(graph.states["idle"].transitions[0].to, "chase");
    assert_eq!(graph.attack.expect("attack block").cooldown_ms, 1200.0);
    assert_eq!(
        graph.death_despawn_ms(),
        BehaviorGraphDescriptor::DEFAULT_DEATH_DESPAWN_MS
    );
}

#[test]
fn js_behavior_rejects_an_initial_naming_no_declared_state() {
    let src = js_behavior(JS_NEAR_GUARD, "").replace(r#"initial: "idle""#, r#"initial: "patrol""#);
    let err = js_error(&src);
    assert!(
        err.contains("`components.behavior.initial`") && err.contains("patrol"),
        "{err}"
    );
}

#[test]
fn lua_behavior_rejects_an_initial_naming_no_declared_state() {
    let src =
        lua_behavior(LUA_NEAR_GUARD, "").replace(r#"initial = "idle""#, r#"initial = "patrol""#);
    let err = lua_error(&src);
    assert!(
        err.contains("`components.behavior.initial`") && err.contains("patrol"),
        "{err}"
    );
}

#[test]
fn js_behavior_rejects_a_transition_target_naming_no_declared_state() {
    // The message must name the state and the transition index, so an author
    // with several edges knows which one is wrong.
    let src = js_behavior(
        JS_NEAR_GUARD,
        &format!(
            r#", flee: {{ animation: "run", motion: "hold",
                 transitions: [{{ to: "chase", when: {JS_NEAR_GUARD} }},
                               {{ to: "hide", when: {JS_NEAR_GUARD} }}] }}"#
        ),
    );
    let err = js_error(&src);
    assert!(
        err.contains("`components.behavior.states.flee.transitions[1].to`") && err.contains("hide"),
        "{err}"
    );
}

#[test]
fn lua_behavior_rejects_a_transition_target_naming_no_declared_state() {
    let src = lua_behavior(
        LUA_NEAR_GUARD,
        &format!(
            r#", flee = {{ animation = "run", motion = "hold",
                 transitions = {{ {{ to = "chase", when = {LUA_NEAR_GUARD} }},
                                  {{ to = "hide", when = {LUA_NEAR_GUARD} }} }} }}"#
        ),
    );
    let err = lua_error(&src);
    assert!(
        err.contains("`components.behavior.states.flee.transitions[1].to`") && err.contains("hide"),
        "{err}"
    );
}

#[test]
fn js_behavior_rejects_an_interrupt_target_naming_no_declared_state() {
    let src = js_behavior(JS_NEAR_GUARD, "").replace(
        r#"moveSpeed: 3,"#,
        &format!(r#"moveSpeed: 3, interrupts: [{{ to: "flinch", when: {JS_NEAR_GUARD} }}],"#),
    );
    let err = js_error(&src);
    assert!(
        err.contains("`components.behavior.interrupts[0].to`") && err.contains("flinch"),
        "{err}"
    );
}

#[test]
fn lua_behavior_rejects_an_interrupt_target_naming_no_declared_state() {
    let src = lua_behavior(LUA_NEAR_GUARD, "").replace(
        r#"moveSpeed = 3,"#,
        &format!(
            r#"moveSpeed = 3, interrupts = {{ {{ to = "flinch", when = {LUA_NEAR_GUARD} }} }},"#
        ),
    );
    let err = lua_error(&src);
    assert!(
        err.contains("`components.behavior.interrupts[0].to`") && err.contains("flinch"),
        "{err}"
    );
}

#[test]
fn js_behavior_rejects_an_empty_state_map() {
    let src = r#"({ components: { behavior: {
        initial: "idle", moveSpeed: 3, states: {}
    } } })"#;
    let err = js_error(src);
    assert!(err.contains("at least one state"), "{err}");
}

#[test]
fn lua_behavior_rejects_an_empty_state_map() {
    // An empty Luau table converts to a JSON object, matching the JS `{}`.
    let src = r#"return { components = { behavior = {
        initial = "idle", moveSpeed = 3, states = {}
    } } }"#;
    let err = lua_error(src);
    assert!(err.contains("at least one state"), "{err}");
}

#[test]
fn js_behavior_rejects_a_guard_naming_an_unknown_input() {
    let guard = JS_NEAR_GUARD.replace("@brain.targetDistance", "@brain.morale");
    let err = js_error(&js_behavior(&guard, ""));
    assert!(
        err.contains("`components.behavior.states.idle.transitions[0].when`")
            && err.contains("@brain.morale"),
        "{err}"
    );
}

#[test]
fn lua_behavior_rejects_a_guard_naming_an_unknown_input() {
    let guard = LUA_NEAR_GUARD.replace("@brain.targetDistance", "@brain.morale");
    let err = lua_error(&lua_behavior(&guard, ""));
    assert!(
        err.contains("`components.behavior.states.idle.transitions[0].when`")
            && err.contains("@brain.morale"),
        "{err}"
    );
}

#[test]
fn js_behavior_rejects_a_type_mismatched_guard() {
    // `hasTarget` is a boolean; feeding it to a numeric comparison is a
    // declaration-time bind error, not a tick-time surprise.
    let guard = JS_NEAR_GUARD.replace("@brain.targetDistance", "@brain.hasTarget");
    let err = js_error(&js_behavior(&guard, ""));
    assert!(
        err.contains("`components.behavior.states.idle.transitions[0].when`"),
        "{err}"
    );
}

#[test]
fn lua_behavior_rejects_a_type_mismatched_guard() {
    let guard = LUA_NEAR_GUARD.replace("@brain.targetDistance", "@brain.hasTarget");
    let err = lua_error(&lua_behavior(&guard, ""));
    assert!(
        err.contains("`components.behavior.states.idle.transitions[0].when`"),
        "{err}"
    );
}

#[test]
fn js_behavior_rejects_a_guard_that_does_not_produce_a_boolean() {
    let guard = r#"{ op: "input", name: "@brain.targetDistance" }"#;
    let err = js_error(&js_behavior(guard, ""));
    assert!(
        err.contains("`components.behavior.states.idle.transitions[0].when`")
            && err.contains("must produce a boolean"),
        "{err}"
    );
}

#[test]
fn lua_behavior_rejects_a_guard_that_does_not_produce_a_boolean() {
    let guard = r#"{ op = "input", name = "@brain.targetDistance" }"#;
    let err = lua_error(&lua_behavior(guard, ""));
    assert!(
        err.contains("`components.behavior.states.idle.transitions[0].when`")
            && err.contains("must produce a boolean"),
        "{err}"
    );
}

#[test]
fn js_behavior_accepts_a_per_entity_state_guard() {
    // The stagger shape: an interrupt over an `@state.` leaf. `@state.` names
    // need no declaration — an unset field reads as zero.
    let guard = r#"{ op: "ge", a: { op: "input", name: "@state.staggered" }, b: { op: "const", value: 1 } }"#;
    let d = eval_js(&js_behavior(guard, ""), |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap()
    });
    assert!(d.behavior.is_some());
}

#[test]
fn lua_behavior_accepts_a_per_entity_state_guard() {
    let guard = r#"{ op = "ge", a = { op = "input", name = "@state.staggered" }, b = { op = "const", value = 1 } }"#;
    let d = eval_lua(&lua_behavior(guard, ""), |v| {
        entity_descriptor_from_lua(v).unwrap()
    });
    assert!(d.behavior.is_some());
}

#[test]
fn js_authoring_both_ai_and_behavior_is_a_parse_error() {
    let src = js_behavior(JS_NEAR_GUARD, "").replace(
        "components: {",
        r#"components: { ai: {
            detectionRange: 18, attackRange: 2.2, leashRange: 26,
            attackDamage: 8, attackCooldownMs: 1200, moveSpeed: 3.5,
            deathDespawnMs: 1500,
            states: { idle: "idle", alert: "walk", attack: "attack", death: "die" } },"#,
    );
    let err = js_error(&src);
    assert!(
        err.contains("`components.ai`") && err.contains("`components.behavior`"),
        "{err}"
    );
}

#[test]
fn lua_authoring_both_ai_and_behavior_is_a_parse_error() {
    let src = lua_behavior(LUA_NEAR_GUARD, "").replace(
        "components = {",
        r#"components = { ai = {
            detectionRange = 18, attackRange = 2.2, leashRange = 26,
            attackDamage = 8, attackCooldownMs = 1200, moveSpeed = 3.5,
            deathDespawnMs = 1500,
            states = { idle = "idle", alert = "walk", attack = "attack", death = "die" } },"#,
    );
    let err = lua_error(&src);
    assert!(
        err.contains("`components.ai`") && err.contains("`components.behavior`"),
        "{err}"
    );
}

#[test]
fn duplicate_state_names_collapse_in_both_runtimes_and_are_rejected_at_the_shared_chokepoint() {
    // Neither a JS object literal nor a Luau table literal can carry a repeated
    // key: both collapse to the last entry before the descriptor bridge sees
    // anything, so both runtimes parse this identically (one state, the second
    // spelling winning).
    let js = r#"({ components: { behavior: {
        initial: "idle", moveSpeed: 3,
        states: { idle: { animation: "idle", motion: "hold" },
                  idle: { animation: "walk", motion: "hold" } }
    } } })"#;
    let lua = r#"return { components = { behavior = {
        initial = "idle", moveSpeed = 3,
        states = { idle = { animation = "idle", motion = "hold" },
                   idle = { animation = "walk", motion = "hold" } }
    } } }"#;
    for graph in [
        eval_js(js, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap())
            .behavior
            .unwrap(),
        eval_lua(lua, |v| entity_descriptor_from_lua(v).unwrap())
            .behavior
            .unwrap(),
    ] {
        assert_eq!(graph.states.len(), 1);
        assert_eq!(graph.states["idle"].animation, "walk");
    }

    // The raw-JSON deserialize path CAN carry a repeated key, and there a
    // silent last-writer-wins would be invisible. The shared serde chokepoint
    // both runtimes funnel through rejects it.
    let err = serde_json::from_str::<BehaviorGraphDescriptor>(
        r#"{ "initial": "idle", "moveSpeed": 3,
             "states": { "idle": { "animation": "idle", "motion": "hold" },
                         "idle": { "animation": "walk", "motion": "hold" } } }"#,
    )
    .unwrap_err();
    assert!(err.to_string().contains("duplicate state name"), "{err}");
}

// --- SDK guard-input helpers ---------------------------------------------

/// `sdk/lib/brain.{ts,luau}` are hand-maintained prelude helpers, not generated
/// declarations, so nothing recompiles when `BRAIN_INPUTS` grows. This test is
/// the sync obligation: it fails until both SDK sources cover exactly the
/// foundation table.
#[test]
fn brain_sdk_helpers_cover_every_brain_input() {
    const BRAIN_TS_SRC: &str = include_str!("../../../../../sdk/lib/brain.ts");
    const BRAIN_LUAU_SRC: &str = include_str!("../../../../../sdk/lib/brain.luau");
    use postretro_foundation::brain::BRAIN_INPUTS;

    // Luau: evaluate the real module and compare the produced leaves.
    let lua = mlua::Lua::new();
    let module: Table = lua
        .load(BRAIN_LUAU_SRC)
        .set_name("sdk/lib/brain.luau")
        .eval()
        .expect("brain.luau evaluates");
    let brain: Table = module.get("brain").expect("brain.luau exports `brain`");

    let mut luau_keys: BTreeSet<String> = BTreeSet::new();
    for pair in brain.clone().pairs::<String, LuaValue>() {
        luau_keys.insert(pair.expect("brain entry").0);
    }
    let expected: BTreeSet<String> = BRAIN_INPUTS
        .iter()
        .map(|(name, _)| {
            name.strip_prefix("@brain.")
                .expect("every fixed brain input is `@brain.`-prefixed")
                .to_string()
        })
        .collect();
    assert_eq!(
        luau_keys, expected,
        "sdk/lib/brain.luau must expose exactly the BRAIN_INPUTS table"
    );

    for (name, _) in BRAIN_INPUTS {
        let property = name.strip_prefix("@brain.").unwrap();
        let leaf: Table = brain.get(property).expect("brain input leaf");
        assert_eq!(leaf.get::<String>("op").unwrap(), "input");
        assert_eq!(leaf.get::<String>("name").unwrap(), name);

        // TypeScript has no runtime here; the source text is the contract.
        assert!(
            BRAIN_TS_SRC.contains(&format!(
                "{property}: Object.freeze(runtime.read(\"{name}\"))"
            )),
            "sdk/lib/brain.ts must wrap `{name}` as `{property}`"
        );
    }
    assert_eq!(
        BRAIN_TS_SRC.matches("\"@brain.").count(),
        BRAIN_INPUTS.len(),
        "sdk/lib/brain.ts must name exactly the BRAIN_INPUTS entries"
    );

    // `state(name)` builds the per-entity leaf in both runtimes.
    let state: mlua::Function = module.get("state").expect("brain.luau exports `state`");
    let leaf: Table = state.call("staggered").expect("state(\"staggered\")");
    assert_eq!(leaf.get::<String>("name").unwrap(), "@state.staggered");
    assert!(BRAIN_TS_SRC.contains(r#"runtime.read("@state." + name)"#));
}
