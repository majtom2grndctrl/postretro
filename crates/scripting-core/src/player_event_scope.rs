// What a reaction needs from a player event's dispatch scope: the `@player`
// token as a command target or a read owner, and plain reads of per-player
// slots, which mean the evaluated player only inside a condition.
// See: context/lib/scripting.md §12 (Player events)

use crate::data_descriptors::{ReactionDescriptor, SequenceTarget};
use crate::player_slots::PlayerSlot;
use crate::slot_table::SlotTable;

/// The wire token for `on.player`, as a `target` and as an IR read `owner`.
pub const EVENT_PLAYER_TOKEN: &str = "@player";

/// Visit every IR input leaf (`{ op: "input", name, owner? }`) in a
/// primitive's raw args, depth first.
fn for_each_input<'a>(
    value: &'a serde_json::Value,
    visit: &mut impl FnMut(&'a str, Option<&'a str>),
) {
    match value {
        serde_json::Value::Object(map) => {
            if map.get("op").and_then(serde_json::Value::as_str) == Some("input")
                && let Some(name) = map.get("name").and_then(serde_json::Value::as_str)
            {
                visit(name, map.get("owner").and_then(serde_json::Value::as_str));
            }
            for child in map.values() {
                for_each_input(child, visit);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                for_each_input(item, visit);
            }
        }
        _ => {}
    }
}

/// Whether `args` hold a `byPlayer(on.player)` read.
pub fn reads_event_player(args: &serde_json::Value) -> bool {
    let mut found = false;
    for_each_input(args, &mut |_, owner| {
        found |= owner == Some(EVENT_PLAYER_TOKEN)
    });
    found
}

/// Whether a reaction body names `on.player` anywhere: as a target, a
/// sequence step's subject, or a read owner.
pub fn reaction_uses_event_player(descriptor: &ReactionDescriptor) -> bool {
    match descriptor {
        ReactionDescriptor::Primitive(primitive) => {
            primitive.target.as_deref() == Some(EVENT_PLAYER_TOKEN)
                || reads_event_player(&primitive.args)
        }
        ReactionDescriptor::Sequence(steps) => steps.iter().any(|step| {
            matches!(step.id, SequenceTarget::EventPlayer) || reads_event_player(&step.args)
        }),
        ReactionDescriptor::Progress(_) => false,
    }
}

/// Whether `name` is a per-player slot: an engine per-player slot or a mod
/// per-owner slot.
pub fn is_per_player_slot(name: &str, slot_table: &SlotTable) -> bool {
    PlayerSlot::from_name(name).is_some()
        || slot_table
            .get(name)
            .is_some_and(|record| record.schema.per_owner)
}

/// The first per-player slot a reaction body reads without an owner. Outside
/// a condition such a read has an implicit owner, the wrong-owner bug.
pub fn plain_per_player_read(
    descriptor: &ReactionDescriptor,
    slot_table: &SlotTable,
) -> Option<String> {
    let mut first = None;
    let mut visit = |name: &str, owner: Option<&str>| {
        if first.is_none() && owner.is_none() && is_per_player_slot(name, slot_table) {
            first = Some(name.to_string());
        }
    };
    match descriptor {
        ReactionDescriptor::Primitive(primitive) => for_each_input(&primitive.args, &mut visit),
        ReactionDescriptor::Sequence(steps) => {
            for step in steps {
                for_each_input(&step.args, &mut visit);
            }
        }
        ReactionDescriptor::Progress(_) => {}
    }
    first
}
