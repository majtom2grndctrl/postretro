// Repository grep gate for the level-seam rename: the SH streaming split
// volume's retired classname appears in no file outside `context/plans/`, so
// "seam" names only the map transition (`level_seam`). Run by preflight's
// `cargo test`, beside the other surface gates.
// See: context/lib/build_pipeline.md §Custom FGD · context/lib/entity_model.md §7e

use std::path::{Path, PathBuf};

/// Built from parts so this gate never holds the name it forbids.
fn retired_split_classname() -> String {
    ["streaming", "seam", "volume"].join("_")
}

/// Trees never scanned: build output, VCS state, and plans, which keep the
/// rename's history.
const SKIPPED_DIRS: &[&str] = &["target", ".git", "node_modules"];
const PLANS_DIR: &str = "context/plans";

fn workspace_root() -> PathBuf {
    crate::workspace_root().expect("workspace root")
}

fn scan(dir: &Path, root: &Path, needle: &str, hits: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if kind.is_dir() {
            let name = entry.file_name();
            if SKIPPED_DIRS.iter().any(|skip| name == *skip)
                || relative.starts_with(PLANS_DIR)
                || name.to_string_lossy().starts_with("target")
            {
                continue;
            }
            scan(&path, root, needle, hits);
        } else if kind.is_file() {
            // Binary files (baked PRLs, images) read as non-UTF-8 and are skipped.
            if let Ok(text) = std::fs::read_to_string(&path)
                && text.contains(needle)
            {
                hits.push(relative.display().to_string());
            }
        }
    }
}

#[test]
fn retired_streaming_split_classname_appears_only_in_plans() {
    let root = workspace_root();
    let mut hits = Vec::new();
    scan(&root, &root, &retired_split_classname(), &mut hits);
    hits.sort();
    assert!(
        hits.is_empty(),
        "the retired streaming split classname must not appear outside context/plans/ \
         (renamed `stream_split_volume`, no alias): {hits:?}"
    );
}
