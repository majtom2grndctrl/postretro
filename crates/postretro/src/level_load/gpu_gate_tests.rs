//! The Gate row: every app-crate test that needs a renderer builds it through
//! one helper, which honors `POSTRETRO_REQUIRE_GPU` as the renderer crate's
//! GPU harness does, so a run with the variable set and no adapter fails
//! instead of skipping. A grep gate proves no test builds a renderer or a GPU
//! device outside that helper; a unit test pins the helper's decision.
//! See: context/plans/done/level-load-pipeline/index.md (Acceptance: Gate)

use std::path::{Path, PathBuf};

use super::test_support::{gpu_required_by, renderer_or_skip};

/// The one app-crate file that may build a renderer: the helper.
const HELPER: &str = "level_load/test_support.rs";

/// Production constructions the gate admits, each pinned to its file and
/// count: the windowed boot's renderer and frame capture's offscreen one.
/// Neither runs under a test.
const PRODUCTION: &[(&str, &str, usize)] = &[
    ("main.rs", "Renderer::new", 1),
    ("capture/prepared.rs", "new_offscreen", 1),
];

/// What builds a renderer or a GPU device: the renderer's constructors (by
/// path, called or passed), and wgpu's instance, adapter and device requests
/// (the app crate has no wgpu dependency, so any of these would also reach
/// one through a re-export). A needle matches only where no identifier
/// character follows it.
const CONSTRUCTIONS: &[&str] = &[
    "Renderer::new",
    "new_offscreen",
    "new_offscreen_holding_pipeline_build",
    "new_offscreen_on",
    "Instance::new",
    "request_adapter",
    "request_device",
    "try_init_gpu",
    "gpu_or_skip",
];

/// `source` with comments removed and string, raw-string and char literals
/// emptied, so only code is scanned.
fn code_only(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let len = chars.len();
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < len {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < len && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && next == Some('*') {
            let mut depth = 0usize;
            while i < len {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            out.push(' ');
            continue;
        }
        let after_ident = i > 0 && ident(chars[i - 1]);
        if !after_ident && (c == 'r' || (c == 'b' && next == Some('r'))) {
            let mut j = i + if c == 'b' { 2 } else { 1 };
            let mut hashes = 0;
            while j < len && chars[j] == '#' {
                hashes += 1;
                j += 1;
            }
            if j < len && chars[j] == '"' {
                j += 1;
                while j < len {
                    if chars[j] == '"' && (0..hashes).all(|k| chars.get(j + 1 + k) == Some(&'#')) {
                        j += 1 + hashes;
                        break;
                    }
                    j += 1;
                }
                out.push_str("\"\"");
                i = j;
                continue;
            }
        }
        if c == '"' {
            i += 1;
            while i < len {
                match chars[i] {
                    '\\' => i += 2,
                    '"' => {
                        i += 1;
                        break;
                    }
                    _ => i += 1,
                }
            }
            out.push_str("\"\"");
            continue;
        }
        if c == '\'' {
            if next == Some('\\') {
                let mut j = i + 2;
                while j < len && chars[j] != '\'' {
                    j += 1;
                }
                i = j + 1;
                out.push_str("' '");
                continue;
            }
            if chars.get(i + 2) == Some(&'\'') {
                i += 3;
                out.push_str("' '");
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Every construction in `source`'s code: `(needle, 1-based line)`.
fn constructions(source: &str) -> Vec<(&'static str, usize)> {
    let code = code_only(source);
    let mut found = Vec::new();
    for (index, line) in code.lines().enumerate() {
        for needle in CONSTRUCTIONS {
            let mut from = 0;
            while let Some(at) = line[from..].find(needle) {
                let end = from + at + needle.len();
                let followed = line[end..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
                if !followed {
                    found.push((*needle, index + 1));
                }
                from = end;
            }
        }
    }
    found
}

/// Every Rust source under the app crate's `src/`, relative to it with `/`
/// separators.
fn app_sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let relative = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((relative, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    out
}

// Gate: no app-crate source builds a renderer or a GPU device outside the
// helper, but for the two pinned production sites; the helper builds each of
// its renderers through the one decision, with the variable read.
#[test]
fn app_gpu_tests_honor_require_gpu() {
    let sources = app_sources();
    assert!(sources.len() > 200, "the scan reads the crate");
    let mut strays = Vec::new();
    for (path, source) in &sources {
        if path == HELPER {
            continue;
        }
        let found = constructions(source);
        let admitted = |needle: &str| {
            PRODUCTION
                .iter()
                .find(|(file, admitted, _)| file == path && *admitted == needle)
                .map(|(_, _, count)| *count)
        };
        for needle in CONSTRUCTIONS {
            let lines: Vec<usize> = found
                .iter()
                .filter(|(found, _)| found == needle)
                .map(|(_, line)| *line)
                .collect();
            match admitted(needle) {
                Some(count) => assert_eq!(
                    lines.len(),
                    count,
                    "{path}: the production `{needle}` sites changed: lines {lines:?}"
                ),
                None => strays.extend(
                    lines
                        .into_iter()
                        .map(|line| format!("{path}:{line} {needle}")),
                ),
            }
        }
    }
    assert!(
        strays.is_empty(),
        "renderer or GPU device built outside {HELPER} (use offscreen_renderer_or_skip or a sibling): {strays:?}"
    );

    let helper = sources
        .iter()
        .find(|(path, _)| path == HELPER)
        .map(|(_, source)| source)
        .expect("the helper exists");
    let helper_code = code_only(helper);
    assert_eq!(
        constructions(helper).len(),
        2,
        "the helper's constructors: the offscreen renderer and its held-build form"
    );
    assert_eq!(
        helper_code.matches("gpu_required(),").count(),
        2,
        "each constructor passes the variable's verdict to the decision"
    );
    assert!(helper.contains("std::env::var(\"POSTRETRO_REQUIRE_GPU\")"));
    // The helper tells a missing adapter by the renderer's own error.
    let init = include_str!("../../../renderer/src/render/renderer_init.rs");
    assert!(init.contains(".context(\"frame capture requires a GPU adapter\")"));

    // The gate sees what it forbids, and nothing in comments or strings.
    let fixture = "fn t() {\n    let r = Renderer::new_offscreen(64, 64);\n    // Renderer::new(&w) in a comment\n    let s = \"Renderer::new(&window)\";\n    let held = postretro_renderer::Renderer::new_offscreen_holding_pipeline_build(1, 1);\n    let f = Renderer::new;\n    let c = '\"';\n    let a = instance.request_adapter(&options);\n}\n";
    assert_eq!(
        constructions(fixture),
        [
            ("new_offscreen", 2),
            ("new_offscreen_holding_pipeline_build", 5),
            ("Renderer::new", 6),
            ("request_adapter", 8),
        ]
    );
    assert!(
        constructions("fn f() { offscreen_renderer_or_skip(test); r#\"new_offscreen(\"#; }")
            .is_empty()
    );
}

// The helper's decision, without removing the real adapter: a missing
// adapter skips unless the variable is set, then it fails; an adapter that
// fails init always fails; the variable reads as the renderer harness reads
// it.
#[test]
fn renderer_helper_fails_a_missing_adapter_only_when_required() {
    let no_adapter = || Err::<u32, _>(anyhow::anyhow!("frame capture requires a GPU adapter"));
    assert_eq!(renderer_or_skip("unset", no_adapter(), false), None);
    let required = std::panic::catch_unwind(|| renderer_or_skip("set", no_adapter(), true));
    assert!(required.is_err(), "set and no adapter: the test fails");
    assert_eq!(renderer_or_skip("present", Ok(7), true), Some(7));
    let broken = std::panic::catch_unwind(|| {
        renderer_or_skip::<u32>("broken", Err(anyhow::anyhow!("device lost in init")), false)
    });
    assert!(broken.is_err(), "an adapter that fails init fails the test");

    assert!(!gpu_required_by(None));
    assert!(!gpu_required_by(Some("")));
    assert!(!gpu_required_by(Some("0")));
    assert!(gpu_required_by(Some("1")));
    assert!(gpu_required_by(Some("yes")));
}
