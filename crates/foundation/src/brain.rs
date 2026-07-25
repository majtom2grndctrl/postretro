// Behavior-graph guard vocabulary: the fixed `@brain.*` input table plus the
// declaration-time `BindingScope` authored guards are validated against.
// See: context/lib/scripting.md §11 (IR substrate — pluggable scope abstraction)

// A behavior-graph transition guard is an IR tree over live brain facts. The
// names it may read are fixed here, in the VM-free foundation, because two
// consumers need them and neither may depend on the other: the twin descriptor
// parsers bind every authored guard at declaration time (against
// `BrainValidationScope` below), while the engine's runtime `BrainScope` names
// entity components and therefore lives in the binary. Both route name
// resolution through `resolve_brain_input`, so the two namespaces are one
// implementation and cannot drift.

use crate::ir::{
    BakedIr, BindError, BindingScope, BoundProgram, CURRENT_IR_VERSION, ENTITY_STATE_INPUT_PREFIX,
    IrNode, IrType, IrValue, ResolvedInput, ResolvedOutput, bind,
};

/// `true` while the enemy has a selected target this tick.
pub const BRAIN_HAS_TARGET_INPUT: &str = "@brain.hasTarget";
/// Distance to the selected target, or [`BRAIN_NO_TARGET_DISTANCE`] with none.
pub const BRAIN_TARGET_DISTANCE_INPUT: &str = "@brain.targetDistance";
/// Milliseconds since the brain entered its current graph state.
pub const BRAIN_TIME_IN_STATE_MS_INPUT: &str = "@brain.timeInStateMs";
/// Milliseconds remaining on the attack cooldown; `0.0` once it has elapsed.
pub const BRAIN_ATTACK_COOLDOWN_MS_INPUT: &str = "@brain.attackCooldownMs";
/// `true` on the think-stride ticks where the engine re-evaluates acquisition.
pub const BRAIN_ACQUISITION_DUE_INPUT: &str = "@brain.acquisitionDue";
/// The enemy's current hit points.
pub const BRAIN_HEALTH_INPUT: &str = "@brain.health";
/// The enemy's maximum hit points.
pub const BRAIN_MAX_HEALTH_INPUT: &str = "@brain.maxHealth";

/// The distance reported for [`BRAIN_TARGET_DISTANCE_INPUT`] when the enemy has
/// no selected target.
///
/// A sentinel rather than an absent value because the IR is total: every input
/// must read as some number. It sits far beyond any plausible authored range so
/// a bare `le(targetDistance, r)` guard reads false with no target, without the
/// author having to conjoin [`BRAIN_HAS_TARGET_INPUT`].
pub const BRAIN_NO_TARGET_DISTANCE: f32 = 1.0e9;

/// The fixed brain input namespace, in handle order. Each entry is a
/// `(name, projected IR type)` pair, and a name's index in this table *is* the
/// runtime scope's read handle.
///
/// The order is load-bearing: the runtime scope's snapshot array is indexed by
/// it, so refresh must write the same slots in the same order. Names use the
/// `@`-reserved, camelCase idiom of the script surface (scripting.md §4).
pub const BRAIN_INPUTS: [(&str, IrType); 7] = [
    (BRAIN_HAS_TARGET_INPUT, IrType::Bool),
    (BRAIN_TARGET_DISTANCE_INPUT, IrType::Number),
    (BRAIN_TIME_IN_STATE_MS_INPUT, IrType::Number),
    (BRAIN_ATTACK_COOLDOWN_MS_INPUT, IrType::Number),
    (BRAIN_ACQUISITION_DUE_INPUT, IrType::Bool),
    (BRAIN_HEALTH_INPUT, IrType::Number),
    (BRAIN_MAX_HEALTH_INPUT, IrType::Number),
];

/// What a brain input name resolves to, independent of where the values live.
///
/// This is the single resolution rule both the declaration-time
/// [`BrainValidationScope`] and the binary's runtime `BrainScope` implement
/// their `resolve_input` on top of, so the two agree on every name by
/// construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrainInputRef<'a> {
    /// A fixed engine fact: the index into [`BRAIN_INPUTS`] and its type.
    Fixed { index: usize, ir_type: IrType },
    /// A per-entity numeric state leaf; carries the field name after the
    /// [`ENTITY_STATE_INPUT_PREFIX`]. Always projects as a number.
    State(&'a str),
}

/// Resolve a guard input name, or `None` when it belongs to neither the fixed
/// table nor the `@state.` namespace.
pub fn resolve_brain_input(name: &str) -> Option<BrainInputRef<'_>> {
    if let Some(field) = name.strip_prefix(ENTITY_STATE_INPUT_PREFIX) {
        return Some(BrainInputRef::State(field));
    }
    let index = BRAIN_INPUTS
        .iter()
        .position(|(input_name, _)| *input_name == name)?;
    Some(BrainInputRef::Fixed {
        index,
        ir_type: BRAIN_INPUTS[index].1,
    })
}

/// The declaration-time brain namespace: resolves exactly the names the runtime
/// scope resolves, but holds no live values.
///
/// Bind consults only names and types, so an authored guard type-checks against
/// this with no entity, no registry, and no tick in hand — which is what lets
/// both script runtimes reject a bad guard at descriptor validation with the
/// authored path in the message. `read` stays total by answering with the
/// resolved type's zero.
#[derive(Clone, Copy, Debug, Default)]
pub struct BrainValidationScope;

impl BindingScope for BrainValidationScope {
    /// A validation bind never reads live state, so the resolved name's
    /// projected type is the whole handle — it is all `read` needs to answer
    /// with a type-correct zero.
    type InputHandle = IrType;
    /// Read-only scope: no output is ever resolved, so this is never
    /// constructed.
    type OutputHandle = IrType;

    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<IrType>> {
        let ir_type = match resolve_brain_input(name)? {
            BrainInputRef::Fixed { ir_type, .. } => ir_type,
            BrainInputRef::State(_) => IrType::Number,
        };
        Some(ResolvedInput {
            handle: ir_type,
            ir_type,
        })
    }

    fn resolve_output(&self, _name: &str) -> Option<ResolvedOutput<IrType>> {
        // Guards are read-only: they consume brain and entity state, never
        // write it. Writes to `@state.*` are the impact-policy path.
        None
    }

    fn read(&self, handle: &IrType) -> IrValue {
        handle.zero()
    }

    fn write(&mut self, _handle: &IrType, _value: IrValue) {
        unreachable!(
            "BrainValidationScope is read-only; resolve_output never grants a write handle"
        )
    }
}

/// Bind one authored transition guard at declaration time.
///
/// Wraps the node in a read-only [`BakedIr`] envelope — guards produce a value,
/// they never write an output — and binds it against [`BrainValidationScope`],
/// mirroring the dash `bind_dash_node` path. Callers own the surrounding
/// diagnostics: the returned [`BindError`] carries no authored path, and the
/// guard's root type is the caller's to check (a guard must produce a boolean,
/// but only the caller knows which state and transition index to name).
pub fn bind_brain_guard(node: &IrNode) -> Result<BoundProgram<BrainValidationScope>, BindError> {
    let baked = BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root: node.clone(),
    };
    bind(&baked, &BrainValidationScope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::eval_value;

    fn input(name: &str) -> IrNode {
        IrNode::Input {
            name: name.to_string(),
        }
    }

    #[test]
    fn validation_scope_resolves_every_fixed_input_with_its_declared_type() {
        let scope = BrainValidationScope;
        for (name, ir_type) in BRAIN_INPUTS {
            let resolved = scope
                .resolve_input(name)
                .unwrap_or_else(|| panic!("`{name}` must resolve"));
            assert_eq!(
                resolved.ir_type, ir_type,
                "`{name}` projects its declared type"
            );
        }
    }

    #[test]
    fn validation_scope_routes_only_the_exact_state_prefix_as_a_number() {
        let scope = BrainValidationScope;
        let resolved = scope
            .resolve_input("@state.staggered")
            .expect("`@state.` leaves resolve without declaration");
        assert_eq!(resolved.ir_type, IrType::Number);

        for name in ["@stateful.staggered", "@brain.notAnInput", "staggered"] {
            assert!(
                scope.resolve_input(name).is_none(),
                "`{name}` must not resolve"
            );
        }
    }

    #[test]
    fn validation_scope_denies_every_output() {
        let scope = BrainValidationScope;
        for (name, _) in BRAIN_INPUTS {
            assert!(
                scope.resolve_output(name).is_none(),
                "`{name}` must not resolve as a writable output"
            );
        }
        assert!(
            scope.resolve_output("@state.staggered").is_none(),
            "guards never write entity state"
        );
    }

    #[test]
    fn validation_scope_reads_type_correct_zeros() {
        let scope = BrainValidationScope;
        for (name, ir_type) in BRAIN_INPUTS {
            let program = bind_brain_guard(&input(name)).unwrap_or_else(|_| panic!("`{name}`"));
            assert_eq!(
                eval_value(&program, &scope),
                ir_type.zero(),
                "`{name}` reads its type's zero with no live values"
            );
        }
    }

    #[test]
    fn bind_brain_guard_accepts_a_mixed_brain_and_state_guard() {
        // The stagger shape: an interrupt over a per-entity field conjoined
        // with a fixed brain fact via `select` (no `and` opcode yet).
        let guard = IrNode::Select {
            cond: Box::new(IrNode::Ge {
                a: Box::new(input("@state.staggered")),
                b: Box::new(IrNode::Const {
                    value: IrValue::Number(1.0),
                }),
            }),
            a: Box::new(IrNode::Le {
                a: Box::new(input(BRAIN_TARGET_DISTANCE_INPUT)),
                b: Box::new(IrNode::Const {
                    value: IrValue::Number(16.0),
                }),
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Bool(false),
            }),
        };
        let program = bind_brain_guard(&guard).expect("guard binds");
        assert_eq!(program.root_type, IrType::Bool);
    }

    #[test]
    fn bind_brain_guard_rejects_an_unknown_input_name() {
        assert_eq!(
            bind_brain_guard(&input("@brain.morale")).unwrap_err(),
            BindError::UnknownInput {
                name: "@brain.morale".to_string()
            }
        );
    }

    #[test]
    fn bind_brain_guard_rejects_a_type_mismatched_operand() {
        // `hasTarget` is a boolean; feeding it to a numeric comparison is a
        // declaration-time error, not a tick-time surprise.
        let guard = IrNode::Le {
            a: Box::new(input(BRAIN_HAS_TARGET_INPUT)),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(1.0),
            }),
        };
        assert!(matches!(
            bind_brain_guard(&guard),
            Err(BindError::TypeMismatch { .. })
        ));
    }
}
