// OS accessibility preferences behind an app-side seam: the live reader
// (`mundy`, plus Windows text scale) or a test fake feeds one channel the App
// polls at the top of each frame.
// See: context/lib/player_options.md §3, §5 · context/lib/boot_sequence.md §1

mod mundy_source;
#[cfg(windows)]
mod windows_text_scale;

use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

/// Whatever follows mod init waits at most this long for the reader's first
/// reply, counted from the end of mod init.
pub(crate) const OS_REPLY_WAIT: Duration = Duration::from_millis(150);

/// One message from an OS source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum OsUpdate {
    /// A reduced-motion / contrast reading. The first one is the reader's
    /// first reply.
    Preferences {
        reduce_motion: Option<bool>,
        increased_contrast: Option<bool>,
    },
    /// Windows text scale (1.0 = 100%).
    TextScale(f32),
}

/// The latest OS readings. `None` means the OS reports no preference or the
/// platform has no such setting.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct OsReadings {
    pub(crate) reduce_motion: Option<bool>,
    /// Seeds the theme variant (U2).
    #[allow(dead_code)]
    pub(crate) increased_contrast: Option<bool>,
    /// Seeds text scale (U2).
    #[allow(dead_code)]
    pub(crate) text_scale: Option<f32>,
}

/// The seam: a channel of OS updates plus whatever keeps the live sources
/// subscribed. Tests build one from a channel they feed.
pub(crate) struct OsPreferenceFeed {
    rx: Receiver<OsUpdate>,
    readings: OsReadings,
    replied: bool,
    /// Keeps OS subscriptions alive; dropping them unsubscribes.
    _sources: Vec<Box<dyn std::any::Any>>,
}

impl OsPreferenceFeed {
    /// Subscribe to the OS. Must run on the main thread after the event loop
    /// exists (macOS requirement); it never blocks: replies arrive on the
    /// channel.
    pub(crate) fn start() -> Self {
        let (tx, rx) = channel();
        let mut sources: Vec<Box<dyn std::any::Any>> = Vec::new();
        sources.push(Box::new(mundy_source::subscribe(tx.clone())));
        #[cfg(windows)]
        if let Some(guard) = windows_text_scale::subscribe(tx.clone()) {
            sources.push(Box::new(guard));
        }
        drop(tx);
        Self::with_sources(rx, sources)
    }

    /// A feed driven by a test's `Sender`.
    #[cfg(test)]
    pub(crate) fn fake() -> (Self, Sender<OsUpdate>) {
        let (tx, rx) = channel();
        (Self::with_sources(rx, Vec::new()), tx)
    }

    fn with_sources(rx: Receiver<OsUpdate>, sources: Vec<Box<dyn std::any::Any>>) -> Self {
        Self {
            rx,
            readings: OsReadings::default(),
            replied: false,
            _sources: sources,
        }
    }

    /// Drain pending updates. Returns the merged readings when any arrived.
    /// Called once per frame at the frame top; allocation-free.
    pub(crate) fn poll(&mut self) -> Option<OsReadings> {
        let mut changed = false;
        loop {
            match self.rx.try_recv() {
                Ok(update) => {
                    changed = true;
                    match update {
                        OsUpdate::Preferences {
                            reduce_motion,
                            increased_contrast,
                        } => {
                            self.replied = true;
                            self.readings.reduce_motion = reduce_motion;
                            self.readings.increased_contrast = increased_contrast;
                        }
                        OsUpdate::TextScale(scale) => self.readings.text_scale = Some(scale),
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        changed.then_some(self.readings)
    }

    /// Whether the reader's first reply has been polled.
    pub(crate) fn has_replied(&self) -> bool {
        self.replied
    }
}

/// Whether boot may leave the splash after mod init, given the OS reader's
/// state. A reply already in proceeds at once; otherwise boot waits up to
/// [`OS_REPLY_WAIT`] from the end of mod init, then proceeds and applies a
/// later reply as a live change.
pub(crate) fn os_wait_complete(replied: bool, mod_init_finished: Instant, now: Instant) -> bool {
    replied || now.saturating_duration_since(mod_init_finished) >= OS_REPLY_WAIT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preferences(reduce_motion: Option<bool>) -> OsUpdate {
        OsUpdate::Preferences {
            reduce_motion,
            increased_contrast: None,
        }
    }

    #[test]
    fn poll_merges_pending_updates_and_marks_the_first_reply() {
        let (mut feed, tx) = OsPreferenceFeed::fake();
        assert_eq!(feed.poll(), None);
        assert!(!feed.has_replied());

        tx.send(OsUpdate::TextScale(1.5)).unwrap();
        assert!(!feed.poll().is_some_and(|_| feed.has_replied()));

        tx.send(preferences(Some(true))).unwrap();
        tx.send(preferences(Some(false))).unwrap();
        let readings = feed.poll().unwrap();
        assert!(feed.has_replied());
        assert_eq!(readings.reduce_motion, Some(false), "latest reply wins");
        assert_eq!(readings.text_scale, Some(1.5));
        assert_eq!(feed.poll(), None, "nothing new, nothing reported");
    }

    #[test]
    fn the_wait_counts_from_the_end_of_mod_init() {
        // UO9: mod init took 400 ms; its length never eats into the wait.
        let boot = Instant::now();
        let mod_init_finished = boot + Duration::from_millis(400);
        let at = |ms| mod_init_finished + Duration::from_millis(ms);

        assert!(!os_wait_complete(false, mod_init_finished, at(0)));
        assert!(!os_wait_complete(false, mod_init_finished, at(149)));
        assert!(os_wait_complete(false, mod_init_finished, at(150)));
        // A reply 100 ms after mod init finishes ends the wait then.
        assert!(os_wait_complete(true, mod_init_finished, at(100)));
        // A reply already in when mod init finishes adds no wait.
        assert!(os_wait_complete(true, mod_init_finished, at(0)));
    }
}
