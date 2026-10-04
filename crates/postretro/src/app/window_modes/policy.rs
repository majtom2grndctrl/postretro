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
    /// Browsable subset only; saved modes and restores use full enumeration.
    fn picker_choices(&self, available: &[DisplayMode]) -> Vec<DisplayMode> {
        available.to_vec()
    }
    /// Choose only an enumerated tuple matching the monitor's current size.
    fn desktop_mode(&self, _choices: &[DisplayMode]) -> Option<DisplayMode> {
        None
    }
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
    SelectionChanged,
    OpenConfirm,
    Accepted,
    Reverted,
}

pub(super) struct Controller {
    pub(super) choices: Vec<DisplayMode>,
    browse_choices: Vec<DisplayMode>,
    pub(super) picked: Option<DisplayMode>,
    accepted: Option<DisplayMode>,
    picker_mode: WindowMode,
    launch_default: Option<DisplayMode>,
    selection_dirty: bool,
    pub(super) effective: Target,
    baseline: Target,
    pub(super) fallback: bool,
    pub(super) escape: bool,
    pub(super) settle_until: Option<Instant>,
    pub(super) pending: Option<Pending>,
}

impl Controller {
    pub(super) fn new(escape: bool) -> Self {
        Self {
            choices: Vec::new(),
            browse_choices: Vec::new(),
            picked: None,
            accepted: None,
            picker_mode: WindowMode::Windowed,
            launch_default: None,
            selection_dirty: false,
            effective: Target::default(),
            baseline: Target::default(),
            fallback: false,
            escape,
            settle_until: None,
            pending: None,
        }
    }

    pub(super) fn refresh(&mut self, backend: &impl Backend, options: &PlayerOptions) {
        self.refresh_choices(backend);
        self.accepted = self.accepted_pick(options);
        self.picker_mode = options.window_mode;
        if !self.selection_dirty
            || !self.picked.as_ref().is_some_and(|mode| {
                self.choices.contains(mode)
                    && self
                        .browse_choices
                        .iter()
                        .any(|choice| super::picker::same_choice(choice, mode))
            })
        {
            self.picked.clone_from(&self.accepted);
            self.selection_dirty = false;
        }
    }

    fn accepted_pick(&self, options: &PlayerOptions) -> Option<DisplayMode> {
        options
            .display_mode
            .as_ref()
            .or(self.launch_default.as_ref())
            .filter(|mode| self.choices.contains(mode))
            .cloned()
    }

    fn refresh_choices(&mut self, backend: &impl Backend) {
        self.choices = backend.enumerate();
        self.choices.sort();
        self.choices.dedup();
        self.browse_choices = backend.picker_choices(&self.choices);
    }

    pub(super) fn can_apply(&self) -> bool {
        self.pending.is_none()
            && self.picker_mode != WindowMode::Borderless
            && self.selection_dirty
            && self.picked.as_ref().is_some_and(|picked| {
                !self
                    .accepted
                    .as_ref()
                    .is_some_and(|accepted| super::picker::same_choice(picked, accepted))
            })
    }

    pub(super) fn apply(&mut self, backend: &mut impl Backend, target: Target, now: Instant) {
        let applied = backend.apply(&target);
        if !applied {
            self.refresh_choices(backend);
            self.picked = self
                .picked
                .take()
                .filter(|mode| self.choices.contains(mode));
            self.selection_dirty = false;
        }
        self.fallback = !applied;
        self.effective = if applied {
            target
        } else {
            log::warn!(
                "[Window] exclusive display mode is unavailable on the current monitor; using borderless for this session"
            );
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
        self.baseline = Target::default();
        self.effective = Target::default();
        self.settle_until = None;
        self.fallback = false;
        self.refresh_choices(backend);
        self.launch_default = backend.desktop_mode(&self.choices);
        self.selection_dirty = false;
        self.picked = self.accepted_pick(options);
        self.accepted.clone_from(&self.picked);
        self.picker_mode = options.window_mode;
        let target = if self.escape {
            Target::default()
        } else {
            Target {
                mode: options.window_mode,
                display: options
                    .display_mode
                    .clone()
                    .or_else(|| self.launch_default.clone()),
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
            display: if mode == WindowMode::Exclusive {
                self.picked.clone().or_else(|| options.display_mode.clone())
            } else {
                options.display_mode.clone()
            },
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
        let prior_picked = self.accepted_pick(options);
        self.escape = false;
        self.apply(backend, target.clone(), now);
        if target.mode == WindowMode::Exclusive && !self.fallback {
            self.picked.clone_from(&target.display);
            self.selection_dirty = false;
            self.pending = Some(Pending {
                prior,
                prior_picked,
                candidate: target,
                deadline: now + Duration::from_secs(15),
            });
            Change::OpenConfirm
        } else {
            options.window_mode = target.mode;
            self.picker_mode = target.mode;
            options.mark_written(crate::options::keys::WINDOW_MODE);
            Change::Accepted
        }
    }

    pub(super) fn step(
        &mut self,
        backend: &mut impl Backend,
        options: &mut PlayerOptions,
        next: bool,
        _now: Instant,
    ) -> Change {
        if self.pending.is_some() || options.window_mode == WindowMode::Borderless {
            return Change::None;
        }
        self.refresh(backend, options);
        if self.browse_choices.is_empty() {
            return Change::None;
        }
        let index = self.picked.as_ref().and_then(|mode| {
            self.browse_choices
                .iter()
                .position(|choice| super::picker::same_choice(choice, mode))
        });
        let count = self.browse_choices.len();
        let index = match (index, next) {
            (Some(index), true) => (index + 1) % count,
            (Some(index), false) => (index + count - 1) % count,
            (None, true) => 0,
            (None, false) => count - 1,
        };
        self.picked = Some(self.browse_choices[index].clone());
        self.selection_dirty = true;
        Change::SelectionChanged
    }

    pub(super) fn apply_selected(
        &mut self,
        backend: &mut impl Backend,
        options: &mut PlayerOptions,
        now: Instant,
    ) -> Change {
        if !self.can_apply() || options.window_mode == WindowMode::Borderless {
            return Change::None;
        }
        let Some(selected) = self.picked.clone() else {
            return Change::None;
        };
        self.refresh_choices(backend);
        if !self
            .browse_choices
            .iter()
            .any(|choice| super::picker::same_choice(choice, &selected))
        {
            self.picked = self.accepted_pick(options);
            self.selection_dirty = false;
            return Change::SelectionChanged;
        }
        if options.window_mode == WindowMode::Exclusive {
            return self.request(
                backend,
                options,
                Target {
                    mode: WindowMode::Exclusive,
                    display: Some(selected),
                },
                now,
            );
        }
        if !self.choices.contains(&selected) {
            self.picked = self.accepted_pick(options);
            self.selection_dirty = false;
            return Change::SelectionChanged;
        }
        options.set_display_mode(selected);
        self.accepted.clone_from(&self.picked);
        self.selection_dirty = false;
        Change::Accepted
    }

    pub(super) fn keep(&mut self, options: &mut PlayerOptions) -> Change {
        let Some(pending) = self.pending.take() else {
            return Change::None;
        };
        options.window_mode = pending.candidate.mode;
        self.picker_mode = options.window_mode;
        self.selection_dirty = false;
        options.mark_written(crate::options::keys::WINDOW_MODE);
        if let Some(mode) = pending.candidate.display {
            options.set_display_mode(mode);
        }
        self.accepted.clone_from(&self.picked);
        Change::Accepted
    }

    pub(super) fn revert(&mut self, backend: &mut impl Backend, now: Instant) -> Change {
        let Some(pending) = self.pending.take() else {
            return Change::None;
        };
        self.apply(backend, pending.prior, now);
        // The prior display may have disappeared while the confirm was open.
        self.refresh_choices(backend);
        self.picked = pending
            .prior_picked
            .filter(|mode| self.choices.contains(mode));
        self.selection_dirty = false;
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

impl Controller {
    /// Read once on every redraw, even before session installation and during
    /// Loading. Settling adopts an unwritten actual baseline, never a preference.
    pub(super) fn observe(
        &mut self,
        backend: &mut impl Backend,
        options: Option<&mut PlayerOptions>,
        now: Instant,
    ) -> Change {
        let actual = backend.readback();
        if let Some(until) = self.settle_until {
            if now >= until {
                self.baseline.clone_from(actual);
                self.effective.clone_from(actual);
                self.settle_until = None;
            }
            return Change::None;
        }
        if self.pending.is_some() || self.fallback || *actual == self.baseline {
            return Change::None;
        }
        let Some(options) = options else {
            // Before session installation, keep this reading available for the
            // first frame that can accept and save it.
            return Change::None;
        };
        self.baseline.clone_from(actual);
        self.effective.clone_from(actual);
        options.window_mode = actual.mode;
        self.picker_mode = actual.mode;
        options.mark_written(crate::options::keys::WINDOW_MODE);
        if let Some(mode) = &actual.display {
            options.set_display_mode(mode.clone());
            self.picked = Some(mode.clone());
            self.accepted = Some(mode.clone());
            self.selection_dirty = false;
        }
        Change::Accepted
    }
}
