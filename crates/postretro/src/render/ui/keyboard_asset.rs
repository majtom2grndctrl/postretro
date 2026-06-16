// Engine-shipped on-screen keyboard descriptor: loads the keyboard `AnchoredTree`
// from `content/base/ui/keyboard.json` at boot and registers it under the
// `keyboard` name in the modal-stack registry. Read from disk (NOT embedded) so a
// mod author can edit the layout JSON and reload to change the keyboard with no
// Rust change — the gamepad accessibility accommodation, built entirely from F's
// grid/spatial-focus/button primitives plus Task 1's text-edit reactions.
// See: context/lib/ui.md

#[cfg(test)]
use std::path::PathBuf;

#[cfg(test)]
use super::descriptor::AnchoredTree;
#[cfg(test)]
use super::tree_asset::ui_asset_path;

/// Registry name the on-screen keyboard registers under. A `showDialog { tree:
/// "keyboard", onCommit }` resolves this name through the modal stack.
pub(crate) const KEYBOARD_TREE_NAME: &str = "keyboard";

/// Reserved sentinel `onPress` name the keyboard's `done` key carries. The
/// canonical value lives in `actions`; this alias keeps the keyboard asset tests
/// and comments close to the JSON that references it.
#[cfg(test)]
pub(crate) const COMMIT_TEXT_ENTRY_SENTINEL: &str = super::actions::COMMIT_TEXT_ENTRY_ACTION;

/// Engine-shipped keyboard descriptor path, relative to the working directory —
/// the same `content/base/...` convention the splash PNG uses. The boot path
/// registers the keyboard through `tree_asset::register_tree_from_disk`; this
/// anchors the same asset for the keyboard's own deserialization tests.
#[cfg(test)]
pub(crate) fn keyboard_asset_path() -> PathBuf {
    ui_asset_path("keyboard.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::ui::descriptor::{CaptureMode, FocusKind, Widget};

    /// Deserialize the on-disk keyboard JSON through the standard wire path and
    /// load its bytes the same way the boot path does, so the test exercises the
    /// real asset, not a fixture. The boot loader uses a working-directory-relative
    /// path (the engine runs from the workspace root); `cargo test` runs from the
    /// crate dir, so the test anchors the same asset off `CARGO_MANIFEST_DIR`.
    fn load_from_disk() -> AnchoredTree {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(keyboard_asset_path());
        let bytes =
            std::fs::read_to_string(&path).expect("keyboard asset exists under content/base/ui");
        serde_json::from_str(&bytes).expect("keyboard asset deserializes through the wire path")
    }

    #[test]
    fn keyboard_asset_deserializes_to_capturing_text_entry_grid() {
        // The shipped keyboard is a capturing modal whose envelope declares the
        // `ui.textEntry` target, and whose root is a spatial-focus grid — the four
        // properties Task 3's routing + F's spatial nav depend on.
        let tree = load_from_disk();
        assert_eq!(
            tree.capture_mode,
            CaptureMode::Capture,
            "the keyboard captures input (gameplay freezes while open)",
        );
        assert_eq!(
            tree.text_entry_target.as_deref(),
            Some("ui.textEntry"),
            "the envelope declares ui.textEntry as its text-entry target",
        );
        let Widget::Grid(grid) = &tree.root else {
            panic!("keyboard root is a grid");
        };
        assert_eq!(
            grid.focus.as_ref().map(|f| f.kind()),
            Some(FocusKind::Spatial),
            "the grid uses spatial focus for nearest-neighbor nav",
        );
    }

    #[test]
    fn keyboard_done_key_carries_the_commit_sentinel() {
        // The `done` key's onPress is the reserved commit sentinel — the data hook
        // the activation path intercepts to reach `commit_text_entry`.
        let tree = load_from_disk();
        let Widget::Grid(grid) = &tree.root else {
            panic!("keyboard root is a grid");
        };
        let done = grid
            .children
            .iter()
            .find_map(|w| match w {
                Widget::Button(b) if b.id == "key_done" => Some(b),
                _ => None,
            })
            .expect("keyboard has a done key");
        assert_eq!(done.on_press, COMMIT_TEXT_ENTRY_SENTINEL);
    }

    #[test]
    fn keyboard_backspace_key_repeats_on_hold_letters_do_not() {
        // The backspace key opts into activation-repeat (`repeatOnHold`); letter
        // keys do not (one append per press), per the AC.
        let tree = load_from_disk();
        let Widget::Grid(grid) = &tree.root else {
            panic!("keyboard root is a grid");
        };
        let buttons: Vec<_> = grid
            .children
            .iter()
            .filter_map(|w| match w {
                Widget::Button(b) => Some(b),
                _ => None,
            })
            .collect();

        let backspace = buttons
            .iter()
            .find(|b| b.id == "key_backspace")
            .expect("keyboard has a backspace key");
        assert!(
            backspace.repeat_on_hold.is_some(),
            "backspace repeats on hold",
        );

        let letter = buttons
            .iter()
            .find(|b| b.id == "key_a")
            .expect("keyboard has a letter key");
        assert!(
            letter.repeat_on_hold.is_none(),
            "a letter key fires once per press (no repeatOnHold)",
        );
    }

    #[test]
    fn deleting_a_key_still_deserializes_with_no_rust_change() {
        // AC 6: editing the layout JSON (here: dropping one key) still parses
        // through the same wire path and yields a valid keyboard with one fewer
        // key — the keys are data, not Rust. Simulated by parsing the real asset,
        // removing a key, re-serializing, and re-parsing.
        let mut tree = load_from_disk();
        let Widget::Grid(grid) = &mut tree.root else {
            panic!("keyboard root is a grid");
        };
        let before = grid.children.len();
        grid.children
            .retain(|w| !matches!(w, Widget::Button(b) if b.id == "key_z"));
        assert_eq!(grid.children.len(), before - 1, "one key removed");

        let edited = serde_json::to_string(&tree).expect("edited layout re-serializes");
        let reparsed: AnchoredTree =
            serde_json::from_str(&edited).expect("edited layout deserializes — keys are data");
        let Widget::Grid(grid) = &reparsed.root else {
            panic!("keyboard root is still a grid");
        };
        assert!(
            !grid
                .children
                .iter()
                .any(|w| matches!(w, Widget::Button(b) if b.id == "key_z")),
            "the removed key stays gone after the round-trip",
        );
    }
}
