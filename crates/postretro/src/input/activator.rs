// Binding activators: press, release, tap, hold, and the shared-key rule.
// See: context/lib/input.md §2 (Commands, activators, and layering)

use std::collections::{HashMap, HashSet};

use super::input_names::DeviceClass;
use super::types::{Action, ActivatorKind, Binding, PhysicalInput};

/// One physical input that is currently down and resolving.
#[derive(Debug, Clone, Copy)]
struct HeldInput {
    down_at: f64,
    /// `hold_timing_scale` captured when the input went down, so a scale change
    /// mid-hold never moves this press's thresholds.
    scale: f32,
    /// The hold binding(s) on this input have fired; the short partner never will.
    hold_fired: bool,
    /// A rebuild changed this input's bindings while it was down. A resolution
    /// already made stands until release; nothing new fires on this press.
    stale: bool,
}

/// Resolves timed input edges into per-binding down levels and per-command
/// down edges. Activators resolve here, client-side, before any snapshot or
/// sim command is built; the wire carries only the resolved phases.
///
/// Within one input, at most one short binding (`press`, `release`, `tap`) and
/// one `hold` share the key (the Steam rule; the conflict checker enforces it).
/// On a shared input the hold fires once its min elapses while the input is
/// down, and the short binding fires only on a release before that min.
#[derive(Debug, Default)]
pub(super) struct ActivatorResolver {
    held: HashMap<PhysicalInput, HeldInput>,
    /// Inputs held across a cancel. A level-sourced input (a polled gamepad
    /// button) stays inert until it is observed up, so a button held through a
    /// capturing menu fires nothing when the menu closes.
    suppressed: HashSet<PhysicalInput>,
    /// Down level per binding index, aligned with the binding table.
    binding_down: Vec<bool>,
    /// Down count per command; a 0→1 transition is the command's press edge.
    command_down: HashMap<Action, u32>,
    /// Press edges per command since the last `take_frame`: two presses
    /// between snapshots count two, so a notch-read command steps twice.
    went_down: HashMap<Action, u32>,
}

impl ActivatorResolver {
    /// Resize per-binding state for a binding table and drop all resolution.
    pub(super) fn reset_for(&mut self, binding_count: usize) {
        self.held.clear();
        self.binding_down.clear();
        self.binding_down.resize(binding_count, false);
        self.command_down.clear();
        self.went_down.clear();
    }

    pub(super) fn binding_down(&self, index: usize) -> bool {
        self.binding_down.get(index).copied().unwrap_or(false)
    }

    pub(super) fn command_went_down(&self, action: Action) -> bool {
        self.went_down.contains_key(&action)
    }

    /// How many times the command went down since the last `take_frame`.
    pub(super) fn command_press_count(&self, action: Action) -> u32 {
        self.went_down.get(&action).copied().unwrap_or(0)
    }

    /// Clear the per-frame edge record after a snapshot has read it.
    pub(super) fn take_frame(&mut self) {
        self.went_down.clear();
    }

    /// An authoritative edge from an event source (winit key or button event,
    /// gilrs button event). Any event edge clears suppression: a press is
    /// fresh, and a release means the next press is too, whichever source
    /// reports it.
    pub(super) fn event_edge(
        &mut self,
        bindings: &[Binding],
        input: PhysicalInput,
        down: bool,
        t: f64,
        scale: f32,
    ) {
        self.suppressed.remove(&input);
        self.edge(bindings, input, down, t, scale);
    }

    /// An edge inferred from a polled level. A suppressed input stays inert
    /// until the poll reports it up.
    pub(super) fn level_edge(
        &mut self,
        bindings: &[Binding],
        input: PhysicalInput,
        down: bool,
        t: f64,
        scale: f32,
    ) {
        if self.suppressed.contains(&input) {
            if !down {
                self.suppressed.remove(&input);
            }
            return;
        }
        self.edge(bindings, input, down, t, scale);
    }

    fn edge(&mut self, bindings: &[Binding], input: PhysicalInput, down: bool, t: f64, scale: f32) {
        if down {
            self.input_down(bindings, input, t, scale);
        } else {
            self.input_up(bindings, input, t);
        }
    }

    fn input_down(&mut self, bindings: &[Binding], input: PhysicalInput, t: f64, scale: f32) {
        // Untracked inputs bind nothing; a repeat down of a held input is noise.
        if self.held.contains_key(&input) || !bindings.iter().any(|b| b.input == input) {
            return;
        }
        self.held.insert(
            input,
            HeldInput {
                down_at: t,
                scale,
                hold_fired: false,
                stale: false,
            },
        );
        if shares_hold(bindings, input) {
            return;
        }
        for (index, binding) in bindings.iter().enumerate() {
            if binding.input == input && binding.activator.kind == ActivatorKind::Press {
                self.set_down(bindings, index);
            }
        }
    }

    fn input_up(&mut self, bindings: &[Binding], input: PhysicalInput, t: f64) {
        let Some(mut held) = self.held.remove(&input) else {
            // Never seen down (or newly bound by a rebuild while held): nothing
            // resolves on this release.
            return;
        };
        if held.stale {
            for (index, binding) in bindings.iter().enumerate() {
                if binding.input == input {
                    self.set_up(bindings, index);
                }
            }
            return;
        }
        let elapsed = t - held.down_at;
        // A hold whose min passed before this release fired first, even if no
        // frame advanced time across the threshold in between.
        if !held.hold_fired && self.hold_threshold_passed(bindings, input, &held, elapsed) {
            self.fire_holds(bindings, input);
            held.hold_fired = true;
        }
        let shared = shares_hold(bindings, input);
        for (index, binding) in bindings.iter().enumerate() {
            if binding.input != input {
                continue;
            }
            let threshold = f64::from(binding.activator.threshold * held.scale);
            match binding.activator.kind {
                ActivatorKind::Hold => self.set_up(bindings, index),
                ActivatorKind::Press if !shared => self.set_up(bindings, index),
                ActivatorKind::Press | ActivatorKind::Release if !held.hold_fired => {
                    self.pulse(bindings, index);
                }
                // Release at exactly the tap's max counts as within it.
                ActivatorKind::Tap if !held.hold_fired && elapsed <= threshold => {
                    self.pulse(bindings, index);
                }
                ActivatorKind::Press | ActivatorKind::Release | ActivatorKind::Tap => {}
            }
        }
    }

    /// Fire every hold whose min has elapsed by `now` on an input still down.
    pub(super) fn advance(&mut self, bindings: &[Binding], now: f64) {
        let due: Vec<PhysicalInput> = self
            .held
            .iter()
            .filter(|(input, held)| {
                !held.hold_fired
                    && !held.stale
                    && self.hold_threshold_passed(bindings, **input, held, now - held.down_at)
            })
            .map(|(input, _)| *input)
            .collect();
        for input in due {
            self.fire_holds(bindings, input);
            if let Some(held) = self.held.get_mut(&input) {
                held.hold_fired = true;
            }
        }
    }

    /// Cancel every pending resolution and lift every down binding without a
    /// pulse: losing window focus or a capturing tree opening fires neither
    /// binding of a pending pair. Held inputs are suppressed until seen up,
    /// and press edges not yet read are dropped, so nothing pressed before the
    /// cancel reads Pressed after it.
    pub(super) fn cancel_all(&mut self, bindings: &[Binding]) {
        for (input, _) in self.held.drain() {
            self.suppressed.insert(input);
        }
        for index in 0..self.binding_down.len() {
            self.set_up(bindings, index);
        }
        self.went_down.clear();
    }

    /// Forget every gamepad input, as when the pad disconnects: bindings on
    /// pad inputs lift without a pulse, pending pad holds never fire, and pad
    /// suppression drops so a reconnected pad's next press is fresh. Press
    /// edges already resolved this frame stand.
    pub(super) fn cancel_gamepad(&mut self, bindings: &[Binding]) {
        let is_pad = |input: &PhysicalInput| DeviceClass::of(*input) == DeviceClass::Gamepad;
        self.held.retain(|input, _| !is_pad(input));
        self.suppressed.retain(|input| !is_pad(input));
        for (index, binding) in bindings.iter().enumerate() {
            if is_pad(&binding.input) {
                self.set_up(bindings, index);
            }
        }
    }

    /// Carry resolution across a binding-table rebuild. An input whose
    /// bindings are unchanged resolves as if no rebuild happened. On a changed
    /// input, bindings already down and still present stay down until release,
    /// a pending resolution is cancelled, and nothing new fires on this press.
    /// An input the rebuild newly binds is not tracked, so it waits for a
    /// fresh press.
    pub(super) fn rebind(&mut self, old: &[Binding], new: &[Binding]) {
        fn same(a: &Binding, b: &Binding) -> bool {
            a.input == b.input
                && a.action == b.action
                && a.activator == b.activator
                && a.scale == b.scale
        }
        let mut new_down = vec![false; new.len()];
        for (input, held) in &mut self.held {
            let old_on: Vec<usize> = (0..old.len()).filter(|&i| old[i].input == *input).collect();
            let new_on: Vec<usize> = (0..new.len()).filter(|&i| new[i].input == *input).collect();
            let unchanged = old_on.len() == new_on.len()
                && old_on
                    .iter()
                    .zip(&new_on)
                    .all(|(&o, &n)| same(&old[o], &new[n]));
            for &o in &old_on {
                if self.binding_down.get(o).copied().unwrap_or(false)
                    && let Some(&n) = new_on.iter().find(|&&n| same(&old[o], &new[n]))
                {
                    new_down[n] = true;
                }
            }
            if !unchanged {
                held.stale = true;
            }
        }
        self.binding_down = new_down;
        self.command_down.clear();
        for (index, down) in self.binding_down.iter().enumerate() {
            if *down {
                *self.command_down.entry(new[index].action).or_insert(0) += 1;
            }
        }
    }

    /// The hold's min has passed strictly: release at exactly the min counts as
    /// the release, so the short partner wins the tie.
    fn hold_threshold_passed(
        &self,
        bindings: &[Binding],
        input: PhysicalInput,
        held: &HeldInput,
        elapsed: f64,
    ) -> bool {
        bindings
            .iter()
            .filter(|b| b.input == input && b.activator.kind == ActivatorKind::Hold)
            .any(|b| elapsed > f64::from(b.activator.threshold * held.scale))
    }

    fn fire_holds(&mut self, bindings: &[Binding], input: PhysicalInput) {
        for (index, binding) in bindings.iter().enumerate() {
            if binding.input == input && binding.activator.kind == ActivatorKind::Hold {
                self.set_down(bindings, index);
            }
        }
    }

    fn set_down(&mut self, bindings: &[Binding], index: usize) {
        if self.binding_down[index] {
            return;
        }
        self.binding_down[index] = true;
        let count = self.command_down.entry(bindings[index].action).or_insert(0);
        if *count == 0 {
            *self.went_down.entry(bindings[index].action).or_insert(0) += 1;
        }
        *count += 1;
    }

    fn set_up(&mut self, bindings: &[Binding], index: usize) {
        if !self.binding_down[index] {
            return;
        }
        self.binding_down[index] = false;
        if let Some(count) = self.command_down.get_mut(&bindings[index].action) {
            *count = count.saturating_sub(1);
        }
    }

    /// The binding goes down and up within one resolution: the command reads
    /// Pressed on this frame and Released on the next.
    fn pulse(&mut self, bindings: &[Binding], index: usize) {
        self.set_down(bindings, index);
        self.set_up(bindings, index);
    }
}

/// Whether this input carries a hold beside a short binding, so the short
/// binding waits for the release.
fn shares_hold(bindings: &[Binding], input: PhysicalInput) -> bool {
    let mut has_hold = false;
    let mut has_short = false;
    for binding in bindings.iter().filter(|b| b.input == input) {
        match binding.activator.kind {
            ActivatorKind::Hold => has_hold = true,
            ActivatorKind::Press | ActivatorKind::Release | ActivatorKind::Tap => has_short = true,
        }
    }
    has_hold && has_short
}
