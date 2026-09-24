//! Source-text validation for compiler-only streaming hint brushes.
//!
//! shalrath deliberately recovers from malformed faces by dropping the
//! affected entity. That recovery is appropriate for ordinary editor work but
//! cannot be allowed to silently erase a strict streaming authoring contract.

use anyhow::Result;

use super::{parse_origin, quake_to_engine};

/// Check streaming-hint entities before shalrath builds its geometry map.
/// The source scan identifies only authored hints; shalrath's own entity
/// parser then verifies their complete face syntax, so its permissive map
/// parser cannot silently discard a malformed hint after a valid prefix.
pub(super) fn reject_invalid_streaming_hint_source_hulls(map_text: &str, scale: f64) -> Result<()> {
    let (entities, unterminated) = source_entity_blocks(map_text);
    for entity in entities {
        let Some(classname) = source_entity_property(entity, "classname") else {
            continue;
        };
        if !matches!(
            classname,
            "streaming_seam_volume" | "stream_resident_volume" | "stream_priority_region"
        ) {
            continue;
        }

        let location = source_hint_location(entity, scale);
        validate_source_hint_brush_point_triples(entity, classname, &location)?;
        match shambler::shalrath::parser::repr::parse_entity(entity) {
            Ok((remaining, _)) if remaining.trim().is_empty() => {}
            _ => anyhow::bail!(
                "{classname} {location} has invalid brush/entity syntax; every hint face must parse completely"
            ),
        }
    }
    if let Some(entity) = unterminated
        && let Some(
            classname @ ("streaming_seam_volume"
            | "stream_resident_volume"
            | "stream_priority_region"),
        ) = source_entity_property(entity, "classname")
    {
        let location = source_hint_location(entity, scale);
        anyhow::bail!("{classname} {location} has an unterminated entity/brush block");
    }
    Ok(())
}

/// Return complete top-level entity blocks and any unterminated tail while
/// ignoring line comments and braces inside quoted property values.
fn source_entity_blocks(map_text: &str) -> (Vec<&str>, Option<&str>) {
    let mut blocks = Vec::new();
    let mut start = None;
    let mut depth = 0usize;
    let mut in_quote = false;
    let mut escaped = false;
    let mut in_comment = false;

    for (index, ch) in map_text.char_indices() {
        if in_comment {
            if ch == '\n' {
                in_comment = false;
            }
            continue;
        }
        if in_quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_quote = false;
            }
            continue;
        }
        match ch {
            '"' => in_quote = true,
            '/' if map_text[index..].starts_with("//") => in_comment = true,
            '{' if depth == 0 => {
                start = Some(index);
                depth = 1;
            }
            '{' => depth += 1,
            '}' if depth > 1 => depth -= 1,
            '}' if depth == 1 => {
                depth = 0;
                if let Some(start_index) = start.take() {
                    blocks.push(&map_text[start_index..=index]);
                }
            }
            _ => {}
        }
    }
    (blocks, start.map(|index| &map_text[index..]))
}

/// Find a quoted KVP in one top-level source entity without confusing quoted
/// brush texture names or comments for authoring properties.
fn source_entity_property<'a>(entity: &'a str, key: &str) -> Option<&'a str> {
    let mut depth = 0usize;
    let mut in_comment = false;
    let mut pending_key = None;
    let mut chars = entity.char_indices().peekable();

    while let Some((index, ch)) = chars.next() {
        if in_comment {
            if ch == '\n' {
                in_comment = false;
            }
            continue;
        }
        match ch {
            '/' if entity[index..].starts_with("//") => in_comment = true,
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            '"' if depth == 1 => {
                let value_start = index + ch.len_utf8();
                let mut escaped = false;
                let mut value_end = None;
                for (quote_index, quote_ch) in chars.by_ref() {
                    if escaped {
                        escaped = false;
                    } else if quote_ch == '\\' {
                        escaped = true;
                    } else if quote_ch == '"' {
                        value_end = Some(quote_index);
                        break;
                    }
                }
                let value = &entity[value_start..value_end?];
                if let Some(candidate_key) = pending_key.take() {
                    if candidate_key == key {
                        return Some(value);
                    }
                } else {
                    pending_key = Some(value);
                }
            }
            _ => {}
        }
    }
    None
}

/// Validate only unquoted, uncommented triples nested in brush blocks
/// (`depth >= 2`). Arbitrary author notes, comments, and quoted texture names
/// are therefore never mistaken for geometry.
fn validate_source_hint_brush_point_triples(
    entity: &str,
    classname: &str,
    location: &str,
) -> Result<()> {
    let mut depth = 0usize;
    let mut in_comment = false;
    let mut in_quote = false;
    let mut escaped = false;
    let mut chars = entity.char_indices().peekable();

    while let Some((index, ch)) = chars.next() {
        if in_comment {
            if ch == '\n' {
                in_comment = false;
            }
            continue;
        }
        if in_quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_quote = false;
            }
            continue;
        }
        match ch {
            '/' if entity[index..].starts_with("//") => in_comment = true,
            '"' => in_quote = true,
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            '(' if depth >= 2 => {
                let value_start = index + ch.len_utf8();
                let mut value_end = None;
                for (close_index, close_ch) in chars.by_ref() {
                    if close_ch == ')' {
                        value_end = Some(close_index);
                        break;
                    }
                }
                let value_end = value_end.ok_or_else(|| {
                    anyhow::anyhow!(
                        "{classname} {location} has an invalid brush hull: unterminated point triple"
                    )
                })?;
                let values = entity[value_start..value_end]
                    .split_whitespace()
                    .collect::<Vec<_>>();
                if values.len() != 3 {
                    anyhow::bail!(
                        "{classname} {location} has an invalid brush hull: point triples require three coordinates"
                    );
                }
                for value in values {
                    let parsed = value.parse::<f64>().map_err(|error| {
                        anyhow::anyhow!(
                            "{classname} {location} has an invalid brush hull coordinate `{value}` ({error})"
                        )
                    })?;
                    if !parsed.is_finite() {
                        anyhow::bail!(
                            "{classname} {location} has a non-finite brush hull coordinate `{value}`"
                        );
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Give source-level validation errors canonical engine-space location wording
/// whenever a finite authored origin is present.
fn source_hint_location(entity: &str, scale: f64) -> String {
    let Some(origin) = source_entity_property(entity, "origin").and_then(parse_origin) else {
        return "at an entity without an authored origin".to_string();
    };
    if !origin.is_finite() {
        return "at a non-finite authored origin".to_string();
    }
    let engine_origin = quake_to_engine(origin) * scale;
    format!(
        "at ({:.3}, {:.3}, {:.3}) m",
        engine_origin.x, engine_origin.y, engine_origin.z
    )
}

#[cfg(test)]
mod tests {
    use super::{
        reject_invalid_streaming_hint_source_hulls, source_entity_blocks, source_entity_property,
    };

    #[test]
    fn ignores_comments_and_quoted_text_when_scanning_hint_hulls() {
        let source = r#"// { "classname" "streaming_seam_volume" "( nan 0 0 )" }
{
"classname" "streaming_seam_volume"
"origin" "1 2 3"
"note" "( nan 0 0 ) // { not geometry }"
{
( 0 0 0 ) ( 0 32 0 ) ( 0 0 32 ) "classname" 0 0 0 1 1
( 32 0 0 ) ( 32 0 32 ) ( 32 32 0 ) "classname" 0 0 0 1 1
( 0 0 0 ) ( 0 0 32 ) ( 32 0 0 ) "classname" 0 0 0 1 1
( 0 32 0 ) ( 32 32 0 ) ( 0 32 32 ) "classname" 0 0 0 1 1
( 0 0 0 ) ( 32 0 0 ) ( 0 32 0 ) "classname" 0 0 0 1 1
( 0 0 32 ) ( 0 32 32 ) ( 32 0 32 ) "classname" 0 0 0 1 1
}
}
"#;
        let (blocks, unterminated) = source_entity_blocks(source);
        assert_eq!(blocks.len(), 1, "comment braces must not open an entity");
        assert!(unterminated.is_none());
        assert_eq!(
            source_entity_property(blocks[0], "classname"),
            Some("streaming_seam_volume"),
            "only a top-level KVP may identify a streaming hint"
        );
        let normalized = super::super::encode_quoted_brush_textures(source);
        reject_invalid_streaming_hint_source_hulls(&normalized, 0.0254)
            .expect("comments, note text, and quoted brush textures are not point triples");
    }
}
