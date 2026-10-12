// Output-only per-pawn record of the last authoritative movement tick's resolved immersion.
// See: context/lib/entity_model.md §7b · context/lib/scripting.md §5 (Per-player engine slots)

use serde::{Deserialize, Serialize};

/// The fraction of the pawn's capsule, feet to head, its winning fluid covered
/// on the last authoritative movement tick, 0..=1 (0 when dry). It is the
/// tick-start resolve that tick's state intent used.
///
/// Output only: host movement writes it after each tick, and only the
/// per-player `player.immersion` slot reads it. No state intent, reconcile
/// replay or wire merge reads it, which is why it is its own component rather
/// than a field on [`crate::PlayerMovementComponent`]: that struct is cloned
/// into replay and merged from the wire.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ImmersionComponent {
    pub immersion: f32,
}
