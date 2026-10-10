// Addressing wire validation shared by the QuickJS and Luau reaction
// converters — group `kind` and subject-token `target` — so both runtimes
// reject the same malformed entry with the same diagnostic.
// See: context/lib/scripting.md §12 (Entity addressing)

use postretro_entities::GroupKind;
use postretro_entities::data_descriptors::SequenceTarget;

/// An authored optional string field (`kind`, `target`), lowered from either
/// VM before validation.
#[derive(Debug, Clone)]
pub enum AuthoredText {
    /// The key is absent, `null`, `undefined` or `nil`.
    Absent,
    Text(String),
    /// Any non-string value, named by its VM type for the diagnostic.
    NonString(String),
}

/// Validate one descriptor's `kind` against the fields beside it. `site` names
/// the entry inside the reaction (`"primitive"`, `"sequence step 2"`). An entry
/// carrying both an id (or a sentinel target) and a `kind` is rejected: the two
/// address different things, and a silent winner would hide the author's
/// mistake. Errors name the reaction so the skip warning points at it.
pub fn validate_authored_group_kind(
    reaction: &str,
    site: &str,
    kind: AuthoredText,
    has_id: bool,
    has_target: bool,
) -> Result<Option<GroupKind>, String> {
    let kind = match kind {
        AuthoredText::Absent => return Ok(None),
        AuthoredText::Text(spelling) => GroupKind::from_wire(&spelling).ok_or_else(|| {
            format!(
                "reaction `{reaction}` {site}: `kind` must be \"npc\" or \"player\", got \"{spelling}\""
            )
        })?,
        AuthoredText::NonString(type_name) => {
            return Err(format!(
                "reaction `{reaction}` {site}: `kind` must be \"npc\" or \"player\", got a {type_name}"
            ));
        }
    };
    if has_id {
        return Err(format!(
            "reaction `{reaction}` {site}: an entry cannot carry both `id` and `kind`"
        ));
    }
    if has_target {
        return Err(format!(
            "reaction `{reaction}` {site}: an entry cannot carry both `target` and `kind`"
        ));
    }
    Ok(Some(kind))
}

/// Reject `spawnFromSpawner` beside a group `kind`: it acts on a spawner,
/// addressed by `tag` or member id, so the `kind` would be silently dropped.
/// Every other primitive passes; the handler decides what a group applies to.
pub fn validate_group_kind_primitive(
    reaction: &str,
    site: &str,
    primitive: &str,
) -> Result<(), String> {
    if primitive == "spawnFromSpawner" {
        return Err(format!(
            "reaction `{reaction}` {site}: `spawnFromSpawner` addresses spawners, so it cannot carry a group `kind`"
        ));
    }
    Ok(())
}

/// A trigger-fire subject token: the entity a fire is about. Legal only before
/// any `wait`, because the fire context does not survive it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubjectToken {
    /// `on.activators`: the pawns that caused the edge.
    Activators,
    /// `on.trigger`: the volume that fired.
    FiredTrigger,
    /// `on.player`: the player a player event fires for.
    EventPlayer,
}

impl SubjectToken {
    /// Parse the wire spelling. `None` for anything else, the control
    /// sentinels `@wait`/`@fire` included — those are never a command target.
    pub fn from_wire(spelling: &str) -> Option<Self> {
        match spelling {
            "@activators" => Some(Self::Activators),
            "@trigger" => Some(Self::FiredTrigger),
            "@player" => Some(Self::EventPlayer),
            _ => None,
        }
    }

    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Activators => "@activators",
            Self::FiredTrigger => "@trigger",
            Self::EventPlayer => "@player",
        }
    }

    /// The token a parsed sequence target carries, if it is one.
    pub fn of_sequence_target(target: &SequenceTarget) -> Option<Self> {
        match target {
            SequenceTarget::Activators => Some(Self::Activators),
            SequenceTarget::FiredTrigger => Some(Self::FiredTrigger),
            SequenceTarget::EventPlayer => Some(Self::EventPlayer),
            _ => None,
        }
    }

    pub fn sequence_target(self) -> SequenceTarget {
        match self {
            Self::Activators => SequenceTarget::Activators,
            Self::FiredTrigger => SequenceTarget::FiredTrigger,
            Self::EventPlayer => SequenceTarget::EventPlayer,
        }
    }
}

/// Validate one descriptor's subject-token `target` against the fields beside
/// it. A token verb (`on.activators.grantHealth(25)`, `on.trigger.arm()`)
/// lowers to `{ primitive, target, args }`, legal both as a reaction body and
/// directly as a sequence entry, so this one check serves both sites. Rejects
/// an unknown sentinel and a `target` beside `id`, `kind` or `tag`: each
/// addresses something else, and a silent winner would hide the mistake.
pub fn validate_authored_subject_token(
    reaction: &str,
    site: &str,
    target: AuthoredText,
    has_id: bool,
    has_kind: bool,
    has_tag: bool,
) -> Result<Option<SubjectToken>, String> {
    let token = match target {
        AuthoredText::Absent => return Ok(None),
        AuthoredText::Text(spelling) => SubjectToken::from_wire(&spelling).ok_or_else(|| {
            format!(
                "reaction `{reaction}` {site}: `target` must be \"@activators\", \"@trigger\" or \"@player\", got \"{spelling}\""
            )
        })?,
        AuthoredText::NonString(type_name) => {
            return Err(format!(
                "reaction `{reaction}` {site}: `target` must be \"@activators\", \"@trigger\" or \"@player\", got a {type_name}"
            ));
        }
    };
    for (present, field) in [(has_id, "id"), (has_kind, "kind"), (has_tag, "tag")] {
        if present {
            return Err(format!(
                "reaction `{reaction}` {site}: an entry cannot carry both `target` and `{field}`"
            ));
        }
    }
    Ok(Some(token))
}

/// A subject token carries only the verbs its subject supports: the fired
/// volume arms and disarms; the activators never do. Applied to every
/// subject-token entry, whichever field carries the token.
pub fn validate_subject_token_primitive(
    reaction: &str,
    site: &str,
    token: SubjectToken,
    primitive: &str,
) -> Result<(), String> {
    let arms_a_trigger = matches!(primitive, "armTrigger" | "disarmTrigger");
    match token {
        SubjectToken::Activators | SubjectToken::EventPlayer if arms_a_trigger => Err(format!(
            "reaction `{reaction}` {site}: `{primitive}` targets a trigger volume, so it takes `@trigger` (`on.trigger`), not `{}`",
            token.as_wire()
        )),
        SubjectToken::FiredTrigger if !arms_a_trigger => Err(format!(
            "reaction `{reaction}` {site}: `@trigger` (`on.trigger`) carries only `armTrigger` and `disarmTrigger`, not `{primitive}`"
        )),
        _ => Ok(()),
    }
}
