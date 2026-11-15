// The clip event name rule, shared by the model loader and descriptor load.
// See: context/lib/resource_management.md §7 (Clip events)

/// Whether `name` is a valid clip event name: `[A-Za-z_][A-Za-z0-9_]*`,
/// case-sensitive. The loader treats any other name as malformed, and
/// descriptor load rejects an `action.at` outside the rule.
pub fn is_clip_event_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_event_names_follow_the_identifier_rule() {
        for valid in ["hit", "_kick", "Hit2", "a_b"] {
            assert!(is_clip_event_name(valid), "{valid}");
        }
        for invalid in ["", "2hit", "hit-1", "hit point", "é"] {
            assert!(!is_clip_event_name(invalid), "{invalid}");
        }
    }
}
