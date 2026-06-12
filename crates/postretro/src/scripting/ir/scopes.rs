// Concrete `BindingScope` implementations: the store-backed adapter the engine
// and scripts bind real behavior IR against, plus an indexed test stub that
// proves the namespace seam is pluggable (not store-shaped).
// See: context/lib/scripting.md §11 (IR substrate — pluggable scope abstraction)

// Two scopes live here. `StoreScope` bridges the IR evaluator to the live
// `SlotTable` through a captured `ScriptCtx`; it projects `Number`/`Boolean`
// slots into the IR value model and gates writes by a capability `mode`
// mirroring the engine-bypass vs script-gated split in `primitives::store`.
// `StubScope` (test-only) carries a fixed input/output set keyed by *index*
// handles, distinct from the store scope's owned-name handles — a bound program
// is therefore portable across differently-shaped namespaces.

use super::scope::{BindingScope, ResolvedInput, ResolvedOutput};
use super::{IrType, IrValue};
use crate::scripting::ctx::ScriptCtx;
use crate::scripting::primitives::store::write_store_slot;
use crate::scripting::slot_table::{SlotType, SlotValue};

/// Write-capability mode for a [`StoreScope`]. Mirrors the two write paths in
/// `primitives::store`: an engine-policy program bypasses the readonly flag
/// (engine systems own those slots), while a script-authored program is gated
/// by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoreCapability {
    /// Engine-policy IR (e.g. shield recharge). `resolve_output` grants a write
    /// handle for *any* projectable slot, readonly included; `write` delegates
    /// to the validated engine-bypass path.
    Engine,
    /// Script-authored IR (the deferred UI `setState`). `resolve_output` grants
    /// a handle only for non-readonly projectable slots; a readonly slot is
    /// denied at bind.
    Script,
}

/// A resolved store handle: the slot's stable dotted name plus its projected IR
/// type. Owning the name keeps the handle valid for the program's lifetime
/// without borrowing the table; the type is cached so `read` need not re-derive
/// it from the live slot.
#[derive(Clone, Debug)]
pub(crate) struct StoreHandle {
    name: String,
    ir_type: IrType,
}

/// Binds and evaluates IR against the engine-global [`SlotTable`] via a captured
/// [`ScriptCtx`]. Cloning the ctx is cheap (it bumps `Rc`s); the scope owns its
/// clone so it can read and write the live table without an external borrow.
pub(crate) struct StoreScope {
    ctx: ScriptCtx,
    mode: StoreCapability,
}

impl StoreScope {
    /// An engine-policy scope: writes bypass the readonly flag through the
    /// validated engine path.
    pub(crate) fn engine(ctx: ScriptCtx) -> Self {
        Self {
            ctx,
            mode: StoreCapability::Engine,
        }
    }

    /// A script-capability scope: readonly slots are denied a write handle at
    /// bind; granted writes flow through the same validated engine write path.
    pub(crate) fn script(ctx: ScriptCtx) -> Self {
        Self {
            ctx,
            mode: StoreCapability::Script,
        }
    }

    /// Project a slot's declared type into the IR value model, or `None` for the
    /// non-projectable kinds (`String`/`Enum`/`Array`).
    fn project(slot_type: &SlotType) -> Option<IrType> {
        match slot_type {
            SlotType::Number => Some(IrType::Number),
            SlotType::Boolean => Some(IrType::Bool),
            SlotType::String | SlotType::Enum { .. } | SlotType::Array => None,
        }
    }
}

impl BindingScope for StoreScope {
    type InputHandle = StoreHandle;
    type OutputHandle = StoreHandle;

    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<StoreHandle>> {
        let table = self.ctx.slot_table.borrow();
        let record = table.get(name)?;
        let ir_type = Self::project(&record.schema.slot_type)?;
        Some(ResolvedInput {
            handle: StoreHandle {
                name: name.to_string(),
                ir_type,
            },
            ir_type,
        })
    }

    fn resolve_output(&self, name: &str) -> Option<ResolvedOutput<StoreHandle>> {
        let table = self.ctx.slot_table.borrow();
        let record = table.get(name)?;
        let ir_type = Self::project(&record.schema.slot_type)?;
        // Script-capability scopes cannot write readonly slots — deny the handle
        // at bind so the write path is never reached for them. Engine scopes
        // bypass readonly, matching `write_store_slot`'s engine-bypass policy.
        if self.mode == StoreCapability::Script && record.schema.readonly {
            return None;
        }
        Some(ResolvedOutput {
            handle: StoreHandle {
                name: name.to_string(),
                ir_type,
            },
            ir_type,
        })
    }

    fn read(&self, handle: &StoreHandle) -> IrValue {
        // Alloc-free re-hash through the existing `get(&str)`; no new store API.
        let table = self.ctx.slot_table.borrow();
        let value = table
            .get(&handle.name)
            .and_then(|record| record.value.as_ref());
        match (handle.ir_type, value) {
            (IrType::Number, Some(SlotValue::Number(n))) => IrValue::Number(*n),
            (IrType::Bool, Some(SlotValue::Boolean(b))) => IrValue::Bool(*b),
            // Absent value, or a slot whose live kind disagrees with the bound
            // projection (cannot happen post-bind for a stable table): total
            // type-zero per the eval contract.
            (IrType::Number, _) => IrValue::Number(0.0),
            (IrType::Bool, _) => IrValue::Bool(false),
        }
    }

    fn write(&mut self, handle: &StoreHandle, value: IrValue) {
        // Both modes funnel the engine-validated `write_store_slot` (type/range
        // validation, clamp-with-warning). The capability difference is enforced
        // at bind: Script mode never resolves a readonly output, so reaching
        // here means the write is permitted. We deliberately do not duplicate
        // the script-gated readonly *re-check* — bind already denied it, and the
        // typed (Number/Bool) values eval produces are never the non-projectable
        // kinds the script path additionally guards.
        let slot_value = match value {
            IrValue::Number(n) => SlotValue::Number(n),
            IrValue::Bool(b) => SlotValue::Boolean(b),
        };
        // A failed write (unknown slot / type mismatch) cannot arise for a
        // bound handle against a stable table; if it somehow does, the engine
        // path logs and we drop the error rather than panicking per-tick.
        let _ = write_store_slot(&self.ctx, &handle.name, slot_value);
    }
}

// ---------------------------------------------------------------------------
// Test-stub scope: an indexed namespace proving the seam is pluggable.
// ---------------------------------------------------------------------------

/// The value kind a [`StubScope`] output accepts. Distinct from `IrType` only to
/// keep the stub's surface self-describing at call sites.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StubWrite {
    Number,
    Bool,
}

#[cfg(test)]
struct StubInput {
    name: &'static str,
    ir_type: IrType,
    value: Option<IrValue>,
}

#[cfg(test)]
struct StubOutput {
    name: &'static str,
    ir_type: IrType,
    written: Option<IrValue>,
}

/// A fixed-set test scope with **indexed** handles (`usize`), deliberately
/// unlike [`StoreScope`]'s owned-name handles so a single bound program can be
/// shown portable across differently-shaped namespaces. Seeds a movement-like
/// input set and a configurable output set; supports read and write.
///
/// Reusable by the end-to-end composition tests via [`StubScope::new`] and
/// [`StubScope::with_writes`].
#[cfg(test)]
pub(crate) struct StubScope {
    inputs: Vec<StubInput>,
    outputs: Vec<StubOutput>,
}

#[cfg(test)]
impl StubScope {
    /// A stub seeded with a fixed movement-like input set and no outputs.
    ///
    /// Inputs: `speed` (number = 4.0), `grounded` (bool = true),
    /// `unset_number` (number, no current value → reads as 0.0),
    /// `unset_flag` (bool, no current value → reads as false).
    pub(crate) fn new() -> Self {
        Self {
            inputs: vec![
                StubInput {
                    name: "speed",
                    ir_type: IrType::Number,
                    value: Some(IrValue::Number(4.0)),
                },
                StubInput {
                    name: "grounded",
                    ir_type: IrType::Bool,
                    value: Some(IrValue::Bool(true)),
                },
                StubInput {
                    name: "unset_number",
                    ir_type: IrType::Number,
                    value: None,
                },
                StubInput {
                    name: "unset_flag",
                    ir_type: IrType::Bool,
                    value: None,
                },
            ],
            outputs: Vec::new(),
        }
    }

    /// As [`StubScope::new`], plus the named outputs of the given kinds (all
    /// writable). The stub grants write handles only for these; any other output
    /// name fails to bind.
    pub(crate) fn with_writes(outputs: &[(&'static str, StubWrite)]) -> Self {
        let mut scope = Self::new();
        scope.outputs = outputs
            .iter()
            .map(|&(name, kind)| StubOutput {
                name,
                ir_type: match kind {
                    StubWrite::Number => IrType::Number,
                    StubWrite::Bool => IrType::Bool,
                },
                written: None,
            })
            .collect();
        scope
    }

    /// Override an input's current value (e.g. to drive cross-scope tests).
    pub(crate) fn set_input(&mut self, name: &str, value: IrValue) {
        if let Some(input) = self.inputs.iter_mut().find(|input| input.name == name) {
            input.value = Some(value);
        }
    }

    /// The most recent value written to the named output, if any.
    pub(crate) fn written(&self, name: &str) -> Option<IrValue> {
        self.outputs
            .iter()
            .find(|output| output.name == name)
            .and_then(|output| output.written)
    }
}

#[cfg(test)]
impl BindingScope for StubScope {
    type InputHandle = usize;
    type OutputHandle = usize;

    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<usize>> {
        self.inputs
            .iter()
            .position(|input| input.name == name)
            .map(|handle| ResolvedInput {
                handle,
                ir_type: self.inputs[handle].ir_type,
            })
    }

    fn resolve_output(&self, name: &str) -> Option<ResolvedOutput<usize>> {
        self.outputs
            .iter()
            .position(|output| output.name == name)
            .map(|handle| ResolvedOutput {
                handle,
                ir_type: self.outputs[handle].ir_type,
            })
    }

    fn read(&self, handle: &usize) -> IrValue {
        let input = &self.inputs[*handle];
        // Missing value → type-zero, per the scope's totality contract.
        input.value.unwrap_or(match input.ir_type {
            IrType::Number => IrValue::Number(0.0),
            IrType::Bool => IrValue::Bool(false),
        })
    }

    fn write(&mut self, handle: &usize, value: IrValue) {
        self.outputs[*handle].written = Some(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ir::eval::{eval_and_write, eval_value};
    use crate::scripting::ir::{BakedIr, BindError, CURRENT_IR_VERSION, IrNode, bind};
    use crate::scripting::slot_table::{
        NumericRange, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    };

    const EPSILON: f32 = 1e-6;

    fn num(v: f32) -> Box<IrNode> {
        Box::new(IrNode::Const {
            value: IrValue::Number(v),
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

    fn number_slot(value: f32, readonly: bool) -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: SlotType::Number,
            default: Some(SlotValue::Number(value)),
            range: Some(NumericRange {
                min: 0.0,
                max: 100.0,
            }),
            persist: false,
            readonly,
            ownership: if readonly {
                SlotOwnership::Engine
            } else {
                SlotOwnership::Mod
            },
        })
    }

    fn bool_slot(value: bool) -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: SlotType::Boolean,
            default: Some(SlotValue::Boolean(value)),
            range: None,
            persist: false,
            readonly: false,
            ownership: SlotOwnership::Mod,
        })
    }

    fn string_slot() -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: SlotType::String,
            default: Some(SlotValue::String("x".to_string())),
            range: None,
            persist: false,
            readonly: false,
            ownership: SlotOwnership::Mod,
        })
    }

    /// A ctx seeded with: `test.number` (writable, value 25), `test.flag`
    /// (bool true), `test.label` (string — non-projectable), and the built-in
    /// readonly `player.health`.
    fn seeded_ctx() -> ScriptCtx {
        let ctx = ScriptCtx::new();
        {
            let mut table = ctx.slot_table.borrow_mut();
            table
                .insert("test.number".to_string(), number_slot(25.0, false))
                .unwrap();
            table
                .insert("test.flag".to_string(), bool_slot(true))
                .unwrap();
            table
                .insert("test.label".to_string(), string_slot())
                .unwrap();
        }
        // Give the readonly engine slot a current value so reads/writes are
        // observable.
        write_store_slot(&ctx, "player.health", SlotValue::Number(50.0)).unwrap();
        ctx
    }

    #[test]
    fn store_scope_projects_number_and_bool_inputs_and_reads_them() {
        let ctx = seeded_ctx();
        let scope = StoreScope::engine(ctx);

        let program = bind(&read_only(*input("test.number")), &scope).expect("number projects");
        assert_eq!(program.root_type, IrType::Number);
        assert_number(eval_value(&program, &scope), 25.0);

        let program = bind(&read_only(*input("test.flag")), &scope).expect("bool projects");
        assert_eq!(eval_value(&program, &scope), IrValue::Bool(true));
    }

    #[test]
    fn store_scope_denies_non_projectable_and_unknown_inputs() {
        let ctx = seeded_ctx();
        let scope = StoreScope::engine(ctx);
        for name in ["test.label", "test.missing"] {
            assert_eq!(
                bind(&read_only(*input(name)), &scope).unwrap_err(),
                BindError::UnknownInput {
                    name: name.to_string()
                }
            );
        }
    }

    #[test]
    fn store_scope_reads_absent_value_as_type_zero() {
        let ctx = ScriptCtx::new();
        let scope = StoreScope::engine(ctx);
        // `player.health` is readonly with no default/value → reads 0.0.
        let program = bind(&read_only(*input("player.health")), &scope).expect("projects");
        assert_number(eval_value(&program, &scope), 0.0);
    }

    #[test]
    fn engine_mode_writes_readonly_slot_through_validated_path() {
        // Engine policy bypasses readonly: it resolves a write handle for a
        // readonly engine-owned slot and writes through the validated path,
        // which range-clamps. The slot carries a known [0, 100] range so the
        // clamp is asserted against a pinned bound.
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("engine.shield".to_string(), number_slot(50.0, true))
            .unwrap();
        let mut scope = StoreScope::engine(ctx.clone());
        let baked = BakedIr {
            version: CURRENT_IR_VERSION,
            output: Some("engine.shield".to_string()),
            // 200 exceeds the slot range [0, 100]; the validated path clamps.
            root: *num(200.0),
        };
        let program = bind(&baked, &scope).expect("engine grants readonly write handle");
        eval_and_write(&program, &mut scope);
        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("engine.shield")
                .and_then(|r| r.value.clone()),
            Some(SlotValue::Number(100.0)),
            "engine write is validated and range-clamped"
        );
    }

    #[test]
    fn script_mode_denies_readonly_output_at_bind() {
        let ctx = seeded_ctx();
        let scope = StoreScope::script(ctx);
        let baked = BakedIr {
            version: CURRENT_IR_VERSION,
            output: Some("player.health".to_string()),
            root: *num(10.0),
        };
        assert_eq!(
            bind(&baked, &scope).unwrap_err(),
            BindError::UnknownOutput {
                name: "player.health".to_string()
            },
            "script capability must not grant a readonly write handle"
        );
    }

    #[test]
    fn script_mode_writes_writable_slot() {
        let ctx = seeded_ctx();
        let mut scope = StoreScope::script(ctx.clone());
        let baked = BakedIr {
            version: CURRENT_IR_VERSION,
            output: Some("test.number".to_string()),
            root: *num(42.0),
        };
        let program = bind(&baked, &scope).expect("writable slot binds in script mode");
        eval_and_write(&program, &mut scope);
        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("test.number")
                .and_then(|r| r.value.clone()),
            Some(SlotValue::Number(42.0))
        );
    }

    #[test]
    fn stub_scope_grants_writes_only_for_declared_outputs() {
        // An envelope targeting a granted output binds and writes; one targeting
        // an ungranted output fails to bind (write capability is a bind-time grant).
        let mut scope = StubScope::with_writes(&[("out_number", StubWrite::Number)]);
        let granted = BakedIr {
            version: CURRENT_IR_VERSION,
            output: Some("out_number".to_string()),
            root: IrNode::Add {
                a: num(1.0),
                b: input("speed"),
            },
        };
        let program = bind(&granted, &scope).expect("granted output binds");
        eval_and_write(&program, &mut scope);
        assert_number(scope.written("out_number").expect("written"), 5.0);

        let denied = BakedIr {
            version: CURRENT_IR_VERSION,
            output: Some("not_declared".to_string()),
            root: *num(1.0),
        };
        assert_eq!(
            bind(&denied, &scope).unwrap_err(),
            BindError::UnknownOutput {
                name: "not_declared".to_string()
            }
        );
    }

    #[test]
    fn same_tree_binds_against_store_and_stub_scopes() {
        // One IR tree, two scopes with distinct handle types (owned-name vs
        // index), each reading its own `speed` value. The slot table imposes no
        // dotted-name requirement, so inserting under the plain name "speed"
        // makes both scopes resolvable from the identical tree.
        let tree = read_only(IrNode::Add {
            a: input("speed"),
            b: num(1.0),
        });

        // Store scope: declare a `speed` number slot at value 10.
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("speed".to_string(), number_slot(10.0, false))
            .unwrap();
        let store_scope = StoreScope::engine(ctx);
        let store_program = bind(&tree, &store_scope).expect("store binds");
        assert_number(eval_value(&store_program, &store_scope), 11.0);

        // Stub scope: `speed` is 4.0 by construction — same tree, different scope.
        let stub_scope = StubScope::new();
        let stub_program = bind(&tree, &stub_scope).expect("stub binds");
        assert_number(eval_value(&stub_program, &stub_scope), 5.0);
    }

    #[test]
    fn stub_set_input_drives_reads() {
        let mut scope = StubScope::new();
        scope.set_input("speed", IrValue::Number(9.0));
        let program = bind(&read_only(*input("speed")), &scope).expect("binds");
        assert_number(eval_value(&program, &scope), 9.0);
    }
}
