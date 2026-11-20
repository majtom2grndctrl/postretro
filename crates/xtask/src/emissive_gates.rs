// Repository grep gates for emissive volumes: only the compiler opens
// `materials.toml` or reads the authored glow range keys, no emissive
// primitive's arguments carry that range, the photosensitivity source floor
// names `emissive_volume` as a built clamp source, and no shader reads `_e`
// alpha. Run by preflight's `cargo test`, beside the scripting surface gates.
// See: context/lib/rendering_pipeline.md §7.3, §7.8 · context/lib/scripting.md §10.2a · context/lib/build_pipeline.md §Compiler pipeline

use std::path::{Path, PathBuf};

use crate::surface_gates::{is_identifier_char, relative, workspace_root};

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    Ts,
    Luau,
}

impl Lang {
    fn of(file: &str) -> Self {
        if file.ends_with(".luau") {
            Self::Luau
        } else {
            Self::Ts
        }
    }
}

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

fn line_at(text: &str, at: usize) -> usize {
    text[..at].matches('\n').count() + 1
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

// ---------------------------------------------------------------------------
// Rust: comments dropped, string and char literals lifted out.
// ---------------------------------------------------------------------------

/// Rust source split into code and literals. `code` keeps every newline, so
/// line numbers hold; each string literal is replaced by `""`, its contents
/// listed in `literals` at the offset of that placeholder in `code`.
struct RustText {
    code: String,
    literals: Vec<(usize, String)>,
}

fn lex_rust(source: &str) -> RustText {
    let chars: Vec<char> = source.chars().collect();
    let mut code = String::with_capacity(source.len());
    let mut literals = Vec::new();
    let mut i = 0;
    let previous_is_identifier =
        |i: usize| i > 0 && chars.get(i - 1).is_some_and(|&c| is_identifier_char(c));
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && next == Some('*') {
            let mut depth = 0;
            while i < chars.len() {
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
                    if chars[i] == '\n' {
                        code.push('\n');
                    }
                    i += 1;
                }
            }
            continue;
        }
        // Raw strings: `r"…"`, `r#"…"#`, `br#"…"#`.
        let raw_start = match (c, next) {
            ('r', Some('"' | '#')) if !previous_is_identifier(i) => Some(i + 1),
            ('b', Some('r')) if !previous_is_identifier(i) => Some(i + 2),
            _ => None,
        };
        if let Some(mut j) = raw_start {
            let mut hashes = 0;
            while chars.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if chars.get(j) == Some(&'"') {
                let start = j + 1;
                let mut end = start;
                loop {
                    if end >= chars.len() {
                        break;
                    }
                    if chars[end] == '"'
                        && (0..hashes).all(|k| chars.get(end + 1 + k) == Some(&'#'))
                    {
                        break;
                    }
                    end += 1;
                }
                let literal: String = chars[start..end.min(chars.len())].iter().collect();
                literals.push((code.len(), literal.clone()));
                code.push_str("\"\"");
                code.extend(literal.chars().filter(|&c| c == '\n'));
                i = end + 1 + hashes;
                continue;
            }
        }
        if c == '"' {
            let mut literal = String::new();
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '"' {
                if chars[j] == '\\' {
                    literal.push(chars[j]);
                    j += 1;
                }
                if let Some(&escaped) = chars.get(j) {
                    literal.push(escaped);
                }
                j += 1;
            }
            literals.push((code.len(), literal.clone()));
            code.push_str("\"\"");
            code.extend(literal.chars().filter(|&c| c == '\n'));
            i = j + 1;
            continue;
        }
        if c == '\'' {
            // A char literal (`'a'`, `'\n'`, `'\''`); otherwise a lifetime.
            if next == Some('\\') {
                let mut j = i + 2;
                while j < chars.len() && chars[j] != '\'' {
                    j += if chars[j] == '\\' { 2 } else { 1 };
                }
                code.push_str("' '");
                i = j + 1;
                continue;
            }
            if chars.get(i + 2) == Some(&'\'') {
                code.push_str("' '");
                i += 3;
                continue;
            }
        }
        code.push(c);
        i += 1;
    }
    RustText { code, literals }
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

/// The byte span of `struct name`'s body in `code`, brackets included, and
/// whether it is a tuple struct.
fn struct_body(code: &str, name: &str) -> Option<(usize, usize, bool)> {
    let at = identifier_at(code, name)
        .into_iter()
        .find(|&at| code[..at].trim_end().ends_with("struct"))?;
    let rest = &code[at + name.len()..];
    let open = at + name.len() + rest.find(['{', '('])?;
    let close = matching_close(code, open)?;
    Some((open, close, code[open..].starts_with('(')))
}

/// The field names of a brace struct body (between its braces): each
/// identifier followed by `:` at depth 0, attributes skipped.
fn rust_field_names(body: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut depth = 0i32;
    let mut item = String::new();
    let mut flush = |item: &mut String| {
        let mut text = item.trim();
        // Attributes come first; `pub(crate)` and `pub` next.
        while let Some(rest) = text.strip_prefix('#') {
            let Some(close) = rest.find(']') else { break };
            text = rest[close + 1..].trim_start();
        }
        if let Some(rest) = text.strip_prefix("pub") {
            text = rest.trim_start();
            if let Some(rest) = text.strip_prefix('(') {
                text = rest
                    .split_once(')')
                    .map_or("", |(_, rest)| rest)
                    .trim_start();
            }
        }
        let ident: String = text
            .chars()
            .take_while(|&c| is_identifier_char(c))
            .collect();
        let after = text[ident.len()..].trim_start();
        if !ident.is_empty() && after.starts_with(':') && !after.starts_with("::") {
            fields.push(ident);
        }
        item.clear();
    };
    for c in body.chars() {
        match c {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth -= 1,
            ',' if depth == 0 => {
                flush(&mut item);
                continue;
            }
            _ => {}
        }
        item.push(c);
    }
    flush(&mut item);
    fields
}

/// Range-carrying names in `struct name` of `text`: its fields, and serde
/// renames or other attribute literals inside its body. `None` when the file
/// declares no such struct. For a tuple struct, the identifiers inside it are
/// returned as `carried` for the caller to resolve.
struct StructScan {
    violations: Vec<String>,
    carried: Vec<String>,
    tuple: bool,
}

fn scan_rust_struct(text: &RustText, name: &str) -> Option<StructScan> {
    let (open, close, tuple) = struct_body(&text.code, name)?;
    let body = &text.code[open + 1..close];
    let mut violations = Vec::new();
    if !tuple {
        for field in rust_field_names(body) {
            if names_range(&field) {
                violations.push(format!("field `{field}`"));
            }
        }
    }
    for (at, literal) in &text.literals {
        if (open..close).contains(at) && names_range(literal) {
            violations.push(format!("attribute literal \"{literal}\""));
        }
    }
    let carried = if tuple {
        let mut idents = Vec::new();
        let mut ident = String::new();
        for c in body.chars().chain(std::iter::once(' ')) {
            if is_identifier_char(c) {
                ident.push(c);
            } else if !ident.is_empty() {
                idents.push(std::mem::take(&mut ident));
            }
        }
        idents
    } else {
        Vec::new()
    };
    Some(StructScan {
        violations,
        carried,
        tuple,
    })
}

// ---------------------------------------------------------------------------
// TypeScript and Luau: comments dropped, then object and table shapes read.
// ---------------------------------------------------------------------------

/// `text` with its comments blanked to spaces (newlines kept): `//` and
/// `/* */` in TypeScript, `--` and `--[[ ]]` in Luau. Strings stay.
fn strip_script_comments(text: &str, lang: Lang) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut quote: Option<char> = None;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if let Some(q) = quote {
            out.push(c);
            if c == '\\' {
                if let Some(escaped) = next {
                    out.push(escaped);
                }
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        let (line, block_open, block_close) = match lang {
            Lang::Ts => ("//", "/*", "*/"),
            Lang::Luau => ("--", "--[[", "]]"),
        };
        let at = |needle: &str| {
            chars[i..]
                .iter()
                .take(needle.len())
                .copied()
                .eq(needle.chars())
        };
        if at(block_open) {
            let mut j = i + block_open.len();
            while j < chars.len()
                && !chars[j..]
                    .iter()
                    .take(block_close.len())
                    .copied()
                    .eq(block_close.chars())
            {
                out.push(if chars[j] == '\n' { '\n' } else { ' ' });
                j += 1;
            }
            i = j + block_close.len();
            continue;
        }
        if at(line) {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '"' || c == '\'' || (c == '`' && lang == Lang::Ts) {
            quote = Some(c);
        }
        out.push(c);
        i += 1;
    }
    out
}

/// The top-level members of a brace body (text between `{` and `}`), as
/// (key, value) pairs: `key: value`, `key?: value`, `key = value`,
/// `["key"] = value` and shorthand `key`. Spreads and methods are skipped.
/// Members split at a depth-0 `,`, `;` or newline.
fn brace_members(body: &str) -> Vec<(String, String)> {
    let mut members = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let mut push = |item: &str| {
        let mut text = item.trim();
        if text.is_empty() || text.starts_with("...") {
            return;
        }
        text = text.strip_prefix("readonly ").unwrap_or(text).trim_start();
        let (key, rest) = if let Some(quoted) = text.strip_prefix('[') {
            let quoted = quoted.trim_start();
            let Some(q) = quoted.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                return;
            };
            let Some((key, rest)) = quoted[1..].split_once(q) else {
                return;
            };
            (
                key.to_string(),
                rest.trim_start().strip_prefix(']').unwrap_or(rest),
            )
        } else if let Some(q) = text.chars().next().filter(|c| *c == '"' || *c == '\'') {
            let Some((key, rest)) = text[1..].split_once(q) else {
                return;
            };
            (key.to_string(), rest)
        } else {
            let key: String = text
                .chars()
                .take_while(|&c| is_identifier_char(c))
                .collect();
            if key.is_empty() {
                return;
            }
            let rest = &text[key.len()..];
            (key, rest)
        };
        let rest = rest.trim_start();
        let value = if let Some(value) = rest.strip_prefix("?:") {
            value
        } else if let Some(value) = rest.strip_prefix(':') {
            value
        } else if let Some(value) = rest.strip_prefix('=').filter(|v| !v.starts_with('=')) {
            value
        } else if rest.is_empty() {
            ""
        } else {
            return;
        };
        members.push((key, value.trim().to_string()));
    };
    for (offset, c) in body.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' | ';' | '\n' if depth == 0 => {
                push(&body[start..offset]);
                start = offset + 1;
            }
            _ => {}
        }
    }
    push(&body[start..]);
    members
}

/// The body of `type name = { … }` in `text`, braces excluded.
fn type_body<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    identifier_at(text, name).into_iter().find_map(|at| {
        if !text[..at].trim_end().ends_with("type") {
            return None;
        }
        let rest = text[at + name.len()..].trim_start().strip_prefix('=')?;
        let open = text.len() - rest.trim_start().len();
        if !text[open..].starts_with('{') {
            return None;
        }
        let close = matching_close(text, open)?;
        Some(&text[open + 1..close])
    })
}

/// Identifiers in a type expression that may name a declared shape.
fn type_names(expression: &str) -> Vec<String> {
    const NOT_SHAPES: &[&str] = &[
        "null",
        "nil",
        "undefined",
        "number",
        "string",
        "boolean",
        "readonly",
        "import",
        "postretro",
    ];
    let mut names = Vec::new();
    let mut ident = String::new();
    for c in expression.chars().chain(std::iter::once(' ')) {
        if is_identifier_char(c) {
            ident.push(c);
        } else if !ident.is_empty() {
            let name = std::mem::take(&mut ident);
            if !NOT_SHAPES.contains(&name.as_str()) && !name.chars().all(|c| c.is_ascii_digit()) {
                names.push(name);
            }
        }
    }
    names
}

/// The keys an `args` type expression carries: an inline `{ … }`, or the
/// named shapes it references, resolved in `texts` (the step's own file
/// first). `Err` names a shape the gate cannot resolve.
fn args_type_keys(expression: &str, texts: &[&str]) -> Result<Vec<String>, String> {
    let expression = expression.trim();
    if expression.starts_with('{') {
        let close = matching_close(expression, 0).ok_or("unclosed `{`")?;
        return Ok(brace_members(&expression[1..close])
            .into_iter()
            .map(|(key, _)| key)
            .collect());
    }
    let mut keys = Vec::new();
    let names = type_names(expression);
    if names.is_empty() {
        return Err(format!("`args: {expression}` names no shape"));
    }
    for name in names {
        let body = texts
            .iter()
            .find_map(|text| type_body(text, &name))
            .ok_or_else(|| format!("cannot resolve `{name}`; update this gate"))?;
        keys.extend(brace_members(body).into_iter().map(|(key, _)| key));
    }
    Ok(keys)
}

/// The expression following each `args:` (TypeScript) or `args =` (Luau)
/// member in `text`, read to its depth-0 end.
fn args_expressions(text: &str, lang: Lang) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for at in identifier_at(text, "args") {
        let rest = text[at + "args".len()..].trim_start();
        let value = match lang {
            Lang::Ts => rest.strip_prefix(':'),
            Lang::Luau => rest.strip_prefix('=').filter(|v| !v.starts_with('=')),
        };
        let Some(value) = value else { continue };
        let value = value.trim_start();
        let mut depth = 0i32;
        let mut end = value.len();
        for (offset, c) in value.char_indices() {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' if depth == 0 => {
                    end = offset;
                    break;
                }
                ')' | ']' | '}' => depth -= 1,
                ',' | ';' | '\n' if depth == 0 => {
                    end = offset;
                    break;
                }
                _ => {}
            }
        }
        found.push((at, value[..end].trim().to_string()));
    }
    found
}

/// The keys of every table or object literal a function's `return`
/// statements build, for `function name(` in `text`: the function's text runs
/// to the next top-level function declaration.
fn builder_return_keys(text: &str, name: &str) -> Option<Vec<String>> {
    let start = identifier_at(text, name)
        .into_iter()
        .find(|&at| text[..at].trim_end().ends_with("function"))?;
    let end = text[start..]
        .match_indices("\nfunction ")
        .chain(text[start..].match_indices("\nlocal function "))
        .chain(text[start..].match_indices("\nexport function "))
        .map(|(offset, _)| start + offset)
        .min()
        .unwrap_or(text.len());
    let region = &text[start..end];
    let mut keys = Vec::new();
    for at in identifier_at(region, "return") {
        let mut offset = at + "return".len();
        let mut consumed = false;
        while offset < region.len() {
            let c = region[offset..].chars().next().unwrap();
            if c == '{' {
                let close = matching_close(region, offset)?;
                keys.extend(
                    brace_members(&region[offset + 1..close])
                        .into_iter()
                        .map(|(key, _)| key),
                );
                offset = close + 1;
                consumed = true;
                continue;
            }
            if c == '(' || c == '[' {
                offset = matching_close(region, offset)? + 1;
                consumed = true;
                continue;
            }
            if c == ';' || (c == '\n' && consumed) {
                break;
            }
            if !c.is_whitespace() {
                consumed = true;
            }
            offset += c.len_utf8();
        }
    }
    Some(keys)
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
                        line_at(&text.code, *at)
                    ));
                }
            }
        }
    }
    violations
}

/// The authored range keys read outside the compiler: as a Rust identifier
/// or an exact string key in another crate, or as a quoted key in a script.
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
                        line_at(&text.code, at)
                    ));
                }
                for (at, literal) in &text.literals {
                    if literal.trim() == *key {
                        violations.push(format!(
                            "{}:{}: \"{key}\"",
                            relative(root, &path),
                            line_at(&text.code, *at)
                        ));
                    }
                }
            }
        }
    }
    for script_root in SCRIPT_ROOTS {
        for path in files_with_extensions(&root.join(script_root), SCRIPT_EXTENSIONS) {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for key in RANGE_KEYS {
                for quote in ['"', '\''] {
                    let needle = format!("{quote}{key}{quote}");
                    for (at, _) in text.match_indices(&needle) {
                        violations.push(format!(
                            "{}:{}: {needle}",
                            relative(root, &path),
                            line_at(&text, at)
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
            let line = line_at(&text, at);
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

/// WGSL with `//` and `/* */` comments blanked.
fn strip_wgsl_comments(text: &str) -> String {
    strip_script_comments(text, Lang::Ts)
}

/// Each sample of an emissive texture in `wgsl` that does not end `.rgb`, as
/// (line, binding); plus how many samples were read. An emissive texture is a
/// `var` whose name names `emissive` and whose type is a texture.
fn emissive_alpha_reads(wgsl: &str) -> (Vec<(usize, String)>, usize) {
    let code = strip_wgsl_comments(wgsl);
    let mut bindings = Vec::new();
    for at in identifier_at(&code, "var") {
        let rest = &code[at + 3..];
        let rest = rest
            .trim_start()
            .strip_prefix('<')
            .and_then(|r| r.split_once('>').map(|(_, r)| r))
            .unwrap_or(rest)
            .trim_start();
        let name: String = rest
            .chars()
            .take_while(|&c| is_identifier_char(c))
            .collect();
        let ty = rest[name.len()..]
            .trim_start()
            .strip_prefix(':')
            .map(str::trim_start);
        if name.to_lowercase().contains("emissive")
            && ty.is_some_and(|ty| ty.starts_with("texture_"))
        {
            bindings.push(name);
        }
    }
    let mut reads = Vec::new();
    let mut samples = 0;
    for binding in bindings {
        for at in identifier_at(&code, &binding) {
            // The declaration itself.
            if code[..at].trim_end().ends_with('>') || code[..at].trim_end().ends_with("var") {
                continue;
            }
            samples += 1;
            let mut depth = 0i32;
            let open = code[..at]
                .char_indices()
                .rev()
                .find_map(|(offset, c)| match c {
                    ')' => {
                        depth += 1;
                        None
                    }
                    '(' if depth == 0 => Some(offset),
                    '(' => {
                        depth -= 1;
                        None
                    }
                    ';' | '{' | '}' if depth == 0 => Some(usize::MAX),
                    _ => None,
                });
            let ends_rgb = open
                .filter(|&open| open != usize::MAX)
                .and_then(|open| matching_close(&code, open))
                .is_some_and(|close| {
                    let after = code[close + 1..].trim_start();
                    after
                        .strip_prefix(".rgb")
                        .is_some_and(|rest| !rest.chars().next().is_some_and(is_identifier_char))
                });
            if !ends_rgb {
                reads.push((line_at(&code, at), binding.clone()));
            }
        }
    }
    (reads, samples)
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

#[test]
fn rust_lexer_lifts_literals_and_drops_comments() {
    let source = "// emissive_min in a comment\n\
                  /* nested /* emissive_max */ still */\n\
                  let a = \"emissive_min\"; let b = r#\"materials.toml\"#;\n\
                  fn f<'a>(x: &'a str) -> char { 'x' }\n\
                  let doc = \"the `emissive_max` key\";\n";
    let text = lex_rust(source);
    assert!(identifier_at(&text.code, "emissive_min").is_empty());
    assert!(identifier_at(&text.code, "emissive_max").is_empty());
    let literals: Vec<&str> = text.literals.iter().map(|(_, l)| l.as_str()).collect();
    assert_eq!(
        literals,
        ["emissive_min", "materials.toml", "the `emissive_max` key"]
    );
    assert_eq!(line_at(&text.code, text.literals[1].0), 3);
    assert!(text.code.contains("fn f<'a>(x: &'a str)"));
}

#[test]
fn rust_struct_scan_reads_fields_renames_and_tuple_wrappers() {
    let source = "#[derive(Deserialize)]\n\
                  #[serde(rename_all = \"camelCase\")]\n\
                  pub(crate) struct SetArgs {\n\
                      pub(crate) brightness: f32,\n\
                      #[serde(rename = \"max\")]\n\
                      pub(crate) ceiling: Option<f32>,\n\
                      pub(crate) min_level: f32,\n\
                  }\n\
                  pub(crate) struct WrapArgs(pub(crate) Option<Curve>);\n";
    let text = lex_rust(source);
    let scan = scan_rust_struct(&text, "SetArgs").expect("struct found");
    assert_eq!(
        scan.violations,
        ["field `min_level`", "attribute literal \"max\""]
    );
    let wrapper = scan_rust_struct(&text, "WrapArgs").expect("tuple struct found");
    assert!(wrapper.tuple);
    assert!(wrapper.carried.contains(&"Curve".to_string()));
}

#[test]
fn script_shape_readers_check_emitted_args_not_method_options() {
    let ts = "function anim(p: number, b: number[], c?: number): Anim {\n\
              return c === undefined ? { periodMs: p, brightness: b } : { periodMs: p, playCount: c, brightness: b };\n\
              }\n\
              export function wrap() {\n\
                return {\n\
                  pulse(opts: { min: number; max: number }) { return [{ id, primitive: \"x\", args: anim(1, curve(opts.min, opts.max)) }]; },\n\
                  hold() { return [{ id, primitive: \"y\", args: { brightness: 1, max: 2 } }]; },\n\
                };\n\
              }\n";
    let text = strip_script_comments(ts, Lang::Ts);
    let sites = args_expressions(&text, Lang::Ts);
    assert_eq!(sites.len(), 2);
    assert_eq!(
        builder_return_keys(&text, "anim").unwrap(),
        [
            "periodMs",
            "brightness",
            "periodMs",
            "playCount",
            "brightness"
        ]
    );
    let inline = &sites[1].1;
    let close = matching_close(inline, 0).unwrap();
    let keys: Vec<String> = brace_members(&inline[1..close])
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(keys, ["brightness", "max"]);

    let luau = "-- args = { min = 0 } in a comment\n\
                local function anim(p: number): Anim\n\
                  return {\n    periodMs = p,\n    [\"brightness\"] = {},\n  }\n\
                end\n\
                local x = { args = anim(1) }\n";
    let text = strip_script_comments(luau, Lang::Luau);
    let sites = args_expressions(&text, Lang::Luau);
    assert_eq!(sites.len(), 1);
    assert_eq!(
        builder_return_keys(&text, "anim").unwrap(),
        ["periodMs", "brightness"]
    );

    let types = "export type Anim = {\n  periodMs: number;\n  playCount?: number | null;\n};\n\
                 export type Step = { id: number; args: Anim | null };\n";
    let body = type_body(types, "Step").unwrap();
    let (_, args) = brace_members(body)
        .into_iter()
        .find(|(k, _)| k == "args")
        .unwrap();
    assert_eq!(
        args_type_keys(&args, &[types]).unwrap(),
        ["periodMs", "playCount"]
    );
    assert!(args_type_keys("Missing", &[types]).is_err());
}

#[test]
fn emissive_sample_scan_flags_alpha_and_whole_texel_reads() {
    let wgsl = "@group(1) @binding(1) var emissive_texture: texture_2d<f32>;\n\
                // textureSample(emissive_texture, s, uv).a in a comment\n\
                fn f() {\n\
                  let a = sample(emissive_texture, s, uv, d(x)).rgb;\n\
                  let b = textureSample(emissive_texture, s, uv).a;\n\
                  let c = textureSample(emissive_texture, s, uv).rgba;\n\
                  let d = textureSample(emissive_texture, s, uv);\n\
                }\n";
    let (reads, samples) = emissive_alpha_reads(wgsl);
    assert_eq!(samples, 4);
    let lines: Vec<usize> = reads.iter().map(|(line, _)| *line).collect();
    assert_eq!(lines, [5, 6, 7]);
}
