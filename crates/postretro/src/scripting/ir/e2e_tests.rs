// End-to-end composition gate for the behavior-IR substrate: it proves the
// builders, evaluator, and versioned envelope compose along the *whole*
// authored-behavior path — SDK builders author an expression in each runtime,
// the node crosses the FFI, `load_baked_ir`/`bind`/`eval_value` turn it into
// a value, and that value matches the one computed by hand from the stub
// scope's known inputs.
// See: context/lib/scripting.md §11 (Typed Command Buffer / IR substrate)
//
// This is the *composition* gate, not a re-run of the low-level units. The node
// wire round-trip lives in `mod.rs::wire_format_tests`, the version check in
// `load.rs`, bind/eval semantics in `bind.rs`/`eval.rs`. Here every test starts
// from an authored program and asserts the value that falls out the far end.
//
// Crossing path: identical to `parity_tests.rs` — each runtime authors the
// expression with the `runtime.*` builder vocabulary installed by the SDK prelude
// and returns the node through the existing `run_script` / `run_source` value
// path. QuickJS returns `JSON.stringify(node)` (deserialized with `serde_json`);
// Luau returns the node table (deserialized via the `conv`/mlua serde bridge).
// Both land in `IrNode`. We do *not* invent a new collection sink.

use mlua::LuaSerdeExt as _;

use crate::scripting::ctx::ScriptCtx;
use crate::scripting::ir::test_scope::StubScope;
use crate::scripting::ir::{
    BakedIr, CURRENT_IR_VERSION, IrNode, IrValue, bind, eval_value, load_baked_ir,
};
use crate::scripting::luau::{LuauConfig, LuauSubsystem, Which};
use crate::scripting::primitives::register_all;
use crate::scripting::primitives_registry::PrimitiveRegistry;
use crate::scripting::quickjs::{QuickJsConfig, QuickJsSubsystem, run_script};

const EPSILON: f32 = 1e-6;

/// Author `expr_src` in QuickJS as a bare expression evaluating to an IR node,
/// return it as a JSON string, and deserialize into [`IrNode`] — the same
/// crossing `parity_tests.rs` uses, kept to the node (not the canonical string)
/// so the e2e path can wrap it in an envelope.
fn author_in_quickjs(expr_src: &str) -> IrNode {
    let ctx = ScriptCtx::new();
    let mut registry = PrimitiveRegistry::new();
    register_all(&mut registry, ctx);
    let subsys = QuickJsSubsystem::new(&registry, &QuickJsConfig::default()).unwrap();

    let json = subsys.definition_ctx().with(|ctx| {
        let src = format!("JSON.stringify({expr_src})");
        run_script::<String>(&ctx, &src, "e2e.js").expect("quickjs eval")
    });

    serde_json::from_str(&json).expect("quickjs json -> IrNode")
}

/// Author `expr_src` in Luau as a bare expression evaluating to an IR node,
/// return the table, and deserialize into [`IrNode`] via the mlua serde bridge.
fn author_in_luau(expr_src: &str) -> IrNode {
    let ctx = ScriptCtx::new();
    let mut registry = PrimitiveRegistry::new();
    register_all(&mut registry, ctx);
    let subsys = LuauSubsystem::new(&registry, &LuauConfig::default()).unwrap();

    let value: mlua::Value = subsys
        .run_source(Which::Definition, &format!("return {expr_src}"), "e2e.luau")
        .expect("luau eval");

    subsys
        .definition_lua()
        .from_value(value)
        .expect("luau value -> IrNode")
}

/// Wrap a read-only (no output) root in a current-version envelope.
fn read_only(root: IrNode) -> BakedIr {
    BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root,
    }
}

/// Assert an [`IrValue`] is a number within `EPSILON` of `expected`.
fn assert_number(value: IrValue, expected: f32) {
    match value {
        IrValue::Number(actual) => assert!(
            (actual - expected).abs() <= EPSILON,
            "expected {expected}, got {actual}"
        ),
        other => panic!("expected a number, got {other:?}"),
    }
}

/// Bind a read-only root against a fresh `StubScope::new()` and evaluate it.
fn eval_against_stub(root: IrNode) -> IrValue {
    let scope = StubScope::new();
    let program = bind(&read_only(root), &scope).expect("authored program binds against stub");
    eval_value(&program, &scope)
}

// ---------------------------------------------------------------------------
// Read-path value test — builders → cross FFI → bind → eval → asserted value, BOTH runtimes.
// ---------------------------------------------------------------------------

// `clamp(speed + 1, 0, 100)`. `StubScope::new()` seeds `speed = 4.0`, so by
// hand: add(4.0, 1) = 5.0; clamp(5.0, 0, 100) = 5.0.
const CLAMP_EXPR: &str = "runtime.clamp(runtime.add(runtime.read(\"speed\"), runtime.constant(1)), runtime.constant(0), runtime.constant(100))";
const CLAMP_EXPECTED: f32 = 5.0;

// `select(speed > 5, 10, 20)`. `speed = 4.0`, so `4.0 > 5` is false and the
// `select` takes its `b` arm: the value is 20.0.
const SELECT_EXPR: &str = "runtime.select(runtime.gt(runtime.read(\"speed\"), runtime.constant(5)), runtime.constant(10), runtime.constant(20))";
const SELECT_EXPECTED: f32 = 20.0;

#[test]
fn quickjs_authored_clamp_evaluates_to_hand_computed_value() {
    let root = author_in_quickjs(CLAMP_EXPR);
    assert_number(eval_against_stub(root), CLAMP_EXPECTED);
}

#[test]
fn luau_authored_clamp_evaluates_to_hand_computed_value() {
    let root = author_in_luau(CLAMP_EXPR);
    assert_number(eval_against_stub(root), CLAMP_EXPECTED);
}

#[test]
fn quickjs_authored_select_over_comparison_evaluates_to_hand_computed_value() {
    let root = author_in_quickjs(SELECT_EXPR);
    assert_number(eval_against_stub(root), SELECT_EXPECTED);
}

#[test]
fn luau_authored_select_over_comparison_evaluates_to_hand_computed_value() {
    let root = author_in_luau(SELECT_EXPR);
    assert_number(eval_against_stub(root), SELECT_EXPECTED);
}

#[test]
fn both_runtimes_author_the_same_evaluated_value() {
    // The crossing is runtime-agnostic: the TS- and Luau-authored programs must
    // bind+eval to the same value against the same stub. (parity_tests proves
    // byte-identical IR; this proves identical *evaluated outcome* end to end.)
    for expr in [CLAMP_EXPR, SELECT_EXPR] {
        let ts = eval_against_stub(author_in_quickjs(expr));
        let luau = eval_against_stub(author_in_luau(expr));
        assert_eq!(ts, luau, "TS and Luau authored `{expr}` diverged at eval");
    }
}

// ---------------------------------------------------------------------------
// Envelope round-trip — wire-format round-trip at the composition (envelope + eval) level.
// ---------------------------------------------------------------------------

#[test]
fn authored_envelope_survives_serialize_load_bind_eval_round_trip() {
    // Author through the real builders, wrap in an envelope, serialize to the
    // wire form, `load_baked_ir` it back, then bind+eval — the value must
    // survive the full round-trip unchanged.
    let root = author_in_quickjs(CLAMP_EXPR);
    let envelope = read_only(root);

    let json = serde_json::to_string(&envelope).expect("serialize envelope");
    let loaded = load_baked_ir(&json).expect("current-version envelope loads");

    let scope = StubScope::new();
    let program = bind(&loaded, &scope).expect("loaded program binds");
    assert_number(eval_value(&program, &scope), CLAMP_EXPECTED);
}

// ---------------------------------------------------------------------------
// Version-stamp round-trip/rejection — current loads+evaluates; unsupported is
// rejected and the consumer falls back (no panic), tying load → bind → eval together.
// ---------------------------------------------------------------------------

#[test]
fn current_version_envelope_loads_and_evaluates() {
    let envelope = read_only(author_in_luau(SELECT_EXPR));
    let json = serde_json::to_string(&envelope).expect("serialize envelope");

    let loaded = load_baked_ir(&json).expect("current version loads");
    let scope = StubScope::new();
    let program = bind(&loaded, &scope).expect("binds");
    assert_number(eval_value(&program, &scope), SELECT_EXPECTED);
}

#[test]
fn unsupported_version_envelope_is_rejected_and_consumer_falls_back() {
    // An envelope stamped with an unsupported version must not load — the
    // consumer falls back to native behavior instead of binding/evaluating it.
    let mut envelope = read_only(author_in_quickjs(CLAMP_EXPR));
    envelope.version = CURRENT_IR_VERSION + 1;
    let json = serde_json::to_string(&envelope).expect("serialize envelope");

    // The composition-level fallback: a None from load means the adopter never
    // reaches bind/eval. We model that fallback explicitly and assert no panic.
    let evaluated = match load_baked_ir(&json) {
        Some(loaded) => {
            let scope = StubScope::new();
            let program = bind(&loaded, &scope).expect("binds");
            Some(eval_value(&program, &scope))
        }
        None => None, // unsupported version → fall back to native behavior
    };
    assert!(
        evaluated.is_none(),
        "an unsupported-version envelope must be rejected at load, never evaluated"
    );
}

// ---------------------------------------------------------------------------
// Write-path end-to-end: author a read of `speed`, bind an envelope with an
// output, `eval_and_write`, and assert the stub captured the written value.
// ---------------------------------------------------------------------------

#[test]
fn authored_program_with_output_writes_evaluated_value_to_stub() {
    use crate::scripting::ir::eval_and_write;
    use crate::scripting::ir::test_scope::StubWrite;

    // `add(speed, 1)` over `speed = 4.0` → 5.0, written to the granted output.
    let root = author_in_quickjs("runtime.add(runtime.read(\"speed\"), runtime.constant(1))");
    let envelope = BakedIr {
        version: CURRENT_IR_VERSION,
        output: Some("out_speed".to_string()),
        root,
    };

    let json = serde_json::to_string(&envelope).expect("serialize envelope");
    let loaded = load_baked_ir(&json).expect("loads");

    let mut scope = StubScope::with_writes(&[("out_speed", StubWrite::Number)]);
    let program = bind(&loaded, &scope).expect("output binds to granted write handle");
    let value = eval_and_write(&program, &mut scope);

    assert_number(value, 5.0);
    assert_number(scope.written("out_speed").expect("output written"), 5.0);
}
