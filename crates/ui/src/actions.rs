/// Reserved `Button.onPress` value for committing the active text-entry modal.
/// The App intercepts this before named-reaction dispatch.
pub const COMMIT_TEXT_ENTRY_ACTION: &str = "ui.commitTextEntry";

/// Reserved `Button.onPress` value for closing the active modal. The App
/// intercepts this before named-reaction dispatch.
pub const CLOSE_DIALOG_ACTION: &str = "ui.closeDialog";

/// Reserved `Button.onPress` value for requesting a clean app shutdown. The App
/// intercepts this before named-reaction dispatch.
pub const EXIT_TO_DESKTOP_ACTION: &str = "ui.exitToDesktop";

/// Reserved `Button.onPress` value for returning to the frontend menu. The App
/// intercepts this before named-reaction dispatch.
pub const QUIT_TO_MENU_ACTION: &str = "ui.quitToMenu";

/// Reserved `Button.onPress` value that opens the engine accessibility panel.
/// The App intercepts this before named-reaction dispatch.
pub const OPEN_ACCESSIBILITY_ACTION: &str = "ui.openAccessibility";

/// Reserved `Button.onPress` value that opens the engine controls panel. The
/// App intercepts this before named-reaction dispatch.
pub const OPEN_CONTROLS_ACTION: &str = "ui.openControls";

/// Prefix of the engine controls panel's own actions (`ui.controls.<op>…`).
/// Only the engine-built panel and its dialogs carry them.
pub const CONTROLS_ACTION_PREFIX: &str = "ui.controls.";

/// A parsed controls-panel action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlsAction<'a> {
    /// Open the capture prompt for one binding slot.
    Capture {
        command: &'a str,
        class: &'a str,
        slot: usize,
    },
    /// Return one command to the author's defaults.
    Reset { command: &'a str },
    /// Return every command to the author's defaults.
    ResetAll,
    /// Apply the pending conflicting binding, taking the input from the others.
    Replace,
    /// Drop the pending conflicting binding.
    Keep,
}

/// Parse a `ui.controls.*` action; `None` for anything else.
pub fn parse_controls_action(on_press: &str) -> Option<ControlsAction<'_>> {
    let rest = on_press.strip_prefix(CONTROLS_ACTION_PREFIX)?;
    let mut parts = rest.split('.');
    let action = match (parts.next()?, parts.next(), parts.next(), parts.next()) {
        ("capture", Some(command), Some(class), Some(slot)) => ControlsAction::Capture {
            command,
            class,
            slot: slot.parse().ok()?,
        },
        ("reset", Some(command), None, None) => ControlsAction::Reset { command },
        ("resetAll", None, None, None) => ControlsAction::ResetAll,
        ("replace", None, None, None) => ControlsAction::Replace,
        ("keep", None, None, None) => ControlsAction::Keep,
        _ => return None,
    };
    parts.next().is_none().then_some(action)
}

/// Prefix of the reserved accessibility field-action family,
/// `ui.accessibility.<op>.<field>`: `op` is `cycle`, `increase`, or `decrease`;
/// `field` is the `accessibility.*` slot's camelCase suffix. The App intercepts
/// a string under this prefix only when it parses as exactly two non-empty
/// segments (`<op>.<field>`) and owns the field vocabulary for those; any other
/// string under the prefix falls through to named-reaction dispatch as an
/// ordinary reaction name.
pub const ACCESSIBILITY_FIELD_ACTION_PREFIX: &str = "ui.accessibility.";

/// A parsed accessibility field action: the raw `op` and `field` segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessibilityFieldAction<'a> {
    pub op: &'a str,
    pub field: &'a str,
}

/// Split `ui.accessibility.<op>.<field>`. `None` for any other `onPress`,
/// including a family name missing either segment.
pub fn parse_accessibility_field_action(on_press: &str) -> Option<AccessibilityFieldAction<'_>> {
    let rest = on_press.strip_prefix(ACCESSIBILITY_FIELD_ACTION_PREFIX)?;
    let (op, field) = rest.split_once('.')?;
    (!op.is_empty() && !field.is_empty() && !field.contains('.'))
        .then_some(AccessibilityFieldAction { op, field })
}

/// Whether `tree` offers accessibility: a button whose `onPress` opens the
/// engine panel or fires an accessibility field action.
pub fn tree_offers_accessibility(tree: &crate::descriptor::AnchoredTree) -> bool {
    use crate::descriptor::Widget;
    fn visit(widget: &Widget) -> bool {
        match widget {
            Widget::Button(button) => {
                button.on_press == OPEN_ACCESSIBILITY_ACTION
                    || parse_accessibility_field_action(&button.on_press).is_some()
            }
            Widget::VStack(container) | Widget::HStack(container) => {
                container.children.iter().any(visit)
            }
            Widget::Grid(grid) => grid.children.iter().any(visit),
            _ => false,
        }
    }
    visit(&tree.root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_mode_actions_accept_only_closed_operations() {
        for action in [
            DisplayModeAction::Next,
            DisplayModeAction::Previous,
            DisplayModeAction::Apply,
            DisplayModeAction::Keep,
            DisplayModeAction::Revert,
        ] {
            assert_eq!(
                parse_display_mode_action(&format!("ui.displayMode.{}", action.op())),
                Some(action)
            );
        }
        for action in [
            "ui.displayMode",
            "ui.displayMode.",
            "ui.displayMode.toggle",
            "ui.displayMode.next.extra",
            "frontend.displayMode.next",
        ] {
            assert_eq!(parse_display_mode_action(action), None);
        }
    }

    #[test]
    fn accessibility_field_actions_parse_op_and_field() {
        assert_eq!(
            parse_accessibility_field_action("ui.accessibility.cycle.reduceMotion"),
            Some(AccessibilityFieldAction {
                op: "cycle",
                field: "reduceMotion"
            })
        );
        for other in [
            OPEN_ACCESSIBILITY_ACTION,
            "ui.accessibility.cycle",
            "ui.accessibility..reduceMotion",
            "ui.accessibility.cycle.a.b",
            "frontend.options.invertY.on",
        ] {
            assert_eq!(parse_accessibility_field_action(other), None, "{other}");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayModeAction {
    Next,
    Previous,
    Apply,
    Keep,
    Revert,
}

impl DisplayModeAction {
    pub fn op(self) -> &'static str {
        match self {
            Self::Next => "next",
            Self::Previous => "previous",
            Self::Apply => "apply",
            Self::Keep => "keep",
            Self::Revert => "revert",
        }
    }
}

pub fn parse_display_mode_action(action: &str) -> Option<DisplayModeAction> {
    match action {
        "ui.displayMode.next" => Some(DisplayModeAction::Next),
        "ui.displayMode.previous" => Some(DisplayModeAction::Previous),
        "ui.displayMode.apply" => Some(DisplayModeAction::Apply),
        "ui.displayMode.keep" => Some(DisplayModeAction::Keep),
        "ui.displayMode.revert" => Some(DisplayModeAction::Revert),
        _ => None,
    }
}
