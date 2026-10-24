// Ghost membership for a headless capture: one at-rest evaluation of spawn state.
// See: context/lib/rendering_pipeline.md §7.8 (Frame capture), §7.9

use postretro_entities::EntityRegistry;

use super::clocks::{ClockSource, PoseStamps, PresentationClocks};
use super::eligibility::CandidateView;
use super::{GhostFrame, GhostLinks};

impl GhostLinks {
    /// Evaluate membership once for a single-instant capture: each link at its
    /// installed spawn state and members of links armed on spawn at full
    /// membership (the install settle), at the capture's alpha of 1. A capture
    /// has no ticks to stamp, no presented-frame time and no flash clamp.
    ///
    /// Capture loads no descriptors and never refreshes the opt-ins, so only
    /// tagged movers can ghost in a capture: pawns, descriptor entities and
    /// projectiles never do, whatever their descriptors set.
    pub(crate) fn evaluate_for_capture(&mut self, registry: &EntityRegistry) {
        self.update(
            registry,
            GhostFrame {
                clocks: PresentationClocks::compute(
                    ClockSource::SinglePlayer { completed_ticks: 0 },
                    1.0,
                ),
                alpha: 1.0,
                elapsed_seconds: 0.0,
                view: CandidateView::Authoritative,
                stamps: PoseStamps::None,
            },
        );
    }
}
