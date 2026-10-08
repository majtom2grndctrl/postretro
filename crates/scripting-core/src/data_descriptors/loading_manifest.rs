// VM-agnostic resolution of the manifest's `uiImages` map, its `loading`
// block, and a map catalog entry's `loadingTree` pool, shared by the QuickJS
// and Luau drains so both runtimes degrade and warn identically. The drains
// only classify raw VM values into `AuthoredValue`; every shape rule and
// warning lives here. Nothing here rejects the manifest.
// See: context/lib/ui.md · context/lib/scripting.md §1

use std::collections::BTreeMap;
use std::path::{Component, Path};

use super::ModLoading;
use super::input_block::AuthoredValue;

/// Image registry keys under this prefix belong to the engine (its splash logo
/// and other built-in art); a mod may not declare one.
pub(crate) const ENGINE_IMAGE_PREFIX: &str = "engine/";

/// Resolve the authored `uiImages` value into image key → mod-relative PNG
/// path. Each bad entry warns naming it and is skipped; the rest still load.
/// File existence and PNG decoding are the app's checks at load time.
pub(crate) fn resolve_ui_images(scope: &str, raw: AuthoredValue) -> BTreeMap<String, String> {
    if matches!(raw, AuthoredValue::Absent) {
        return BTreeMap::new();
    }
    let Some(entries) = raw.into_entries() else {
        log::warn!(
            "[Scripting] {scope}: `uiImages` must be an object of image name → PNG path; no mod images load"
        );
        return BTreeMap::new();
    };
    let mut images = BTreeMap::new();
    for (name, value) in entries {
        if name.is_empty() {
            log::warn!("[Scripting] {scope}: `uiImages` has an empty image name; skipping it");
            continue;
        }
        if name.starts_with(ENGINE_IMAGE_PREFIX) {
            log::warn!(
                "[Scripting] {scope}: `uiImages.{name}` uses the reserved `{ENGINE_IMAGE_PREFIX}` prefix; skipping it"
            );
            continue;
        }
        let AuthoredValue::String(path) = value else {
            log::warn!(
                "[Scripting] {scope}: `uiImages.{name}` must be a PNG path string; skipping it"
            );
            continue;
        };
        if !is_mod_relative_path(&path) {
            log::warn!(
                "[Scripting] {scope}: `uiImages.{name}` path `{path}` must be relative and stay inside the mod; skipping it"
            );
            continue;
        }
        images.insert(name, path);
    }
    images
}

/// Resolve the authored `loading` block. Absent yields the default (no mod
/// pool). A malformed block or `tree` warns and is treated as absent.
pub(crate) fn resolve_loading(scope: &str, raw: AuthoredValue) -> ModLoading {
    if matches!(raw, AuthoredValue::Absent) {
        return ModLoading::default();
    }
    let Some(entries) = raw.into_entries() else {
        log::warn!("[Scripting] {scope}: `loading` must be an object; ignoring it");
        return ModLoading::default();
    };
    let tree = entries
        .into_iter()
        .find_map(|(key, value)| (key == "tree").then_some(value))
        .unwrap_or(AuthoredValue::Absent);
    if matches!(tree, AuthoredValue::Absent) {
        log::warn!("[Scripting] {scope}: `loading` has no `tree`; no mod loading pool applies");
        return ModLoading::default();
    }
    ModLoading {
        tree: resolve_tree_pool(scope, "loading.tree", tree),
    }
}

/// Resolve a loading-tree pool (`loading.tree` or a catalog entry's
/// `loadingTree`): one registry name or an array of them. Absent and an empty
/// array both yield an empty pool. Any other shape, including an array with a
/// non-string element, warns naming `path` and yields an empty pool. Names
/// are deduplicated in authored order so a uniform pick stays uniform.
pub(crate) fn resolve_tree_pool(scope: &str, path: &str, raw: AuthoredValue) -> Vec<String> {
    let items = match raw {
        AuthoredValue::Absent => return Vec::new(),
        AuthoredValue::String(name) => vec![AuthoredValue::String(name)],
        other => match other.into_items() {
            Some(items) => items,
            None => {
                log::warn!(
                    "[Scripting] {scope}: `{path}` must be a tree name or an array of tree names; ignoring it"
                );
                return Vec::new();
            }
        },
    };
    let mut pool = Vec::with_capacity(items.len());
    for item in items {
        let AuthoredValue::String(name) = item else {
            log::warn!(
                "[Scripting] {scope}: `{path}` must hold only tree-name strings; ignoring the whole pool"
            );
            return Vec::new();
        };
        if !pool.contains(&name) {
            pool.push(name);
        }
    }
    pool
}

/// A path inside the mod root: non-empty, relative, with no `..`. The same
/// rule the app applies to `input.glyphs` directories.
pub(crate) fn is_mod_relative_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}
