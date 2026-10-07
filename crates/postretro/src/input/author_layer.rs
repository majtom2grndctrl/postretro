// The mod's `input` block → the validated author layer. Validation degrades
// per command and device class: a diagnosed entry falls back to the engine
// default there, and nothing else changes.
// See: context/lib/input.md §2 · context/lib/player_options.md §6

use std::collections::HashMap;

use postretro_scripting_core::runtime::{ModInputBinding, ModInputBlock};

use super::binding_table::{
    AuthorBinding, AuthorLayer, BindingOrigin, CommandPresentation, EffectiveTable, GUARDED,
    GlyphDirs, PlayerLayer, swapped_command,
};
use super::commands::Command;
use super::input_names::{DeviceClass, input_name, parse_input};
use super::player_rows::input_fits;
use super::relevance::{RelevanceFacts, show_override_allowed};
use super::types::{Activator, ActivatorKind, DEFAULT_ACTIVATOR_THRESHOLD, PhysicalInput};

fn class_label(class: DeviceClass) -> &'static str {
    class.manifest_key()
}

/// The shortest activator threshold an author may set, in seconds: a few
/// frames at 60 Hz, so a tap stays possible to release in time.
const MIN_ACTIVATOR_THRESHOLD: f64 = 0.05;
/// The longest activator threshold an author may set, in seconds, before
/// `hold_timing_scale` lengthens it further.
const MAX_ACTIVATOR_THRESHOLD: f64 = 5.0;

fn parse_activator(raw: Option<&str>) -> Result<ActivatorKind, String> {
    match raw {
        None => Ok(ActivatorKind::Press),
        Some("press") => Ok(ActivatorKind::Press),
        Some("release") => Ok(ActivatorKind::Release),
        Some("tap") => Ok(ActivatorKind::Tap),
        Some("hold") => Ok(ActivatorKind::Hold),
        Some(other) => Err(format!("activator `{other}` is unknown")),
    }
}

fn activator_name(kind: ActivatorKind) -> &'static str {
    match kind {
        ActivatorKind::Press => "press",
        ActivatorKind::Release => "release",
        ActivatorKind::Tap => "tap",
        ActivatorKind::Hold => "hold",
    }
}

/// Validate one authored binding for `command` on `class`.
fn validate_binding(
    command: Command,
    class: DeviceClass,
    binding: &ModInputBinding,
) -> Result<AuthorBinding, String> {
    // The drain leaves `input` empty for a binding that is not an object or
    // has no `input` string.
    if binding.input.is_empty() {
        return Err("a binding must be an object with an `input` string".to_string());
    }
    let input = parse_input(&binding.input)
        .ok_or_else(|| format!("`{}` is not a known input", binding.input))?;
    if !input_fits(command, class, input) {
        return Err(format!(
            "`{}` cannot drive `{}` on {}",
            binding.input,
            command.id(),
            class_label(class)
        ));
    }
    let kind = parse_activator(binding.activator.as_deref())?;
    if !command.accepts(kind) {
        return Err(format!(
            "`{}` does not accept the `{}` activator",
            command.id(),
            activator_name(kind)
        ));
    }
    if matches!(
        input,
        PhysicalInput::MouseWheelUp | PhysicalInput::MouseWheelDown
    ) && kind != ActivatorKind::Press
    {
        return Err(format!(
            "`{}` is a wheel notch and takes `press` only",
            binding.input
        ));
    }
    // Clamped while still f64, so a value f32 cannot hold (1e-50 would round
    // to 0, 1e300 to infinity) clamps like any other out-of-range value.
    let threshold = match binding.threshold {
        None => DEFAULT_ACTIVATOR_THRESHOLD,
        Some(seconds) if seconds.is_finite() && seconds > 0.0 => {
            let clamped = seconds.clamp(MIN_ACTIVATOR_THRESHOLD, MAX_ACTIVATOR_THRESHOLD);
            if clamped != seconds {
                log::warn!(
                    "[Input] input block: `{}` on {}: `threshold` {seconds:?}s is outside \
                     {MIN_ACTIVATOR_THRESHOLD}s..={MAX_ACTIVATOR_THRESHOLD}s; using {clamped}s",
                    command.id(),
                    class_label(class)
                );
            }
            clamped as f32
        }
        Some(_) => return Err("`threshold` must be a positive number of seconds".to_string()),
    };
    Ok(AuthorBinding {
        input,
        activator: Activator::with_threshold(kind, threshold),
    })
}

/// Build the author layer from the mod's `input` block. With no block the
/// layer is empty and the engine table applies.
pub fn author_layer_from_block(block: Option<&ModInputBlock>) -> AuthorLayer {
    let mut layer = AuthorLayer::default();
    let Some(block) = block else {
        return layer;
    };
    // Commands whose entries were diagnosed on a class: their engine fallback
    // colliding with another binding is reported too.
    let mut fell_back: Vec<(Command, DeviceClass)> = Vec::new();

    for authored in &block.commands {
        let Some(command) = Command::from_id(&authored.id) else {
            log::warn!(
                "[Input] input block: unknown command `{}`; ignoring it",
                authored.id
            );
            continue;
        };
        // A JS object or Luau table keeps one value per key, so an authored
        // manifest never repeats a command; this guards a block built some
        // other way.
        if layer.manifest_order.contains(&command) {
            log::warn!(
                "[Input] input block: `{}` appears more than once; keeping the first",
                command.id()
            );
            continue;
        }
        layer.manifest_order.push(command);
        if let Some(show) = authored.show {
            if show_override_allowed(command) {
                layer.show.insert(command, show);
            } else {
                log::warn!(
                    "[Input] input block: `{}` cannot be shown or hidden; ignoring `show`",
                    command.id()
                );
            }
        }
        layer.presentation.insert(
            command,
            CommandPresentation {
                label: authored.label.clone(),
                category: authored.category.clone(),
                order: authored.order,
            },
        );
        for (class, list) in [
            (DeviceClass::KeyboardMouse, &authored.keyboard_mouse),
            (DeviceClass::Gamepad, &authored.gamepad),
        ] {
            let Some(list) = list else {
                continue;
            };
            let validated: Result<Vec<_>, _> = list
                .iter()
                .map(|binding| validate_binding(command, class, binding))
                .collect();
            match validated {
                Ok(bindings) => {
                    layer.defaults.insert((command, class), bindings);
                }
                Err(reason) => {
                    log::warn!(
                        "[Input] input block: `{}` on {}: {reason}; using the engine default there",
                        command.id(),
                        class_label(class)
                    );
                    fell_back.push((command, class));
                }
            }
        }
    }

    diagnose_tap_past_hold(&mut layer, &mut fell_back);
    diagnose_guard(&mut layer, &mut fell_back);
    report_collisions(&layer, &fell_back);

    layer.glyphs = GlyphDirs {
        keyboard_mouse: block.glyphs.keyboard_mouse.clone(),
        xbox: block.glyphs.xbox.clone(),
        playstation: block.glyphs.playstation.clone(),
        nintendo: block.glyphs.nintendo.clone(),
    };
    layer
}

/// A tap's max above a hold's min on the same input would let a release
/// between them fire the tap after the hold was due. The tap's command falls
/// back on that class.
fn diagnose_tap_past_hold(layer: &mut AuthorLayer, fell_back: &mut Vec<(Command, DeviceClass)>) {
    let mut holds: HashMap<(PhysicalInput, DeviceClass), f32> = HashMap::new();
    for ((_, class), bindings) in &layer.defaults {
        for binding in bindings {
            if binding.activator.kind == ActivatorKind::Hold {
                let min = holds
                    .entry((binding.input, *class))
                    .or_insert(binding.activator.threshold);
                *min = min.min(binding.activator.threshold);
            }
        }
    }
    let offending: Vec<(Command, DeviceClass, PhysicalInput, f32, f32)> = layer
        .defaults
        .iter()
        .flat_map(|((command, class), bindings)| {
            bindings.iter().filter_map(|binding| {
                let hold_min = holds.get(&(binding.input, *class))?;
                (binding.activator.kind == ActivatorKind::Tap
                    && binding.activator.threshold > *hold_min)
                    .then_some((
                        *command,
                        *class,
                        binding.input,
                        binding.activator.threshold,
                        *hold_min,
                    ))
            })
        })
        .collect();
    for (command, class, input, tap_max, hold_min) in offending {
        log::warn!(
            "[Input] input block: `{}` on {} taps `{}` for up to {tap_max}s, past the \
             {hold_min}s hold on that input; using the engine default there",
            command.id(),
            class_label(class),
            input_name(input).unwrap_or("?")
        );
        layer.defaults.remove(&(command, class));
        fell_back.push((command, class));
    }
}

/// Author defaults must leave confirm, cancel, and menu bound on each class,
/// with the player's confirm/cancel swap off and on. A guarded command its own
/// entry leaves unbound falls back to the engine default there. An engine
/// default an author binding takes — `use` on `start`, which menu needs in
/// gameplay — stays with the guarded command, and that author binding is
/// unbound on the input, as the later of two conflicting entries is. Author
/// outranks engine in a collision, so without this a mod could leave a pad
/// with no way to pause. The swap is a player option, so both states are
/// checked: `nav_cancel` on `start` shares menu's button with the swap off but
/// drives confirm with it on, and confirm and menu conflict.
fn diagnose_guard(layer: &mut AuthorLayer, fell_back: &mut Vec<(Command, DeviceClass)>) {
    // Every pass that runs again removed an author entry or binding, so the
    // loop ends. A fallback or a drop can break another guarded command, which
    // the next pass sees.
    loop {
        let mut changed = false;
        for swap in [false, true] {
            changed |= diagnose_guard_once(layer, fell_back, swap);
        }
        if !changed {
            break;
        }
    }
}

/// One guard pass over the validation table for one swap state. Returns
/// whether it changed the layer.
fn diagnose_guard_once(
    layer: &mut AuthorLayer,
    fell_back: &mut Vec<(Command, DeviceClass)>,
    swap: bool,
) -> bool {
    // The table names commands after the swap; the layer stores them before.
    let stored = |command: Command, class: DeviceClass| swapped_command(command, class, swap);
    let swap_note = if swap {
        " with the confirm/cancel swap on"
    } else {
        ""
    };
    let table = validation_table(layer, swap);
    let mut changed = false;
    for (command, class) in table.guard_violations() {
        debug_assert!(GUARDED.contains(&command));
        // Named after the entry the author wrote: with the swap on, cancel's
        // entry is what leaves confirm unbound, and the reverse.
        let entry = stored(command, class);
        if layer.defaults.remove(&(entry, class)).is_some() {
            log::warn!(
                "[Input] input block leaves `{}` unbound on {}{swap_note}; using the engine \
                 default there",
                entry.id(),
                class_label(class)
            );
            fell_back.push((entry, class));
            changed = true;
            continue;
        }
        for (lost, holder) in table.suppressed() {
            let takes_guarded_default = lost.command == command
                && lost.class == class
                && lost.origin == BindingOrigin::Engine
                && holder.origin == BindingOrigin::Author;
            if !takes_guarded_default {
                continue;
            }
            let authored = stored(holder.command, holder.class);
            let Some(list) = layer.defaults.get_mut(&(authored, holder.class)) else {
                continue;
            };
            let before = list.len();
            list.retain(|binding| binding.input != holder.input);
            if list.len() != before {
                log::warn!(
                    "[Input] input block: `{}` on `{}` would leave `{}` unbound on {}{swap_note}; \
                     `{}` is unbound there",
                    authored.id(),
                    input_name(holder.input).unwrap_or("?"),
                    command.id(),
                    class_label(class),
                    authored.id()
                );
                changed = true;
            }
        }
    }
    changed
}

/// Report author entries a conflict unbound (the later one in manifest order
/// loses) and diagnosed commands whose engine fallback collides.
fn report_collisions(layer: &AuthorLayer, fell_back: &[(Command, DeviceClass)]) {
    let table = validation_table(layer, false);
    for (loser, winner) in table.suppressed() {
        let reported = loser.origin == BindingOrigin::Author
            || fell_back.contains(&(loser.command, loser.class));
        if !reported {
            continue;
        }
        log::warn!(
            "[Input] input block: `{}` on `{}` conflicts with `{}`; `{}` is unbound there",
            loser.command.id(),
            input_name(loser.input).unwrap_or("?"),
            winner.command.id(),
            loser.command.id()
        );
    }
}

/// The table validation checks against: every data-driven command relevant,
/// so a conflict is reported whatever the registry holds. Conflicts are
/// reported with the swap off; the guard checks both states.
fn validation_table(layer: &AuthorLayer, swap: bool) -> EffectiveTable {
    EffectiveTable::build(layer, &PlayerLayer::default(), RelevanceFacts::EVERY, swap)
}
