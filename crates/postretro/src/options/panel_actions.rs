// The engine accessibility panel's field actions: the closed vocabulary behind
// `ui.accessibility.<op>.<field>` and how each op writes the store.
// See: context/lib/ui.md §4.1 · context/lib/player_options.md §5

use super::{PlayerOptions, keys};

/// The field whose action only the engine panel may fire.
pub(crate) const FLASH_LIMITER_FIELD: &str = "flashLimiter";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanelOp {
    Cycle,
    Increase,
    Decrease,
}

impl PanelOp {
    pub(crate) fn parse(op: &str) -> Option<Self> {
        match op {
            "cycle" => Some(Self::Cycle),
            "increase" => Some(Self::Increase),
            "decrease" => Some(Self::Decrease),
            _ => None,
        }
    }
}

/// What a field action did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanelActionOutcome {
    /// The field was written and marked player-set; `changed` is false when it
    /// already held the result (a step clamped at its range end).
    Written {
        changed: bool,
    },
    UnknownField,
    UnknownOp,
    /// `cycle` on a numeric field, or `increase`/`decrease` on a toggle.
    MismatchedOp,
}

enum Kind {
    /// OS-seedable toggle: System → On → Off → System.
    SystemToggle(fn(&mut PlayerOptions) -> &mut Option<bool>),
    Toggle(fn(&mut PlayerOptions) -> &mut bool),
    /// `[0, 1]` in fixed steps.
    Numeric {
        step: f32,
        value: fn(&mut PlayerOptions) -> &mut f32,
    },
}

struct Field {
    name: &'static str,
    key: &'static str,
    kind: Kind,
}

const FIELDS: &[Field] = &[
    Field {
        name: "reduceMotion",
        key: keys::REDUCE_MOTION,
        kind: Kind::SystemToggle(|o| &mut o.accessibility.reduce_motion),
    },
    Field {
        name: "screenShakeScale",
        key: keys::SCREEN_SHAKE_SCALE,
        kind: Kind::Numeric {
            step: 0.1,
            value: |o| &mut o.accessibility.screen_shake_scale,
        },
    },
    Field {
        name: "viewFeelScale",
        key: keys::VIEW_FEEL_SCALE,
        kind: Kind::Numeric {
            step: 0.1,
            value: |o| &mut o.view_feel_scale,
        },
    },
    Field {
        name: FLASH_LIMITER_FIELD,
        key: keys::FLASH_LIMITER,
        kind: Kind::Toggle(|o| &mut o.accessibility.flash_limiter),
    },
    Field {
        name: "masterVolume",
        key: keys::MASTER_VOLUME,
        kind: Kind::Numeric {
            step: 0.05,
            value: |o| &mut o.accessibility.master_volume,
        },
    },
    Field {
        name: "sfxVolume",
        key: keys::SFX_VOLUME,
        kind: Kind::Numeric {
            step: 0.05,
            value: |o| &mut o.accessibility.sfx_volume,
        },
    },
    Field {
        name: "musicVolume",
        key: keys::MUSIC_VOLUME,
        kind: Kind::Numeric {
            step: 0.05,
            value: |o| &mut o.accessibility.music_volume,
        },
    },
    Field {
        name: "uiVolume",
        key: keys::UI_VOLUME,
        kind: Kind::Numeric {
            step: 0.05,
            value: |o| &mut o.accessibility.ui_volume,
        },
    },
    Field {
        name: "monoAudio",
        key: keys::MONO_AUDIO,
        kind: Kind::Toggle(|o| &mut o.accessibility.mono_audio),
    },
];

/// Every field the panel carries, by slot suffix. The panel descriptor and the
/// SDK's typed `accessibilityAction` are checked against this list.
#[cfg(test)]
pub(crate) fn panel_field_names() -> impl Iterator<Item = &'static str> {
    FIELDS.iter().map(|field| field.name)
}

/// Whether `field` is numeric (steps) rather than a toggle (cycles).
pub(crate) fn is_numeric_field(field: &str) -> bool {
    FIELDS
        .iter()
        .any(|f| f.name == field && matches!(f.kind, Kind::Numeric { .. }))
}

/// Apply one field action to the store and mark the field player-written. The
/// caller schedules the settled save; the options bridge projects and reseeds
/// the slots on its next update, the same frame.
pub(crate) fn apply_panel_action(
    options: &mut PlayerOptions,
    field: &str,
    op: &str,
) -> PanelActionOutcome {
    let Some(entry) = FIELDS.iter().find(|f| f.name == field) else {
        return PanelActionOutcome::UnknownField;
    };
    let Some(op) = PanelOp::parse(op) else {
        return PanelActionOutcome::UnknownOp;
    };
    let changed = match (&entry.kind, op) {
        (Kind::SystemToggle(value), PanelOp::Cycle) => {
            let value = value(options);
            let next = match *value {
                None => Some(true),
                Some(true) => Some(false),
                Some(false) => None,
            };
            let changed = *value != next;
            *value = next;
            changed
        }
        (Kind::Toggle(value), PanelOp::Cycle) => {
            let value = value(options);
            *value = !*value;
            true
        }
        (Kind::Numeric { step, value }, PanelOp::Increase | PanelOp::Decrease) => {
            let value = value(options);
            let direction = if op == PanelOp::Increase { 1.0 } else { -1.0 };
            // Snap to the step grid so repeated steps never drift.
            let next = (((*value + direction * step) / step).round() * step).clamp(0.0, 1.0);
            let changed = *value != next;
            *value = next;
            changed
        }
        _ => return PanelActionOutcome::MismatchedOp,
    };
    options.mark_written(entry.key);
    PanelActionOutcome::Written { changed }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduce_motion_cycles_system_on_off_and_back_to_system() {
        let mut options = PlayerOptions::default();
        let mut seen = Vec::new();
        for _ in 0..4 {
            apply_panel_action(&mut options, "reduceMotion", "cycle");
            seen.push(options.accessibility.reduce_motion);
        }
        assert_eq!(seen, [Some(true), Some(false), None, Some(true)]);
    }

    #[test]
    fn numeric_fields_step_on_their_grid_and_clamp() {
        let mut options = PlayerOptions::default();
        for _ in 0..3 {
            apply_panel_action(&mut options, "sfxVolume", "decrease");
        }
        assert!((options.accessibility.sfx_volume - 0.85).abs() < 1e-6);
        assert_eq!(
            apply_panel_action(&mut options, "masterVolume", "increase"),
            PanelActionOutcome::Written { changed: false },
            "already at the top of its range"
        );
        for _ in 0..12 {
            apply_panel_action(&mut options, "screenShakeScale", "decrease");
        }
        assert_eq!(options.accessibility.screen_shake_scale, 0.0);
    }

    #[test]
    fn a_mismatched_or_unknown_action_writes_nothing() {
        let mut options = PlayerOptions::default();
        let before = options.clone();
        assert_eq!(
            apply_panel_action(&mut options, "monoAudio", "increase"),
            PanelActionOutcome::MismatchedOp
        );
        assert_eq!(
            apply_panel_action(&mut options, "sfxVolume", "cycle"),
            PanelActionOutcome::MismatchedOp
        );
        assert_eq!(
            apply_panel_action(&mut options, "fov", "cycle"),
            PanelActionOutcome::UnknownField
        );
        assert_eq!(
            apply_panel_action(&mut options, "monoAudio", "toggle"),
            PanelActionOutcome::UnknownOp
        );
        assert_eq!(options, before);
    }

    /// Hub AC 10: the panel carries every field in the accessibility group,
    /// derived from the engine's field table so a new field is covered.
    #[test]
    fn the_panel_descriptor_carries_every_accessibility_field() {
        let panel = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../core/ui/accessibilityPanel.json"
        ))
        .expect("panel descriptor");
        for field in panel_field_names() {
            let action = if is_numeric_field(field) {
                format!("\"slot\": \"accessibility.{field}\"")
            } else {
                format!("\"ui.accessibility.cycle.{field}\"")
            };
            assert!(
                panel.contains(&action),
                "panel lacks a control for `{field}`"
            );
        }
    }
}
