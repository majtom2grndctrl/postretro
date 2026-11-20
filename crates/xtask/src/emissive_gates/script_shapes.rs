// TypeScript and Luau scanning for the emissive gates: comments dropped, then
// object and table shapes read (step `args` types, emitted `args` literals).

use crate::surface_gates::is_identifier_char;

use super::{identifier_at, matching_close};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Lang {
    Ts,
    Luau,
}

impl Lang {
    pub(super) fn of(file: &str) -> Self {
        if file.ends_with(".luau") {
            Self::Luau
        } else {
            Self::Ts
        }
    }
}

/// `text` with its comments blanked to spaces (newlines kept): `//` and
/// `/* */` in TypeScript, `--` and `--[[ ]]` in Luau. Strings stay.
pub(super) fn strip_script_comments(text: &str, lang: Lang) -> String {
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

/// Byte offsets where `key` stands as an unquoted table or object key in
/// comment-stripped script `code`: a whole identifier, opening a member (after
/// `{`, `,` or `;`), followed by `=` (Luau) or `:` / `?:` (TypeScript).
/// Comparisons (`==`), paths (`::`) and reads (`x.key`) do not match.
pub(super) fn unquoted_range_keys(code: &str, key: &str) -> Vec<usize> {
    identifier_at(code, key)
        .into_iter()
        .filter(|&at| {
            let opens_member = code[..at]
                .trim_end()
                .chars()
                .next_back()
                .is_none_or(|c| matches!(c, '{' | ',' | ';'));
            let after = code[at + key.len()..].trim_start();
            let after = after.strip_prefix('?').unwrap_or(after);
            let assigned = after
                .strip_prefix('=')
                .is_some_and(|rest| !rest.starts_with('='))
                || after
                    .strip_prefix(':')
                    .is_some_and(|rest| !rest.starts_with(':'));
            opens_member && assigned
        })
        .collect()
}

/// The top-level members of a brace body (text between `{` and `}`), as
/// (key, value) pairs: `key: value`, `key?: value`, `key = value`,
/// `["key"] = value` and shorthand `key`. Spreads and methods are skipped.
/// Members split at a depth-0 `,`, `;` or newline.
pub(super) fn brace_members(body: &str) -> Vec<(String, String)> {
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
pub(super) fn type_body<'a>(text: &'a str, name: &str) -> Option<&'a str> {
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
pub(super) fn type_names(expression: &str) -> Vec<String> {
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
pub(super) fn args_type_keys(expression: &str, texts: &[&str]) -> Result<Vec<String>, String> {
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
pub(super) fn args_expressions(text: &str, lang: Lang) -> Vec<(usize, String)> {
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
pub(super) fn builder_return_keys(text: &str, name: &str) -> Option<Vec<String>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquoted_range_keys_match_table_and_object_members_only() {
        let luau = strip_script_comments(
            "-- { emissive_min = 1 } in prose\nlocal t = { id = 1, emissive_min = 0.5 }\n\
             local u = { emissive_max=2 }\nif x.emissive_min == 1 then end\n",
            Lang::Luau,
        );
        assert_eq!(unquoted_range_keys(&luau, "emissive_min").len(), 1);
        assert_eq!(unquoted_range_keys(&luau, "emissive_max").len(), 1);
        let ts = strip_script_comments(
            "// { emissive_min: 1 }\nconst o = { a: 1, emissive_min: 0.5 };\n\
             type T = { emissive_max?: number };\nconst v = o.emissive_min;\n",
            Lang::Ts,
        );
        assert_eq!(unquoted_range_keys(&ts, "emissive_min").len(), 1);
        assert_eq!(unquoted_range_keys(&ts, "emissive_max").len(), 1);
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
}
