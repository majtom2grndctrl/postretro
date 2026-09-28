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

/// Prefix of the reserved accessibility field-action family,
/// `ui.accessibility.<op>.<field>`: `op` is `cycle`, `increase`, or `decrease`;
/// `field` is the `accessibility.*` slot's camelCase suffix. The App intercepts
/// the family before named-reaction dispatch and owns the field vocabulary.
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

/// Whether any button in `tree` fires `on_press`.
pub fn tree_has_button_action(tree: &crate::descriptor::AnchoredTree, on_press: &str) -> bool {
    use crate::descriptor::Widget;
    fn visit(widget: &Widget, on_press: &str) -> bool {
        match widget {
            Widget::Button(button) => button.on_press == on_press,
            Widget::VStack(container) | Widget::HStack(container) => container
                .children
                .iter()
                .any(|child| visit(child, on_press)),
            Widget::Grid(grid) => grid.children.iter().any(|child| visit(child, on_press)),
            _ => false,
        }
    }
    visit(&tree.root, on_press)
}

#[cfg(test)]
mod tests {
    use super::*;

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
