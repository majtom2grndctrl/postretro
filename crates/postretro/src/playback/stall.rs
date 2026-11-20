// The stall timeout a measurement run arms at launch: a run whose window
// presents nothing, or presents but reaches no world frame, fails fast
// instead of waiting forever (pin P17). Pure: the clock is passed in.
// See: context/lib/rendering_pipeline.md §12 · context/lib/boot_sequence.md §1 (Window visibility)

use std::time::{Duration, Instant};

/// The screen a run that presents frames but no world frame waits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitScreen {
    Splash,
    Loading,
    Settling,
    FirstLaunchPanel,
    FrontendMenu,
    /// A level runs, but no measured world frame has presented.
    Level,
}

impl WaitScreen {
    fn describe(self) -> &'static str {
        match self {
            Self::Splash => "the splash",
            Self::Loading => "loading",
            Self::Settling => "settling",
            Self::FirstLaunchPanel => "the first-launch accessibility panel",
            Self::FrontendMenu => "the frontend menu",
            Self::Level => "a level with no route frame",
        }
    }
}

/// Armed from launch until the run's first world frame presents.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StallWatch {
    deadline: Instant,
    timeout: Duration,
    presented_any: bool,
}

impl StallWatch {
    pub(crate) fn arm(launched: Instant, timeout: Duration) -> Self {
        Self {
            deadline: launched + timeout,
            timeout,
            presented_any: false,
        }
    }

    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }

    /// A frame of any kind presented: splash, loading, Settling, frontend or
    /// the first-launch panel.
    pub(crate) fn note_present(&mut self) {
        self.presented_any = true;
    }

    /// At or past the deadline: why the run fails, naming a stalled window
    /// when nothing presented, else the screen it waited on.
    pub(crate) fn expired(&self, now: Instant, screen: WaitScreen) -> Option<String> {
        if now < self.deadline {
            return None;
        }
        let seconds = self.timeout.as_secs_f64();
        Some(if self.presented_any {
            format!(
                "no world frame within the {seconds} s stall timeout: waited on {}",
                screen.describe()
            )
        } else {
            format!(
                "stalled window: no frame presented within the {seconds} s stall timeout (window hidden, display asleep or screen locked?)"
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stall_watch_names_a_stalled_window_when_nothing_presented() {
        let launched = Instant::now();
        let watch = StallWatch::arm(launched, Duration::from_secs(3));
        assert_eq!(watch.deadline(), launched + Duration::from_secs(3));
        assert_eq!(
            watch.expired(launched + Duration::from_millis(2_999), WaitScreen::Splash),
            None
        );
        let reason = watch
            .expired(launched + Duration::from_secs(3), WaitScreen::Splash)
            .unwrap();
        assert!(reason.starts_with("stalled window"), "{reason}");
        assert!(reason.contains("3 s"), "{reason}");
    }

    #[test]
    fn stall_watch_names_the_screen_a_presenting_run_waited_on() {
        let launched = Instant::now();
        let mut watch = StallWatch::arm(launched, Duration::from_secs(3));
        watch.note_present();
        let late = launched + Duration::from_secs(4);
        for (screen, named) in [
            (
                WaitScreen::FirstLaunchPanel,
                "first-launch accessibility panel",
            ),
            (WaitScreen::FrontendMenu, "frontend menu"),
            (WaitScreen::Loading, "loading"),
            (WaitScreen::Settling, "settling"),
        ] {
            let reason = watch.expired(late, screen).unwrap();
            assert!(
                reason.contains(named) && !reason.contains("stalled window"),
                "{reason}"
            );
        }
    }
}
