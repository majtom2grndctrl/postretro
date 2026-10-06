// Player rebinding: the effect of binding one input to one command slot,
// checked against the effective table before it applies.
// See: context/lib/player_options.md §6 · context/lib/input.md §2

use super::binding_table::{
    AuthorLayer, EffectiveBinding, EffectiveTable, GUARDED, PlayerLayer, conflicts,
};
use super::commands::Command;
use super::input_names::DeviceClass;
use super::relevance::{Relevance, RelevanceFacts};
use super::types::PhysicalInput;

/// What binding an input to a command slot would do.
#[derive(Debug, Clone, PartialEq)]
pub enum RebindProposal {
    /// No conflict: apply `player` as the new player layer.
    Clean { player: PlayerLayer },
    /// The input already drives other commands live in the same context. The
    /// player chooses: `replace` takes the input from them, or cancel.
    Conflict {
        with: Vec<Command>,
        replace: PlayerLayer,
    },
    /// Applying it, or replacing, would leave a guarded command unbound on the
    /// device class (confirm, cancel, menu), so the panel refuses it.
    Refused { unbound: Command },
    /// The input is already this slot's binding: nothing changes.
    Unchanged,
}

/// The player row a slot write produces from the current effective inputs:
/// the slot takes `input`, and every other slot keeps what it shows now.
fn row_with(current: &[PhysicalInput], slot: usize, input: PhysicalInput) -> Vec<Option<PhysicalInput>> {
    let mut row: Vec<Option<PhysicalInput>> = current.iter().copied().map(Some).collect();
    if slot < row.len() {
        row[slot] = Some(input);
    } else {
        row.push(Some(input));
    }
    // One input once per row.
    let mut seen = Vec::new();
    row.retain(|entry| match entry {
        Some(input) if seen.contains(input) => false,
        Some(input) => {
            seen.push(*input);
            true
        }
        None => true,
    });
    row
}

/// Build what binding `input` to `command`'s `slot` on `class` would do.
///
/// `table` is the current effective table (built from `author`, `player`,
/// `facts`, and `swap`). The commands that hold `input` now and conflict with
/// the new binding are asked about; the replace removes the input from them.
/// Either result that leaves a guarded command unbound is refused. With the
/// swap on, a gamepad capture for confirm or cancel is stored on the
/// counterpart command, so the captured button drives the command the row
/// shows.
#[allow(clippy::too_many_arguments)]
pub fn propose_rebind(
    table: &EffectiveTable,
    author: &AuthorLayer,
    player: &PlayerLayer,
    facts: RelevanceFacts,
    swap: bool,
    command: Command,
    class: DeviceClass,
    slot: usize,
    input: PhysicalInput,
) -> RebindProposal {
    let shown = table.inputs(command, class);
    if shown.get(slot) == Some(&input) {
        return RebindProposal::Unchanged;
    }
    let stored = stored_command(command, class, swap);
    let mut proposed = player.clone();
    proposed
        .rows
        .insert((stored, class), stored_row(table, command, class, slot, input));
    let candidate = EffectiveTable::build(author, &proposed, facts, swap);

    // The new binding as the candidate resolves it: kept, or dropped by a
    // collision (the suppressed list names commands before the swap).
    let new_binding = candidate
        .entries()
        .iter()
        .find(|e| e.command == command && e.class == class && e.input == input)
        .copied()
        .or_else(|| {
            candidate
                .suppressed()
                .iter()
                .map(|(dropped, _)| *dropped)
                .find(|e| e.command == stored && e.class == class && e.input == input)
                .map(|e| EffectiveBinding { command, ..e })
        });
    let holders: Vec<Command> = new_binding
        .map(|new_binding| {
            table
                .entries()
                .iter()
                .filter(|e| e.command != command && e.class == class && e.input == input)
                .filter(|e| table.relevance(e.command) == Relevance::Relevant)
                .filter(|e| conflicts(e, &new_binding))
                .map(|e| e.command)
                .collect()
        })
        .unwrap_or_default();
    if holders.is_empty() {
        return match guard_break(&candidate, class) {
            Some(unbound) => RebindProposal::Refused { unbound },
            None => RebindProposal::Clean { player: proposed },
        };
    }
    let mut replace = proposed;
    for holder in &holders {
        let remaining = table
            .inputs(*holder, class)
            .into_iter()
            .filter(|bound| *bound != input)
            .map(Some)
            .collect();
        replace
            .rows
            .insert((stored_command(*holder, class, swap), class), remaining);
    }
    let replaced = EffectiveTable::build(author, &replace, facts, swap);
    if let Some(unbound) = guard_break(&replaced, class) {
        return RebindProposal::Refused { unbound };
    }
    RebindProposal::Conflict {
        with: holders,
        replace,
    }
}

/// With the swap on, the gamepad rows of confirm and cancel are stored on the
/// counterpart command, since the swap exchanges them after the player layer.
fn stored_command(command: Command, class: DeviceClass, swap: bool) -> Command {
    match (swap, class, command) {
        (true, DeviceClass::Gamepad, Command::NavConfirm) => Command::NavCancel,
        (true, DeviceClass::Gamepad, Command::NavCancel) => Command::NavConfirm,
        _ => command,
    }
}

/// The row stored for a slot write: what the row shows now, with the slot
/// taking `input`. With the swap on it is stored on the counterpart command.
fn stored_row(
    table: &EffectiveTable,
    command: Command,
    class: DeviceClass,
    slot: usize,
    input: PhysicalInput,
) -> Vec<Option<PhysicalInput>> {
    row_with(&table.inputs(command, class), slot, input)
}

/// The first guarded command the table leaves unbound on `class`.
fn guard_break(table: &EffectiveTable, class: DeviceClass) -> Option<Command> {
    GUARDED.into_iter().find(|command| table.inputs(*command, class).is_empty())
}

/// The player layer with `command`'s rows removed, so it follows the author
/// default again on every class. With the swap on, the gamepad row a
/// confirm/cancel row shows is stored on the counterpart command.
pub fn reset_command(player: &PlayerLayer, command: Command, swap: bool) -> PlayerLayer {
    let mut reset = player.clone();
    for class in DeviceClass::ALL {
        reset.rows.remove(&(stored_command(command, class, swap), class));
    }
    reset
}
