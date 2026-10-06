// Saved binding rows → the player layer. Unknown commands are ignored (their
// rows stay in the file); an unreadable input falls back for its slot alone.
// See: context/lib/player_options.md §6

use std::collections::HashMap;

use super::binding_table::PlayerLayer;
use super::commands::{Command, CommandKind};
use super::input_names::{DeviceClass, is_whole_axis, parse_input};
use super::types::PhysicalInput;

/// Whether `input` may drive `command` on `class`: the right device, and a
/// whole axis exactly when the command is analog.
pub fn input_fits(command: Command, class: DeviceClass, input: PhysicalInput) -> bool {
    DeviceClass::of(input) == class
        && (command.kind() == CommandKind::Analog) == is_whole_axis(input)
}

fn class_from_settings_key(key: &str) -> Option<DeviceClass> {
    DeviceClass::ALL
        .into_iter()
        .find(|class| class.settings_key() == key)
}

/// Build the player layer from saved rows `(class key, command ID, inputs)`.
/// A `None` input (a non-string element), a string that names no input, or
/// an input that cannot drive the command on that class stays a `None` slot,
/// which falls back to that slot's author default.
pub fn player_layer_from_rows<'a>(
    rows: impl IntoIterator<Item = (&'a str, &'a str, &'a [Option<String>])>,
) -> PlayerLayer {
    let mut layer = PlayerLayer {
        rows: HashMap::new(),
    };
    for (class_key, command_id, inputs) in rows {
        let (Some(class), Some(command)) = (
            class_from_settings_key(class_key),
            Command::from_id(command_id),
        ) else {
            continue;
        };
        let slots = inputs
            .iter()
            .map(|stored| {
                let parsed = stored
                    .as_deref()
                    .and_then(parse_input)
                    .filter(|input| input_fits(command, class, *input));
                if parsed.is_none() {
                    log::warn!(
                        "[Input] saved binding {stored:?} for `{command_id}` ({class_key}) is not \
                         an input it accepts; that slot uses the author's default"
                    );
                }
                parsed
            })
            .collect();
        layer.rows.insert((command, class), slots);
    }
    layer
}

#[cfg(test)]
mod tests {
    use winit::keyboard::KeyCode;

    use super::*;

    fn row(inputs: &[Option<&str>]) -> Vec<Option<String>> {
        inputs.iter().map(|i| i.map(str::to_string)).collect()
    }

    #[test]
    fn rows_parse_by_command_id_and_skip_unknown_commands() {
        let dash = row(&[Some("KeyV")]);
        let unknown = row(&[Some("KeyB")]);
        let layer = player_layer_from_rows([
            ("keyboard_mouse", "dash", dash.as_slice()),
            ("keyboard_mouse", "postretro.dev.dash", unknown.as_slice()),
            ("keyboard_mouse", "grapple", unknown.as_slice()),
        ]);
        assert_eq!(layer.rows.len(), 1);
        assert_eq!(
            layer.rows[&(Command::Dash, DeviceClass::KeyboardMouse)],
            vec![Some(PhysicalInput::Key(KeyCode::KeyV))]
        );
    }

    #[test]
    fn an_unknown_input_string_keeps_its_slot_for_the_author_default() {
        let dash = row(&[Some("KeyNope"), Some("KeyN")]);
        let layer = player_layer_from_rows([("keyboard_mouse", "dash", dash.as_slice())]);
        assert_eq!(
            layer.rows[&(Command::Dash, DeviceClass::KeyboardMouse)],
            vec![None, Some(PhysicalInput::Key(KeyCode::KeyN))]
        );
    }

    #[test]
    fn an_input_from_the_wrong_device_or_kind_falls_back() {
        let look = row(&[Some("KeyW"), Some("mouse_x")]);
        let jump = row(&[Some("south"), Some("mouse_y")]);
        let layer = player_layer_from_rows([
            ("keyboard_mouse", "look_x", look.as_slice()),
            ("keyboard_mouse", "jump", jump.as_slice()),
        ]);
        assert_eq!(
            layer.rows[&(Command::LookX, DeviceClass::KeyboardMouse)],
            vec![None, Some(PhysicalInput::MouseAxisX)]
        );
        assert_eq!(
            layer.rows[&(Command::Jump, DeviceClass::KeyboardMouse)],
            vec![None, None]
        );
    }

    #[test]
    fn an_empty_row_unbinds() {
        let empty: Vec<Option<String>> = Vec::new();
        let layer = player_layer_from_rows([("gamepad", "dash", empty.as_slice())]);
        assert!(layer.rows[&(Command::Dash, DeviceClass::Gamepad)].is_empty());
    }
}
