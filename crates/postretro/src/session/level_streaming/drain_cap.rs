//! The budget each drain admits against, and the spike's override of the
//! play cap: one seam the dev-tools slider and the walk measurement both set.
//! See: context/lib/rendering_pipeline.md §4 · context/lib/experimental_spikes.md §Tuning levers

#[cfg(any(test, feature = "dev-tools"))]
use std::num::NonZeroU64;

use super::LevelStreaming;
use crate::streaming::drain_budget::{
    DrainBytesOverflow, MAX_INSTALL_DECODED_BYTES_PER_DRAIN,
    SETTLING_INSTALL_DECODED_BYTES_PER_DRAIN,
};

impl LevelStreaming {
    /// Admits the drain's offered items once, against the settling budget
    /// while Settling and the play cap otherwise.
    pub(super) fn admit_offered(&mut self, settling: bool) -> Result<(), DrainBytesOverflow> {
        let budget = if settling {
            SETTLING_INSTALL_DECODED_BYTES_PER_DRAIN
        } else {
            self.play_drain_cap()
        };
        self.drain.admit_within(budget)
    }

    #[cfg(not(any(test, feature = "dev-tools")))]
    fn play_drain_cap(&self) -> u64 {
        MAX_INSTALL_DECODED_BYTES_PER_DRAIN
    }

    #[cfg(any(test, feature = "dev-tools"))]
    fn play_drain_cap(&self) -> u64 {
        self.play_drain_cap_override
            .map_or(MAX_INSTALL_DECODED_BYTES_PER_DRAIN, NonZeroU64::get)
    }

    /// The play cap override; `None` is the production cap.
    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn play_drain_cap_override(&self) -> Option<NonZeroU64> {
        self.play_drain_cap_override
    }

    /// Replaces the play cap for every later play drain; `None` restores
    /// production. Settling and the lightmap in-hand caps never read it.
    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn set_play_drain_cap_override(&mut self, cap: Option<NonZeroU64>) {
        self.play_drain_cap_override = cap;
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
