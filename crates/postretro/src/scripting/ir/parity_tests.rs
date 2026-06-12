// Dual-runtime parity: the same behavior-IR expression authored in TypeScript
// (QuickJS) and Luau must produce byte-identical IR once canonicalized through
// the Task 1 `IrNode` serde form. See: context/lib/scripting.md §11.
//
// Crossing path: each runtime authors the expression with the `ir` builder
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
/// expression — the `ir.*` builders share an identical surface, so a string
/// using only `ir.<op>(...)` and numeric/boolean/string literals works in
/// both.
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
        "ir.clamp(ir.add(ir.input(\"base\"), ir.mul(ir.input(\"charges\"), ir.constant(10))), ir.constant(0), ir.constant(100))",
    );
}

#[test]
fn select_with_comparison_is_byte_identical_across_runtimes() {
    // select(speed > threshold, lerp(a, b, t), const) — exercises select,
    // comparison, lerp, const, and a boolean literal leaf.
    assert_parity(
        "ir.select(ir.gt(ir.input(\"speed\"), ir.constant(5)), ir.lerp(ir.constant(0), ir.constant(1), ir.input(\"t\")), ir.constant(true))",
    );
}

#[test]
fn every_opcode_round_trips_identically_across_runtimes() {
    // One assertion per opcode so a divergence names the offending builder.
    let n = "ir.constant(1)";
    let leaves = format!("{n}, {n}");
    for expr in [
        "ir.constant(3.5)".to_string(),
        "ir.constant(true)".to_string(),
        "ir.input(\"speed\")".to_string(),
        format!("ir.add({leaves})"),
        format!("ir.sub({leaves})"),
        format!("ir.mul({leaves})"),
        format!("ir.div({leaves})"),
        format!("ir.clamp({n}, {n}, {n})"),
        format!("ir.lerp({n}, {n}, {n})"),
        format!("ir.lt({leaves})"),
        format!("ir.le({leaves})"),
        format!("ir.gt({leaves})"),
        format!("ir.ge({leaves})"),
        format!("ir.eq({leaves})"),
        format!("ir.ne({leaves})"),
        format!("ir.select(ir.constant(true), {n}, {n})"),
    ] {
        assert_parity(&expr);
    }
}
