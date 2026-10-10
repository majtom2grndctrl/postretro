//! Query-only hit geometry for a network descriptor display copy.
//! Carries no health, damage ledger, impact policy or AI authority.

use serde::{Deserialize, Serialize};

use super::health::Hitbox;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HitVolumeComponent {
    pub hitbox: Hitbox,
}
