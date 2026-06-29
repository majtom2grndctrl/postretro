// The eval pass: a pure, total, bounded, allocation-free walk over a
// `BoundProgram`, plus the optional write-back step.
// See: context/lib/scripting.md §11 (Typed Command Buffer / IR substrate)

// Eval is the per-tick phase of the two-phase evaluator. Bind already
// type-checked the tree and resolved every leaf to a scope handle, so eval
// makes no allocations and never fails: it recurses the bound tree, reads
// `Input` leaves through the scope, and folds each op to an `IrValue`.
//
// Totality is pinned (see the module-level test suite and scripting.md §11):
//   - a missing-value read returns the scope's type-zero (the scope maps
//     `None → 0.0 / false`; eval trusts the value it receives);
//   - `div` by zero yields `0.0`;
//   - any arithmetic result that is non-finite (`NaN`/`±Inf`) coerces to `0.0`
//     via a per-node finite guard;
//   - `clamp(x, lo, hi) = min(max(x, lo), hi)` — total for any bounds; inverted
//     bounds (`lo > hi`) naturally yield the `hi` operand;
//   - `lerp(a, b, t) = a + (b - a) * t`, then finite-guarded.
//
// No per-tick logging: numeric edge cases are absorbed silently per the
// semantics above. The only fallible phase is bind, which runs once.

use super::IrValue;
use super::bind::{BoundNode, BoundProgram};
use super::scope::BindingScope;

/// Evaluate a bound program's root and return its value, reading inputs through
/// `scope`. Pure and read-only — it never writes back, even when the program
/// carries an output. Allocation-free: the recursive walk touches only the
/// already-allocated bound tree and stack-resident `IrValue`s.
pub fn eval_value<S: BindingScope>(program: &BoundProgram<S>, scope: &S) -> IrValue {
    eval_node(&program.root, scope)
}

/// Evaluate a bound program and, if it carries a write handle, write the root's
/// value back through `scope`. Returns the evaluated value either way.
///
/// The value-computing core ([`eval_value`]) runs against `&*scope` and is
/// allocation-free; the write step needs `&mut scope` and runs only when an
/// output is present.
pub fn eval_and_write<S: BindingScope>(program: &BoundProgram<S>, scope: &mut S) -> IrValue {
    let value = eval_value(program, scope);
    if let Some(handle) = &program.output {
        scope.write(handle, value);
    }
    value
}

/// Recursively evaluate one bound node. Total and allocation-free: every arm
/// folds to a stack `IrValue`, reading `Input` leaves through the scope.
fn eval_node<S: BindingScope>(node: &BoundNode<S::InputHandle>, scope: &S) -> IrValue {
    match node {
        BoundNode::Const(value) => *value,
        BoundNode::Input(handle) => scope.read(handle),

        BoundNode::Add(a, b) => arith(eval_num(a, scope) + eval_num(b, scope)),
        BoundNode::Sub(a, b) => arith(eval_num(a, scope) - eval_num(b, scope)),
        BoundNode::Mul(a, b) => arith(eval_num(a, scope) * eval_num(b, scope)),
        BoundNode::Div(a, b) => {
            let denom = eval_num(b, scope);
            // Division by zero is total: yield 0.0 rather than producing an
            // infinity/NaN the finite guard would have to scrub anyway.
            if denom == 0.0 {
                IrValue::Number(0.0)
            } else {
                arith(eval_num(a, scope) / denom)
            }
        }

        BoundNode::Clamp { x, lo, hi } => {
            let x = eval_num(x, scope);
            let lo = eval_num(lo, scope);
            let hi = eval_num(hi, scope);
            // min(max(x, lo), hi): total for any bounds. Inverted bounds
            // (lo > hi) collapse to the `hi` operand, which is the pinned
            // semantics. f32::min/max also propagate the non-NaN operand, so a
            // NaN bound does not poison the result; the finite guard catches a
            // NaN `x` that survives both clamps.
            arith(x.max(lo).min(hi))
        }
        BoundNode::Lerp { a, b, t } => {
            let a = eval_num(a, scope);
            let b = eval_num(b, scope);
            let t = eval_num(t, scope);
            arith(a + (b - a) * t)
        }

        BoundNode::Lt(a, b) => IrValue::Bool(eval_num(a, scope) < eval_num(b, scope)),
        BoundNode::Le(a, b) => IrValue::Bool(eval_num(a, scope) <= eval_num(b, scope)),
        BoundNode::Gt(a, b) => IrValue::Bool(eval_num(a, scope) > eval_num(b, scope)),
        BoundNode::Ge(a, b) => IrValue::Bool(eval_num(a, scope) >= eval_num(b, scope)),

        BoundNode::Eq(a, b) => {
            IrValue::Bool(values_equal(eval_node(a, scope), eval_node(b, scope)))
        }
        BoundNode::Ne(a, b) => {
            IrValue::Bool(!values_equal(eval_node(a, scope), eval_node(b, scope)))
        }

        BoundNode::Select { cond, a, b } => {
            if eval_bool(cond, scope) {
                eval_node(a, scope)
            } else {
                eval_node(b, scope)
            }
        }
    }
}

/// Evaluate a node bind proved to be `Number` and extract the `f32`. Bind
/// guarantees the projection, so a `Bool` here is a bind bug; eval stays total
/// by treating it as type-zero rather than panicking.
#[inline]
fn eval_num<S: BindingScope>(node: &BoundNode<S::InputHandle>, scope: &S) -> f32 {
    match eval_node(node, scope) {
        IrValue::Number(value) => value,
        IrValue::Bool(_) => 0.0,
    }
}

/// Evaluate a node bind proved to be `Bool` and extract the `bool`. As with
/// [`eval_num`], a type surprise degrades to `false` rather than panicking.
#[inline]
fn eval_bool<S: BindingScope>(node: &BoundNode<S::InputHandle>, scope: &S) -> bool {
    match eval_node(node, scope) {
        IrValue::Bool(value) => value,
        IrValue::Number(_) => false,
    }
}

/// Per-node finite guard: coerce a non-finite arithmetic result (`NaN`/`±Inf`)
/// to `0.0`, keeping eval total. Applied at every arithmetic/clamp/lerp node.
#[inline]
fn arith(value: f32) -> IrValue {
    IrValue::Number(if value.is_finite() { value } else { 0.0 })
}

/// Equality over same-typed operands (bind guarantees the operands share a
/// type). Number compare uses exact `f32` equality, which is the pinned
/// semantics for `eq`/`ne`. A cross-type pair cannot arise post-bind; it folds
/// to `false`.
#[inline]
fn values_equal(left: IrValue, right: IrValue) -> bool {
    match (left, right) {
        (IrValue::Number(a), IrValue::Number(b)) => a == b,
        (IrValue::Bool(a), IrValue::Bool(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::test_scope::{StubScope, StubWrite};
    use crate::ir::{BakedIr, CURRENT_IR_VERSION, IrNode, bind};

    const EPSILON: f32 = 1e-6;

    fn num(v: f32) -> Box<IrNode> {
        Box::new(IrNode::Const {
            value: IrValue::Number(v),
        })
    }

    fn boolean(v: bool) -> Box<IrNode> {
        Box::new(IrNode::Const {
            value: IrValue::Bool(v),
        })
    }

    fn input(name: &str) -> Box<IrNode> {
        Box::new(IrNode::Input {
            name: name.to_string(),
        })
    }

    fn read_only(root: IrNode) -> BakedIr {
        BakedIr {
            version: CURRENT_IR_VERSION,
            output: None,
            root,
        }
    }

    fn assert_number(value: IrValue, expected: f32) {
        match value {
            IrValue::Number(actual) => assert!(
                (actual - expected).abs() <= EPSILON,
                "expected {expected}, got {actual}"
            ),
            other => panic!("expected a number, got {other:?}"),
        }
    }

    /// Evaluate a read-only root against a fresh stub scope and return its value.
    fn eval_root(root: IrNode) -> IrValue {
        let scope = StubScope::new();
        let program = bind(&read_only(root), &scope).expect("well-typed program binds");
        eval_value(&program, &scope)
    }

    #[test]
    fn missing_input_reads_as_type_zero() {
        // `unset_number` is a declared stub input whose current value is None;
        // the scope maps it to 0.0. `unset_flag` maps to false.
        assert_number(eval_root(*input("unset_number")), 0.0);
        assert_eq!(eval_root(*input("unset_flag")), IrValue::Bool(false));
    }

    #[test]
    fn div_by_zero_is_zero() {
        assert_number(
            eval_root(IrNode::Div {
                a: num(7.0),
                b: num(0.0),
            }),
            0.0,
        );
    }

    #[test]
    fn non_finite_arithmetic_coerces_to_zero() {
        // 1e30 * 1e30 overflows f32 to +Inf; the finite guard scrubs it to 0.0.
        assert_number(
            eval_root(IrNode::Mul {
                a: num(1e30),
                b: num(1e30),
            }),
            0.0,
        );
        // f32::MAX - (-f32::MAX) overflows to +Inf; coerced to 0.0.
        assert_number(
            eval_root(IrNode::Sub {
                a: num(f32::MAX),
                b: num(-f32::MAX),
            }),
            0.0,
        );
    }

    #[test]
    fn clamp_is_total_with_normal_bounds() {
        assert_number(
            eval_root(IrNode::Clamp {
                x: num(5.0),
                lo: num(0.0),
                hi: num(1.0),
            }),
            1.0,
        );
        assert_number(
            eval_root(IrNode::Clamp {
                x: num(-3.0),
                lo: num(0.0),
                hi: num(1.0),
            }),
            0.0,
        );
        assert_number(
            eval_root(IrNode::Clamp {
                x: num(0.5),
                lo: num(0.0),
                hi: num(1.0),
            }),
            0.5,
        );
    }

    #[test]
    fn inverted_clamp_returns_hi_operand() {
        // lo (10) > hi (2): min(max(x, 10), 2) == 2 for any x, the `hi` operand.
        assert_number(
            eval_root(IrNode::Clamp {
                x: num(5.0),
                lo: num(10.0),
                hi: num(2.0),
            }),
            2.0,
        );
        assert_number(
            eval_root(IrNode::Clamp {
                x: num(50.0),
                lo: num(10.0),
                hi: num(2.0),
            }),
            2.0,
        );
    }

    #[test]
    fn lerp_interpolates_and_finite_guards() {
        assert_number(
            eval_root(IrNode::Lerp {
                a: num(0.0),
                b: num(10.0),
                t: num(0.5),
            }),
            5.0,
        );
        assert_number(
            eval_root(IrNode::Lerp {
                a: num(2.0),
                b: num(4.0),
                t: num(0.0),
            }),
            2.0,
        );
        // A non-finite blend term coerces to 0.0 rather than escaping as Inf.
        assert_number(
            eval_root(IrNode::Lerp {
                a: num(0.0),
                b: num(f32::MAX),
                t: num(f32::MAX),
            }),
            0.0,
        );
    }

    #[test]
    fn comparisons_and_select_route_by_bool() {
        assert_eq!(
            eval_root(IrNode::Lt {
                a: num(1.0),
                b: num(2.0)
            }),
            IrValue::Bool(true)
        );
        assert_eq!(
            eval_root(IrNode::Ge {
                a: num(2.0),
                b: num(2.0)
            }),
            IrValue::Bool(true)
        );
        assert_number(
            eval_root(IrNode::Select {
                cond: boolean(true),
                a: num(1.0),
                b: num(2.0),
            }),
            1.0,
        );
        assert_number(
            eval_root(IrNode::Select {
                cond: boolean(false),
                a: num(1.0),
                b: num(2.0),
            }),
            2.0,
        );
    }

    #[test]
    fn eq_and_ne_compare_same_typed_operands() {
        assert_eq!(
            eval_root(IrNode::Eq {
                a: num(2.0),
                b: num(2.0)
            }),
            IrValue::Bool(true)
        );
        assert_eq!(
            eval_root(IrNode::Ne {
                a: num(2.0),
                b: num(3.0)
            }),
            IrValue::Bool(true)
        );
        assert_eq!(
            eval_root(IrNode::Eq {
                a: boolean(true),
                b: boolean(true),
            }),
            IrValue::Bool(true)
        );
    }

    #[test]
    fn eval_and_write_pushes_root_value_to_output() {
        // A stub scope with a writable number output; the root computes 3 + 4
        // and eval_and_write stores 7 into the output.
        let mut scope = StubScope::with_writes(&[("out_number", StubWrite::Number)]);
        let baked = BakedIr {
            version: CURRENT_IR_VERSION,
            output: Some("out_number".to_string()),
            root: IrNode::Add {
                a: num(3.0),
                b: num(4.0),
            },
        };
        let program = bind(&baked, &scope).expect("number root matches number output");
        let value = eval_and_write(&program, &mut scope);
        assert_number(value, 7.0);
        assert_number(scope.written("out_number").expect("output written"), 7.0);
    }

    #[test]
    fn read_only_program_does_not_write() {
        let mut scope = StubScope::with_writes(&[("out_number", StubWrite::Number)]);
        let program = bind(&read_only(*num(9.0)), &scope).expect("binds");
        eval_and_write(&program, &mut scope);
        assert!(
            scope.written("out_number").is_none(),
            "a read-only program must not write any output"
        );
    }
}
