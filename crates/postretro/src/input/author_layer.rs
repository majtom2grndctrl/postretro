// The mod's `input` block → the validated author layer. Validation degrades
// per command and device class: a diagnosed entry falls back to the engine
// default there, and nothing else changes.
// See: context/lib/input.md §2 · context/lib/player_options.md §6

use std::collections::HashMap;

use postretro_scripting_core::runtime::{ModInputBinding, ModInputBlock};

use super::binding_table::{
    AuthorBinding, AuthorLayer, BindingOrigin, CommandPresentation, EffectiveTable, GUARDED,
    GlyphDirs, PlayerLayer,
};
use super::commands::Command;
use super::input_names::{DeviceClass, input_name, parse_input};
use super::player_rows::input_fits;
use super::relevance::{RelevanceFacts, show_override_allowed};
use super::types::{Activator, ActivatorKind, DEFAULT_ACTIVATOR_THRESHOLD, PhysicalInput};

fn class_label(class: DeviceClass) -> &'static str {
    class.manifest_key()
}

/// The shortest activator threshold an author may set, in seconds: below it a
/// tap or hold resolves within a frame or two.
const MIN_ACTIVATOR_THRESHOLD: f32 = 0.01;
/// The longest activator threshold an author may set, in seconds, before
/// `hold_timing_scale` lengthens it further.
const MAX_ACTIVATOR_THRESHOLD: f32 = 5.0;

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
    // Checked after the cast, so a value f32 cannot hold (1e-50 rounds to 0,
    // 1e300 to infinity) is refused rather than slipping through.
    let threshold = match binding.threshold.map(|seconds| seconds as f32) {
        None => DEFAULT_ACTIVATOR_THRESHOLD,
        Some(seconds) if seconds.is_finite() && seconds > 0.0 => {
            let clamped = seconds.clamp(MIN_ACTIVATOR_THRESHOLD, MAX_ACTIVATOR_THRESHOLD);
            if clamped != seconds {
                log::warn!(
                    "[Input] input block: `{}` on {}: `threshold` {seconds}s is outside \
                     {MIN_ACTIVATOR_THRESHOLD}s..={MAX_ACTIVATOR_THRESHOLD}s; using {clamped}s",
                    command.id(),
                    class_label(class)
                );
            }
            clamped
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

/// Author defaults must leave confirm, cancel, and menu bound on each class;
/// a guarded command left unbound falls back to the engine default there.
fn diagnose_guard(layer: &mut AuthorLayer, fell_back: &mut Vec<(Command, DeviceClass)>) {
    let table = validation_table(layer);
    for (command, class) in table.guard_violations() {
        debug_assert!(GUARDED.contains(&command));
        log::warn!(
            "[Input] input block leaves `{}` unbound on {}; using the engine default there",
            command.id(),
            class_label(class)
        );
        layer.defaults.remove(&(command, class));
        fell_back.push((command, class));
    }
}

/// Report author entries a conflict unbound (the later one in manifest order
/// loses) and diagnosed commands whose engine fallback collides.
fn report_collisions(layer: &AuthorLayer, fell_back: &[(Command, DeviceClass)]) {
    let table = validation_table(layer);
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
/// so a conflict is reported whatever the registry holds.
fn validation_table(layer: &AuthorLayer) -> EffectiveTable {
    EffectiveTable::build(layer, &PlayerLayer::default(), RelevanceFacts::EVERY, false)
}
