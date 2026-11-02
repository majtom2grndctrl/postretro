// Portal-walk capture: one batched `portal_trace` log message per armed frame.
// See: context/lib/input.md §8

use std::fmt::Write as _;

use glam::Vec3;

use super::RegionWalkStats;
use postretro_level_loader::LevelWorld;

/// Why a portal test rejected, as the trace line names it.
#[derive(Debug, Clone, Copy)]
pub(super) enum Reject {
    Blocked,
    Solid,
    Invalid,
    /// Wholly behind the seeded entry's near plane.
    Near,
    Facing,
    /// Clipped to fewer than three vertices; carries how many survived.
    Clip(usize),
}

/// The armed capture's buffer. Exists only when the capture is armed, so an
/// unarmed walk formats and allocates nothing.
pub(crate) struct WalkTrace {
    buf: String,
    /// Facing rejects have no release counter; the summary counts its own.
    facing: u32,
}

impl WalkTrace {
    /// Header: the camera and its cell. `solid` is omitted, since a solid
    /// camera cell never reaches the walk.
    pub(crate) fn begin(eye: Vec3, camera_cell: usize, world: &LevelWorld) -> Self {
        let mut buf = String::with_capacity(512);
        let (bounds_min, bounds_max) = world
            .cell_bounds(camera_cell)
            .unwrap_or((Vec3::ZERO, Vec3::ZERO));
        let _ = writeln!(
            buf,
            "cam=({:.2},{:.2},{:.2}) cell={} faces={} bnds=({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2}) cells={}",
            eye.x,
            eye.y,
            eye.z,
            camera_cell,
            world.cell_face_count(camera_cell),
            bounds_min.x,
            bounds_min.y,
            bounds_min.z,
            bounds_max.x,
            bounds_max.y,
            bounds_max.z,
            world.cell_count(),
        );
        Self { buf, facing: 0 }
    }

    pub(super) fn accept(&mut self, cell: usize, neighbor: usize, verts: usize, bypass: bool) {
        let tag = if bypass { " bypass" } else { "" };
        let _ = writeln!(self.buf, "  acc {cell}->{neighbor} v={verts}{tag}");
    }

    pub(super) fn reject(
        &mut self,
        cell: usize,
        neighbor: Option<usize>,
        verts: usize,
        reason: Reject,
    ) {
        let buf = &mut self.buf;
        let _ = match neighbor {
            Some(neighbor) => write!(buf, "  rej {cell}->{neighbor} "),
            None => write!(buf, "  rej {cell}->? "),
        };
        let _ = match reason {
            Reject::Blocked => writeln!(buf, "v={verts} blocked"),
            Reject::Solid => writeln!(buf, "v={verts} solid"),
            Reject::Invalid => writeln!(buf, "invalid"),
            Reject::Near => writeln!(buf, "v={verts} near"),
            Reject::Facing => {
                self.facing += 1;
                writeln!(buf, "v={verts} facing")
            }
            Reject::Clip(clipped) => writeln!(buf, "v={clipped}/{verts} clip"),
        };
    }

    pub(super) fn limit(&mut self, cell: usize, step_limit: u32) {
        let _ = writeln!(self.buf, "  rej cell={cell} limit steps={step_limit}");
    }

    /// Append the summary: reach count, the considered and accepted totals,
    /// and a `rej[...]` bracket that elides zero counters. An all-clean walk
    /// still prints `rej[]`, so every summary has the same shape.
    pub(crate) fn finish(mut self, stats: &RegionWalkStats, reached: usize) -> String {
        let buf = &mut self.buf;
        let _ = write!(
            buf,
            "  = reach={} cons={} acc={} rej[",
            reached, stats.considered, stats.accepted,
        );
        let mut first = true;
        for (name, count) in [
            ("blocked", stats.rejected_blocked),
            ("clip", stats.rejected_clipped),
            ("facing", self.facing),
            ("solid", stats.rejected_solid),
            ("limit", u32::from(stats.step_limit_hit)),
            ("invalid", stats.rejected_invalid),
        ] {
            if count == 0 {
                continue;
            }
            if !first {
                buf.push(' ');
            }
            let _ = write!(buf, "{name}={count}");
            first = false;
        }
        let _ = writeln!(buf, "]");
        self.buf
    }

    /// Finish and log as one message: one timestamp and target prefix per
    /// traced frame instead of one per line.
    pub(crate) fn emit(self, stats: &RegionWalkStats, reached: usize) {
        log_trace(&self.finish(stats, reached));
    }
}

/// The capture line for a camera cell the world does not have; no walk runs.
pub(crate) fn trace_out_of_range(eye: Vec3, camera_cell: usize, cell_count: usize) {
    log_trace(&format!(
        "abort cell_oor cam=({:.2},{:.2},{:.2}) cell={} cells={}\n",
        eye.x, eye.y, eye.z, camera_cell, cell_count,
    ));
}

fn log_trace(buf: &str) {
    log::info!(target: "postretro::portal_trace", "[portal_trace]\n{}", buf);
}
