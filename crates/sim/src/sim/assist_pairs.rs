//! Host evaluation of each assist pair's radius and required ticks from the
//! target state's `assistable` profile.
//! See: context/lib/scripting.md §11 (interaction-pair scope) · §12 (Assist)
//!
//! The table binds every `assistable` expression once per level install or
//! recompose against one [`InteractionPairScope`], keyed by (descriptor, state
//! id). Each pass seeds that scope's sides once ([`AssistPairTable::seed_sides`])
//! and then [`evaluate`]s pairs, applying AC13's results:
//!
//! | Result | [`PairValues`] |
//! |---|---|
//! | radius finite and > 16 | radius 16 |
//! | radius finite and ≤ 0 | radius 0: this helper cannot assist this target |
//! | radius non-finite or a side value missing | radius 0 (out of range); warns once |
//! | holdMs finite and > 60000 | `required_count(60000)` |
//! | holdMs 0 (−0.0 included) | `required_ticks` 1: completes on the first held tick |
//! | holdMs negative, non-finite or a side value missing | `required_ticks` 0 (kept, not advanced); warns once |
//!
//! "Unassistable" and "out of range" share radius 0: a radius of 0 is never
//! in range ([`PairValues::in_range`]), so the accumulator and the wire pair
//! table need no third state. Fault warnings fire once per (descriptor,
//! state, field) and are re-armed by a rebind, which replaces the table. The
//! memory sits behind a `Cell`, so a caller holding the table by shared
//! reference (the snapshot build) shares it.

use std::cell::Cell;

use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{EntityId, EntityRegistry, ScriptCtx};
use postretro_foundation::{
    ASSIST_MAX_HOLD_MS, ASSIST_MAX_RADIUS_M, BakedIr, BoundProgram, CURRENT_IR_VERSION, IrNode,
    IrType, IrValue, NumberOrIr, PlayerMovementComponent, bind, eval_value_checked,
};
use postretro_scripting_core::group_resolution::group_commands_apply_here;
use postretro_scripting_core::ir_pair_scope::{ASSIST_PAIR_ROLES, InteractionPairScope, PairInput};
use postretro_scripting_core::slot_table::SlotTable;

use super::assist::required_count;

/// One pair's evaluated values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PairValues {
    /// Reach between pawn origins in metres, in `[0, 16]`. 0 means this
    /// helper cannot assist this target, or the radius faulted (out of range).
    pub radius: f32,
    /// The hold's required tick count; 0 is the "kept, not advanced" sentinel
    /// of a negative or faulted `holdMs`.
    pub required_ticks: u32,
}

impl PairValues {
    /// A pair that is never in range and never advances.
    pub const UNASSISTABLE: PairValues = PairValues {
        radius: 0.0,
        required_ticks: 0,
    };

    /// Whether a target at `distance` lies within this pair's radius. Never
    /// for radius 0.
    pub fn in_range(&self, distance: f32) -> bool {
        self.radius > 0.0 && distance <= self.radius
    }
}

/// The target side of a pair: its pawn and the (descriptor, state) whose
/// `assistable` profile applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssistTarget<'a> {
    pub pawn: EntityId,
    pub descriptor: &'a str,
    pub life_state: u8,
}

impl<'a> AssistTarget<'a> {
    /// The pawn's descriptor and current life state, read from the registry.
    pub fn of(registry: &'a EntityRegistry, pawn: EntityId) -> Option<Self> {
        let descriptor = registry
            .get_component::<DescriptorProvenance>(pawn)
            .ok()?
            .canonical_name
            .as_str();
        let life_state = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .ok()?
            .life_state;
        Some(Self {
            pawn,
            descriptor,
            life_state,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Radius,
    HoldMs,
}

impl Field {
    fn key(self) -> &'static str {
        match self {
            Field::Radius => "radius",
            Field::HoldMs => "holdMs",
        }
    }

    fn bit(self) -> u8 {
        match self {
            Field::Radius => 1,
            Field::HoldMs => 2,
        }
    }
}

enum BoundValue {
    Literal(f32),
    Expr {
        program: BoundProgram<InteractionPairScope>,
        inputs: Vec<PairInput>,
    },
}

struct BoundAssistable {
    descriptor: String,
    state: u8,
    state_name: String,
    radius: BoundValue,
    hold_ms: BoundValue,
    /// Both fields literal: the values every pair shares.
    literal: Option<PairValues>,
    /// Fault-warning memory, one bit per field.
    warned: Cell<u8>,
}

/// The host's bound `assistable` profiles, keyed by (descriptor, state id).
/// The default table binds nothing: every pair is [`PairValues::UNASSISTABLE`].
#[derive(Default)]
pub struct AssistPairTable {
    /// Present when some field is an expression; bound programs index into
    /// this instance's side inputs, so they live together.
    scope: Option<InteractionPairScope>,
    entries: Vec<BoundAssistable>,
}

impl AssistPairTable {
    /// Bind every `assistable` profile of the active descriptors. A connected
    /// client binds nothing. A field that fails to bind (validation already
    /// refused such content at load) drops its entry with a warning.
    pub fn bind(ctx: &ScriptCtx) -> Self {
        let mut table = Self::default();
        if !group_commands_apply_here(ctx) {
            return table;
        }
        let data_registry = ctx.data_registry.borrow();
        let scope = InteractionPairScope::new(ctx.clone(), ASSIST_PAIR_ROLES);
        let mut uses_scope = false;
        for descriptor in &data_registry.entities {
            let (Some(name), Some(block)) = (
                descriptor.canonical_name.as_deref(),
                descriptor.life_states.as_ref(),
            ) else {
                continue;
            };
            for (state_name, profile) in &block.states {
                let (Some(assistable), Some(&state)) =
                    (profile.assistable.as_ref(), block.state_ids.get(state_name))
                else {
                    continue;
                };
                let bound = |field: Field, value: &NumberOrIr| -> Option<BoundValue> {
                    match value {
                        NumberOrIr::Literal(value) => Some(BoundValue::Literal(*value)),
                        NumberOrIr::Ir(node) => bind_field(&scope, node)
                            .map_err(|reason| {
                                log::warn!(
                                    "[Assist] descriptor `{name}` `components.lifeStates.states.{state_name}.assistable.{}` did not bind: {reason}; the state is not assistable",
                                    field.key()
                                );
                            })
                            .ok(),
                    }
                };
                let (Some(radius), Some(hold_ms)) = (
                    bound(Field::Radius, &assistable.radius),
                    bound(Field::HoldMs, &assistable.hold_ms),
                ) else {
                    continue;
                };
                let literal = match (&radius, &hold_ms) {
                    (BoundValue::Literal(radius), BoundValue::Literal(hold_ms)) => {
                        Some(PairValues {
                            radius: radius_result(*radius).unwrap_or(0.0),
                            required_ticks: hold_result(*hold_ms).unwrap_or(0),
                        })
                    }
                    _ => {
                        uses_scope = true;
                        None
                    }
                };
                table.entries.push(BoundAssistable {
                    descriptor: name.to_string(),
                    state,
                    state_name: state_name.clone(),
                    radius,
                    hold_ms,
                    literal,
                    warned: Cell::new(0),
                });
            }
        }
        if uses_scope {
            table.scope = Some(scope);
        }
        table
    }

    /// Whether (descriptor, state) has a bound `assistable` profile.
    pub fn declares(&self, descriptor: &str, life_state: u8) -> bool {
        self.entry(descriptor, life_state).is_some()
    }

    /// Snapshot every bound side input for each pawn, once per pass, so the
    /// pass's evaluations read one consistent registry state. Linear in pawns;
    /// a literal-only table seeds nothing.
    pub fn seed_sides(
        &self,
        registry: &EntityRegistry,
        slot_table: &SlotTable,
        pawns: &[EntityId],
    ) {
        if let Some(scope) = &self.scope {
            scope.seed_sides(registry, slot_table, pawns);
        }
    }

    fn entry(&self, descriptor: &str, life_state: u8) -> Option<&BoundAssistable> {
        self.entries
            .iter()
            .find(|entry| entry.state == life_state && entry.descriptor == descriptor)
    }
}

fn bind_field(scope: &InteractionPairScope, node: &IrNode) -> Result<BoundValue, String> {
    scope.begin_expression();
    let baked = BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root: node.clone(),
    };
    let program = bind(&baked, scope)
        .map_err(|error| scope.take_bind_error().unwrap_or_else(|| error.to_string()))?;
    let inputs = scope.finish_expression();
    if program.root_type != IrType::Number {
        return Err("expression must produce a number".to_string());
    }
    Ok(BoundValue::Expr { program, inputs })
}

/// A finite radius: clamped to the cap, or 0 at or below 0. `None` is a fault.
fn radius_result(radius: f32) -> Option<f32> {
    radius.is_finite().then(|| {
        if radius > 0.0 {
            radius.min(ASSIST_MAX_RADIUS_M)
        } else {
            0.0
        }
    })
}

/// A finite `holdMs` from 0 up (−0.0 counts as 0), clamped and converted.
/// `None` is a fault: negative or non-finite.
fn hold_result(hold_ms: f32) -> Option<u32> {
    (hold_ms.is_finite() && hold_ms >= 0.0).then(|| required_count(hold_ms.min(ASSIST_MAX_HOLD_MS)))
}

/// Evaluate one (helper, target) pair against the target state's profile.
/// The table's sides must have been seeded this pass
/// ([`AssistPairTable::seed_sides`]) with both pawns; a pawn missing from the
/// seed reads as a missing side value. A target whose (descriptor, state) has
/// no bound profile is [`PairValues::UNASSISTABLE`].
pub fn evaluate(table: &AssistPairTable, helper: EntityId, target: AssistTarget<'_>) -> PairValues {
    let Some(entry) = table.entry(target.descriptor, target.life_state) else {
        return PairValues::UNASSISTABLE;
    };
    if let Some(values) = entry.literal {
        return values;
    }
    let selected = table
        .scope
        .as_ref()
        .is_some_and(|scope| scope.select(helper, target.pawn));
    let radius = match evaluate_field(table, entry, Field::Radius, selected).and_then(radius_result)
    {
        Some(radius) => radius,
        None => {
            warn_fault(entry, Field::Radius);
            0.0
        }
    };
    let required_ticks =
        match evaluate_field(table, entry, Field::HoldMs, selected).and_then(hold_result) {
            Some(required_ticks) => required_ticks,
            None => {
                warn_fault(entry, Field::HoldMs);
                0
            }
        };
    PairValues {
        radius,
        required_ticks,
    }
}

/// One field's raw value, or `None` on a fault: a missing side value or
/// non-finite arithmetic.
fn evaluate_field(
    table: &AssistPairTable,
    entry: &BoundAssistable,
    field: Field,
    selected: bool,
) -> Option<f32> {
    let value = match field {
        Field::Radius => &entry.radius,
        Field::HoldMs => &entry.hold_ms,
    };
    match value {
        BoundValue::Literal(value) => Some(*value),
        BoundValue::Expr { program, inputs } => {
            let scope = table.scope.as_ref()?;
            if !selected || !scope.observes(inputs) {
                return None;
            }
            match eval_value_checked(program, scope) {
                Ok(IrValue::Number(value)) if value.is_finite() => Some(value),
                _ => None,
            }
        }
    }
}

fn warn_fault(entry: &BoundAssistable, field: Field) {
    let warned = entry.warned.get();
    if warned & field.bit() != 0 {
        return;
    }
    entry.warned.set(warned | field.bit());
    let (cause, outcome) = match field {
        Field::Radius => (
            "a non-finite result or a missing side value",
            "the pair is out of range",
        ),
        Field::HoldMs => (
            "a negative or non-finite result or a missing side value",
            "assist progress is kept, not advanced",
        ),
    };
    log::warn!(
        "[Assist] descriptor `{}` `components.lifeStates.states.{}.assistable.{}` faulted with {cause}; {outcome}",
        entry.descriptor,
        entry.state_name,
        field.key(),
    );
}

#[cfg(test)]
#[path = "assist_pairs_tests.rs"]
mod tests;
