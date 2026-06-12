// The bind pass: type-checks an IR tree once and resolves named leaves to scope
// handles, producing an eval-ready `BoundProgram`.
// See: context/lib/scripting.md §11 (Typed Command Buffer / IR substrate)

// Bind is the once-per-program phase of the two-phase evaluator. It walks the
// `IrNode` tree against the static type table (the rows in §11), resolves every
// `Input` name and the envelope's optional `output` through the `BindingScope`
// seam, and verifies the root's result type matches the output's projection.
// It returns a `BoundProgram` whose leaves hold resolved handles, not strings —
// the form the eval pass (`eval.rs`) walks allocation-free. Every fault is a typed
// `BindError`; bind never panics. Logging the error once is the caller's job.

use thiserror::Error;

use super::scope::BindingScope;
use super::{BakedIr, IrNode, IrType, IrValue};

/// A fault surfaced by [`bind`]. Each variant is matchable so an adopter can
/// distinguish a missing name from a type error. Structurally-malformed *JSON*
/// is a serde error upstream of bind; bind's faults are type-table and
/// name-resolution violations on an already-deserialized tree.
#[derive(Debug, Error, PartialEq)]
pub(crate) enum BindError {
    /// An `input` leaf named a source the scope did not resolve — either truly
    /// unknown, or backed by a non-projectable slot (`String`/`Enum`/`Array`).
    /// The scope collapses both into a `None`; bind cannot tell them apart.
    #[error("unknown or non-projectable input `{name}`")]
    UnknownInput { name: String },

    /// The envelope's `output` named a target the scope refused to grant a
    /// write handle for — unknown, non-projectable, or not writable by this
    /// scope (a readonly/forbidden output).
    #[error("unknown, forbidden, or non-projectable output `{name}`")]
    UnknownOutput { name: String },

    /// A node operand did not have the type the opcode requires. `context`
    /// names the offending position (e.g. `add.a`, `select.cond`).
    #[error("type mismatch at {context}: expected {expected}, found {found}")]
    TypeMismatch {
        context: &'static str,
        expected: &'static str,
        found: &'static str,
    },

    /// A `select`/`eq`/`ne` required both arms to share a type but they
    /// differed.
    #[error("type mismatch at {context}: operands must share a type, found {left} and {right}")]
    OperandTypeDisagreement {
        context: &'static str,
        left: &'static str,
        right: &'static str,
    },

    /// An `output` was present but the root's result type did not match the
    /// output slot's projected type.
    #[error("root type {root} does not match output `{output}` of type {output_type}")]
    OutputTypeMismatch {
        output: String,
        output_type: &'static str,
        root: &'static str,
    },
}

/// A bound IR node: structurally mirrors [`IrNode`] but every `Input` leaf
/// carries a resolved scope handle instead of a name, and the tree is known to
/// type-check. This is the form eval walks — a bound tree, not a flattened vec.
///
/// The bound tree is chosen over a flattened instruction vector because it
/// mirrors `IrNode` one-to-one (each adopter that already reasons about the
/// node tree reads the bound form with no mental remap) and a recursive eval
/// walk over `Box` children is allocation-free at tick time — the boxes are
/// allocated once at bind. `H` is the scope's `InputHandle` type.
///
/// `Clone` is derived so an adopter can hold a bound program inside a component
/// that flows through `Clone`/`PartialEq`/serde container derives (the dash
/// `DashPrograms` case). The derive adds an `H: Clone` bound, satisfied by every
/// real scope's handle (`usize` for `MovementScope`).
#[derive(Debug, Clone)]
pub(crate) enum BoundNode<H> {
    Const(IrValue),
    Input(H),

    Add(Box<BoundNode<H>>, Box<BoundNode<H>>),
    Sub(Box<BoundNode<H>>, Box<BoundNode<H>>),
    Mul(Box<BoundNode<H>>, Box<BoundNode<H>>),
    Div(Box<BoundNode<H>>, Box<BoundNode<H>>),

    Clamp {
        x: Box<BoundNode<H>>,
        lo: Box<BoundNode<H>>,
        hi: Box<BoundNode<H>>,
    },
    Lerp {
        a: Box<BoundNode<H>>,
        b: Box<BoundNode<H>>,
        t: Box<BoundNode<H>>,
    },

    Lt(Box<BoundNode<H>>, Box<BoundNode<H>>),
    Le(Box<BoundNode<H>>, Box<BoundNode<H>>),
    Gt(Box<BoundNode<H>>, Box<BoundNode<H>>),
    Ge(Box<BoundNode<H>>, Box<BoundNode<H>>),

    Eq(Box<BoundNode<H>>, Box<BoundNode<H>>),
    Ne(Box<BoundNode<H>>, Box<BoundNode<H>>),

    Select {
        cond: Box<BoundNode<H>>,
        a: Box<BoundNode<H>>,
        b: Box<BoundNode<H>>,
    },
}

/// An eval-ready bound program: a type-checked bound tree, the root's static
/// result type, and the optional resolved output write handle.
///
/// `output` is `Some` exactly when the envelope carried an `output` the scope
/// granted; eval writes the root's value through it. `None` ⇒ a read-only
/// (value-producing) buffer whose result the adopter reads directly.
///
/// Generic over the scope so it carries the scope's concrete handle types. The
/// eval pass (`eval.rs`) walks `root`, reads inputs via the scope, and — when
/// `output` is `Some` — writes the result back through the same scope.
pub(crate) struct BoundProgram<S: BindingScope> {
    pub(crate) root: BoundNode<S::InputHandle>,
    pub(crate) root_type: IrType,
    pub(crate) output: Option<S::OutputHandle>,
}

// Manual `Clone` bounded on the handle types only, mirroring the manual `Debug`
// below. A `#[derive(Clone)]` would add a spurious `S: Clone` bound (the struct
// holds no `S`, only its associated handle types), forcing every concrete scope
// to be `Clone` just to clone a program. Bounding on the handle types instead
// keeps the requirement where it belongs.
impl<S: BindingScope> Clone for BoundProgram<S>
where
    S::InputHandle: Clone,
    S::OutputHandle: Clone,
{
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            root_type: self.root_type,
            output: self.output.clone(),
        }
    }
}

// Manual `Debug` bounded on the handle types only. The `#[derive(Debug)]` macro
// would add a spurious `S: Debug` bound (the struct holds no `S`, only its
// associated handle types), forcing every concrete scope — e.g. the store
// adapter wrapping a `ScriptCtx` — to be `Debug` just to debug-print a program.
impl<S: BindingScope> std::fmt::Debug for BoundProgram<S>
where
    S::InputHandle: std::fmt::Debug,
    S::OutputHandle: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundProgram")
            .field("root", &self.root)
            .field("root_type", &self.root_type)
            .field("output", &self.output)
            .finish()
    }
}

/// Type-check `baked`'s tree against the static type table and resolve every
/// named leaf through `scope`, producing an eval-ready [`BoundProgram`].
///
/// Returns a typed [`BindError`] on the first structural / type / name /
/// projection fault. Bind never panics. The version check on `baked.version`
/// is performed at load by `load::load_baked_ir`, not here — bind receives only
/// already-validated `BakedIr` values.
pub(crate) fn bind<S: BindingScope>(
    baked: &BakedIr,
    scope: &S,
) -> Result<BoundProgram<S>, BindError> {
    let (root, root_type) = bind_node(&baked.root, scope)?;

    let output = match &baked.output {
        None => None,
        Some(name) => {
            let resolved = scope
                .resolve_output(name)
                .ok_or_else(|| BindError::UnknownOutput { name: name.clone() })?;
            if resolved.ir_type != root_type {
                return Err(BindError::OutputTypeMismatch {
                    output: name.clone(),
                    output_type: type_name(resolved.ir_type),
                    root: type_name(root_type),
                });
            }
            Some(resolved.handle)
        }
    };

    Ok(BoundProgram {
        root,
        root_type,
        output,
    })
}

/// Recursively bind one node, returning the bound node and its result type.
fn bind_node<S: BindingScope>(
    node: &IrNode,
    scope: &S,
) -> Result<(BoundNode<S::InputHandle>, IrType), BindError> {
    match node {
        IrNode::Const { value } => Ok((BoundNode::Const(*value), value.ir_type())),

        IrNode::Input { name } => {
            let resolved = scope
                .resolve_input(name)
                .ok_or_else(|| BindError::UnknownInput { name: name.clone() })?;
            Ok((BoundNode::Input(resolved.handle), resolved.ir_type))
        }

        IrNode::Add { a, b } => bind_arithmetic(scope, a, b, "add", BoundNode::Add),
        IrNode::Sub { a, b } => bind_arithmetic(scope, a, b, "sub", BoundNode::Sub),
        IrNode::Mul { a, b } => bind_arithmetic(scope, a, b, "mul", BoundNode::Mul),
        IrNode::Div { a, b } => bind_arithmetic(scope, a, b, "div", BoundNode::Div),

        IrNode::Clamp { x, lo, hi } => {
            let x = bind_expect(scope, x, IrType::Number, "clamp.x")?;
            let lo = bind_expect(scope, lo, IrType::Number, "clamp.lo")?;
            let hi = bind_expect(scope, hi, IrType::Number, "clamp.hi")?;
            Ok((
                BoundNode::Clamp {
                    x: Box::new(x),
                    lo: Box::new(lo),
                    hi: Box::new(hi),
                },
                IrType::Number,
            ))
        }
        IrNode::Lerp { a, b, t } => {
            let a = bind_expect(scope, a, IrType::Number, "lerp.a")?;
            let b = bind_expect(scope, b, IrType::Number, "lerp.b")?;
            let t = bind_expect(scope, t, IrType::Number, "lerp.t")?;
            Ok((
                BoundNode::Lerp {
                    a: Box::new(a),
                    b: Box::new(b),
                    t: Box::new(t),
                },
                IrType::Number,
            ))
        }

        IrNode::Lt { a, b } => bind_comparison(scope, a, b, "lt", BoundNode::Lt),
        IrNode::Le { a, b } => bind_comparison(scope, a, b, "le", BoundNode::Le),
        IrNode::Gt { a, b } => bind_comparison(scope, a, b, "gt", BoundNode::Gt),
        IrNode::Ge { a, b } => bind_comparison(scope, a, b, "ge", BoundNode::Ge),

        IrNode::Eq { a, b } => bind_equality(scope, a, b, "eq", BoundNode::Eq),
        IrNode::Ne { a, b } => bind_equality(scope, a, b, "ne", BoundNode::Ne),

        IrNode::Select { cond, a, b } => {
            let cond = bind_expect(scope, cond, IrType::Bool, "select.cond")?;
            let (a, a_ty) = bind_node(a, scope)?;
            let (b, b_ty) = bind_node(b, scope)?;
            if a_ty != b_ty {
                return Err(BindError::OperandTypeDisagreement {
                    context: "select",
                    left: type_name(a_ty),
                    right: type_name(b_ty),
                });
            }
            Ok((
                BoundNode::Select {
                    cond: Box::new(cond),
                    a: Box::new(a),
                    b: Box::new(b),
                },
                a_ty,
            ))
        }
    }
}

/// A bound-node constructor for a two-operand opcode (`Add`, `Lt`, `Eq`, …):
/// takes the two bound child boxes and returns the parent bound node. Aliased so
/// the `bind_*` helper signatures stay readable.
type BinaryCtor<H> = fn(Box<BoundNode<H>>, Box<BoundNode<H>>) -> BoundNode<H>;

/// Bind a binary arithmetic op: both operands number, result number.
fn bind_arithmetic<S: BindingScope>(
    scope: &S,
    a: &IrNode,
    b: &IrNode,
    op: &'static str,
    build: BinaryCtor<S::InputHandle>,
) -> Result<(BoundNode<S::InputHandle>, IrType), BindError> {
    let a = bind_expect(scope, a, IrType::Number, operand_context(op, 'a'))?;
    let b = bind_expect(scope, b, IrType::Number, operand_context(op, 'b'))?;
    Ok((build(Box::new(a), Box::new(b)), IrType::Number))
}

/// Bind an ordered comparison: both operands number, result boolean.
fn bind_comparison<S: BindingScope>(
    scope: &S,
    a: &IrNode,
    b: &IrNode,
    op: &'static str,
    build: BinaryCtor<S::InputHandle>,
) -> Result<(BoundNode<S::InputHandle>, IrType), BindError> {
    let a = bind_expect(scope, a, IrType::Number, operand_context(op, 'a'))?;
    let b = bind_expect(scope, b, IrType::Number, operand_context(op, 'b'))?;
    Ok((build(Box::new(a), Box::new(b)), IrType::Bool))
}

/// Bind an equality op: both operands the same type T, result boolean.
fn bind_equality<S: BindingScope>(
    scope: &S,
    a: &IrNode,
    b: &IrNode,
    op: &'static str,
    build: BinaryCtor<S::InputHandle>,
) -> Result<(BoundNode<S::InputHandle>, IrType), BindError> {
    let (a, a_ty) = bind_node(a, scope)?;
    let (b, b_ty) = bind_node(b, scope)?;
    if a_ty != b_ty {
        return Err(BindError::OperandTypeDisagreement {
            context: op,
            left: type_name(a_ty),
            right: type_name(b_ty),
        });
    }
    Ok((build(Box::new(a), Box::new(b)), IrType::Bool))
}

/// Bind a child and require it to have `expected` type, else a `TypeMismatch`
/// tagged with `context`.
fn bind_expect<S: BindingScope>(
    scope: &S,
    node: &IrNode,
    expected: IrType,
    context: &'static str,
) -> Result<BoundNode<S::InputHandle>, BindError> {
    let (bound, ty) = bind_node(node, scope)?;
    if ty != expected {
        return Err(BindError::TypeMismatch {
            context,
            expected: type_name(expected),
            found: type_name(ty),
        });
    }
    Ok(bound)
}

/// Static `op.operand` context strings for arithmetic/comparison operands.
/// Returns a `&'static str` so it can flow into [`BindError::TypeMismatch`]
/// without allocating per fault.
fn operand_context(op: &'static str, operand: char) -> &'static str {
    match (op, operand) {
        ("add", 'a') => "add.a",
        ("add", 'b') => "add.b",
        ("sub", 'a') => "sub.a",
        ("sub", 'b') => "sub.b",
        ("mul", 'a') => "mul.a",
        ("mul", 'b') => "mul.b",
        ("div", 'a') => "div.a",
        ("div", 'b') => "div.b",
        ("lt", 'a') => "lt.a",
        ("lt", 'b') => "lt.b",
        ("le", 'a') => "le.a",
        ("le", 'b') => "le.b",
        ("gt", 'a') => "gt.a",
        ("gt", 'b') => "gt.b",
        ("ge", 'a') => "ge.a",
        ("ge", 'b') => "ge.b",
        // Unreachable for the fixed op/operand set above; a generic label keeps
        // the function total without an allocation.
        _ => "operand",
    }
}

fn type_name(ty: IrType) -> &'static str {
    match ty {
        IrType::Number => "number",
        IrType::Bool => "boolean",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ir::scope::{ResolvedInput, ResolvedOutput};
    use std::collections::HashMap;

    /// Minimal stub scope for bind tests. Inputs and outputs are a fixed set of
    /// named (type) entries; non-projectable names are simply absent (the real
    /// store scope folds `String`/`Enum`/`Array` slots into the same `None`).
    /// Handles are owned names so a test can assert which leaf bound to what.
    /// The concrete store adapter lives in `scopes.rs`; this stub stays local.
    struct StubScope {
        inputs: HashMap<&'static str, IrType>,
        outputs: HashMap<&'static str, IrType>,
    }

    impl StubScope {
        fn new() -> Self {
            let mut inputs = HashMap::new();
            inputs.insert("speed", IrType::Number);
            inputs.insert("grounded", IrType::Bool);
            // A non-projectable slot (e.g. a String store slot) is modeled as
            // absent: the real scope returns None for it, which bind reports as
            // UnknownInput. `mode` stands in for that case (see the test that
            // references it — it is intentionally NOT inserted).
            let mut outputs = HashMap::new();
            outputs.insert("player.shield", IrType::Number);
            outputs.insert("flag", IrType::Bool);
            Self { inputs, outputs }
        }
    }

    impl BindingScope for StubScope {
        type InputHandle = String;
        type OutputHandle = String;

        fn resolve_input(&self, name: &str) -> Option<ResolvedInput<String>> {
            self.inputs.get(name).map(|&ir_type| ResolvedInput {
                handle: name.to_string(),
                ir_type,
            })
        }

        fn resolve_output(&self, name: &str) -> Option<ResolvedOutput<String>> {
            self.outputs.get(name).map(|&ir_type| ResolvedOutput {
                handle: name.to_string(),
                ir_type,
            })
        }

        fn read(&self, _handle: &String) -> IrValue {
            // Eval lives in `eval.rs`; bind never calls this.
            IrValue::Number(0.0)
        }

        fn write(&mut self, _handle: &String, _value: IrValue) {}
    }

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

    fn read_only(root: IrNode) -> BakedIr {
        BakedIr {
            version: super::super::CURRENT_IR_VERSION,
            output: None,
            root,
        }
    }

    #[test]
    fn bind_resolves_input_handle_and_projects_its_type() {
        let scope = StubScope::new();
        let baked = read_only(IrNode::Input {
            name: "speed".to_string(),
        });
        let program = bind(&baked, &scope).expect("well-typed program binds");
        assert_eq!(program.root_type, IrType::Number);
        assert!(program.output.is_none(), "no output ⇒ read-only buffer");
        match program.root {
            BoundNode::Input(handle) => assert_eq!(handle, "speed"),
            other => panic!("expected bound Input leaf, got {other:?}"),
        }
    }

    #[test]
    fn bind_resolves_present_output_and_write_handle() {
        let scope = StubScope::new();
        let baked = BakedIr {
            version: super::super::CURRENT_IR_VERSION,
            output: Some("player.shield".to_string()),
            root: IrNode::Add {
                a: num(1.0),
                b: Box::new(IrNode::Input {
                    name: "speed".to_string(),
                }),
            },
        };
        let program = bind(&baked, &scope).expect("number root matches number output");
        assert_eq!(program.root_type, IrType::Number);
        assert_eq!(program.output.as_deref(), Some("player.shield"));
    }

    #[test]
    fn bind_rejects_unknown_input_name_without_panicking() {
        let scope = StubScope::new();
        let baked = read_only(IrNode::Input {
            name: "missing".to_string(),
        });
        assert_eq!(
            bind(&baked, &scope).unwrap_err(),
            BindError::UnknownInput {
                name: "missing".to_string()
            }
        );
    }

    #[test]
    fn bind_rejects_input_bound_to_non_projectable_slot() {
        // `mode` stands in for a String/Enum/Array store slot: the scope
        // returns None for it (absent from the projectable set), so bind reports
        // it as an unknown input rather than binding a non-projectable type.
        let scope = StubScope::new();
        let baked = read_only(IrNode::Input {
            name: "mode".to_string(),
        });
        assert_eq!(
            bind(&baked, &scope).unwrap_err(),
            BindError::UnknownInput {
                name: "mode".to_string()
            }
        );
    }

    #[test]
    fn bind_rejects_clamp_over_boolean_operand() {
        // Type-system row violation: clamp requires number operands.
        let scope = StubScope::new();
        let baked = read_only(IrNode::Clamp {
            x: boolean(true),
            lo: num(0.0),
            hi: num(1.0),
        });
        assert_eq!(
            bind(&baked, &scope).unwrap_err(),
            BindError::TypeMismatch {
                context: "clamp.x",
                expected: "number",
                found: "boolean",
            }
        );
    }

    #[test]
    fn bind_rejects_numeric_select_cond() {
        // select's cond must be boolean.
        let scope = StubScope::new();
        let baked = read_only(IrNode::Select {
            cond: num(1.0),
            a: num(1.0),
            b: num(2.0),
        });
        assert_eq!(
            bind(&baked, &scope).unwrap_err(),
            BindError::TypeMismatch {
                context: "select.cond",
                expected: "boolean",
                found: "number",
            }
        );
    }

    #[test]
    fn bind_rejects_select_arms_of_disagreeing_type() {
        let scope = StubScope::new();
        let baked = read_only(IrNode::Select {
            cond: boolean(true),
            a: num(1.0),
            b: boolean(false),
        });
        assert_eq!(
            bind(&baked, &scope).unwrap_err(),
            BindError::OperandTypeDisagreement {
                context: "select",
                left: "number",
                right: "boolean",
            }
        );
    }

    #[test]
    fn bind_rejects_eq_arms_of_disagreeing_type() {
        let scope = StubScope::new();
        let baked = read_only(IrNode::Eq {
            a: num(1.0),
            b: boolean(true),
        });
        assert_eq!(
            bind(&baked, &scope).unwrap_err(),
            BindError::OperandTypeDisagreement {
                context: "eq",
                left: "number",
                right: "boolean",
            }
        );
    }

    #[test]
    fn bind_rejects_output_type_mismatch() {
        // Boolean root cannot write a number-projected output.
        let scope = StubScope::new();
        let baked = BakedIr {
            version: super::super::CURRENT_IR_VERSION,
            output: Some("player.shield".to_string()),
            root: IrNode::Lt {
                a: num(1.0),
                b: num(2.0),
            },
        };
        assert_eq!(
            bind(&baked, &scope).unwrap_err(),
            BindError::OutputTypeMismatch {
                output: "player.shield".to_string(),
                output_type: "number",
                root: "boolean",
            }
        );
    }

    #[test]
    fn bind_rejects_unknown_output_name() {
        let scope = StubScope::new();
        let baked = BakedIr {
            version: super::super::CURRENT_IR_VERSION,
            output: Some("nope".to_string()),
            root: num_node(1.0),
        };
        assert_eq!(
            bind(&baked, &scope).unwrap_err(),
            BindError::UnknownOutput {
                name: "nope".to_string()
            }
        );
    }

    #[test]
    fn bind_accepts_well_typed_nested_tree() {
        // clamp(lerp(0, speed, 0.5), 0, 10) > const, fed into select — exercises
        // every operand-arity path in one bind.
        let scope = StubScope::new();
        let root = IrNode::Select {
            cond: Box::new(IrNode::Gt {
                a: Box::new(IrNode::Clamp {
                    x: Box::new(IrNode::Lerp {
                        a: num(0.0),
                        b: Box::new(IrNode::Input {
                            name: "speed".to_string(),
                        }),
                        t: num(0.5),
                    }),
                    lo: num(0.0),
                    hi: num(10.0),
                }),
                b: num(5.0),
            }),
            a: num(1.0),
            b: num(0.0),
        };
        let program = bind(&read_only(root), &scope).expect("nested tree binds");
        assert_eq!(program.root_type, IrType::Number);
    }

    fn num_node(v: f32) -> IrNode {
        IrNode::Const {
            value: IrValue::Number(v),
        }
    }
}
