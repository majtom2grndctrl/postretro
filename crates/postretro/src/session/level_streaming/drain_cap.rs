//! The budget each drain admits against, and the developer override of the
//! play budget: one seam the dev-tools slider and the walk measurement both set.
//! See: context/lib/rendering_pipeline.md §4 · context/lib/experimental_spikes.md §Tuning levers

#[cfg(any(test, feature = "dev-tools"))]
use std::num::NonZeroU64;

use super::LevelStreaming;
#[cfg(feature = "dev-tools")]
use crate::streaming::drain_budget::MAX_INSTALL_DECODED_BYTES_PER_DRAIN;
use crate::streaming::drain_budget::{DrainBudget, DrainBytesOverflow};

/// A developer lever's play budget in place of production. The byte cap is
/// the dev-tools slider's and the walk's; the time budget is the walk's alone
/// and builds only for tests.
#[cfg(any(test, feature = "dev-tools"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlayBudgetOverride {
    /// A decoded-byte cap.
    DecodedBytes(NonZeroU64),
    /// An estimated install-time budget in µs.
    #[cfg(test)]
    EstimatedMicros(NonZeroU64),
}

#[cfg(any(test, feature = "dev-tools"))]
impl PlayBudgetOverride {
    fn budget(self) -> DrainBudget {
        match self {
            Self::DecodedBytes(cap) => DrainBudget::DecodedBytes(cap.get()),
            #[cfg(test)]
            Self::EstimatedMicros(micros) => DrainBudget::EstimatedMicros(micros.get()),
        }
    }

    fn decoded_bytes(self) -> Option<NonZeroU64> {
        match self {
            Self::DecodedBytes(cap) => Some(cap),
            #[cfg(test)]
            Self::EstimatedMicros(_) => None,
        }
    }
}

impl LevelStreaming {
    /// Admits the drain's offered items once, against the settling budget
    /// while Settling and the play budget otherwise.
    pub(super) fn admit_offered(&mut self, settling: bool) -> Result<(), DrainBytesOverflow> {
        let budget = if settling {
            DrainBudget::SETTLING
        } else {
            self.play_drain_budget()
        };
        self.drain.admit_within(budget)
    }

    #[cfg(not(any(test, feature = "dev-tools")))]
    fn play_drain_budget(&self) -> DrainBudget {
        DrainBudget::PLAY
    }

    #[cfg(any(test, feature = "dev-tools"))]
    fn play_drain_budget(&self) -> DrainBudget {
        self.play_budget_override
            .map_or(DrainBudget::PLAY, PlayBudgetOverride::budget)
    }

    /// The play byte cap override; `None` is production or a time override.
    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn play_drain_cap_override(&self) -> Option<NonZeroU64> {
        self.play_budget_override
            .and_then(PlayBudgetOverride::decoded_bytes)
    }

    /// Replaces the play budget with a byte cap for every later play drain;
    /// `None` restores production. Settling and the lightmap in-hand caps
    /// never read it.
    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn set_play_drain_cap_override(&mut self, cap: Option<NonZeroU64>) {
        self.play_budget_override = cap.map(PlayBudgetOverride::DecodedBytes);
    }

    /// The play time budget override in µs; `None` is production or a byte
    /// override.
    #[cfg(test)]
    pub(crate) fn play_drain_time_override(&self) -> Option<NonZeroU64> {
        match self.play_budget_override {
            Some(PlayBudgetOverride::EstimatedMicros(micros)) => Some(micros),
            _ => None,
        }
    }

    /// Replaces the play budget with an estimated install-time budget in µs
    /// for every later play drain; `None` restores production. Settling and
    /// the lightmap in-hand caps never read it.
    #[cfg(test)]
    pub(crate) fn set_play_drain_time_override(&mut self, micros: Option<NonZeroU64>) {
        self.play_budget_override = micros.map(PlayBudgetOverride::EstimatedMicros);
    }
}

/// The dev-tools Streaming tab's view of the override.
#[cfg(feature = "dev-tools")]
impl LevelStreaming {
    pub(crate) fn drain_cap_lever(&self) -> crate::render::debug_ui::StreamingDrainCapLever {
        crate::render::debug_ui::StreamingDrainCapLever {
            cap_bytes: self.play_drain_cap_override().map(NonZeroU64::get),
            production_bytes: MAX_INSTALL_DECODED_BYTES_PER_DRAIN,
        }
    }

    pub(crate) fn apply_drain_cap_lever(
        &mut self,
        lever: &crate::render::debug_ui::StreamingDrainCapLever,
    ) {
        self.set_play_drain_cap_override(lever.cap_bytes.and_then(NonZeroU64::new));
    }
}
