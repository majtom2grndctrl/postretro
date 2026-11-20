// The playback record: a header and one row per recorded frame, filled
// without per-frame allocation and written once when the run ends.
// See: context/lib/rendering_pipeline.md §12

mod header;
mod row;

use std::path::Path;

use postretro_camera_route::PoseFile;
use postretro_renderer::{GpuTimingSupport, PlaybackGpuSample};
use serde::Serialize;

use super::args::{Pacing, PlaybackArgs};
use super::marks::{ChangeSet, LifeKey};
use super::route::Identity;
use super::run::{FramePlan, RowKind};
pub(crate) use header::{AdapterReport, DisplayModeReport, RunContext, WindowModeReport};
use header::{LevelReport, LifeStateEntry, ProjectionReport, RecordHeader, TimingState};
pub(crate) use row::{RowDrain, RowFill, RowGpu};

pub(crate) const SCHEMA: &str = "postretro.camera_route_playback.v1";

/// Rows reserved past the planned ones, so no-draw rows still fill without
/// allocating.
const ROW_SLACK: usize = 256;

/// Wall-clock route rows are planned at this frame rate over the route's
/// duration; a faster run grows the row store rarely, never per frame.
const WALL_CLOCK_PLANNED_HZ: f64 = 240.0;

/// Life-state entries reserved: a run logs one per death, respawn or
/// teleport, so the steady state never allocates.
const LIFE_STATE_SLACK: usize = 32;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PlaybackRecord {
    header: RecordHeader,
    rows: Vec<Row>,
    /// Local pawn life-state changes, each on the row of the frame that saw it.
    life_states: Vec<LifeStateEntry>,
}

/// One recorded frame. Plain data, so filling one allocates nothing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(crate) struct Row {
    /// The record's frame index, and the frame mark's `k`.
    pub(crate) frame: u64,
    pub(crate) phase: RowKind,
    /// The pose drawn; under wall-clock pacing, the pose the frame's route
    /// time falls after.
    pub(crate) pose: usize,
    /// Wall-clock route and no-draw rows: seconds since route frame 0 began.
    pub(crate) route_time_s: Option<f64>,
    /// Microseconds since the previous world frame began; `null` for the
    /// first, which has none.
    pub(crate) interval_us: Option<u64>,
    /// Every mid-run change from the first row it touched on.
    pub(crate) changes: ChangeSet,
    /// The interval spans a hidden window or a frame that presented nothing.
    pub(crate) gap: bool,
    #[serde(flatten)]
    pub(crate) fill: RowFill,
    /// The frame's GPU times, once its readback lands; `null` with GPU
    /// timing off, for a no-draw frame, and for one whose readback never
    /// returned.
    pub(crate) gpu: Option<RowGpu>,
}

impl PlaybackRecord {
    pub(crate) fn new(args: &PlaybackArgs, file: &PoseFile) -> Self {
        Self {
            header: RecordHeader::new(args, file),
            rows: Vec::with_capacity(
                args.hold as usize + planned_route_rows(args, file) + ROW_SLACK,
            ),
            life_states: Vec::with_capacity(LIFE_STATE_SLACK),
        }
    }

    pub(crate) fn set_settle(&mut self, timed_out: bool) {
        self.header.settle = Some(if timed_out { "timed_out" } else { "settled" });
    }

    /// Marks the record aborted with `reason`.
    pub(crate) fn set_aborted(&mut self, reason: String) {
        self.header.outcome = Some("aborted");
        self.header.abort_reason = Some(reason);
    }

    pub(crate) fn set_complete(&mut self) {
        self.header.outcome = Some("complete");
    }

    /// Logs a life-state change on the row the current frame takes.
    pub(crate) fn push_life_state(
        &mut self,
        now: Option<LifeKey>,
        name: Option<String>,
        previous: Option<LifeKey>,
    ) {
        self.life_states.push(LifeStateEntry {
            frame: self.next_frame(),
            pawn: now.map(|key| key.pawn),
            life_state: now.map(|key| key.life_state),
            life_state_name: name,
            life_state_entry: now.map(|key| key.life_state_entry),
            teleport_generation: now.map(|key| key.teleport_generation),
            table_revision: now.map(|key| key.table_revision),
            previous_life_state: previous.map(|key| key.life_state),
        });
    }

    pub(crate) fn set_level(&mut self, bake: String, identity: Identity) {
        self.header.level = Some(LevelReport {
            bake,
            identity: identity.key(),
        });
    }

    /// Whether CPU stage timing runs, and the GPU timing gate's state, as
    /// the run arms.
    pub(crate) fn set_timing(&mut self, cpu_enabled: bool, gpu: GpuTimingSupport) {
        self.header.cpu_timing = TimingState::cpu(cpu_enabled);
        self.header.gpu_timing.state = TimingState::gpu(gpu);
    }

    pub(crate) fn gpu_timing_enabled(&self) -> bool {
        self.header.gpu_timing.state.enabled()
    }

    /// The run's machine state as its first row records: adapter, extents,
    /// window mode and present mode.
    pub(crate) fn set_context(&mut self, context: RunContext) {
        self.header.machine = Some(context);
    }

    pub(crate) fn has_context(&self) -> bool {
        self.header.machine.is_some()
    }

    /// Route frame 0's projection: the file's horizontal FOV at the
    /// window's aspect.
    pub(crate) fn set_rendered(&mut self, hfov_degrees: f32, aspect: f32) {
        self.header.rendered_projection = Some(ProjectionReport {
            hfov_degrees,
            aspect,
        });
    }

    pub(crate) fn has_rendered(&self) -> bool {
        self.header.rendered_projection.is_some()
    }

    /// The index the next row takes: the frame mark of the frame recording.
    pub(crate) fn next_frame(&self) -> u64 {
        self.rows.len() as u64
    }

    pub(crate) fn push_row(
        &mut self,
        plan: FramePlan,
        interval_us: Option<u64>,
        (changes, gap): (ChangeSet, bool),
        fill: RowFill,
    ) {
        self.rows.push(Row {
            frame: self.next_frame(),
            phase: plan.kind,
            pose: plan.pose,
            route_time_s: plan.route_time_s,
            interval_us,
            changes,
            gap,
            fill,
            gpu: None,
        });
    }

    /// A frame's GPU times landed: they go on the row its frame mark names.
    /// A sample for no row (its frame wrote none) is counted, never placed.
    pub(crate) fn patch_gpu(&mut self, sample: PlaybackGpuSample) {
        let report = &mut self.header.gpu_timing;
        let Some(row) = usize::try_from(sample.frame)
            .ok()
            .and_then(|index| self.rows.get_mut(index))
            .filter(|row| row.frame == sample.frame && row.phase != RowKind::NoDraw)
        else {
            report.unmatched_samples += 1;
            return;
        };
        let gpu = RowGpu::from(sample);
        report.malformed_spans += u64::from(gpu.malformed);
        row.gpu = Some(gpu);
    }

    /// The run's end: every hold and route row still without GPU times
    /// never got its readback. `outstanding` readbacks were still in flight
    /// at the bounded wait; `untimed` marked frames found the ring full.
    pub(crate) fn finish_gpu(&mut self, outstanding: usize, untimed: u64) {
        let report = &mut self.header.gpu_timing;
        if !report.state.enabled() {
            return;
        }
        report.rows_missing = Some(
            self.rows
                .iter()
                .filter(|row| row.phase != RowKind::NoDraw && row.gpu.is_none())
                .count() as u64,
        );
        report.readbacks_outstanding = outstanding as u64;
        report.untimed_frames = untimed;
    }

    #[cfg(test)]
    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows
    }

    #[cfg(test)]
    pub(crate) fn header_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.header).expect("the header serializes")
    }

    /// The record's one write.
    pub(crate) fn write(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| format!("serializing the playback record: {err}"))?;
        std::fs::write(path, text)
            .map_err(|err| format!("writing the playback record {}: {err}", path.display()))
    }
}

/// Route rows a run plans: one per pose frame-locked; the route's duration
/// at [`WALL_CLOCK_PLANNED_HZ`] under wall-clock pacing.
fn planned_route_rows(args: &PlaybackArgs, file: &PoseFile) -> usize {
    match args.pacing {
        Pacing::FrameLocked => file.poses.len(),
        Pacing::WallClock => {
            let seconds = file.poses.len() as f64 / f64::from(file.header.cadence_hz.max(1));
            (seconds * WALL_CLOCK_PLANNED_HZ).ceil() as usize + 1
        }
    }
}

#[cfg(test)]
mod tests;
