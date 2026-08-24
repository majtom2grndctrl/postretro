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

/// The fixed brain-fact input namespace, the sibling of
/// [`ENTITY_STATE_INPUT_PREFIX`]. Every name in [`BRAIN_INPUTS`] carries it, and
/// stripping it yields the field an author writes on the SDK's `brain` object
/// (`@brain.hasTarget` is authored `brain.hasTarget`).
pub const BRAIN_INPUT_PREFIX: &str = "@brain.";

/// `true` while the enemy has a selected target this tick.
///
/// This is the sole authoritative target-presence test. Target-side facts read
/// their type's zero with no selected target; [`BRAIN_NO_TARGET_DISTANCE`] is
/// the lone exception to that convention.
pub const BRAIN_HAS_TARGET_INPUT: &str = "@brain.hasTarget";
/// Distance to the selected target, or [`BRAIN_NO_TARGET_DISTANCE`] with none.
pub const BRAIN_TARGET_DISTANCE_INPUT: &str = "@brain.targetDistance";
/// Milliseconds since the currently-evaluated activity was entered.
///
/// Nested behavior envelopes evaluate their own rows against their own clock,
/// so this slot is re-pointed by the statechart evaluator before every level.
pub const BRAIN_TIME_IN_ACTIVITY_MS_INPUT: &str = "@brain.timeInActivityMs";
/// Milliseconds remaining on the selected offense action's cooldown; `0.0`
/// with no selected action or once it has elapsed.
pub const BRAIN_ATTACK_COOLDOWN_MS_INPUT: &str = "@brain.attackCooldownMs";
/// `true` on the think-stride ticks where the engine re-evaluates acquisition.
pub const BRAIN_ACQUISITION_DUE_INPUT: &str = "@brain.acquisitionDue";
/// The enemy's current hit points.
pub const BRAIN_HEALTH_INPUT: &str = "@brain.health";
/// The enemy's maximum hit points.
pub const BRAIN_MAX_HEALTH_INPUT: &str = "@brain.maxHealth";
/// The selected target's current hit points, or `0.0` with no target or no
/// target health component.
pub const BRAIN_TARGET_HEALTH_INPUT: &str = "@brain.targetHealth";
/// The selected target's maximum hit points, or `0.0` with no target or no
/// target health component.
pub const BRAIN_TARGET_MAX_HEALTH_INPUT: &str = "@brain.targetMaxHealth";
/// Whether the selected target's death sweep latch has fired, or `false` with
/// no target or no target health component.
pub const BRAIN_TARGET_DIED_INPUT: &str = "@brain.targetDied";
/// The enemy's XZ distance from its spawn-time home anchor. Unlike target-side
/// facts, this remains meaningful while no target is selected.
pub const BRAIN_DISTANCE_FROM_ANCHOR_INPUT: &str = "@brain.distanceFromAnchor";
/// Whether the selected target's faction differs from the evaluating enemy's,
/// or `false` with no selected target.
///
/// This is the durable authored relationship surface. The numeric faction
/// storage is intentionally an interim `@state` implementation detail.
pub const BRAIN_TARGET_HOSTILE_INPUT: &str = "@brain.targetHostile";
/// Whether the nav pathfinder can currently route from this enemy to its
/// selected target. `false` without a target or when the map has no navmesh.
///
/// This is the cached verdict from the same `find_path` query chase relies on,
/// not a ground-truth reachability oracle: it inherits the pathfinder's current
/// routing limitations.
pub const BRAIN_TARGET_REACHABLE_INPUT: &str = "@brain.targetReachable";
/// Successful attacks fired while the currently-evaluated activity has been
/// active.
///
/// Nested behavior envelopes evaluate their own rows against their own count,
/// so this slot is re-pointed by the statechart evaluator before every level.
/// A fire on the current tick becomes observable only after the next refresh.
pub const BRAIN_ATTACKS_FIRED_IN_ACTIVITY_INPUT: &str = "@brain.attacksFiredInActivity";
/// Whether the selected target is visible along the enemy's debounced
/// static-world eye-to-target sightline. `false` with no selected target.
///
/// This is the exact shared verdict the engine-floor fire gate reads; it is
/// independent of the additional range, cooldown, and facing gates.
pub const BRAIN_TARGET_VISIBLE_INPUT: &str = "@brain.targetVisible";

/// The distance reported for [`BRAIN_TARGET_DISTANCE_INPUT`] when the enemy has
/// no selected target.
///
/// A sentinel rather than an absent value because the IR is total: every input
/// must read as some number. It sits far beyond any plausible authored range so
/// a bare `le`/`lt(targetDistance, r)` guard reads false with no target, without
/// the author having to conjoin [`BRAIN_HAS_TARGET_INPUT`].
///
/// **The `gt`/`ge` direction is the inverse, not a mirror.** With no target,
/// every `gt(targetDistance, r)` / `ge(targetDistance, r)` guard reads **true**
/// — a bare distance guard is NOT a "target is far away" test, it is also the
/// "no target" test, and an authored graph whose only exits are `gt`/`ge`
/// guards will fire them the instant the target is lost. This already shipped
/// as a real defect: the reference enemy took a two-tick stand-down (playing
/// its travel animation for one of those ticks) when its last target
/// despawned. A disengagement edge that must be robust to target loss has to
/// gate on [`BRAIN_HAS_TARGET_INPUT`] directly, or be authored as an
/// root wildcard row that stands the brain down when `hasTarget` is false.
/// Use the explicit `not` opcode for inversion. The older
/// `select(cond, false, true)` spelling remains equivalent.
pub const BRAIN_NO_TARGET_DISTANCE: f32 = 1.0e9;

/// The fixed brain input namespace, in handle order. Each entry is a
/// `(name, projected IR type)` pair, and a name's index in this table *is* the
/// runtime scope's read handle.
///
/// The order is load-bearing: the runtime scope's snapshot array is indexed by
/// it, so refresh must write the same slots in the same order. Names use the
/// camelCase idiom of the script surface (scripting.md §4) inside the
/// `@`-reserved ephemeral-dispatch-input namespace (scripting.md §5).
pub const BRAIN_INPUTS: [(&str, IrType); 15] = [
    (BRAIN_HAS_TARGET_INPUT, IrType::Bool),
    (BRAIN_TARGET_DISTANCE_INPUT, IrType::Number),
    (BRAIN_TIME_IN_ACTIVITY_MS_INPUT, IrType::Number),
    (BRAIN_ATTACK_COOLDOWN_MS_INPUT, IrType::Number),
    (BRAIN_ACQUISITION_DUE_INPUT, IrType::Bool),
    (BRAIN_HEALTH_INPUT, IrType::Number),
    (BRAIN_MAX_HEALTH_INPUT, IrType::Number),
    (BRAIN_TARGET_HEALTH_INPUT, IrType::Number),
    (BRAIN_TARGET_MAX_HEALTH_INPUT, IrType::Number),
    (BRAIN_TARGET_DIED_INPUT, IrType::Bool),
    (BRAIN_DISTANCE_FROM_ANCHOR_INPUT, IrType::Number),
    (BRAIN_TARGET_HOSTILE_INPUT, IrType::Bool),
    (BRAIN_TARGET_REACHABLE_INPUT, IrType::Bool),
    (BRAIN_ATTACKS_FIRED_IN_ACTIVITY_INPUT, IrType::Number),
    (BRAIN_TARGET_VISIBLE_INPUT, IrType::Bool),
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
            owner: None,
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
    fn distance_from_anchor_appends_at_fixed_slot_ten() {
        assert_eq!(
            BRAIN_INPUTS[10],
            (BRAIN_DISTANCE_FROM_ANCHOR_INPUT, IrType::Number),
            "new brain facts append; they never repoint existing guard handles"
        );
    }

    #[test]
    fn target_hostile_appends_at_fixed_slot_eleven() {
        assert_eq!(
            BRAIN_INPUTS[11],
            (BRAIN_TARGET_HOSTILE_INPUT, IrType::Bool),
            "new brain facts append; they never repoint existing guard handles"
        );
    }

    #[test]
    fn target_reachable_appends_at_fixed_slot_twelve() {
        assert_eq!(
            BRAIN_INPUTS[12],
            (BRAIN_TARGET_REACHABLE_INPUT, IrType::Bool),
            "new brain facts append; they never repoint existing guard handles"
        );
    }

    #[test]
    fn attacks_fired_in_activity_appends_at_fixed_slot_thirteen() {
        assert_eq!(
            BRAIN_INPUTS[13],
            (BRAIN_ATTACKS_FIRED_IN_ACTIVITY_INPUT, IrType::Number),
            "new brain facts append; they never repoint existing guard handles"
        );
    }

    #[test]
    fn target_visible_appends_at_fixed_slot_fourteen() {
        assert_eq!(
            BRAIN_INPUTS[14],
            (BRAIN_TARGET_VISIBLE_INPUT, IrType::Bool),
            "new brain facts append; they never repoint existing guard handles"
        );
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
