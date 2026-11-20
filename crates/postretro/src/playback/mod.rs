// Camera-route playback: flies the windowed camera along a pose file, one
// pose per presented world frame, and writes a per-frame record.
// See: context/lib/rendering_pipeline.md §11, §12 · context/lib/boot_sequence.md §1 (Settling)

mod args;
mod lifecycle;
mod marks;
mod pacing;
mod record;
mod route;
mod run;
mod stall;

use std::time::{Duration, Instant};

use glam::Vec3;
use postretro_camera_route::PoseFile;

use crate::app::world_less_frame::HeldLevelView;
use crate::camera::RenderCamera;

use args::{Pacing, PlaybackArgs};
pub(crate) use marks::ChangeKind;
use marks::{LifeWatch, Marks};
use record::PlaybackRecord;
use run::{Clock, RouteRun};
use stall::StallWatch;

/// The pose a playing route draws this frame, at the route's projection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RouteView {
    pub(crate) eye: Vec3,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    /// Base horizontal FOV, radians: the pose file's.
    pub(crate) hfov: f32,
}

impl RouteView {
    /// The held view a Settling frame streams from: this pose with no
    /// view-feel offset, at the window's aspect.
    fn held_view(self, aspect: f32) -> HeldLevelView {
        HeldLevelView {
            camera: RenderCamera::with_base_hfov(
                self.eye,
                aspect,
                self.yaw,
                self.pitch,
                0.0,
                Vec3::ZERO,
                0.0,
                self.hfov,
            ),
        }
    }
}

/// `--camera-route` playback for one session. Inactive unless the launch
/// named a pose file; inactive, every hook is one `Option` check.
#[derive(Default)]
pub(crate) struct Playback {
    active: Option<Box<ActivePlayback>>,
    /// Armed at launch for a measurement run, until its first world frame.
    stall: Option<StallWatch>,
    /// The event loop waits until the stall deadline while armed; the flag
    /// restores the default wait once it disarms.
    waiting_on_stall: bool,
    /// The run ended: every hook holding the event loop exits it.
    exit_pending: bool,
}

struct ActivePlayback {
    args: PlaybackArgs,
    file: PoseFile,
    hfov: f32,
    run: RouteRun,
    record: PlaybackRecord,
    marks: Marks,
    life: LifeWatch,
    /// Why the installed level refused the route, until a Settling frame
    /// stops the run before any world frame.
    refusal: Option<String>,
    /// The current world frame presented.
    presented: bool,
    /// The current world frame ran the level streaming step.
    streamed: bool,
    /// When the previous world frame of this run began.
    last_frame_start: Option<Instant>,
    /// The current world frame's interval from the previous one.
    frame_interval_us: Option<u64>,
}

impl Playback {
    /// Reads the playback flags and loads the pose file. A bad flag, file or
    /// launch stops it: it prints the reason and exits 1, before any window.
    pub(crate) fn from_args_or_exit(args: &[String]) -> Self {
        match Self::from_args(args, Instant::now()) {
            Ok(playback) => playback,
            Err(err) => {
                eprintln!("[Playback] {err}");
                crate::telemetry::end_run_before_exit(1, &format!("camera route: {err}"));
                std::process::exit(1);
            }
        }
    }

    fn from_args(args: &[String], launched: Instant) -> Result<Self, String> {
        let Some(args) = args::playback_args(args)? else {
            return Ok(Self::default());
        };
        let file = route::load_route(&args.poses)?;
        log::info!(
            "[Playback] camera route {}: {} poses at {}°, hold {} frames, {} pacing, stall timeout {} s, record {}",
            args.poses.display(),
            file.poses.len(),
            file.header.projection.hfov_degrees,
            args.hold,
            args.pacing.key(),
            args.stall_timeout.as_secs_f64(),
            args.report.display(),
        );
        let clock = match args.pacing {
            Pacing::FrameLocked => Clock::FrameLocked,
            Pacing::WallClock => Clock::WallClock {
                cadence_hz: f64::from(file.header.cadence_hz),
                cut: file.poses.iter().map(|pose| pose.cut).collect(),
            },
        };
        let mut playback = Self::default();
        playback.arm_stall(launched, args.stall_timeout);
        playback.active = Some(Box::new(ActivePlayback {
            hfov: file.header.projection.hfov_degrees.to_radians(),
            run: RouteRun::new(args.hold, file.poses.len(), clock),
            record: PlaybackRecord::new(&args, &file),
            marks: Marks::default(),
            life: LifeWatch::default(),
            args,
            file,
            refusal: None,
            presented: false,
            streamed: false,
            last_frame_start: None,
            frame_interval_us: None,
        }));
        Ok(playback)
    }

    /// Arms the stall timeout from launch: playback here, recording later.
    pub(crate) fn arm_stall(&mut self, launched: Instant, timeout: Duration) {
        self.stall = Some(StallWatch::arm(launched, timeout));
    }

    /// Armed and not ended: gameplay input stays suspended and the pause
    /// menu stays shut, so nothing moves the pawn or covers a measured frame.
    pub(crate) fn captures_gameplay(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.run.is_playing())
    }

    /// The pose this world frame draws, between its begin and end hooks.
    pub(crate) fn route_view(&self) -> Option<RouteView> {
        let active = self.active.as_ref()?;
        let plan = active.run.current()?;
        active.view(plan.pose, plan.blend)
    }

    /// The current world frame's present: it presented the world, or not.
    /// Any present also tells the stall timeout the window is alive.
    pub(crate) fn note_presented(&mut self, presented: bool) {
        if presented {
            self.note_any_present();
        }
        if let Some(active) = self.active.as_mut() {
            active.presented = presented;
        }
    }

    /// The current world frame ran the level streaming step: one that then
    /// presents nothing writes a no-draw row.
    pub(crate) fn note_streamed(&mut self) {
        if let Some(active) = self.active.as_mut() {
            active.streamed = true;
        }
    }

    /// A splash, loading, Settling, frontend or first-launch panel frame
    /// presented.
    pub(crate) fn note_any_present(&mut self) {
        if let Some(stall) = self.stall.as_mut() {
            stall.note_present();
        }
    }

    /// The window was hidden or shown again (`WindowEvent::Occluded`).
    pub(crate) fn note_occluded(&mut self, occluded: bool) {
        if let Some(active) = self.active.as_mut() {
            active.marks.note_occluded(occluded);
        }
    }

    /// A mid-run change landed: every row from the next one on carries it.
    /// Before the first row it changes nothing the record compares.
    pub(crate) fn note_change(&mut self, kind: ChangeKind) {
        if let Some(active) = self.active.as_mut()
            && active.run.writes_rows()
        {
            log::info!("[Playback] mid-route change: {}", kind.key());
            active.marks.note_change(kind);
        }
    }
}

impl ActivePlayback {
    /// Pose `pose`, `blend` of the way to the next one.
    fn view(&self, pose: usize, blend: f32) -> Option<RouteView> {
        let from = self.file.poses.get(pose)?;
        let (eye, yaw, pitch) = match self.file.poses.get(pose + 1) {
            Some(to) => pacing::interpolate(from, to, blend),
            None => (from.eye, from.yaw, from.pitch),
        };
        Some(RouteView {
            eye,
            yaw,
            pitch,
            hfov: self.hfov,
        })
    }
}

#[cfg(test)]
mod tests;
