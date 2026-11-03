//! Render-rate weapon edges, consumed by real fixed-tick input commands.
use super::{Action, ActionSnapshot, ButtonState};
use postretro_foundation::{ActivationInput, ActivationLane, ActivationRelease, ActivationToken};

#[derive(Debug, Default)]
pub struct ActivationInputCapture {
    pressed: [bool; 2],
    released: [bool; 2],
    active: Option<ActivationToken>,
    pending_cancel: Option<ActivationToken>,
    local_tick: u32,
}
impl ActivationInputCapture {
    pub fn observe(&mut self, snapshot: &ActionSnapshot) {
        for (index, action) in [Action::Shoot, Action::AltFire].into_iter().enumerate() {
            self.pressed[index] |= snapshot.button(action) == ButtonState::Pressed;
            self.released[index] |= snapshot.button(action) == ButtonState::Released;
        }
    }
    pub fn suspend(&mut self) {
        if let Some(token) = self.active.take()
            && self.pending_cancel != Some(token)
        {
            self.pending_cancel = Some(token);
        }
        self.pressed = [false; 2];
        self.released = [false; 2];
    }
    /// Predicted idle/recovery logic calls this for a held restart; no host path
    /// invents a restart from a synthetic held command.
    #[cfg(test)]
    pub fn request_restart(&mut self, lane: ActivationLane) {
        self.pressed[lane as usize] = true;
    }
    pub fn set_active(&mut self, token: Option<ActivationToken>) {
        self.active = token.filter(|token| self.pending_cancel != Some(*token));
    }
    pub fn terminal(&mut self, token: ActivationToken) {
        if self.active == Some(token) {
            self.active = None;
        }
    }
    pub fn next_local_tick(&mut self) -> u32 {
        let tick = self.local_tick;
        self.local_tick = tick.wrapping_add(1);
        tick
    }
    pub fn command(&mut self, tick: u32) -> ActivationInput {
        let mut input = ActivationInput {
            cancel: self.pending_cancel.take(),
            ..ActivationInput::default()
        };
        let lane = if self.pressed[1] {
            Some(ActivationLane::Secondary)
        } else if self.pressed[0] {
            Some(ActivationLane::Primary)
        } else {
            None
        };
        if let Some(lane) = lane {
            let token = ActivationToken {
                start_tick: tick,
                lane,
            };
            input.initiation = Some(token);
            // A blocked press is consumed by the executor, never held in this latch.
            if self.active.is_none() {
                self.active = Some(token);
            }
        }
        if let Some(token) = self
            .active
            .filter(|token| self.released[token.lane as usize])
        {
            input.release = Some(ActivationRelease {
                token,
                release_tick: tick,
            });
        }
        self.pressed = [false; 2];
        self.released = [false; 2];
        input
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activation_capture_preserves_press_release_before_fixed_tick() {
        let mut capture = ActivationInputCapture::default();
        let mut snapshot = ActionSnapshot::neutral();
        snapshot
            .button_states
            .insert(Action::AltFire, ButtonState::Pressed);
        capture.observe(&snapshot);
        snapshot
            .button_states
            .insert(Action::AltFire, ButtonState::Released);
        capture.observe(&snapshot);
        let input = capture.command(44);
        assert_eq!(input.initiation.unwrap().lane, ActivationLane::Secondary);
        assert_eq!(input.release.unwrap().token, input.initiation.unwrap());
        assert!(capture.command(45).initiation.is_none());
    }
    #[test]
    fn activation_capture_rejected_old_token_does_not_capture_new_quick_release() {
        let mut capture = ActivationInputCapture::default();
        capture.request_restart(ActivationLane::Primary);
        let old = capture.command(8).initiation.unwrap();
        capture.terminal(old);
        let mut edge = ActionSnapshot::neutral();
        edge.button_states
            .insert(Action::AltFire, ButtonState::Pressed);
        capture.observe(&edge);
        edge.button_states
            .insert(Action::AltFire, ButtonState::Released);
        capture.observe(&edge);
        let new = capture.command(9);
        assert_eq!(new.release.unwrap().token, new.initiation.unwrap());
        capture.terminal(old);
        assert_eq!(
            capture.active, new.initiation,
            "stale terminal cannot clear newer token"
        );
    }
    /// A suspension on a frame with no fixed tick latches its cancel for the
    /// next ticked command, once, however often UI capture repeats before it.
    #[test]
    fn activation_capture_zero_tick_cancel_rides_the_next_fixed_tick_once() {
        let mut capture = ActivationInputCapture::default();
        capture.request_restart(ActivationLane::Primary);
        let token = capture.command(8).initiation.unwrap();
        capture.suspend();
        // A UI capture/refocus can repeat without any real command.
        capture.set_active(Some(token));
        capture.suspend();
        assert_eq!(capture.command(9).cancel, Some(token));
        assert_eq!(capture.command(10).cancel, None);
    }
}
