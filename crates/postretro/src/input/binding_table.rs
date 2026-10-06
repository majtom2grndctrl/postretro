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
    /// Position in the command's list on this class; a rebound key keeps its
    /// slot's activator.
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
            // Today's Start closes an open pause menu, so menu stays live under
            // a capturing tree; its overlap with cancel is resolved below.
            Command::NavMenu => &[LiveContext::UiOpen, LiveContext::UiCapture],
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

/// The resolved binding table for one mod, relevance included.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EffectiveTable {
    entries: Vec<EffectiveBinding>,
    relevance: HashMap<Command, Relevance>,
    /// Commands whose author or engine default the player displaced.
    displaced: Vec<(Command, DeviceClass)>,
    /// Every binding a collision dropped, with the binding that kept the
    /// input; the input-block validation reports these.
    suppressed: Vec<(EffectiveBinding, EffectiveBinding)>,
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
        table.entries = table.resolve_collisions(candidates, &author.manifest_order);
        if swap_confirm_cancel {
            for entry in &mut table.entries {
                if entry.class == DeviceClass::Gamepad {
                    entry.command = match entry.command {
                        Command::NavConfirm => Command::NavCancel,
                        Command::NavCancel => Command::NavConfirm,
                        other => other,
                    };
                }
            }
        }
        table
    }

    /// Drop the loser of each collision: a higher layer keeps the input, and
    /// within one layer the earlier entry does. A player binding that pushes an
    /// author or engine default off its input flags that command for the panel,
    /// so nothing the player chose changes silently.
    fn resolve_collisions(
        &mut self,
        candidates: Vec<EffectiveBinding>,
        manifest_order: &[Command],
    ) -> Vec<EffectiveBinding> {
        let mut ordered: Vec<(usize, EffectiveBinding)> =
            candidates.into_iter().enumerate().collect();
        // Higher origin first; author entries in manifest order; then table order.
        let manifest_rank = |binding: &EffectiveBinding| match binding.origin {
            BindingOrigin::Author => manifest_order
                .iter()
                .position(|command| *command == binding.command)
                .unwrap_or(usize::MAX),
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
                    if winner.origin == BindingOrigin::Player
                        && candidate.origin != BindingOrigin::Player
                        && !self
                            .displaced
                            .contains(&(candidate.command, candidate.class))
                    {
                        self.displaced.push((candidate.command, candidate.class));
                    }
                }
                None => kept.push((index, candidate)),
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

    /// The inputs bound to a command on one class, in slot order.
    pub fn inputs(&self, command: Command, class: DeviceClass) -> Vec<PhysicalInput> {
        self.entries
            .iter()
            .filter(|e| e.command == command && e.class == class)
            .map(|e| e.input)
            .collect()
    }

    /// Guarded commands left unbound on a device class.
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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

/// Resolve one command on one class through the three layers.
fn layer_command(
    command: Command,
    class: DeviceClass,
    command_relevance: Relevance,
    author: &AuthorLayer,
    player: &PlayerLayer,
    out: &mut Vec<EffectiveBinding>,
) {
    let engine = || -> Vec<(AuthorBinding, BindingOrigin)> {
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
    };
    // A dev-only command is bound to its engine defaults alone.
    if command_relevance == Relevance::DevOnly {
        push_slots(command, class, engine(), out);
        return;
    }
    let base: Vec<(AuthorBinding, BindingOrigin)> = match author.defaults.get(&(command, class)) {
        Some(list) => list.iter().map(|b| (*b, BindingOrigin::Author)).collect(),
        None => engine(),
    };
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
                    // A slot past the author's list takes `press`.
                    activator: base
                        .get(slot)
                        .map_or(Activator::PRESS, |(b, _)| b.activator),
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
