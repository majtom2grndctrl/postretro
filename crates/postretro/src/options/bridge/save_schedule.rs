// Settled persistence for accepted option changes: a 250 ms debounce, then one
// atomic save; closing the menu or exiting flushes a pending save.
// See: context/lib/player_options.md §3

use std::io;
use std::path::Path;

use super::super::PlayerOptions;

const SAVE_DEBOUNCE_SECONDS: f32 = 0.250;

/// A pending settled save, if any. Failures warn and clear the pending save;
/// the next accepted change arms it again.
#[derive(Default)]
pub(super) struct SaveSchedule {
    remaining_seconds: Option<f32>,
}

impl SaveSchedule {
    /// Restart the settle window after an accepted change. Without a settings
    /// path there is nothing to save, so nothing is scheduled.
    pub(super) fn arm(&mut self, settings_path: Option<&Path>) {
        self.remaining_seconds = settings_path.map(|_| SAVE_DEBOUNCE_SECONDS);
    }

    /// Count down a frame with no accepted change; save once the window settles.
    pub(super) fn tick<F>(
        &mut self,
        frame_dt_seconds: f32,
        options: &PlayerOptions,
        settings_path: Option<&Path>,
        save: &mut F,
    ) where
        F: FnMut(&PlayerOptions, &Path) -> io::Result<()>,
    {
        let Some(remaining) = self.remaining_seconds.as_mut() else {
            return;
        };
        let elapsed = if frame_dt_seconds.is_finite() {
            frame_dt_seconds.max(0.0)
        } else {
            0.0
        };
        *remaining -= elapsed;
        if *remaining <= 0.0 {
            self.attempt(options, settings_path, save);
        }
    }

    /// Save now if a save is pending.
    pub(super) fn flush<F>(
        &mut self,
        options: &PlayerOptions,
        settings_path: Option<&Path>,
        save: &mut F,
    ) where
        F: FnMut(&PlayerOptions, &Path) -> io::Result<()>,
    {
        if self.remaining_seconds.is_some() {
            self.attempt(options, settings_path, save);
        }
    }

    fn attempt<F>(&mut self, options: &PlayerOptions, settings_path: Option<&Path>, save: &mut F)
    where
        F: FnMut(&PlayerOptions, &Path) -> io::Result<()>,
    {
        self.remaining_seconds = None;
        let Some(path) = settings_path else {
            return;
        };
        if let Err(error) = save(options, path) {
            log::warn!(
                "[Options] failed to save changed settings to {}: {error}; keeping the in-memory value",
                path.display()
            );
        }
    }
}
