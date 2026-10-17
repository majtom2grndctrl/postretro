// Map source text at the shalrath boundary: layout normalization before the
// parse, and a completeness check after it.
// See: context/lib/build_pipeline.md §Supported Map Formats

use anyhow::Result;
use shambler::shalrath::repr::Map;

/// Longest excerpt of the offending source line quoted in a parse error.
const ERROR_EXCERPT_CHARS: usize = 60;

/// Map source text rewritten into the strict layout shalrath's grammar
/// accepts, with a record of where each surviving line came from.
pub(super) struct NormalizedMapSource {
    pub(super) text: String,
    /// `source_lines[i]` is the 1-based line number in the original file of
    /// line `i` of `text`. Later per-line rewrites must keep the line count so
    /// this map stays valid.
    source_lines: Vec<usize>,
}

/// Rewrite `raw` so layout that Quake tools accept cannot end shalrath's
/// entity list early.
///
/// shalrath separates entities, properties and brushes by exactly one line
/// ending and has no notion of indentation, so a blank line, a whitespace-only
/// line, an indented line, or a `//` comment where it expects a property makes
/// the next item fail. At the top level that failure ends the entity list
/// without an error. Outside quoted strings this pass therefore drops blank,
/// whitespace-only and `//` comment lines, trims surrounding horizontal
/// whitespace, and turns CRLF into LF. A line that starts inside a multi-line
/// quoted value is kept verbatim apart from its line ending.
pub(super) fn normalize_map_source(raw: &str) -> NormalizedMapSource {
    let mut text = String::with_capacity(raw.len());
    let mut source_lines = Vec::new();
    let mut in_string = false;

    for (index, line) in raw.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let starts_in_string = in_string;
        let candidate = if starts_in_string {
            line
        } else {
            let leading_trimmed = line.trim_start();
            if leading_trimmed.is_empty() || leading_trimmed.starts_with("//") {
                continue;
            }
            leading_trimmed
        };

        // shalrath's string grammar has no escapes: every `"` toggles.
        let quote_count = candidate.bytes().filter(|&b| b == b'"').count();
        in_string ^= quote_count % 2 == 1;
        let kept = if starts_in_string || in_string {
            candidate
        } else {
            candidate.trim_end()
        };

        if !source_lines.is_empty() {
            text.push('\n');
        }
        text.push_str(kept);
        source_lines.push(index + 1);
    }
    if !source_lines.is_empty() {
        text.push('\n');
    }

    NormalizedMapSource { text, source_lines }
}

/// Parse `text` (a normalized source, possibly rewritten line-for-line since)
/// with shalrath and require that the parse consumed every entity.
///
/// shalrath's `FromStr for Map` discards whatever its entity list leaves
/// unconsumed, so a malformed entity truncates the map silently. Here any
/// non-whitespace remainder is an error naming the original line where parsing
/// stopped — the start of the first entity shalrath could not parse.
pub(super) fn parse_complete_map(source: &NormalizedMapSource, text: &str) -> Result<Map> {
    debug_assert_eq!(
        text.lines().count(),
        source.text.lines().count(),
        "map source rewrites must preserve the line count"
    );

    // On a hard error shalrath failed on the first entity, so parsing stopped
    // at the start of the input.
    let (rest, map) = match shambler::shalrath::parser::repr::parse_map(text) {
        Ok((rest, map)) => (rest, Some(map)),
        Err(_) => (text, None),
    };

    let unparsed = rest.trim_start();
    if !unparsed.is_empty() {
        let offset = text.len() - unparsed.len();
        let normalized_line = text[..offset].matches('\n').count();
        let source_line = source
            .source_lines
            .get(normalized_line)
            .copied()
            .unwrap_or(normalized_line + 1);
        let excerpt: String = unparsed
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(ERROR_EXCERPT_CHARS)
            .collect();
        anyhow::bail!(
            "failed to parse .map syntax: parsing stopped at line {source_line} \
             (`{excerpt}`); the entity or text starting there is malformed"
        );
    }

    map.ok_or_else(|| anyhow::anyhow!("failed to parse .map syntax: the file has no entities"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_data::MapData;
    use crate::map_format::MapFormat;

    const WORLD: &str = "{\n\
\"classname\" \"worldspawn\"\n\
{\n\
( -64 -64 -16 ) ( -64 -63 -16 ) ( -64 -64 -15 ) floor 0 0 0 1 1\n\
( 64 64 0 ) ( 64 64 1 ) ( 64 65 0 ) floor 0 0 0 1 1\n\
( -64 -64 -16 ) ( -64 -64 -15 ) ( -63 -64 -16 ) floor 0 0 0 1 1\n\
( 64 64 0 ) ( 65 64 0 ) ( 64 64 1 ) floor 0 0 0 1 1\n\
( -64 -64 -16 ) ( -63 -64 -16 ) ( -64 -63 -16 ) floor 0 0 0 1 1\n\
( 64 64 0 ) ( 64 65 0 ) ( 65 64 0 ) floor 0 0 0 1 1\n\
}\n\
}\n";

    const LIGHT: &str = "{\n\
\"classname\" \"light\"\n\
\"origin\" \"0 0 64\"\n\
\"light\" \"200\"\n\
\"_color\" \"255 255 255\"\n\
\"_falloff_range\" \"256\"\n\
}\n";

    const SPAWN: &str = "{\n\
\"classname\" \"player_spawn\"\n\
\"origin\" \"0 0 24\"\n\
}\n";

    /// Parse `text` through the whole `.map` front end, bytes written verbatim.
    fn parse_map_text(text: &str) -> Result<MapData> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("source.map");
        std::fs::write(&path, text)?;
        crate::parse::parse_map_file(&path, MapFormat::IdTech2)
    }

    fn sorted_classnames(map: &MapData) -> Vec<&str> {
        let mut names: Vec<&str> = map.entities.iter().map(|e| e.classname.as_str()).collect();
        names.sort_unstable();
        names
    }

    fn assert_all_entities_kept(map: &MapData) {
        assert_eq!(
            sorted_classnames(map),
            ["light", "player_spawn", "worldspawn"],
            "every top-level entity must survive the parse"
        );
        assert_eq!(map.lights.len(), 1);
    }

    // Regression: a blank line between top-level entities ended shalrath's
    // entity list, so prl-build silently dropped every later entity and exited 0.
    #[test]
    fn parse_keeps_entities_after_blank_line_between_entities() {
        let map = parse_map_text(&format!("{WORLD}\n{LIGHT}\n\n{SPAWN}")).unwrap();
        assert_all_entities_kept(&map);
    }

    #[test]
    fn parse_keeps_entities_after_whitespace_only_and_indented_lines() {
        let map = parse_map_text(&format!("{WORLD}   \t\n{LIGHT} \n  \n\t{SPAWN}")).unwrap();
        assert_all_entities_kept(&map);
    }

    #[test]
    fn parse_keeps_entities_with_crlf_line_endings_and_blank_lines() {
        let text = format!("{WORLD}\n{LIGHT}\n{SPAWN}").replace('\n', "\r\n");
        let map = parse_map_text(&text).unwrap();
        assert_all_entities_kept(&map);
    }

    #[test]
    fn parse_keeps_entities_with_comment_lines_between_and_inside_entities() {
        let light_with_comment = LIGHT.replacen("\"origin\"", "// a note\n\"origin\"", 1);
        let text = format!(
            "// Game: Postretro\n{WORLD}// entity 1 \"quoted\n\n//\n{light_with_comment}   // entity 2\n{SPAWN}"
        );
        let map = parse_map_text(&text).unwrap();
        assert_all_entities_kept(&map);
    }

    #[test]
    fn parse_rejects_trailing_garbage_with_its_line_number() {
        // WORLD is 11 lines, LIGHT 7, then a blank line: the garbage is line 20.
        let err = parse_map_text(&format!("{WORLD}{LIGHT}\nnot an entity\n{SPAWN}"))
            .err()
            .expect("unparsed trailing content must be an error");
        let message = format!("{err:#}");
        assert!(message.contains("line 20"), "{message}");
        assert!(message.contains("not an entity"), "{message}");
    }

    #[test]
    fn parse_rejects_malformed_later_entity_instead_of_truncating() {
        // The light's `}` is missing, so its brace block swallows the spawn and
        // never closes; parsing must stop loudly at the light's opening line.
        let broken_light = LIGHT.trim_end().trim_end_matches('}');
        let err = parse_map_text(&format!("{WORLD}{broken_light}{SPAWN}"))
            .err()
            .expect("a malformed entity must not truncate the map silently");
        assert!(format!("{err:#}").contains("line 12"), "{err:#}");
    }

    #[test]
    fn parse_rejects_malformed_first_entity_with_its_line_number() {
        let err = parse_map_text("\n\nnot a map\n")
            .err()
            .expect("an unparseable first entity is an error");
        assert!(format!("{err:#}").contains("line 3"), "{err:#}");
    }

    #[test]
    fn normalize_keeps_multi_line_quoted_value_verbatim() {
        let raw = "{\r\n\"message\" \"first\r\n\r\n  // not a comment\r\nlast\"\r\n}\r\n";
        let normalized = normalize_map_source(raw);
        assert_eq!(
            normalized.text,
            "{\n\"message\" \"first\n\n  // not a comment\nlast\"\n}\n"
        );
        assert_eq!(normalized.source_lines, [1, 2, 3, 4, 5, 6]);
    }
}
