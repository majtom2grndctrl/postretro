// `--help` and the argument parser name the same flags: a flag added to the
// parser without a help line, or a help line for a removed flag, fails here.

use super::help_text;

/// Flag-shaped names: `-x` or `--name`.
fn is_flag_name(token: &str) -> bool {
    let bytes = token.as_bytes();
    match bytes {
        [b'-', c] => c.is_ascii_alphabetic(),
        [b'-', b'-', first, rest @ ..] => {
            first.is_ascii_lowercase()
                && rest
                    .iter()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        }
        _ => false,
    }
}

/// Every flag-shaped string literal (`"--name"`) in the top-level item of
/// `source` that begins at `marker`, read up to that item's closing brace.
fn parser_flags(source: &str, marker: &str) -> std::collections::BTreeSet<String> {
    let start = source.find(marker).expect("parser item is present");
    let item = &source[start..];
    let item = &item[..item.find("\n}\n").expect("parser item closes")];
    item.split('"')
        .filter(|piece| is_flag_name(piece))
        .map(str::to_owned)
        .collect()
}

/// Every flag-shaped word the help text names.
fn help_flags(help: &str) -> std::collections::BTreeSet<String> {
    help.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|word| is_flag_name(word))
        .map(str::to_owned)
        .collect()
}

/// Flags that open an entry line: `-j, --jobs <N>   ...` names `-j` and
/// `--jobs`. A flag named only in another entry's prose has no line of its own.
fn entry_flags(help: &str) -> std::collections::BTreeSet<String> {
    help.lines()
        .flat_map(|line| {
            line.split_whitespace()
                .map(|word| word.trim_end_matches(','))
                .take_while(|word| is_flag_name(word))
        })
        .map(str::to_owned)
        .collect()
}

/// Every flag the parser matches has its own help entry, every entry is one
/// the parser matches, and prose names no flag the parser lacks.
#[test]
fn help_lists_every_flag_the_parser_accepts() {
    let help = help_text();
    let exempt = |flag: &String| flag != "-h" && flag != "--help";
    let parsed: std::collections::BTreeSet<_> =
        parser_flags(include_str!("main.rs"), "fn parse_args_from")
            .into_iter()
            .filter(exempt)
            .collect();
    let entries: std::collections::BTreeSet<_> =
        entry_flags(&help).into_iter().filter(exempt).collect();
    assert!(
        parsed.len() > 30,
        "only {} parser flags found",
        parsed.len()
    );
    let undocumented: Vec<_> = parsed.difference(&entries).collect();
    let unparsed: Vec<_> = entries.difference(&parsed).collect();
    assert!(
        undocumented.is_empty() && unparsed.is_empty(),
        "help and parser disagree: parsed but no help entry {undocumented:?}; \
         help entry but never parsed {unparsed:?}"
    );
    let stray: Vec<_> = help_flags(&help)
        .into_iter()
        .filter(|flag| exempt(flag) && !parsed.contains(flag))
        .collect();
    assert!(
        stray.is_empty(),
        "help prose names unparsed flags {stray:?}"
    );
}

/// An entry line names its flags; a flag left only in prose does not count.
#[test]
fn entry_flags_skip_prose_mentions() {
    let help = "OPTIONS:\n    -j, --jobs <N>   Workers\n    --release   Implies --no-cache\n";
    let flags: Vec<_> = entry_flags(help).into_iter().collect();
    assert_eq!(flags, ["--jobs", "--release", "-j"]);
}
