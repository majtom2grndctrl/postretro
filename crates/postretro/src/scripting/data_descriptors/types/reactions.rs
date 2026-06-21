// Data-context descriptors: reaction & crossing descriptor types.
// See: context/lib/scripting.md

use super::super::*;

/// Variants of a single reaction's behavior body. The `name` lives on the
/// wrapping [`NamedReaction`]; this enum captures only the descriptor shape.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ReactionDescriptor {
    Progress(ProgressDescriptor),
    Primitive(PrimitiveDescriptor),
    /// Ordered list of (entity, sequenced-primitive, args) steps. Steps fire
    /// in order at dispatch time; failures and stale entity IDs are logged as
    /// warnings rather than aborting the sequence.
    Sequence(Vec<SequenceStep>),
}

/// One step in a `sequence` reaction.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SequenceStep {
    pub(crate) id: EntityId,
    pub(crate) primitive: String,
    pub(crate) args: serde_json::Value,
}

/// Threshold reaction: counts kills against a tag and fires an event when the
/// kill ratio reaches `at`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProgressDescriptor {
    pub(crate) tag: String,
    pub(crate) at: f32,
    pub(crate) fire: String,
}

/// Primitive-action reaction. One descriptor shape, two execution arms (M13
/// HUD dynamics): when `tag` is `Some`, the primitive resolves the tag to
/// entities and mutates the `EntityRegistry`; when `tag` is `None`, it is a
/// **system reaction** — it targets no entities and instead enqueues a typed
/// `SystemReactionCommand` for the app's per-frame drain. The two arms share
/// one named-event namespace; the dispatcher picks the arm by `tag` presence.
///
/// `args` carries the primitive-specific payload (e.g. `{ "rate": 0.0 }` for
/// `setEmitterRate`, `{ "sound": "alarm" }` for `playSound`). Defaults to an
/// empty JSON object when the descriptor omits the field, so primitives that
/// take no args parse cleanly.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PrimitiveDescriptor {
    pub(crate) primitive: String,
    /// Entity tag to target. `None` ⇒ system-targeted (no entities).
    pub(crate) tag: Option<String>,
    pub(crate) on_complete: Option<String>,
    pub(crate) args: serde_json::Value,
}

/// A reaction descriptor paired with the event name it is registered under.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NamedReaction {
    pub(crate) name: String,
    pub(crate) descriptor: ReactionDescriptor,
}

/// The condition half of a state-crossing watcher (M13 HUD dynamics). A
/// crossing fires when the watched slot transitions across `threshold` in the
/// declared direction. `threshold` is stored as a fraction of the
/// registration's `max` (`raw_threshold / max`); the watcher compares it
/// against the slot's `current / max`, so a registration with no `max`
/// (default `1.0`) degrades to a raw-value comparison.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CrossingCondition {
    /// Fires on a downward crossing: `prev >= threshold && cur < threshold`.
    Below { threshold: f32 },
    /// Fires on an upward crossing: `prev <= threshold && cur > threshold`.
    Above { threshold: f32 },
}

/// A state-crossing watcher declared by `onStateCrossing` and carried back
/// through `setupLevel`'s manifest (scripting.md §12 (Non-Goals): no
/// side-effect FFI — cross-FFI values flow through setup-function returns).
/// The detector watches `slot` after each frame's
/// slot writes and, on a crossing in the condition's direction, dispatches
/// every event in `fire` synchronously through the named-reaction vocabulary.
///
/// `max` is the registration's denominator: thresholds are fractions of it.
/// It defaults to `1.0` (raw-value comparison) when the registration omits it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CrossingDescriptor {
    pub(crate) slot: String,
    pub(crate) condition: CrossingCondition,
    pub(crate) max: f32,
    pub(crate) fire: Vec<String>,
}
