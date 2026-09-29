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
