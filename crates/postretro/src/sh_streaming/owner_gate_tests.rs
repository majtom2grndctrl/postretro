// Grep gate: SH cluster residency has no owners. Clusters are independent, and
// shared storage lives by reference count, so no owner field, sentinel, role or
// closure may return to the format, compiler or runtime.
// See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use std::fs;
use std::path::{Path, PathBuf};

/// Every crate SH residency data crosses, relative to `crates/`.
const SCANNED_CRATES: &[&str] = &[
    "level-format",
    "level-compiler",
    "level-loader",
    "render-cpu",
    "renderer",
    "postretro",
];

/// The retired vocabulary, split so this file does not match itself. Names are
/// specific to SH: a bare `owner` is ordinary English here, and shadowmask
/// `FaceOwners` is an unrelated, live concept.
const RETIRED_TERMS: &[[&str; 2]] = &[
    ["owner_", "cluster_id"],
    ["DENSE_OWNER", "_SENTINEL"],
    ["owner_", "closure"],
    ["owner_", "dependencies"],
    ["derive_node", "_ownership"],
    ["nodes_by", "_owner"],
    ["sparse_row", "_owner"],
    ["sparse_row", "_role"],
    ["missing", "_owner"],
    ["Owner", "Cycle"],
    ["validate_owner", "_graph"],
    ["validate_affinity", "_ownership"],
];

fn rust_sources(dir: &Path, sources: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("source directory is readable") {
        let path = entry.expect("directory entry is readable").path();
        if path.is_dir() {
            rust_sources(&path, sources);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            sources.push(path);
        }
    }
}

#[test]
fn no_sh_owner_vocabulary_remains_in_the_format_compiler_or_runtime() {
    let crates_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate sits under crates/");
    let mut sources = Vec::new();
    for crate_name in SCANNED_CRATES {
        rust_sources(&crates_root.join(crate_name).join("src"), &mut sources);
    }
    assert!(
        sources.len() > 100,
        "the gate must scan the real tree, found {} files",
        sources.len()
    );

    let terms: Vec<String> = RETIRED_TERMS.iter().map(|term| term.concat()).collect();
    let mut hits = Vec::new();
    for path in &sources {
        let text = fs::read_to_string(path).expect("source file is readable");
        for term in &terms {
            if text.contains(term.as_str()) {
                hits.push(format!("{}: {term}", path.display()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "retired SH owner terms remain:\n{}",
        hits.join("\n")
    );
}
