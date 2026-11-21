// Repository grep gate for the level-seam rename: the SH streaming split
// volume's retired classname appears in no file outside `context/plans/`, so
// "seam" names only the map transition (`level_seam`). Run by preflight's
// `cargo test`, beside the other surface gates.
// See: context/lib/build_pipeline.md §Custom FGD · context/lib/entity_model.md §7e

use std::path::Path;
use std::process::Command;

/// Built from parts so this gate never holds the name it forbids.
fn retired_split_classname() -> String {
    ["streaming", "seam", "volume"].join("_")
}

/// Plans keep the rename's history.
const PLANS_DIR: &str = "context/plans/";

fn workspace_root() -> std::path::PathBuf {
    crate::workspace_root().expect("workspace root")
}

/// The repository's files as git lists them: tracked files plus untracked
/// files that are not ignored, as `/`-separated paths. Ignored trees (build
/// output, local worktrees, assembled SDK bundles) hold stale copies of the
/// repository, so walking the disk would flag them and read every copy.
fn repository_files(root: &Path) -> Vec<String> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git lists utf-8 paths")
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect()
}

#[test]
fn retired_streaming_split_classname_appears_only_in_plans() {
    let root = workspace_root();
    let needle = retired_split_classname();
    let mut hits: Vec<String> = repository_files(&root)
        .into_iter()
        .filter(|path| !path.starts_with(PLANS_DIR))
        // Binary files (baked PRLs, images) read as non-UTF-8 and are skipped,
        // as is a tracked file deleted from the working tree.
        .filter(|path| {
            std::fs::read_to_string(root.join(path)).is_ok_and(|text| text.contains(&needle))
        })
        .collect();
    hits.sort();
    // A conflicted file is listed once per merge stage.
    hits.dedup();
    assert!(
        hits.is_empty(),
        "the retired streaming split classname must not appear outside context/plans/ \
         (renamed `stream_split_volume`, no alias): {hits:?}"
    );
}
