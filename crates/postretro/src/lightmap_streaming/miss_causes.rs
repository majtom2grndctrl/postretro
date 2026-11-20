//! Lightmap visible-miss causes: per-block verdicts and the frame's count.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_loader::LightmapBlockClass;
use postretro_renderer::LightmapMissBuckets;

use super::*;
use crate::streaming::drain_budget::DrainRank;

/// Verdicts a drain or an outcome records on a block, valid for one counted
/// frame: a mark from an earlier epoch reads as unset, so a frame's verdicts
/// reset without touching any block.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct FrameMarks {
    epoch: u32,
    flags: u8,
}

impl FrameMarks {
    /// Offered to one of this frame's drains.
    pub(super) const OFFERED: u8 = 1;
    /// This frame's latest drain to carry the pair handed it back unplaced.
    pub(super) const DEFERRED: u8 = 1 << 1;
    /// A read, decode or install failed this frame, or the drain carrying the
    /// pair rolled back.
    pub(super) const FAILED: u8 = 1 << 2;

    pub(super) fn mark(&mut self, epoch: u32, flag: u8) {
        if self.epoch != epoch {
            *self = Self { epoch, flags: 0 };
        }
        self.flags |= flag;
    }

    fn has(self, epoch: u32, flag: u8) -> bool {
        self.epoch == epoch && self.flags & flag != 0
    }
}

/// Why the latest request pass stopped before reaching every targeted block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestWait {
    /// Every permit held, or the band's half of them.
    Permit,
    /// The in-hand pair bytes, or the band's half of them, would overflow.
    InHandCap,
    /// The lightmap pool's band headroom has no room for the next band pair.
    PoolRoom,
}

/// The latest request pass's verdict on the blocks it did not submit. It
/// stands until the next pass runs, so a frame whose outcome frees in-hand
/// bytes after its pass still reports the pass's wait.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct RequestPassVerdict {
    /// The wait that ended the pass and the rank of the first block it left:
    /// every block at or past that rank was not reached.
    stop: Option<(RequestWait, DrainRank)>,
    /// The first band pair with planes the full mask pool skipped; the pass
    /// skipped every later one too.
    mask_room_from: Option<DrainRank>,
}

impl RequestPassVerdict {
    pub(super) fn stop(&mut self, wait: RequestWait, rank: DrainRank) {
        self.stop = Some((wait, rank));
    }

    pub(super) fn skip_for_mask_room(&mut self, rank: DrainRank) {
        self.mask_room_from.get_or_insert(rank);
    }
}

/// One miss's cause; each names its [`LightmapMissBuckets`] field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MissCause {
    OutsideReach,
    ReadInFlight,
    WaitingOnPermit,
    WaitingOnInHandCap,
    HeldByDrainBudget,
    AwaitingRendererDrain,
    DeferredByPool,
    Refused,
    Failed,
}

impl MissCause {
    fn bucket(self, buckets: &mut LightmapMissBuckets) -> &mut u64 {
        match self {
            Self::OutsideReach => &mut buckets.outside_reach,
            Self::ReadInFlight => &mut buckets.read_in_flight,
            Self::WaitingOnPermit => &mut buckets.waiting_on_permit,
            Self::WaitingOnInHandCap => &mut buckets.waiting_on_in_hand_cap,
            Self::HeldByDrainBudget => &mut buckets.held_by_drain_budget,
            Self::AwaitingRendererDrain => &mut buckets.awaiting_renderer_drain,
            Self::DeferredByPool => &mut buckets.deferred_by_pool,
            Self::Refused => &mut buckets.refused,
            Self::Failed => &mut buckets.failed,
        }
    }
}

impl From<RequestWait> for MissCause {
    fn from(wait: RequestWait) -> Self {
        match wait {
            RequestWait::Permit => Self::WaitingOnPermit,
            RequestWait::InHandCap => Self::WaitingOnInHandCap,
            RequestWait::PoolRoom => Self::Refused,
        }
    }
}

impl LightmapResidencyController {
    /// Counts the latest frame's drawn blocks into the visible-miss
    /// buckets, once, then starts the next frame's verdicts. Call after the
    /// frame's drain outcome is applied, so a block that drain installed is
    /// resident for the frame that draws it. Outside the baked set counts
    /// every drawn block not mandatory at lead L, resident or not; not
    /// resident counts every drawn block not installed, whatever its class,
    /// and splits by cause. A frame that draws no cell, or one already
    /// counted, reports zero.
    pub(crate) fn count_visible_misses(&mut self) {
        let (mut outside, mut not_resident) = (0u32, 0u32);
        let mut frame = LightmapMissBuckets::default();
        if std::mem::take(&mut self.misses_due) {
            for &block in self.demand.drawn_blocks() {
                let slot = &self.slots[block as usize];
                if slot
                    .target
                    .is_none_or(|target| target.class != LightmapBlockClass::Mandatory)
                {
                    outside += 1;
                }
                if slot.phase != BlockPhase::Installed {
                    not_resident += 1;
                    let cause = self.miss_cause(block);
                    *cause.bucket(&mut frame) += 1;
                    *cause.bucket(&mut self.counters.miss_buckets) += 1;
                }
            }
        }
        let counters = &mut self.counters;
        counters.drawn_outside_baked_set += u64::from(outside);
        counters.drawn_not_resident += u64::from(not_resident);
        counters.last_frame_drawn_outside_baked_set = outside;
        counters.last_frame_drawn_not_resident = not_resident;
        counters.last_frame_miss_buckets = frame;
        self.miss_epoch = self.miss_epoch.wrapping_add(1);
    }

    /// Marks `block` with this frame's `flag` (a [`FrameMarks`] constant).
    pub(super) fn mark_frame(&mut self, block: u32, flag: u8) {
        let epoch = self.miss_epoch;
        self.slots[block as usize].marks.mark(epoch, flag);
    }

    /// The cause of a drawn block's miss. A failure this frame comes first;
    /// otherwise the block's phase, and for a block not yet read, what kept
    /// the frame from reading it. The caller never asks of an installed
    /// block.
    fn miss_cause(&self, block: u32) -> MissCause {
        let slot = &self.slots[block as usize];
        let marked = |flag| slot.marks.has(self.miss_epoch, flag);
        if marked(FrameMarks::FAILED) {
            return MissCause::Failed;
        }
        match slot.phase {
            BlockPhase::Failed => MissCause::Failed,
            BlockPhase::Refused => MissCause::Refused,
            BlockPhase::InFlight => MissCause::ReadInFlight,
            BlockPhase::InDrain => MissCause::AwaitingRendererDrain,
            BlockPhase::Ready if marked(FrameMarks::DEFERRED) => MissCause::DeferredByPool,
            BlockPhase::Ready if marked(FrameMarks::OFFERED) => MissCause::HeldByDrainBudget,
            BlockPhase::Ready => MissCause::AwaitingRendererDrain,
            BlockPhase::Absent | BlockPhase::Installed => self.unread_cause(block),
        }
    }

    /// Why an absent drawn block was not read: the frame may not read it, or
    /// the latest request pass stopped before it. A targeted block the pass
    /// would read waits for the issuer, as SH's does.
    fn unread_cause(&self, block: u32) -> MissCause {
        let Some(target) = self.slots[block as usize].target else {
            return MissCause::OutsideReach;
        };
        if !self.may_request
            || (self.camera_set_only && !self.demand.in_camera_set(&self.map, block))
        {
            return MissCause::OutsideReach;
        }
        let rank = target.rank(block);
        let verdict = &self.request_verdict;
        match verdict.stop {
            Some((wait, at)) if rank >= at => wait.into(),
            _ if verdict.mask_room_from.is_some_and(|from| rank >= from)
                && target.class == LightmapBlockClass::Band
                && self.map.facts(block).texels.mask != 0 =>
            {
                MissCause::Refused
            }
            _ => MissCause::ReadInFlight,
        }
    }
}
