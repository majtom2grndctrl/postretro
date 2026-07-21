// Per-instance modder-owned numeric state storage.
// See: context/lib/scripting.md §12.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Per-entity numeric fields authored through impact policies.
///
/// Every entity receives an empty component at spawn. Fields are intentionally
/// emergent: writing a name creates it, while an absent name reads as zero.
/// There is no descriptor surface or schema whitelist, so an entity may gain
/// new state during play without a respawn.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EntityStateComponent {
    values: HashMap<String, f32>,
}

impl EntityStateComponent {
    /// Read a field, using the IR's total numeric default for an unset name.
    pub fn get(&self, name: &str) -> f32 {
        self.values.get(name).copied().unwrap_or(0.0)
    }

    /// Write a field, creating it when this is the first write for `name`.
    pub fn set(&mut self, name: impl Into<String>, value: f32) {
        self.values.insert(name.into(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_number_approx_eq(actual: f32, expected: f32) {
        const EPSILON: f32 = 1.0e-6;
        assert!(
            (actual - expected).abs() <= EPSILON,
            "expected {expected} ± {EPSILON}, got {actual}"
        );
    }

    #[test]
    fn absent_field_reads_as_zero_and_first_write_creates_it() {
        let mut state = EntityStateComponent::default();
        assert_number_approx_eq(state.get("hits"), 0.0);

        state.set("hits", 3.0);
        assert_number_approx_eq(state.get("hits"), 3.0);
    }
}
