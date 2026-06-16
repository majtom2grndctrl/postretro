/// Reserved `Button.onPress` value for committing the active text-entry modal.
/// The App intercepts this before named-reaction dispatch.
pub(crate) const COMMIT_TEXT_ENTRY_ACTION: &str = "ui.commitTextEntry";

/// Reserved `Button.onPress` value for closing the active modal. The App
/// intercepts this before named-reaction dispatch.
pub(crate) const CLOSE_DIALOG_ACTION: &str = "ui.closeDialog";
