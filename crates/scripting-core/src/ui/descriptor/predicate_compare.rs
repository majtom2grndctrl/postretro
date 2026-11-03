// Numeric comparison for a UI `Predicate`: `{ slot, compare: { op, value } }`.
// The one ordered test beside `equals`, so a node can show "while a Number slot
// is above 0" (e.g. a revive label while `player.assistProgress > 0`).
// See: context/lib/ui.md §3

use serde::{Deserialize, Serialize};

/// The ordered relation a [`PredicateCompare`] tests the bound value against.
/// Wire names are camelCase words, matching the SDK's `stateCompare` op strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompareOp {
    /// Strictly greater than the threshold.
    GreaterThan,
    /// Strictly less than the threshold.
    LessThan,
    /// Greater than or equal to the threshold.
    AtLeast,
    /// Less than or equal to the threshold.
    AtMost,
}

impl CompareOp {
    /// Every op, in wire-name order used by diagnostics.
    pub const ALL: [CompareOp; 4] = [
        CompareOp::GreaterThan,
        CompareOp::LessThan,
        CompareOp::AtLeast,
        CompareOp::AtMost,
    ];

    /// The stable wire / SDK name.
    pub const fn name(self) -> &'static str {
        match self {
            CompareOp::GreaterThan => "greaterThan",
            CompareOp::LessThan => "lessThan",
            CompareOp::AtLeast => "atLeast",
            CompareOp::AtMost => "atMost",
        }
    }

    /// Parse a wire / SDK name; `None` for anything else.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|op| op.name() == name)
    }
}

/// A numeric comparison carried by a [`super::Predicate`]: true while the bound
/// Number value stands in `op` relation to `value`. Mutually exclusive with
/// `equals`; the bridges reject both together and a non-finite `value` at load.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PredicateCompare {
    pub op: CompareOp,
    pub value: f64,
}

impl PredicateCompare {
    /// Whether `bound` satisfies the comparison. The f32 slot value widens to
    /// f64 exactly. A NaN bound satisfies no op, so it reads as false.
    pub fn holds(&self, bound: f32) -> bool {
        let bound = f64::from(bound);
        match self.op {
            CompareOp::GreaterThan => bound > self.value,
            CompareOp::LessThan => bound < self.value,
            CompareOp::AtLeast => bound >= self.value,
            CompareOp::AtMost => bound <= self.value,
        }
    }
}

/// The diagnostic both bridges raise for an unrecognised op name.
pub fn unknown_compare_op_reason(field: &str) -> String {
    format!("`{field}.compare.op` must be one of `greaterThan`, `lessThan`, `atLeast`, `atMost`")
}

/// Load-time checks shared by both bridges, keyed by the predicate's full path
/// (e.g. ``UI tree `root`.children[1].visibleWhen``): `compare` excludes
/// `equals`, its threshold is finite, and a slot the engine catalog declares
/// must be a Number slot. Mod store slots are not in the catalog; the SDK
/// `stateCompare` constructor rejects a non-number ref for those.
pub fn validate_predicate_compare(
    slot: Option<&str>,
    has_equals: bool,
    compare: &PredicateCompare,
    path: &str,
) -> Result<(), String> {
    if has_equals {
        return Err(format!(
            "{path}: `equals` and `compare` are mutually exclusive"
        ));
    }
    if !compare.value.is_finite() {
        return Err(format!("{path}.compare.value must be a finite number"));
    }
    if let Some(slot) = slot
        && let Some(kind) = engine_slot_non_number_kind(slot)
    {
        return Err(format!(
            "{path}.compare: slot `{slot}` is a {kind} slot; a numeric comparison needs a Number slot"
        ));
    }
    Ok(())
}

/// The declared type name of an engine-catalog slot that is not a Number, or
/// `None` for a Number slot or a slot the catalog does not declare.
fn engine_slot_non_number_kind(slot: &str) -> Option<&'static str> {
    use postretro_entities::{EngineStateCatalog, EngineStateValueType, engine_state_catalog};
    use std::sync::OnceLock;

    static CATALOG: OnceLock<Option<EngineStateCatalog>> = OnceLock::new();
    let catalog = CATALOG
        .get_or_init(|| engine_state_catalog().ok())
        .as_ref()?;
    let entry = catalog
        .entries()
        .iter()
        .find(|entry| entry.wire_name == slot)?;
    match entry.value_type {
        EngineStateValueType::Number => None,
        EngineStateValueType::Boolean => Some("Boolean"),
        EngineStateValueType::String => Some("String"),
        EngineStateValueType::Enum { .. } => Some("Enum"),
        EngineStateValueType::Array => Some("Array"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compare(op: CompareOp, value: f64) -> PredicateCompare {
        PredicateCompare { op, value }
    }

    #[test]
    fn ops_hold_at_the_boundary() {
        let above = compare(CompareOp::GreaterThan, 0.0);
        assert!(!above.holds(0.0), "greaterThan 0 is false at 0");
        assert!(
            above.holds(f32::MIN_POSITIVE),
            "greaterThan 0 is true just above 0"
        );
        assert!(!above.holds(-1.0));
        let below = compare(CompareOp::LessThan, 0.5);
        assert!(below.holds(0.25) && !below.holds(0.5));
        let at_least = compare(CompareOp::AtLeast, 0.5);
        assert!(at_least.holds(0.5) && !at_least.holds(0.25));
        let at_most = compare(CompareOp::AtMost, 0.5);
        assert!(at_most.holds(0.5) && !at_most.holds(0.75));
    }

    #[test]
    fn nan_bound_satisfies_no_op() {
        for op in CompareOp::ALL {
            assert!(
                !compare(op, 0.0).holds(f32::NAN),
                "{op:?} must be false on NaN"
            );
        }
    }

    #[test]
    fn op_names_round_trip_through_wire_and_lookup() {
        for op in CompareOp::ALL {
            assert_eq!(CompareOp::from_name(op.name()), Some(op));
            let wire = serde_json::to_string(&op).expect("serialize op");
            assert_eq!(wire, format!("\"{}\"", op.name()));
        }
        assert_eq!(CompareOp::from_name("gt"), None);
    }

    #[test]
    fn validation_rejects_equals_non_finite_and_non_number_engine_slots() {
        let ok = compare(CompareOp::GreaterThan, 0.0);
        validate_predicate_compare(Some("player.assistProgress"), false, &ok, "p")
            .expect("a Number engine slot is accepted");
        validate_predicate_compare(Some("mod.custom"), false, &ok, "p")
            .expect("a slot outside the engine catalog is left to the SDK kind check");
        assert_eq!(
            validate_predicate_compare(Some("player.assistProgress"), true, &ok, "p"),
            Err("p: `equals` and `compare` are mutually exclusive".to_string())
        );
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                validate_predicate_compare(
                    Some("player.assistProgress"),
                    false,
                    &compare(CompareOp::AtLeast, value),
                    "p"
                ),
                Err("p.compare.value must be a finite number".to_string())
            );
        }
        assert_eq!(
            validate_predicate_compare(Some("player.lifeState"), false, &ok, "p"),
            Err(
                "p.compare: slot `player.lifeState` is a String slot; a numeric comparison needs a Number slot"
                    .to_string()
            )
        );
    }
}
