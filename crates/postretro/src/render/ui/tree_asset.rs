// Generic load-and-register path for engine-shipped UI descriptor trees: reads a
// named `AnchoredTree` from `content/base/ui/<file>.json` on disk (NOT embedded,
// so a mod author can edit the layout JSON and reload to change a built-in screen
// with no Rust change) and registers it under a name in the modal-stack registry.
// A missing/malformed file warns ONCE and skips the registration — that screen is
// unavailable, the engine still boots. The on-screen keyboard's loader builds on
// this same helper.
// See: context/lib/ui.md §1

use std::path::{Path, PathBuf};

use super::descriptor::AnchoredTree;
use super::modal_stack::UiTreeRegistry;

/// Registry name the gameplay HUD registers + resolves under. The per-frame
/// snapshot resolves this name through the registry to compose the always-on
/// bottom passthrough layer; the boot path registers `content/base/ui/hud.json`
/// against it.
pub(crate) const HUD_NAME: &str = "hud";

/// Resolve an engine-shipped UI asset's path, relative to the working directory —
/// the same `content/base/...` convention the splash PNG and keyboard JSON use.
/// Independent of the mod content root (derived from the map path): these screens
/// ship with the engine, so they load from `base` regardless of the active mod.
pub(crate) fn ui_asset_path(file_name: &str) -> PathBuf {
    PathBuf::from("content/base/ui").join(file_name)
}

/// Load and deserialize a UI descriptor tree from `path`. Returns the parsed
/// `AnchoredTree` on success; on a missing or malformed file logs a `warn!` once
/// and returns `None` so the boot path degrades gracefully (that screen is simply
/// unavailable — gameplay still boots). The descriptor flows through the standard
/// serde wire path, so a layout edit is picked up purely from the JSON.
pub(crate) fn load_named_tree(path: &Path) -> Option<AnchoredTree> {
    let bytes = match std::fs::read_to_string(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            log::warn!(
                "[UI] tree asset '{}' could not be read ({err}); that screen is unavailable",
                path.display()
            );
            return None;
        }
    };
    match serde_json::from_str::<AnchoredTree>(&bytes) {
        Ok(tree) => Some(tree),
        Err(err) => {
            log::warn!(
                "[UI] tree asset '{}' failed to deserialize ({err}); that screen is unavailable",
                path.display()
            );
            None
        }
    }
}

/// Load `content/base/ui/<file_name>` and, on success, register it under `name`
/// in `registry`. A missing/malformed asset warns once (via `load_named_tree`)
/// and skips the registration — the one shared boot wiring for engine built-in
/// screens (HUD, pause menu, keyboard).
pub(crate) fn register_tree_from_disk(
    registry: &mut UiTreeRegistry,
    name: &'static str,
    file_name: &str,
) {
    if let Some(tree) = load_named_tree(&ui_asset_path(file_name)) {
        registry.register(name, tree);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A missing file path degrades to `None` (warn-once, no panic) — the graceful
    /// boot path: an absent screen leaves the engine running.
    #[test]
    fn load_named_tree_missing_file_degrades_to_none() {
        let missing = ui_asset_path("does-not-exist.json");
        assert!(
            load_named_tree(&missing).is_none(),
            "a missing UI asset resolves to None, not a panic",
        );
    }

    /// The shipped HUD asset deserializes through the standard wire path to a
    /// well-formed, non-empty tree. JSON is the source of truth for the HUD now (no
    /// hand-assembled builder remains), so this is a structural load check, not a
    /// builder-equality oracle: it proves the asset reaches the registry as a usable
    /// tree. The exact load-bearing HUD values (slot names, tween durations,
    /// styleRange thresholds and band tokens) are pinned by `demo`'s tests. Anchored
    /// off `CARGO_MANIFEST_DIR` (the boot loader uses a cwd-relative path; `cargo
    /// test` runs from the crate dir) — the same precedent the keyboard asset test
    /// uses.
    #[test]
    fn hud_asset_loads_to_a_nonempty_tree() {
        use crate::render::ui::descriptor::Widget;

        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(ui_asset_path("hud.json"));
        let tree = load_named_tree(&path).expect("hud.json loads through the wire path");
        let Widget::VStack(col) = &tree.root else {
            panic!("the HUD root is a vstack column");
        };
        assert!(!col.children.is_empty(), "the HUD has at least one row");
    }

    /// The shipped pause-menu asset deserializes to a well-formed capturing modal.
    /// Like the HUD test, this is a structural load check now that JSON is the
    /// source of truth — not a builder-equality oracle. The pause menu's load-
    /// bearing wiring (captureMode, initialFocus, focus chain, slot binds) is pinned
    /// by `demo`'s tests.
    #[test]
    fn pause_menu_asset_loads_to_a_capturing_modal() {
        use crate::render::ui::descriptor::CaptureMode;

        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(ui_asset_path("pauseMenu.json"));
        let tree = load_named_tree(&path).expect("pauseMenu.json loads through the wire path");
        assert_eq!(
            tree.capture_mode,
            CaptureMode::Capture,
            "the pause menu captures input (freezes gameplay, releases cursor)",
        );
        assert!(
            tree.initial_focus.is_some(),
            "the pause menu declares an initial focus target",
        );
    }
}
