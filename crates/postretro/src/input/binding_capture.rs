// The controls panel's capture prompt: which binding slot it waits on and the
// first input pressed while it is the active tree. The App swallows every
// input the prompt sees and resolves the candidate after that frame's
// activations.
// See: context/lib/input.md §2 · context/lib/player_options.md §6

use super::commands::{Command, CommandKind};
use super::input_names::DeviceClass;
use super::player_rows::input_fits;
use super::types::PhysicalInput;

/// Accumulated mouse travel, in raw device units, that captures a mouse axis
/// for an analog command: far enough that a resting hand never binds one.
pub const MOUSE_AXIS_CAPTURE_DISTANCE: f64 = 60.0;

/// The binding slot a capture prompt writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureTarget {
    pub command: Command,
    pub class: DeviceClass,
    pub slot: usize,
}

/// One open capture prompt. It has no time limit, and every input it accepts
/// stays capturable: Escape, Start and Select bind like any other input.
#[derive(Debug, Clone, PartialEq)]
pub struct BindingCapture {
    target: CaptureTarget,
    candidate: Option<PhysicalInput>,
    mouse_travel: [f64; 2],
}

impl BindingCapture {
    pub fn new(target: CaptureTarget) -> Self {
        Self {
            target,
            candidate: None,
            mouse_travel: [0.0; 2],
        }
    }

    pub fn target(&self) -> CaptureTarget {
        self.target
    }

    /// Offer a press the prompt saw while it was the active tree. The caller
    /// drops OS key repeats, so a key held since before the prompt opened is
    /// captured only when pressed again (P7). The first press that fits the
    /// slot is the candidate; a stick half captures its whole axis for an
    /// analog command. Inputs of the other device class are ignored.
    pub fn offer_press(&mut self, input: PhysicalInput) {
        if self.candidate.is_none() {
            self.candidate = fit(self.target, input);
        }
    }

    /// Offer raw mouse motion. An analog keyboard-and-mouse slot captures the
    /// axis that travels `MOUSE_AXIS_CAPTURE_DISTANCE` first.
    pub fn offer_mouse_motion(&mut self, dx: f64, dy: f64) {
        if self.candidate.is_some()
            || self.target.class != DeviceClass::KeyboardMouse
            || self.target.command.kind() != CommandKind::Analog
        {
            return;
        }
        self.mouse_travel[0] += dx.abs();
        self.mouse_travel[1] += dy.abs();
        let [x, y] = self.mouse_travel;
        if x.max(y) >= MOUSE_AXIS_CAPTURE_DISTANCE {
            self.candidate = Some(if x >= y {
                PhysicalInput::MouseAxisX
            } else {
                PhysicalInput::MouseAxisY
            });
        }
    }

    /// The captured input, once; the prompt resolves it after the frame's
    /// activations.
    pub fn take_candidate(&mut self) -> Option<PhysicalInput> {
        self.candidate.take()
    }
}

/// The input a press binds to the target slot, if it can drive the command on
/// the slot's class.
fn fit(target: CaptureTarget, input: PhysicalInput) -> Option<PhysicalInput> {
    let input = match (target.command.kind(), input) {
        (CommandKind::Analog, PhysicalInput::GamepadAxisHalf(axis, _)) => {
            PhysicalInput::GamepadAxis(axis)
        }
        _ => input,
    };
    input_fits(target.command, target.class, input).then_some(input)
}

#[cfg(test)]
mod tests {
    use gilrs::{Axis, Button};
    use winit::keyboard::KeyCode;

    use super::*;
    use crate::input::types::AxisHalf;

    fn capture(command: Command, class: DeviceClass) -> BindingCapture {
        BindingCapture::new(CaptureTarget {
            command,
            class,
            slot: 0,
        })
    }

    #[test]
    fn the_first_fitting_press_is_the_candidate() {
        let mut prompt = capture(Command::Jump, DeviceClass::Gamepad);
        prompt.offer_press(PhysicalInput::Key(KeyCode::Space));
        prompt.offer_press(PhysicalInput::GamepadButton(Button::Start));
        prompt.offer_press(PhysicalInput::GamepadButton(Button::South));
        assert_eq!(
            prompt.take_candidate(),
            Some(PhysicalInput::GamepadButton(Button::Start)),
            "a keyboard press is ignored on a gamepad slot; Start is capturable"
        );
        assert_eq!(prompt.take_candidate(), None);
    }

    #[test]
    fn escape_is_capturable() {
        let mut prompt = capture(Command::NavCancel, DeviceClass::KeyboardMouse);
        prompt.offer_press(PhysicalInput::Key(KeyCode::Escape));
        assert_eq!(
            prompt.take_candidate(),
            Some(PhysicalInput::Key(KeyCode::Escape))
        );
    }

    #[test]
    fn a_stick_half_captures_its_whole_axis_for_an_analog_command() {
        let mut look = capture(Command::LookX, DeviceClass::Gamepad);
        look.offer_press(PhysicalInput::GamepadButton(Button::South));
        look.offer_press(PhysicalInput::GamepadAxisHalf(
            Axis::RightStickX,
            AxisHalf::Negative,
        ));
        assert_eq!(
            look.take_candidate(),
            Some(PhysicalInput::GamepadAxis(Axis::RightStickX)),
            "a button cannot drive an analog command"
        );

        let mut forward = capture(Command::MoveForward, DeviceClass::Gamepad);
        let half = PhysicalInput::GamepadAxisHalf(Axis::LeftStickY, AxisHalf::Positive);
        forward.offer_press(half);
        assert_eq!(forward.take_candidate(), Some(half));
    }

    #[test]
    fn mouse_travel_captures_an_axis_for_an_analog_keyboard_mouse_slot() {
        let mut look = capture(Command::LookY, DeviceClass::KeyboardMouse);
        look.offer_mouse_motion(5.0, -20.0);
        assert_eq!(look.take_candidate(), None, "a small drift binds nothing");
        look.offer_mouse_motion(10.0, -45.0);
        assert_eq!(look.take_candidate(), Some(PhysicalInput::MouseAxisY));

        let mut jump = capture(Command::Jump, DeviceClass::KeyboardMouse);
        jump.offer_mouse_motion(500.0, 0.0);
        assert_eq!(jump.take_candidate(), None, "motion never binds a digital command");
    }
}
