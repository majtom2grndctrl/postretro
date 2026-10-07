// Player rebinding: the effect of binding one input to one command slot, or
// of resetting a command, checked against the effective table before it
// applies.
// See: context/lib/player_options.md §6 · context/lib/input.md §2

use super::binding_table::{
    AuthorLayer, BindingOrigin, EffectiveBinding, EffectiveTable, GUARDED, PlayerLayer, conflicts,
    swapped_command,
};
use super::commands::Command;
use super::input_names::DeviceClass;
use super::relevance::{Relevance, RelevanceFacts};
use super::types::PhysicalInput;

/// What binding an input to a command slot, or resetting a command, would do.
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
    /// Applying it, or replacing, would leave a guarded command (confirm,
    /// cancel, menu) unbound on `class`, so the panel refuses it.
    Refused {
        unbound: Command,
        class: DeviceClass,
    },
    /// Nothing would change.
    Unchanged,
}

/// The player row a slot write produces from the current effective inputs:
/// the slot takes `input`, and every other slot keeps what it shows now. An
/// input already in another slot of the row trades places with the slot's
/// input, so neither binding is lost.
fn row_with(
    current: &[PhysicalInput],
    slot: usize,
    input: PhysicalInput,
) -> Vec<Option<PhysicalInput>> {
    let mut row = current.to_vec();
    match row.iter().position(|bound| *bound == input) {
        Some(from) if slot < row.len() => row.swap(from, slot),
        // Bound already, and the slot is past the row's end: nothing moves.
        Some(_) => {}
        None if slot < row.len() => row[slot] = input,
        None => row.push(input),
    }
    row.into_iter().map(Some).collect()
}

/// Build what binding `input` to `command`'s `slot` on `class` would do.
///
/// `table` is the current effective table (built from `author`, `player`,
/// `facts`, and `swap`). The commands that hold `input` now and conflict with
/// the new binding are asked about, as is a saved row of a command the game
/// does not use now, which would collide once it does; the replace removes
/// the input from them. Either result that newly leaves a guarded command
/// unbound is refused. With the swap on, a gamepad capture for confirm or
/// cancel is stored on the counterpart command, so the captured button drives
/// the command the row shows.
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
    let row = row_with(&shown, slot, input);
    if row.iter().copied().eq(shown.iter().copied().map(Some)) {
        return RebindProposal::Unchanged;
    }
    let mut proposed = player.clone();
    proposed
        .rows
        .insert((swapped_command(command, class, swap), class), row);
    let candidate = EffectiveTable::build(author, &proposed, facts, swap);

    // The new binding as the candidate resolves it: kept, or dropped by a
    // collision. The table names commands after the swap, as the row does.
    let new_binding = candidate
        .entries()
        .iter()
        .chain(candidate.suppressed().iter().map(|(dropped, _)| dropped))
        .find(|e| e.command == command && e.class == class && e.input == input)
        .copied();
    let holders = new_binding
        .map(|new_binding| holders_of(table, author, player, swap, &new_binding))
        .unwrap_or_default();
    if holders.is_empty() {
        return match new_guard_break(&candidate, table) {
            Some((unbound, class)) => RebindProposal::Refused { unbound, class },
            None => RebindProposal::Clean { player: proposed },
        };
    }
    let mut replace = proposed;
    for holder in &holders {
        let key = (swapped_command(*holder, class, swap), class);
        let live = table.relevance(*holder) == Relevance::Relevant;
        let remaining: Vec<Option<PhysicalInput>> = if live {
            table
                .inputs(*holder, class)
                .into_iter()
                .filter(|bound| *bound != input)
                .map(Some)
                .collect()
        } else {
            // A dormant saved row gives up just this input.
            player
                .rows
                .get(&key)
                .map(|row| {
                    row.iter()
                        .copied()
                        .filter(|stored| *stored != Some(input))
                        .collect()
                })
                .unwrap_or_default()
        };
        replace.rows.insert(key, remaining);
    }
    let replaced = EffectiveTable::build(author, &replace, facts, swap);
    if let Some((unbound, class)) = new_guard_break(&replaced, table) {
        return RebindProposal::Refused { unbound, class };
    }
    RebindProposal::Conflict {
        with: holders,
        replace,
    }
}

/// The commands that hold `new_binding`'s input and conflict with it: live
/// ones in `table`, and saved player rows of commands the game does not use
/// now. Those rows apply again when their command becomes relevant, so they
/// count as holders rather than waking into a collision later.
fn holders_of(
    table: &EffectiveTable,
    author: &AuthorLayer,
    player: &PlayerLayer,
    swap: bool,
    new_binding: &EffectiveBinding,
) -> Vec<Command> {
    let held_here = |e: &&EffectiveBinding| {
        e.command != new_binding.command
            && e.class == new_binding.class
            && e.input == new_binding.input
            && conflicts(e, new_binding)
    };
    let mut holders: Vec<Command> = table
        .entries()
        .iter()
        .filter(held_here)
        .filter(|e| table.relevance(e.command) == Relevance::Relevant)
        .map(|e| e.command)
        .collect();
    let dormant = EffectiveTable::build(author, player, RelevanceFacts::EVERY, swap);
    for e in dormant.entries().iter().filter(held_here) {
        if e.origin == BindingOrigin::Player
            && table.relevance(e.command) == Relevance::Irrelevant
            && dormant.relevance(e.command) == Relevance::Relevant
            && !holders.contains(&e.command)
        {
            holders.push(e.command);
        }
    }
    holders
}

/// Whether a guarded command is unbound on `class`, or bound only because its
/// default was given back over the player's rows.
fn guard_broken(table: &EffectiveTable, command: Command, class: DeviceClass) -> bool {
    table.inputs(command, class).is_empty() || table.guard_restored().contains(&(command, class))
}

/// The first guarded command `candidate` leaves unbound that `current` keeps
/// bound. A break `current` already has (a hand-edited settings file) never
/// refuses an unrelated change.
fn new_guard_break(
    candidate: &EffectiveTable,
    current: &EffectiveTable,
) -> Option<(Command, DeviceClass)> {
    DeviceClass::ALL.into_iter().find_map(|class| {
        GUARDED
            .into_iter()
            .find(|command| {
                guard_broken(candidate, *command, class) && !guard_broken(current, *command, class)
            })
            .map(|command| (command, class))
    })
}

/// Check a player layer against the guard before it applies: `Clean` to
/// apply it, or `Refused` when it newly leaves a guarded command unbound. A
/// conflict dialog's replace runs through this when the player answers, since
/// the table may have changed (a hot reload) while the dialog was open.
pub fn check_player_layer(
    table: &EffectiveTable,
    author: &AuthorLayer,
    player: PlayerLayer,
    facts: RelevanceFacts,
    swap: bool,
) -> RebindProposal {
    let candidate = EffectiveTable::build(author, &player, facts, swap);
    match new_guard_break(&candidate, table) {
        Some((unbound, class)) => RebindProposal::Refused { unbound, class },
        None => RebindProposal::Clean { player },
    }
}

/// Remove `command`'s rows, so it follows the author default again on every
/// class, checked against the guard: a default that another command's player
/// binding holds would leave the command unbound. With the swap on, the
/// gamepad row a confirm/cancel row shows is stored on the counterpart
/// command.
pub fn reset_command(
    table: &EffectiveTable,
    author: &AuthorLayer,
    player: &PlayerLayer,
    facts: RelevanceFacts,
    swap: bool,
    command: Command,
) -> RebindProposal {
    let mut reset = player.clone();
    for class in DeviceClass::ALL {
        reset
            .rows
            .remove(&(swapped_command(command, class, swap), class));
    }
    if reset == *player {
        return RebindProposal::Unchanged;
    }
    check_player_layer(table, author, reset, facts, swap)
}

#[cfg(test)]
mod tests {
    use gilrs::Button;
    use winit::keyboard::KeyCode;

    use super::*;
    use crate::input::binding_table::AuthorBinding;
    use crate::input::types::{Activator, ActivatorKind};

    const KBM: DeviceClass = DeviceClass::KeyboardMouse;
    const PAD: DeviceClass = DeviceClass::Gamepad;

    fn key(code: KeyCode) -> PhysicalInput {
        PhysicalInput::Key(code)
    }

    fn pad(button: Button) -> PhysicalInput {
        PhysicalInput::GamepadButton(button)
    }

    fn propose(
        author: &AuthorLayer,
        player: &PlayerLayer,
        facts: RelevanceFacts,
        command: Command,
        class: DeviceClass,
        slot: usize,
        input: PhysicalInput,
    ) -> RebindProposal {
        let table = EffectiveTable::build(author, player, facts, false);
        propose_rebind(
            &table, author, player, facts, false, command, class, slot, input,
        )
    }

    fn applied(proposal: RebindProposal) -> PlayerLayer {
        match proposal {
            RebindProposal::Clean { player } => player,
            RebindProposal::Conflict { replace, .. } => replace,
            other => panic!("expected a change, got {other:?}"),
        }
    }

    #[test]
    fn capturing_an_input_from_another_slot_of_the_row_swaps_the_two() {
        let author = AuthorLayer::default();
        let mut player = PlayerLayer::default();
        player.rows.insert(
            (Command::Jump, KBM),
            vec![Some(key(KeyCode::Space)), Some(key(KeyCode::KeyJ))],
        );
        let proposal = propose(
            &author,
            &player,
            RelevanceFacts::EVERY,
            Command::Jump,
            KBM,
            1,
            key(KeyCode::Space),
        );
        let player = applied(proposal);
        let table = EffectiveTable::build(&author, &player, RelevanceFacts::EVERY, false);
        assert_eq!(
            table.inputs(Command::Jump, KBM),
            [key(KeyCode::KeyJ), key(KeyCode::Space)],
            "KeyJ moves to slot 1 rather than vanishing"
        );
    }

    #[test]
    fn replacing_an_input_keeps_each_remaining_default_on_its_own_activator() {
        // `use` defaults to E (press) and F (hold); replacing E elsewhere packs
        // F into slot 1, and F stays a hold.
        let mut author = AuthorLayer::default();
        author.defaults.insert(
            (Command::Use, KBM),
            vec![
                AuthorBinding {
                    input: key(KeyCode::KeyE),
                    activator: Activator::PRESS,
                },
                AuthorBinding {
                    input: key(KeyCode::KeyF),
                    activator: Activator::new(ActivatorKind::Hold),
                },
            ],
        );
        author.manifest_order.push(Command::Use);
        let player = PlayerLayer::default();
        let proposal = propose(
            &author,
            &player,
            RelevanceFacts::EVERY,
            Command::Jump,
            KBM,
            0,
            key(KeyCode::KeyE),
        );
        let RebindProposal::Conflict { with, replace } = proposal.clone() else {
            panic!("E drives use, so the player is asked: {proposal:?}");
        };
        assert_eq!(with, [Command::Use]);
        let table = EffectiveTable::build(&author, &replace, RelevanceFacts::EVERY, false);
        let use_bindings: Vec<_> = table
            .entries()
            .iter()
            .filter(|e| e.command == Command::Use && e.class == KBM)
            .map(|e| (e.input, e.activator.kind))
            .collect();
        assert_eq!(use_bindings, [(key(KeyCode::KeyF), ActivatorKind::Hold)]);
    }

    #[test]
    fn a_wheel_notch_captured_into_a_hold_slot_binds_as_a_press() {
        let mut author = AuthorLayer::default();
        author.defaults.insert(
            (Command::Sprint, KBM),
            vec![AuthorBinding {
                input: key(KeyCode::ShiftLeft),
                activator: Activator::new(ActivatorKind::Hold),
            }],
        );
        let player = applied(propose(
            &author,
            &PlayerLayer::default(),
            RelevanceFacts::EVERY,
            Command::Sprint,
            KBM,
            0,
            PhysicalInput::MouseWheelDown,
        ));
        let table = EffectiveTable::build(&author, &player, RelevanceFacts::EVERY, false);
        let wheel = table
            .entries()
            .iter()
            .find(|e| e.command == Command::Sprint && e.input == PhysicalInput::MouseWheelDown)
            .expect("the wheel binds to sprint");
        assert_eq!(wheel.activator, Activator::PRESS);
    }

    #[test]
    fn a_dormant_saved_row_on_the_input_is_asked_about_and_replace_clears_it() {
        // Dash is irrelevant now, but the player's saved dash row holds V.
        let author = AuthorLayer::default();
        let mut player = PlayerLayer::default();
        player
            .rows
            .insert((Command::Dash, KBM), vec![Some(key(KeyCode::KeyV))]);
        let no_dash = RelevanceFacts {
            dash: false,
            ..RelevanceFacts::EVERY
        };
        let proposal = propose(
            &author,
            &player,
            no_dash,
            Command::Jump,
            KBM,
            0,
            key(KeyCode::KeyV),
        );
        let RebindProposal::Conflict { with, replace } = proposal.clone() else {
            panic!("the dormant dash row holds V: {proposal:?}");
        };
        assert_eq!(with, [Command::Dash]);
        assert_eq!(replace.rows[&(Command::Dash, KBM)], Vec::new());
        // Once dash returns, jump keeps V and nothing collides.
        let table = EffectiveTable::build(&author, &replace, RelevanceFacts::EVERY, false);
        assert_eq!(table.inputs(Command::Jump, KBM), [key(KeyCode::KeyV)]);
        assert!(table.displaced().is_empty());
    }

    #[test]
    fn reset_that_would_leave_cancel_unbound_is_refused() {
        // Cancel gave East to confirm and kept North; resetting cancel brings
        // back its East default, which confirm's player binding holds.
        let author = AuthorLayer::default();
        let mut player = PlayerLayer::default();
        player.rows.insert(
            (Command::NavConfirm, PAD),
            vec![Some(pad(Button::South)), Some(pad(Button::East))],
        );
        player
            .rows
            .insert((Command::NavCancel, PAD), vec![Some(pad(Button::North))]);
        let facts = RelevanceFacts::EVERY;
        let table = EffectiveTable::build(&author, &player, facts, false);
        assert!(table.guard_restored().is_empty());
        assert_eq!(
            reset_command(&table, &author, &player, facts, false, Command::NavCancel),
            RebindProposal::Refused {
                unbound: Command::NavCancel,
                class: PAD
            }
        );
        // Resetting confirm instead is fine.
        assert!(matches!(
            reset_command(&table, &author, &player, facts, false, Command::NavConfirm),
            RebindProposal::Clean { .. }
        ));
    }

    #[test]
    fn a_stale_replace_is_refused_when_it_would_unbind_a_guarded_command() {
        let author = AuthorLayer::default();
        let facts = RelevanceFacts::EVERY;
        let table = EffectiveTable::build(&author, &PlayerLayer::default(), facts, false);
        let mut stale = PlayerLayer::default();
        stale.rows.insert((Command::NavCancel, PAD), Vec::new());
        assert_eq!(
            check_player_layer(&table, &author, stale, facts, false),
            RebindProposal::Refused {
                unbound: Command::NavCancel,
                class: PAD
            }
        );
    }

    #[test]
    fn a_guard_break_already_in_the_table_never_refuses_an_unrelated_capture() {
        // A hand-edited empty cancel row: the table gives cancel its default
        // back, and capturing jump on the gamepad still applies.
        let author = AuthorLayer::default();
        let mut player = PlayerLayer::default();
        player.rows.insert((Command::NavCancel, PAD), Vec::new());
        let proposal = propose(
            &author,
            &player,
            RelevanceFacts::EVERY,
            Command::Jump,
            PAD,
            1,
            pad(Button::DPadUp),
        );
        assert!(
            matches!(proposal, RebindProposal::Clean { .. }),
            "{proposal:?}"
        );
    }
}
