// Dual-runtime parity: the same behavior-IR expression authored in TypeScript
// (QuickJS) and Luau must produce byte-identical IR once canonicalized through
// the `IrNode` serde form. See: context/lib/scripting.md §11.
//
// Crossing path: each runtime authors the expression with the `runtime` builder
// vocabulary installed by the SDK prelude and returns the node through the
// existing `run_script` / `run_source` value path — no new collection sink.
// The QuickJS side returns `JSON.stringify(node)` (deserialized with
// `serde_json`); the Luau side returns the node table (deserialized via the
// `conv`/mlua serde bridge with `Lua::from_value`). Both land in `IrNode`, are
// re-serialized through one canonical `serde_json::to_string`, and compared.
// Canonicalizing Rust-side sidesteps cross-runtime float-formatting and
// key-order differences.

use mlua::LuaSerdeExt as _;
use std::path::Path;

use crate::ir::IrNode;
use crate::luau::{LuauConfig, LuauSubsystem, Which, build_lua_state};
use crate::primitives_registry::PrimitiveRegistry;
use crate::quickjs::{QuickJsConfig, QuickJsSubsystem, run_script};

/// Author `expr_src` in QuickJS as a bare expression evaluating to an IR node,
/// return it as a JSON string, and canonicalize through `IrNode`.
fn quickjs_canonical(expr_src: &str) -> String {
    let registry = PrimitiveRegistry::new();
    let subsys = QuickJsSubsystem::new(&registry, &QuickJsConfig::default()).unwrap();

    let json = subsys.definition_ctx().with(|ctx| {
        let src = format!("JSON.stringify({expr_src})");
        run_script::<String>(&ctx, &src, "parity.js").expect("quickjs eval")
    });

    let node: IrNode = serde_json::from_str(&json).expect("quickjs json -> IrNode");
    serde_json::to_string(&node).expect("canonicalize quickjs node")
}

/// Author `expr_src` in Luau as a bare expression evaluating to an IR node,
/// return the table, and canonicalize through `IrNode` via the mlua serde
/// bridge.
fn luau_canonical(expr_src: &str) -> String {
    let registry = PrimitiveRegistry::new();
    let subsys = LuauSubsystem::new(&registry, &LuauConfig::default()).unwrap();

    let value: mlua::Value = subsys
        .run_source(
            Which::Definition,
            &format!("return {expr_src}"),
            "parity.luau",
        )
        .expect("luau eval");

    let node: IrNode = subsys
        .definition_lua()
        .from_value(value)
        .expect("luau value -> IrNode");
    serde_json::to_string(&node).expect("canonicalize luau node")
}

/// Run one authored TypeScript fixture after the SDK prelude has been bundled
/// to JavaScript, then return its descriptor-shaped JSON. UI imports are
/// stripped by `scripts-build`, leaving the prelude globals used here.
fn quickjs_fixture_value(source: &str) -> serde_json::Value {
    let registry = PrimitiveRegistry::new();
    let subsys = QuickJsSubsystem::new(&registry, &QuickJsConfig::default()).unwrap();

    let json = subsys.definition_ctx().with(|ctx| {
        run_script::<String>(&ctx, source, "increment-predicate.ts").expect("quickjs fixture eval")
    });

    serde_json::from_str(&json).expect("quickjs fixture json")
}

/// Run one authored Luau fixture and bridge its descriptor-shaped return value
/// through serde. The fixture uses the public `require("postretro/ui")`
/// module, matching the Luau authoring surface rather than internal globals.
fn luau_fixture_value(source: &str) -> serde_json::Value {
    // A mod-rooted state installs the virtual `postretro/ui` module. The
    // long-lived definition-only state deliberately leaves `require` absent.
    let lua = build_lua_state(&[], None, Some(Path::new(env!("CARGO_MANIFEST_DIR"))))
        .expect("build mod-rooted luau fixture state");
    let value: mlua::Value = lua
        .load(source)
        .set_name("increment-predicate.luau")
        .eval()
        .expect("luau fixture eval");

    lua.from_value(value).expect("luau fixture value -> json")
}

/// Canonicalize one RuntimeValue nested in a fixture descriptor through the
/// Rust IR wire type. This is the byte-level parity contract: both author
/// surfaces must emit the same canonical `IrNode` JSON, independent of their
/// host runtimes' object/table iteration order.
fn fixture_ir_canonical(value: &serde_json::Value, pointer: &str) -> String {
    let node: IrNode = serde_json::from_value(
        value
            .pointer(pointer)
            .unwrap_or_else(|| panic!("fixture missing RuntimeValue at {pointer}"))
            .clone(),
    )
    .unwrap_or_else(|error| panic!("fixture RuntimeValue at {pointer} is invalid: {error}"));
    serde_json::to_string(&node).expect("canonicalize fixture RuntimeValue")
}

/// Asserts the TS and Luau spellings of the same expression canonicalize to
/// byte-identical IR. `expr_src` must be valid as both a JS and a Luau
/// expression — the `runtime.*` builders share an identical surface, so a
/// string using only `runtime.<op>(...)` and numeric/boolean/string literals
/// works in both.
fn assert_parity(expr_src: &str) {
    let ts = quickjs_canonical(expr_src);
    let luau = luau_canonical(expr_src);
    assert_eq!(
        ts, luau,
        "IR parity drift for `{expr_src}`:\n  ts:   {ts}\n  luau: {luau}"
    );
}

#[test]
fn nested_arithmetic_expression_is_byte_identical_across_runtimes() {
    // shield = clamp(base + charges * 10, 0, 100) authored against named inputs.
    assert_parity(
        "runtime.clamp(runtime.add(runtime.read(\"base\"), runtime.mul(runtime.read(\"charges\"), runtime.constant(10))), runtime.constant(0), runtime.constant(100))",
    );
}

#[test]
fn select_with_comparison_is_byte_identical_across_runtimes() {
    // select(speed > threshold, lerp(a, b, t), const) — exercises select,
    // comparison, lerp, const, and a boolean literal leaf.
    assert_parity(
        "runtime.select(runtime.gt(runtime.read(\"speed\"), runtime.constant(5)), runtime.lerp(runtime.constant(0), runtime.constant(1), runtime.read(\"t\")), runtime.constant(true))",
    );
}

#[test]
fn bare_literal_operands_canonicalize_to_explicit_constant_form() {
    // The literal-wrap sugar must canonicalize byte-identically to the
    // explicit-`constant` spelling — and identically across runtimes. A bare
    // `5` / `true` operand auto-wraps into `{ op: "const", value }`, the same
    // node `runtime.constant(...)` emits. Each pairing asserts both halves:
    // the sugared form equals the explicit form (within a runtime), and both
    // forms agree across runtimes (`assert_parity`).
    let explicit = "runtime.clamp(runtime.add(runtime.read(\"speed\"), runtime.constant(1)), runtime.constant(0), runtime.constant(100))";
    let sugared = "runtime.clamp(runtime.add(runtime.read(\"speed\"), 1), 0, 100)";

    assert_parity(explicit);
    assert_parity(sugared);
    assert_eq!(
        quickjs_canonical(explicit),
        quickjs_canonical(sugared),
        "bare-literal sugar diverged from explicit `constant` form (QuickJS)"
    );
    assert_eq!(
        luau_canonical(explicit),
        luau_canonical(sugared),
        "bare-literal sugar diverged from explicit `constant` form (Luau)"
    );

    // A bare boolean operand wraps the same way.
    let explicit_bool =
        "runtime.select(runtime.constant(true), runtime.constant(10), runtime.constant(20))";
    let sugared_bool = "runtime.select(true, 10, 20)";
    assert_eq!(
        quickjs_canonical(explicit_bool),
        quickjs_canonical(sugared_bool),
        "bare boolean sugar diverged from explicit `constant` form (QuickJS)"
    );
    assert_eq!(
        luau_canonical(explicit_bool),
        luau_canonical(sugared_bool),
        "bare boolean sugar diverged from explicit `constant` form (Luau)"
    );
}

#[test]
fn increment_and_predicate_crossing_fixtures_match_across_authoring_runtimes() {
    // These fixtures deliberately use the public UI authoring surfaces: the
    // TypeScript fixture is the post-bundle form (its `postretro/ui` import is
    // stripped), while Luau reads that same surface from the virtual module.
    // Both author one increment reaction and one predicate crossing over the
    // same slot, exercising updateState, onStateCrossing, and literal wrapping.
    const TYPESCRIPT_FIXTURE: &str = r#"
        const ref = { slot: "counter.charge" };
        const increment = updateState(
          ref,
          runtime.add(runtime.read(ref.slot), 1),
        );
        const crossing = onStateCrossing(
          runtime.ge(runtime.read(ref.slot), 2),
          ["chargeReady"],
        );
        JSON.stringify({ increment, crossing });
    "#;
    const LUAU_FIXTURE: &str = r#"
        local Ui = require("postretro/ui")
        local ref = { slot = "counter.charge" }
        local increment = Ui.updateState(
          ref,
          runtime.add(runtime.read(ref.slot), 1)
        )
        local crossing = Ui.onStateCrossing(
          runtime.ge(runtime.read(ref.slot), 2),
          { "chargeReady" }
        )
        return { increment = increment, crossing = crossing }
    "#;

    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    assert_eq!(
        typescript, luau,
        "TS and Luau increment/predicate descriptors diverged"
    );

    let expected = serde_json::json!({
        "increment": {
            "primitive": "setState",
            "args": {
                "slot": "counter.charge",
                "value": {
                    "op": "add",
                    "a": { "op": "input", "name": "counter.charge" },
                    "b": { "op": "const", "value": 1 },
                },
            },
        },
        "crossing": {
            "predicate": {
                "op": "ge",
                "a": { "op": "input", "name": "counter.charge" },
                "b": { "op": "const", "value": 2 },
            },
            "fire": ["chargeReady"],
        },
    });
    assert_eq!(
        typescript, expected,
        "fixture must emit the pinned setState/crossing wire shapes"
    );

    for pointer in ["/increment/args/value", "/crossing/predicate"] {
        assert_eq!(
            fixture_ir_canonical(&typescript, pointer),
            fixture_ir_canonical(&luau, pointer),
            "RuntimeValue at {pointer} must be byte-identical after canonicalization"
        );
    }
}

#[test]
fn every_opcode_round_trips_identically_across_runtimes() {
    // One assertion per opcode so a divergence names the offending builder.
    let n = "runtime.constant(1)";
    let leaves = format!("{n}, {n}");
    for expr in [
        "runtime.constant(3.5)".to_string(),
        "runtime.constant(true)".to_string(),
        "runtime.read(\"speed\")".to_string(),
        format!("runtime.add({leaves})"),
        format!("runtime.sub({leaves})"),
        format!("runtime.mul({leaves})"),
        format!("runtime.div({leaves})"),
        format!("runtime.clamp({n}, {n}, {n})"),
        format!("runtime.lerp({n}, {n}, {n})"),
        format!("runtime.lt({leaves})"),
        format!("runtime.le({leaves})"),
        format!("runtime.gt({leaves})"),
        format!("runtime.ge({leaves})"),
        format!("runtime.eq({leaves})"),
        format!("runtime.ne({leaves})"),
        format!("runtime.select(runtime.constant(true), {n}, {n})"),
    ] {
        assert_parity(&expr);
    }
}
