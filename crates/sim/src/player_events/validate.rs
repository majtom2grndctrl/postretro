//! Install-time checks on a player event's fire list. A reaction that breaks
//! one costs this source its address, never the reaction: the address still
//! runs under every other source.
//! See: context/lib/scripting.md §12 (Player events)

use std::collections::HashSet;

use postretro_entities::reactions::system_commands::{SystemReactionClass, SystemReactionKind};
use postretro_entities::{ReplicationScope, SlotTable};
use postretro_scripting_core::data_descriptors::{
    NamedReaction, ReactionDescriptor, SequenceTarget,
};
use postretro_scripting_core::data_registry::DataRegistry;
use postretro_scripting_core::player_event_scope::{
    EVENT_PLAYER_TOKEN, is_local_player_slot, plain_per_player_read, reaction_uses_event_player,
    reads_dispatch_input,
};

/// The trigger-fire occupancy input. A player-event fire binds against a scope
/// that publishes it, seeded zero, so a read would silently see 0.
const OCCUPANCY_INPUT: &str = "@occupancy";

/// Why `reaction` cannot run under a player event, or `None` when it can.
pub(super) fn fire_list_violation(
    reaction: &NamedReaction,
    data_registry: &DataRegistry,
    slot_table: &SlotTable,
) -> Option<String> {
    if let Some(rule) = direct_violation(&reaction.descriptor, slot_table) {
        return Some(rule);
    }
    // Checked on fire-list reactions only: the drain begins their origin with
    // no paired enter. A reaction reached through `fire` or `onComplete`
    // dispatches with no origin, which the trigger-coupled rows (V2, V3)
    // already govern.
    if holds_interruptible_wait(&reaction.descriptor) {
        return Some(
            "uses an interruptible `wait`, which only a trigger's exit cancels".to_string(),
        );
    }
    route_violation(reaction, data_registry, slot_table)
}

/// An interruptible `wait` parks until its paired trigger exit; a player event
/// has none, so the scheduler would refuse the tail at runtime.
fn holds_interruptible_wait(descriptor: &ReactionDescriptor) -> bool {
    let ReactionDescriptor::Sequence(steps) = descriptor else {
        return false;
    };
    steps.iter().any(|step| {
        matches!(step.id, SequenceTarget::Wait)
            && step
                .args
                .get("interruptible")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
    })
}

/// What no player-event reaction may do itself. Presentation is allowed: the
/// fire routes it to the event player's machine.
fn direct_violation(descriptor: &ReactionDescriptor, slot_table: &SlotTable) -> Option<String> {
    if let Some(slot) = plain_per_player_read(descriptor, slot_table) {
        if is_local_player_slot(&slot) {
            return Some(format!(
                "reads `{slot}`, which each machine publishes for its own player; a player event cannot read it for the event's player"
            ));
        }
        return Some(format!(
            "reads per-player slot `{slot}` without an owner, which outside the event's condition names no player; read it with `byPlayer(on.player)`"
        ));
    }
    if let Some(token) = trigger_token(descriptor) {
        return Some(format!("uses `{token}`, which only trigger events publish"));
    }
    if reads_occupancy(descriptor) {
        return Some("uses `on.occupancy`, which only trigger events publish".to_string());
    }
    machine_local_effect(descriptor, slot_table)
}

/// A UI-stack verb, a text edit, a cell write, or a `setState` on a slot that
/// does not replicate: each would land on the host's screen.
fn machine_local_effect(descriptor: &ReactionDescriptor, slot_table: &SlotTable) -> Option<String> {
    let ReactionDescriptor::Primitive(primitive) = descriptor else {
        return None;
    };
    match SystemReactionKind::from_primitive_name(&primitive.primitive)?.class() {
        SystemReactionClass::MachineLocal => Some(format!(
            "`{}` is machine-local: fired on the host it opens on the host's screen, not the event player's",
            primitive.primitive
        )),
        SystemReactionClass::SlotWrite => {
            let slot = primitive.args.get("slot")?.as_str()?;
            let replicates = slot_table
                .get(slot)
                .is_some_and(|record| record.schema.network != ReplicationScope::None);
            (!replicates).then(|| {
                format!(
                    "`setState` writes `{slot}`, which does not replicate, so the write stays on the host"
                )
            })
        }
        SystemReactionClass::Presentation | SystemReactionClass::HostConsequence => None,
    }
}

fn reads_occupancy(descriptor: &ReactionDescriptor) -> bool {
    match descriptor {
        ReactionDescriptor::Primitive(primitive) => {
            reads_dispatch_input(&primitive.args, OCCUPANCY_INPUT)
        }
        ReactionDescriptor::Sequence(steps) => steps
            .iter()
            .any(|step| reads_dispatch_input(&step.args, OCCUPANCY_INPUT)),
        ReactionDescriptor::Progress(_) => false,
    }
}

fn trigger_token(descriptor: &ReactionDescriptor) -> Option<&'static str> {
    match descriptor {
        ReactionDescriptor::Primitive(primitive) => match primitive.target.as_deref() {
            Some("@activators") => Some("on.activators"),
            Some("@trigger") => Some("on.trigger"),
            _ => None,
        },
        ReactionDescriptor::Sequence(steps) => steps.iter().find_map(|step| match step.id {
            SequenceTarget::Activators => Some("on.activators"),
            SequenceTarget::FiredTrigger => Some("on.trigger"),
            _ => None,
        }),
        ReactionDescriptor::Progress(_) => None,
    }
}

/// `fire` steps and completion follow-ups dispatch with no context, so
/// whatever they reach runs on the host as nobody's reaction. Walk both, at
/// any depth, from `start`.
fn route_violation(
    start: &NamedReaction,
    data_registry: &DataRegistry,
    slot_table: &SlotTable,
) -> Option<String> {
    let mut pending: Vec<String> = context_free_routes(&start.descriptor).collect();
    let mut visited: HashSet<String> = HashSet::new();
    while let Some(address) = pending.pop() {
        if !visited.insert(address.clone()) {
            continue;
        }
        for reached in data_registry
            .reactions
            .iter()
            .filter(|reaction| reaction.name == address)
        {
            if let Some(rule) = reached_violation(&reached.descriptor, slot_table) {
                return Some(format!(
                    "reaches reaction `{}` through `fire` or `onComplete`, which dispatch without the event player, and `{}` {rule}",
                    reached.name, reached.name
                ));
            }
            pending.extend(context_free_routes(&reached.descriptor));
        }
    }
    None
}

/// What a reaction reached without context may not hold: presentation (it
/// would play on the host), a machine-local effect, a plain per-player read,
/// or `on.player` itself.
fn reached_violation(descriptor: &ReactionDescriptor, slot_table: &SlotTable) -> Option<String> {
    if let ReactionDescriptor::Primitive(primitive) = descriptor
        && SystemReactionKind::from_primitive_name(&primitive.primitive)
            .is_some_and(|kind| kind.class() == SystemReactionClass::Presentation)
    {
        return Some(format!(
            "presents `{}`, which would play on the host; list it in the event's fire list instead",
            primitive.primitive
        ));
    }
    if reaction_uses_event_player(descriptor) {
        return Some(format!(
            "uses `{EVENT_PLAYER_TOKEN}`, which a context-free dispatch does not carry"
        ));
    }
    direct_violation(descriptor, slot_table)
}

fn context_free_routes(descriptor: &ReactionDescriptor) -> Box<dyn Iterator<Item = String> + '_> {
    match descriptor {
        ReactionDescriptor::Primitive(primitive) => Box::new(primitive.on_complete.iter().cloned()),
        ReactionDescriptor::Sequence(steps) => Box::new(steps.iter().filter_map(|step| {
            matches!(step.id, SequenceTarget::Fire)
                .then(|| step.args.get("event")?.as_str().map(str::to_string))
                .flatten()
        })),
        // A `progress` fires its target from the tracker, not this event.
        ReactionDescriptor::Progress(_) => Box::new(std::iter::empty()),
    }
}
