// Tests: behavior-graph component parsing and the SDK guard-input helpers.

use super::super::*;
use super::common::*;

// --- components.behavior (both parsers) ----------------------------------
//
// Both runtimes convert the authored block to JSON and funnel it through the
// same `BehaviorGraphDescriptor` serde + `validate` chokepoint, so acceptance
// and every rejection below is identical by construction. Tested on a bare
// descriptor value with NO entity materialized: the state → mesh
// animation-state mapping is cross-component and stays a SPAWN-time check.

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
    // Absent `engagementRadius` falls back to `attack.range` (the fixture's 2).
    assert_eq!(graph.engagement_radius, None);
    assert_eq!(graph.engagement_radius(), 2.0);
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
    assert_eq!(graph.engagement_radius, None);
    assert_eq!(graph.engagement_radius(), 2.0);
}

#[test]
fn both_runtimes_reject_invalid_or_non_boolean_candidate_filters_with_the_authored_path() {
    let js_invalid = js_behavior(JS_NEAR_GUARD, "").replace(
        "moveSpeed: 3,",
        r#"moveSpeed: 3, candidateFilter: { op: "input", name: "@candidate.missing" },"#,
    );
    let lua_invalid = lua_behavior(LUA_NEAR_GUARD, "").replace(
        "moveSpeed = 3,",
        r#"moveSpeed = 3, candidateFilter = { op = "input", name = "@candidate.missing" },"#,
    );
    let js_number = js_behavior(JS_NEAR_GUARD, "").replace(
        "moveSpeed: 3,",
        "moveSpeed: 3, candidateFilter: { op: \"const\", value: 1 },",
    );
    let lua_number = lua_behavior(LUA_NEAR_GUARD, "").replace(
        "moveSpeed = 3,",
        "moveSpeed = 3, candidateFilter = { op = \"const\", value = 1 },",
    );
    for error in [
        js_error(&js_invalid),
        lua_error(&lua_invalid),
        js_error(&js_number),
        lua_error(&lua_number),
    ] {
        assert!(
            error.contains("components.behavior.candidateFilter"),
            "{error}"
        );
    }
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

/// Candidate helpers are a separate fixed vocabulary. The TypeScript literal
/// count deliberately catches an added documentation spelling as well as a
/// missing wrapper, while the Luau half checks the module's exposed key set.
#[test]
fn candidate_sdk_helpers_cover_every_candidate_input() {
    const BRAIN_TS_SRC: &str = include_str!("../../../../../sdk/lib/brain.ts");
    const BRAIN_LUAU_SRC: &str = include_str!("../../../../../sdk/lib/brain.luau");
    use postretro_foundation::candidate::CANDIDATE_INPUTS;

    let lua = mlua::Lua::new();
    let module: Table = lua
        .load(BRAIN_LUAU_SRC)
        .set_name("sdk/lib/brain.luau")
        .eval()
        .expect("brain.luau evaluates");
    let candidate: Table = module
        .get("candidate")
        .expect("brain.luau exports `candidate`");
    let luau_keys: BTreeSet<String> = candidate
        .clone()
        .pairs::<String, LuaValue>()
        .map(|entry| entry.expect("candidate entry").0)
        .collect();
    let expected: BTreeSet<String> = CANDIDATE_INPUTS
        .iter()
        .map(|(name, _)| name.strip_prefix("@candidate.").unwrap().to_string())
        .collect();
    assert_eq!(luau_keys, expected);

    for (name, _) in CANDIDATE_INPUTS {
        let property = name.strip_prefix("@candidate.").unwrap();
        let leaf: Table = candidate.get(property).expect("candidate input leaf");
        assert_eq!(leaf.get::<String>("op").unwrap(), "input");
        assert_eq!(leaf.get::<String>("name").unwrap(), name);
        assert!(
            BRAIN_TS_SRC.contains(&format!(
                "{property}: Object.freeze(runtime.read(\"{name}\"))"
            )),
            "sdk/lib/brain.ts must wrap `{name}` as `{property}`"
        );
    }
    assert_eq!(
        BRAIN_TS_SRC.matches("\"@candidate.").count(),
        CANDIDATE_INPUTS.len()
    );
}

// --- the shipped reference enemy (both authorings) ------------------------

/// The TypeScript authoring of the map-placeable reference enemy — the one
/// archetype real content places.
const REFERENCE_ENTITIES_TS_SRC: &str =
    include_str!("../../../../../sdk/behaviors/reference/entities.ts");
/// The Luau twin of [`REFERENCE_ENTITIES_TS_SRC`].
const REFERENCE_ENTITIES_LUAU_SRC: &str =
    include_str!("../../../../../sdk/behaviors/reference/entities.luau");

/// Bundle the shipped TypeScript module through the production `scripts-build`
/// path, run it in the real QuickJS SDK prelude, and parse `referenceEnemyEntity`
/// with the production JS descriptor bridge.
///
/// The module is compiled VERBATIM: a trailing statement stashes the exported
/// descriptor on `globalThis` so the value is read back independently of however
/// the bundler wraps the module's own exports.
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
    let subsys =
        crate::quickjs::QuickJsSubsystem::new(&registry, &crate::quickjs::QuickJsConfig::default())
            .expect("quickjs definition context");
    subsys.definition_ctx().with(|ctx| {
        let _: JsValue = crate::quickjs::run_script(&ctx, &bundled, "entities.ts")
            .expect("the shipped reference entities module evaluates");
        let value: JsValue =
            crate::quickjs::run_script(&ctx, "globalThis.__referenceEntity", "read")
                .expect("the module exported the requested reference descriptor");
        entity_descriptor_from_js(&ctx, value).expect("the shipped TS reference descriptor parses")
    })
}

/// Luau twin of [`shipped_reference_enemy_from_typescript`]: evaluate the shipped
/// module in a real mod-rooted Luau state (whose prelude supplies the
/// `defineEntity` / `runtime` / `brain` globals it authors against) and parse the
/// result with the production Luau descriptor bridge.
fn shipped_reference_descriptor_from_luau(export_name: &str) -> EntityTypeDescriptor {
    let lua = crate::luau::build_lua_state(
        &[],
        None,
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))),
    )
    .expect("mod-rooted luau state");
    // The module returns its own `M` table; wrapping it in an immediately-called
    // function keeps the shipped source verbatim while letting this reach the
    // one export under test.
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

/// The shipped reference enemy is the one archetype a real map places, and it is
/// authored twice — `entities.ts` and `entities.luau`. Nothing else in the repo
/// reads either file, so before this test a constant could be retuned in one
/// spelling (or in both, away from what the engine tests assume) with the whole
/// suite staying green.
///
/// WHAT THIS CATCHES. Both shipped sources are compiled/evaluated verbatim
/// through the PRODUCTION paths — `scripts-build` + the QuickJS SDK prelude on
/// one side, the mod-rooted Luau prelude on the other — and each result is run
/// through the production descriptor bridge. The two resulting
/// `BehaviorGraphDescriptor`s are then compared for full structural equality, so
/// ANY divergence between the twins fails here: a retuned range, a reordered
/// edge, a changed motion/action verb, a renamed state or animation, a dropped
/// interrupt, a different guard tree. The absolute pin below additionally fails
/// when both twins are changed together in a way that leaves the shipped enemy
/// disagreeing with the shape the engine's parity tests assume.
///
/// The pose fixture below extends the same production-path coverage to the
/// cross-component mesh-animation names its direct graph drives.
#[test]
fn the_shipped_reference_enemy_graph_is_identical_in_both_authorings() {
    let ts = shipped_reference_enemy_from_typescript()
        .behavior
        .expect("the TS reference enemy carries a behavior graph");
    let luau = shipped_reference_enemy_from_luau()
        .behavior
        .expect("the Luau reference enemy carries a behavior graph");

    assert_eq!(
        ts, luau,
        "the shipped reference enemy's two authorings must produce the identical graph"
    );
    assert!(
        postretro_foundation::data_descriptors::types::behavior_lints::inspect(&ts).is_empty(),
        "the shipped reference enemy authors both disengagement paths"
    );

    // The absolute pin: the shape the engine's behavior tests assume.
    assert_eq!(ts.initial, "idle");
    assert_eq!(ts.move_speed, 3.0);
    assert_eq!(ts.engagement_radius, Some(2.0));
    let attack = ts.attack.expect("the reference enemy attacks");
    assert_eq!(
        (attack.damage, attack.range, attack.cooldown_ms),
        (8.0, 2.0, 1200.0)
    );

    assert_eq!(
        ts.states.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["alert", "attack", "idle"],
        "three states, and no `death` state — death is not a graph transition"
    );
    for (name, animation, motion, action) in [
        ("idle", "idle", MotionVerb::Hold, None),
        ("alert", "walk", MotionVerb::ChaseTarget, None),
        (
            "attack",
            "attack",
            MotionVerb::ChaseTarget,
            Some(ActionVerb::Attack),
        ),
    ] {
        let state = &ts.states[name];
        assert_eq!(state.animation, animation, "`{name}` animation");
        assert_eq!(state.motion, motion, "`{name}` motion");
        assert_eq!(state.action, action, "`{name}` action");
    }

    // Edge ORDER is load-bearing: guards are first-true-wins, and the stand-down
    // interrupt outranks every state-local edge.
    assert_eq!(
        ts.interrupts
            .iter()
            .map(|edge| edge.to.as_str())
            .collect::<Vec<_>>(),
        vec!["idle", "idle", "idle"],
        "the ordered any-state edges stand down on target loss, target death, then range"
    );
    assert!(
        ts.candidate_filter.is_some(),
        "the reference graph filters dead candidates and bounds acquisition"
    );
    for (state, targets) in [
        ("idle", vec!["attack", "alert"]),
        ("alert", vec!["attack", "idle"]),
        ("attack", vec!["alert"]),
    ] {
        assert_eq!(
            ts.states[state]
                .transitions
                .iter()
                .map(|edge| edge.to.as_str())
                .collect::<Vec<_>>(),
            targets,
            "`{state}` edge targets, in declaration order"
        );
    }
}

/// The E21 pose-modifier fixture is a real, shipped animated mesh rather than a
/// test-only descriptor. Its direct graph must survive both production parser
/// paths, stay identical between authorings, and drive only declared mesh
/// animation states. That final assertion mirrors the spawn-time validation
/// contract at the descriptor boundary, where this test can show the full
/// authored data that reaches the loader.
#[test]
fn shipped_pose_fixture_is_a_direct_graph_with_valid_mesh_animation_states() {
    let ts = shipped_reference_descriptor_from_typescript("poseFixtureEnemyEntity");
    let luau = shipped_reference_descriptor_from_luau("poseFixtureEnemyEntity");

    assert_eq!(
        ts.behavior, luau.behavior,
        "the pose graphs stay in lockstep"
    );
    assert_eq!(
        ts.mesh, luau.mesh,
        "the pose mesh declarations stay in lockstep"
    );

    for descriptor in [&ts, &luau] {
        let graph = descriptor
            .behavior
            .as_ref()
            .expect("the pose fixture carries a direct behavior graph");
        let mesh = descriptor
            .mesh
            .as_ref()
            .expect("the pose fixture carries an animated mesh");
        assert_eq!(
            graph
                .interrupts
                .iter()
                .map(|edge| edge.to.as_str())
                .collect::<Vec<_>>(),
            vec!["idle", "idle", "idle"],
            "target loss, target death, then authored 50 m stand-down remain ordered"
        );
        for state in graph.states.values() {
            assert!(
                mesh.animations.contains_key(&state.animation),
                "behavior state animation `{}` must be declared by the pose mesh",
                state.animation
            );
        }
    }
}

// --- empty array-valued fields (twin parity) -----------------------------
//
// An empty Luau table is ambiguous: `{}` is both the empty array and the empty
// map, and the Lua→JSON bridge resolves it to an OBJECT because it cannot see
// the target type. Declaring "this graph has no interrupts" is the spelling
// authors reach for most, so the two runtimes must agree on it.

#[test]
fn both_parsers_accept_an_empty_interrupts_list() {
    let js = eval_js(
        &js_behavior(JS_NEAR_GUARD, "").replace("initial:", "interrupts: [], initial:"),
        |ctx, v| entity_descriptor_from_js(ctx, v).unwrap(),
    );
    let lua = eval_lua(
        &lua_behavior(LUA_NEAR_GUARD, "").replace("initial =", "interrupts = {}, initial ="),
        |v| entity_descriptor_from_lua(v).unwrap(),
    );

    let (js_graph, lua_graph) = (
        js.behavior.expect("JS behavior parsed"),
        lua.behavior.expect("Luau behavior parsed"),
    );
    assert!(js_graph.interrupts.is_empty());
    assert!(
        lua_graph.interrupts.is_empty(),
        "`interrupts = {{}}` must parse as the empty list, not fail as a map"
    );
    assert_eq!(
        js_graph, lua_graph,
        "the two spellings must produce the identical graph"
    );
}

#[test]
fn both_parsers_accept_an_empty_transitions_list_on_a_state() {
    // `chase` already declares no `transitions` key at all; this pins the
    // explicit empty spelling, which travels the same ambiguous bridge.
    let js = eval_js(
        &js_behavior(JS_NEAR_GUARD, "").replace(
            r#"chase: { animation: "walk", motion: "chaseTarget" }"#,
            r#"chase: { animation: "walk", motion: "chaseTarget", transitions: [] }"#,
        ),
        |ctx, v| entity_descriptor_from_js(ctx, v).unwrap(),
    );
    let lua = eval_lua(
        &lua_behavior(LUA_NEAR_GUARD, "").replace(
            r#"chase = { animation = "walk", motion = "chaseTarget" }"#,
            r#"chase = { animation = "walk", motion = "chaseTarget", transitions = {} }"#,
        ),
        |v| entity_descriptor_from_lua(v).unwrap(),
    );

    let (js_graph, lua_graph) = (
        js.behavior.expect("JS behavior parsed"),
        lua.behavior.expect("Luau behavior parsed"),
    );
    assert!(js_graph.states["chase"].transitions.is_empty());
    assert!(lua_graph.states["chase"].transitions.is_empty());
    assert_eq!(js_graph, lua_graph);
}

#[test]
fn lua_behavior_rejects_a_named_key_table_where_an_array_belongs() {
    // Only the EMPTY table is re-seated as an array. A table with named keys is
    // a genuine authoring mistake, and the error must name the path — serde's
    // own message names neither the field nor the state.
    let src = lua_behavior(LUA_NEAR_GUARD, "").replace(
        "initial =",
        r#"interrupts = { flinch = { to = "chase" } }, initial ="#,
    );
    let err = lua_error(&src);
    assert!(
        err.contains("components.behavior.interrupts") && err.contains("flinch"),
        "the error must name the authored path and the offending key: {err}"
    );
}

#[test]
fn lua_behavior_rejects_a_named_key_transitions_table_naming_its_state() {
    let src = lua_behavior(LUA_NEAR_GUARD, "").replace(
        r#"chase = { animation = "walk", motion = "chaseTarget" }"#,
        r#"chase = { animation = "walk", motion = "chaseTarget", transitions = { onward = {} } }"#,
    );
    let err = lua_error(&src);
    assert!(
        err.contains("components.behavior.states.chase.transitions"),
        "the error must name the state whose transitions are malformed: {err}"
    );
}

#[test]
fn js_behavior_rejects_a_named_key_object_where_an_array_belongs() {
    // JS twin of `lua_behavior_rejects_a_named_key_table_where_an_array_belongs`.
    // JavaScript has no `{}`/`[]` ambiguity to re-seat, but the mistake is just
    // as natural — and serde's bare "invalid type: map, expected a sequence"
    // names neither the field nor the offending key.
    let src = js_behavior(JS_NEAR_GUARD, "").replace(
        "initial:",
        r#"interrupts: { flinch: { to: "chase" } }, initial:"#,
    );
    let err = js_error(&src);
    assert!(
        err.contains("components.behavior.interrupts") && err.contains("flinch"),
        "the error must name the authored path and the offending key: {err}"
    );
}

#[test]
fn js_behavior_rejects_a_named_key_transitions_object_naming_its_state() {
    let src = js_behavior(JS_NEAR_GUARD, "").replace(
        r#"chase: { animation: "walk", motion: "chaseTarget" }"#,
        r#"chase: { animation: "walk", motion: "chaseTarget", transitions: { onward: {} } }"#,
    );
    let err = js_error(&src);
    assert!(
        err.contains("components.behavior.states.chase.transitions"),
        "the error must name the state whose transitions are malformed: {err}"
    );
}

// --- engagementRadius (both parsers) --------------------------------------

/// A behavior block with neither `attack` nor `engagementRadius` — the
/// pure-pursuit graph whose radius must resolve to the shared default rather
/// than to zero. JS source body; [`LUA_PURSUIT_ONLY`] is its twin.
const JS_PURSUIT_ONLY: &str = r#"({ components: { behavior: {
    initial: "idle", moveSpeed: 3,
    states: { idle: { animation: "idle", motion: "chaseTarget" } }
} } })"#;
/// Luau twin of [`JS_PURSUIT_ONLY`].
const LUA_PURSUIT_ONLY: &str = r#"return { components = { behavior = {
    initial = "idle", moveSpeed = 3,
    states = { idle = { animation = "idle", motion = "chaseTarget" } }
} } }"#;

#[test]
fn both_parsers_carry_an_explicit_engagement_radius_over_the_attack_range() {
    // The base fixture authors `attack.range = 2`; the explicit field outranks
    // it, so a graph can space its chasers wider than it can damage.
    let js = eval_js(
        &js_behavior(JS_NEAR_GUARD, "")
            .replace("moveSpeed: 3,", "moveSpeed: 3, engagementRadius: 4.5,"),
        |ctx, v| entity_descriptor_from_js(ctx, v).unwrap(),
    );
    let lua = eval_lua(
        &lua_behavior(LUA_NEAR_GUARD, "")
            .replace("moveSpeed = 3,", "moveSpeed = 3, engagementRadius = 4.5,"),
        |v| entity_descriptor_from_lua(v).unwrap(),
    );

    let (js_graph, lua_graph) = (
        js.behavior.expect("JS behavior parsed"),
        lua.behavior.expect("Luau behavior parsed"),
    );
    assert_eq!(js_graph.engagement_radius, Some(4.5));
    assert_eq!(js_graph.engagement_radius(), 4.5);
    assert_eq!(lua_graph.engagement_radius(), 4.5);
    assert_eq!(js_graph, lua_graph);
}

#[test]
fn both_parsers_resolve_an_absent_engagement_radius_to_the_default_without_an_attack_block() {
    // No `attack` to fall back to: this is the case that must NOT land on zero,
    // since a zero ring generates no combat slots and every chaser piles onto
    // the raw target position.
    let js_graph = eval_js(JS_PURSUIT_ONLY, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap()
    })
    .behavior
    .expect("JS behavior parsed");
    let lua_graph = eval_lua(LUA_PURSUIT_ONLY, |v| entity_descriptor_from_lua(v).unwrap())
        .behavior
        .expect("Luau behavior parsed");

    for graph in [&js_graph, &lua_graph] {
        assert_eq!(graph.engagement_radius, None);
        assert!(graph.attack.is_none());
        assert_eq!(
            graph.engagement_radius(),
            BehaviorGraphDescriptor::DEFAULT_ENGAGEMENT_RADIUS
        );
    }
    assert_eq!(js_graph, lua_graph);
}

#[test]
fn both_parsers_reject_a_non_positive_engagement_radius_with_its_path() {
    for value in ["0", "-1.5"] {
        let js = js_error(&js_behavior(JS_NEAR_GUARD, "").replace(
            "moveSpeed: 3,",
            &format!("moveSpeed: 3, engagementRadius: {value},"),
        ));
        let lua = lua_error(&lua_behavior(LUA_NEAR_GUARD, "").replace(
            "moveSpeed = 3,",
            &format!("moveSpeed = 3, engagementRadius = {value},"),
        ));
        for err in [&js, &lua] {
            assert!(
                err.contains("`components.behavior.engagementRadius`")
                    && err.contains("finite value > 0.0"),
                "{err}"
            );
        }
        assert_eq!(js, lua, "the twins must report the same text");
    }
}

#[test]
fn both_parsers_reject_a_non_finite_engagement_radius_with_its_path() {
    // Pinned to the OPPOSITE outcome before: the shared JSON bridge has no
    // representation for Infinity/NaN, so both runtimes converted them to null,
    // and `Option<f32>` read null as absent. That made `engagementRadius: -1` a
    // clean validation error while `engagementRadius: Infinity` silently became
    // the default — the same authoring mistake with two outcomes, on every
    // optional numeric field of every descriptor. The bridge now rejects
    // non-finite numbers and names the field. The finiteness rule still guards
    // the raw-JSON and lowering paths
    // (`move_speed_and_engagement_radius_must_be_finite_and_positive`). What
    // matters at this seam is that the two runtimes reject on the same
    // condition and report the same reason and path; only the VM-supplied
    // wrapper around the message differs, as it does for the depth guard.
    for (js_value, lua_value) in [
        ("Infinity", "math.huge"),
        ("-Infinity", "-math.huge"),
        ("NaN", "0/0"),
    ] {
        let js = js_error(&js_behavior(JS_NEAR_GUARD, "").replace(
            "moveSpeed: 3,",
            &format!("moveSpeed: 3, engagementRadius: {js_value},"),
        ));
        let lua = lua_error(&lua_behavior(LUA_NEAR_GUARD, "").replace(
            "moveSpeed = 3,",
            &format!("moveSpeed = 3, engagementRadius = {lua_value},"),
        ));
        for err in [&js, &lua] {
            assert!(
                err.contains("non-finite number at `engagementRadius`")
                    && err.contains("authored numbers must be finite"),
                "{err}"
            );
        }
    }
}

#[test]
fn both_parsers_name_the_same_nested_path_for_a_non_finite_number() {
    // The rejection is worth nothing to an author who cannot find the field, and
    // a path that differs between runtimes is the divergence this seam exists to
    // prevent. Nested under a state so the message has to walk more than one
    // segment; the twins must name the identical path.
    let js = js_error(&js_behavior(JS_NEAR_GUARD, "").replace(
        r#"chase: { animation: "walk", motion: "chaseTarget" }"#,
        r#"chase: { animation: "walk", motion: "chaseTarget", speedScale: Infinity }"#,
    ));
    let lua = lua_error(&lua_behavior(LUA_NEAR_GUARD, "").replace(
        r#"chase = { animation = "walk", motion = "chaseTarget" }"#,
        r#"chase = { animation = "walk", motion = "chaseTarget", speedScale = math.huge }"#,
    ));
    for err in [&js, &lua] {
        assert!(
            err.contains("non-finite number at `states.chase.speedScale`"),
            "{err}"
        );
    }
}

// --- self-targeting edges (both parsers) ----------------------------------

#[test]
fn both_parsers_reject_a_state_local_transition_targeting_its_own_state() {
    // A state-local self-edge is a silent transition BLOCKER: first-true-wins
    // selection short-circuits on it, so every later transition in that state
    // stops being evaluated — and it does not re-enter. The message must name
    // the state and the index so an author with several edges knows which.
    let js = js_error(&js_behavior(
        JS_NEAR_GUARD,
        &format!(
            r#", flee: {{ animation: "run", motion: "hold",
                 transitions: [{{ to: "chase", when: {JS_NEAR_GUARD} }},
                               {{ to: "flee", when: {JS_NEAR_GUARD} }}] }}"#
        ),
    ));
    let lua = lua_error(&lua_behavior(
        LUA_NEAR_GUARD,
        &format!(
            r#", flee = {{ animation = "run", motion = "hold",
                 transitions = {{ {{ to = "chase", when = {LUA_NEAR_GUARD} }},
                                  {{ to = "flee", when = {LUA_NEAR_GUARD} }} }} }}"#
        ),
    ));
    for err in [&js, &lua] {
        assert!(
            err.contains("`components.behavior.states.flee.transitions[1].to`")
                && err.contains("flee"),
            "{err}"
        );
    }
    assert_eq!(js, lua, "the twins must report the same text");
}

#[test]
fn both_parsers_accept_a_self_targeting_interrupt() {
    // Deliberately asymmetric with the state-local rule above, and pinned here
    // so a future reader does not "fix" the inconsistency: the evaluator SKIPS
    // an interrupt naming the current state rather than letting it win, so it
    // blocks nothing — and the legacy lowering emits exactly this shape for its
    // "stand down on target loss" edge.
    let js = eval_js(
        &js_behavior(JS_NEAR_GUARD, "").replace(
            "moveSpeed: 3,",
            &format!(r#"moveSpeed: 3, interrupts: [{{ to: "idle", when: {JS_NEAR_GUARD} }}],"#),
        ),
        |ctx, v| entity_descriptor_from_js(ctx, v).unwrap(),
    );
    let lua = eval_lua(
        &lua_behavior(LUA_NEAR_GUARD, "").replace(
            "moveSpeed = 3,",
            &format!(
                r#"moveSpeed = 3, interrupts = {{ {{ to = "idle", when = {LUA_NEAR_GUARD} }} }},"#
            ),
        ),
        |v| entity_descriptor_from_lua(v).unwrap(),
    );

    let (js_graph, lua_graph) = (
        js.behavior.expect("JS behavior parsed"),
        lua.behavior.expect("Luau behavior parsed"),
    );
    assert_eq!(js_graph.interrupts[0].to, "idle");
    assert_eq!(lua_graph.interrupts[0].to, "idle");
    assert_eq!(js_graph, lua_graph);
}
