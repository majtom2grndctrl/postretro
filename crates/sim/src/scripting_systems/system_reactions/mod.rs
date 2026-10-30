// System-reaction command path: the second execution arm of the shared
// named-reaction vocabulary. Entity reactions mutate `EntityRegistry`; system
// reactions (no `tag`) push typed commands onto a per-frame queue the app
// drains after the post-tick event drains, so audio/input/UI subsystems
// consume their commands without threading engine services into scripting.
// See: context/lib/scripting.md §10.4

use postretro_entities::ScriptCtx;
use postretro_scripting_core::reaction_registry::ReactionError;
#[cfg(test)]
pub(crate) use postretro_scripting_core::reaction_registry::SystemCommandQueue;
pub use postretro_scripting_core::reaction_registry::{
    SystemReactionCommand, SystemReactionRegistry,
};

/// Maximum `screenShake` amplitude in logical-reference px. Matches
/// `SHAKE_REFERENCE_WIDTH` in `render/screen_effects.rs` (1280 px): at this
/// amplitude the peak UV offset is exactly 1.0 — one full reference frame-width
/// — which is the natural ceiling of the 1280×720 reference coordinate system.
/// Amplitudes beyond this produce UV offsets > 1.0 that cause whole-frame
/// ClampToEdge edge-smear with no meaningful additional shake effect.
const MAX_SHAKE_AMPLITUDE_PX: f32 = 1280.0;

/// Apply one name-addressed live-sentiment write at the app's frame-end system
/// reaction drain. The authored registry remains immutable: resolve names and
/// the baseline while holding its read borrow, then mutate only the sparse
/// session overlay after that borrow has dropped.
fn apply_sentiment_write(
    script_ctx: &ScriptCtx,
    from: &str,
    to: &str,
    value: f32,
    is_adjustment: bool,
) -> Result<(), String> {
    if !value.is_finite() {
        return Err(format!(
            "{} value must be finite, got {value}",
            if is_adjustment {
                "adjustSentiment delta"
            } else {
                "setSentiment"
            }
        ));
    }

    let (from_index, to_index, baseline) = {
        let data_registry = script_ctx.data_registry.borrow();
        let factions = &data_registry.factions;
        let from_index = factions
            .index_for_name(from)
            .ok_or_else(|| format!("unknown from faction `{from}`"))?;
        let to_index = factions
            .index_for_name(to)
            .ok_or_else(|| format!("unknown to faction `{to}`"))?;
        let baseline = factions.sentiment(from_index, to_index);
        (from_index, to_index, baseline)
    };

    // The App drain owns the primary client-authority gate. Keep the writer itself
    // closed as well so focused callers cannot accidentally bypass that boundary.
    // This comes after reaction input validation, so a client still observes the
    // same invalid-value/unknown-faction rejection as the host.
    if !script_ctx.owner_slot_writes_enabled.get() {
        return Ok(());
    }

    let mut overlay = script_ctx.faction_sentiment.borrow_mut();
    if is_adjustment {
        overlay
            .adjust(from_index, to_index, value, baseline)
            .map_err(|error| error.to_string())?;
    } else {
        overlay
            .set(from_index, to_index, value, baseline)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Apply an absolute live-sentiment write queued by `setSentiment`.
pub fn apply_set_sentiment(
    script_ctx: &ScriptCtx,
    from: &str,
    to: &str,
    value: f32,
) -> Result<(), String> {
    apply_sentiment_write(script_ctx, from, to, value, false)
}

/// Apply a relative live-sentiment write queued by `adjustSentiment`.
pub fn apply_adjust_sentiment(
    script_ctx: &ScriptCtx,
    from: &str,
    to: &str,
    delta: f32,
) -> Result<(), String> {
    apply_sentiment_write(script_ctx, from, to, delta, true)
}

/// Register the system-reaction primitives onto `registry`:
/// - Audio: `playSound`
/// - Input/rumble: `rumble`
/// - Display/flash: `flashScreen`
/// - Screen-space effects: `vignette`, `screenShake`
/// - UI stack: `showDialog`, `openMenu`, `closeDialog` (push/pop `PushTree`/`PopTree`)
/// - Game flow: `loadLevel`, `restartLevel`, `returnToFrontend`
/// - Slot write: `setState`
/// - Live faction sentiment writes: `setSentiment`, `adjustSentiment`
/// - Ghost bus override: `setGhostAudio`
/// - Presentation-cell write: `cellWrite`
/// - Text-edit: `appendText`, `backspaceText`, `clearText`
pub fn register_system_reaction_primitives(registry: &mut SystemReactionRegistry) {
    // Sources warned about for a `playSound` that reads `on.emitter` but was
    // fired without one: one warning per (source, sound), never a dry play.
    let missing_emitter_warned: std::rc::Rc<
        std::cell::RefCell<std::collections::HashSet<(String, String)>>,
    > = Default::default();
    registry.register("playSound", move |args, queue| {
        let parsed: PlaySoundArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("playSound: failed to deserialize args: {e}"),
            })?;
        let at = match parsed.at.as_deref() {
            None => None,
            Some(postretro_entities::EMITTER_AT_TOKEN) => {
                let context = queue.fire_context();
                match context.emitter {
                    Some(emitter) => Some(emitter),
                    None => {
                        if missing_emitter_warned
                            .borrow_mut()
                            .insert((context.source.clone(), parsed.sound.clone()))
                        {
                            // Trigger residuals carry their trigger source; this
                            // fallback labels other contexts with no source.
                            let source = if context.source.is_empty() {
                                "an unnamed source".to_string()
                            } else {
                                format!("source `{}`", context.source)
                            };
                            log::warn!(
                                "[Scripting] playSound `{}` reads `on.emitter`, but {} publishes no emitter; skipping",
                                parsed.sound,
                                source,
                            );
                        }
                        return Ok(());
                    }
                }
            }
            Some(other) => {
                return Err(ReactionError::InvalidArgument {
                    reason: format!("playSound: `at` must be `on.emitter`, got `{other}`"),
                });
            }
        };
        queue.push(SystemReactionCommand::PlaySound {
            audience: parsed.audience,
            sound: parsed.sound,
            bus: parsed.bus,
            at,
        });
        Ok(())
    });
    registry.register("rumble", |args, queue| {
        let parsed: RumbleArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("rumble: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::Rumble {
            audience: parsed.audience,
            strong: parsed.strong,
            weak: parsed.weak,
            duration_ms: parsed.duration_ms,
        });
        Ok(())
    });
    registry.register("flashScreen", |args, queue| {
        let parsed: FlashScreenArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("flashScreen: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::FlashScreen {
            audience: parsed.audience,
            color: parsed.color,
            duration_ms: parsed.duration_ms,
        });
        Ok(())
    });
    // `vignette` darkens (or tints) the screen edges, rising then decaying. An
    // omitted `color` defaults to black (pure strength-only edge-darken); the
    // default is applied at the drain when the command maps onto
    // `VignetteDecay::start`, so the absent-color case is one behavior.
    registry.register("vignette", |args, queue| {
        let parsed: VignetteArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("vignette: failed to deserialize args: {e}"),
            })?;
        // The JSON bridge (serde_json::Number::from_f64) maps non-finite f32
        // values to JSON null, so NaN/Infinity arrive as null and are rejected
        // at deserialize before reaching this guard. The is_finite() arm is
        // defense-in-depth for any future non-JSON caller.
        if !parsed.duration_ms.is_finite() || parsed.duration_ms <= 0.0 {
            return Err(ReactionError::InvalidArgument {
                reason: format!(
                    "vignette: durationMs must be finite and > 0, got {}",
                    parsed.duration_ms
                ),
            });
        }
        queue.push(SystemReactionCommand::Vignette {
            audience: parsed.audience,
            color: parsed.color,
            strength: parsed.strength,
            duration_ms: parsed.duration_ms,
        });
        Ok(())
    });
    // `screenShake` starts a decaying oscillation. The omitted-frequency 18 Hz
    // default is applied by the DRIVER (`ShakeDecay::start`), not here — the
    // deserializer passes `None` through unchanged so the absent-frequency case
    // is one behavior regardless of how the reaction surface parses its args.
    registry.register("screenShake", |args, queue| {
        let parsed: ScreenShakeArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("screenShake: failed to deserialize args: {e}"),
            })?;
        // The JSON bridge (serde_json::Number::from_f64) maps non-finite f32
        // values to JSON null, so NaN/Infinity arrive as null and are rejected
        // at deserialize before reaching this guard. The is_finite() arm is
        // defense-in-depth for any future non-JSON caller.
        if !parsed.duration_ms.is_finite() || parsed.duration_ms <= 0.0 {
            return Err(ReactionError::InvalidArgument {
                reason: format!(
                    "screenShake: durationMs must be finite and > 0, got {}",
                    parsed.duration_ms
                ),
            });
        }
        // amplitude == 0.0 is a valid no-op shake (zero displacement); reject
        // only non-finite and negative. The upper bound caps the peak UV offset
        // at 1.0 (one full reference frame-width) to prevent whole-frame
        // ClampToEdge edge-smear. The JSON bridge maps non-finite to null
        // (rejected at deserialize); the is_finite() arm is defense-in-depth.
        if !parsed.amplitude.is_finite() || parsed.amplitude < 0.0 {
            return Err(ReactionError::InvalidArgument {
                reason: format!(
                    "screenShake: amplitude must be finite and >= 0, got {}",
                    parsed.amplitude
                ),
            });
        }
        if parsed.amplitude > MAX_SHAKE_AMPLITUDE_PX {
            return Err(ReactionError::InvalidArgument {
                reason: format!(
                    "screenShake: amplitude must be <= {MAX_SHAKE_AMPLITUDE_PX} px, got {}",
                    parsed.amplitude
                ),
            });
        }
        if let Some(freq) = parsed.frequency {
            // The JSON bridge maps non-finite to null (rejected at deserialize);
            // the is_finite() arm is defense-in-depth for non-JSON callers.
            if !freq.is_finite() || freq <= 0.0 {
                return Err(ReactionError::InvalidArgument {
                    reason: format!("screenShake: frequency must be finite and > 0, got {freq}"),
                });
            }
        }
        queue.push(SystemReactionCommand::ScreenShake {
            audience: parsed.audience,
            amplitude: parsed.amplitude,
            duration_ms: parsed.duration_ms,
            frequency: parsed.frequency,
        });
        Ok(())
    });
    // `showDialog` / `openMenu` are v1 aliases: both push a `PushTree` for a
    // named registered tree. `showDialog` carries the optional `onCommit`
    // reaction fired when the pushed tree commits; `openMenu` never does (a menu
    // has no commit payload), so its handler ignores any `onCommit` key. The
    // capture mode etc. travel on the tree's registered envelope (F's concern),
    // not the command. `closeDialog` pops the top tree.
    registry.register("showDialog", |args, queue| {
        let parsed: ShowDialogArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("showDialog: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::PushTree {
            tree: parsed.tree,
            on_commit: parsed.on_commit,
        });
        Ok(())
    });
    registry.register("openMenu", |args, queue| {
        let parsed: OpenMenuArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("openMenu: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::PushTree {
            tree: parsed.tree,
            // A menu carries no commit payload; the alias never sets `on_commit`.
            on_commit: None,
        });
        Ok(())
    });
    registry.register("closeDialog", |_args, queue| {
        queue.push(SystemReactionCommand::PopTree);
        Ok(())
    });
    registry.register("loadLevel", |args, queue| {
        let parsed: LoadLevelArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("loadLevel: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::LoadLevel {
            map: parsed.map,
            dispatch_source: queue.fire_context().source,
        });
        Ok(())
    });
    registry.register("restartLevel", |_args, queue| {
        queue.push(SystemReactionCommand::RestartLevel {
            dispatch_source: queue.fire_context().source,
        });
        Ok(())
    });
    registry.register("returnToFrontend", |_args, queue| {
        queue.push(SystemReactionCommand::ReturnToFrontend);
        Ok(())
    });
    // `setState` writes a value to a writable store slot at the game-logic stage
    // (M13 Goal F, Task 4). It carries no `tag` (system-targeted); the drain
    // applies it through the readonly-gated JSON write. The slider widget emits
    // this on a captured nav step; scripts may fire it as a named reaction.
    registry.register("setState", |args, queue| {
        let parsed: SetStateArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("setState: failed to deserialize args: {e}"),
            })?;
        let fire_context = queue.fire_context();
        queue.push(SystemReactionCommand::SetState {
            slot: parsed.slot,
            value: parsed.value,
            dispatch_source: fire_context.source,
            dispatch_values: fire_context.values,
        });
        Ok(())
    });
    // Sentiment writes are consequential system reactions like `setState`, but
    // they intentionally stay on this app-drain route even for trigger fires:
    // the overlay is not a tick-context slot table, so the next AI tick is the
    // first reader that can observe the write.
    registry.register("setSentiment", |args, queue| {
        let parsed: SetSentimentArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("setSentiment: failed to deserialize args: {e}"),
            })?;
        if !parsed.value.is_finite() {
            return Err(ReactionError::InvalidArgument {
                reason: format!("setSentiment: value must be finite, got {}", parsed.value),
            });
        }
        queue.push(SystemReactionCommand::SetSentiment {
            from: parsed.from,
            to: parsed.to,
            value: parsed.value,
        });
        Ok(())
    });
    registry.register("adjustSentiment", |args, queue| {
        let parsed: AdjustSentimentArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("adjustSentiment: failed to deserialize args: {e}"),
            })?;
        if !parsed.delta.is_finite() {
            return Err(ReactionError::InvalidArgument {
                reason: format!(
                    "adjustSentiment: delta must be finite, got {}",
                    parsed.delta
                ),
            });
        }
        queue.push(SystemReactionCommand::AdjustSentiment {
            from: parsed.from,
            to: parsed.to,
            delta: parsed.delta,
        });
        Ok(())
    });
    // `setGhostAudio` (from `postretro/audio`) is a host consequence like the
    // sentiment writes: it drains at frame end from every source and the host
    // applies it at its next fixed tick. A malformed call drops whole here.
    registry.register(postretro_foundation::SET_GHOST_AUDIO, |args, queue| {
        let targets = crate::ghost_audio_command::parse_set_ghost_audio(args)
            .map_err(|reason| ReactionError::InvalidArgument { reason })?;
        queue.push(SystemReactionCommand::SetGhostAudio(targets));
        Ok(())
    });
    // `cellWrite` writes a presentation cell at the game-logic stage.
    // It carries no `tag` (system-targeted); the drain routes it into the
    // app-side `PresentationCellStore`, NOT the slot table. Distinct from
    // `setState` (which writes the authoritative store); the `ui.createLocalState`
    // handle's `.set(v)` emits this, never `setState`.
    registry.register("cellWrite", |args, queue| {
        let parsed: CellWriteArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("cellWrite: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::CellWrite {
            scope: parsed.scope,
            cell: parsed.cell,
            value: parsed.value,
        });
        Ok(())
    });
    // Text-edit reactions (M13 Text Entry, Task 1): `appendText` / `backspaceText`
    // / `clearText` mutate a writable String slot at the game-logic stage through
    // the same readonly-gated path as `setState`. No `tag` — system-targeted. The
    // hardware-keyboard and on-screen-keyboard paths both fire these against
    // `ui.textEntry`.
    registry.register("appendText", |args, queue| {
        let parsed: AppendTextArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("appendText: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::AppendText {
            slot: parsed.slot,
            text: parsed.text,
        });
        Ok(())
    });
    registry.register("backspaceText", |args, queue| {
        let parsed: SlotOnlyArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("backspaceText: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::BackspaceText { slot: parsed.slot });
        Ok(())
    });
    registry.register("clearText", |args, queue| {
        let parsed: SlotOnlyArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("clearText: failed to deserialize args: {e}"),
            })?;
        queue.push(SystemReactionCommand::ClearText { slot: parsed.slot });
        Ok(())
    });
}

// --- args shapes ------------------------------------------------------------
// Script-facing camelCase keys; absent optionals fall through serde defaults.

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaySoundArgs {
    #[serde(default)]
    audience: Option<postretro_entities::Audience>,
    sound: String,
    #[serde(default)]
    bus: Option<String>,
    /// `"@emitter"` when the reaction authored `at: on.emitter`.
    #[serde(default)]
    at: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RumbleArgs {
    #[serde(default)]
    audience: Option<postretro_entities::Audience>,
    strong: f32,
    #[serde(default)]
    weak: Option<f32>,
    duration_ms: f32,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlashScreenArgs {
    #[serde(default)]
    audience: Option<postretro_entities::Audience>,
    color: [f32; 4],
    duration_ms: f32,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct VignetteArgs {
    #[serde(default)]
    audience: Option<postretro_entities::Audience>,
    // Absent ⇒ the drain defaults to black (strength-only edge-darken).
    #[serde(default)]
    color: Option<[f32; 3]>,
    strength: f32,
    duration_ms: f32,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScreenShakeArgs {
    #[serde(default)]
    audience: Option<postretro_entities::Audience>,
    amplitude: f32,
    duration_ms: f32,
    // Absent ⇒ `None` rides to the driver, which applies its 18 Hz default. The
    // default is NOT applied here.
    #[serde(default)]
    frequency: Option<f32>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShowDialogArgs {
    tree: String,
    #[serde(default)]
    on_commit: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenMenuArgs {
    tree: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadLevelArgs {
    map: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetStateArgs {
    slot: String,
    value: serde_json::Value,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetSentimentArgs {
    from: String,
    to: String,
    value: f32,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdjustSentimentArgs {
    from: String,
    to: String,
    delta: f32,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CellWriteArgs {
    scope: String,
    cell: String,
    value: serde_json::Value,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppendTextArgs {
    slot: String,
    text: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SlotOnlyArgs {
    slot: String,
}

mod ir_bindings;
pub use ir_bindings::{SystemReactionIrBindings, SystemReactionIrDispatch, is_ir_node};

#[cfg(test)]
mod tests;
