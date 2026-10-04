// CPU window-mode policy; settings remain authoritative for accepted choices.
// See: context/lib/player_options.md §7

use std::time::{Duration, Instant};
use crate::options::{DisplayMode, PlayerOptions, WindowMode};

pub(super) const SETTLE_TIME: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Target {
    pub(super) mode: WindowMode,
    pub(super) display: Option<DisplayMode>,
}

pub(super) trait Backend {
    fn enumerate(&self) -> Vec<DisplayMode>;
    /// Re-find an exclusive handle in a fresh enumeration. False means the
    /// adapter applied borderless fallback and never submitted an unlisted mode.
    fn apply(&mut self, target: &Target) -> bool;
    fn readback(&mut self) -> &Target;
}

pub(super) struct Controller {
    pub(super) choices: Vec<DisplayMode>,
    pub(super) picked: Option<DisplayMode>,
    pub(super) effective: Target,
    pub(super) fallback: bool,
    pub(super) escape: bool,
    pub(super) settle_until: Option<Instant>,
}

impl Controller {
    pub(super) fn new(escape: bool) -> Self {
        Self { choices: Vec::new(), picked: None, effective: Target::default(), fallback: false, escape, settle_until: None }
    }

    pub(super) fn refresh(&mut self, backend: &impl Backend, options: &PlayerOptions) {
        self.choices = backend.enumerate();
        self.choices.sort();
        self.choices.dedup();
        self.picked = options.display_mode.as_ref().filter(|mode| self.choices.contains(mode)).cloned();
    }

    pub(super) fn apply(&mut self, backend: &mut impl Backend, target: Target, now: Instant) {
        let applied = backend.apply(&target);
        self.fallback = !applied;
        self.effective = if applied { target } else { Target { mode: WindowMode::Borderless, display: None } };
        self.settle_until = Some(now + SETTLE_TIME);
    }

    pub(super) fn boot(&mut self, backend: &mut impl Backend, options: &PlayerOptions, now: Instant) {
        self.refresh(backend, options);
        let target = if self.escape { Target::default() } else { Target { mode: options.window_mode, display: options.display_mode.clone() } };
        if target.mode != WindowMode::Windowed { self.apply(backend, target, now); }
    }
}
