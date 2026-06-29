// Continuous value→style mapping for HUD widgets (M13 Goal E, `styleRanges`):
// the `StyleRanges` descriptor types and the pure evaluator that turns a rendered
// value into a resolved color plus pulse/flash alpha effects. Widget-agnostic by
// design — it knows a value, a max, the ordered entries, the theme, a base color,
// and per-node effect state — so Goal F's `bar` and any text/panel widget call it
// unchanged. Pure data + a pure-ish evaluator: no taffy, no GPU, no store write.
// See: context/lib/ui.md §3

use serde::{Deserialize, Serialize};

use super::descriptor::ColorValue;
use super::theme::UiTheme;

/// A continuous value→style map carried by a `text`/`panel` widget. The value the
/// widget renders is normalized against `max` (`value / max`) and matched against
/// `entries` in order: the FIRST entry whose `up_to` bound covers the fraction
/// wins; a trailing entry with no `up_to` is the default for any value above every
/// bound. Appearance-only — a styleRange change drives a redraw, never a relayout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StyleRanges {
    /// Denominator the rendered value is normalized against (`value / max`). The
    /// entry `up_to` bounds are fractions of this — e.g. `max: 100`, `upTo: 0.25`
    /// matches a rendered value at or below 25.
    pub max: f32,
    /// Ordered match list. First covering entry wins; a trailing no-`upTo` entry
    /// is the default. An empty list resolves to the widget's own base color.
    pub entries: Vec<StyleEntry>,
}

/// One band in a `StyleRanges` map. `up_to` is the inclusive upper bound (a
/// fraction of `max`) this band covers; `None` makes it the trailing default
/// (matches any value above every prior bound). `color` overrides the widget's
/// base color while the band is active (`None` keeps the base color, so a band may
/// add only an effect). `pulse`/`flash` are the optional alpha effects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StyleEntry {
    /// Inclusive upper bound as a fraction of `max`. `None` is the trailing
    /// default band. Omitted on the wire when `None` (skip_serializing_if).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up_to: Option<f32>,
    /// Color override while this band is active; `None` keeps the widget's base
    /// color. A token resolves against the active theme (unknown → magenta + warn,
    /// the existing theme rule); a literal is its own RGBA. Omitted when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorValue>,
    /// Sinusoidal alpha pulse while this band is active. Omitted when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pulse: Option<Pulse>,
    /// One-shot alpha spike on entry into this band, decaying over its duration.
    /// Omitted when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flash: Option<Flash>,
}

/// Sinusoidal alpha pulse: the resolved color's alpha oscillates over `period_ms`.
/// Clocked by dt-accumulated game time, so pausing game logic pauses the pulse.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pulse {
    /// Full oscillation period in milliseconds.
    pub period_ms: f32,
}

/// One-shot alpha flash: on entry into the band the resolved color's alpha spikes
/// to full and decays linearly back over `duration_ms`. Re-arms only on a fresh
/// entry into the band (not while the band stays active).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Flash {
    /// Decay duration in milliseconds.
    pub duration_ms: f32,
}

/// Per-node effect state the evaluator clocks across frames. Lives on the node's
/// `NodeContext` beside the tween state, clocked by the snapshot's `time_seconds`
/// (dt-accumulated game time — pausing game logic freezes both effects). Born on
/// the first evaluation; carries the active-band identity to detect a fresh entry
/// (which re-arms the flash) and the flash's entry clock.
#[derive(Debug, Clone, Default)]
pub struct StyleEffectState {
    /// Index of the band active on the previous evaluation, or `None` before the
    /// first evaluation / when no band matched. A change re-arms the flash.
    active_band: Option<usize>,
    /// Frame time (seconds) the active band was entered. Drives the flash decay;
    /// `None` until the first entry into a flashing band.
    flash_entered_at: Option<f64>,
}

/// Resolve a `StyleRanges` map at a rendered `value`, returning the band's color
/// (theme-resolved, base-color fallback) with any pulse/flash applied to its
/// alpha. Pure w.r.t. the descriptor and theme; advances `state` for the per-node
/// flash entry clock. `base` is the widget's own resolved color (used when the
/// matched band names no color, or when no band matches). `now` is the frame's
/// dt-accumulated `time_seconds`.
///
/// Widget-agnostic: it never sees the widget, only the value to map. Goal F's
/// `bar` and the text/panel draw build call it the same way.
pub fn evaluate(
    ranges: &StyleRanges,
    value: f32,
    base: [f32; 4],
    theme: &UiTheme,
    state: &mut StyleEffectState,
    now: f64,
) -> [f32; 4] {
    let matched = select_entry(ranges, value);

    // Detect a fresh entry into the matched band to re-arm the flash. A band
    // index change (including first match) is an entry; staying in the same band
    // is not. `None` (no match) clears the active band so re-entering re-arms.
    if state.active_band != matched {
        state.active_band = matched;
        state.flash_entered_at = matched.map(|_| now);
    }

    let Some(idx) = matched else {
        // No band matched (an empty or all-bounded entry list with a value past
        // every bound): the widget keeps its own base color, no effect.
        return base;
    };
    let entry = &ranges.entries[idx];

    // Band color overrides the base; an absent color keeps the base color so a
    // band may carry only an effect. A token resolves through the existing theme
    // rule (unknown → opaque magenta + one warn).
    let mut color = match &entry.color {
        Some(value) => super::tree::resolve_color(value, theme),
        None => base,
    };

    // Effects modulate the resolved color's alpha only. Pulse and flash compose
    // multiplicatively when both are present (flash spike scales the pulsed
    // alpha), so a flashing pulse still reads as a flash on entry.
    if let Some(pulse) = &entry.pulse {
        color[3] = (color[3] * pulse_factor(pulse, now)).clamp(0.0, 1.0);
    }
    if let Some(flash) = &entry.flash {
        if let Some(entered) = state.flash_entered_at {
            color[3] = (color[3] * flash_factor(flash, now - entered)).clamp(0.0, 1.0);
        }
    }
    color
}

/// Select the first entry whose `up_to` fraction covers `value / max`, or the
/// trailing no-`up_to` default. Returns the entry index, or `None` when no band
/// covers the value (every entry is bounded and the value exceeds them all, or the
/// list is empty). Pure — the core value→band decision, with no effect state.
///
/// A non-positive `max` makes the fraction degenerate; the value is then treated
/// as `0.0` so the lowest band wins (a sane floor rather than a NaN/inf).
fn select_entry(ranges: &StyleRanges, value: f32) -> Option<usize> {
    let fraction = if ranges.max > 0.0 {
        value / ranges.max
    } else {
        0.0
    };
    ranges.entries.iter().position(|entry| match entry.up_to {
        Some(bound) => fraction <= bound,
        // A no-`up_to` entry is the trailing default: it covers anything that
        // reached it (every prior bounded entry already failed to match).
        None => true,
    })
}

/// Sinusoidal alpha factor in `[0, 1]` for a pulse at frame time `now`. A full
/// `period_ms` cycles the factor `1 → 0 → 1` (cosine, starting bright). A
/// non-positive period holds the color steady (factor 1).
fn pulse_factor(pulse: &Pulse, now: f64) -> f32 {
    if pulse.period_ms <= 0.0 {
        return 1.0;
    }
    let period = (pulse.period_ms as f64) / 1000.0;
    let phase = (now % period) / period; // 0..1 within the cycle
    // Cosine eased to [0, 1]: starts at 1, dips to 0 at the half-cycle, back to 1.
    let factor = 0.5 + 0.5 * (phase * std::f64::consts::TAU).cos();
    factor as f32
}

/// Linear flash-decay factor in `[1, 0]` for a flash `elapsed` seconds after entry.
/// At entry (`elapsed == 0`) the factor is `1.0` (full alpha spike); it decays
/// linearly to `0.0` at `duration_ms`, then holds at `0.0`. A non-positive
/// duration collapses to an instantaneous spike (factor 0 immediately after entry).
fn flash_factor(flash: &Flash, elapsed: f64) -> f32 {
    let duration = (flash.duration_ms as f64) / 1000.0;
    if duration <= 0.0 {
        return if elapsed <= 0.0 { 1.0 } else { 0.0 };
    }
    let remaining = 1.0 - (elapsed / duration);
    (remaining.clamp(0.0, 1.0)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Linear-RGBA approximate equality (floats — never compare exact).
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    fn entry(up_to: Option<f32>, color: Option<ColorValue>) -> StyleEntry {
        StyleEntry {
            up_to,
            color,
            pulse: None,
            flash: None,
        }
    }

    /// Three-band health map: red ≤ 0.25, amber ≤ 0.5, default green.
    fn health_ranges() -> StyleRanges {
        StyleRanges {
            max: 100.0,
            entries: vec![
                entry(Some(0.25), Some(ColorValue::Literal([1.0, 0.0, 0.0, 1.0]))),
                entry(Some(0.5), Some(ColorValue::Literal([1.0, 1.0, 0.0, 1.0]))),
                entry(None, Some(ColorValue::Literal([0.0, 1.0, 0.0, 1.0]))),
            ],
        }
    }

    fn theme() -> UiTheme {
        UiTheme::engine_default()
    }

    #[test]
    fn first_matching_entry_wins_at_low_value() {
        // value/max = 0.10 ≤ 0.25, so the FIRST band (red) wins even though it
        // also satisfies the later 0.5 band — order decides.
        let ranges = health_ranges();
        let mut state = StyleEffectState::default();
        let c = evaluate(&ranges, 10.0, [1.0; 4], &theme(), &mut state, 0.0);
        assert!(close(c[0], 1.0) && close(c[1], 0.0) && close(c[2], 0.0));
    }

    #[test]
    fn middle_band_matches_its_fraction() {
        // value/max = 0.40: past 0.25 (red), within 0.5 (amber).
        let ranges = health_ranges();
        let mut state = StyleEffectState::default();
        let c = evaluate(&ranges, 40.0, [1.0; 4], &theme(), &mut state, 0.0);
        assert!(close(c[0], 1.0) && close(c[1], 1.0) && close(c[2], 0.0));
    }

    #[test]
    fn trailing_default_entry_covers_values_above_every_bound() {
        // value/max = 0.90 exceeds both bounded bands, so the trailing no-`upTo`
        // default (green) wins.
        let ranges = health_ranges();
        let mut state = StyleEffectState::default();
        let c = evaluate(&ranges, 90.0, [1.0; 4], &theme(), &mut state, 0.0);
        assert!(close(c[0], 0.0) && close(c[1], 1.0) && close(c[2], 0.0));
    }

    #[test]
    fn boundary_value_matches_its_band_inclusively() {
        // Exactly at the bound (value/max = 0.25) the first band still covers it —
        // the bound is inclusive (`fraction <= up_to`).
        let ranges = health_ranges();
        let mut state = StyleEffectState::default();
        let c = evaluate(&ranges, 25.0, [1.0; 4], &theme(), &mut state, 0.0);
        assert!(close(c[0], 1.0) && close(c[1], 0.0) && close(c[2], 0.0));
    }

    #[test]
    fn fraction_uses_value_over_max_not_raw_value() {
        // A max of 50 puts a raw value of 20 at fraction 0.4 → amber, where the
        // same raw value against max 100 would be 0.2 → red. The fraction, not the
        // raw value, drives the match.
        let ranges = StyleRanges {
            max: 50.0,
            ..health_ranges()
        };
        let mut state = StyleEffectState::default();
        let c = evaluate(&ranges, 20.0, [1.0; 4], &theme(), &mut state, 0.0);
        assert!(
            close(c[1], 1.0) && close(c[2], 0.0),
            "amber at fraction 0.4"
        );
    }

    #[test]
    fn entry_without_color_keeps_the_widget_base_color() {
        // A band that names no color keeps the widget's base color (it may carry
        // only an effect). Here the matched band has no color, so base passes through.
        let ranges = StyleRanges {
            max: 100.0,
            entries: vec![entry(None, None)],
        };
        let mut state = StyleEffectState::default();
        let base = [0.2, 0.4, 0.6, 1.0];
        let c = evaluate(&ranges, 50.0, base, &theme(), &mut state, 0.0);
        assert_eq!(c, base);
    }

    #[test]
    fn unknown_color_token_degrades_to_magenta() {
        // A band color naming an unknown token degrades per the existing theme
        // rule: opaque magenta (the unknown-color fallback).
        let ranges = StyleRanges {
            max: 100.0,
            entries: vec![entry(None, Some(ColorValue::Token("no.such.token".into())))],
        };
        let mut state = StyleEffectState::default();
        let c = evaluate(&ranges, 50.0, [1.0; 4], &theme(), &mut state, 0.0);
        assert!(close(c[0], 1.0) && close(c[1], 0.0) && close(c[2], 1.0) && close(c[3], 1.0));
    }

    #[test]
    fn no_band_matches_returns_base_color() {
        // Every entry is bounded and the value exceeds all of them: no band wins,
        // so the widget keeps its base color.
        let ranges = StyleRanges {
            max: 100.0,
            entries: vec![entry(
                Some(0.25),
                Some(ColorValue::Literal([1.0, 0.0, 0.0, 1.0])),
            )],
        };
        let mut state = StyleEffectState::default();
        let base = [0.1, 0.2, 0.3, 1.0];
        let c = evaluate(&ranges, 90.0, base, &theme(), &mut state, 0.0);
        assert_eq!(c, base);
    }

    #[test]
    fn pulse_modulates_alpha_sinusoidally_over_its_period() {
        // A 1000ms pulse: at t=0 alpha is full (cos starts at 1), at the half
        // period (500ms) the factor dips to 0, and at the full period it returns.
        let ranges = StyleRanges {
            max: 100.0,
            entries: vec![StyleEntry {
                up_to: None,
                color: Some(ColorValue::Literal([1.0, 1.0, 1.0, 1.0])),
                pulse: Some(Pulse { period_ms: 1000.0 }),
                flash: None,
            }],
        };
        let mut state = StyleEffectState::default();
        let at0 = evaluate(&ranges, 50.0, [1.0; 4], &theme(), &mut state, 0.0);
        assert!(close(at0[3], 1.0), "pulse starts bright");
        let at_half = evaluate(&ranges, 50.0, [1.0; 4], &theme(), &mut state, 0.5);
        assert!(at_half[3] < 0.01, "pulse troughs at the half period");
        let at_full = evaluate(&ranges, 50.0, [1.0; 4], &theme(), &mut state, 1.0);
        assert!(
            close(at_full[3], 1.0),
            "pulse returns to bright at the period"
        );
    }

    #[test]
    fn flash_spikes_alpha_on_entry_and_decays_over_duration() {
        // Crossing into a flashing band spikes alpha to full at entry, decaying
        // linearly to 0 over the 200ms duration. The band only flashes on a FRESH
        // entry (a band-index change), so we enter from a different band first.
        let ranges = StyleRanges {
            max: 100.0,
            entries: vec![
                StyleEntry {
                    up_to: Some(0.25),
                    color: Some(ColorValue::Literal([1.0, 0.0, 0.0, 1.0])),
                    pulse: None,
                    flash: Some(Flash { duration_ms: 200.0 }),
                },
                entry(None, Some(ColorValue::Literal([0.0, 1.0, 0.0, 1.0]))),
            ],
        };
        let mut state = StyleEffectState::default();
        // Start in the default (green) band — no flash there.
        evaluate(&ranges, 90.0, [1.0; 4], &theme(), &mut state, 0.0);
        // Cross into the red flashing band at t=1.0s: alpha spikes to full.
        let on_entry = evaluate(&ranges, 10.0, [1.0; 4], &theme(), &mut state, 1.0);
        assert!(close(on_entry[3], 1.0), "alpha spikes on entry");
        // Halfway through the 200ms decay: alpha is ~half.
        let mid = evaluate(&ranges, 10.0, [1.0; 4], &theme(), &mut state, 1.1);
        assert!(close(mid[3], 0.5), "alpha half-decayed at half duration");
        // Past the duration: the flash has fully decayed.
        let after = evaluate(&ranges, 10.0, [1.0; 4], &theme(), &mut state, 1.3);
        assert!(after[3] < 0.01, "flash fully decayed past its duration");
    }

    #[test]
    fn flash_does_not_rearm_while_the_band_stays_active() {
        // Staying in the same band must NOT re-arm the flash: the decay keeps
        // running from the original entry, it does not restart each frame.
        let ranges = StyleRanges {
            max: 100.0,
            entries: vec![StyleEntry {
                up_to: None,
                color: Some(ColorValue::Literal([1.0, 0.0, 0.0, 1.0])),
                pulse: None,
                flash: Some(Flash { duration_ms: 200.0 }),
            }],
        };
        let mut state = StyleEffectState::default();
        // First evaluation enters the band at t=0 (a fresh entry → flash arms).
        let on_entry = evaluate(&ranges, 50.0, [1.0; 4], &theme(), &mut state, 0.0);
        assert!(close(on_entry[3], 1.0));
        // Re-evaluating the SAME band at t=0.3 (past the 200ms duration) shows a
        // decayed flash, proving it did not re-arm.
        let later = evaluate(&ranges, 50.0, [1.0; 4], &theme(), &mut state, 0.3);
        assert!(
            later[3] < 0.01,
            "flash did not re-arm while the band stayed active"
        );
    }

    #[test]
    fn style_ranges_round_trips_through_serde_json() {
        // The wire form is locked: camelCase, `upTo`/`periodMs`/`durationMs`, with
        // absent optionals omitted. A descriptor with all fields round-trips.
        let json = r#"{"max":100.0,"entries":[{"upTo":0.25,"color":[1.0,0.0,0.0,1.0],"pulse":{"periodMs":800.0},"flash":{"durationMs":200.0}},{"color":"warning"}]}"#;
        let ranges: StyleRanges = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&ranges).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    // AC-3 contract (M13 Goal E): while a tween is active, styleRanges evaluate
    // the eased DISPLAY value, but the crossing detector evaluates the
    // AUTHORITATIVE slot. Mid-tween the two diverge — the HUD still shows the
    // pre-damage band color while the crossing has already fired off the true
    // slot value. This is a render/scripting seam test: it drives the real
    // styleRanges evaluator and the real `CrossingDetector` against one slot.
    #[test]
    fn styleranges_display_value_and_crossing_authoritative_slot_diverge_mid_tween() {
        use crate::state_crossings::CrossingDetector;
        use postretro_entities::{
            CrossingCondition, CrossingDescriptor, DataRegistry, ReplicationScope, SlotOwnership,
            SlotRecord, SlotSchema, SlotTable, SlotType, SlotValue,
        };

        // Health styleRanges: red ≤ 0.2 of max, default green above.
        let ranges = StyleRanges {
            max: 100.0,
            entries: vec![
                StyleEntry {
                    up_to: Some(0.2),
                    color: Some(ColorValue::Literal([1.0, 0.0, 0.0, 1.0])),
                    pulse: None,
                    flash: None,
                },
                entry(None, Some(ColorValue::Literal([0.0, 1.0, 0.0, 1.0]))),
            ],
        };

        // The authoritative `player.health` slot: a mod-style Number slot under
        // a fresh namespace seeded at full health.
        let mut slot_table = SlotTable::new();
        slot_table
            .insert(
                "hud.health".to_string(),
                SlotRecord {
                    schema: SlotSchema {
                        slot_type: SlotType::Number,
                        default: None,
                        range: None,
                        persist: false,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                        network: ReplicationScope::None,
                    },
                    value: Some(SlotValue::Number(100.0)),
                },
            )
            .unwrap();

        // A `below: 20` (of max 100) crossing watching the authoritative slot.
        let mut data_registry = DataRegistry::new();
        data_registry.populate_level(
            Vec::new(),
            vec![CrossingDescriptor {
                slot: "hud.health".to_string(),
                condition: CrossingCondition::Below { threshold: 0.2 },
                max: 100.0,
                fire: vec!["lowHealth".to_string()],
            }],
            &[],
        );
        let mut detector = CrossingDetector::new();
        detector.initialize(&data_registry, &slot_table);

        // Authoritative health drops below 20 this frame.
        slot_table.get_mut("hud.health").unwrap().value = Some(SlotValue::Number(15.0));

        // Mid-tween the eased DISPLAY value still reads 50 — the bar is catching
        // up. styleRanges, fed the DISPLAY value, resolve the GREEN default band.
        let mut style_state = StyleEffectState::default();
        let display_color = evaluate(&ranges, 50.0, [1.0; 4], &theme(), &mut style_state, 0.0);
        assert!(
            close(display_color[0], 0.0) && close(display_color[1], 1.0),
            "display value (mid-tween) resolves the green band, got {display_color:?}"
        );

        // The detector, fed the AUTHORITATIVE slot, fires the below-20% crossing
        // this same frame — the two reads diverge.
        assert_eq!(
            detector.detect(&slot_table),
            vec!["lowHealth".to_string()],
            "the crossing fires off the authoritative slot while the HUD shows green"
        );
    }

    #[test]
    fn entry_omits_absent_optional_fields_on_the_wire() {
        // A bare default entry (no upTo, color, pulse, flash) serializes to an
        // empty object — every optional skip-serializes when absent.
        let ranges = StyleRanges {
            max: 50.0,
            entries: vec![entry(None, None)],
        };
        let json = serde_json::to_string(&ranges).expect("must serialize");
        assert_eq!(json, r#"{"max":50.0,"entries":[{}]}"#);
    }
}
