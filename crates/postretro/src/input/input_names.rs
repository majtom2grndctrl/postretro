// Input strings: physical-position names shared by the manifest and settings.
// See: context/lib/input.md §2 (Inputs by physical position)

use gilrs::{Axis as GilrsAxis, Button as GilrsButton};
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

use super::types::{AxisHalf, PhysicalInput};

/// The device class an input belongs to. Defaults and saved bindings are kept
/// per class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DeviceClass {
    KeyboardMouse,
    Gamepad,
}

impl DeviceClass {
    pub const ALL: [DeviceClass; 2] = [DeviceClass::KeyboardMouse, DeviceClass::Gamepad];

    /// The manifest key (`keyboardMouse`, `gamepad`).
    #[allow(dead_code)]
    pub const fn manifest_key(self) -> &'static str {
        match self {
            DeviceClass::KeyboardMouse => "keyboardMouse",
            DeviceClass::Gamepad => "gamepad",
        }
    }

    /// The settings table key (`keyboard_mouse`, `gamepad`).
    #[allow(dead_code)]
    pub const fn settings_key(self) -> &'static str {
        match self {
            DeviceClass::KeyboardMouse => "keyboard_mouse",
            DeviceClass::Gamepad => "gamepad",
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub const fn of(input: PhysicalInput) -> Self {
        match input {
            PhysicalInput::Key(_)
            | PhysicalInput::MouseButton(_)
            | PhysicalInput::MouseWheelUp
            | PhysicalInput::MouseWheelDown
            | PhysicalInput::MouseAxisX
            | PhysicalInput::MouseAxisY => DeviceClass::KeyboardMouse,
            PhysicalInput::GamepadButton(_)
            | PhysicalInput::GamepadAxis(_)
            | PhysicalInput::GamepadAxisHalf(_, _) => DeviceClass::Gamepad,
        }
    }
}

/// Keyboard keys by W3C `KeyboardEvent.code`. winit's `KeyCode` variants carry
/// the same names except the OS keys, which W3C calls `Meta`.
macro_rules! same_name_keys {
    ($($key:ident),* $(,)?) => {
        &[$((KeyCode::$key, stringify!($key))),*]
    };
}

const KEYS: &[(KeyCode, &str)] = same_name_keys![
    KeyA,
    KeyB,
    KeyC,
    KeyD,
    KeyE,
    KeyF,
    KeyG,
    KeyH,
    KeyI,
    KeyJ,
    KeyK,
    KeyL,
    KeyM,
    KeyN,
    KeyO,
    KeyP,
    KeyQ,
    KeyR,
    KeyS,
    KeyT,
    KeyU,
    KeyV,
    KeyW,
    KeyX,
    KeyY,
    KeyZ,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    Backquote,
    Minus,
    Equal,
    BracketLeft,
    BracketRight,
    Backslash,
    Semicolon,
    Quote,
    Comma,
    Period,
    Slash,
    IntlBackslash,
    CapsLock,
    Tab,
    Enter,
    Escape,
    Space,
    Backspace,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    ContextMenu,
    PrintScreen,
    ScrollLock,
    Pause,
    NumLock,
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    NumpadAdd,
    NumpadSubtract,
    NumpadMultiply,
    NumpadDivide,
    NumpadDecimal,
    NumpadEnter,
];

const META_KEYS: &[(KeyCode, &str)] = &[
    (KeyCode::SuperLeft, "MetaLeft"),
    (KeyCode::SuperRight, "MetaRight"),
];

const MOUSE: &[(PhysicalInput, &str)] = &[
    (PhysicalInput::MouseButton(MouseButton::Left), "mouse_left"),
    (
        PhysicalInput::MouseButton(MouseButton::Right),
        "mouse_right",
    ),
    (
        PhysicalInput::MouseButton(MouseButton::Middle),
        "mouse_middle",
    ),
    (PhysicalInput::MouseButton(MouseButton::Back), "mouse_back"),
    (
        PhysicalInput::MouseButton(MouseButton::Forward),
        "mouse_forward",
    ),
    (PhysicalInput::MouseWheelUp, "wheel_up"),
    (PhysicalInput::MouseWheelDown, "wheel_down"),
    (PhysicalInput::MouseAxisX, "mouse_x"),
    (PhysicalInput::MouseAxisY, "mouse_y"),
];

const GAMEPAD: &[(PhysicalInput, &str)] = &[
    (PhysicalInput::GamepadButton(GilrsButton::South), "south"),
    (PhysicalInput::GamepadButton(GilrsButton::East), "east"),
    (PhysicalInput::GamepadButton(GilrsButton::West), "west"),
    (PhysicalInput::GamepadButton(GilrsButton::North), "north"),
    (
        PhysicalInput::GamepadButton(GilrsButton::LeftTrigger),
        "left_shoulder",
    ),
    (
        PhysicalInput::GamepadButton(GilrsButton::RightTrigger),
        "right_shoulder",
    ),
    (
        PhysicalInput::GamepadButton(GilrsButton::LeftTrigger2),
        "left_trigger",
    ),
    (
        PhysicalInput::GamepadButton(GilrsButton::RightTrigger2),
        "right_trigger",
    ),
    (PhysicalInput::GamepadButton(GilrsButton::Select), "select"),
    (PhysicalInput::GamepadButton(GilrsButton::Start), "start"),
    (
        PhysicalInput::GamepadButton(GilrsButton::LeftThumb),
        "left_stick_press",
    ),
    (
        PhysicalInput::GamepadButton(GilrsButton::RightThumb),
        "right_stick_press",
    ),
    (PhysicalInput::GamepadButton(GilrsButton::DPadUp), "dpad_up"),
    (
        PhysicalInput::GamepadButton(GilrsButton::DPadDown),
        "dpad_down",
    ),
    (
        PhysicalInput::GamepadButton(GilrsButton::DPadLeft),
        "dpad_left",
    ),
    (
        PhysicalInput::GamepadButton(GilrsButton::DPadRight),
        "dpad_right",
    ),
    (
        PhysicalInput::GamepadAxis(GilrsAxis::LeftStickX),
        "left_stick_x",
    ),
    (
        PhysicalInput::GamepadAxis(GilrsAxis::LeftStickY),
        "left_stick_y",
    ),
    (
        PhysicalInput::GamepadAxis(GilrsAxis::RightStickX),
        "right_stick_x",
    ),
    (
        PhysicalInput::GamepadAxis(GilrsAxis::RightStickY),
        "right_stick_y",
    ),
    (
        PhysicalInput::GamepadAxisHalf(GilrsAxis::LeftStickY, AxisHalf::Positive),
        "left_stick_up",
    ),
    (
        PhysicalInput::GamepadAxisHalf(GilrsAxis::LeftStickY, AxisHalf::Negative),
        "left_stick_down",
    ),
    (
        PhysicalInput::GamepadAxisHalf(GilrsAxis::LeftStickX, AxisHalf::Negative),
        "left_stick_left",
    ),
    (
        PhysicalInput::GamepadAxisHalf(GilrsAxis::LeftStickX, AxisHalf::Positive),
        "left_stick_right",
    ),
    (
        PhysicalInput::GamepadAxisHalf(GilrsAxis::RightStickY, AxisHalf::Positive),
        "right_stick_up",
    ),
    (
        PhysicalInput::GamepadAxisHalf(GilrsAxis::RightStickY, AxisHalf::Negative),
        "right_stick_down",
    ),
    (
        PhysicalInput::GamepadAxisHalf(GilrsAxis::RightStickX, AxisHalf::Negative),
        "right_stick_left",
    ),
    (
        PhysicalInput::GamepadAxisHalf(GilrsAxis::RightStickX, AxisHalf::Positive),
        "right_stick_right",
    ),
];

/// The input string for a physical input, or `None` for an input that has no
/// bindable name (an unlisted key, a trigger's raw axis).
#[cfg_attr(not(test), allow(dead_code))]
pub fn input_name(input: PhysicalInput) -> Option<&'static str> {
    if let PhysicalInput::Key(code) = input {
        return KEYS
            .iter()
            .chain(META_KEYS)
            .find(|(key, _)| *key == code)
            .map(|(_, name)| *name);
    }
    MOUSE
        .iter()
        .chain(GAMEPAD)
        .find(|(candidate, _)| *candidate == input)
        .map(|(_, name)| *name)
}

/// A display label for an input: its name split into upper-case words, with
/// the W3C `Key`/`Digit` prefixes dropped (`KeyW` → `W`, `ShiftLeft` →
/// `SHIFT LEFT`, `left_stick_up` → `LEFT STICK UP`). Glyph art replaces these
/// where a mod ships it.
pub fn input_label(input: PhysicalInput) -> String {
    let Some(name) = input_name(input) else {
        return "?".to_string();
    };
    let name = name
        .strip_prefix("Key")
        .or_else(|| name.strip_prefix("Digit"))
        .filter(|rest| rest.len() == 1)
        .unwrap_or(name);
    let mut out = String::with_capacity(name.len() + 4);
    let mut previous_lower = false;
    for c in name.chars() {
        if c == '_' {
            out.push(' ');
            previous_lower = false;
            continue;
        }
        if c.is_ascii_uppercase() && previous_lower {
            out.push(' ');
        }
        previous_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        out.push(c.to_ascii_uppercase());
    }
    out
}

/// Parse an input string from a manifest or settings row.
#[cfg_attr(not(test), allow(dead_code))]
pub fn parse_input(name: &str) -> Option<PhysicalInput> {
    KEYS.iter()
        .chain(META_KEYS)
        .find(|(_, key_name)| *key_name == name)
        .map(|(key, _)| PhysicalInput::Key(*key))
        .or_else(|| {
            MOUSE
                .iter()
                .chain(GAMEPAD)
                .find(|(_, input_name)| *input_name == name)
                .map(|(input, _)| *input)
        })
}

/// Every input with a bindable name.
#[allow(dead_code)]
pub fn named_inputs() -> impl Iterator<Item = PhysicalInput> {
    KEYS.iter()
        .chain(META_KEYS)
        .map(|(key, _)| PhysicalInput::Key(*key))
        .chain(MOUSE.iter().chain(GAMEPAD).map(|(input, _)| *input))
}

/// Analog polarity per source, in physical terms: + is right or up. The OS
/// reports mouse motion downward as +, so `mouse_y` alone is negated. A
/// command's own sign then maps the physical direction onto its action.
pub const fn analog_polarity(input: PhysicalInput) -> f32 {
    match input {
        PhysicalInput::MouseAxisY => -1.0,
        PhysicalInput::MouseAxisX
        | PhysicalInput::GamepadAxis(_)
        | PhysicalInput::GamepadAxisHalf(_, _)
        | PhysicalInput::Key(_)
        | PhysicalInput::MouseButton(_)
        | PhysicalInput::MouseWheelUp
        | PhysicalInput::MouseWheelDown
        | PhysicalInput::GamepadButton(_) => 1.0,
    }
}

/// Whether an input is a whole axis (bindable only to analog commands).
pub const fn is_whole_axis(input: PhysicalInput) -> bool {
    matches!(
        input,
        PhysicalInput::MouseAxisX | PhysicalInput::MouseAxisY | PhysicalInput::GamepadAxis(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_input_name_round_trips() {
        let mut count = 0;
        for input in named_inputs() {
            let name = input_name(input).expect("a named input has a name");
            assert_eq!(parse_input(name), Some(input), "{name}");
            count += 1;
        }
        assert!(count > 150, "only {count} named inputs");
    }

    #[test]
    fn input_names_are_unique() {
        let mut names: Vec<_> = named_inputs().filter_map(input_name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total);
    }

    #[test]
    fn w3c_names_cover_the_examples_and_os_keys() {
        assert_eq!(parse_input("KeyW"), Some(PhysicalInput::Key(KeyCode::KeyW)));
        assert_eq!(
            parse_input("ShiftLeft"),
            Some(PhysicalInput::Key(KeyCode::ShiftLeft))
        );
        assert_eq!(
            parse_input("MetaLeft"),
            Some(PhysicalInput::Key(KeyCode::SuperLeft))
        );
        assert_eq!(parse_input("SuperLeft"), None);
        assert_eq!(parse_input("keyw"), None);
    }

    #[test]
    fn half_axis_names_are_physical_directions() {
        assert_eq!(
            parse_input("left_stick_up"),
            Some(PhysicalInput::GamepadAxisHalf(
                GilrsAxis::LeftStickY,
                AxisHalf::Positive
            ))
        );
        assert_eq!(AxisHalf::Positive.magnitude(0.7), 0.7);
        assert_eq!(AxisHalf::Negative.magnitude(0.7), 0.0);
        assert_eq!(AxisHalf::Negative.magnitude(-0.4), 0.4);
    }
}
