// Token references: an attack's `draws` and a profile's overrides, each a
// token handle or `token.with({ ... })`, parsed with the fields its place allows.
// See: context/lib/entity_model.md §7c (Attack budget) · context/lib/scripting.md §2

use serde::{Deserialize, Deserializer, Serialize};

use super::MAX_ATTACK_DRAWS;
use crate::defense::{HandleRef, validate_defense_name};

/// One entry of an attack's `draws` as authored: a token handle, or
/// `token.with({ cost })`. Kept raw so a bad one is reported with its key
/// path instead of a bare serde message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TokenDraw(pub serde_json::Value);

/// A parsed draw: the token it names and its cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawRef<'a> {
    pub token: &'a str,
    pub cost: u32,
}

impl TokenDraw {
    /// Parse this draw at `path`: a handle draws one; a reference carries
    /// only an optional `cost`.
    pub fn parse(&self, path: &str) -> Result<DrawRef<'_>, String> {
        let parsed = parse_token_ref(path, &self.0, RefPosition::Draw)?;
        Ok(DrawRef {
            token: parsed.token,
            cost: parsed.cost.unwrap_or(1),
        })
    }
}

/// Check an attack's `draws` shapes at `path`, registry-free: each entry is a
/// token handle or reference with a valid `cost`, no token appears twice,
/// and the list holds at most [`MAX_ATTACK_DRAWS`].
pub fn check_draw_shapes(path: &str, draws: &[TokenDraw]) -> Result<(), String> {
    parse_draw_list(path, draws).map(|_| ())
}

pub(super) fn parse_draw_list<'a>(
    path: &str,
    draws: &'a [TokenDraw],
) -> Result<Vec<(String, DrawRef<'a>)>, String> {
    if draws.len() > MAX_ATTACK_DRAWS {
        return Err(format!(
            "`{path}` draws {} tokens; an attack draws at most {MAX_ATTACK_DRAWS}",
            draws.len()
        ));
    }
    let mut parsed: Vec<(String, DrawRef<'a>)> = Vec::with_capacity(draws.len());
    for (index, draw) in draws.iter().enumerate() {
        let entry_path = format!("{path}[{index}]");
        let draw = draw.parse(&entry_path)?;
        if parsed.iter().any(|(_, seen)| seen.token == draw.token) {
            return Err(format!(
                "`{entry_path}`: attack token `{}` is drawn twice by one attack; draw it once with `.with({{ cost }})`",
                draw.token
            ));
        }
        parsed.push((entry_path, draw));
    }
    Ok(parsed)
}

/// `draws` as authored. Luau writes an empty list as an empty table, which
/// crosses as `{}`; that one shape reads as `[]`.
pub fn deserialize_draws<'de, D>(deserializer: D) -> Result<Option<Vec<TokenDraw>>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<serde_json::Value>::deserialize(deserializer)? {
        None => Ok(None),
        Some(serde_json::Value::Array(entries)) => {
            Ok(Some(entries.into_iter().map(TokenDraw).collect()))
        }
        Some(serde_json::Value::Object(map)) if map.is_empty() => Ok(Some(Vec::new())),
        Some(other) => Err(serde::de::Error::custom(format!(
            "`draws` must be a list of attack token references, got {other}"
        ))),
    }
}

/// Where a token reference sits, which decides the fields it may carry.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RefPosition {
    /// An attack's draw: `cost`.
    Draw,
    /// A profile override: `limit` and `refreshMs`.
    Profile,
}

pub(super) struct ParsedRef<'a> {
    pub(super) token: &'a str,
    pub(super) cost: Option<u32>,
    pub(super) limit: Option<u32>,
    pub(super) refresh_ms: Option<f32>,
}

/// Parse a token reference: a handle, or `{ token, ...fields }` as
/// `token.with({ ... })` builds it.
pub(super) fn parse_token_ref<'a>(
    path: &str,
    value: &'a serde_json::Value,
    position: RefPosition,
) -> Result<ParsedRef<'a>, String> {
    let Some(object) = value.as_object() else {
        return Err(match value.as_str() {
            Some(text) => format!(
                "`{path}`: `\"{text}\"` is a bare string; pass the token handle `defineAttackToken` returned"
            ),
            None => format!("`{path}`: expected an attack token handle or `token.with({{ ... }})`"),
        });
    };
    let Some(token) = object.get("token") else {
        return Ok(ParsedRef {
            token: handle_name(path, value)?,
            cost: None,
            limit: None,
            refresh_ms: None,
        });
    };
    let mut parsed = ParsedRef {
        token: handle_name(&format!("{path}.token"), token)?,
        cost: None,
        limit: None,
        refresh_ms: None,
    };
    for (key, field) in object {
        let field_path = format!("{path}.{key}");
        match (key.as_str(), position) {
            ("token", _) => {}
            ("cost", RefPosition::Draw) => {
                parsed.cost = Some(positive_integer(&field_path, field)?)
            }
            ("limit", RefPosition::Profile) => {
                parsed.limit = Some(positive_integer(&field_path, field)?)
            }
            ("refreshMs", RefPosition::Profile) => {
                parsed.refresh_ms = Some(refresh_ms(&field_path, field)?)
            }
            ("cost", RefPosition::Profile) => {
                return Err(format!(
                    "`{field_path}`: `cost` belongs in an attack's draw, not a profile override"
                ));
            }
            ("limit" | "refreshMs", RefPosition::Draw) => {
                return Err(format!(
                    "`{field_path}`: `{key}` belongs in a profile override, not an attack's draw"
                ));
            }
            _ => {
                return Err(format!(
                    "`{field_path}`: a token reference has no field `{key}`"
                ));
            }
        }
    }
    Ok(parsed)
}

/// The name an attack-token handle at `path` carries.
fn handle_name<'a>(path: &str, value: &'a serde_json::Value) -> Result<&'a str, String> {
    HandleRef::from_json(value)
        .and_then(|handle| handle.expect_attack_token().map(|_| ()))
        .map_err(|error| format!("`{path}`: {error}"))?;
    let name = value
        .get("name")
        .and_then(serde_json::Value::as_str)
        .expect("an attack token handle carries a string name");
    validate_defense_name(name).map_err(|error| format!("`{path}.name`: {error}"))?;
    Ok(name)
}

pub(super) fn positive_integer(path: &str, value: &serde_json::Value) -> Result<u32, String> {
    let number = value.as_f64().filter(|number| {
        number.is_finite()
            && number.fract() == 0.0
            && *number >= 1.0
            && *number <= f64::from(u32::MAX)
    });
    number
        .map(|number| number as u32)
        .ok_or_else(|| format!("`{path}` must be an integer of at least 1, got {value}"))
}

pub(super) fn refresh_ms(path: &str, value: &serde_json::Value) -> Result<f32, String> {
    value
        .as_f64()
        .map(|number| number as f32)
        .filter(|number| number.is_finite() && *number >= 0.0)
        .ok_or_else(|| format!("`{path}` must be a finite number of at least 0, got {value}"))
}
