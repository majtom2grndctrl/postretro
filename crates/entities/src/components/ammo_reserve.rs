// Pawn-owned ammunition balances keyed by authored ammo type.
// See: context/lib/entity_model.md §2 (engine components)

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmmoReserve {
    amounts: HashMap<String, u32>,
}

impl AmmoReserve {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn available(&self, ammo_type: &str) -> u32 {
        self.amounts.get(ammo_type).copied().unwrap_or(0)
    }

    /// Visit every stored ammo type and its exact balance.
    ///
    /// Cross-level carry uses this read-only view to restore balances through
    /// [`Self::set_exact`] without exposing the backing map for mutation.
    pub fn balances(&self) -> impl Iterator<Item = (&str, u32)> {
        self.amounts
            .iter()
            .map(|(ammo_type, amount)| (ammo_type.as_str(), *amount))
    }

    /// Credit a reserve balance, saturating instead of wrapping when repeated
    /// credits exceed `u32::MAX`.
    pub fn credit(&mut self, ammo_type: &str, amount: u32) {
        let balance = self.amounts.entry(ammo_type.to_string()).or_default();
        *balance = balance.saturating_add(amount);
    }

    /// Replace one ammo type's balance without affecting any other reserve.
    ///
    /// Cross-level carry restores the authored type's exact carried amount;
    /// crediting would incorrectly add the descriptor's default on every spawn.
    pub fn set_exact(&mut self, ammo_type: &str, amount: u32) {
        self.amounts.insert(ammo_type.to_string(), amount);
    }

    /// Take up to `n` units and return the amount removed. Centralizing the
    /// clamped debit keeps the backing map private and lets reloads consume a
    /// partial final reserve.
    pub fn take(&mut self, ammo_type: &str, n: u32) -> u32 {
        let Some(balance) = self.amounts.get_mut(ammo_type) else {
            return 0;
        };
        let taken = (*balance).min(n);
        *balance -= taken;
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_defaults_to_zero_and_credit_is_type_local() {
        let mut reserve = AmmoReserve::new();
        assert_eq!(reserve.available("shells"), 0);
        reserve.credit("shells", 24);
        reserve.credit("cells", 60);
        assert_eq!(reserve.available("shells"), 24);
        assert_eq!(reserve.available("cells"), 60);
    }

    #[test]
    fn take_atomically_returns_the_amount_removed() {
        let mut reserve = AmmoReserve::new();
        reserve.credit("shells", 8);
        assert_eq!(reserve.take("shells", 3), 3);
        assert_eq!(reserve.available("shells"), 5);
        assert_eq!(reserve.take("shells", 9), 5);
        assert_eq!(reserve.available("shells"), 0);
        assert_eq!(reserve.take("rockets", 1), 0);
    }

    #[test]
    fn repeated_credit_saturates_without_wrapping() {
        let mut reserve = AmmoReserve::new();
        reserve.credit("cells", u32::MAX);
        reserve.credit("cells", 1);
        assert_eq!(reserve.available("cells"), u32::MAX);
    }

    #[test]
    fn set_exact_replaces_one_balance_without_touching_others() {
        let mut reserve = AmmoReserve::new();
        reserve.credit("shells", 24);
        reserve.credit("cells", 60);

        reserve.set_exact("shells", 7);

        assert_eq!(reserve.available("shells"), 7);
        assert_eq!(reserve.available("cells"), 60);
    }

    #[test]
    fn balances_exposes_positive_and_zero_entries_read_only() {
        let mut reserve = AmmoReserve::new();
        reserve.set_exact("shells", 7);
        reserve.set_exact("rockets", 0);

        let mut balances = reserve.balances().collect::<Vec<_>>();
        balances.sort_unstable();

        assert_eq!(balances, vec![("rockets", 0), ("shells", 7)]);
    }

    #[test]
    fn component_value_round_trip_preserves_private_balances() {
        use crate::registry::{Component, ComponentKind, ComponentValue};

        let mut reserve = AmmoReserve::new();
        reserve.credit("cells", 60);
        let value = reserve.into_value();
        assert_eq!(value.kind(), ComponentKind::AmmoReserve);
        assert_eq!(AmmoReserve::KIND, ComponentKind::AmmoReserve);

        let json = serde_json::to_string(&value).unwrap();
        let decoded: ComponentValue = serde_json::from_str(&json).unwrap();
        let ComponentValue::AmmoReserve(decoded) = decoded else {
            panic!("expected ammo reserve component");
        };
        assert_eq!(decoded.available("cells"), 60);
    }
}
