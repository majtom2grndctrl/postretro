// Repository grep gates for emissive volumes: only the compiler opens
// `materials.toml` or reads the authored glow range keys, no emissive
// primitive's arguments carry that range, the photosensitivity source floor
// names `emissive_volume` as a built clamp source, and no shader reads `_e`
// alpha. Run by preflight's `cargo test`, beside the scripting surface gates.
// See: context/lib/rendering_pipeline.md §7.3, §7.8 · context/lib/scripting.md §10.2a · context/lib/build_pipeline.md §Compiler pipeline

use std::path::{Path, PathBuf};

use crate::surface_gates::{is_identifier_char, line_of, relative, workspace_root};

mod rust_scan;
mod script_shapes;
mod wgsl_scan;

use rust_scan::{RustText, lex_rust, scan_rust_struct};
use script_shapes::{
    Lang, args_expressions, args_type_keys, brace_members, builder_return_keys,
    strip_script_comments, type_body, unquoted_range_keys,
};
use wgsl_scan::emissive_alpha_reads;

/// Workspace crates, one directory each.
const CRATES_DIR: &str = "crates";
/// The one crate that reads `materials.toml` and the authored range keys.
const COMPILER_CRATE: &str = "level-compiler";
/// This crate: the gates spell their own needles.
const GATE_CRATE: &str = "xtask";

/// Per-texture glow strength lives here, and only the bake reads it: the
/// runtime reads the baked values (id 60).
const MATERIALS_FILE: &str = "materials.toml";

/// The authored glow range: map content the compiler bakes into id 60.
const RANGE_KEYS: &[&str] = &["emissive_min", "emissive_max"];
/// Field-name words that would carry the range through a primitive.
const RANGE_WORDS: &[&str] = &["min", "max"];

/// The emissive primitives: one file per primitive's args, and the module
/// whose `EMISSIVE_PRIMITIVES` names every primitive.
const EMISSIVE_REACTIONS_DIR: &str = "crates/sim/src/fx/emissive_reactions";
const EMISSIVE_PRIMITIVES_FILE: &str = "crates/sim/src/fx/emissive_reactions/mod.rs";
const EMISSIVE_PRIMITIVES_CONST: &str = "EMISSIVE_PRIMITIVES";
/// Where the component types an args wrapper carries are declared
/// (`EmissiveAnimation`).
const EMISSIVE_COMPONENT_FILE: &str = "crates/entities/src/components/emissive_volume.rs";

/// The SDK handles whose methods emit primitive steps. Their `pulse` /
/// `flicker` option bags take `min` / `max` as brightness sugar; only the
/// emitted `args` are checked.
const SDK_HANDLE_FILES: &[&str] = &[
    "sdk/lib/entities/emissive_volumes.ts",
    "sdk/lib/entities/emissive_volumes.luau",
];
/// Files that declare emissive step shapes, hand-written or generated.
const SDK_STEP_FILES: &[&str] = &[
    "sdk/lib/data_script.ts",
    "sdk/lib/data_script.luau",
    GENERATED_TS,
    GENERATED_LUAU,
];
/// Generated typedefs: each declares every emissive step.
const GENERATED_TS: &str = "sdk/types/postretro.d.ts";
const GENERATED_LUAU: &str = "sdk/types/postretro.d.luau";

/// Script trees that could read a map KVP by name (`getEntityProperty`).
const SCRIPT_ROOTS: &[&str] = &["sdk/lib", "sdk/behaviors", "sdk/templates", "content"];
const SCRIPT_EXTENSIONS: &[&str] = &["ts", "js", "luau"];

/// The photosensitivity source floor's brief, found in whichever plan status
/// directory it sits in today.
const PLANS_DIR: &str = "context/plans";
const SOURCE_FLOOR_BRIEF: &str = "E23--photosensitivity-source-floor/index.md";
const EMISSIVE_CLASS: &str = "emissive_volume";

/// Renderer shaders; every emissive texture sample takes `.rgb`.
const SHADER_DIR: &str = "crates/renderer/src/shaders";

fn read(root: &Path, file: &str) -> String {
    std::fs::read_to_string(root.join(file))
        .unwrap_or_else(|_| panic!("{file} moved; point this gate at it"))
}

fn files_with_extensions(dir: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                // Build output and dependencies are not authored source.
                if path
                    .file_name()
                    .is_some_and(|name| name == "node_modules" || name == "target")
                {
                    continue;
                }
                pending.push(path);
            } else if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| extensions.contains(&ext))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Byte offsets where `word` stands as a whole identifier in `text`.
fn identifier_at(text: &str, word: &str) -> Vec<usize> {
    text.match_indices(word)
        .filter(|(at, _)| {
            let before = text[..*at].chars().next_back();
            let after = text[at + word.len()..].chars().next();
            !before.is_some_and(is_identifier_char) && !after.is_some_and(is_identifier_char)
        })
        .map(|(at, _)| at)
        .collect()
}

/// The lowercase words of a snake_case or camelCase name.
fn name_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut previous_lower = false;
    for c in name.chars() {
        if !c.is_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            previous_lower = false;
            continue;
        }
        if c.is_uppercase() && previous_lower {
            words.push(std::mem::take(&mut word));
        }
        previous_lower = c.is_lowercase() || c.is_ascii_digit();
        word.extend(c.to_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

fn names_range(name: &str) -> bool {
    name_words(name)
        .iter()
        .any(|word| RANGE_WORDS.contains(&word.as_str()))
}

/// The offset of the bracket closing the one at `open` in `text`, counting
/// every bracket kind as nesting. `text` has its strings removed or holds
/// none that carry brackets.
fn matching_close(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (offset, c) in text[open..].char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Gates
// ---------------------------------------------------------------------------

/// Every workspace crate directory but the compiler's and this one's.
fn runtime_crates(root: &Path) -> Vec<PathBuf> {
    let mut crates: Vec<_> = std::fs::read_dir(root.join(CRATES_DIR))
        .expect("crates/ reads")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("Cargo.toml").is_file())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name != COMPILER_CRATE && name != GATE_CRATE)
        })
        .collect();
    crates.sort();
    crates
}

/// `materials.toml` in Rust source outside the compiler: a string literal
/// naming it is how a crate would open it. Comments do not count.
fn materials_file_violations(root: &Path) -> Vec<String> {
    let mut violations = Vec::new();
    for dir in runtime_crates(root) {
        for path in files_with_extensions(&dir, &["rs"]) {
            let source = std::fs::read_to_string(&path).expect("crate source reads");
            let text = lex_rust(&source);
            for (at, literal) in &text.literals {
                if literal.contains(MATERIALS_FILE) {
                    violations.push(format!(
                        "{}:{}: \"{literal}\"",
                        relative(root, &path),
                        line_of(&text.code, *at)
                    ));
                }
            }
        }
    }
    violations
}

/// The authored range keys read outside the compiler: as a Rust identifier
/// or an exact string key in another crate, or as a quoted or
/// unquoted table or object key in a script.
/// Prose naming them (doc comments, doc strings, help text) passes.
fn range_key_read_violations(root: &Path) -> Vec<String> {
    let mut violations = Vec::new();
    for dir in runtime_crates(root) {
        for path in files_with_extensions(&dir, &["rs"]) {
            let source = std::fs::read_to_string(&path).expect("crate source reads");
            let text = lex_rust(&source);
            for key in RANGE_KEYS {
                for at in identifier_at(&text.code, key) {
                    violations.push(format!(
                        "{}:{}: `{key}`",
                        relative(root, &path),
                        line_of(&text.code, at)
                    ));
                }
                for (at, literal) in &text.literals {
                    if literal.trim() == *key {
                        violations.push(format!(
                            "{}:{}: \"{key}\"",
                            relative(root, &path),
                            line_of(&text.code, *at)
                        ));
                    }
                }
            }
        }
    }
    for script_root in SCRIPT_ROOTS {
        for path in files_with_extensions(&root.join(script_root), SCRIPT_EXTENSIONS) {
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            // Comments are blanked first, so prose naming a key passes.
            let text = strip_script_comments(&raw, Lang::of(&relative(root, &path)));
            for key in RANGE_KEYS {
                for at in unquoted_range_keys(&text, key) {
                    violations.push(format!(
                        "{}:{}: unquoted key `{key}`",
                        relative(root, &path),
                        line_of(&text, at)
                    ));
                }
                for quote in ['"', '\''] {
                    let needle = format!("{quote}{key}{quote}");
                    for (at, _) in text.match_indices(&needle) {
                        violations.push(format!(
                            "{}:{}: {needle}",
                            relative(root, &path),
                            line_of(&text, at)
                        ));
                    }
                }
            }
        }
    }
    violations
}

/// The primitive names `EMISSIVE_PRIMITIVES` lists.
fn emissive_primitives(root: &Path) -> Vec<String> {
    let text = lex_rust(&read(root, EMISSIVE_PRIMITIVES_FILE));
    let at = identifier_at(&text.code, EMISSIVE_PRIMITIVES_CONST)
        .into_iter()
        .find(|&at| text.code[..at].trim_end().ends_with("const"))
        .unwrap_or_else(|| {
            panic!("{EMISSIVE_PRIMITIVES_FILE} declares no `{EMISSIVE_PRIMITIVES_CONST}`")
        });
    // The type may hold a `;` (`[&str; 3]`): the value runs from `=` to `;`.
    let equals = at + text.code[at..].find('=').expect("const has a value");
    let end = equals + text.code[equals..].find(';').expect("const ends");
    let primitives: Vec<String> = text
        .literals
        .iter()
        .filter(|(offset, _)| (at..end).contains(offset))
        .map(|(_, literal)| literal.clone())
        .collect();
    assert!(
        primitives.len() >= 3,
        "{EMISSIVE_PRIMITIVES_CONST} lists {primitives:?}; update this gate"
    );
    primitives
}

/// `setEmissiveBrightness` → `SetEmissiveBrightness`.
fn pascal(name: &str) -> String {
    let mut chars = name.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The three primitives' Rust argument types carry no range field: each
/// `<Primitive>Args` struct, and the component types a wrapper args struct
/// carries (`EmissiveAnimation`).
fn rust_args_violations(root: &Path, primitives: &[String]) -> Vec<String> {
    let sources: Vec<(String, RustText)> =
        files_with_extensions(&root.join(EMISSIVE_REACTIONS_DIR), &["rs"])
            .into_iter()
            .filter(|path| path.file_stem().is_some_and(|stem| stem != "tests"))
            .map(|path| {
                let source = std::fs::read_to_string(&path).expect("reaction source reads");
                (relative(root, &path), lex_rust(&source))
            })
            .collect();
    let component = lex_rust(&read(root, EMISSIVE_COMPONENT_FILE));
    let mut violations = Vec::new();
    for primitive in primitives {
        let name = format!("{}Args", pascal(primitive));
        let Some((file, scan)) = sources
            .iter()
            .find_map(|(file, text)| scan_rust_struct(text, &name).map(|scan| (file, scan)))
        else {
            violations.push(format!(
                "{EMISSIVE_REACTIONS_DIR}: no `struct {name}` for `{primitive}`; update this gate"
            ));
            continue;
        };
        for violation in scan.violations {
            violations.push(format!("{file}: `{name}` {violation}"));
        }
        if scan.tuple {
            let mut resolved = 0;
            for carried in &scan.carried {
                let Some(inner) = scan_rust_struct(&component, carried) else {
                    continue;
                };
                resolved += 1;
                for violation in inner.violations {
                    violations.push(format!(
                        "{EMISSIVE_COMPONENT_FILE}: `{carried}` (carried by `{name}`) {violation}"
                    ));
                }
            }
            if resolved == 0 {
                violations.push(format!(
                    "{file}: `{name}` wraps {:?}, none declared in {EMISSIVE_COMPONENT_FILE}; \
                     update this gate",
                    scan.carried
                ));
            }
        }
    }
    violations
}

/// The emissive step shapes' `args` carry no range key, in every file that
/// declares a step with a brace body. The generated typedefs declare every
/// step.
fn sdk_step_violations(root: &Path, primitives: &[String]) -> (Vec<String>, usize) {
    let generated_ts = strip_script_comments(&read(root, GENERATED_TS), Lang::Ts);
    let generated_luau = strip_script_comments(&read(root, GENERATED_LUAU), Lang::Luau);
    let mut violations = Vec::new();
    let mut checked = 0;
    for file in SDK_STEP_FILES {
        let lang = Lang::of(file);
        let text = strip_script_comments(&read(root, file), lang);
        let generated = match lang {
            Lang::Ts => generated_ts.as_str(),
            Lang::Luau => generated_luau.as_str(),
        };
        for primitive in primitives {
            let step = format!("{}Step", pascal(primitive));
            let Some(body) = type_body(&text, &step) else {
                // A hand-written file may alias a generated step
                // (`= import("postretro").…`) or leave it to the typedefs; a
                // generated file declares every shape.
                if *file == GENERATED_TS || *file == GENERATED_LUAU {
                    violations.push(format!("{file}: declares no `{step}` shape"));
                }
                continue;
            };
            let Some((_, args)) = brace_members(body)
                .into_iter()
                .find(|(key, _)| key == "args")
            else {
                violations.push(format!("{file}: `{step}` has no `args`"));
                continue;
            };
            checked += 1;
            match args_type_keys(&args, &[&text, generated]) {
                Ok(keys) => {
                    for key in keys.iter().filter(|key| names_range(key)) {
                        violations.push(format!("{file}: `{step}` args carry `{key}`"));
                    }
                }
                Err(reason) => violations.push(format!("{file}: `{step}` args: {reason}")),
            }
        }
    }
    (violations, checked)
}

/// What the SDK handles emit as `args` carries no range key: inline literals
/// and the literals each args builder returns. The handles' own option bags
/// (`pulse({ min, max, periodMs })`) are brightness sugar and are not read.
fn sdk_handle_violations(root: &Path) -> Vec<String> {
    let mut violations = Vec::new();
    for file in SDK_HANDLE_FILES {
        let lang = Lang::of(file);
        let text = strip_script_comments(&read(root, file), lang);
        let sites = args_expressions(&text, lang);
        if sites.len() < 3 {
            violations.push(format!(
                "{file}: {} `args` sites, expected pulse, fade and flicker's; update this gate",
                sites.len()
            ));
        }
        for (at, expression) in sites {
            let line = line_of(&text, at);
            let keys = if expression.starts_with('{') {
                matching_close(&expression, 0).map(|close| {
                    brace_members(&expression[1..close])
                        .into_iter()
                        .map(|(key, _)| key)
                        .collect::<Vec<_>>()
                })
            } else {
                let builder: String = expression
                    .chars()
                    .take_while(|&c| is_identifier_char(c))
                    .collect();
                (!builder.is_empty() && expression[builder.len()..].starts_with('('))
                    .then(|| builder_return_keys(&text, &builder))
                    .flatten()
            };
            let Some(keys) = keys else {
                violations.push(format!(
                    "{file}:{line}: cannot read `args` `{expression}`; update this gate"
                ));
                continue;
            };
            if keys.is_empty() {
                violations.push(format!(
                    "{file}:{line}: `args` `{expression}` builds no literal; update this gate"
                ));
            }
            for key in keys.iter().filter(|key| names_range(key)) {
                violations.push(format!("{file}:{line}: `args` carry `{key}`"));
            }
        }
    }
    violations
}

/// The source floor's brief, wherever its plan folder sits.
fn source_floor_briefs(root: &Path) -> Vec<PathBuf> {
    let Ok(statuses) = std::fs::read_dir(root.join(PLANS_DIR)) else {
        return Vec::new();
    };
    let mut briefs: Vec<PathBuf> = statuses
        .filter_map(Result::ok)
        .map(|entry| entry.path().join(SOURCE_FLOOR_BRIEF))
        .filter(|path| path.is_file())
        .collect();
    briefs.sort();
    briefs
}

// The runtime never opens `materials.toml`: no crate but the compiler names
// the file in its code (comments may).
#[test]
fn materials_toml_is_named_only_by_the_compiler() {
    let root = workspace_root();
    assert!(
        runtime_crates(&root).len() >= 10,
        "{CRATES_DIR} moved or holds too few crates"
    );
    let violations = materials_file_violations(&root);
    assert!(
        violations.is_empty(),
        "only `{COMPILER_CRATE}` reads `{MATERIALS_FILE}`; the runtime reads the baked \
         strengths (id 60):\n{}",
        violations.join("\n")
    );
}

// Only `setEmissiveBrightness`, `setEmissiveAnimation` and `setEmissiveColor`
// write a volume, and none carries the authored range: no Rust args type, SDK
// step shape or emitted SDK args has a `min` or `max` field. The authored
// keys are read only by the compiler.
#[test]
fn emissive_primitives_carry_no_range_and_only_the_compiler_reads_its_keys() {
    let root = workspace_root();
    let primitives = emissive_primitives(&root);
    let mut violations = rust_args_violations(&root, &primitives);
    let (step_violations, checked) = sdk_step_violations(&root, &primitives);
    violations.extend(step_violations);
    assert!(
        checked >= 2 * primitives.len(),
        "only {checked} emissive step shapes read; the generated typedefs declare every step"
    );
    violations.extend(sdk_handle_violations(&root));
    violations.extend(range_key_read_violations(&root));
    assert!(
        violations.is_empty(),
        "an emissive primitive carries the authored range, or a crate other than \
         `{COMPILER_CRATE}` reads {RANGE_KEYS:?}:\n{}",
        violations.join("\n")
    );
}

// The photosensitivity source floor records emissive volumes as a clamp
// source already built.
#[test]
fn source_floor_brief_names_emissive_volumes_as_a_built_clamp_source() {
    let root = workspace_root();
    let briefs = source_floor_briefs(&root);
    let [brief] = &briefs[..] else {
        panic!(
            "expected one {PLANS_DIR}/*/{SOURCE_FLOOR_BRIEF}, found {briefs:?}; point this gate \
             at where the source floor's design lives now"
        );
    };
    let text = std::fs::read_to_string(brief).expect("brief reads");
    assert!(
        !identifier_at(&text, EMISSIVE_CLASS).is_empty(),
        "{} no longer names `{EMISSIVE_CLASS}` as a built clamp source",
        relative(&root, brief)
    );
}

// `_e` alpha is reserved: every emissive texture sample in the renderer's
// shaders ends `.rgb`.
#[test]
fn every_emissive_texture_sample_reads_rgb_only() {
    let root = workspace_root();
    let mut violations = Vec::new();
    let mut sampled_files = Vec::new();
    for path in files_with_extensions(&root.join(SHADER_DIR), &["wgsl"]) {
        let wgsl = std::fs::read_to_string(&path).expect("shader reads");
        let (reads, samples) = emissive_alpha_reads(&wgsl);
        if samples > 0 {
            sampled_files.push(relative(&root, &path));
        }
        for (line, binding) in reads {
            violations.push(format!("{}:{line}: `{binding}`", relative(&root, &path)));
        }
    }
    assert!(
        sampled_files
            .iter()
            .any(|file| file.ends_with("forward.wgsl")),
        "no emissive texture sample found in forward.wgsl ({sampled_files:?}); update this gate"
    );
    assert!(
        violations.is_empty(),
        "an emissive texture sample reads more than `.rgb` (`_e` alpha is reserved):\n{}",
        violations.join("\n")
    );
}
