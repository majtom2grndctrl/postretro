// Rust source scanning for the emissive gates: a comment-dropping lexer that
// lifts string literals out, and a struct field scan.

use crate::surface_gates::{is_identifier_char, line_of};

use super::{identifier_at, matching_close, names_range};

/// Rust source split into code and literals. `code` keeps every newline, so
/// line numbers hold; each string literal is replaced by `""`, its contents
/// listed in `literals` at the offset of that placeholder in `code`.
pub(super) struct RustText {
    pub(super) code: String,
    pub(super) literals: Vec<(usize, String)>,
}

pub(super) fn lex_rust(source: &str) -> RustText {
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

/// The byte span of `struct name`'s body in `code`, brackets included, and
/// whether it is a tuple struct.
pub(super) fn struct_body(code: &str, name: &str) -> Option<(usize, usize, bool)> {
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
pub(super) fn rust_field_names(body: &str) -> Vec<String> {
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
pub(super) struct StructScan {
    pub(super) violations: Vec<String>,
    pub(super) carried: Vec<String>,
    pub(super) tuple: bool,
}

pub(super) fn scan_rust_struct(text: &RustText, name: &str) -> Option<StructScan> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(line_of(&text.code, text.literals[1].0), 3);
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
}
