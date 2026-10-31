//! Source gates for coloured bounce: means are never persisted, one material
//! module computes them, and authors get no albedo or saturation knob and one
//! bounce knob, the map key `_bounce_fill`.
//! See: context/lib/build_pipeline.md §Compiler pipeline, §Baked texture mips

use std::path::{Path, PathBuf};
use std::process::Command;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn workspace_root() -> PathBuf {
    crate_root()
        .parent()
        .and_then(Path::parent)
        .expect("level-compiler crate must be two levels below the workspace root")
        .to_path_buf()
}

struct Source {
    /// `/`-separated path below `src/`.
    path: String,
    text: String,
}

/// Every Rust source of the level compiler, production and test alike.
fn compiler_sources() -> Vec<Source> {
    fn walk(root: &Path, directory: &Path, out: &mut Vec<Source>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                out.push(Source {
                    path: path
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    text: std::fs::read_to_string(&path).unwrap(),
                });
            }
        }
    }
    let root = crate_root().join("src");
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    assert!(out.len() > 100, "the walk must reach the whole compiler");
    out
}

/// The text between the `(` at `open` and its matching `)`, ignoring
/// parentheses inside string literals.
fn balanced_args(text: &str, open: usize) -> &str {
    assert_eq!(text.as_bytes()[open], b'(');
    let (mut depth, mut in_string, mut escaped) = (0_usize, false, false);
    for (offset, byte) in text.as_bytes()[open..].iter().enumerate() {
        match (in_string, escaped, byte) {
            (true, true, _) => escaped = false,
            (true, false, b'\\') => escaped = true,
            (true, false, b'"') => in_string = false,
            (true, false, _) => {}
            (false, _, b'"') => in_string = true,
            (false, _, b'(') => depth += 1,
            (false, _, b')') => {
                depth -= 1;
                if depth == 0 {
                    return &text[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced call at byte {open}");
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn contains_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(start, _)| {
        let before = start.checked_sub(1).map(|index| text.as_bytes()[index]);
        let after = text.as_bytes().get(start + word.len()).copied();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// Calls whose arguments reach a file or a stage-cache entry: direct and
/// atomic writes, streamed writers, cache puts and writer serializers.
const PERSIST_SINKS: [&str; 7] = [
    "fs::write",
    "atomic_write",
    "write_all",
    ".put",
    ".put_streamed",
    "to_writer",
    "to_io",
];

/// Names that carry a derived material statistic.
const STATS_TOKENS: [&str; 6] = [
    "MaterialStats",
    "BounceMaterials",
    "bounce_materials",
    "material_stats",
    "snapped_mean",
    "albedo",
];

struct SinkCall {
    path: String,
    sink: &'static str,
    args: String,
}

fn persist_calls(source: &Source) -> Vec<SinkCall> {
    let mut calls = Vec::new();
    for sink in PERSIST_SINKS {
        for (start, _) in source.text.match_indices(sink) {
            let end = start + sink.len();
            let before = start
                .checked_sub(1)
                .map(|index| source.text.as_bytes()[index]);
            // `fs::write` inside `std::fs::write` is fine; `xfs::write` is not.
            if !sink.starts_with('.') && before.is_some_and(is_ident) {
                continue;
            }
            // Skip `atomic_write` definitions and `put` prefixes like `.put_streamed`.
            if source.text[..start].ends_with("fn ") {
                continue;
            }
            let Some(open) = source.text[end..].find(|c: char| !is_ident(c as u8)) else {
                continue;
            };
            let open = end + open;
            if open != end || source.text.as_bytes().get(open) != Some(&b'(') {
                continue;
            }
            calls.push(SinkCall {
                path: source.path.clone(),
                sink,
                args: balanced_args(&source.text, open).to_owned(),
            });
        }
    }
    calls
}

/// A sink call that names a stats value. In files that hold the stats types,
/// a bare `stats` binding is one too.
fn persists_stats(call: &SinkCall, holds_stats: bool) -> bool {
    STATS_TOKENS
        .iter()
        .any(|token| contains_word(&call.args, token))
        || (holds_stats && contains_word(&call.args, "stats"))
}

fn holds_stats(source: &Source) -> bool {
    ["MaterialStats", "BounceMaterials"]
        .iter()
        .any(|token| source.text.contains(token))
}

#[test]
fn no_compiler_write_or_cache_put_takes_a_material_stat() {
    // The scanner must see a violation for the gate to mean anything.
    let planted = Source {
        path: "planted.rs".into(),
        text: "use crate::surface_materials::stats::MaterialStats;\n\
               fn leak(stats: &[MaterialStats], cache: &Cache) {\n\
                   cache.put(&key, &postcard::to_allocvec(stats).unwrap());\n\
                   std::fs::write(path, format!(\"{:?}\", stats[0].snapped_mean()));\n\
               }\n"
        .into(),
    };
    let planted_calls = persist_calls(&planted);
    assert_eq!(planted_calls.len(), 2);
    assert!(planted_calls.iter().all(|call| persists_stats(call, true)));

    let sources = compiler_sources();
    let mut checked = 0;
    let mut violations = Vec::new();
    for source in &sources {
        let holds = holds_stats(source);
        for call in persist_calls(source) {
            checked += 1;
            if persists_stats(&call, holds) {
                violations.push(format!("{} {}({})", call.path, call.sink, call.args));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "stats persisted:\n{}",
        violations.join("\n")
    );
    // The material stage's own sidecar write and the SH caches are in scope.
    for (path, sink) in [
        ("texture_mips/world.rs", "atomic_write"),
        ("surface_radiance/layer_set.rs", ".put_streamed"),
        ("surface_radiance/volume.rs", ".put"),
        ("delta_sh_cache.rs", ".put"),
    ] {
        let source = sources.iter().find(|source| source.path == path).unwrap();
        assert!(
            persist_calls(source).iter().any(|call| call.sink == sink),
            "{path} must expose its {sink} call to the gate"
        );
    }
    assert!(checked > 100, "only {checked} persistence calls scanned");

    // The record type itself has no serialized form to persist.
    let stats = sources
        .iter()
        .find(|source| source.path == "surface_materials/stats.rs")
        .unwrap();
    let production = stats.text.split("#[cfg(test)]").next().unwrap();
    for token in [
        "Serialize",
        "serde",
        "postcard",
        "to_bytes",
        "std::fs",
        "io::Write",
    ] {
        assert!(
            !production.contains(token),
            "surface_materials/stats.rs must not serialize stats ({token})"
        );
    }
}

fn under(path: &str, module: &str) -> bool {
    path == format!("{module}.rs") || path.starts_with(&format!("{module}/"))
}

#[test]
fn only_the_material_modules_build_the_srgb_decode_or_derive_stats() {
    let mut callers = Vec::new();
    for source in compiler_sources() {
        for needle in [
            "build_srgb_to_linear_lut(",
            "MaterialStats::from_slots(",
            "mean_from_slots(",
        ] {
            for (start, _) in source.text.match_indices(needle) {
                if source.text[..start].ends_with("fn ") {
                    continue;
                }
                callers.push((source.path.clone(), needle));
            }
        }
    }
    let stray: Vec<_> = callers
        .iter()
        .filter(|(path, _)| !under(path, "texture_mips") && !under(path, "surface_materials"))
        .collect();
    assert!(
        stray.is_empty(),
        "texture means are computed only by TextureMips and the shared material module: {stray:?}"
    );
    for module in ["texture_mips", "surface_materials"] {
        assert!(
            callers.iter().any(|(path, _)| under(path, module)),
            "{module} must hold a scanned caller"
        );
    }
    assert!(
        callers
            .iter()
            .any(|(path, needle)| path == "texture_mips/world.rs"
                && *needle == "MaterialStats::from_slots("),
        "the material stage derives stats through the shared module"
    );
}

const KNOB_WORDS: [&str; 3] = ["bounce", "albedo", "saturation"];

fn knob_word(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    KNOB_WORDS.into_iter().find(|word| lower.contains(word))
}

/// String literals in `text`, without their quotes.
fn string_literals(text: &str) -> Vec<&str> {
    let mut literals = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'"' {
            let start = index + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'"' {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            literals.push(&text[start..end.min(bytes.len())]);
            index = end + 1;
        } else {
            index += 1;
        }
    }
    literals
}

/// One FGD class: its name and each field's declaration, description and
/// choice lines. Comments are dropped.
struct FgdClass {
    name: String,
    fields: Vec<(String, String)>,
}

/// `line` with its string literals removed, so bracketed prose in a
/// description cannot open or close a block.
fn unquoted(line: &str) -> String {
    line.split('"').step_by(2).collect()
}

fn fgd_classes(fgd: &str) -> Vec<FgdClass> {
    let mut classes = Vec::new();
    let mut header = String::new();
    let mut current: Option<FgdClass> = None;
    let mut depth = 0_usize;
    for line in fgd.lines() {
        let code = line.split("//").next().unwrap().trim();
        if code.is_empty() {
            continue;
        }
        if let Some(class) = current.as_mut() {
            if depth == 1 && code == "]" {
                classes.push(current.take().unwrap());
                depth = 0;
                continue;
            }
            if depth == 1 && !code.starts_with('[') {
                let field = code.split(['(', ' ']).next().unwrap().to_owned();
                class.fields.push((field, code.to_owned()));
            } else if let Some((_, text)) = class.fields.last_mut() {
                text.push('\n');
                text.push_str(code);
            }
            let unquoted = unquoted(code);
            depth += unquoted.matches('[').count();
            depth -= unquoted.matches(']').count();
            continue;
        }
        if code.starts_with('@') || !header.is_empty() {
            header.push(' ');
            header.push_str(code);
            if header.trim_end().ends_with('[') {
                let name = header
                    .split_once('=')
                    .expect("class header names its class")
                    .1
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .to_owned();
                current = Some(FgdClass {
                    name,
                    fields: Vec::new(),
                });
                header.clear();
                depth = 1;
            }
        }
    }
    assert!(current.is_none(), "FGD ends inside a class");
    classes
}

/// The one author-facing bounce control: a worldspawn key, never a flag.
const BOUNCE_FILL: &str = "_bounce_fill";

#[test]
fn no_worldspawn_key_fgd_field_or_cli_flag_steers_bounce_albedo_or_saturation() {
    // FGD: `_bounce_fill` is the one bounce field; fog's scatter
    // `saturation` predates and is unrelated to bounce.
    const ADMITTED: &[(&str, &str)] = &[
        ("worldspawn", BOUNCE_FILL),
        ("fog_volume", "saturation"),
        ("fog_lamp", "saturation"),
        ("fog_tube", "saturation"),
    ];
    let fgd = std::fs::read_to_string(workspace_root().join("sdk/TrenchBroom/postretro.fgd"))
        .expect("the shipped FGD");
    let classes = fgd_classes(&fgd);
    let worldspawn = classes
        .iter()
        .find(|class| class.name == "worldspawn")
        .expect("FGD declares worldspawn");
    assert!(worldspawn.fields.len() > 3, "worldspawn fields parsed");
    let mut allowed = Vec::new();
    for class in &classes {
        for (field, text) in &class.fields {
            let Some(word) = knob_word(text) else {
                continue;
            };
            assert!(
                ADMITTED.contains(&(class.name.as_str(), field.as_str())),
                "FGD {}.{field} mentions {word}: {text}",
                class.name
            );
            allowed.push((class.name.as_str(), field.as_str()));
        }
    }
    assert_eq!(allowed, ADMITTED, "the allowlist names live fields only");

    // Worldspawn keys: every literal the map parser reads against worldspawn.
    let parse = std::fs::read_to_string(crate_root().join("src/parse.rs")).unwrap();
    let mut keys = Vec::new();
    for (start, _) in parse.match_indices("&worldspawn_id,") {
        let rest = &parse[start..];
        let call_end = rest.find(')').unwrap();
        keys.extend(
            string_literals(&rest[..call_end])
                .into_iter()
                .map(str::to_owned),
        );
    }
    for key in [
        "data_script",
        "_lightmap_density",
        "_sh_coarsen",
        "nav_agent_radius",
        BOUNCE_FILL,
    ] {
        assert!(
            keys.iter().any(|read| read == key),
            "missed worldspawn key {key}"
        );
    }
    let knob_keys: Vec<&str> = keys
        .iter()
        .map(String::as_str)
        .filter(|key| knob_word(key).is_some())
        .collect();
    assert_eq!(knob_keys, [BOUNCE_FILL], "worldspawn keys naming a knob");

    // CLI: every literal the argument parser matches, and the help it prints.
    let main = std::fs::read_to_string(crate_root().join("src/main.rs")).unwrap();
    let parser_start = main.find("fn parse_args_from").unwrap();
    let parser_end = parser_start + main[parser_start..].find("\n}\n").unwrap();
    let flags: Vec<_> = string_literals(&main[parser_start..parser_end])
        .into_iter()
        .filter(|literal| literal.starts_with('-'))
        .collect();
    for flag in [
        "--no-cache",
        "--jobs",
        "--soft-shadow-samples",
        "--baked-root",
    ] {
        assert!(flags.contains(&flag), "missed parser flag {flag}");
    }
    for flag in &flags {
        assert!(knob_word(flag).is_none(), "prl-build flag {flag}");
    }
    let help = Command::new(env!("CARGO_BIN_EXE_prl-build"))
        .arg("--help")
        .output()
        .expect("run prl-build --help");
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("--no-cache"), "help lists flags");
    assert_eq!(knob_word(&help), None, "prl-build --help:\n{help}");
}

/// The compiler's `DEFAULT_BOUNCE_FILL`, read from its source as `{:?}`
/// prints it, so the docs gate follows the constant.
fn default_bounce_fill() -> String {
    let source =
        std::fs::read_to_string(crate_root().join("src/surface_radiance/bounce.rs")).unwrap();
    let declaration = "const DEFAULT_BOUNCE_FILL: f32 = ";
    let start = source
        .find(declaration)
        .expect("DEFAULT_BOUNCE_FILL declared")
        + declaration.len();
    let end = start + source[start..].find(';').unwrap();
    let value: f32 = source[start..end].trim().parse().expect("a float literal");
    format!("{value:?}")
}

// `_bounce_fill` reaches authors in the FGD's worldspawn and in
// `docs/level_design.md`, each naming its range and the compiler's default,
// and the docs present it as a map key with no build command beside it.
#[test]
fn bounce_fill_is_documented_in_the_fgd_and_level_design_docs() {
    let default = default_bounce_fill();
    let fgd = std::fs::read_to_string(workspace_root().join("sdk/TrenchBroom/postretro.fgd"))
        .expect("the shipped FGD");
    let classes = fgd_classes(&fgd);
    let worldspawn = classes
        .iter()
        .find(|class| class.name == "worldspawn")
        .expect("FGD declares worldspawn");
    let (_, field) = worldspawn
        .fields
        .iter()
        .find(|(name, _)| name == BOUNCE_FILL)
        .expect("worldspawn declares _bounce_fill");
    assert!(field.starts_with("_bounce_fill(float)"), "{field}");
    for needle in [format!("default {default}"), "0..1".to_owned()] {
        assert!(field.contains(&needle), "FGD field lacks {needle}: {field}");
    }

    let docs = std::fs::read_to_string(workspace_root().join("docs/level_design.md")).unwrap();
    // Whitespace-normalised, so reflowing the paragraph never fails the gate.
    let paragraph = docs
        .split("\n\n")
        .find(|paragraph| paragraph.contains("`_bounce_fill`"))
        .expect("docs/level_design.md describes _bounce_fill")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for needle in [
        "`worldspawn`".to_owned(),
        "`0`".to_owned(),
        "`1`".to_owned(),
        format!("default `{default}`"),
        "not a compiler flag".to_owned(),
    ] {
        assert!(
            paragraph.contains(&needle),
            "docs paragraph lacks {needle:?}:\n{paragraph}"
        );
    }
    for command in ["cargo", "xtask", "prl-build"] {
        assert!(
            !paragraph.contains(command),
            "docs paragraph names {command}"
        );
    }
}
