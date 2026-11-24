//! Rename gate: the per-character mount table is `mesh.mounts` with entries
//! `{ item, attachments }`, and the retired names appear nowhere a modder or
//! an engine author reads. The old key rejects as an unknown key (proved in
//! `postretro-scripting-core`'s mount descriptor tests), so only that proof and
//! this file may spell the retired names.
//! See: context/lib/entity_model.md §2 (Weapon mounts)

use std::path::{Path, PathBuf};

/// The retired table key, built from parts so this file does not name it.
fn retired_key() -> String {
    ["weapon", "Mounts"].concat()
}

/// The retired descriptor type name, built from parts likewise.
fn retired_type() -> String {
    ["Weapon", "Mount", "Descriptor"].concat()
}

/// Trees the gate walks, relative to the workspace root.
const ROOTS: &[&str] = &["content", "sdk", "docs", "context/lib", "crates"];

/// Files that may name the retired key: the test that proves it rejects, and
/// this gate. Relative paths, components joined by `/`.
const ALLOWED: &[&str] = &[
    "crates/scripting-core/src/data_descriptors/tests/mounts.rs",
    "crates/postretro/src/level_load/mount_rename_gate_tests.rs",
];

/// Extensions of the text files that carry names: source, typedefs, docs.
const TEXT_EXTENSIONS: &[&str] = &["rs", "ts", "luau", "lua", "js", "md", "toml", "json"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `path` relative to `root`, its components joined by `/`.
fn relative_slashed(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap()
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether `path` is a `.js` build output: a script with a `.ts` source beside
/// it. Those are gitignored and stale until the next build.
fn is_build_output(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "js") && path.with_extension("ts").is_file()
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name == "target" || name == "node_modules" || name == ".git" {
                continue;
            }
            collect(root, &path, out);
        } else if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| TEXT_EXTENSIONS.contains(&ext))
            && !is_build_output(&path)
        {
            out.push(path);
        }
    }
}

#[test]
fn retired_mount_names_appear_only_in_the_rejection_proof() {
    let root = workspace_root();
    let (key, type_name) = (retired_key(), retired_type());
    let mut files = Vec::new();
    for tree in ROOTS {
        collect(&root, &root.join(tree), &mut files);
    }
    assert!(
        files.len() > 100,
        "the gate walked only {} files; the trees moved or the walk broke",
        files.len()
    );

    let mut offenders = Vec::new();
    for path in &files {
        let relative = relative_slashed(&root, path);
        if ALLOWED.contains(&relative.as_str()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            if line.contains(&key) || line.contains(&type_name) {
                offenders.push(format!("{relative}:{}", index + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "the mount table is `mesh.mounts` with `{{ item, attachments }}` entries; \
         retired names remain at:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_rejection_proof_still_names_the_retired_key() {
    let root = workspace_root();
    let key = retired_key();
    for relative in &ALLOWED[..1] {
        let text = std::fs::read_to_string(root.join(relative)).expect("allowlisted file exists");
        assert!(
            text.contains(&key),
            "{relative} no longer names the retired key; drop it from the allowlist"
        );
    }
}
