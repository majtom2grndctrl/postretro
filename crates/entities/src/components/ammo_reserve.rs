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

    /// Credit a reserve balance, saturating instead of wrapping when repeated
    /// credits exceed `u32::MAX`.
    pub fn credit(&mut self, ammo_type: &str, amount: u32) {
        let balance = self.amounts.entry(ammo_type.to_string()).or_default();
        *balance = balance.saturating_add(amount);
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
