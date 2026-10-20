// Repository grep gates over source and the FGD: environment volumes (projectile
// flight makes no environment query, no volume FGD key authors swim tuning),
// the audio crate's public API (the muffle included) names no kira type, player
// option names (reflections take no style option, only the cost tier), and the
// world material inputs (material prefixes, texture suffixes, FGD classes and
// keys take no reflection input). Run by preflight's `cargo test`, beside the
// scripting surface gates.
// See: context/lib/entity_model.md §7 (Environment Volumes) · context/lib/build_pipeline.md §Custom FGD · context/lib/audio.md §1 · context/lib/player_options.md §4

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

// Where every player-option name is declared: the options module's settings
// keys and slot names (its `const` strings), the store's fields, and the
// engine-state catalog's wire names.

/// The options module tree, whose `const` strings are the settings keys and slot names.
const OPTIONS_DIR: &str = "crates/postretro/src/options";
/// The store structs whose field names are option names, as (file, struct).
const PLAYER_OPTIONS_STRUCTS: &[(&str, &str)] = &[
    ("crates/postretro/src/options/mod.rs", "PlayerOptions"),
    (
        "crates/postretro/src/options/accessibility.rs",
        "AccessibilityOptions",
    ),
];
/// The engine-state catalog, whose `options.*` wire names are option slots.
const ENGINE_STATE_CATALOG: &str = "crates/entities/src/engine_state_catalog.rs";

/// The only reflection option: the player's cost tier, as settings key and
/// field (`reflection_quality`) and as slot (`options.reflectionQuality`). The
/// style is the author's `render.reflections.style`, never the player's.
const ALLOWED_REFLECTION_OPTION_NAMES: &[&str] =
    &["reflection_quality", "options.reflectionQuality"];

/// Words that make a name a reflection setting, matched against the start of
/// each of its snake_case, dotted or camelCase words.
const REFLECTION_WORDS: &[&str] = &["reflect", "ssr", "mirror"];

/// Material vocabulary and texture-name rules: the material enum, its prefix
/// table and per-material properties, and the sibling-suffix rules that bind
/// surface maps to a face. None takes a reflection input.
const MATERIAL_RULE_FILES: &[&str] = &[
    MATERIAL_PREFIXES_FILE,
    BUNDLE_SUFFIXES_FILE,
    "crates/level-compiler/src/texture_validation.rs",
];

// What the material-input gate pins, with the exact sets each holds today. The
// word scan misses a new input spelled `_r` or `rough`, so each of these is
// pinned and a change to one needs a deliberate edit here (owner review):
//   - the material prefix table (`MATERIAL_PREFIXES`) in its file;
//   - the sibling-suffix list (`BUNDLE_SUFFIXES`), which only chooses the base
//     name a bundle resolves to;
//   - the suffix literals of the `format!("{resolved_base}_X")` lookups in the
//     same file, which are what actually bind each sibling map to a face.
// `texture_validation.rs`'s `ends_with("_n")` chain only picks which files get
// a color-space check; it binds nothing, so it is word-scanned but not pinned.

// The material prefix table: its file, const name and the prefixes it holds.
const MATERIAL_PREFIXES_FILE: &str = "crates/render-data/src/material.rs";
const MATERIAL_PREFIXES_CONST: &str = "MATERIAL_PREFIXES";
const MATERIAL_PREFIX_NAMES: &[&str] = &["metal", "concrete", "grate", "neon", "glass", "wood"];
// The sibling-suffix list: its file, const name and the suffixes it holds (the
// bare diffuse is `""`).
const BUNDLE_SUFFIXES_FILE: &str = "crates/level-compiler/src/texture_mips/resolution.rs";
const BUNDLE_SUFFIXES_CONST: &str = "BUNDLE_SUFFIXES";
const BUNDLE_SUFFIX_NAMES: &[&str] = &["", "_s", "_n", "_e", "_h"];

// The sibling lookups: the base identifier the `format!` literals in
// `BUNDLE_SUFFIXES_FILE` start from, and the suffixes they bind today, in
// source order.
const BUNDLE_BINDING_BASE: &str = "resolved_base";
const BUNDLE_BINDING_SUFFIX_NAMES: &[&str] = &["_s", "_n", "_e", "_h"];

/// The one FGD class that authors reflection, and its exact keys: an authored
/// probe box. A new key here is a new reflection input.
const REFLECTION_VOLUME_CLASS: &str = "reflection_volume";
const REFLECTION_VOLUME_KEYS: &[&str] = &["_capture_offset"];

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
/// escapes), `//` comments (whose brackets need not balance) and nested
/// `choices` blocks are skipped, so only key names count.
/// A `@BaseClass` has no title, so its body follows the name directly.
/// `None` when `class` has no body in `fgd`.
fn fgd_class_keys(fgd: &str, class: &str) -> Option<Vec<String>> {
    let declaration = fgd.match_indices(class).find_map(|(at, _)| {
        let before = fgd[..at].trim_end();
        let after = &fgd[at + class.len()..];
        (before.ends_with('=')
            && !after.chars().next().is_some_and(is_identifier_char)
            && after.trim_start().starts_with([':', '[']))
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
            '/' if fgd[open + offset..].starts_with("//") => {
                while chars.next_if(|&(_, c)| c != '\n').is_some() {}
                continue;
            }
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

/// Every class an FGD declares (`@…Class … = name : "Title"`, or an untitled
/// `@BaseClass = Name [`), in file order.
fn fgd_class_names(fgd: &str) -> Vec<String> {
    let mut names = Vec::new();
    for (at, _) in fgd.match_indices('@') {
        if at != 0 && !fgd[..at].ends_with('\n') {
            continue;
        }
        // The header runs to the body's `[`; its first `=` outside quotes
        // and attribute parentheses (`model({ … [radius, …] })`) precedes
        // the class name.
        let mut quoted = false;
        let mut parens = 0usize;
        let mut equals = None;
        for (offset, c) in fgd[at..].char_indices() {
            match c {
                '"' => quoted = !quoted,
                _ if quoted => {}
                '(' => parens += 1,
                ')' => parens = parens.saturating_sub(1),
                '=' if parens == 0 => {
                    equals = Some(at + offset + 1);
                    break;
                }
                '[' if parens == 0 => break,
                _ => {}
            }
        }
        let Some(equals) = equals else {
            continue;
        };
        let name: String = fgd[equals..]
            .trim_start()
            .chars()
            .take_while(|&c| is_identifier_char(c))
            .collect();
        if !name.is_empty() {
            names.push(name);
        }
    }
    names
}

/// The lowercase words of a name spelled in snake_case, dotted or camelCase.
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

/// Whether a name names a reflection setting: one of its words starts with a
/// reflection word.
fn names_reflection(name: &str) -> bool {
    name_words(name).iter().any(|word| {
        REFLECTION_WORDS
            .iter()
            .any(|reflection| word.starts_with(reflection))
    })
}

/// Every string literal in each `const` declaration typed over `str` (a
/// `&str`, or an array of them) in `code`.
///
/// Known limit: callers pass [`strip_line_comments`] output, which cuts a line
/// at any `//`, so a `//` inside a string literal (`"http://…"`) drops the rest
/// of that line. No such constant exists today.
fn const_string_literals(code: &str) -> Vec<String> {
    let mut literals = Vec::new();
    for (at, _) in code.match_indices("const ") {
        if code[..at]
            .chars()
            .next_back()
            .is_some_and(is_identifier_char)
        {
            continue;
        }
        // The type may hold a `;` (`[&str; 5]`), so the value starts at the
        // first `=` and ends at the next `;`. A `const fn` body opens first.
        let rest = &code[at..];
        let Some(equals) = rest.find('=') else {
            continue;
        };
        let ty = &rest[..equals];
        if ty.contains('{') || !ty.contains("str") {
            continue;
        }
        let value = &rest[equals + 1..];
        let value = &value[..value.find(';').unwrap_or(value.len())];
        literals.extend(value.split('"').skip(1).step_by(2).map(str::to_string));
    }
    literals
}

/// The string literals in the value of `const name` in `code`, in order: the
/// text between its first `=` (the type may hold a `;`) and the next `;`.
/// `None` when `code` declares no such const. Shares the `//` limit of
/// [`const_string_literals`].
fn const_named_literals(code: &str, name: &str) -> Option<Vec<String>> {
    let declaration = code
        .match_indices(&format!("const {name}"))
        .find_map(|(at, _)| {
            let after = code[at + "const ".len() + name.len()..].chars().next();
            (!after.is_some_and(is_identifier_char)).then_some(at)
        })?;
    let rest = &code[declaration..];
    let value = &rest[rest.find('=')? + 1..];
    let value = &value[..value.find(';').unwrap_or(value.len())];
    Some(
        value
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect(),
    )
}

/// The suffixes in each `format!("{base}...")` literal in `code`, in order: the
/// text between the `{base}` placeholder and the closing quote, raw. A suffix
/// bound through another placeholder (`"{base}{SUFFIX}"`) is kept as written,
/// so a pinned list of fixed suffixes cannot match it. A literal that starts
/// from another placeholder is not bound to `base` and is not collected.
/// Shares the `//` limit of [`const_string_literals`].
fn format_suffix_literals(code: &str, base: &str) -> Vec<String> {
    let opening = format!("format!(\"{{{base}}}");
    code.match_indices(&opening)
        .filter_map(|(at, _)| {
            let rest = &code[at + opening.len()..];
            Some(rest[..rest.find('"')?].to_string())
        })
        .collect()
}

/// The field names in `struct name`'s body in `code`: each line at brace
/// depth 1 spelled `[pub[(…)]] field: Type`.
fn struct_fields(code: &str, name: &str) -> Option<Vec<String>> {
    let open = code.find(&format!("struct {name} {{"))?;
    let mut fields = Vec::new();
    let mut depth = 0usize;
    for line in code[open..].lines() {
        if depth == 1 {
            let mut field = line.trim();
            if let Some(rest) = field.strip_prefix("pub") {
                field = rest.trim_start();
                if field.starts_with('(') {
                    field = field
                        .split_once(')')
                        .map_or("", |(_, rest)| rest)
                        .trim_start();
                }
            }
            let ident: String = field
                .chars()
                .take_while(|&c| is_identifier_char(c))
                .collect();
            let rest = &field[ident.len()..];
            if !ident.is_empty() && rest.starts_with(':') && !rest.starts_with("::") {
                fields.push(ident);
            }
        }
        for c in line.chars() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(fields);
                    }
                }
                _ => {}
            }
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

// The player owns reflection cost, never style: the only reflection name
// among the settings keys, option slots and `PlayerOptions` fields is the
// cost tier, so no option can name the author's `render.reflections.style`.
#[test]
fn player_options_name_no_reflection_style() {
    let root = workspace_root();
    let mut names = Vec::new();
    for path in rust_files(&root.join(OPTIONS_DIR)) {
        // Test files spell rejected names on purpose.
        if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with("tests.rs"))
        {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("options source reads");
        let file = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        for literal in const_string_literals(&strip_line_comments(&source)) {
            names.push((file.clone(), literal));
        }
    }
    for (file, struct_name) in PLAYER_OPTIONS_STRUCTS {
        let source = std::fs::read_to_string(root.join(file)).expect("options source reads");
        let fields = struct_fields(&strip_line_comments(&source), struct_name)
            .unwrap_or_else(|| panic!("{file} declares no `struct {struct_name}`"));
        assert!(
            fields.len() >= 5,
            "`{struct_name}` parsed with too few fields: {fields:?}"
        );
        names.extend(
            fields
                .into_iter()
                .map(|field| (format!("{struct_name} field"), field)),
        );
    }
    let catalog = std::fs::read_to_string(root.join(ENGINE_STATE_CATALOG)).expect("catalog reads");
    for line in strip_line_comments(&catalog).lines() {
        if let Some(wire) = line.trim().strip_prefix("wire_name: \"") {
            let wire = wire.split('"').next().unwrap_or_default();
            if wire.starts_with("options.") {
                names.push((ENGINE_STATE_CATALOG.to_string(), wire.to_string()));
            }
        }
    }

    // The scan reads what it claims: the tier is found as settings key, slot,
    // catalog wire name and field.
    for (source, expected) in [
        ("crates/postretro/src/options/mod.rs", "reflection_quality"),
        (
            "crates/postretro/src/options/bridge/top_level.rs",
            "options.reflectionQuality",
        ),
        (ENGINE_STATE_CATALOG, "options.reflectionQuality"),
        ("PlayerOptions field", "reflection_quality"),
    ] {
        assert!(
            names
                .iter()
                .any(|(file, name)| file == source && name == expected),
            "the scan found no `{expected}` in {source}; point this gate at the option's new home"
        );
    }

    let violations: Vec<String> = names
        .iter()
        .filter(|(_, name)| {
            names_reflection(name) && !ALLOWED_REFLECTION_OPTION_NAMES.contains(&name.as_str())
        })
        .map(|(file, name)| format!("{file}: `{name}`"))
        .collect();
    assert!(
        violations.is_empty(),
        "a player option names a reflection setting other than the cost tier; the \
         reflection style is the author's `render.reflections.style`:\n{}",
        violations.join("\n")
    );
}

// Reflections take no world-material input: no material, material property,
// texture prefix or surface-map suffix rule names one, and the FGD's only
// reflection authoring is `reflection_volume`'s probe box.
#[test]
fn no_world_material_fgd_key_or_texture_rule_takes_a_reflection_input() {
    let root = workspace_root();
    let mut violations = Vec::new();
    for file in MATERIAL_RULE_FILES {
        let source = std::fs::read_to_string(root.join(file))
            .unwrap_or_else(|_| panic!("{file} moved; point this gate at the material rules"));
        let code = strip_line_comments(&source).to_lowercase();
        for word in REFLECTION_WORDS {
            for (at, _) in code.match_indices(word) {
                // Only at a word start: `ssr` inside `glassrender` is not one.
                let before = code[..at].chars().next_back();
                if before.is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
                    continue;
                }
                let line = code[..at].matches('\n').count() + 1;
                violations.push(format!("{file}:{line}: `{word}`"));
            }
        }
    }

    // The material prefix table and the sibling suffixes are pinned exactly: a
    // new entry is a new material input, which the word scan cannot see.
    for (file, name, expected) in [
        (
            MATERIAL_PREFIXES_FILE,
            MATERIAL_PREFIXES_CONST,
            MATERIAL_PREFIX_NAMES,
        ),
        (
            BUNDLE_SUFFIXES_FILE,
            BUNDLE_SUFFIXES_CONST,
            BUNDLE_SUFFIX_NAMES,
        ),
    ] {
        let source = std::fs::read_to_string(root.join(file))
            .unwrap_or_else(|_| panic!("{file} moved; point this gate at the material rules"));
        let found = const_named_literals(&strip_line_comments(&source), name)
            .unwrap_or_else(|| panic!("{file} declares no `const {name}`; update this gate"));
        if found != expected {
            violations.push(format!(
                "{file}: `{name}` is {found:?}, not {expected:?}; a new suffix or prefix is \
                 a new material input and needs a deliberate edit to this gate (owner review)"
            ));
        }
    }

    // `BUNDLE_SUFFIXES` only chooses the base name; the sibling maps are bound
    // by the `format!("{resolved_base}_X")` literals beside it, so pin those.
    {
        let file = BUNDLE_SUFFIXES_FILE;
        let source = std::fs::read_to_string(root.join(file))
            .unwrap_or_else(|_| panic!("{file} moved; point this gate at the material rules"));
        let found = format_suffix_literals(&strip_line_comments(&source), BUNDLE_BINDING_BASE);
        if found != BUNDLE_BINDING_SUFFIX_NAMES {
            violations.push(format!(
                "{file}: the `format!(\"{{{BUNDLE_BINDING_BASE}}}...\")` lookups bind {found:?}, \
                 not {BUNDLE_BINDING_SUFFIX_NAMES:?}; a new suffix or prefix is a new sibling \
                 map, a new material input, and needs a deliberate edit to this gate (owner review)"
            ));
        }
    }

    // One class per FGD line that opens with `@` (the FGD uses no other `@`
    // directive), so a class the parser skips fails here instead of escaping
    // the key scan below.
    let fgd = std::fs::read_to_string(root.join(FGD_FILE)).expect("FGD reads");
    let classes = fgd_class_names(&fgd);
    let declarations = fgd
        .lines()
        .filter(|line| line.trim_start().starts_with('@'))
        .count();
    assert!(
        classes.len() == declarations && classes.iter().any(|class| class == "worldspawn"),
        "{FGD_FILE} has {declarations} `@` declarations but {} classes parsed: {classes:?}",
        classes.len()
    );
    assert!(
        classes.iter().any(|class| class == REFLECTION_VOLUME_CLASS),
        "{FGD_FILE} no longer declares `{REFLECTION_VOLUME_CLASS}`; update this gate"
    );
    for class in &classes {
        let keys = fgd_class_keys(&fgd, class)
            .unwrap_or_else(|| panic!("{FGD_FILE} declares no `{class}` body"));
        if class == REFLECTION_VOLUME_CLASS {
            if keys != REFLECTION_VOLUME_KEYS {
                violations.push(format!(
                    "{FGD_FILE}: `{class}` keys are {keys:?}, not {REFLECTION_VOLUME_KEYS:?}"
                ));
            }
            continue;
        }
        if names_reflection(class) {
            violations.push(format!("{FGD_FILE}: class `{class}`"));
        }
        for key in keys.iter().filter(|key| names_reflection(key)) {
            violations.push(format!("{FGD_FILE}: `{class}` key `{key}`"));
        }
    }
    assert!(
        violations.is_empty(),
        "a world material, texture rule or FGD key takes a reflection input:\n{}",
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
@BaseClass = Spawn
[
    // A comment's brackets need not balance: phase is [0, 1) of the cycle.
    tag(string) : "Tag"
]
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
    assert_eq!(fgd_class_keys(fgd, "Spawn"), Some(vec!["tag".to_string()]));
}

#[test]
fn reflection_gate_reads_every_spelling_of_a_reflection_name() {
    for name in [
        "reflection_style",
        "reflectionStyle",
        "options.reflections.style",
        "reflection_smooth",
        "pixel_reflections",
        "ssrQuality",
        "mirror_mode",
    ] {
        assert!(names_reflection(name), "{name}");
    }
    for name in ["fog_quality", "options.surfaceDepthQuality", "glassRender"] {
        assert!(!names_reflection(name), "{name}");
    }
    assert_eq!(
        name_words("options.reflectionQuality"),
        ["options", "reflection", "quality"]
    );

    let code = "pub(crate) const A: &str = \"options.a\";\n\
                pub(super) const KEYS: [&str; 2] = [\n    \"b_key\",\n    \"c_key\",\n];\n\
                const N: f32 = 1.0;\n\
                let s = \"not a const\";\n";
    assert_eq!(const_string_literals(code), ["options.a", "b_key", "c_key"]);

    let code = "pub struct Opts {\n    pub a: f32,\n    pub(crate) b: Mode,\n    \
                c: Option<[u8; 16]>,\n    #[serde(default)]\n    d: crate::X,\n}\n\
                impl Opts { fn e(&self) {} }\n";
    assert_eq!(
        struct_fields(code, "Opts"),
        Some(vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string()
        ])
    );

    let fgd = r#"@SolidClass = worldspawn : "World" [ ]
@PointClass base(Light)
    color(1 2 3)
    = light_spot : "Spot = cone"
[
]
@PointClass
    model({ "path": "lamp.obj", "scale": [radius, radius, radius] })
    = fog_lamp : "Lamp"
[
]
// @SolidClass = commented : "no"
"#;
    assert_eq!(
        fgd_class_names(fgd),
        ["worldspawn", "light_spot", "fog_lamp"]
    );
}

#[test]
fn const_named_literals_reads_one_constants_string_values() {
    let code = strip_line_comments(
        "pub(crate) const SUFFIXES: [&str; 2] = [\"\", \"_s\"]; // \"_x\"\n\
         const OTHER: [&str; 1] = [\"o\"];\n\
         pub const TABLE: [(&str, M); 2] = [(\"a\", M::A), (\"b\", M::B)];\n",
    );
    assert_eq!(
        const_named_literals(&code, "SUFFIXES"),
        Some(vec![String::new(), "_s".to_string()])
    );
    assert_eq!(
        const_named_literals(&code, "TABLE"),
        Some(vec!["a".to_string(), "b".to_string()])
    );
    assert_eq!(const_named_literals(&code, "SUFF"), None);
}

#[test]
fn format_suffix_literals_reads_the_suffixes_bound_to_one_base() {
    let code = strip_line_comments(
        "let any = name_to_path.contains_key(&format!(\"{normalized}{suffix}\"));\n\
         specular: name_to_path.get(&format!(\"{resolved_base}_s\")).cloned(),\n\
         normal: name_to_path.get(&format!(\"{resolved_base}_n\")).cloned(),\n\
         rough: name_to_path.get(&format!(\"{resolved_base}_r\")).cloned(),\n\
         both: name_to_path.get(&format!(\"{resolved_base}{other}\")).cloned(),\n\
         // name_to_path.get(&format!(\"{resolved_base}_x\"))\n\
         other: name_to_path.get(&format!(\"{normalized}_h\")).cloned(),\n",
    );
    // A suffix bound through a placeholder is kept raw, so it cannot pass
    // as one of the pinned fixed suffixes.
    assert_eq!(
        format_suffix_literals(&code, "resolved_base"),
        ["_s", "_n", "_r", "{other}"]
    );
    assert_eq!(
        format_suffix_literals(&code, "normalized"),
        ["{suffix}", "_h"]
    );
    assert!(format_suffix_literals(&code, "missing").is_empty());
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
