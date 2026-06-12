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

use crate::scripting::ctx::ScriptCtx;
use crate::scripting::ir::IrNode;
use crate::scripting::luau::{LuauConfig, LuauSubsystem, Which};
use crate::scripting::primitives::register_all;
use crate::scripting::primitives_registry::PrimitiveRegistry;
use crate::scripting::quickjs::{QuickJsConfig, QuickJsSubsystem, run_script};

/// Author `expr_src` in QuickJS as a bare expression evaluating to an IR node,
/// return it as a JSON string, and canonicalize through `IrNode`.
fn quickjs_canonical(expr_src: &str) -> String {
    let ctx = ScriptCtx::new();
    let mut registry = PrimitiveRegistry::new();
    register_all(&mut registry, ctx);
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
    let ctx = ScriptCtx::new();
    let mut registry = PrimitiveRegistry::new();
    register_all(&mut registry, ctx);
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
