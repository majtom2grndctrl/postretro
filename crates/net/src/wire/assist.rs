// Assist facts on the local-pawn record: the recipient's pair table as helper
// and the required ticks it receives as target.
// See: context/lib/networking.md §Version gates

use bitcode::{Decode, Encode};

use super::{RawEntityRecord, ValidationError};

/// The widest pair radius a record may carry, in metres. Declared here because
/// this crate depends on no engine crate; a drift guard in `postretro-netcode`
/// pins it to the engine's `ASSIST_MAX_RADIUS_M`.
pub const WIRE_ASSIST_MAX_RADIUS: f32 = 16.0;

/// The largest required tick count a record may carry: the 60 s hold cap at
/// 60 Hz. A drift guard in `postretro-netcode` pins it to
/// `assist::required_count(ASSIST_MAX_HOLD_MS)`.
pub const WIRE_ASSIST_MAX_REQUIRED_TICKS: u32 = 3600;

/// One pair-table entry: the helper's values against one other pawn whose
/// state declares `assistable`. Radius 0 means the helper cannot assist that
/// pawn; `required_ticks` 0 means held progress is kept, not advanced.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WireAssistPair {
    pub network_id: u32,
    pub radius: f32,
    pub required_ticks: u32,
}

/// A helper's pair table, sorted by `network_id`, one entry per other
/// assistable pawn. Empty means no assistable candidate.
pub type WireAssistPairs = Vec<WireAssistPair>;

/// The assist facts one local-pawn record carries for its recipient.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WireAssistFacts {
    pub pairs: WireAssistPairs,
    /// The required ticks of the highest-progress helper on the recipient's
    /// pawn; 0 when no assist is received.
    pub received_required_ticks: u32,
}

impl WireAssistFacts {
    /// Whether every value lies inside the wire bounds.
    #[must_use]
    pub fn in_bounds(&self) -> bool {
        self.received_required_ticks <= WIRE_ASSIST_MAX_REQUIRED_TICKS
            && self.pairs.iter().all(|pair| {
                pair.radius.is_finite()
                    && (0.0..=WIRE_ASSIST_MAX_RADIUS).contains(&pair.radius)
                    && pair.required_ticks <= WIRE_ASSIST_MAX_REQUIRED_TICKS
            })
    }

    /// Write these facts onto `record` behind its flag.
    pub fn stamp(&self, record: &mut RawEntityRecord) {
        record.has_assist_facts = true;
        record.assist_pairs = self.pairs.clone();
        record.received_required_ticks = self.received_required_ticks;
    }
}

impl RawEntityRecord {
    /// Whether this record carries any assist field, flagged or not.
    pub(super) fn carries_assist_fields(&self) -> bool {
        self.has_assist_facts || !self.assist_pairs.is_empty() || self.received_required_ticks != 0
    }

    /// Validate the assist facts of a live record. They ride only the
    /// recipient's local-pawn record, behind their flag, inside the bounds.
    pub(super) fn validate_assist_facts(&self) -> Result<Option<WireAssistFacts>, ValidationError> {
        if !self.has_assist_facts {
            if self.carries_assist_fields() {
                return Err(ValidationError::InvalidAssistFacts);
            }
            return Ok(None);
        }
        if !self.local_player {
            return Err(ValidationError::MovementMetadataWithoutMovement);
        }
        let facts = WireAssistFacts {
            pairs: self.assist_pairs.clone(),
            received_required_ticks: self.received_required_ticks,
        };
        if !facts.in_bounds() {
            return Err(ValidationError::InvalidAssistFacts);
        }
        Ok(Some(facts))
    }
}

#[cfg(test)]
#[path = "assist_tests.rs"]
mod tests;
