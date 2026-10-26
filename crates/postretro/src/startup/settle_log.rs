//! The per-entry Settling log: what one level entry's Settling stretch saw,
//! accumulated per frame without allocating and logged as one `[Settling]`
//! line on its reveal frame, once that frame's visible misses are counted.
//! See: context/lib/boot_sequence.md §1 (Settling)

use std::fmt::{self, Display};
use std::time::Duration;

use postretro_visibility::VisibilityPath;

use crate::session::level_streaming::settle::{ResourceSettle, SettleReport, SettleSetSize};

/// One entry's Settling frames, from install to the release decision. A
/// resource that never answered (it does not stream, or was never asked)
/// keeps its peaks `None`.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(crate) struct SettleLog {
    frames: u32,
    walk_frames: u32,
    step_limit_frames: u32,
    sh_set_peak: Option<usize>,
    lightmap_set_peak: Option<usize>,
    sh_unsettled_peak: Option<usize>,
    lightmap_unsettled_peak: Option<usize>,
    /// The height of the plane the latest held frame's mirror drew, `None`
    /// with no mirror view.
    planar_held: Option<f32>,
    /// The latest held frame's lightmap blocks in mirror-only cells outside
    /// the camera cell's baked set; `None` when the lightmap does not stream.
    mirror_only_blocks: Option<usize>,
}

fn raise(peak: &mut Option<usize>, value: Option<usize>) {
    if let Some(value) = value {
        *peak = Some(peak.map_or(value, |peak| peak.max(value)));
    }
}

fn unsettled(answer: ResourceSettle) -> Option<usize> {
    match answer {
        ResourceSettle::NotStreamed | ResourceSettle::NotAsked => None,
        ResourceSettle::Settled => Some(0),
        ResourceSettle::Unsettled(count) => Some(count),
    }
}

impl SettleLog {
    /// One Settling frame. `path` is the held view's visibility path, `None`
    /// when the frame walked nothing (no full-ready renderer yet); `report`
    /// and `set` are the frame's settle check and settle-set size.
    pub(crate) fn observe_frame(
        &mut self,
        path: Option<VisibilityPath>,
        report: SettleReport,
        set: SettleSetSize,
    ) {
        self.frames += 1;
        match path {
            Some(VisibilityPath::PrlPortal { .. }) => self.walk_frames += 1,
            Some(VisibilityPath::PortalStepLimitFallback { .. }) => self.step_limit_frames += 1,
            _ => {}
        }
        raise(&mut self.sh_set_peak, set.sh_targets);
        raise(&mut self.lightmap_set_peak, set.lightmap_blocks);
        raise(&mut self.sh_unsettled_peak, unsettled(report.sh));
        raise(
            &mut self.lightmap_unsettled_peak,
            unsettled(report.lightmap),
        );
    }

    /// One held frame's planar mirror: `plane` is the selected plane's height
    /// when the mirror drew, `mirror_only_blocks` the lightmap blocks of its
    /// mirror-only cells outside the camera cell's baked set (`None` when
    /// the lightmap does not stream). The held pose is fixed, so the latest
    /// frame's values stand for the entry.
    pub(crate) fn observe_planar(&mut self, plane: Option<f32>, mirror_only_blocks: Option<usize>) {
        self.planar_held = plane;
        self.mirror_only_blocks = mirror_only_blocks;
    }

    /// Closes the stretch on its release decision. `hold` runs from entry to
    /// the deciding frame, the span the timeout measures.
    pub(crate) fn finish(self, timed_out: bool, hold: Duration) -> SettleSummary {
        SettleSummary {
            log: self,
            timed_out,
            hold,
        }
    }
}

/// A released entry's Settling record, waiting for its reveal frame's misses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SettleSummary {
    log: SettleLog,
    timed_out: bool,
    hold: Duration,
}

/// A count, or `n/a` for a resource that never answered.
struct Count<T>(Option<T>);

impl<T: Display> Display for Count<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(value) => value.fmt(f),
            None => f.write_str("n/a"),
        }
    }
}

/// A planar plane's height to the millimetre, or `none` with no mirror view.
struct Plane(Option<f32>);

impl Display for Plane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(h) => write!(f, "{h:.3}"),
            None => f.write_str("none"),
        }
    }
}

impl SettleSummary {
    /// The entry's one `[Settling]` line. `reveal_misses` is the lightmap
    /// blocks the reveal frame drew while not installed, `None` when the
    /// lightmap does not stream. On a settled entry every settle-set block
    /// was installed, so when the reveal pose is the settled pose these lie
    /// outside the settle set; a timed-out entry's count also holds
    /// settle-set blocks still missing. `planar_reveal` is the plane the
    /// reveal frame's mirror drew: at the held pose it is `planar_held`'s.
    /// `mirror_only_blocks` counts the held pose's lightmap blocks that only
    /// the mirror draws and the camera cell's baked set does not hold: what
    /// planar adds to the settle set past the baked lead.
    pub(crate) fn line(&self, reveal_misses: Option<u32>, planar_reveal: Option<f32>) -> String {
        let log = &self.log;
        format!(
            "[Settling] entry outcome={} hold_ms={} frames={} walk_frames={} \
             step_limit_frames={} sh_set_peak={} lightmap_set_peak={} \
             sh_unsettled_peak={} lightmap_unsettled_peak={} reveal_misses={} \
             planar_held={} planar_reveal={} mirror_only_blocks={}",
            if self.timed_out {
                "timed_out"
            } else {
                "settled"
            },
            self.hold.as_millis(),
            log.frames,
            log.walk_frames,
            log.step_limit_frames,
            Count(log.sh_set_peak),
            Count(log.lightmap_set_peak),
            Count(log.sh_unsettled_peak),
            Count(log.lightmap_unsettled_peak),
            Count(reveal_misses),
            Plane(log.planar_held),
            Plane(planar_reveal),
            Count(log.mirror_only_blocks),
        )
    }

    pub(crate) fn log(&self, reveal_misses: Option<u32>, planar_reveal: Option<f32>) {
        log::info!("{}", self.line(reveal_misses, planar_reveal));
    }
}

#[cfg(test)]
mod tests {
    use postretro_test_log_capture::LogCapture;

    use super::*;

    const WALK: VisibilityPath = VisibilityPath::PrlPortal { walk_reach: 4 };
    const STEP_LIMIT: VisibilityPath = VisibilityPath::PortalStepLimitFallback {
        considered: 9,
        accepted: 3,
    };

    fn report(sh: ResourceSettle, lightmap: ResourceSettle) -> SettleReport {
        SettleReport { sh, lightmap }
    }

    fn set(sh: usize, lightmap: usize) -> SettleSetSize {
        SettleSetSize {
            sh_targets: Some(sh),
            lightmap_blocks: Some(lightmap),
        }
    }

    // Only a step-limit frame counts as one; other paths and frames that
    // walked nothing count toward the total alone.
    #[test]
    fn step_limit_frame_count_increments_only_on_step_limit_frames() {
        let mut log = SettleLog::default();
        let pending = report(ResourceSettle::Unsettled(1), ResourceSettle::NotStreamed);
        for path in [
            Some(WALK),
            Some(STEP_LIMIT),
            None,
            Some(VisibilityPath::SolidCellFallback),
            Some(STEP_LIMIT),
            Some(WALK),
        ] {
            log.observe_frame(path, pending, SettleSetSize::default());
        }
        let line = log.finish(false, Duration::ZERO).line(None, None);
        assert!(
            line.contains("frames=6 walk_frames=2 step_limit_frames=2 "),
            "{line}"
        );
    }

    // The peaks are each resource's own, never one combined shortfall: the
    // set may grow after the shortfall peaks, and the resources peak on
    // different frames.
    #[test]
    fn settled_entry_logs_per_resource_peaks_once() {
        let capture = LogCapture::start();
        let mut log = SettleLog::default();
        log.observe_frame(
            Some(WALK),
            report(ResourceSettle::NotAsked, ResourceSettle::Unsettled(40)),
            SettleSetSize {
                sh_targets: None,
                lightmap_blocks: Some(40),
            },
        );
        log.observe_frame(
            Some(WALK),
            report(ResourceSettle::Unsettled(7), ResourceSettle::Unsettled(12)),
            set(9, 52),
        );
        log.observe_frame(
            Some(WALK),
            report(ResourceSettle::Settled, ResourceSettle::Settled),
            set(9, 50),
        );
        let summary = log.finish(false, Duration::from_millis(742));
        summary.log(Some(3), None);
        capture.assert_logged_once(
            log::Level::Info,
            "[Settling] entry outcome=settled hold_ms=742 frames=3 walk_frames=3 \
             step_limit_frames=0 sh_set_peak=9 lightmap_set_peak=52 sh_unsettled_peak=7 \
             lightmap_unsettled_peak=40 reveal_misses=3 planar_held=none planar_reveal=none \
             mirror_only_blocks=n/a",
        );
    }

    // A timed-out entry says so, and a resource that never streamed reads
    // n/a rather than zero.
    #[test]
    fn timed_out_entry_logs_its_outcome_and_unstreamed_resources() {
        let capture = LogCapture::start();
        let mut log = SettleLog::default();
        for _ in 0..4 {
            log.observe_frame(
                Some(STEP_LIMIT),
                report(ResourceSettle::Unsettled(2), ResourceSettle::NotStreamed),
                SettleSetSize {
                    sh_targets: Some(5),
                    lightmap_blocks: None,
                },
            );
        }
        log.finish(true, Duration::from_secs(10)).log(None, None);
        capture.assert_logged_once(
            log::Level::Info,
            "[Settling] entry outcome=timed_out hold_ms=10000 frames=4 walk_frames=0 \
             step_limit_frames=4 sh_set_peak=5 lightmap_set_peak=n/a sh_unsettled_peak=2 \
             lightmap_unsettled_peak=n/a reveal_misses=n/a planar_held=none \
             planar_reveal=none mirror_only_blocks=n/a",
        );
    }

    // O2, O3, AC 38: the line ends with the plane the held frames' mirror
    // drew, the reveal frame's plane, and the held pose's mirror-only blocks
    // outside the baked set, from the latest held frame. A pose whose mirror
    // stopped drawing reads `none`; a streaming lightmap with nothing
    // mirror-only reads 0, not n/a.
    #[test]
    fn settle_line_reports_the_held_and_reveal_planes_and_mirror_only_blocks() {
        let capture = LogCapture::start();
        let mut log = SettleLog::default();
        let settled = report(ResourceSettle::Settled, ResourceSettle::Settled);
        log.observe_frame(Some(WALK), settled, set(4, 12));
        log.observe_planar(Some(-0.25), Some(5));
        log.observe_frame(Some(WALK), settled, set(4, 12));
        log.observe_planar(Some(-16.0), Some(3));
        log.finish(false, Duration::from_millis(80))
            .log(Some(0), Some(-16.0));
        capture.assert_logged_once(
            log::Level::Info,
            "[Settling] entry outcome=settled hold_ms=80 frames=2 walk_frames=2 \
             step_limit_frames=0 sh_set_peak=4 lightmap_set_peak=12 sh_unsettled_peak=0 \
             lightmap_unsettled_peak=0 reveal_misses=0 planar_held=-16.000 \
             planar_reveal=-16.000 mirror_only_blocks=3",
        );

        let mut log = SettleLog::default();
        log.observe_frame(Some(STEP_LIMIT), settled, set(1, 2));
        log.observe_planar(None, Some(0));
        let line = log.finish(false, Duration::ZERO).line(Some(0), None);
        assert!(
            line.ends_with(" planar_held=none planar_reveal=none mirror_only_blocks=0"),
            "{line}"
        );
    }
}
