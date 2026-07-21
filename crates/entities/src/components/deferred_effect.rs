// Per-entity deferred impact-effect storage.
// See: context/lib/entity_model.md §2 · context/lib/scripting.md §11.

use serde::{Deserialize, Serialize};

/// One deferred impact effect awaiting a future fixed tick.
///
/// Entries stay in insertion order. The game-logic executor owns countdown
/// advancement and terminal removal semantics; this component is deliberately
/// data-only so non-brain entities can defer effects too.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingEffect {
    pub kind: DeferredEffectKind,
    pub countdown_ms: f32,
    /// The absolute health value carried by a deferred `setHealth` write.
    /// `despawn` has no payload.
    pub value: Option<f32>,
}

/// Closed deferred-effect vocabulary. Immediate presentation work does not
/// enter this queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeferredEffectKind {
    Despawn,
    SetHealth,
}

/// Runtime state for impact effects that outlive the current dispatch.
///
/// Every entity receives this component at spawn. `inert` means the entity is
/// bound for the frame-end removal pass, so AI and steering leave it alone while
/// an in-tick presentation effect can still address its live id.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeferredEffectComponent {
    pub pending: Vec<PendingEffect>,
    pub inert: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_component_is_active_and_has_no_pending_effects() {
        let effects = DeferredEffectComponent::default();
        assert!(!effects.inert);
        assert!(effects.pending.is_empty());
    }
}
