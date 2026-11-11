// Attack tokens and budget profiles: the registered vocabulary an enemy's
// attacks draw against, and the token references both SDKs serialize.
// See: context/lib/entity_model.md §7c (Attack budget) · context/lib/scripting.md §2

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

use crate::defense::validate_defense_name;

mod references;

pub use references::{DrawRef, TokenDraw, check_draw_shapes, deserialize_draws};
use references::{RefPosition, parse_draw_list, parse_token_ref, positive_integer, refresh_ms};

/// The SDK's `defaultToken`: registered by every mod, listed by none.
pub const DEFAULT_TOKEN_NAME: &str = "default";
/// `defineAttackToken`'s default `limit`, which `defaultToken` declares.
pub const DEFAULT_TOKEN_LIMIT: u32 = 3;
/// `defineAttackToken`'s default `refreshMs`.
pub const DEFAULT_TOKEN_REFRESH_MS: f32 = 0.0;
/// The profile enemies in no running attack pool take turns under.
pub const DEFAULT_PROFILE_NAME: &str = "default";
/// Most tokens one attack may draw. Turn state stores draw lists inline.
pub const MAX_ATTACK_DRAWS: usize = 8;

const TOKEN_KIND: &str = "attackToken";

/// One token's budget on one target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenBudget {
    /// How much of the token may be drawn on one target at once.
    pub limit: u32,
    /// How long a spent draw stays counted after its turn ends.
    pub refresh_ms: f32,
}

impl TokenBudget {
    pub const DEFAULT: TokenBudget = TokenBudget {
        limit: DEFAULT_TOKEN_LIMIT,
        refresh_ms: DEFAULT_TOKEN_REFRESH_MS,
    };
}

/// One registered token: its name and declared (default) budget.
#[derive(Debug, Clone, PartialEq)]
pub struct AttackTokenDeclaration {
    pub name: String,
    pub budget: TokenBudget,
}

/// One profile entry: the fields it overrides on one token. An absent field
/// keeps the token's declared value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BudgetOverride {
    /// The token's id: its index in [`AttackBudgetVocabulary::tokens`].
    pub token: u16,
    pub limit: Option<u32>,
    pub refresh_ms: Option<f32>,
}

/// A mod's registered attack tokens and budget profiles. Token ids are
/// indexes into `tokens`: `defaultToken` is always 0, then the mod's tokens
/// in `attackTokens` order, so ids are stable for one registration.
#[derive(Debug, Clone, PartialEq)]
pub struct AttackBudgetVocabulary {
    tokens: Vec<AttackTokenDeclaration>,
    profiles: BTreeMap<String, Vec<BudgetOverride>>,
}

impl Default for AttackBudgetVocabulary {
    fn default() -> Self {
        Self {
            tokens: vec![AttackTokenDeclaration {
                name: DEFAULT_TOKEN_NAME.to_string(),
                budget: TokenBudget::DEFAULT,
            }],
            profiles: BTreeMap::new(),
        }
    }
}

static SHARED_DEFAULT: LazyLock<Arc<AttackBudgetVocabulary>> =
    LazyLock::new(|| Arc::new(AttackBudgetVocabulary::default()));

impl AttackBudgetVocabulary {
    /// One process-wide copy of the unregistered vocabulary, so holders that
    /// have seen no commit compare equal by pointer.
    pub fn shared_default() -> Arc<Self> {
        Arc::clone(&SHARED_DEFAULT)
    }

    /// Every registered token by id, `defaultToken` first.
    pub fn tokens(&self) -> &[AttackTokenDeclaration] {
        &self.tokens
    }

    pub fn token_index(&self, name: &str) -> Option<u16> {
        self.tokens
            .iter()
            .position(|token| token.name == name)
            .and_then(|index| u16::try_from(index).ok())
    }

    /// Each token's declared budget, by id.
    pub fn declared_budgets(&self) -> impl Iterator<Item = TokenBudget> + '_ {
        self.tokens.iter().map(|token| token.budget)
    }

    /// Every declared profile, by name.
    pub fn profiles(&self) -> &BTreeMap<String, Vec<BudgetOverride>> {
        &self.profiles
    }

    /// Each token's budget under profile `name`: the declared budgets with
    /// that profile's named fields applied. `default` without a declaration
    /// is the declared budgets; any other undeclared name is `None`.
    pub fn profile_budgets(&self, name: &str) -> Option<Vec<TokenBudget>> {
        let mut budgets: Vec<TokenBudget> = self.declared_budgets().collect();
        let Some(overrides) = self.profiles.get(name) else {
            return (name == DEFAULT_PROFILE_NAME).then_some(budgets);
        };
        for entry in overrides {
            if let Some(budget) = budgets.get_mut(entry.token as usize) {
                budget.limit = entry.limit.unwrap_or(budget.limit);
                budget.refresh_ms = entry.refresh_ms.unwrap_or(budget.refresh_ms);
            }
        }
        Some(budgets)
    }

    /// Parse the manifest's `attackTokens` and `attackBudgets` values (each
    /// absent when `None`). Every failure names its key path.
    pub fn from_manifest(
        attack_tokens: Option<&serde_json::Value>,
        attack_budgets: Option<&serde_json::Value>,
    ) -> Result<Self, String> {
        let mut vocabulary = Self::default();
        for (index, entry) in list_entries("attackTokens", attack_tokens)?
            .iter()
            .enumerate()
        {
            let path = format!("attackTokens[{index}]");
            let declaration = parse_declaration(&path, entry)?;
            if declaration.name == DEFAULT_TOKEN_NAME {
                return Err(format!(
                    "`{path}`: `{DEFAULT_TOKEN_NAME}` is the SDK's `defaultToken`, which every mod has; a mod never lists it or declares a token by that name"
                ));
            }
            if vocabulary.token_index(&declaration.name).is_some() {
                return Err(format!(
                    "`{path}`: attack token `{}` is registered twice",
                    declaration.name
                ));
            }
            if vocabulary.tokens.len() > usize::from(u16::MAX) {
                return Err(format!("`{path}`: too many attack tokens"));
            }
            vocabulary.tokens.push(declaration);
        }
        vocabulary.profiles = parse_profiles(attack_budgets, &vocabulary)?;
        Ok(vocabulary)
    }

    /// Check an attack's `draws` at `path` against the registered tokens:
    /// each reference must name one. Shape rules are the descriptor's own
    /// ([`check_draw_shapes`]), and are re-checked here.
    pub fn check_draws(&self, path: &str, draws: Option<&[TokenDraw]>) -> Result<(), String> {
        for parsed in parse_draw_list(path, draws.unwrap_or_default())? {
            let (entry_path, draw) = parsed;
            if self.token_index(draw.token).is_none() {
                return Err(format!(
                    "`{entry_path}`: attack token `{}` is not registered in `defineMod({{ attackTokens }})`",
                    draw.token
                ));
            }
        }
        Ok(())
    }

    /// Resolve an attack's `draws` to `(token id, cost)` pairs. `None` draws
    /// one `defaultToken`; `Some([])` draws nothing. A reference that names
    /// no registered token reaches `unresolved` and is left out; load
    /// validation rejects one, so only unvalidated content meets it.
    pub fn resolve_draws<'a>(
        &self,
        draws: Option<&'a [TokenDraw]>,
        mut unresolved: impl FnMut(&'a str),
    ) -> Vec<(u16, u32)> {
        let Some(draws) = draws else {
            return vec![(0, 1)];
        };
        draws
            .iter()
            .filter_map(|draw| draw.parse("draws").ok())
            .filter_map(|draw| match self.token_index(draw.token) {
                Some(token) => Some((token, draw.cost)),
                None => {
                    unresolved(draw.token);
                    None
                }
            })
            .collect()
    }
}

/// A `defineMod({ attackTokens })` entry: the declaration `defineMod`
/// resolved from a handle, `{ kind, name, budget }`.
fn parse_declaration(
    path: &str,
    entry: &serde_json::Value,
) -> Result<AttackTokenDeclaration, String> {
    let name = handle_name_of_declaration(path, entry)?;
    let object = entry.as_object().expect("a declaration is an object");
    let mut budget = TokenBudget::DEFAULT;
    for (key, value) in object {
        match key.as_str() {
            "kind" | "name" => {}
            "budget" => {
                let budget_path = format!("{path}.budget");
                let fields = match value {
                    serde_json::Value::Null => continue,
                    serde_json::Value::Array(entries) if entries.is_empty() => continue,
                    serde_json::Value::Object(fields) => fields,
                    _ => return Err(format!("`{budget_path}` must be an object")),
                };
                for (field, field_value) in fields {
                    let field_path = format!("{budget_path}.{field}");
                    match field.as_str() {
                        "limit" => budget.limit = positive_integer(&field_path, field_value)?,
                        "refreshMs" => budget.refresh_ms = refresh_ms(&field_path, field_value)?,
                        "cost" => {
                            return Err(format!(
                                "`{field_path}`: `cost` belongs in an attack's draw, not a token's budget"
                            ));
                        }
                        _ => {
                            return Err(format!(
                                "`{field_path}`: `defineAttackToken` takes no field `{field}`"
                            ));
                        }
                    }
                }
            }
            _ => return Err(format!("`{path}.{key}`: unexpected field `{key}`")),
        }
    }
    Ok(AttackTokenDeclaration { name, budget })
}

fn handle_name_of_declaration(path: &str, entry: &serde_json::Value) -> Result<String, String> {
    if let Some(text) = entry.as_str() {
        return Err(format!(
            "`{path}`: `\"{text}\"` is a bare string; list the handle `defineAttackToken` returned"
        ));
    }
    let object = entry
        .as_object()
        .ok_or_else(|| format!("`{path}`: expected an attack token handle"))?;
    let kind = object
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if kind != TOKEN_KIND {
        let found = if kind.is_empty() {
            "a value with no `kind`".to_string()
        } else {
            format!("a `{kind}` handle")
        };
        return Err(format!(
            "`{path}`: expected an `{TOKEN_KIND}` handle, got {found}"
        ));
    }
    let name = object
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("`{path}.name` must be a string"))?;
    validate_defense_name(name).map_err(|error| format!("`{path}.name`: {error}"))?;
    Ok(name.to_string())
}

fn list_entries<'a>(
    key: &str,
    value: Option<&'a serde_json::Value>,
) -> Result<&'a [serde_json::Value], String> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(&[]),
        Some(serde_json::Value::Array(entries)) => Ok(entries),
        // An empty Luau table crosses as `{}`.
        Some(serde_json::Value::Object(map)) if map.is_empty() => Ok(&[]),
        Some(_) => Err(format!("`{key}` must be an array of attack token handles")),
    }
}

/// `attackBudgets`: `{ [profile]: [token reference] }`.
fn parse_profiles(
    value: Option<&serde_json::Value>,
    vocabulary: &AttackBudgetVocabulary,
) -> Result<BTreeMap<String, Vec<BudgetOverride>>, String> {
    let object = match value {
        None | Some(serde_json::Value::Null) => return Ok(BTreeMap::new()),
        Some(serde_json::Value::Array(entries)) if entries.is_empty() => {
            return Ok(BTreeMap::new());
        }
        Some(serde_json::Value::Object(object)) => object,
        Some(_) => {
            return Err(
                "`attackBudgets` must be an object of profiles, each a list of token references"
                    .to_string(),
            );
        }
    };
    let mut profiles = BTreeMap::new();
    for (name, entries) in object {
        let path = format!("attackBudgets.{name}");
        validate_defense_name(name).map_err(|error| format!("`{path}`: profile {error}"))?;
        let entries: &[serde_json::Value] = match entries {
            serde_json::Value::Array(entries) => entries,
            serde_json::Value::Object(map) if map.is_empty() => &[],
            _ => return Err(format!("`{path}` must be a list of token references")),
        };
        let mut overrides: Vec<BudgetOverride> = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            let entry_path = format!("{path}[{index}]");
            let parsed = parse_token_ref(&entry_path, entry, RefPosition::Profile)?;
            let token = vocabulary.token_index(parsed.token).ok_or_else(|| {
                format!(
                    "`{entry_path}`: attack token `{}` is not registered in `defineMod({{ attackTokens }})`",
                    parsed.token
                )
            })?;
            if overrides.iter().any(|seen| seen.token == token) {
                return Err(format!(
                    "`{entry_path}`: attack token `{}` is overridden twice by one profile",
                    parsed.token
                ));
            }
            overrides.push(BudgetOverride {
                token,
                limit: parsed.limit,
                refresh_ms: parsed.refresh_ms,
            });
        }
        profiles.insert(name.clone(), overrides);
    }
    Ok(profiles)
}

#[cfg(test)]
mod tests;
