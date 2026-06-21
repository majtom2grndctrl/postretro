// Per-frame bound-value diff + tween drivers for the retained UI tree. Resolves
// each bound node against the frame snapshot, easing tweened display values and
// classifying each change as content (relayout) or appearance (redraw) only.
// See: context/lib/ui.md §3 (display vs. authoritative value / tween contract)

use std::collections::HashMap;

use super::super::descriptor::{Easing, PanelBind, SliderBind, TextBind};
use super::super::theme::UiTheme;
use crate::scripting::slot_table::SlotValue;

use super::CellValues;
use super::draw::{
    bar_max_value, bar_slot_value, bind_target_name, resolve_panel_fill, resolve_text,
};
use super::predicate::lookup_bound;
use super::style::{TweenState, apply, lerp_rgba};

/// Walk-invariant context threaded through `collect_node`'s recursion: the
/// device-pixel projection, the slot snapshot the draw reads, the dt-accumulated
/// UI clock, and the inert theme the (pre-resolved) styleRanges evaluator takes.
pub(crate) struct DrawWalkCtx<'a> {
    pub canvas_origin: [f32; 2],
    pub scale: f32,
    pub slot_values: &'a HashMap<String, SlotValue>,
    /// Presentation-cell values for `{ local }` bind resolution.
    pub cell_values: &'a CellValues,
    pub time_seconds: f64,
    pub inert_theme: &'a UiTheme,
}

/// Result of one retained-frame bound-value diff. Each flag is set when at least
/// one bound node of that class changed since the previous diff. `content_changed`
/// is layout-affecting (forces a relayout); `appearance_changed` is appearance-
/// only (forces a draw-list rebuild but never a relayout).
#[derive(Default)]
pub(crate) struct BindingDiff {
    pub content_changed: bool,
    pub appearance_changed: bool,
}

/// Drive one bound TEXT node for this frame and return whether its rendered
/// (`last_resolved`) string changed since the last diff. Three paths:
///
/// - **Untweened** (`bind.tween` is `None`): the original behavior — resolve the
///   string via `resolve_text`, store it, report change. `tween` stays `None`.
/// - **Tweened, slot resolves to a `Number`**: the number is the eased target.
///   `drive_tween_f32` advances the per-node `f32` display from its segment; the
///   rounded display is formatted through `bind.format`'s `{}` (same integral
///   formatting as `slot_value_string`) and stored as the rendered string, so the
///   measure seam shapes the displayed value.
/// - **Tweened, slot resolves to any other shape**: snap-through — render via the
///   unchanged `resolve_text` path and log one `log::warn!` per retained frame
///   (the node is visited once per `resolve_bindings` call; there is no cross-frame
///   dedup, matching the `resolve_panel_fill` precedent).
///
/// `now` is the frame's `time_seconds`.
// Wide by necessity: bind + its resolved scope + the node's mutable display state
// (content/last_resolved/tween) + both value maps (slots, cells) + the frame
// clock are all distinct per-node diff inputs; a struct would only obscure them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_text_binding(
    bind: &TextBind,
    bind_scope: Option<&str>,
    content: &str,
    last_resolved: &mut Option<String>,
    tween: &mut Option<TweenState<f32>>,
    slot_values: &HashMap<String, SlotValue>,
    cell_values: &CellValues,
    now: f64,
) -> bool {
    let Some(cfg) = bind.tween.as_ref() else {
        // No tween config: the untweened path, byte-for-byte as before.
        let resolved = resolve_text(Some(bind), bind_scope, content, slot_values, cell_values);
        let changed = last_resolved.as_deref() != Some(resolved.as_str());
        if changed {
            *last_resolved = Some(resolved);
        }
        return changed;
    };

    let rendered = match lookup_bound(&bind.source, bind_scope, slot_values, cell_values) {
        Some(SlotValue::Number(n)) => {
            // Tweenable: the number is the eased target. Advance the display value
            // and render the rounded integer through the format template.
            let target = *n;
            let display =
                drive_tween_f32(tween, cfg.from, target, cfg.duration_ms, cfg.easing, now);
            let integral = display.round() as i64;
            match &bind.format {
                Some(template) => template.replacen("{}", &integral.to_string(), 1),
                None => integral.to_string(),
            }
        }
        _ => {
            // A tween on a non-`Number` slot (or an absent slot): snap through the
            // unchanged resolution path, warning once per retained frame. An absent
            // slot is the normal fallback case and does NOT warn (`resolve_text`
            // already treats absence silently); only a present, non-numeric value
            // is an authoring error worth the warn.
            if lookup_bound(&bind.source, bind_scope, slot_values, cell_values).is_some() {
                warn_non_tweenable_text(bind);
            }
            resolve_text(Some(bind), bind_scope, content, slot_values, cell_values)
        }
    };

    let changed = last_resolved.as_deref() != Some(rendered.as_str());
    if changed {
        *last_resolved = Some(rendered);
    }
    changed
}

/// Advance (or initialize / retarget) a text node's `f32` tween segment toward
/// `target` at frame time `now`, returning the eased display value for this frame.
/// Stores the advanced state back into `tween`. See `resolve_bindings` for the
/// first-resolve / retarget / in-flight / settle mechanics.
pub(crate) fn drive_tween_f32(
    tween: &mut Option<TweenState<f32>>,
    from: Option<f32>,
    target: f32,
    duration_ms: f32,
    easing: Easing,
    now: f64,
) -> f32 {
    match tween {
        None => {
            // First resolution. With `from` present, start there and ease toward
            // the target (the level-load flourish); with `from` absent, snap to the
            // target (no tween on first sight) by seeding a settled segment.
            let start = from.unwrap_or(target);
            let mut state = TweenState {
                display: start,
                start,
                start_time: now,
                target,
            };
            state.display = advance_f32(&state, duration_ms, easing, now);
            let display = state.display;
            *tween = Some(state);
            display
        }
        Some(state) => {
            // Retarget: a new target restarts the segment from the CURRENT display
            // (never snapping mid-flight) at this frame's time.
            if state.target != target {
                state.start = state.display;
                state.start_time = now;
                state.target = target;
            }
            state.display = advance_f32(state, duration_ms, easing, now);
            state.display
        }
    }
}

/// Sample a text tween segment at `now`: normalized progress `(now - start_time) /
/// duration`, eased, lerped from `start` to `target`. At `t >= 1` (including a
/// non-positive duration) the value equals `target` EXACTLY so the settle is bit-
/// exact. `duration_ms` is milliseconds; converted to seconds for the f64 clock.
fn advance_f32(state: &TweenState<f32>, duration_ms: f32, easing: Easing, now: f64) -> f32 {
    let duration = (duration_ms as f64) / 1000.0;
    if duration <= 0.0 || now - state.start_time >= duration {
        return state.target;
    }
    let t = ((now - state.start_time) / duration) as f32;
    let e = apply(easing, t);
    state.start + (state.target - state.start) * e
}

/// Log the snap-through warning for a text tween whose slot resolved to a
/// non-`Number` shape. Fires once per retained frame: the caller visits each node
/// once per `resolve_bindings` call (one per retained frame) and there is no
/// cross-frame dedup, matching the `resolve_panel_fill` precedent. The snap itself
/// renders via `resolve_text`, so this never touches the tween state.
fn warn_non_tweenable_text(bind: &TextBind) {
    log::warn!(
        "[UI] text bind '{}' carries a tween but did not resolve to a Number; \
         rendering the raw value without easing",
        bind_target_name(&bind.source),
    );
}

/// Drive one bound PANEL node for this frame and return whether its rendered
/// (`last_resolved`) fill changed since the last diff. Mirrors
/// `drive_text_binding`:
///
/// - **Untweened**: resolve the fill via `resolve_panel_fill`, store, report.
/// - **Tweened, slot resolves to a length-4 `Array`**: the array is the eased
///   target; `drive_tween_rgba` advances the per-node `[f32; 4]` display
///   per-channel (alpha included, no rounding) and stores it as the rendered fill.
/// - **Tweened, slot resolves to any other shape**: snap through the unchanged
///   `resolve_panel_fill` path (which already warns once per retained frame on a
///   present-but-malformed slot) — no extra tween warn, since that path owns the
///   warning.
// Wide by necessity: see `drive_text_binding` — same per-node diff input set.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_panel_binding(
    bind: &PanelBind,
    bind_scope: Option<&str>,
    fallback: [f32; 4],
    last_resolved: &mut Option<[f32; 4]>,
    tween: &mut Option<TweenState<[f32; 4]>>,
    slot_values: &HashMap<String, SlotValue>,
    cell_values: &CellValues,
    now: f64,
) -> bool {
    let Some(cfg) = bind.tween.as_ref() else {
        let resolved =
            resolve_panel_fill(Some(bind), bind_scope, fallback, slot_values, cell_values);
        let changed = last_resolved.is_none_or(|prev| !colors_eq(prev, resolved));
        if changed {
            *last_resolved = Some(resolved);
        }
        return changed;
    };

    let resolved = match lookup_bound(&bind.source, bind_scope, slot_values, cell_values) {
        Some(SlotValue::Array(rgba)) if rgba.len() == 4 => {
            let target = [rgba[0], rgba[1], rgba[2], rgba[3]];
            drive_tween_rgba(tween, cfg.from, target, cfg.duration_ms, cfg.easing, now)
        }
        // Non-tweenable shape (absent, wrong variant, or wrong length): snap
        // through the unchanged fill-resolution path. `resolve_panel_fill` already
        // owns the once-per-frame warn for a present-but-malformed value, so the
        // tween adds none here.
        _ => resolve_panel_fill(Some(bind), bind_scope, fallback, slot_values, cell_values),
    };

    let changed = last_resolved.is_none_or(|prev| !colors_eq(prev, resolved));
    if changed {
        *last_resolved = Some(resolved);
    }
    changed
}

/// Drive one bound BAR node for this frame and return whether its displayed value
/// changed since the last diff. Mirrors the tweened-text numeric path but stores
/// an `f32` display value (the bar draws a fill fraction, not a string):
///
/// - **Untweened**: the displayed value is the raw slot `Number` (or `0.0` when
///   absent/non-numeric); store it, report the change.
/// - **Tweened, slot resolves to a `Number`**: `drive_tween_f32` eases a per-node
///   display value toward the slot target so the rendered fill fraction eases.
/// - **Tweened, slot resolves to any other shape**: snap to the raw value (`0.0`).
pub(crate) fn drive_bar_binding(
    bind: &SliderBind,
    bind_scope: Option<&str>,
    last_resolved: &mut Option<f32>,
    tween: &mut Option<TweenState<f32>>,
    slot_values: &HashMap<String, SlotValue>,
    cell_values: &CellValues,
    now: f64,
) -> bool {
    let resolved = match bind.tween.as_ref() {
        Some(cfg) => match lookup_bound(&bind.source, bind_scope, slot_values, cell_values) {
            Some(SlotValue::Number(n)) => {
                drive_tween_f32(tween, cfg.from, *n, cfg.duration_ms, cfg.easing, now)
            }
            _ => bar_slot_value(bind, bind_scope, slot_values, cell_values),
        },
        None => bar_slot_value(bind, bind_scope, slot_values, cell_values),
    };
    let changed = last_resolved.is_none_or(|prev| (prev - resolved).abs() > f32::EPSILON);
    if changed {
        *last_resolved = Some(resolved);
    }
    changed
}

/// Drive a bar's denominator dependency. Literal max values settle after the
/// first retained diff; state-backed max values subscribe to their slot and
/// rebuild the draw list when only the denominator changes.
pub(crate) fn drive_bar_max(
    max: &super::super::descriptor::BarMax,
    last_resolved: &mut Option<f32>,
    slot_values: &HashMap<String, SlotValue>,
) -> bool {
    let resolved = bar_max_value(max, slot_values);
    let changed = last_resolved.is_none_or(|prev| (prev - resolved).abs() > f32::EPSILON);
    if changed {
        *last_resolved = Some(resolved);
    }
    changed
}

/// Advance (or initialize / retarget) a panel node's RGBA tween segment toward
/// `target` at frame time `now`, returning the eased per-channel display fill.
/// Same first-resolve / retarget / in-flight / settle mechanics as the text
/// driver, but the lerp runs per channel (alpha included, no rounding).
fn drive_tween_rgba(
    tween: &mut Option<TweenState<[f32; 4]>>,
    from: Option<[f32; 4]>,
    target: [f32; 4],
    duration_ms: f32,
    easing: Easing,
    now: f64,
) -> [f32; 4] {
    match tween {
        None => {
            let start = from.unwrap_or(target);
            let mut state = TweenState {
                display: start,
                start,
                start_time: now,
                target,
            };
            state.display = advance_rgba(&state, duration_ms, easing, now);
            let display = state.display;
            *tween = Some(state);
            display
        }
        Some(state) => {
            if state.target != target {
                state.start = state.display;
                state.start_time = now;
                state.target = target;
            }
            state.display = advance_rgba(state, duration_ms, easing, now);
            state.display
        }
    }
}

/// Sample a panel tween segment at `now`: eased fraction (as `advance_f32`),
/// applied per channel via `lerp_rgba`. At `t >= 1` (or a non-positive duration)
/// the fill equals `target` EXACTLY so the settle is bit-exact.
fn advance_rgba(
    state: &TweenState<[f32; 4]>,
    duration_ms: f32,
    easing: Easing,
    now: f64,
) -> [f32; 4] {
    let duration = (duration_ms as f64) / 1000.0;
    if duration <= 0.0 || now - state.start_time >= duration {
        return state.target;
    }
    let t = ((now - state.start_time) / duration) as f32;
    let e = apply(easing, t);
    lerp_rgba(state.start, state.target, e)
}

/// Exact per-channel equality for a resolved fill. The diff compares the resolved
/// color against the last-resolved one to decide whether the appearance changed;
/// both sides come from the same resolution path (slot array or literal fallback),
/// so bit-identical values compare equal and the flash settling to a constant
/// color stops re-flagging.
pub(crate) fn colors_eq(a: [f32; 4], b: [f32; 4]) -> bool {
    a == b
}
