// Repository grep gates for environment volumes: projectile flight makes no
// environment query, and no volume FGD key authors swim tuning. Run by
// preflight's `cargo test`, beside the scripting surface gates.
// See: context/plans/in-progress/E24--water-and-environment-volumes/index.md §Decisions

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
