// Bindable commands: stable IDs, contexts, accepted inputs and activators.
// See: context/lib/input.md §2 (Commands, activators, and layering)

use super::types::{Action, ActivatorKind};

/// A bindable command. The set is engine-closed: gameplay commands come from
/// the `Action` set, UI commands from the nav intents. Every per-command
/// property below is an exhaustive match, so a new command is a compile error
/// until it is classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Command {
    MoveForward,
    MoveBack,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    LookX,
    LookY,
    Sprint,
    Jump,
    Dash,
    Crouch,
    Use,
    Drop,
    Shoot,
    AltFire,
    Reload,
    SelectWieldable1,
    SelectWieldable2,
    SelectWieldable3,
    SelectWieldable4,
    SelectWieldable5,
    SelectWieldable6,
    SelectWieldable7,
    SelectWieldable8,
    SelectWieldable9,
    SelectWieldable10,
    CycleWieldableNext,
    CycleWieldablePrevious,
    ToggleLastWieldable,
    NavUp,
    NavDown,
    NavLeft,
    NavRight,
    NavNext,
    NavPrev,
    NavTabNext,
    NavTabPrev,
    NavConfirm,
    NavCancel,
    NavMenu,
    NavOptions,
    TextBackspace,
    TextSpace,
    TextCommit,
}

/// Which command set a command belongs to. Conflicts are checked only among
/// commands live in the same context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandContext {
    Gameplay,
    Ui,
}

/// What a command reads from its inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandKind {
    /// Keys, buttons, wheel notches, and half-axis stick inputs. A half-axis
    /// input on a movement command carries its stick magnitude.
    Digital,
    /// Whole axes only: mouse motion and stick axes.
    Analog,
    /// Digital inputs counted as discrete steps (weapon cycling).
    WheelNotch,
}

impl Command {
    pub const ALL: [Command; 45] = [
        Command::MoveForward,
        Command::MoveBack,
        Command::MoveLeft,
        Command::MoveRight,
        Command::MoveUp,
        Command::MoveDown,
        Command::LookX,
        Command::LookY,
        Command::Sprint,
        Command::Jump,
        Command::Dash,
        Command::Crouch,
        Command::Use,
        Command::Drop,
        Command::Shoot,
        Command::AltFire,
        Command::Reload,
        Command::SelectWieldable1,
        Command::SelectWieldable2,
        Command::SelectWieldable3,
        Command::SelectWieldable4,
        Command::SelectWieldable5,
        Command::SelectWieldable6,
        Command::SelectWieldable7,
        Command::SelectWieldable8,
        Command::SelectWieldable9,
        Command::SelectWieldable10,
        Command::CycleWieldableNext,
        Command::CycleWieldablePrevious,
        Command::ToggleLastWieldable,
        Command::NavUp,
        Command::NavDown,
        Command::NavLeft,
        Command::NavRight,
        Command::NavNext,
        Command::NavPrev,
        Command::NavTabNext,
        Command::NavTabPrev,
        Command::NavConfirm,
        Command::NavCancel,
        Command::NavMenu,
        Command::NavOptions,
        Command::TextBackspace,
        Command::TextSpace,
        Command::TextCommit,
    ];

    /// Stable snake_case ID used in the manifest and in saved bindings. Engine
    /// IDs never contain `.`, which reserves `<mod_id>.<name>` for later
    /// mod-defined commands.
    pub const fn id(self) -> &'static str {
        match self {
            Command::MoveForward => "move_forward",
            Command::MoveBack => "move_back",
            Command::MoveLeft => "move_left",
            Command::MoveRight => "move_right",
            Command::MoveUp => "move_up",
            Command::MoveDown => "move_down",
            Command::LookX => "look_x",
            Command::LookY => "look_y",
            Command::Sprint => "sprint",
            Command::Jump => "jump",
            Command::Dash => "dash",
            Command::Crouch => "crouch",
            Command::Use => "use",
            Command::Drop => "drop",
            Command::Shoot => "shoot",
            Command::AltFire => "alt_fire",
            Command::Reload => "reload",
            Command::SelectWieldable1 => "select_wieldable_1",
            Command::SelectWieldable2 => "select_wieldable_2",
            Command::SelectWieldable3 => "select_wieldable_3",
            Command::SelectWieldable4 => "select_wieldable_4",
            Command::SelectWieldable5 => "select_wieldable_5",
            Command::SelectWieldable6 => "select_wieldable_6",
            Command::SelectWieldable7 => "select_wieldable_7",
            Command::SelectWieldable8 => "select_wieldable_8",
            Command::SelectWieldable9 => "select_wieldable_9",
            Command::SelectWieldable10 => "select_wieldable_10",
            Command::CycleWieldableNext => "cycle_wieldable_next",
            Command::CycleWieldablePrevious => "cycle_wieldable_previous",
            Command::ToggleLastWieldable => "toggle_last_wieldable",
            Command::NavUp => "nav_up",
            Command::NavDown => "nav_down",
            Command::NavLeft => "nav_left",
            Command::NavRight => "nav_right",
            Command::NavNext => "nav_next",
            Command::NavPrev => "nav_prev",
            Command::NavTabNext => "nav_tab_next",
            Command::NavTabPrev => "nav_tab_prev",
            Command::NavConfirm => "nav_confirm",
            Command::NavCancel => "nav_cancel",
            Command::NavMenu => "nav_menu",
            Command::NavOptions => "nav_options",
            Command::TextBackspace => "text_backspace",
            Command::TextSpace => "text_space",
            Command::TextCommit => "text_commit",
        }
    }

    /// Parse a command ID. A `<mod_id>.<name>` form is not an engine command.
    #[allow(dead_code)]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|command| command.id() == id)
    }

    pub const fn context(self) -> CommandContext {
        match self {
            Command::MoveForward
            | Command::MoveBack
            | Command::MoveLeft
            | Command::MoveRight
            | Command::MoveUp
            | Command::MoveDown
            | Command::LookX
            | Command::LookY
            | Command::Sprint
            | Command::Jump
            | Command::Dash
            | Command::Crouch
            | Command::Use
            | Command::Drop
            | Command::Shoot
            | Command::AltFire
            | Command::Reload
            | Command::SelectWieldable1
            | Command::SelectWieldable2
            | Command::SelectWieldable3
            | Command::SelectWieldable4
            | Command::SelectWieldable5
            | Command::SelectWieldable6
            | Command::SelectWieldable7
            | Command::SelectWieldable8
            | Command::SelectWieldable9
            | Command::SelectWieldable10
            | Command::CycleWieldableNext
            | Command::CycleWieldablePrevious
            | Command::ToggleLastWieldable => CommandContext::Gameplay,
            Command::NavUp
            | Command::NavDown
            | Command::NavLeft
            | Command::NavRight
            | Command::NavNext
            | Command::NavPrev
            | Command::NavTabNext
            | Command::NavTabPrev
            | Command::NavConfirm
            | Command::NavCancel
            | Command::NavMenu
            | Command::NavOptions
            | Command::TextBackspace
            | Command::TextSpace
            | Command::TextCommit => CommandContext::Ui,
        }
    }

    pub const fn kind(self) -> CommandKind {
        match self {
            Command::LookX | Command::LookY => CommandKind::Analog,
            Command::CycleWieldableNext | Command::CycleWieldablePrevious => {
                CommandKind::WheelNotch
            }
            Command::MoveForward
            | Command::MoveBack
            | Command::MoveLeft
            | Command::MoveRight
            | Command::MoveUp
            | Command::MoveDown
            | Command::Sprint
            | Command::Jump
            | Command::Dash
            | Command::Crouch
            | Command::Use
            | Command::Drop
            | Command::Shoot
            | Command::AltFire
            | Command::Reload
            | Command::SelectWieldable1
            | Command::SelectWieldable2
            | Command::SelectWieldable3
            | Command::SelectWieldable4
            | Command::SelectWieldable5
            | Command::SelectWieldable6
            | Command::SelectWieldable7
            | Command::SelectWieldable8
            | Command::SelectWieldable9
            | Command::SelectWieldable10
            | Command::ToggleLastWieldable
            | Command::NavUp
            | Command::NavDown
            | Command::NavLeft
            | Command::NavRight
            | Command::NavNext
            | Command::NavPrev
            | Command::NavTabNext
            | Command::NavTabPrev
            | Command::NavConfirm
            | Command::NavCancel
            | Command::NavMenu
            | Command::NavOptions
            | Command::TextBackspace
            | Command::TextSpace
            | Command::TextCommit => CommandKind::Digital,
        }
    }

    /// Whether a binding of this command may use `activator`. Charge requires
    /// `press` on shoot and alt-fire; sprint and crouch take `press` or `hold`;
    /// analog and wheel-notch commands have no duration to time.
    #[allow(dead_code)]
    pub const fn accepts(self, activator: ActivatorKind) -> bool {
        match (self.kind(), self) {
            (CommandKind::Analog | CommandKind::WheelNotch, _)
            | (_, Command::Shoot | Command::AltFire) => {
                matches!(activator, ActivatorKind::Press)
            }
            (_, Command::Sprint | Command::Crouch) => {
                matches!(activator, ActivatorKind::Press | ActivatorKind::Hold)
            }
            _ => true,
        }
    }

    /// The gameplay action and signed contribution this command drives. A
    /// digital movement command is one direction of its axis action; an
    /// analog command maps a physical direction (right, up) onto the engine's
    /// look convention. UI commands drive no gameplay action.
    pub const fn gameplay_target(self) -> Option<(Action, f32)> {
        let target = match self {
            Command::MoveForward => (Action::MoveForward, 1.0),
            Command::MoveBack => (Action::MoveForward, -1.0),
            Command::MoveRight => (Action::MoveRight, 1.0),
            Command::MoveLeft => (Action::MoveRight, -1.0),
            Command::MoveUp => (Action::MoveUp, 1.0),
            Command::MoveDown => (Action::MoveUp, -1.0),
            // Engine yaw turns left for positive values; look right is +.
            Command::LookX => (Action::LookYaw, -1.0),
            Command::LookY => (Action::LookPitch, 1.0),
            Command::Sprint => (Action::Sprint, 1.0),
            Command::Jump => (Action::Jump, 1.0),
            Command::Dash => (Action::Dash, 1.0),
            Command::Crouch => (Action::Crouch, 1.0),
            Command::Use => (Action::Use, 1.0),
            Command::Drop => (Action::Drop, 1.0),
            Command::Shoot => (Action::Shoot, 1.0),
            Command::AltFire => (Action::AltFire, 1.0),
            Command::Reload => (Action::Reload, 1.0),
            Command::SelectWieldable1 => (Action::SelectWieldable1, 1.0),
            Command::SelectWieldable2 => (Action::SelectWieldable2, 1.0),
            Command::SelectWieldable3 => (Action::SelectWieldable3, 1.0),
            Command::SelectWieldable4 => (Action::SelectWieldable4, 1.0),
            Command::SelectWieldable5 => (Action::SelectWieldable5, 1.0),
            Command::SelectWieldable6 => (Action::SelectWieldable6, 1.0),
            Command::SelectWieldable7 => (Action::SelectWieldable7, 1.0),
            Command::SelectWieldable8 => (Action::SelectWieldable8, 1.0),
            Command::SelectWieldable9 => (Action::SelectWieldable9, 1.0),
            Command::SelectWieldable10 => (Action::SelectWieldable10, 1.0),
            Command::CycleWieldableNext => (Action::CycleWieldableNext, 1.0),
            Command::CycleWieldablePrevious => (Action::CycleWieldablePrevious, 1.0),
            Command::ToggleLastWieldable => (Action::ToggleLastWieldable, 1.0),
            Command::NavUp
            | Command::NavDown
            | Command::NavLeft
            | Command::NavRight
            | Command::NavNext
            | Command::NavPrev
            | Command::NavTabNext
            | Command::NavTabPrev
            | Command::NavConfirm
            | Command::NavCancel
            | Command::NavMenu
            | Command::NavOptions
            | Command::TextBackspace
            | Command::TextSpace
            | Command::TextCommit => return None,
        };
        Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_ids_round_trip_and_never_contain_a_dot() {
        for command in Command::ALL {
            let id = command.id();
            assert!(!id.contains('.'), "{id} reserves the mod-defined form");
            assert_eq!(Command::from_id(id), Some(command));
        }
        assert_eq!(Command::from_id("postretro.dev.dash"), None);
    }

    #[test]
    fn command_ids_are_unique() {
        let mut ids: Vec<_> = Command::ALL.iter().map(|c| c.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Command::ALL.len());
    }

    #[test]
    fn every_gameplay_action_has_a_command() {
        // Derived from the commands, so an action no command drives fails here.
        let driven: std::collections::HashSet<Action> = Command::ALL
            .iter()
            .filter_map(|command| command.gameplay_target())
            .map(|(action, _)| action)
            .collect();
        for action in super::super::defaults::legacy_actions() {
            assert!(driven.contains(&action), "{action:?} has no command");
        }
    }

    #[test]
    fn activator_acceptance_per_command() {
        use ActivatorKind::*;
        for command in Command::ALL {
            let accepted: Vec<_> = [Press, Release, Tap, Hold]
                .into_iter()
                .filter(|kind| command.accepts(*kind))
                .collect();
            let expected: &[ActivatorKind] = match command {
                Command::Shoot | Command::AltFire => &[Press],
                Command::Sprint | Command::Crouch => &[Press, Hold],
                Command::LookX
                | Command::LookY
                | Command::CycleWieldableNext
                | Command::CycleWieldablePrevious => &[Press],
                _ => &[Press, Release, Tap, Hold],
            };
            assert_eq!(accepted, expected, "{}", command.id());
        }
    }
}
