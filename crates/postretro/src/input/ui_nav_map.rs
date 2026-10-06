// UI navigation from the effective binding table: which nav command a press of
// an input resolves to under the current tree, and which inputs hold a repeat.
// See: context/lib/input.md §7 · context/lib/player_options.md §6

use gilrs::Axis as GilrsAxis;

use super::binding_table::EffectiveTable;
use super::commands::{Command, CommandContext};
use super::types::{AxisHalf, PhysicalInput};
use super::ui_nav::NavIntent;

/// Which UI commands are live: the tree on top of the modal stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiNavContext {
    /// No capturing tree: only `nav_menu` is live.
    Open,
    /// A capturing tree.
    Capture,
    /// A text-entry tree: nav plus the on-screen keyboard shortcuts.
    TextEntry,
}

fn live_in(command: Command, context: UiNavContext) -> bool {
    match (command, context) {
        (Command::NavMenu, UiNavContext::Open | UiNavContext::Capture) => true,
        (Command::NavMenu, UiNavContext::TextEntry) => false,
        (Command::TextBackspace | Command::TextSpace | Command::TextCommit, context) => {
            context == UiNavContext::TextEntry
        }
        (_, UiNavContext::Open) => false,
        (_, UiNavContext::Capture | UiNavContext::TextEntry) => true,
    }
}

/// The nav intent a UI command produces. The text shortcuts produce none.
pub fn nav_intent_for_command(command: Command) -> Option<NavIntent> {
    Some(match command {
        Command::NavUp => NavIntent::Up,
        Command::NavDown => NavIntent::Down,
        Command::NavLeft => NavIntent::Left,
        Command::NavRight => NavIntent::Right,
        Command::NavNext => NavIntent::Next,
        Command::NavPrev => NavIntent::Prev,
        Command::NavTabNext => NavIntent::TabNext,
        Command::NavTabPrev => NavIntent::TabPrev,
        Command::NavConfirm => NavIntent::Confirm,
        Command::NavCancel => NavIntent::Cancel,
        Command::NavMenu => NavIntent::Menu,
        Command::NavOptions => NavIntent::Options,
        _ => return None,
    })
}

const DIRECTIONS: [Command; 4] = [
    Command::NavUp,
    Command::NavDown,
    Command::NavLeft,
    Command::NavRight,
];

/// The UI-context slice of the effective table.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UiNavMap {
    entries: Vec<(PhysicalInput, Command)>,
}

impl UiNavMap {
    pub fn from_table(table: &EffectiveTable) -> Self {
        Self {
            entries: table
                .entries()
                .iter()
                .filter(|e| e.command.context() == CommandContext::Ui)
                .map(|e| (e.input, e.command))
                .collect(),
        }
    }

    /// The UI command a press of `input` resolves to in `context`. Menu and
    /// cancel may share an input; under a capturing tree it acts as cancel.
    pub fn command_for(&self, input: PhysicalInput, context: UiNavContext) -> Option<Command> {
        let mut found = None;
        for (bound, command) in &self.entries {
            if *bound != input || !live_in(*command, context) {
                continue;
            }
            if *command == Command::NavCancel {
                return Some(*command);
            }
            found.get_or_insert(*command);
        }
        found
    }

    /// The nav intent a press of `input` produces in `context`.
    pub fn intent_for(&self, input: PhysicalInput, context: UiNavContext) -> Option<NavIntent> {
        self.command_for(input, context)
            .and_then(nav_intent_for_command)
    }

    pub fn is_bound_to(&self, input: PhysicalInput, command: Command) -> bool {
        self.entries.contains(&(input, command))
    }

    /// Whether `input` drives a nav direction: its release ends a held
    /// direction's repeat. An input no longer bound to a direction never does.
    pub fn binds_direction(&self, input: PhysicalInput) -> bool {
        DIRECTIONS
            .iter()
            .any(|command| self.is_bound_to(input, *command))
    }

    /// Every input bound to a nav direction.
    pub fn direction_inputs(&self) -> impl Iterator<Item = PhysicalInput> + '_ {
        self.entries
            .iter()
            .filter(|(_, command)| DIRECTIONS.contains(command))
            .map(|(input, _)| *input)
    }
}

/// The half-axis input a stick's dominant direction names.
pub fn stick_half_for(stick: StickSide, direction: NavIntent) -> Option<PhysicalInput> {
    let (x, y) = match stick {
        StickSide::Left => (GilrsAxis::LeftStickX, GilrsAxis::LeftStickY),
        StickSide::Right => (GilrsAxis::RightStickX, GilrsAxis::RightStickY),
    };
    Some(match direction {
        NavIntent::Up => PhysicalInput::GamepadAxisHalf(y, AxisHalf::Positive),
        NavIntent::Down => PhysicalInput::GamepadAxisHalf(y, AxisHalf::Negative),
        NavIntent::Left => PhysicalInput::GamepadAxisHalf(x, AxisHalf::Negative),
        NavIntent::Right => PhysicalInput::GamepadAxisHalf(x, AxisHalf::Positive),
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StickSide {
    Left,
    Right,
}

#[cfg(test)]
mod tests {
    use gilrs::Button as GilrsButton;
    use winit::keyboard::KeyCode;

    use super::*;
    use crate::input::binding_table::{AuthorLayer, PlayerLayer};
    use crate::input::input_names::DeviceClass;
    use crate::input::relevance::RelevanceFacts;

    fn map(player: &[(Command, DeviceClass, Vec<Option<PhysicalInput>>)], swap: bool) -> UiNavMap {
        let player = PlayerLayer {
            rows: player
                .iter()
                .map(|(c, d, row)| ((*c, *d), row.clone()))
                .collect(),
        };
        UiNavMap::from_table(&EffectiveTable::build(
            &AuthorLayer::default(),
            &player,
            RelevanceFacts::default(),
            swap,
        ))
    }

    fn key(code: KeyCode) -> PhysicalInput {
        PhysicalInput::Key(code)
    }

    #[test]
    fn engine_defaults_resolve_todays_nav_keys_and_buttons() {
        let nav = map(&[], false);
        use UiNavContext::*;
        assert_eq!(
            nav.intent_for(key(KeyCode::ArrowUp), Capture),
            Some(NavIntent::Up)
        );
        assert_eq!(
            nav.intent_for(key(KeyCode::Tab), Capture),
            Some(NavIntent::Next)
        );
        assert_eq!(
            nav.intent_for(key(KeyCode::Enter), Capture),
            Some(NavIntent::Confirm)
        );
        assert_eq!(
            nav.intent_for(key(KeyCode::Escape), Capture),
            Some(NavIntent::Cancel)
        );
        assert_eq!(
            nav.intent_for(key(KeyCode::Escape), Open),
            Some(NavIntent::Menu)
        );
        assert_eq!(nav.intent_for(key(KeyCode::ArrowUp), Open), None);
        let pad = |b| PhysicalInput::GamepadButton(b);
        assert_eq!(
            nav.intent_for(pad(GilrsButton::South), Capture),
            Some(NavIntent::Confirm)
        );
        assert_eq!(
            nav.intent_for(pad(GilrsButton::East), Capture),
            Some(NavIntent::Cancel)
        );
        assert_eq!(
            nav.intent_for(pad(GilrsButton::Start), Open),
            Some(NavIntent::Menu)
        );
        assert_eq!(
            nav.intent_for(pad(GilrsButton::Start), Capture),
            Some(NavIntent::Menu)
        );
        assert_eq!(
            nav.intent_for(pad(GilrsButton::Select), Capture),
            Some(NavIntent::Options)
        );
        assert_eq!(
            nav.intent_for(pad(GilrsButton::RightTrigger), Capture),
            Some(NavIntent::TabNext)
        );
        assert_eq!(
            nav.intent_for(pad(GilrsButton::LeftTrigger), Capture),
            Some(NavIntent::TabPrev)
        );
        assert_eq!(
            nav.command_for(pad(GilrsButton::Start), TextEntry),
            Some(Command::TextCommit)
        );
        let stick_up = stick_half_for(StickSide::Left, NavIntent::Up).unwrap();
        assert_eq!(nav.intent_for(stick_up, Capture), Some(NavIntent::Up));
    }

    #[test]
    fn a_direction_rebound_to_w_resolves_from_w_and_no_longer_from_the_arrow() {
        let nav = map(
            &[(
                Command::NavDown,
                DeviceClass::KeyboardMouse,
                vec![Some(key(KeyCode::KeyW))],
            )],
            false,
        );
        assert_eq!(
            nav.intent_for(key(KeyCode::KeyW), UiNavContext::Capture),
            Some(NavIntent::Down)
        );
        assert_eq!(
            nav.intent_for(key(KeyCode::ArrowDown), UiNavContext::Capture),
            None
        );
        assert!(nav.binds_direction(key(KeyCode::KeyW)));
        assert!(!nav.binds_direction(key(KeyCode::ArrowDown)));
    }

    #[test]
    fn the_swap_makes_east_confirm_and_south_cancel() {
        let nav = map(&[], true);
        let pad = |b| PhysicalInput::GamepadButton(b);
        assert_eq!(
            nav.intent_for(pad(GilrsButton::East), UiNavContext::Capture),
            Some(NavIntent::Confirm)
        );
        assert_eq!(
            nav.intent_for(pad(GilrsButton::South), UiNavContext::Capture),
            Some(NavIntent::Cancel)
        );
        assert_eq!(
            nav.intent_for(key(KeyCode::Enter), UiNavContext::Capture),
            Some(NavIntent::Confirm)
        );
    }

    #[test]
    fn binding_nav_down_to_the_right_stick_navigates_with_it() {
        let right_down = stick_half_for(StickSide::Right, NavIntent::Down).unwrap();
        let nav = map(
            &[(
                Command::NavDown,
                DeviceClass::Gamepad,
                vec![Some(right_down)],
            )],
            false,
        );
        assert_eq!(
            nav.intent_for(right_down, UiNavContext::Capture),
            Some(NavIntent::Down)
        );
    }
}
