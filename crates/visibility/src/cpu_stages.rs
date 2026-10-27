// Visibility's CPU stage set: portal-walk time, traversal counters, path markers.
// See: context/lib/rendering_pipeline.md §12

use postretro_stage_timing::{StageFrame, StageKind, StageSet};

use crate::region_walk::RegionWalkStats;

/// Stages visibility reports per frame. The binary places the roots under its
/// own visibility stage.
///
/// A walk frame (the portal path, including one that trips the step limit)
/// records `portal_walk` and every counter, zeros included. A frame on any
/// other path records only `portal_fallback`, so walk averages never mix in
/// fallback frames. The mirror's walk records `mirror_walk` alone, on every
/// frame it walks from a usable seed, a tripped walk included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityStage {
    /// Time inside the portal walk only.
    PortalWalk,
    /// Time inside the planar water mirror's seeded walk only, separate from
    /// the camera walk.
    MirrorWalk,
    Considered,
    Accepted,
    RejectedBlocked,
    RejectedSolid,
    RejectedClipped,
    RejectedInvalid,
    /// The walk tripped its step cap; the frame drew through AABB culling.
    StepLimit,
    /// The frame took a non-portal path (solid or exterior camera, no portal
    /// data, empty world). No walk ran.
    PortalFallback,
}

impl StageSet for VisibilityStage {
    const ALL: &'static [Self] = &[
        Self::PortalWalk,
        Self::MirrorWalk,
        Self::Considered,
        Self::Accepted,
        Self::RejectedBlocked,
        Self::RejectedSolid,
        Self::RejectedClipped,
        Self::RejectedInvalid,
        Self::StepLimit,
        Self::PortalFallback,
    ];

    fn index(self) -> usize {
        self as usize
    }

    fn label(self) -> &'static str {
        match self {
            Self::PortalWalk => "portal_walk",
            Self::MirrorWalk => "mirror_walk",
            Self::Considered => "walk_considered",
            Self::Accepted => "walk_accepted",
            Self::RejectedBlocked => "walk_rejected_blocked",
            Self::RejectedSolid => "walk_rejected_solid",
            Self::RejectedClipped => "walk_rejected_clipped",
            Self::RejectedInvalid => "walk_rejected_invalid",
            Self::StepLimit => "walk_step_limit",
            Self::PortalFallback => "portal_fallback",
        }
    }

    fn parent(self) -> Option<Self> {
        match self {
            Self::PortalWalk | Self::MirrorWalk | Self::StepLimit | Self::PortalFallback => None,
            _ => Some(Self::PortalWalk),
        }
    }

    fn kind(self) -> StageKind {
        match self {
            Self::PortalWalk | Self::MirrorWalk => StageKind::Time,
            Self::StepLimit | Self::PortalFallback => StageKind::Marker,
            _ => StageKind::Count,
        }
    }
}

/// Records one walk, a tripped one included: its time and every traversal
/// counter. A portal test rejected for facing away from the eye counts toward
/// `walk_considered` alone.
pub(crate) fn record_walk(
    frame: &StageFrame<VisibilityStage>,
    stats: &RegionWalkStats,
    walk_nanos: u64,
) {
    frame.add_nanos(VisibilityStage::PortalWalk, walk_nanos);
    for (stage, count) in [
        (VisibilityStage::Considered, stats.considered),
        (VisibilityStage::Accepted, stats.accepted),
        (VisibilityStage::RejectedBlocked, stats.rejected_blocked),
        (VisibilityStage::RejectedSolid, stats.rejected_solid),
        (VisibilityStage::RejectedClipped, stats.rejected_clipped),
        (VisibilityStage::RejectedInvalid, stats.rejected_invalid),
    ] {
        frame.add_count(stage, u64::from(count));
    }
    if stats.step_limit_hit {
        frame.mark(VisibilityStage::StepLimit);
    }
}
