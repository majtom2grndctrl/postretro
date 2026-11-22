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

/// The parser and the help name the same flags, apart from `-h`/`--help`
/// and `foreign` (another program's flags the help mentions).
fn assert_help_matches_parser(source: &str, marker: &str, help: &str, foreign: &[&str]) {
    let exempt =
        |flag: &String| flag != "-h" && flag != "--help" && !foreign.contains(&flag.as_str());
    let parsed: std::collections::BTreeSet<_> = parser_flags(source, marker)
        .into_iter()
        .filter(exempt)
        .collect();
    let listed: std::collections::BTreeSet<_> =
        help_flags(help).into_iter().filter(exempt).collect();
    assert!(!parsed.is_empty(), "no flags found in `{marker}`");
    let undocumented: Vec<_> = parsed.difference(&listed).collect();
    let unparsed: Vec<_> = listed.difference(&parsed).collect();
    assert!(
        undocumented.is_empty() && unparsed.is_empty(),
        "help and parser disagree: parsed but missing from help {undocumented:?}; \
         in help but never parsed {unparsed:?}"
    );
}

#[test]
fn help_lists_every_flag_the_parser_accepts() {
    assert_help_matches_parser(
        include_str!("main.rs"),
        "fn parse_args_from",
        &help_text(),
        &[],
    );
}
