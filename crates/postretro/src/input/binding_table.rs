// Effective bindings: player override, else author default, else engine
// default, per command and device class; conflicts and the nav guard.
// See: context/lib/input.md §2 · context/lib/player_options.md §6

use std::collections::HashMap;

use super::commands::{Command, CommandContext};
use super::defaults::{engine_default_inputs, gameplay_binding};
use super::input_names::DeviceClass;
use super::relevance::{Relevance, RelevanceFacts, relevance};
use super::types::{Activator, ActivatorKind, Binding, PhysicalInput};

/// One author default: an input and the activator the author fixed for it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AuthorBinding {
    pub input: PhysicalInput,
    pub activator: Activator,
}

/// The mod author's layer, already validated: every entry here is accepted.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AuthorLayer {
    /// Defaults per command and device class. An absent key keeps the engine
    /// default; an empty list leaves the command unbound there.
    pub defaults: HashMap<(Command, DeviceClass), Vec<AuthorBinding>>,
    /// The author's `show` override per command.
    pub show: HashMap<Command, bool>,
    /// Commands in manifest order. Of two author entries that conflict, the
    /// later one in this order is unbound.
    pub manifest_order: Vec<Command>,
    /// Panel presentation per command: label, category, order.
    pub presentation: HashMap<Command, CommandPresentation>,
    /// Glyph art directories per device family.
    pub glyphs: GlyphDirs,
}

/// How the controls panel presents a command.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommandPresentation {
    pub label: Option<String>,
    pub category: Option<String>,
    pub order: Option<f64>,
}

/// Mod glyph art directories per device family; an asset is `<dir>/<input>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlyphDirs {
    pub keyboard_mouse: Option<String>,
    pub xbox: Option<String>,
    pub playstation: Option<String>,
    pub nintendo: Option<String>,
}

/// The player's saved diff over the author's defaults.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayerLayer {
    /// One entry per slot. `None` marks a stored input string this build
    /// cannot read: that slot falls back to its author default alone.
    pub rows: HashMap<(Command, DeviceClass), Vec<Option<PhysicalInput>>>,
}

/// Which layer an effective binding came from. Higher layers win collisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BindingOrigin {
    Engine,
    Author,
    Player,
}

/// One resolved binding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectiveBinding {
    pub command: Command,
    pub class: DeviceClass,
    /// Position in the command's list on this class. A player input's
    /// activator follows the input, not the slot (`player_activator`).
    pub slot: usize,
    pub input: PhysicalInput,
    pub activator: Activator,
    pub origin: BindingOrigin,
}

/// Where a command is live. Conflicts are checked among commands that share
/// one of these, so South may be both `jump` and `nav_confirm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveContext {
    Gameplay,
    /// No capturing tree on top.
    UiOpen,
    /// A capturing tree on top.
    UiCapture,
    /// A text-entry tree on top.
    UiTextEntry,
}

fn live_contexts(command: Command) -> &'static [LiveContext] {
    match command.context() {
        CommandContext::Gameplay => &[LiveContext::Gameplay],
        CommandContext::Ui => match command {
            // Menu opens the pause menu from gameplay, so it shares no input
            // with a gameplay command. Today's Start closes an open pause menu,
            // so menu stays live under a capturing tree too; its overlap with
            // cancel is resolved below.
            Command::NavMenu => &[
                LiveContext::Gameplay,
                LiveContext::UiOpen,
                LiveContext::UiCapture,
            ],
            Command::TextBackspace | Command::TextSpace | Command::TextCommit => {
                &[LiveContext::UiTextEntry]
            }
            _ => &[LiveContext::UiCapture, LiveContext::UiTextEntry],
        },
    }
}

/// Commands whose activator set is `press` alone never share a key.
fn press_only(command: Command) -> bool {
    !command.accepts(ActivatorKind::Hold)
}

fn is_hold(binding: &EffectiveBinding) -> bool {
    binding.activator.kind == ActivatorKind::Hold
}

/// Whether two bindings conflict: one input, two commands live in a shared
/// context, and not the one legal pairing (a short binding plus a `hold`,
/// neither press-only). Menu and cancel on one input are not a conflict: under
/// a capturing tree the input acts as cancel, which closes an open menu too.
/// Menu and a gameplay command always conflict: UI nav reads presses, never
/// activators, so no hold pairing keeps them apart.
pub fn conflicts(a: &EffectiveBinding, b: &EffectiveBinding) -> bool {
    if a.input != b.input || a.command == b.command {
        return false;
    }
    let shared = live_contexts(a.command)
        .iter()
        .any(|context| live_contexts(b.command).contains(context));
    if !shared {
        return false;
    }
    if matches!(
        (a.command, b.command),
        (Command::NavMenu, Command::NavCancel) | (Command::NavCancel, Command::NavMenu)
    ) {
        return false;
    }
    if a.command.context() != b.command.context() {
        return true;
    }
    let one_hold = is_hold(a) != is_hold(b);
    !(one_hold && !press_only(a.command) && !press_only(b.command))
}

/// Whether `candidate` must give way to an already-kept binding. Beyond a
/// conflict, a lower layer's `hold` landing on a player's short binding would
/// newly delay the player's press to its release, so it collides too.
fn collides(kept: &EffectiveBinding, candidate: &EffectiveBinding) -> bool {
    if conflicts(kept, candidate) {
        return true;
    }
    kept.origin == BindingOrigin::Player
        && candidate.origin != BindingOrigin::Player
        && kept.input == candidate.input
        && kept.command != candidate.command
        && !is_hold(kept)
        && is_hold(candidate)
        && live_contexts(kept.command)
            .iter()
            .any(|context| live_contexts(candidate.command).contains(context))
}

/// Commands that must stay bound on every device class.
pub const GUARDED: [Command; 3] = [Command::NavConfirm, Command::NavCancel, Command::NavMenu];

/// The command a stored binding drives. With the swap on, gamepad confirm and
/// cancel exchange; the mapping is its own inverse, so it also gives the
/// command a shown binding is stored on.
pub fn swapped_command(command: Command, class: DeviceClass, swap: bool) -> Command {
    match (swap, class, command) {
        (true, DeviceClass::Gamepad, Command::NavConfirm) => Command::NavCancel,
        (true, DeviceClass::Gamepad, Command::NavCancel) => Command::NavConfirm,
        _ => command,
    }
}

/// The resolved binding table for one mod, relevance included.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EffectiveTable {
    entries: Vec<EffectiveBinding>,
    relevance: HashMap<Command, Relevance>,
    /// Commands that lost an input to a player binding (an author or engine
    /// default, or another player row) or to a guarded default given back.
    displaced: Vec<(Command, DeviceClass)>,
    /// Every binding a collision dropped, with the binding that kept the
    /// input; the input-block validation reports these.
    suppressed: Vec<(EffectiveBinding, EffectiveBinding)>,
    /// Guarded commands the player layer left unbound on a class, whose
    /// defaults were given back.
    guard_restored: Vec<(Command, DeviceClass)>,
}

impl EffectiveTable {
    pub fn build(
        author: &AuthorLayer,
        player: &PlayerLayer,
        facts: RelevanceFacts,
        swap_confirm_cancel: bool,
    ) -> Self {
        let mut table = Self::default();
        let mut candidates = Vec::new();
        for command in Command::ALL {
            let command_relevance = relevance(command, facts, author.show.get(&command).copied());
            table.relevance.insert(command, command_relevance);
            if command_relevance == Relevance::Irrelevant {
                continue;
            }
            for class in DeviceClass::ALL {
                layer_command(
                    command,
                    class,
                    command_relevance,
                    author,
                    player,
                    &mut candidates,
                );
            }
        }
        // The swap applies before collisions resolve, so conflicts, the
        // menu-and-cancel pairing, and the displaced flags all name the
        // command each input drives.
        for candidate in &mut candidates {
            candidate.command =
                swapped_command(candidate.command, candidate.class, swap_confirm_cancel);
        }
        let kept =
            table.resolve_collisions(candidates, &author.manifest_order, swap_confirm_cancel);
        table.entries = table.restore_guard(kept, author, swap_confirm_cancel);
        table
    }

    /// Drop the loser of each collision: a higher layer keeps the input, and
    /// within one layer the earlier entry does. A player binding that pushes
    /// any other binding off its input flags that command for the panel, so
    /// nothing the player chose changes silently. Returns the kept bindings
    /// with their candidate positions, in candidate order.
    fn resolve_collisions(
        &mut self,
        candidates: Vec<EffectiveBinding>,
        manifest_order: &[Command],
        swap: bool,
    ) -> Vec<(usize, EffectiveBinding)> {
        let mut ordered: Vec<(usize, EffectiveBinding)> =
            candidates.into_iter().enumerate().collect();
        // Higher origin first; author entries in manifest order (of the command
        // the author wrote, before the swap); then table order.
        let manifest_rank = |binding: &EffectiveBinding| match binding.origin {
            BindingOrigin::Author => {
                let authored = swapped_command(binding.command, binding.class, swap);
                manifest_order
                    .iter()
                    .position(|command| *command == authored)
                    .unwrap_or(usize::MAX)
            }
            BindingOrigin::Engine | BindingOrigin::Player => 0,
        };
        ordered.sort_by(|(ia, a), (ib, b)| {
            b.origin
                .cmp(&a.origin)
                .then(manifest_rank(a).cmp(&manifest_rank(b)))
                .then(ia.cmp(ib))
        });
        let mut kept: Vec<(usize, EffectiveBinding)> = Vec::with_capacity(ordered.len());
        for (index, candidate) in ordered {
            let duplicate = kept
                .iter()
                .any(|(_, k)| k.command == candidate.command && k.input == candidate.input);
            if duplicate {
                continue;
            }
            let dev_only = self.relevance_of(candidate.command) == Relevance::DevOnly;
            let winner = if dev_only {
                None
            } else {
                kept.iter()
                    .filter(|(_, k)| self.relevance_of(k.command) != Relevance::DevOnly)
                    .find(|(_, k)| collides(k, &candidate))
                    .map(|(_, k)| *k)
            };
            match winner {
                Some(winner) => {
                    self.suppressed.push((candidate, winner));
                    // A player row losing to another player row (a saved row
                    // waking as its command becomes relevant) is flagged, as
                    // is a default losing to one.
                    if winner.origin == BindingOrigin::Player {
                        self.flag_displaced(candidate.command, candidate.class);
                    }
                }
                None => kept.push((index, candidate)),
            }
        }
        kept.sort_by_key(|(index, _)| *index);
        kept
    }

    fn flag_displaced(&mut self, command: Command, class: DeviceClass) {
        if !self.displaced.contains(&(command, class)) {
            self.displaced.push((command, class));
        }
    }

    /// Give a guarded command the player layer left unbound on a class (a
    /// hand-edited empty row, or a newer author default landing on an input a
    /// player row holds) its author or engine default back. The player
    /// bindings on those inputs yield and their commands are flagged. A
    /// default an author or engine binding holds is not given back, so an
    /// author layer that unbinds a guarded command still reads as a guard
    /// violation for validation to diagnose.
    fn restore_guard(
        &mut self,
        mut kept: Vec<(usize, EffectiveBinding)>,
        author: &AuthorLayer,
        swap: bool,
    ) -> Vec<EffectiveBinding> {
        let mut next_index = kept.iter().map(|(index, _)| index + 1).max().unwrap_or(0);
        let mut attempted: Vec<(Command, DeviceClass)> = Vec::new();
        // Commands whose player bindings yielded here: their flag stands even
        // if they get their own defaults back later.
        let mut evicted: Vec<(Command, DeviceClass)> = Vec::new();
        // Each (command, class) is tried at most once, so the loop ends.
        // Evicting a player binding of another guarded command can leave that
        // one unbound for a later pass.
        loop {
            let missing = GUARDED
                .into_iter()
                .flat_map(|command| DeviceClass::ALL.map(|class| (command, class)))
                .find(|(command, class)| {
                    !attempted.contains(&(*command, *class))
                        && self.relevance_of(*command) == Relevance::Relevant
                        && !kept
                            .iter()
                            .any(|(_, k)| k.command == *command && k.class == *class)
                });
            let Some((command, class)) = missing else {
                break;
            };
            attempted.push((command, class));
            let stored = swapped_command(command, class, swap);
            let mut slot = 0;
            for (binding, origin) in default_slots(stored, class, author) {
                let restored = EffectiveBinding {
                    command,
                    class,
                    slot,
                    input: binding.input,
                    activator: binding.activator,
                    origin,
                };
                let holders: Vec<usize> = kept
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, k))| {
                        self.relevance_of(k.command) != Relevance::DevOnly
                            && (collides(k, &restored) || collides(&restored, k))
                    })
                    .map(|(position, _)| position)
                    .collect();
                if holders
                    .iter()
                    .any(|position| kept[*position].1.origin != BindingOrigin::Player)
                {
                    continue;
                }
                for position in holders.into_iter().rev() {
                    let (_, holder) = kept.remove(position);
                    self.suppressed.push((holder, restored));
                    self.flag_displaced(holder.command, holder.class);
                    evicted.push((holder.command, holder.class));
                }
                kept.push((next_index, restored));
                next_index += 1;
                slot += 1;
            }
            if slot > 0 {
                self.guard_restored.push((command, class));
                // Its defaults are back, so a flag from losing them to a
                // player binding no longer applies.
                if !evicted.contains(&(command, class)) {
                    self.displaced
                        .retain(|flagged| *flagged != (command, class));
                }
            }
        }
        kept.sort_by_key(|(index, _)| *index);
        kept.into_iter().map(|(_, binding)| binding).collect()
    }

    fn relevance_of(&self, command: Command) -> Relevance {
        self.relevance
            .get(&command)
            .copied()
            .unwrap_or(Relevance::Irrelevant)
    }

    pub fn entries(&self) -> &[EffectiveBinding] {
        &self.entries
    }

    pub fn relevance(&self, command: Command) -> Relevance {
        self.relevance_of(command)
    }

    /// Bindings a collision dropped, each with the binding that kept the input.
    pub fn suppressed(&self) -> &[(EffectiveBinding, EffectiveBinding)] {
        &self.suppressed
    }

    pub fn displaced(&self) -> &[(Command, DeviceClass)] {
        &self.displaced
    }

    /// Guarded commands whose defaults were given back because the player
    /// layer left them unbound on a class.
    pub fn guard_restored(&self) -> &[(Command, DeviceClass)] {
        &self.guard_restored
    }

    /// The inputs bound to a command on one class, in slot order.
    pub fn inputs(&self, command: Command, class: DeviceClass) -> Vec<PhysicalInput> {
        self.entries
            .iter()
            .filter(|e| e.command == command && e.class == class)
            .map(|e| e.input)
            .collect()
    }

    /// Guarded commands left unbound on a device class.
    pub fn guard_violations(&self) -> Vec<(Command, DeviceClass)> {
        GUARDED
            .into_iter()
            .flat_map(|command| DeviceClass::ALL.map(|class| (command, class)))
            .filter(|(command, class)| {
                !self
                    .entries
                    .iter()
                    .any(|e| e.command == *command && e.class == *class)
            })
            .collect()
    }

    /// Every conflicting pair among relevant, non-dev bindings.
    #[cfg(test)]
    pub fn conflicting_pairs(&self) -> Vec<(EffectiveBinding, EffectiveBinding)> {
        let checked: Vec<_> = self
            .entries
            .iter()
            .filter(|e| self.relevance_of(e.command) == Relevance::Relevant)
            .collect();
        let mut pairs = Vec::new();
        for (i, a) in checked.iter().enumerate() {
            for b in &checked[i + 1..] {
                if conflicts(a, b) {
                    pairs.push((**a, **b));
                }
            }
        }
        pairs
    }

    /// The input-system bindings for gameplay commands.
    pub fn gameplay_bindings(&self) -> Vec<Binding> {
        self.entries
            .iter()
            .filter_map(|e| gameplay_binding(e.command, e.input, e.activator))
            .collect()
    }
}

/// A command's defaults on one class: the author's list, else the engine's.
fn default_slots(
    command: Command,
    class: DeviceClass,
    author: &AuthorLayer,
) -> Vec<(AuthorBinding, BindingOrigin)> {
    match author.defaults.get(&(command, class)) {
        Some(list) => list.iter().map(|b| (*b, BindingOrigin::Author)).collect(),
        None => engine_slots(command, class),
    }
}

fn engine_slots(command: Command, class: DeviceClass) -> Vec<(AuthorBinding, BindingOrigin)> {
    engine_default_inputs(command, class)
        .iter()
        .map(|input| {
            (
                AuthorBinding {
                    input: *input,
                    activator: Activator::PRESS,
                },
                BindingOrigin::Engine,
            )
        })
        .collect()
}

/// The activator a player-row input takes. Players rebind keys, never
/// activators, so each default's activator applies to at most one input:
///
/// - An input that is one of the command's defaults keeps that default's
///   activator wherever the row puts it.
/// - Any other input replaced its slot's default when that default's input is
///   gone from the row, and takes its activator. When the default's input is
///   still in the row (a swap within the row moved it), the other input takes
///   `press`, so a swap never copies a default's activator onto it.
/// - A slot past the defaults takes `press`, and a wheel notch always does: it
///   has no duration to time.
///
/// Rows store keys only, so a default another command took reads the same as
/// one the player replaced: the input left in that slot takes its activator.
fn player_activator(
    defaults: &[(AuthorBinding, BindingOrigin)],
    row: &[Option<PhysicalInput>],
    slot: usize,
    input: PhysicalInput,
) -> Activator {
    if matches!(
        input,
        PhysicalInput::MouseWheelUp | PhysicalInput::MouseWheelDown
    ) {
        return Activator::PRESS;
    }
    if let Some((own, _)) = defaults.iter().find(|(binding, _)| binding.input == input) {
        return own.activator;
    }
    match defaults.get(slot) {
        Some((replaced, _)) if !row.contains(&Some(replaced.input)) => replaced.activator,
        _ => Activator::PRESS,
    }
}

/// Resolve one command on one class through the three layers.
fn layer_command(
    command: Command,
    class: DeviceClass,
    command_relevance: Relevance,
    author: &AuthorLayer,
    player: &PlayerLayer,
    out: &mut Vec<EffectiveBinding>,
) {
    // A dev-only command is bound to its engine defaults alone.
    if command_relevance == Relevance::DevOnly {
        push_slots(command, class, engine_slots(command, class), out);
        return;
    }
    let base = default_slots(command, class, author);
    let Some(row) = player.rows.get(&(command, class)) else {
        push_slots(command, class, base, out);
        return;
    };
    let layered = row
        .iter()
        .enumerate()
        .filter_map(|(slot, stored)| match stored {
            Some(input) => Some((
                AuthorBinding {
                    input: *input,
                    activator: player_activator(&base, row, slot, *input),
                },
                BindingOrigin::Player,
            )),
            // An unreadable stored input falls back to that slot's default.
            None => base.get(slot).copied(),
        })
        .collect();
    push_slots(command, class, layered, out);
}

fn push_slots(
    command: Command,
    class: DeviceClass,
    slots: Vec<(AuthorBinding, BindingOrigin)>,
    out: &mut Vec<EffectiveBinding>,
) {
    out.extend(
        slots
            .into_iter()
            .enumerate()
            .map(|(slot, (binding, origin))| EffectiveBinding {
                command,
                class,
                slot,
                input: binding.input,
                activator: binding.activator,
                origin,
            }),
    );
}
