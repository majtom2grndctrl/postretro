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
    /// `unsettled` is a session's count, `None` before its first demand
    /// update; `None` for the session itself means the resource does not
    /// stream.
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

impl crate::session::Session {
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

    // P1: an empty target list before the first demand update is "not yet
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
