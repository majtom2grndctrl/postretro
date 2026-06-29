// Bind/predicate resolution against the per-frame slot + cell snapshot.
// See: context/lib/ui.md §3 (display vs. authoritative value)

use std::collections::HashMap;

use super::super::descriptor::{BindSource, PredicateValue};
use postretro_entities::SlotValue;

use super::CellValues;

/// Resolve a bind's value against the frame's snapshot: a `{ slot }` bind reads
/// the authoritative store slot map; a `{ local }` bind reads the presentation
/// cell `(scope, name)` where `scope` is the nearest declaring ancestor resolved
/// at tree-build time (`None` when the local bind had no enclosing scope, so it
/// degrades to "absent" — the bind silently falls back to its literal). This is
/// the single seam every bind-resolution helper routes through.
pub(crate) fn lookup_bound<'a>(
    source: &BindSource,
    scope: Option<&str>,
    slots: &'a HashMap<String, SlotValue>,
    cells: &'a CellValues,
) -> Option<&'a SlotValue> {
    match source {
        BindSource::Slot { slot } => slots.get(slot),
        BindSource::Local { local } => {
            scope.and_then(|s| cells.get(&(s.to_string(), local.to_string())))
        }
    }
}

/// Resolve a [`Predicate`] against the frame's snapshot to a deterministic
/// `0.0`/`1.0` (M13 G2). This single helper drives BOTH the author-wired highlight
/// (a button's `styleRanges` `bind`) AND the a11y `selected`/`checked` state on the
/// focus-rect readback — each consumer resolves independently (no shared per-node
/// store; the resolve is cheap and deterministic). Semantics:
///
/// - **No `equals`, `Boolean` source** → its truthiness (`1.0` if true, else `0.0`).
/// - **No `equals`, non-`Boolean` source** → `0.0` (a bare value with no comparand
///   has no defined truthiness here).
/// - **With `equals`** → `1.0` iff the resolved `SlotValue` equals the comparand,
///   else `0.0`. `String`/`Enum` match the comparand by name; number/bool match
///   exactly. A type mismatch (e.g. a `Number` slot vs a string comparand) is `0.0`.
/// - **Absent slot/cell** (the bind resolves to nothing) → `0.0`.
pub(crate) fn resolve_predicate(
    source: &BindSource,
    equals: Option<&PredicateValue>,
    scope: Option<&str>,
    slots: &HashMap<String, SlotValue>,
    cells: &CellValues,
) -> f32 {
    let Some(value) = lookup_bound(source, scope, slots, cells) else {
        return 0.0;
    };
    match equals {
        None => match value {
            SlotValue::Boolean(true) => 1.0,
            SlotValue::Boolean(false) => 0.0,
            // A bare (no-`equals`) predicate over a non-boolean source has no
            // defined truthiness — it resolves false.
            _ => 0.0,
        },
        Some(comparand) => {
            if predicate_value_matches(value, comparand) {
                1.0
            } else {
                0.0
            }
        }
    }
}

/// Whether a resolved `SlotValue` equals a `PredicateValue` comparand. Number and
/// boolean compare exactly; a `String` comparand matches both `String` and `Enum`
/// slots by name (an enum's wire form is its name). Any other pairing is a type
/// mismatch and does not match. `rgba`/array comparands are unrepresentable
/// (rejected at load time), so an `Array` slot never matches.
pub(crate) fn predicate_value_matches(value: &SlotValue, comparand: &PredicateValue) -> bool {
    match (value, comparand) {
        // Widen the f32 slot value to f64 for the comparison. Integer-valued
        // comparands (the dominant tab-index/enum case) compare exactly. A
        // fractional authored comparand like 0.1 will silently never match
        // (0.1_f64 != 0.1_f32 as f64 due to differing precision). This is
        // acceptable: authored predicate comparands are integer-valued
        // tab/enum selectors, so fractional mismatches do not arise in practice.
        (SlotValue::Number(n), PredicateValue::Number(c)) => *n as f64 == *c,
        (SlotValue::Boolean(b), PredicateValue::Boolean(c)) => b == c,
        (SlotValue::String(s), PredicateValue::String(c))
        | (SlotValue::Enum(s), PredicateValue::String(c)) => s == c,
        _ => false,
    }
}
