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
    /// Ordered reaction steps. Entity-targeted primitive steps run in order;
    /// `fire` queues a named dispatch, while `wait` parks the remaining tail and
    /// ends the current drain. Primitive failures and stale entity IDs warn
    /// without aborting later steps in the same segment.
    Sequence(Vec<SequenceStep>),
}

/// One entity-targeted primitive step or `wait`/`fire` control step in a
/// `sequence` reaction.
#[derive(Debug, Clone, PartialEq)]
pub struct SequenceStep {
    pub id: SequenceTarget,
    pub primitive: String,
    pub args: serde_json::Value,
}

/// The entity kinds whose membership changes after install, so they are
/// addressed as groups resolved when a command takes effect. Wire spelling is
/// `"npc"` / `"player"` (`kind` on a primitive descriptor or sequence entry).
/// See: context/lib/scripting.md §12 (Entity addressing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupKind {
    /// Brain-driven characters that are not players, whatever their faction
    /// sentiment.
    Npc,
    /// Seat-bound player pawns; in single player, the local pawn.
    Player,
}

impl GroupKind {
    /// Parse the manifest spelling. `None` for any other value, so both VM
    /// converters reject an unknown kind with the same diagnostic.
    pub fn from_wire(spelling: &str) -> Option<Self> {
        match spelling {
            "npc" => Some(Self::Npc),
            "player" => Some(Self::Player),
            _ => None,
        }
    }

    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Npc => "npc",
            Self::Player => "player",
        }
    }
}

/// A group command target: every live entity of `kind`, filtered by `tag`
/// when present. Resolved on the draining machine each time the command takes
/// effect — never at install.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupTarget {
    pub kind: GroupKind,
    pub tag: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SequenceTarget {
    Entity(EntityId),
    /// Group step: resolved against the registry when the step runs, so a
    /// step after a `wait` sees whoever exists at landing.
    Group(GroupTarget),
    Activators,
    FiredTrigger,
    /// Control step: `wait`. Enrolls the remainder of the body with the host-only
    /// reaction scheduler and stops the drain. Payload-free — `durationMs` and
    /// `interruptible` ride in the step's `args`.
    Wait,
    /// Control step: `fire`. Dispatches a named reaction on the same app drain.
    /// Payload-free — the target `event` name rides in the step's `args`.
    Fire,
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
    /// Trigger-fire sentinel target. Mutually exclusive with `tag` and `kind`.
    pub target: Option<String>,
    /// Group kind. `Some` ⇒ a group command: resolution covers only that kind,
    /// filtered by `tag` when present, and runs on host / single player only.
    /// `None` ⇒ the raw kindless path: a `Some` tag scans every tagged
    /// `Transform` entity, a `None` tag is a system reaction.
    pub kind: Option<GroupKind>,
    /// Entity tag to target. With no `kind`, `None` ⇒ system-targeted (no
    /// entities); with a `kind`, an optional filter.
    pub tag: Option<String>,
    pub on_complete: Option<String>,
    pub args: serde_json::Value,
}

impl PrimitiveDescriptor {
    /// The group this descriptor addresses, when it carries a `kind`.
    pub fn group_target(&self) -> Option<GroupTarget> {
        self.kind.map(|kind| GroupTarget {
            kind,
            tag: self.tag.clone(),
        })
    }
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
