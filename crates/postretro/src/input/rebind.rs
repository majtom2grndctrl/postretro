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
    /// The input already drives other commands live in the same context, or
    /// a saved row of a command the game does not use now holds it. The
    /// player chooses: `replace` takes the input from them, or cancel.
    Conflict {
        with: Vec<Command>,
        replace: PlayerLayer,
    },
    /// Applying it, or replacing, would leave a guarded command (confirm,
    /// cancel, menu) unbound on `class`, or the new binding would only lose
    /// its input back to that command's guarded default, so the panel
    /// refuses it.
    Refused {
        unbound: Command,
        class: DeviceClass,
    },
    /// Nothing would change.
    Unchanged,
}

/// The layers a rebind is checked against: the current effective table and
/// what it was built from.
#[derive(Clone, Copy)]
struct Layers<'a> {
    table: &'a EffectiveTable,
    author: &'a AuthorLayer,
    player: &'a PlayerLayer,
    facts: RelevanceFacts,
    swap: bool,
}

impl<'a> Layers<'a> {
    /// The saved row `command` shows on `class`, with the row slot behind each
    /// shown input. `None` when the command has no saved row there, or when
    /// the row is broken and the table shows the guarded default given back
    /// over it, which no row slot is behind.
    fn saved_row(
        &self,
        command: Command,
        class: DeviceClass,
    ) -> Option<(&'a [Option<PhysicalInput>], Vec<usize>)> {
        if self.table.guard_restored().contains(&(command, class)) {
            return None;
        }
        let player: &'a PlayerLayer = self.player;
        let row = player
            .rows
            .get(&(swapped_command(command, class, self.swap), class))?;
        Some((row.as_slice(), self.table.slots(command, class)))
    }
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

/// `row_with` over a saved row: shown slot `slot` maps through `slots` to the
/// row slot it shows, which takes `input`. Every other row slot keeps what it
/// holds, so an unreadable slot survives with its saved string, and no slot
/// moves: a write past the shown slots appends. An input the row holds
/// already (saved, or shown as an unreadable slot's fallback) trades places
/// with what the slot shows. Only a write to an unreadable slot itself, or a
/// trade with the fallback it shows, replaces it, in place.
fn saved_row_with(
    row: &[Option<PhysicalInput>],
    slots: &[usize],
    shown: &[PhysicalInput],
    slot: usize,
    input: PhysicalInput,
) -> Vec<Option<PhysicalInput>> {
    let mut edited = row.to_vec();
    let from = row
        .iter()
        .position(|stored| *stored == Some(input))
        .or_else(|| {
            let shown_at = shown.iter().position(|bound| *bound == input)?;
            slots.get(shown_at).copied()
        });
    match (from, slots.get(slot).copied()) {
        (Some(from), Some(target)) => {
            edited[from] = shown.get(slot).copied().or(row[target]);
            edited[target] = Some(input);
        }
        // Held already, and the slot is past what the row shows: nothing
        // moves.
        (Some(_), None) => {}
        (None, Some(target)) => edited[target] = Some(input),
        (None, None) => edited.push(Some(input)),
    }
    edited
}

/// Build what binding `input` to `command`'s `slot` on `class` would do.
///
/// `table` is the current effective table (built from `author`, `player`,
/// `facts`, and `swap`). The commands that hold `input` now and conflict with
/// the new binding are asked about, as is a saved row of a command the game
/// does not use now, which would collide once it does; the replace removes
/// the input from them. Either result that newly leaves a guarded command
/// unbound is refused, as is one whose replace would empty a guarded command's
/// row: a guarded row is never written empty. With the swap on, a gamepad
/// capture for confirm or cancel is stored on the counterpart command, so the
/// captured button drives the command the row shows. Edits apply to the saved
/// rows, so an unreadable slot the edit does not reach keeps its saved string.
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
    let layers = Layers {
        table,
        author,
        player,
        facts,
        swap,
    };
    let shown = table.inputs(command, class);
    if shown.get(slot) == Some(&input) {
        return keep_shown(layers, command, class, &shown);
    }
    let (current, row) = match layers.saved_row(command, class) {
        Some((saved, slots)) => (
            saved.to_vec(),
            saved_row_with(saved, &slots, &shown, slot, input),
        ),
        None => (
            shown.iter().copied().map(Some).collect(),
            row_with(&shown, slot, input),
        ),
    };
    // An input the row holds but a collision drops still goes through the
    // checks below, so the player learns what holds it.
    if row == current && shown.contains(&input) {
        return keep_shown(layers, command, class, &shown);
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
        let refusal = new_guard_break(&candidate, table)
            .or_else(|| lands_behind_guard(&candidate, command, class, input));
        return match refusal {
            Some((unbound, class)) => RebindProposal::Refused { unbound, class },
            None => RebindProposal::Clean { player: proposed },
        };
    }
    let mut replace = proposed;
    for holder in &holders {
        let key = (swapped_command(*holder, class, swap), class);
        let live = table.relevance(*holder) == Relevance::Relevant;
        // A saved row gives up just this input: only its readable slots
        // holding it go. An unreadable slot whose fallback is the input stays
        // saved; the new binding beats that fallback, which flags the holder.
        let saved = if live {
            layers.saved_row(*holder, class).map(|(row, _)| row)
        } else {
            player.rows.get(&key).map(Vec::as_slice)
        };
        let remaining: Vec<Option<PhysicalInput>> = match saved {
            Some(row) => row
                .iter()
                .copied()
                .filter(|stored| *stored != Some(input))
                .collect(),
            None if live => table
                .inputs(*holder, class)
                .into_iter()
                .filter(|bound| *bound != input)
                .map(Some)
                .collect(),
            None => Vec::new(),
        };
        // Taking a guarded command's last input would write its row empty.
        // A break the current table already masks (its default given back
        // over the player's rows) would pass the guard check below, so this
        // refuses before the row exists.
        if remaining.is_empty() && GUARDED.contains(holder) {
            return RebindProposal::Refused {
                unbound: *holder,
                class,
            };
        }
        replace.rows.insert(key, remaining);
    }
    let replaced = EffectiveTable::build(author, &replace, facts, swap);
    let refusal = new_guard_break(&replaced, table)
        .or_else(|| lands_behind_guard(&replaced, command, class, input));
    if let Some((unbound, class)) = refusal {
        return RebindProposal::Refused { unbound, class };
    }
    RebindProposal::Conflict {
        with: holders,
        replace,
    }
}

/// Pressing an input the row already shows keeps the row. When the row shows a
/// guarded command's default given back over the command's own broken saved
/// row (a hand edit), keeping writes the shown row, so the next save replaces
/// the broken row instead of it being restored, with a warning, every load.
/// The whole broken row goes, unreadable slots included: the given-back
/// default is what the player kept, and no row slot is behind it.
fn keep_shown(
    layers: Layers<'_>,
    command: Command,
    class: DeviceClass,
    shown: &[PhysicalInput],
) -> RebindProposal {
    let key = (swapped_command(command, class, layers.swap), class);
    if !layers.table.guard_restored().contains(&(command, class))
        || !layers.player.rows.contains_key(&key)
    {
        return RebindProposal::Unchanged;
    }
    let mut kept = layers.player.clone();
    kept.rows
        .insert(key, shown.iter().copied().map(Some).collect());
    if kept == *layers.player {
        return RebindProposal::Unchanged;
    }
    check_player_layer(
        layers.table,
        layers.author,
        kept,
        layers.facts,
        layers.swap,
        None,
    )
}

/// The guarded command whose given-back default drops the new binding in
/// `candidate`: the input would bind only to be taken back. This catches a
/// restore the current table already has, which `new_guard_break` ignores.
fn lands_behind_guard(
    candidate: &EffectiveTable,
    command: Command,
    class: DeviceClass,
    input: PhysicalInput,
) -> Option<(Command, DeviceClass)> {
    candidate
        .suppressed()
        .iter()
        .find(|(lost, winner)| {
            lost.command == command
                && lost.class == class
                && lost.input == input
                && candidate
                    .guard_restored()
                    .contains(&(winner.command, winner.class))
        })
        .map(|(_, winner)| (winner.command, winner.class))
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
    // The dormant table is a full rebuild; skip it unless a saved row belongs
    // to a command the game does not use now.
    let any_dormant_row = player
        .rows
        .keys()
        .any(|(command, _)| table.relevance(*command) == Relevance::Irrelevant);
    if !any_dormant_row {
        return holders;
    }
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
/// refuses an unrelated change; it also never reads as newly broken, so
/// `propose_rebind` refuses the masked cases (an emptied guarded row, a
/// binding that lands behind the given-back default) on its own.
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
/// apply it, or `Refused` when it newly leaves a guarded command unbound, or
/// when `added` (the binding the layer adds: command, class, input) would
/// only land behind a guarded default given back. A conflict dialog's replace
/// runs through this when the player answers, since the table may have
/// changed (a hot reload) while the dialog was open.
pub fn check_player_layer(
    table: &EffectiveTable,
    author: &AuthorLayer,
    player: PlayerLayer,
    facts: RelevanceFacts,
    swap: bool,
    added: Option<(Command, DeviceClass, PhysicalInput)>,
) -> RebindProposal {
    let candidate = EffectiveTable::build(author, &player, facts, swap);
    let refusal = new_guard_break(&candidate, table).or_else(|| {
        let (command, class, input) = added?;
        lands_behind_guard(&candidate, command, class, input)
    });
    match refusal {
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
    check_player_layer(table, author, reset, facts, swap, None)
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
            check_player_layer(&table, &author, stale, facts, false, None),
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

    // Regression: with menu's Start already given back over jump's saved row,
    // capturing Start on shoot asked about menu, and REPLACE wrote an empty
    // menu row that passed the guard check and lost shoot's Start again.
    #[test]
    fn taking_the_input_a_given_back_menu_needs_is_refused_and_writes_no_empty_row() {
        let author = AuthorLayer::default();
        let mut player = PlayerLayer::default();
        player
            .rows
            .insert((Command::Jump, PAD), vec![Some(pad(Button::Start))]);
        let facts = RelevanceFacts::EVERY;
        let table = EffectiveTable::build(&author, &player, facts, false);
        assert_eq!(table.guard_restored(), [(Command::NavMenu, PAD)]);
        assert_eq!(
            propose(
                &author,
                &player,
                facts,
                Command::Shoot,
                PAD,
                0,
                pad(Button::Start)
            ),
            RebindProposal::Refused {
                unbound: Command::NavMenu,
                class: PAD
            }
        );
        // Capturing Start back onto jump, the row it is saved on, is refused
        // the same way rather than asking to empty menu's row.
        assert_eq!(
            propose(
                &author,
                &player,
                facts,
                Command::Jump,
                PAD,
                0,
                pad(Button::Start)
            ),
            RebindProposal::Refused {
                unbound: Command::NavMenu,
                class: PAD
            }
        );
    }

    // Regression: pressing the given-back default again was a no-op, so the
    // hand-edited empty cancel row stayed saved and warned on every load.
    #[test]
    fn keeping_a_given_back_default_writes_it_over_the_broken_row() {
        let author = AuthorLayer::default();
        let mut player = PlayerLayer::default();
        player.rows.insert((Command::NavCancel, PAD), Vec::new());
        let facts = RelevanceFacts::EVERY;
        let kept = applied(propose(
            &author,
            &player,
            facts,
            Command::NavCancel,
            PAD,
            0,
            pad(Button::East),
        ));
        assert_eq!(
            kept.rows[&(Command::NavCancel, PAD)],
            [Some(pad(Button::East))]
        );
        let table = EffectiveTable::build(&author, &kept, facts, false);
        assert!(table.guard_restored().is_empty());
        assert_eq!(table.inputs(Command::NavCancel, PAD), [pad(Button::East)]);

        // With no broken row of its own, keeping changes nothing.
        assert_eq!(
            propose(
                &author,
                &PlayerLayer::default(),
                facts,
                Command::NavCancel,
                PAD,
                0,
                pad(Button::East)
            ),
            RebindProposal::Unchanged
        );
    }

    // Regression: swapping Shift into sprint's second slot put the default's
    // hold on the other key.
    #[test]
    fn a_same_row_swap_keeps_the_hold_on_the_default_and_press_on_the_other_key() {
        let mut author = AuthorLayer::default();
        author.defaults.insert(
            (Command::Sprint, KBM),
            vec![AuthorBinding {
                input: key(KeyCode::ShiftLeft),
                activator: Activator::new(ActivatorKind::Hold),
            }],
        );
        let mut player = PlayerLayer::default();
        player.rows.insert(
            (Command::Sprint, KBM),
            vec![Some(key(KeyCode::ShiftLeft)), Some(key(KeyCode::KeyN))],
        );
        let swapped = applied(propose(
            &author,
            &player,
            RelevanceFacts::EVERY,
            Command::Sprint,
            KBM,
            1,
            key(KeyCode::ShiftLeft),
        ));
        let table = EffectiveTable::build(&author, &swapped, RelevanceFacts::EVERY, false);
        let sprint: Vec<_> = table
            .entries()
            .iter()
            .filter(|e| e.command == Command::Sprint && e.class == KBM)
            .map(|e| (e.input, e.activator.kind))
            .collect();
        assert_eq!(
            sprint,
            [
                (key(KeyCode::KeyN), ActivatorKind::Press),
                (key(KeyCode::ShiftLeft), ActivatorKind::Hold),
            ]
        );
    }

    // Regression: the edited row was built from what the panel shows, which
    // reads an unreadable slot as its fallback default, so capturing into
    // another slot saved the default's name over the player's string.
    #[test]
    fn capturing_into_another_slot_keeps_an_unreadable_slot() {
        let author = AuthorLayer::default();
        let mut player = PlayerLayer::default();
        player
            .rows
            .insert((Command::Jump, KBM), vec![None, Some(key(KeyCode::KeyJ))]);
        let facts = RelevanceFacts::EVERY;
        let table = EffectiveTable::build(&author, &player, facts, false);
        assert_eq!(
            table.inputs(Command::Jump, KBM),
            [key(KeyCode::Space), key(KeyCode::KeyJ)],
            "the unreadable slot shows its default"
        );
        let edited = applied(propose(
            &author,
            &player,
            facts,
            Command::Jump,
            KBM,
            1,
            key(KeyCode::KeyK),
        ));
        assert_eq!(
            edited.rows[&(Command::Jump, KBM)],
            [None, Some(key(KeyCode::KeyK))]
        );

        // Capturing into the unreadable slot itself replaces it in place.
        let overwritten = applied(propose(
            &author,
            &player,
            facts,
            Command::Jump,
            KBM,
            0,
            key(KeyCode::KeyK),
        ));
        assert_eq!(
            overwritten.rows[&(Command::Jump, KBM)],
            [Some(key(KeyCode::KeyK)), Some(key(KeyCode::KeyJ))]
        );
    }

    #[test]
    fn a_replace_on_a_live_holder_keeps_its_unreadable_slot() {
        let author = AuthorLayer::default();
        let mut player = PlayerLayer::default();
        player
            .rows
            .insert((Command::Jump, KBM), vec![None, Some(key(KeyCode::KeyJ))]);
        let facts = RelevanceFacts::EVERY;
        let proposal = propose(
            &author,
            &player,
            facts,
            Command::Use,
            KBM,
            0,
            key(KeyCode::KeyJ),
        );
        let RebindProposal::Conflict { with, replace } = proposal.clone() else {
            panic!("J drives jump, so the player is asked: {proposal:?}");
        };
        assert_eq!(with, [Command::Jump]);
        assert_eq!(replace.rows[&(Command::Jump, KBM)], [None]);
        let table = EffectiveTable::build(&author, &replace, facts, false);
        assert_eq!(table.inputs(Command::Jump, KBM), [key(KeyCode::Space)]);
        assert_eq!(table.inputs(Command::Use, KBM), [key(KeyCode::KeyJ)]);
    }

    #[test]
    fn a_replace_of_the_input_an_unreadable_slot_falls_back_to_leaves_it_saved_and_flagged() {
        // Jump's unreadable slot shows Space; use takes Space, and the slot
        // stays saved, its fallback lost to the player's binding.
        let author = AuthorLayer::default();
        let mut player = PlayerLayer::default();
        player.rows.insert((Command::Jump, KBM), vec![None]);
        let facts = RelevanceFacts::EVERY;
        let proposal = propose(
            &author,
            &player,
            facts,
            Command::Use,
            KBM,
            0,
            key(KeyCode::Space),
        );
        let RebindProposal::Conflict { with, replace } = proposal.clone() else {
            panic!("Space drives jump, so the player is asked: {proposal:?}");
        };
        assert_eq!(with, [Command::Jump]);
        assert_eq!(replace.rows[&(Command::Jump, KBM)], [None]);
        let table = EffectiveTable::build(&author, &replace, facts, false);
        assert!(table.inputs(Command::Jump, KBM).is_empty());
        assert!(table.displaced().contains(&(Command::Jump, KBM)));
    }

    // Regression: the replace check on the player's answer ran only the
    // guard-break check, which a default already given back passes, so a
    // binding landing behind menu's given-back Start applied.
    #[test]
    fn a_stale_replace_whose_binding_lands_behind_a_given_back_default_is_refused() {
        let author = AuthorLayer::default();
        let facts = RelevanceFacts::EVERY;
        let mut current = PlayerLayer::default();
        current
            .rows
            .insert((Command::Jump, PAD), vec![Some(pad(Button::Start))]);
        let table = EffectiveTable::build(&author, &current, facts, false);
        assert_eq!(table.guard_restored(), [(Command::NavMenu, PAD)]);
        let mut stale = PlayerLayer::default();
        stale
            .rows
            .insert((Command::Shoot, PAD), vec![Some(pad(Button::Start))]);
        assert!(
            matches!(
                check_player_layer(&table, &author, stale.clone(), facts, false, None),
                RebindProposal::Clean { .. }
            ),
            "menu is broken in both tables, so no new break shows"
        );
        assert_eq!(
            check_player_layer(
                &table,
                &author,
                stale,
                facts,
                false,
                Some((Command::Shoot, PAD, pad(Button::Start)))
            ),
            RebindProposal::Refused {
                unbound: Command::NavMenu,
                class: PAD
            }
        );
    }
}
