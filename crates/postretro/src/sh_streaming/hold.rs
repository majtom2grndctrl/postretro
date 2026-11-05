//! The SH hold: what a frame off the portal path may demand.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)
//!
//! A frustum-culled path can draw the whole map, so one clip into a wall must
//! not make the map mandatory. A held frame reads only what the camera cell's
//! reach and the pins demand: its drawn clusters add no reads, residents stay,
//! and an absent drawn cluster counts a miss. The lightmap holds the same way
//! (`lightmap_streaming/demand.rs`).

use std::collections::BTreeSet;

use postretro_visibility::VisibleCells;

use super::*;
use crate::streaming::cell_demand::DemandFrame;

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct HoldState {
    /// The latest target update held.
    held: bool,
    /// Monotonic seconds spent in held runs. Hysteresis ages on monotonic
    /// time less this, so a run leaves every timer where it found it.
    paused_seconds: f64,
}

impl ClusterResidencyState {
    /// Resident or on its way: read, being read, or in the renderer's hands.
    /// Only such a cluster can keep a view-driven target on a held frame, so
    /// the target costs no read.
    const fn holds_payload(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Ready | Self::InstalledUncomposed | Self::Sampleable
        )
    }
}

impl TargetClass {
    /// Pins and the reach keep reading through a hold: they follow the camera
    /// cell, not what the frame draws, so the mandatory bound still covers them.
    const fn reads_on_hold(self) -> bool {
        matches!(self, Self::Pinned | Self::Lead | Self::Band)
    }
}

impl ShResidencyController {
    /// Decides whether this frame holds, and returns its time on the
    /// hysteresis clock. A frame holds off the portal path with a usable id
    /// 51, outside full demand: without id 51 only Visible and pins remain, so
    /// a hold would stop SH loading new areas.
    pub(super) fn begin_frame_hold(
        &mut self,
        frame: Option<&DemandFrame<'_>>,
        monotonic_seconds: f64,
    ) -> f64 {
        let held = !self.full_demand
            && frame.is_some_and(|frame| {
                !frame.is_portal_walk()
                    && ShReach::is_usable(frame, &self.topology.hints.cell_to_cluster)
            });
        // The spans into and out of a run count as the run, so a timer resumes
        // on the first portal frame with exactly the time it had left.
        if (held || self.hold.held)
            && let Some(last_time) = self.last_time
        {
            self.hold.paused_seconds += monotonic_seconds - last_time;
        }
        self.hold.held = held;
        monotonic_seconds - self.hold.paused_seconds
    }

    /// This frame's visible clusters, its misses counted on the way.
    ///
    /// Misses count from the frame's real drawn set, which holds the mirror's
    /// cells on a path where `visible`, the set targeting reads, leaves them
    /// out. A held frame targets from the drawn set too, for it reads nothing
    /// and must not free what it draws; it keeps only what is resident or on
    /// its way. The empty world's `DrawAll` draws nothing there, so it cannot
    /// read as every cluster.
    pub(super) fn frame_visible_clusters(
        &mut self,
        visible: &VisibleCells,
        frame: Option<&DemandFrame<'_>>,
    ) -> Result<BTreeSet<u32>, ShResidencyControllerError> {
        let drawn_cells = frame.map_or(visible, |frame| frame.visible_cells);
        let held = self.hold.held;
        let target_cells = if held { drawn_cells } else { visible };
        let mut clusters = if held && frame.is_some_and(|frame| !frame.draws_cells()) {
            BTreeSet::new()
        } else {
            self.visible_clusters(target_cells)?
        };
        self.drawn_outside_targets.clear();
        if !self.full_demand {
            if std::ptr::eq(target_cells, drawn_cells) {
                self.record_visible_misses(&clusters)?;
            } else {
                let drawn = self.visible_clusters(drawn_cells)?;
                self.record_visible_misses(&drawn)?;
                self.drawn_outside_targets = drawn;
            }
        }
        if held {
            clusters.retain(|&cluster_id| self.states[cluster_id as usize].state.holds_payload());
        }
        Ok(clusters)
    }

    /// Whether a target that names no reach or pin may stand for this cluster
    /// this frame: always off a hold, and on one only if it costs no read.
    pub(super) fn may_target_without_reading(&self, cluster_id: u32) -> bool {
        !self.hold.held || self.states[cluster_id as usize].state.holds_payload()
    }

    /// The last word before a read. A held target's read can still be
    /// cancelled back to `Absent` after this frame's targets are set, and it
    /// must not read again until a frame targets it for a class that may.
    pub(super) fn may_read(&self, class: Option<TargetClass>) -> bool {
        !self.hold.held || class.is_some_and(TargetClass::reads_on_hold)
    }

    pub(super) fn is_held(&self) -> bool {
        self.hold.held
    }
}
