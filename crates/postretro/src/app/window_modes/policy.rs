// CPU window-mode policy; settings remain authoritative for accepted choices.
// See: context/lib/player_options.md §7

use crate::options::{DisplayMode, PlayerOptions, WindowMode};
use std::time::{Duration, Instant};

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

pub(super) struct Pending {
    prior: Target,
    prior_picked: Option<DisplayMode>,
    candidate: Target,
    pub(super) deadline: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Change {
    None,
    OpenConfirm,
    Accepted,
    Reverted,
}

pub(super) struct Controller {
    pub(super) choices: Vec<DisplayMode>,
    pub(super) picked: Option<DisplayMode>,
    pub(super) effective: Target,
    pub(super) fallback: bool,
    pub(super) escape: bool,
    pub(super) settle_until: Option<Instant>,
    pub(super) pending: Option<Pending>,
}

impl Controller {
    pub(super) fn new(escape: bool) -> Self {
        Self {
            choices: Vec::new(),
            picked: None,
            effective: Target::default(),
            fallback: false,
            escape,
            settle_until: None,
            pending: None,
        }
    }

    pub(super) fn refresh(&mut self, backend: &impl Backend, options: &PlayerOptions) {
        self.choices = backend.enumerate();
        self.choices.sort();
        self.choices.dedup();
        self.picked = options
            .display_mode
            .as_ref()
            .filter(|mode| self.choices.contains(mode))
            .cloned();
    }

    pub(super) fn apply(&mut self, backend: &mut impl Backend, target: Target, now: Instant) {
        let applied = backend.apply(&target);
        self.fallback = !applied;
        self.effective = if applied {
            target
        } else {
            Target {
                mode: WindowMode::Borderless,
                display: None,
            }
        };
        self.settle_until = Some(now + SETTLE_TIME);
    }

    pub(super) fn boot(
        &mut self,
        backend: &mut impl Backend,
        options: &PlayerOptions,
        now: Instant,
    ) {
        self.refresh(backend, options);
        let target = if self.escape {
            Target::default()
        } else {
            Target {
                mode: options.window_mode,
                display: options.display_mode.clone(),
            }
        };
        if target.mode != WindowMode::Windowed {
            self.apply(backend, target, now);
        }
    }
}

impl Controller {
    pub(super) fn request_mode(
        &mut self,
        backend: &mut impl Backend,
        options: &mut PlayerOptions,
        mode: WindowMode,
        now: Instant,
    ) -> Change {
        if self.pending.is_some() {
            return Change::None;
        }
        self.refresh(backend, options);
        let target = Target {
            mode,
            display: options.display_mode.clone(),
        };
        self.request(backend, options, target, now)
    }

    fn request(
        &mut self,
        backend: &mut impl Backend,
        options: &mut PlayerOptions,
        target: Target,
        now: Instant,
    ) -> Change {
        let prior = self.effective.clone();
        let prior_picked = self.picked.clone();
        self.escape = false;
        self.apply(backend, target.clone(), now);
        if target.mode == WindowMode::Exclusive && !self.fallback {
            self.picked.clone_from(&target.display);
            self.pending = Some(Pending {
                prior,
                prior_picked,
                candidate: target,
                deadline: now + Duration::from_secs(15),
            });
            Change::OpenConfirm
        } else {
            options.window_mode = target.mode;
            options.mark_written(crate::options::keys::WINDOW_MODE);
            Change::Accepted
        }
    }

    pub(super) fn step(
        &mut self,
        backend: &mut impl Backend,
        options: &mut PlayerOptions,
        next: bool,
        now: Instant,
    ) -> Change {
        if self.pending.is_some() {
            return Change::None;
        }
        self.refresh(backend, options);
        if self.choices.is_empty() {
            return Change::None;
        }
        let index = self
            .picked
            .as_ref()
            .and_then(|mode| self.choices.iter().position(|choice| choice == mode));
        let count = self.choices.len();
        let index = match (index, next) {
            (Some(index), true) => (index + 1) % count,
            (Some(index), false) => (index + count - 1) % count,
            (None, true) => 0,
            (None, false) => count - 1,
        };
        let selected = self.choices[index].clone();
        if self.effective.mode == WindowMode::Exclusive {
            self.request(
                backend,
                options,
                Target {
                    mode: WindowMode::Exclusive,
                    display: Some(selected),
                },
                now,
            )
        } else {
            self.picked = Some(selected.clone());
            options.set_display_mode(selected);
            Change::Accepted
        }
    }

    pub(super) fn keep(&mut self, options: &mut PlayerOptions) -> Change {
        let Some(pending) = self.pending.take() else {
            return Change::None;
        };
        options.window_mode = pending.candidate.mode;
        options.mark_written(crate::options::keys::WINDOW_MODE);
        if let Some(mode) = pending.candidate.display {
            options.set_display_mode(mode);
        }
        Change::Accepted
    }

    pub(super) fn revert(&mut self, backend: &mut impl Backend, now: Instant) -> Change {
        let Some(pending) = self.pending.take() else {
            return Change::None;
        };
        self.apply(backend, pending.prior, now);
        self.picked = pending.prior_picked;
        Change::Reverted
    }

    pub(super) fn service(
        &mut self,
        backend: &mut impl Backend,
        confirm_present: bool,
        now: Instant,
    ) -> Change {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| !confirm_present || now >= pending.deadline)
        {
            self.revert(backend, now)
        } else {
            Change::None
        }
    }

    pub(super) fn seconds_remaining(&self, now: Instant) -> f32 {
        self.pending.as_ref().map_or(0.0, |pending| {
            pending
                .deadline
                .saturating_duration_since(now)
                .as_secs_f32()
                .ceil()
        })
    }
}
