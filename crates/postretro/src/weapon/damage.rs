// The damage primitive exchanged between weapon producers and the health
// chokepoint. Amount-only by design — spatial info rides beside it, never
// inside it. Kept dependency-free so the scripting tree (and the standalone
// `gen-script-types` bin) can reference it without pulling in the weapon
// subsystem's renderer/collision dependencies.
//
// See: context/lib/entity_model.md §2 · context/research/weapon-model.md §7

/// A unit of damage applied to an entity's `Health` component through
/// `apply_damage`. Consumers must not assume it stays amount-only — damage
/// types, resistances, and falloff are future fields.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DamagePayload {
    pub(crate) amount: f32,
}
