//! The Settling boot state: an installed level held behind the loading tree
//! until every streamed resource its first frame draws is resident, or a
//! timeout releases it.
//! See: context/lib/boot_sequence.md §1

use std::time::{Duration, Instant};

use winit::event_loop::ActiveEventLoop;

use crate::App;
use crate::session::level_streaming::settle::{ResourceSettle, SettleReport};
use crate::startup::BootState;
use crate::startup::loading_screen::LOAD_PARSE_SHARE;

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
}

impl SettleState {
    pub(crate) fn new(started: Instant) -> Self {
        Self {
            started,
            peak_unsettled: 0,
            release: None,
        }
    }
}

/// Settles first, so a set that completes on the deadline frame releases
/// without a warning (P3).
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
/// moving the bar back; the caller still never lowers the bar (P10).
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
        if let Some(release) = self.settle.as_ref().and_then(|settle| settle.release) {
            self.reveal_level(release);
            return true;
        }
        let _ = self.poll_world_less_transport(frame_dt);
        self.poll_staged_manifest_results();
        if self.boot_state != BootState::Settling {
            return false;
        }

        let frame_start = Instant::now();
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
        let report = self.session.as_ref().map_or(
            SettleReport {
                sh: ResourceSettle::NotStreamed,
                lightmap: ResourceSettle::NotStreamed,
            },
            |session| session.settle_report(),
        );
        self.advance_settle(report, frame_start);

        self.advance_loading_screen(frame_dt);
        self.sync_glyph_art();
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

    /// Decide this frame's release and move the bar, never down.
    fn advance_settle(&mut self, report: SettleReport, now: Instant) {
        let Some(settle) = self.settle.as_mut() else {
            return;
        };
        let release = settle_decision(report, now.duration_since(settle.started));
        let unsettled = unsettled_units(report);
        if let Some(unsettled) = unsettled {
            settle.peak_unsettled = settle.peak_unsettled.max(unsettled);
        }
        settle.release = release;
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
        self.settle = None;
        self.level_timings.record("settle_hold");
        self.end_loading_screen();
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.clear_splash();
            renderer.start_next_promotions_whole();
        }
        // Settling frames never count, and no CPU timing surface may show a
        // window from before the reveal.
        self.cpu_timer.level_changed();
        self.boot_state = BootState::Running;
        // Defer log line C until after the reveal frame's render returns, so
        // `first_level_frame` captures GPU work the user actually sees.
        self.pending_level_log = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(sh: ResourceSettle, lightmap: ResourceSettle) -> SettleReport {
        SettleReport { sh, lightmap }
    }

    // P3: a settle on the deadline frame is a settle, not a timeout.
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

    // P10: a growing shortfall holds the bar; a shrinking one advances it;
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

    use postretro_entities::slot_table::SlotValue;
    use postretro_scripting_core::store_bridge::read_store_slot;
    use postretro_test_log_capture::LogCapture;

    use crate::startup::lifecycle::tests::test_app;
    use crate::startup::{LevelLoadEntry, LevelRequest};

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

    // P2: a request drained on a frame whose settle check would pass wins.
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

    // P7: a suspend during Settling fires no reveal and leaves no timer.
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
}
