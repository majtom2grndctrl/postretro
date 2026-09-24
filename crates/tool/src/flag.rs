//! One shared match for a tool flag, in split (`--flag value`) or equals
//! (`--flag=value`) form.
//!
//! Every tool flag goes through here because an unmatched form is not an
//! error everywhere: `run` forwards unrecognized tokens to the engine, so a
//! missed `--project=<dir>` would silently launch the working directory's
//! project instead of the named one.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging (§The project marker)

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};

/// A token recognized as naming a flag, however it was spelled.
pub(crate) struct Matched<'a> {
    /// How many argument-list tokens the match consumed: 1 for `--flag=value`
    /// (the value rode along in the same token), 2 for `--flag value`.
    pub(crate) tokens: usize,
    /// The value, or `None` when there isn't a usable one: nothing follows a
    /// split-form flag at the end of the arguments, or an equals-form flag was
    /// written with nothing after the `=` (`--flag=`). Both are the caller's
    /// error to report — only the caller knows the right message (a project
    /// path, an install root, a helper binary, …) and whether `tokens` still
    /// matters once it errors out.
    pub(crate) value: Option<Cow<'a, OsStr>>,
}

/// Recognize `token` as `flag`, in split (`--flag value`, value from `next`)
/// or equals (`--flag=value`, value inline) form. `None` when `token` names a
/// different flag entirely, leaving it to the caller's own matching.
pub(crate) fn match_flag<'a>(
    token: &str,
    next: Option<&'a OsString>,
    flag: &str,
) -> Option<Matched<'a>> {
    if token == flag {
        return Some(Matched {
            tokens: 2,
            value: next.map(|value| Cow::Borrowed(value.as_os_str())),
        });
    }
    let inline = token.strip_prefix(flag)?.strip_prefix('=')?;
    let value = if inline.is_empty() {
        None
    } else {
        Some(Cow::Owned(OsString::from(inline)))
    };
    Some(Matched { tokens: 1, value })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(value: &str) -> OsString {
        OsString::from(value)
    }

    #[test]
    fn split_form_matches_and_consumes_two_tokens() {
        let next = os("../game");
        let matched =
            match_flag("--project", Some(&next), "--project").expect("split form matches");
        assert_eq!(matched.tokens, 2);
        assert_eq!(matched.value.as_deref(), Some(OsStr::new("../game")));
    }

    #[test]
    fn equals_form_matches_and_consumes_one_token() {
        let matched =
            match_flag("--project=../game", None, "--project").expect("equals form matches");
        assert_eq!(matched.tokens, 1);
        assert_eq!(matched.value.as_deref(), Some(OsStr::new("../game")));
    }

    #[test]
    fn a_token_naming_a_different_flag_does_not_match() {
        assert!(match_flag("--manifest", None, "--project").is_none());
        // A prefix match is not a match: `--projectile` is not `--project`.
        assert!(match_flag("--projectile", None, "--project").is_none());
    }

    #[test]
    fn split_form_with_no_next_token_still_matches_with_no_value() {
        let matched =
            match_flag("--project", None, "--project").expect("the flag itself still matches");
        assert_eq!(matched.tokens, 2);
        assert_eq!(matched.value, None);
    }

    /// `--project=` names the flag but supplies nothing — the empty-value case
    /// a caller must reject rather than silently treat as an unset project.
    #[test]
    fn empty_equals_value_matches_with_no_value() {
        let matched =
            match_flag("--project=", None, "--project").expect("the flag itself still matches");
        assert_eq!(matched.tokens, 1);
        assert_eq!(matched.value, None);
    }
}
