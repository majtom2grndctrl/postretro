//! AC1's `assistable` profile key in both authoring runtimes: defaults, the
//! literal and expression lowering, and each descriptor-level rejection.

use postretro_foundation::{
    ACTIVATION_IR_CAP_REASON, ASSIST_DEFAULT_HOLD_MS, ASSIST_DEFAULT_RADIUS_M, Assistable,
    NumberOrIr,
};
use serde_json::{Value, json};

use super::life_states::{lua_value, parse};

const PATH: &str = "components.lifeStates.states.down.assistable";

fn block(assistable: Value) -> Value {
    json!({"initial":"alive","states":{"alive":{},"down":{"fire":false,"assistable":assistable}}})
}

/// Parse in both runtimes, assert they lower identically, return the `down`
/// state's `assistable`.
fn lowered(assistable: Value) -> Assistable {
    let block = block(assistable);
    let (js, lua) = parse(&block.to_string(), &lua_value(&block), true);
    let js = js.unwrap().life_states.unwrap();
    assert_eq!(js, lua.unwrap().life_states.unwrap(), "{block}");
    assert_eq!(js.state("alive").unwrap().1.assistable, None);
    js.state("down").unwrap().1.assistable.clone().unwrap()
}

/// Both runtimes reject with the same error naming `path`.
fn rejected(js: &str, lua: &str, path: &str) -> String {
    let wrap_js = format!("{{initial:'alive',states:{{alive:{{}},down:{{assistable:{js}}}}}}}");
    let wrap_lua = format!("{{initial='alive',states={{alive={{}},down={{assistable={lua}}}}}}}");
    let (js, lua) = parse(&wrap_js, &wrap_lua, true);
    let js = js.unwrap_err();
    assert_eq!(js, lua.unwrap_err());
    let message = js.to_string();
    assert!(message.contains(&format!("`{path}`")), "{message}");
    message
}

fn rejected_json(assistable: Value, path: &str) -> String {
    rejected(&assistable.to_string(), &lua_value(&assistable), path)
}

/// A right-leaning `add` chain `depth` nodes deep, in JS and Luau source.
fn chain(depth: usize) -> (String, String) {
    (
        format!(
            "(() => {{ let e = {{op:'const',value:1}}; for (let i = 1; i < {depth}; i++) e = {{op:'add',a:e,b:{{op:'const',value:1}}}}; return e; }})()"
        ),
        format!(
            "(function() local e = {{op='const',value=1}}; for i = 2, {depth} do e = {{op='add',a=e,b={{op='const',value=1}}}} end; return e end)()"
        ),
    )
}

#[test]
fn assistable_omitted_fields_lower_to_engine_defaults() {
    let empty = lowered(json!({}));
    assert_eq!(empty, Assistable::default());
    assert_eq!(empty.radius, NumberOrIr::Literal(ASSIST_DEFAULT_RADIUS_M));
    assert_eq!(empty.hold_ms, NumberOrIr::Literal(ASSIST_DEFAULT_HOLD_MS));
    let radius_only = lowered(json!({"radius": 2.5}));
    assert_eq!(radius_only.radius, NumberOrIr::Literal(2.5));
    assert_eq!(radius_only.hold_ms, NumberOrIr::Literal(3000.0));
    let hold_only = lowered(json!({"holdMs": 0}));
    assert_eq!(hold_only.radius, NumberOrIr::Literal(1.5));
    assert_eq!(hold_only.hold_ms, NumberOrIr::Literal(0.0));
    // Each range bound itself is authorable.
    let bounds = lowered(json!({"radius": 16, "holdMs": 60000}));
    assert_eq!(bounds.literal_fault(), None);
}

#[test]
fn assistable_literal_and_expression_profiles_lower_to_one_shape() {
    let literal = lowered(json!({"radius": 2, "holdMs": 1500}));
    let expression = lowered(json!({
        "radius": {"op":"input","name":"medic.reach","owner":"@assist.helper"},
        "holdMs": {"op":"mul",
            "a":{"op":"const","value":3000},
            "b":{"op":"input","name":"player.inLifeState:down","owner":"@assist.target"}}
    }));
    assert_eq!(literal.radius, NumberOrIr::Literal(2.0));
    assert!(matches!(expression.radius, NumberOrIr::Ir(_)));
    assert!(matches!(expression.hold_ms, NumberOrIr::Ir(_)));
    // One serialized shape: the same two keys, a number or an op-tagged node.
    for assistable in [&literal, &expression] {
        let json = serde_json::to_value(assistable).unwrap();
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["holdMs", "radius"]);
        assert_eq!(
            serde_json::from_value::<Assistable>(json).unwrap(),
            *assistable
        );
    }
}

#[test]
fn assistable_rejects_each_literal_fault_with_its_key_path() {
    for (assistable, field) in [
        (json!({"radius": 0}), "radius"),
        (json!({"radius": -1}), "radius"),
        (json!({"radius": 16.5}), "radius"),
        (json!({"radius": 1e-50}), "radius"),
        (json!({"holdMs": -1}), "holdMs"),
        (json!({"holdMs": 60000.5}), "holdMs"),
        (json!({"holdMs": 1e300}), "holdMs"),
        (json!({"radius": "far"}), "radius"),
        (json!({"holdMs": true}), "holdMs"),
    ] {
        rejected_json(assistable, &format!("{PATH}.{field}"));
    }
    for (js, lua, field) in [
        ("{radius:NaN}", "{radius=0/0}", "radius"),
        ("{radius:Infinity}", "{radius=math.huge}", "radius"),
        ("{holdMs:-Infinity}", "{holdMs=-math.huge}", "holdMs"),
        ("{holdMs:NaN}", "{holdMs=0/0}", "holdMs"),
    ] {
        rejected(js, lua, &format!("{PATH}.{field}"));
    }
}

#[test]
fn assistable_rejects_unknown_keys_and_non_object_shapes() {
    rejected_json(json!({"range": 2}), &format!("{PATH}.range"));
    rejected_json(
        json!({"radius": 2, "holdMs": 100, "cooldown": 1}),
        &format!("{PATH}.cooldown"),
    );
    rejected("true", "true", PATH);
    rejected("4", "4", PATH);
    rejected_json(
        json!({"radius": {"op": "bogus"}}),
        &format!("{PATH}.radius"),
    );
}

#[test]
fn assistable_expression_nests_to_the_ir_depth_cap_and_no_further() {
    // Deeper than the life-state object depth, within the activation cap.
    let (js, lua) = chain(32);
    let wrap = |field: &str, value: &str, lua: bool| {
        if lua {
            format!(
                "{{initial='alive',states={{alive={{}},down={{assistable={{{field}={value}}}}}}}}}"
            )
        } else {
            format!(
                "{{initial:'alive',states:{{alive:{{}},down:{{assistable:{{{field}:{value}}}}}}}}}"
            )
        }
    };
    for field in ["radius", "holdMs"] {
        let (js_ok, lua_ok) = parse(&wrap(field, &js, false), &wrap(field, &lua, true), true);
        let js_ok = js_ok.unwrap().life_states.unwrap();
        assert_eq!(js_ok, lua_ok.unwrap().life_states.unwrap());
    }
    let (js, lua) = chain(33);
    for field in ["radius", "holdMs"] {
        let message = rejected(
            &format!("{{{field}:{js}}}"),
            &format!("{{{field}={lua}}}"),
            &format!("{PATH}.{field}"),
        );
        assert!(message.contains(ACTIVATION_IR_CAP_REASON), "{message}");
        assert!(!message.contains("object depth"), "{message}");
    }
    // Far past the cap: the walk stops at the cap, reported at the field.
    let (js, lua) = chain(400);
    let message = rejected(
        &format!("{{radius:{js}}}"),
        &format!("{{radius={lua}}}"),
        &format!("{PATH}.radius"),
    );
    assert!(message.contains(ACTIVATION_IR_CAP_REASON), "{message}");
}

#[test]
fn assistable_expression_node_cap_bounds_wide_and_shared_trees() {
    // 511 distinct nodes nine deep, and a 2^40-node tree of 41 shared objects:
    // both stop at the node cap rather than expanding.
    for (js_leaf, lua_leaf, depth) in [
        (
            "{op:'add',a:build(d-1),b:build(d-1)}",
            "{op='add',a=build(d-1),b=build(d-1)}",
            9,
        ),
        (
            "(() => { const c = build(d-1); return {op:'add',a:c,b:c}; })()",
            "(function() local c = build(d-1); return {op='add',a=c,b=c} end)()",
            30,
        ),
    ] {
        let js = format!(
            "(() => {{ const build = (d) => d <= 1 ? {{op:'const',value:1}} : {js_leaf}; return {{holdMs: build({depth})}}; }})()"
        );
        let lua = format!(
            "(function() local function build(d) if d <= 1 then return {{op='const',value=1}} end return {lua_leaf} end return {{holdMs=build({depth})}} end)()"
        );
        let message = rejected(&js, &lua, &format!("{PATH}.holdMs"));
        assert!(message.contains(ACTIVATION_IR_CAP_REASON), "{message}");
    }
}
