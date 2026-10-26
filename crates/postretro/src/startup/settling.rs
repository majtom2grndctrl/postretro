//! The Settling boot state: an installed level held behind the loading tree
//! until every streamed resource its first frame draws is resident, or a
//! timeout releases it.
//! See: context/lib/boot_sequence.md §1

use std::time::{Duration, Instant};

use winit::event_loop::ActiveEventLoop;

use crate::App;
use crate::session::level_streaming::settle::{ResourceSettle, SettleReport, SettleSetSize};
use crate::startup::BootState;
use crate::startup::loading_screen::LOAD_PARSE_SHARE;
use crate::startup::settle_log::SettleLog;

/// How long a level entry waits for its settle set before revealing with
/// each resource's miss fallback.
pub(crate) const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

/// Why a Settling stretch ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettleRelease {
    Settled,
    TimedOut(SettleReport),
}

/// One level entry's Settling stretch. Created fresh at install and dropped at
/// reveal, unload or suspend, so no entry inherits another's timer.
#[derive(Debug)]
pub(crate) struct SettleState {
    started: Instant,
    /// The largest settle-set shortfall seen: the bar's denominator.
    peak_unsettled: usize,
    /// Decided on the previous Settling frame, which painted the bar full; the
    /// next redraw, after its request drain, reveals.
    release: Option<SettleRelease>,
    /// Entry to the latest decision; at release, to the deciding frame.
    hold: Duration,
    /// What the per-entry `[Settling]` line reports.
    log: SettleLog,
}

impl SettleState {
    pub(crate) fn new(started: Instant) -> Self {
        Self {
            started,
            peak_unsettled: 0,
            release: None,
            hold: Duration::ZERO,
            log: SettleLog::default(),
        }
    }
}

/// Settles first, so a set that completes on the deadline frame releases
/// without a warning.
pub(crate) fn settle_decision(report: SettleReport, elapsed: Duration) -> Option<SettleRelease> {
    if report.settled() {
        Some(SettleRelease::Settled)
    } else if elapsed >= SETTLE_TIMEOUT {
        Some(SettleRelease::TimedOut(report))
    } else {
        None
    }
}

fn unsettled_units(report: SettleReport) -> Option<usize> {
    let units = |answer: ResourceSettle| match answer {
        ResourceSettle::NotStreamed | ResourceSettle::Settled => Some(0),
        ResourceSettle::NotAsked => None,
        ResourceSettle::Unsettled(count) => Some(count),
    };
    Some(units(report.sh)? + units(report.lightmap)?)
}

/// The bar's value through Settling, from the parse share up to 1.0, which a
/// release reaches. A shortfall that grows raises the denominator instead of
/// moving the bar back; the caller still never lowers the bar.
pub(crate) fn settle_progress(unsettled: usize, peak_unsettled: usize, release: bool) -> f32 {
    if release {
        return 1.0;
    }
    let done = if peak_unsettled == 0 {
        0.0
    } else {
        1.0 - unsettled as f32 / peak_unsettled as f32
    };
    LOAD_PARSE_SHARE + (1.0 - LOAD_PARSE_SHARE) * done
}

impl App {
    /// Install's last step: hold the level behind the loading tree. The
    /// loading screen stays active, so its tree and bar keep drawing.
    pub(crate) fn enter_settling(&mut self, now: Instant) {
        self.boot_state = BootState::Settling;
        self.settle = Some(SettleState::new(now));
    }

    /// One Settling redraw. A release decided on the previous frame reveals
    /// now, and this redraw runs as the first Running frame (returns true).
    /// Otherwise: a world-less transport poll, a staged-reload poll (Settling
    /// counts as installed), then the held level's streaming step from the
    /// presented pose, the settle check, the bar, and the loading tree over a
    /// compose-only frame. No tick, no system-command drain, no world present.
    pub(crate) fn run_settling_frame(
        &mut self,
        event_loop: &ActiveEventLoop,
        frame_dt: f32,
    ) -> bool {
        if self.reveal_if_released() {
            return true;
        }
        let _ = self.poll_world_less_transport(frame_dt);
        self.poll_staged_manifest_results();
        if self.boot_state != BootState::Settling {
            return false;
        }
        self.suspend_held_level_input();

        let frame_start = Instant::now();
        // The held view projects at the extent the reveal frame will commit,
        // not the camera's default aspect left by an unload.
        self.commit_render_extents();
        let view = self.presented_pose().held_view(self.camera.aspect());
        let held = match self.prepare_held_level_frame(view) {
            Ok(held) => held,
            Err(err) => {
                log::error!("[Loader] level streaming failed while settling: {err:#}");
                self.exit_result = Err(err);
                event_loop.exit();
                return false;
            }
        };
        // No session means nothing streams.
        let (report, set) = self.session.as_ref().map_or(
            (
                SettleReport {
                    sh: ResourceSettle::NotStreamed,
                    lightmap: ResourceSettle::NotStreamed,
                },
                SettleSetSize::default(),
            ),
            |session| (session.settle_report(), session.settle_set_size()),
        );
        // A load or unload request that arrived in this frame's transport
        // poll wins: no release is decided for a level about to unload.
        if self.level_requests.is_empty() {
            if let Some(settle) = self.settle.as_mut() {
                let path = held.as_ref().map(|held| held.path());
                settle.log.observe_frame(path, report, set);
            }
            self.advance_settle(report, frame_start);
        }

        self.advance_loading_screen(frame_dt);
        self.sync_glyph_art();
        self.poll_loading_images();
        // Without a registered loading tree the held frame still composes, over
        // an empty UI on the splash color, so the settle advances.
        let snapshot = self.loading_screen_snapshot().unwrap_or_default();
        let painted = held.is_some_and(|held| {
            self.present_held_level_frame(
                event_loop,
                frame_start,
                snapshot,
                crate::render::SPLASH_CLEAR_COLOR,
                held,
            )
        });
        if !painted {
            let _ = self.paint_splash(event_loop);
        }
        if self
            .settle
            .as_ref()
            .is_some_and(|settle| settle.release.is_some())
        {
            // The sim owes nothing for the held stretch: the reveal frame
            // ticks only for the time since this frame.
            self.frame_timing.rearm(Instant::now());
        }
        self.request_redraw();
        false
    }

    /// Nothing the player does while the level is hidden acts on it: mouse
    /// look and gameplay presses lift without a pulse, as on the frontend.
    /// The pad's levels are recorded first, as the frontend does, so a pad
    /// input held through the hold stays inert after reveal instead of
    /// pressing fresh; the frame's pad presses and nav intents are dropped
    /// with it.
    fn suspend_held_level_input(&mut self) {
        let App {
            session,
            nav_stick_tracker,
            ..
        } = self;
        let Some(session) = session.as_mut() else {
            return;
        };
        let context = session.ui_nav_context();
        if let Some(gamepad) = session.gamepad_system.as_mut() {
            let _ = gamepad.update(
                &mut session.input_system,
                nav_stick_tracker,
                session.bindings.ui_nav(),
                context,
            );
        }
        session.input_system.suspend_gameplay();
    }

    /// Reveal on the redraw after a release decision; returns whether it did.
    /// Input that arrived since the deciding frame is suspended as on a held
    /// frame, so a mouse delta or press from that gap neither turns the pose
    /// the reveal frame presents nor reaches its first tick. The frame loop
    /// applies no look on the reveal frame, so a held stick does not turn it
    /// either.
    fn reveal_if_released(&mut self) -> bool {
        let Some(release) = self.settle.as_ref().and_then(|settle| settle.release) else {
            return false;
        };
        self.suspend_held_level_input();
        self.reveal_level(release);
        true
    }

    /// Decide this frame's release and move the bar, never down.
    fn advance_settle(&mut self, report: SettleReport, now: Instant) {
        let Some(settle) = self.settle.as_mut() else {
            return;
        };
        let elapsed = now.duration_since(settle.started);
        let release = settle_decision(report, elapsed);
        let unsettled = unsettled_units(report);
        if let Some(unsettled) = unsettled {
            settle.peak_unsettled = settle.peak_unsettled.max(unsettled);
        }
        settle.release = release;
        settle.hold = elapsed;
        let progress = match unsettled {
            Some(unsettled) => settle_progress(unsettled, settle.peak_unsettled, release.is_some()),
            None if release.is_some() => 1.0,
            None => LOAD_PARSE_SHARE,
        };
        self.raise_loading_progress(progress);
    }

    /// The reveal edge: the held level becomes the Running level. A
    /// timed-out reveal is a reveal for every purpose.
    fn reveal_level(&mut self, release: SettleRelease) {
        if let SettleRelease::TimedOut(report) = release {
            log::warn!(
                "[Loader] revealing the level after the {} s settle timeout: SH {}, lightmap {}",
                SETTLE_TIMEOUT.as_secs(),
                report.sh.describe(),
                report.lightmap.describe(),
            );
        }
        if let Some(settle) = self.settle.take() {
            let timed_out = matches!(release, SettleRelease::TimedOut(_));
            self.settle_summary = Some(settle.log.finish(timed_out, settle.hold));
        }
        self.level_timings.record("settle_hold");
        self.end_loading_screen();
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.clear_splash();
            renderer.start_next_promotions_whole();
        }
        // Settling frames never count, and no CPU timing surface may show a
        // window from before the reveal.
        self.cpu_timer.level_changed();
        self.publish_net_reveal();
        self.boot_state = BootState::Running;
        // Defer log line C until after the reveal frame's render returns, so
        // `first_level_frame` captures GPU work the user actually sees.
        self.pending_level_log = true;
    }

    /// The reveal frame's tail, after its lightmap drain counted the frame's
    /// visible misses: logs the entry's one `[Settling]` line. An entry
    /// abandoned before its reveal logs none.
    pub(crate) fn log_settle_summary(&mut self) {
        let Some(summary) = self.settle_summary.take() else {
            return;
        };
        let reveal_misses = self
            .session
            .as_ref()
            .and_then(|session| session.last_frame_lightmap_misses());
        summary.log(reveal_misses);
    }
}

#[cfg(test)]
mod tests {
    use postretro_entities::slot_table::SlotValue;
    use postretro_scripting_core::store_bridge::read_store_slot;
    use postretro_test_log_capture::LogCapture;
    use winit::keyboard::KeyCode;

    use super::*;
    use crate::input::{Action, ButtonState};
    use crate::startup::lifecycle::tests::test_app;
    use crate::startup::{LevelLoadEntry, LevelRequest};

    fn report(sh: ResourceSettle, lightmap: ResourceSettle) -> SettleReport {
        SettleReport { sh, lightmap }
    }

    // A settle on the deadline frame is a settle, not a timeout.
    #[test]
    fn settle_on_deadline_frame_releases_without_warning() {
        let settled = report(ResourceSettle::Settled, ResourceSettle::NotStreamed);
        assert_eq!(
            settle_decision(settled, SETTLE_TIMEOUT),
            Some(SettleRelease::Settled)
        );
        assert_eq!(
            settle_decision(settled, SETTLE_TIMEOUT - Duration::from_millis(1)),
            Some(SettleRelease::Settled)
        );
        let pending = report(ResourceSettle::Unsettled(2), ResourceSettle::NotAsked);
        assert_eq!(
            settle_decision(pending, SETTLE_TIMEOUT - Duration::from_millis(1)),
            None
        );
        assert_eq!(
            settle_decision(pending, SETTLE_TIMEOUT),
            Some(SettleRelease::TimedOut(pending))
        );
    }

    // A growing shortfall holds the bar; a shrinking one advances it;
    // a release fills it.
    #[test]
    fn settle_progress_is_monotone_and_reaches_one_before_reveal() {
        let start = settle_progress(4, 4, false);
        assert!((start - LOAD_PARSE_SHARE).abs() < 1e-6);
        let half = settle_progress(2, 4, false);
        assert!(half > start && half < 1.0);
        // The total grows to 8 with 4 left: the formula reads lower, which the
        // caller's never-decreasing write absorbs.
        assert!(settle_progress(4, 8, false) <= half);
        assert_eq!(settle_progress(3, 8, true), 1.0);
    }

    const TIMEOUT_WARNING: &str = "settle timeout";

    fn progress(app: &App) -> SlotValue {
        read_store_slot(
            &app.session.as_ref().unwrap().scripting.script_ctx,
            "loading.progress",
        )
        .unwrap()
    }

    fn entry() -> LevelLoadEntry {
        LevelLoadEntry {
            catalog_id: Some("e1m1".to_string()),
            path: "maps/e1m1.prl".to_string(),
            name: "Entryway".to_string(),
            tags: Vec::new(),
            loading_tree: Vec::new(),
        }
    }

    /// An app holding a level in Settling since `started`, its loading
    /// screen active at the parse share.
    fn settling_app(started: Instant) -> App {
        let mut app = test_app();
        app.begin_loading_screen(&entry());
        app.raise_loading_progress(LOAD_PARSE_SHARE);
        app.level = Some(crate::runtime_movers::tests::single_cell_world(
            Default::default(),
        ));
        app.enter_settling(started);
        app
    }

    fn unsettled(count: usize) -> SettleReport {
        report(
            ResourceSettle::Unsettled(count),
            ResourceSettle::NotStreamed,
        )
    }

    fn release(app: &App) -> Option<SettleRelease> {
        app.settle.as_ref().and_then(|settle| settle.release)
    }

    fn has_mark(app: &App, mark: &str) -> bool {
        app.level_timings
            .entries
            .iter()
            .any(|(stage, _)| *stage == mark)
    }

    #[test]
    fn settling_counts_as_installed() {
        let app = settling_app(Instant::now());
        assert!(
            app.has_installed_level(),
            "hot reload and observe-live see it"
        );
        assert!(
            app.level_is_installed_state(),
            "request draining unloads it"
        );
    }

    #[test]
    fn device_limited_mandatory_sh_reveals_into_running_only_at_timeout() {
        use crate::session::level_streaming::{StreamingFrame, WantedStreaming};
        use crate::session::sh_residency::ShStreamingSession;
        use crate::sh_streaming::sync_manifest_test_fixture;
        use postretro_level_loader::{ShDrainOutcome, ShStreamingMode};
        use postretro_stage_timing::StageFrame;
        use postretro_visibility::{VisibilityPath, VisibleCells};
        use std::sync::Arc;

        let (_temp, path) = sync_manifest_test_fixture::write_one_cluster_prl();
        let world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
        let manifest = Arc::clone(world.sh_stream_manifest().unwrap());
        let started = Instant::now();
        let mut app = settling_app(started);
        let capture = LogCapture::start();
        let session = app.session.as_mut().unwrap();
        session
            .level_streaming
            .ensure_sessions(
                &mut session.sh_streaming,
                WantedStreaming {
                    sh: Some((&manifest, ShStreamingMode::SyncProof)),
                    lightmap: None,
                    cluster_directory: world.cluster_directory(),
                    residency_set: None,
                },
                |manifest, _, hints| {
                    ShStreamingSession::from_snapshot(
                        manifest,
                        postretro_renderer::ShResidencySnapshot {
                            effective_floor_bytes: 1024 * 1024,
                            ..Default::default()
                        },
                        hints.unwrap(),
                    )
                },
            )
            .unwrap();
        let cpu = StageFrame::default();
        let visible = VisibleCells::Culled(vec![0]);
        let batch = session
            .level_streaming
            .prepare_drains(
                &mut session.sh_streaming,
                None,
                StreamingFrame {
                    visible_cells: &visible,
                    sh_visible_cells: &visible,
                    camera_cell: Some(0),
                    path: VisibilityPath::PrlPortal { walk_reach: 0 },
                    monotonic_seconds: 0.0,
                    settling: true,
                    cpu: &cpu,
                },
            )
            .unwrap();
        assert_eq!(batch.ready.len(), 1);
        session
            .sh_streaming
            .as_mut()
            .unwrap()
            .apply_controller_outcome(ShDrainOutcome {
                refused: vec![0],
                ..Default::default()
            })
            .unwrap();
        let report = session.settle_report();
        assert_eq!(report.sh, ResourceSettle::Unsettled(1));
        app.advance_settle(report, started + SETTLE_TIMEOUT - Duration::from_millis(1));
        assert_eq!(release(&app), None);
        assert_eq!(app.boot_state, BootState::Settling);
        app.advance_settle(report, started + SETTLE_TIMEOUT);
        assert!(app.reveal_if_released());
        assert_eq!(app.boot_state, BootState::Running);
        assert!(app.exit_result.is_ok());
        capture.assert_logged_once(log::Level::Warn, "device limit refused cluster 0");
        capture.assert_logged_once(log::Level::Warn, TIMEOUT_WARNING);
        capture.assert_not_logged(log::Level::Warn, "read/decode failed");
    }

    #[test]
    fn settle_timeout_warns_once_and_reveals_into_running() {
        let capture = LogCapture::start();
        let started = Instant::now();
        let mut app = settling_app(started);
        app.advance_settle(unsettled(3), started + Duration::from_secs(5));
        assert_eq!(release(&app), None);
        app.advance_settle(unsettled(3), started + SETTLE_TIMEOUT);
        let Some(timed_out) = release(&app) else {
            panic!("the deadline releases");
        };
        assert_eq!(
            progress(&app),
            SlotValue::Number(1.0),
            "the last Settling frame's bar is full, timed out or not"
        );
        app.reveal_level(timed_out);
        capture.assert_logged_once(log::Level::Warn, TIMEOUT_WARNING);
        capture.assert_logged_once(log::Level::Warn, "SH 3 unsettled, lightmap not streamed");
        assert_eq!(app.boot_state, BootState::Running);
        assert!(app.settle.is_none());
        assert!(app.pending_level_log, "line C closes on the reveal frame");
        assert_eq!(
            progress(&app),
            SlotValue::Number(0.0),
            "the loading state resets at reveal"
        );
        assert!(
            has_mark(&app, "settle_hold"),
            "the hold is its own line C mark"
        );
    }

    const SETTLE_LINE: &str = "[Settling] entry outcome=";

    fn observe(app: &mut App, path: postretro_visibility::VisibilityPath, report: SettleReport) {
        let settle = app.settle.as_mut().unwrap();
        settle
            .log
            .observe_frame(Some(path), report, SettleSetSize::default());
    }

    // The entry's line waits for the reveal frame's tail, carries the hold to
    // the deciding frame, and logs once; the timeout warning stays as it was.
    #[test]
    fn timed_out_entry_logs_one_settle_line_after_the_reveal_frame() {
        use postretro_visibility::VisibilityPath;

        let capture = LogCapture::start();
        let started = Instant::now();
        let mut app = settling_app(started);
        observe(
            &mut app,
            VisibilityPath::PrlPortal { walk_reach: 1 },
            unsettled(3),
        );
        app.advance_settle(unsettled(3), started + Duration::from_secs(5));
        let step_limit = VisibilityPath::PortalStepLimitFallback {
            considered: 9,
            accepted: 1,
        };
        observe(&mut app, step_limit, unsettled(2));
        app.advance_settle(unsettled(2), started + SETTLE_TIMEOUT);
        app.reveal_level(release(&app).unwrap());
        capture.assert_not_logged(log::Level::Info, SETTLE_LINE);
        capture.assert_logged_once(log::Level::Warn, TIMEOUT_WARNING);

        app.log_settle_summary();
        app.log_settle_summary();
        capture.assert_logged_once(
            log::Level::Info,
            "[Settling] entry outcome=timed_out hold_ms=10000 frames=2 walk_frames=1 \
             step_limit_frames=1 sh_set_peak=n/a lightmap_set_peak=n/a sh_unsettled_peak=3 \
             lightmap_unsettled_peak=n/a reveal_misses=n/a",
        );
    }

    #[test]
    fn abandoned_settle_logs_no_settle_line() {
        let capture = LogCapture::start();
        let mut app = settling_app(Instant::now());
        observe(
            &mut app,
            postretro_visibility::VisibilityPath::PrlPortal { walk_reach: 1 },
            unsettled(1),
        );
        app.enqueue_level_request(LevelRequest::Unload);
        app.drain_level_requests();
        app.log_settle_summary();
        capture.assert_not_logged(log::Level::Info, SETTLE_LINE);
    }

    #[test]
    fn settled_reveal_logs_no_timeout_warning() {
        let capture = LogCapture::start();
        let started = Instant::now();
        let mut app = settling_app(started);
        app.advance_settle(
            report(ResourceSettle::Settled, ResourceSettle::Settled),
            started + SETTLE_TIMEOUT,
        );
        assert_eq!(release(&app), Some(SettleRelease::Settled));
        app.reveal_level(SettleRelease::Settled);
        capture.assert_not_logged(log::Level::Warn, TIMEOUT_WARNING);
        assert_eq!(app.boot_state, BootState::Running);
    }

    #[test]
    fn restart_after_timeout_starts_fresh_settle_timer() {
        let started = Instant::now();
        let mut app = settling_app(started);
        app.advance_settle(unsettled(1), started + SETTLE_TIMEOUT);
        app.reveal_level(release(&app).unwrap());

        let restarted = started + SETTLE_TIMEOUT + Duration::from_secs(1);
        app.begin_loading_screen(&entry());
        app.enter_settling(restarted);
        app.advance_settle(unsettled(1), restarted + Duration::from_secs(9));
        assert_eq!(release(&app), None, "the new entry gets the whole timeout");
    }

    #[test]
    fn settle_progress_never_decreases_across_a_growing_set() {
        let started = Instant::now();
        let mut app = settling_app(started);
        let mut last = LOAD_PARSE_SHARE;
        for count in [4, 2, 6, 3, 0] {
            app.advance_settle(unsettled(count), started);
            let SlotValue::Number(shown) = progress(&app) else {
                panic!("progress is a number");
            };
            assert!(shown >= last, "{count}: {shown} < {last}");
            last = shown;
        }
        assert_eq!(last, 1.0, "the settled frame fills the bar");
    }

    // A request drained on a frame whose settle check would pass wins.
    #[test]
    fn level_request_during_settling_unloads_without_reveal_edge() {
        let started = Instant::now();
        let mut app = settling_app(started);
        app.advance_settle(
            report(ResourceSettle::Settled, ResourceSettle::Settled),
            started,
        );
        assert!(release(&app).is_some(), "the next redraw would reveal");
        app.enqueue_level_request(LevelRequest::Unload);
        app.drain_level_requests();
        assert_eq!(app.boot_state, BootState::Frontend);
        assert!(app.settle.is_none(), "no stale settle or timer");
        assert!(!app.pending_level_log, "no reveal edge fired");
        assert!(!has_mark(&app, "settle_hold"));
        assert_eq!(
            progress(&app),
            SlotValue::Number(0.0),
            "the held level's loading screen ends with it"
        );
    }

    // A suspend during Settling fires no reveal and leaves no timer.
    #[test]
    fn suspend_during_settling_leaves_no_settle_state() {
        let mut app = settling_app(Instant::now());
        app.reset_boot_state_after_suspend();
        assert_eq!(app.boot_state, BootState::Booting);
        assert!(app.settle.is_none());
        assert!(!app.pending_level_log);
    }

    #[test]
    fn settling_drops_ui_input_like_loading() {
        assert!(BootState::Settling.drops_ui_input());
        assert!(BootState::Loading.drops_ui_input());
        assert!(!BootState::Running.drops_ui_input());
    }

    fn host_revealed(app: &App) -> Option<String> {
        match app.session.as_ref()?.net_endpoint.as_ref()? {
            crate::netcode::NetEndpoint::Host { server, .. } => {
                server.revealed_level().map(str::to_owned)
            }
            crate::netcode::NetEndpoint::Client { .. } => None,
        }
    }

    // A timed-out reveal is a reveal, and the host's own reveal is retracted
    // by unload and by suspend (both through `clear_net_level_parity`).
    #[test]
    fn timeout_release_is_a_reveal_edge_and_unload_retracts_it() {
        let started = Instant::now();
        let mut app = settling_app(started);
        app.active_level_source = Some(crate::startup::LevelSource::Catalog("e1m1".into()));
        app.published_level_identity = Some("e1m1".into());
        app.session.as_mut().unwrap().net_endpoint = crate::netcode::NetEndpoint::from_role(
            &crate::netcode::NetRole::Host { port: 0 },
            None,
        )
        .unwrap();
        assert_eq!(host_revealed(&app), None, "Settling has not revealed");

        app.advance_settle(unsettled(2), started + SETTLE_TIMEOUT);
        app.reveal_level(release(&app).expect("the deadline releases"));
        assert_eq!(host_revealed(&app).as_deref(), Some("e1m1"));

        app.clear_net_level_parity();
        assert_eq!(host_revealed(&app), None);
    }

    // The reveal fires in boot dispatch, before the frame's world poll, so
    // the promotion it causes is consumed by a world poll that spawns the
    // pawn, never by a world-less one.
    #[test]
    fn host_reveal_lands_before_the_reveal_frames_world_poll() {
        let frame = include_str!("../frame_loop/mod.rs");
        let dispatch = frame.find("app.drive_boot_state_for_redraw(").unwrap();
        let world_poll = frame
            .find("frame_order::run_snapshot_apply_stage(")
            .unwrap();
        assert!(dispatch < world_poll);
        // The Running path, from the end of the Frontend early return to the
        // world poll, has no world-less poll to consume the promotion first.
        let running = frame
            .split("app.finish_first_launch_hold_if_closed();")
            .nth(1)
            .unwrap()
            .split("frame_order::run_snapshot_apply_stage(")
            .next()
            .unwrap();
        assert!(
            !running.contains("poll_world_less_transport("),
            "the reveal frame's Running path polls only the world"
        );
        let settling = include_str!("settling.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        let body = settling.split("fn run_settling_frame(").nth(1).unwrap();
        let reveal = body.find("self.reveal_if_released()").unwrap();
        let world_less = body.find("self.poll_world_less_transport(").unwrap();
        assert!(
            reveal < world_less,
            "a revealing redraw polls only as Running"
        );
    }

    // Input from the gap between the deciding frame and the reveal frame is
    // suspended as on a held frame: a mouse delta turns nothing and a press
    // reaches no tick, so the reveal frame presents the settled pose.
    #[test]
    fn reveal_frame_input_is_neutral() {
        let started = Instant::now();
        let mut app = settling_app(started);
        app.advance_settle(
            report(ResourceSettle::Settled, ResourceSettle::Settled),
            started,
        );
        assert!(release(&app).is_some(), "the next redraw reveals");

        let input = &mut app.session.as_mut().unwrap().input_system;
        input.handle_mouse_delta(40.0, -25.0);
        input.handle_keyboard_event(KeyCode::Space, true);

        assert!(app.reveal_if_released());
        assert_eq!(app.boot_state, BootState::Running);
        let input = &mut app.session.as_mut().unwrap().input_system;
        let look = input.drain_look_inputs();
        assert_eq!(look.yaw_delta(1.0 / 60.0), 0.0, "no gap mouse look");
        assert_eq!(look.pitch_delta(1.0 / 60.0), 0.0, "no gap mouse look");
        let snapshot = input.snapshot();
        assert_eq!(
            snapshot.button(Action::Jump),
            ButtonState::Inactive,
            "a gap press reaches no tick, and stays inert while held"
        );

        // A held stick is persistent, not suspended: the frame loop applies
        // no look on the reveal frame instead.
        let frame = include_str!("../frame_loop/mod.rs");
        assert!(frame.contains("let reveal_frame = app.boot_state == BootState::Settling;"));
        assert!(frame.contains("let look = if ui_captures_gameplay || reveal_frame {"));
    }

    // Commands install queued for the reveal frame die with an
    // abandoned settle; none plays a sound in the next level or the frontend.
    #[test]
    fn abandoned_settle_discards_commands_install_queued() {
        let mut app = settling_app(Instant::now());
        app.session
            .as_ref()
            .unwrap()
            .scripting
            .script_ctx
            .system_commands
            .push(
                postretro_entities::reactions::system_commands::SystemReactionCommand::PlaySound {
                    audience: None,
                    sound: "door".into(),
                    bus: None,
                    at: None,
                },
            );
        app.enqueue_level_request(LevelRequest::Unload);
        app.drain_level_requests();
        assert!(
            app.session
                .as_ref()
                .unwrap()
                .scripting
                .script_ctx
                .system_commands
                .is_empty()
        );
    }
}
