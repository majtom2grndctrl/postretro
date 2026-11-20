// Camera-route playback: flies the windowed camera along a pose file, one
// pose per presented world frame, and writes a per-frame record.
// See: context/lib/rendering_pipeline.md §11, §12 · context/lib/boot_sequence.md §1 (Settling)

mod args;
mod record;
mod route;
mod run;

use std::time::Instant;

use glam::Vec3;
use postretro_camera_route::PoseFile;
use winit::event_loop::ActiveEventLoop;

use crate::App;
use crate::app::world_less_frame::HeldLevelView;
use crate::camera::RenderCamera;

use args::PlaybackArgs;
use record::PlaybackRecord;
use route::Identity;
use run::{Phase, RouteRun, RowKind};

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
}

struct ActivePlayback {
    args: PlaybackArgs,
    file: PoseFile,
    hfov: f32,
    run: RouteRun,
    record: PlaybackRecord,
    /// Why the installed level refused the route, until a Settling frame
    /// stops the run before any world frame.
    refusal: Option<String>,
    /// The current world frame presented.
    presented: bool,
    /// When the previous world frame of this run began.
    last_frame_start: Option<Instant>,
    /// The current world frame's interval from the previous one.
    frame_interval_us: Option<u64>,
}

impl Playback {
    /// Reads the playback flags and loads the pose file. A bad flag or file
    /// stops the launch: it prints the reason and exits 1, before any window.
    pub(crate) fn from_args_or_exit(args: &[String]) -> Self {
        match Self::from_args(args) {
            Ok(playback) => playback,
            Err(err) => {
                eprintln!("[Playback] {err}");
                crate::telemetry::end_run_before_exit(1, &format!("camera route: {err}"));
                std::process::exit(1);
            }
        }
    }

    fn from_args(args: &[String]) -> Result<Self, String> {
        let Some(args) = args::playback_args(args)? else {
            return Ok(Self::default());
        };
        let file = route::load_route(&args.poses)?;
        log::info!(
            "[Playback] camera route {}: {} poses at {}°, hold {} frames, {} pacing, record {}",
            args.poses.display(),
            file.poses.len(),
            file.header.projection.hfov_degrees,
            args.hold,
            args.pacing.key(),
            args.report.display(),
        );
        Ok(Self {
            active: Some(Box::new(ActivePlayback {
                hfov: file.header.projection.hfov_degrees.to_radians(),
                run: RouteRun::new(args.hold, file.poses.len()),
                record: PlaybackRecord::new(&args, &file),
                args,
                file,
                refusal: None,
                presented: false,
                last_frame_start: None,
                frame_interval_us: None,
            })),
        })
    }

    /// Armed and not complete: gameplay input stays suspended and the pause
    /// menu stays shut, so nothing moves the pawn or covers a measured frame.
    pub(crate) fn captures_gameplay(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.run.is_playing())
    }

    /// The pose this world frame draws, between its begin and end hooks.
    pub(crate) fn route_view(&self) -> Option<RouteView> {
        let active = self.active.as_ref()?;
        active.view(active.run.current()?.pose)
    }

    pub(crate) fn note_presented(&mut self, presented: bool) {
        if let Some(active) = self.active.as_mut() {
            active.presented = presented;
        }
    }
}

impl ActivePlayback {
    fn view(&self, pose: usize) -> Option<RouteView> {
        let pose = self.file.poses.get(pose)?;
        Some(RouteView {
            eye: pose.eye,
            yaw: pose.yaw,
            pitch: pose.pitch,
            hfov: self.hfov,
        })
    }
}

impl App {
    /// Install's camera step on the session's first gameplay level: the
    /// route's bake must be the level's (id-51 hash and walk epoch). A match
    /// arms the run; a mismatch is held for the first Settling frame, which
    /// stops the run. A frontend backdrop is never the route's level.
    pub(crate) fn playback_level_installed(&mut self) {
        let backdrop = self.frontend_menu_is_present();
        let Some(active) = self.playback.active.as_mut() else {
            return;
        };
        if backdrop || active.run.phase() != Phase::AwaitingInstall {
            return;
        }
        let Some(world) = self.level.as_ref() else {
            return;
        };
        let path = self
            .level_load
            .as_ref()
            .map(|load| load.map_path.display().to_string())
            .or_else(|| {
                self.map_path
                    .as_ref()
                    .map(|path| path.display().to_string())
            })
            .unwrap_or_default();
        let bake = postretro_camera_route::bake_identity(&path, world);
        match route::match_level(&active.file, &bake) {
            Ok(identity) => {
                if identity == Identity::Unproven {
                    log::warn!(
                        "[Playback] level {path} has no id-51 hash; camera route {} accepted unproven",
                        active.args.poses.display(),
                    );
                }
                active.record.set_level(bake, identity);
                active.run.arm();
            }
            Err(reason) => {
                active.refusal = Some(format!(
                    "camera route {} does not match level {path}: {reason}",
                    active.args.poses.display(),
                ));
            }
        }
    }

    /// A Settling frame's held view: the route's first pose at its FOV while
    /// the run is armed, else `fallback`. `None` after stopping a run whose
    /// route the level refused.
    pub(crate) fn playback_settling_view(
        &mut self,
        event_loop: &ActiveEventLoop,
        fallback: HeldLevelView,
    ) -> Option<HeldLevelView> {
        let aspect = self.camera.aspect();
        let Some(active) = self.playback.active.as_mut() else {
            return Some(fallback);
        };
        if let Some(refusal) = active.refusal.take() {
            log::error!("[Playback] {refusal}");
            self.playback.active = None;
            self.exit_result = Err(anyhow::anyhow!(refusal));
            event_loop.exit();
            return None;
        }
        let held = active
            .run
            .settling_pose()
            .and_then(|pose| active.view(pose))
            .map(|view| view.held_view(aspect));
        Some(held.unwrap_or(fallback))
    }

    /// A Running frame, before its eye and streaming step: pick the frame's
    /// pose and mark its GPU work with the frame index its row will take.
    pub(crate) fn playback_begin_world_frame(&mut self, now: Instant) {
        let Some(active) = self.playback.active.as_mut() else {
            return;
        };
        if !active.run.is_playing() {
            return;
        }
        active.presented = false;
        active.frame_interval_us = active.last_frame_start.map(|previous| {
            u64::try_from(now.duration_since(previous).as_micros()).unwrap_or(u64::MAX)
        });
        active.last_frame_start = Some(now);
        let Some(plan) = active.run.begin_world_frame() else {
            return;
        };
        if plan.kind == RowKind::Route && !active.record.has_rendered() {
            let adapter = self.renderer.as_ref().map(|renderer| {
                let identity = renderer.capture_measurement_adapter_identity();
                (identity.name.clone(), identity.backend.clone())
            });
            active.record.set_rendered(
                active.file.header.projection.hfov_degrees,
                self.camera.aspect(),
                adapter,
            );
        }
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_frame_mark(Some(active.record.next_frame()));
        }
    }

    /// The frame's end: a frame that presented the world writes its row and
    /// advances the route. After the last pose's route frame the record is
    /// written, once, and the session exits 0, or non-zero if the write failed.
    pub(crate) fn playback_end_frame(&mut self, event_loop: &ActiveEventLoop) {
        let Some(active) = self.playback.active.as_mut() else {
            return;
        };
        let outcome = active
            .run
            .finish_frame(std::mem::take(&mut active.presented));
        if let Some(plan) = outcome.row {
            active.record.push_row(plan, active.frame_interval_us);
        }
        if !outcome.complete {
            return;
        }
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_frame_mark(None);
        }
        let report = &active.args.report;
        match active.record.write(report) {
            Ok(()) => log::info!(
                "[Playback] camera route complete: {} frames recorded to {}",
                active.record.next_frame(),
                report.display(),
            ),
            Err(err) => {
                log::error!("[Playback] {err}");
                self.exit_result = Err(anyhow::anyhow!(err));
            }
        }
        self.release_cursor_for_exit();
        event_loop.exit();
    }
}

#[cfg(test)]
mod tests;
