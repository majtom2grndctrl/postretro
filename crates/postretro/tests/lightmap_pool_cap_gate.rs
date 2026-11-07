// Grep gate: the lightmap pool is the cap. No non-test source sizes a pool from
// the old default, carries a cap on a drain, or offers a pool cap lever, and no
// context doc describes one.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::fs;
use std::path::{Path, PathBuf};

/// The retired cap, its lever and what they fed.
const RETIRED: &[&str] = &[
    "DEFAULT_LIGHTMAP_POOL_CAP_LAYERS",
    "MAX_LIGHTMAP_POOL_CAP_LAYERS",
    "pool_cap_layers",
    "lightmap_pool_cap",
    "cap_layers",
    "cap_eff",
    "evict_band_over_cap",
    "EvictionReason::OverCap",
    "GROWTH_HEADROOM",
    "LightmapLevers",
    "LightmapStreamingLevers",
    "Pool cap (layers)",
    "peak / cap",
    "pool cap ",
    "pool-cap",
];

/// How the docs described the retired cap, lowercase. These docs hold other,
/// legitimate caps (shadow pools, SH), so each phrase names the lightmap's, and
/// matches whole words: "pool capacity" is not "pool cap".
const RETIRED_DOC_WORDING: &[&str] = &[
    "lightmap cap",
    "lightmap pool cap",
    "pool cap",
    "pool-cap",
    "cap slider",
    "lightmap slider",
    "default cap",
    "fixed cap",
    "grows past its cap",
    "pairs past the cap",
    "the cap bounds",
    "holds the cap",
];

/// The docs that described the cap, from the workspace root.
const CAP_DOCS: &[&str] = &[
    "context/lib/rendering_pipeline.md",
    "context/plans/large-map-spatial-residency.md",
];

/// Modules compiled only under `cfg(test)` that the file-name rule cannot see.
const TEST_ONLY_DIRS: &[&str] = &["walk_measurement", "lightmap_residency_dry_run"];

fn is_test_source(path: &Path) -> bool {
    path.components().any(|part| {
        let part = part.as_os_str().to_string_lossy();
        part.contains("test") || TEST_ONLY_DIRS.contains(&part.as_ref())
    })
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read a source directory") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// `source` up to its trailing `#[cfg(test)]` test module.
fn without_test_module(source: &str) -> &str {
    let module = ["#[cfg(test)]\nmod tests {", "#[cfg(test)]\nmod test {"]
        .iter()
        .find_map(|start| source.find(start));
    &source[..module.unwrap_or(source.len())]
}

#[test]
fn no_lightmap_pool_cap_survives() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate sits in the workspace's crates directory");
    let mut sources = Vec::new();
    for entry in fs::read_dir(crates).expect("read the crates directory") {
        let src = entry.expect("a crate directory").path().join("src");
        if src.is_dir() {
            rust_sources(&src, &mut sources);
        }
    }
    sources.retain(|path| !is_test_source(path.strip_prefix(crates).expect("under crates")));
    assert!(
        sources
            .iter()
            .any(|path| path.ends_with("policy/placement.rs"))
            && sources
                .iter()
                .any(|path| path.ends_with("capture/scene.rs"))
            && sources
                .iter()
                .any(|path| path.ends_with("debug_ui/streaming_tab.rs")),
        "the gate must read the pool, capture and lever sources"
    );

    let mut survivors = Vec::new();
    for path in &sources {
        let source = fs::read_to_string(path).expect("read a source file");
        for (number, line) in without_test_module(&source).lines().enumerate() {
            if let Some(token) = RETIRED.iter().find(|token| line.contains(**token)) {
                survivors.push(format!("{}:{}: {token}", path.display(), number + 1));
            }
        }
    }
    assert!(
        survivors.is_empty(),
        "the retired lightmap pool cap survives:\n{}",
        survivors.join("\n")
    );
}

/// `doc` as one lowercase line, so a phrase matches across a hard wrap, in a
/// blockquote too.
fn doc_words(doc: &str) -> String {
    let words: Vec<&str> = doc.split_whitespace().filter(|&word| word != ">").collect();
    words.join(" ").to_lowercase()
}

fn has_whole_phrase(text: &str, phrase: &str) -> bool {
    let boundary = |c: Option<char>| !c.is_some_and(char::is_alphanumeric);
    text.match_indices(phrase).any(|(at, _)| {
        boundary(text[..at].chars().next_back())
            && boundary(text[at + phrase.len()..].chars().next())
    })
}

#[test]
fn docs_describe_no_lightmap_pool_cap() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two levels under the workspace root");
    let mut survivors = Vec::new();
    for doc in CAP_DOCS {
        let words = doc_words(&fs::read_to_string(root.join(doc)).expect("read a context doc"));
        for phrase in RETIRED_DOC_WORDING {
            if has_whole_phrase(&words, phrase) {
                survivors.push(format!("{doc}: {phrase}"));
            }
        }
    }
    assert!(
        survivors.is_empty(),
        "a doc still describes the retired lightmap pool cap:\n{}",
        survivors.join("\n")
    );
}
