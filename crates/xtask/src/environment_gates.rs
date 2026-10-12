// Repository grep gates for environment volumes: projectile flight makes no
// environment query, no volume FGD key authors swim tuning, and the audio
// crate's public API (the muffle included) names no kira type. Run by
// preflight's `cargo test`, beside the scripting surface gates.
// See: context/lib/entity_model.md §7 (Environment Volumes) · context/lib/build_pipeline.md §Custom FGD · context/lib/audio.md §1

use std::path::{Path, PathBuf};

/// Projectile flight code, relative to the workspace: the stage file and, if
/// the stage is ever split, its submodule directory.
const PROJECTILE_FLIGHT_FILE: &str = "crates/sim/src/sim/projectile_stage.rs";
const PROJECTILE_FLIGHT_DIR: &str = "crates/sim/src/sim/projectile_stage";

/// The environment queries projectiles never make: they apply no gravity, no
/// push and no fluid, and splash from projectiles is a follow-up.
const ENVIRONMENT_QUERIES: &[&str] = &[
    "resolve_environment",
    "resolve_gravity",
    "visit_fluid_crossings",
    "fluid_at_eye",
];

const FGD_FILE: &str = "sdk/TrenchBroom/postretro.fgd";

/// Volume classes: they own the place, never the swimmer.
const VOLUME_CLASSES: &[&str] = &["fluid_volume", "gravity_volume", "push_volume"];

/// The player descriptor's `movement.swim` fields. Swim tuning is never
/// map-overridable, so no volume key may share a name with one.
const SWIM_FIELDS: &[&str] = &[
    "speed",
    "accel",
    "sinkSpeed",
    "enterDepth",
    "exitDepth",
    "fluidJumpVelocity",
];

/// The audio crate's sources. Its public API carries primitives only, so the
/// muffle and every other control cross the boundary without kira.
const AUDIO_CRATE_SRC: &str = "crates/audio/src";

fn workspace_root() -> PathBuf {
    crate::workspace_root().expect("workspace root")
}

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Rust source with each line's `//` comment removed, so a doc line naming a
/// query is not a call.
fn strip_line_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| line.find("//").map_or(line, |at| &line[..at]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 1-based lines on which `identifier` appears as a whole word in `code`.
fn identifier_lines(code: &str, identifier: &str) -> Vec<usize> {
    code.match_indices(identifier)
        .filter(|(at, _)| {
            let before = code[..*at].chars().next_back();
            let after = code[at + identifier.len()..].chars().next();
            !before.is_some_and(is_identifier_char) && !after.is_some_and(is_identifier_char)
        })
        .map(|(at, _)| code[..at].matches('\n').count() + 1)
        .collect()
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// The key names declared directly in an FGD class body: the first token of
/// each line at bracket depth 1, followed by `(`. Help strings (with `\"`
/// escapes) and nested `choices` blocks are skipped, so only key names count.
/// `None` when `class` has no body in `fgd`.
fn fgd_class_keys(fgd: &str, class: &str) -> Option<Vec<String>> {
    let declaration = fgd.match_indices(class).find_map(|(at, _)| {
        let before = fgd[..at].trim_end();
        let after = &fgd[at + class.len()..];
        (before.ends_with('=')
            && !after.chars().next().is_some_and(is_identifier_char)
            && after.trim_start().starts_with(':'))
        .then_some(at)
    })?;
    // The body opens at the first `[` outside the class's quoted title.
    let mut quoted = false;
    let open = declaration
        + fgd[declaration..].char_indices().find_map(|(at, c)| {
            if c == '"' {
                quoted = !quoted;
            }
            (c == '[' && !quoted).then_some(at)
        })?;

    let mut keys = Vec::new();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut line_start = true;
    let mut chars = fgd[open..].char_indices().peekable();
    while let Some((offset, c)) = chars.next() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                '\n' => line_start = true,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(keys);
                }
            }
            '\n' => {
                line_start = true;
                continue;
            }
            c if line_start && depth == 1 && is_identifier_char(c) => {
                let rest = &fgd[open + offset..];
                let name: String = rest
                    .chars()
                    .take_while(|&c| is_identifier_char(c))
                    .collect();
                if rest[name.len()..].trim_start().starts_with('(') {
                    keys.push(name.clone());
                }
                for _ in 1..name.chars().count() {
                    chars.next();
                }
            }
            _ => {}
        }
        if !c.is_whitespace() {
            line_start = false;
        }
    }
    None
}

/// Each unrestricted `pub` declaration in `code` (`pub(crate)`, `pub(super)`
/// and `pub(in …)` are skipped), with its 1-based line: the text from `pub`
/// through its signature, ending at the first `{`, `;` or `,` outside
/// brackets, so a multi-line signature, a field's type and a `pub use` path
/// are all read whole.
fn public_declarations(code: &str) -> Vec<(usize, String)> {
    let mut declarations = Vec::new();
    for (at, _) in code.match_indices("pub") {
        let before = code[..at].chars().next_back();
        let after = code[at + 3..].chars().next();
        if before.is_some_and(is_identifier_char) || !after.is_some_and(char::is_whitespace) {
            continue;
        }
        let mut depth = 0i32;
        let mut previous = ' ';
        let mut end = code.len();
        for (offset, c) in code[at..].char_indices() {
            match c {
                '(' | '[' | '<' => depth += 1,
                ')' | ']' => depth -= 1,
                '>' if previous != '-' && previous != '=' => depth -= 1,
                '{' | ';' | ',' if depth <= 0 => {
                    end = at + offset;
                    break;
                }
                _ => {}
            }
            previous = c;
        }
        let line = code[..at].matches('\n').count() + 1;
        declarations.push((line, code[at..end].to_string()));
    }
    declarations
}

// Projectiles apply no environment: their flight code never resolves
// gravity, push or fluid, and never walks fluid crossings.
#[test]
fn projectile_flight_makes_no_environment_query() {
    let root = workspace_root();
    let stage = root.join(PROJECTILE_FLIGHT_FILE);
    assert!(
        stage.is_file(),
        "{PROJECTILE_FLIGHT_FILE} moved; point this gate at the projectile flight code"
    );
    let mut files = vec![stage];
    files.extend(rust_files(&root.join(PROJECTILE_FLIGHT_DIR)));

    let mut violations = Vec::new();
    for path in files {
        let source = std::fs::read_to_string(&path).expect("projectile flight source reads");
        let code = strip_line_comments(&source);
        let file = path.strip_prefix(&root).unwrap_or(&path).display();
        for query in ENVIRONMENT_QUERIES {
            for line in identifier_lines(&code, query) {
                violations.push(format!("{file}:{line}: `{query}`"));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "projectile flight makes an environment query (E24 non-goal):\n{}",
        violations.join("\n")
    );
}

// Swim tuning belongs to the player descriptor; a volume's FGD keys never
// name a `movement.swim` field.
#[test]
fn volume_fgd_keys_name_no_swim_field() {
    let root = workspace_root();
    let fgd = std::fs::read_to_string(root.join(FGD_FILE)).expect("FGD reads");
    let mut violations = Vec::new();
    for class in VOLUME_CLASSES {
        let keys = fgd_class_keys(&fgd, class)
            .unwrap_or_else(|| panic!("{FGD_FILE} declares no `{class}` body"));
        assert!(
            keys.iter().any(|key| key == "priority"),
            "`{class}` parsed without its `priority` key: {keys:?}"
        );
        for key in keys {
            if SWIM_FIELDS
                .iter()
                .any(|field| field.eq_ignore_ascii_case(&key))
            {
                violations.push(format!("{class}: `{key}`"));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "volume FGD keys name a movement.swim field:\n{}",
        violations.join("\n")
    );
}

// The audio crate is the only crate that names kira, and its public API
// carries primitives only: `Audio` is concrete over the default backend.
#[test]
fn audio_public_api_names_no_kira_type() {
    let root = workspace_root();
    let mut violations = Vec::new();
    let mut declarations = 0;
    for path in rust_files(&root.join(AUDIO_CRATE_SRC)) {
        // In-crate test modules drive the core on capturing and mock backends.
        if path.components().any(|part| part.as_os_str() == "tests") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("audio source reads");
        let code = strip_line_comments(&source);
        let file = path.strip_prefix(&root).unwrap_or(&path).display();
        for (line, declaration) in public_declarations(&code) {
            declarations += 1;
            if !identifier_lines(&declaration, "kira").is_empty() {
                violations.push(format!("{file}:{line}: `{}`", declaration.trim()));
            }
        }
    }
    assert!(
        declarations >= 20,
        "{AUDIO_CRATE_SRC} moved or the scan found only {declarations} public declarations"
    );
    assert!(
        violations.is_empty(),
        "the audio crate's public API names a kira type:\n{}",
        violations.join("\n")
    );
}

#[test]
fn public_declaration_scan_reads_whole_signatures_and_skips_restricted_items() {
    let code = "pub struct Audio<B: kira::backend::Backend = DefaultBackend> {\n\
                    core: Core,\n\
                }\n\
                pub(crate) struct AudioCore<B: kira::backend::Backend> {}\n\
                pub fn update(\n    &mut self,\n    resolve: impl FnMut(u64) -> Option<[f32; 3]>,\n    track: &kira::Track,\n) {}\n\
                pub use kira::Tween;\n\
                pub field: [f32; 3],\n";
    let declarations = public_declarations(code);
    let texts: Vec<&str> = declarations.iter().map(|(_, text)| text.as_str()).collect();
    assert_eq!(texts.len(), 4, "{texts:?}");
    assert!(texts[0].contains("kira::backend::Backend"));
    assert_eq!(declarations[1].0, 5);
    assert!(texts[1].contains("track: &kira::Track"), "{}", texts[1]);
    assert_eq!(texts[2], "pub use kira::Tween");
    assert_eq!(texts[3], "pub field: [f32; 3]");
}

#[test]
fn fgd_key_parser_reads_key_names_not_help_text() {
    let fgd = r#"
@SolidClass = fluid_volume_extra : "not this one" [ speed(float) : "x" ]
@SolidClass
    color(1 2 3)
    = fluid_volume : "Fluid [brush]"
[
    fluid(string) : "Fluid" : "" : "Swim speed and acceleration come from \"movement.swim\" [not here]."
    mode(choices) : "Mode" : 0 =
    [
        0 : "accel"
        1 : "speed"
    ]
    sinkSpeed(float) : "Sink" : "" : "flagged"
]
@SolidClass = gravity_volume : "Gravity" [ gravity(float) : "g" ]
"#;
    assert_eq!(
        fgd_class_keys(fgd, "fluid_volume"),
        Some(vec![
            "fluid".to_string(),
            "mode".to_string(),
            "sinkSpeed".to_string()
        ])
    );
    assert_eq!(fgd_class_keys(fgd, "push_volume"), None);
}

#[test]
fn projectile_gate_matches_code_identifiers_only() {
    let source = "// resolve_gravity in a comment\n\
                  let g = resolve_gravity(view, p);\n\
                  let h = my_resolve_gravity_cache;\n\
                  postretro_collision::visit_fluid_crossings(v, o, d, t, |_| {});\n";
    let code = strip_line_comments(source);
    assert_eq!(identifier_lines(&code, "resolve_gravity"), vec![2]);
    assert_eq!(identifier_lines(&code, "visit_fluid_crossings"), vec![4]);
}
