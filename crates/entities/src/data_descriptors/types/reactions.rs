// Data-context descriptors: reaction & crossing descriptor types.
// See: context/lib/scripting.md

use crate::registry::EntityId;
use postretro_foundation::ir::IrNode;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TriggerEventDescriptor {
    pub tag: String,
    pub event: String,
    pub fire: Vec<String>,
    pub levels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TriggerPoolDescriptor {
    pub tag: String,
    pub arm: TriggerPoolArm,
    pub levels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TriggerPoolArm {
    Count(u32),
    Percentage(f64),
}

/// Variants of a single reaction's behavior body. The `name` lives on the
/// wrapping [`NamedReaction`]; this enum captures only the descriptor shape.
#[derive(Debug, Clone, PartialEq)]
pub enum ReactionDescriptor {
    Progress(ProgressDescriptor),
    Primitive(PrimitiveDescriptor),
    /// Ordered list of (entity, sequenced-primitive, args) steps. Steps fire
    /// in order at dispatch time; failures and stale entity IDs are logged as
    /// warnings rather than aborting the sequence.
    Sequence(Vec<SequenceStep>),
}

/// One step in a `sequence` reaction.
#[derive(Debug, Clone, PartialEq)]
pub struct SequenceStep {
    pub id: SequenceTarget,
    pub primitive: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SequenceTarget {
    Entity(EntityId),
    Activators,
    FiredTrigger,
}

impl From<EntityId> for SequenceTarget {
    fn from(value: EntityId) -> Self {
        Self::Entity(value)
    }
}

/// Threshold reaction: counts kills against a tag and fires an event when the
/// kill ratio reaches `at`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressDescriptor {
    pub tag: String,
    pub at: f32,
    pub fire: String,
}

/// Primitive-action reaction. One descriptor shape, two targeting arms (M13
/// HUD dynamics): when `tag` is `Some`, the primitive resolves the tag to
/// entities and mutates the `EntityRegistry`; when `tag` is `None`, it is a
/// **system reaction** and targets no entities. Targeting and execution surface
/// are separate: crossing-, named-event-, and level-fired system reactions
/// enqueue commands for the app-side drain; trigger `on_fire`/`on_exit` store
/// writes execute in the simulation tick. The two arms share one named-event
/// namespace; the dispatcher picks the targeting arm by `tag` presence.
///
/// `args` carries the primitive-specific payload (e.g. `{ "rate": 0.0 }` for
/// `setEmitterRate`, `{ "sound": "alarm" }` for `playSound`). Defaults to an
/// empty JSON object when the descriptor omits the field, so primitives that
/// take no args parse cleanly.
#[derive(Debug, Clone, PartialEq)]
pub struct PrimitiveDescriptor {
    pub primitive: String,
    /// Trigger-fire sentinel target. Mutually exclusive with `tag`.
    pub target: Option<String>,
    /// Entity tag to target. `None` ⇒ system-targeted (no entities).
    pub tag: Option<String>,
    pub on_complete: Option<String>,
    pub args: serde_json::Value,
}

/// A reaction descriptor paired with the event name it is registered under.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedReaction {
    pub name: String,
    pub descriptor: ReactionDescriptor,
}

/// The condition half of a state-crossing watcher. Threshold conditions fire
/// when their watched slot crosses a normalized edge. IR predicates fire on
/// false-to-true transitions. Thresholds are fractions of `max`; absent `max`
/// defaults to `1.0` for a raw-value comparison. See: context/lib/scripting.md
/// §12.
#[derive(Debug, Clone, PartialEq)]
pub enum CrossingCondition {
    /// Fires on a downward crossing: `prev >= threshold && cur < threshold`.
    Below { threshold: f32 },
    /// Fires on an upward crossing: `prev <= threshold && cur > threshold`.
    Above { threshold: f32 },
    /// Fires when a StoreScope-bound predicate transitions from false to true.
    /// The descriptor retains the raw foundation IR; scripting-core owns its
    /// scope-specialized bound program at runtime.
    Ir(IrNode),
}

/// A state-crossing watcher declared by `onStateCrossing` and carried back
/// through `setupLevel`'s manifest (scripting.md §12 (Non-Goals): no
/// side-effect FFI — cross-FFI values flow through setup-function returns).
/// After each frame's slot writes, the detector evaluates the condition and
/// dispatches every event in `fire` on the authored threshold/predicate edge;
/// optional `edge: "both"` also dispatches the mirrored transition.
///
/// `max` is the threshold registration's denominator. It defaults to `1.0`
/// (raw-value comparison) when the registration omits it; predicate watchers
/// ignore it.
#[derive(Debug, Clone, PartialEq)]
pub struct CrossingDescriptor {
    /// The slot watched by a threshold crossing. Predicate crossings have no
    /// single meaningful slot and carry `None` here.
    pub slot: Option<String>,
    pub condition: CrossingCondition,
    pub max: f32,
    /// Normalized optional edge mode. `Some("both")` fires both transitions;
    /// absence preserves the shipped authored-edge lifecycle.
    pub edge: Option<String>,
    pub fire: Vec<String>,
}
