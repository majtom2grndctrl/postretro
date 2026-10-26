//! The settle chokepoint: whether every streamed resource holds what a level
//! entry's first frame draws from the presented pose.
//! See: context/lib/boot_sequence.md §1 · context/lib/rendering_pipeline.md §4

/// One streamed resource's answer for the settle pose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourceSettle {
    /// The level has no session for this resource, or it declined.
    NotStreamed,
    /// A session exists but has not derived demand from a view yet. An empty
    /// target set here means "not yet asked", never "nothing to hold".
    NotAsked,
    /// This many settle-set units (SH clusters, lightmap blocks) are not yet
    /// usable.
    Unsettled(usize),
    Settled,
}

impl ResourceSettle {
    /// `session` is `None` when the resource does not stream; otherwise it
    /// carries the session's unsettled count, itself `None` before the
    /// session's first demand update.
    pub(crate) fn of(session: Option<Option<usize>>) -> Self {
        match session {
            None => Self::NotStreamed,
            Some(None) => Self::NotAsked,
            Some(Some(0)) => Self::Settled,
            Some(Some(count)) => Self::Unsettled(count),
        }
    }

    pub(crate) fn is_settled(self) -> bool {
        matches!(self, Self::NotStreamed | Self::Settled)
    }

    /// Units still missing, for the timeout warning; a resource never asked
    /// reports as such rather than as zero.
    pub(crate) fn describe(self) -> String {
        match self {
            Self::NotStreamed => "not streamed".into(),
            Self::NotAsked => "not yet asked".into(),
            Self::Unsettled(count) => format!("{count} unsettled"),
            Self::Settled => "settled".into(),
        }
    }
}

/// Every streamed resource's answer; the level may be revealed once each is
/// settled or does not stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SettleReport {
    pub(crate) sh: ResourceSettle,
    pub(crate) lightmap: ResourceSettle,
}

impl SettleReport {
    pub(crate) fn settled(self) -> bool {
        self.sh.is_settled() && self.lightmap.is_settled()
    }
}

/// How many units each streamed resource's settle set holds, usable or not:
/// what the Settling log's peaks read. `None` for a resource that does not
/// stream or has not derived demand from a view yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SettleSetSize {
    pub(crate) sh_targets: Option<usize>,
    pub(crate) lightmap_blocks: Option<usize>,
}

impl crate::session::Session {
    /// The settle set's size per resource, from the same demand
    /// [`Self::settle_report`] reads.
    pub(crate) fn settle_set_size(&self) -> SettleSetSize {
        SettleSetSize {
            sh_targets: self
                .sh_streaming
                .as_ref()
                .and_then(|streaming| streaming.settle_set_targets()),
            lightmap_blocks: self
                .level_streaming
                .lightmap()
                .and_then(|lightmap| lightmap.settle_set_blocks()),
        }
    }

    /// Lightmap blocks the latest frame drew while not installed; `None`
    /// when the level's lightmap does not stream. Read after the reveal
    /// frame's drain, it is that frame's misses.
    pub(crate) fn last_frame_lightmap_misses(&self) -> Option<u32> {
        self.level_streaming
            .lightmap()
            .map(|lightmap| lightmap.last_frame_drawn_not_resident())
    }

    /// The one settle chokepoint. Asks each streamed resource whether the
    /// settle pose's set is usable: SH clusters Sampleable, lightmap blocks
    /// installed. Call only after the level's sessions exist (install creates
    /// them), so a missing session means the resource does not stream.
    pub(crate) fn settle_report(&self) -> SettleReport {
        SettleReport {
            sh: ResourceSettle::of(
                self.sh_streaming
                    .as_ref()
                    .map(|streaming| streaming.unsettled_targets()),
            ),
            lightmap: ResourceSettle::of(
                self.level_streaming
                    .lightmap()
                    .map(|lightmap| lightmap.unsettled_blocks()),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settle_check_answers_settled_when_no_resource_streams() {
        let report = SettleReport {
            sh: ResourceSettle::of(None),
            lightmap: ResourceSettle::of(None),
        };
        assert!(report.settled());
    }

    // An empty target list before the first demand update is "not yet
    // asked", whatever the session would count.
    #[test]
    fn settle_check_refuses_before_first_settle_pose_update() {
        for (sh, lightmap) in [
            (Some(None), None),
            (None, Some(None)),
            (Some(None), Some(Some(0))),
        ] {
            let report = SettleReport {
                sh: ResourceSettle::of(sh),
                lightmap: ResourceSettle::of(lightmap),
            };
            assert!(!report.settled(), "{report:?}");
        }
    }

    #[test]
    fn settle_check_needs_every_streamed_resource_settled() {
        let settled = SettleReport {
            sh: ResourceSettle::of(Some(Some(0))),
            lightmap: ResourceSettle::of(None),
        };
        assert!(settled.settled());
        let lightmap_pending = SettleReport {
            sh: ResourceSettle::of(Some(Some(0))),
            lightmap: ResourceSettle::of(Some(Some(3))),
        };
        assert!(!lightmap_pending.settled());
        assert_eq!(lightmap_pending.lightmap.describe(), "3 unsettled");
    }
}
