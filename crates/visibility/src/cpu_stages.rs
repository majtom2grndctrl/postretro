// Visibility's CPU stage set: portal-walk time, traversal counters, path markers.
// See: context/lib/rendering_pipeline.md §12

use postretro_stage_timing::{StageFrame, StageKind, StageSet};

use crate::portal_vis::PortalTraversalStats;

/// Stages visibility reports per frame. The binary places the roots under its
/// own visibility stage.
///
/// A walk frame (the portal path, including one that trips the step limit)
/// records `portal_walk` and every counter, zeros included. A frame on any
/// other path records only `portal_fallback`, so walk averages never mix in
/// fallback frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityStage {
    /// Time inside the portal flood only.
    PortalWalk,
    Considered,
    Accepted,
    RejectedBlocked,
    RejectedSolid,
    RejectedClipped,
    RejectedNarrow,
    RejectedInvalid,
    RejectedPathCycle,
    RejectedDepthLimit,
    /// The walk tripped its step budget; the frame drew through AABB culling.
    StepLimit,
    /// The frame took a non-portal path (solid or exterior camera, no portal
    /// data, empty world). No walk ran.
    PortalFallback,
}

impl StageSet for VisibilityStage {
    const ALL: &'static [Self] = &[
        Self::PortalWalk,
        Self::Considered,
        Self::Accepted,
        Self::RejectedBlocked,
        Self::RejectedSolid,
        Self::RejectedClipped,
        Self::RejectedNarrow,
        Self::RejectedInvalid,
        Self::RejectedPathCycle,
        Self::RejectedDepthLimit,
        Self::StepLimit,
        Self::PortalFallback,
    ];

    fn index(self) -> usize {
        self as usize
    }

    fn label(self) -> &'static str {
        match self {
            Self::PortalWalk => "portal_walk",
            Self::Considered => "walk_considered",
            Self::Accepted => "walk_accepted",
            Self::RejectedBlocked => "walk_rejected_blocked",
            Self::RejectedSolid => "walk_rejected_solid",
            Self::RejectedClipped => "walk_rejected_clipped",
            Self::RejectedNarrow => "walk_rejected_narrow",
            Self::RejectedInvalid => "walk_rejected_invalid",
            Self::RejectedPathCycle => "walk_rejected_cycle",
            Self::RejectedDepthLimit => "walk_rejected_depth",
            Self::StepLimit => "walk_step_limit",
            Self::PortalFallback => "portal_fallback",
        }
    }

    fn parent(self) -> Option<Self> {
        match self {
            Self::PortalWalk | Self::StepLimit | Self::PortalFallback => None,
            _ => Some(Self::PortalWalk),
        }
    }

    fn kind(self) -> StageKind {
        match self {
            Self::PortalWalk => StageKind::Time,
            Self::StepLimit | Self::PortalFallback => StageKind::Marker,
            _ => StageKind::Count,
        }
    }
}

/// Records one completed walk: its flood time and every traversal counter.
pub(crate) fn record_walk(
    frame: &StageFrame<VisibilityStage>,
    stats: &PortalTraversalStats,
    walk_nanos: u64,
) {
    frame.add_nanos(VisibilityStage::PortalWalk, walk_nanos);
    for (stage, count) in [
        (VisibilityStage::Considered, stats.considered),
        (VisibilityStage::Accepted, stats.accepted),
        (
            VisibilityStage::RejectedBlocked,
            stats.rejected_blocked_portal,
        ),
        (VisibilityStage::RejectedSolid, stats.rejected_solid),
        (VisibilityStage::RejectedClipped, stats.rejected_clipped),
        (VisibilityStage::RejectedNarrow, stats.rejected_narrow),
        (VisibilityStage::RejectedInvalid, stats.rejected_invalid),
        (
            VisibilityStage::RejectedPathCycle,
            stats.rejected_path_cycle,
        ),
        (
            VisibilityStage::RejectedDepthLimit,
            stats.rejected_depth_limit,
        ),
    ] {
        frame.add_count(stage, u64::from(count));
    }
    if stats.step_limit_hit {
        frame.mark(VisibilityStage::StepLimit);
    }
}
