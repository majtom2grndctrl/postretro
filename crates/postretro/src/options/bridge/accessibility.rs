// Accessibility half of the options bridge: menu writes to the `options.*`
// working copies, the readonly `accessibility.*` projection, and engine-write
// reseeds of the working copies.
// See: context/lib/player_options.md §5

use postretro_entities::slot_table::{SlotTable, SlotValue};

use super::super::resolved::{OsPreferences, ResolvedAccessibility};
use super::super::{PlayerOptions, keys};

/// One accessibility field's slots and its store binding.
struct Field {
    /// The readonly resolved slot.
    resolved_slot: &'static str,
    /// The writable working copy; `None` for the flash limiter and for derived
    /// source flags, which no script writes.
    working_copy: Option<&'static str>,
    /// `settings.toml` key a menu write marks.
    key: &'static str,
    resolved: fn(&ResolvedAccessibility) -> SlotValue,
    /// Apply a menu write to the store. `None` when the value has the wrong
    /// type; otherwise whether the stored field changed.
    apply: fn(&mut PlayerOptions, &SlotValue) -> Option<bool>,
}

fn apply_number(field: &mut f32, value: &SlotValue) -> Option<bool> {
    let SlotValue::Number(value) = value else {
        return None;
    };
    let changed = *field != *value;
    *field = *value;
    Some(changed)
}

fn apply_bool(field: &mut bool, value: &SlotValue) -> Option<bool> {
    let SlotValue::Boolean(value) = value else {
        return None;
    };
    let changed = *field != *value;
    *field = *value;
    Some(changed)
}

fn no_working_copy(_: &mut PlayerOptions, _: &SlotValue) -> Option<bool> {
    None
}

const FIELDS: &[Field] = &[
    Field {
        resolved_slot: "accessibility.reduceMotion",
        working_copy: Some("options.reduceMotion"),
        key: keys::REDUCE_MOTION,
        resolved: |r| SlotValue::Boolean(r.reduce_motion),
        // A menu write marks the field player-set even when it writes the
        // value the field already resolves to.
        apply: |o, v| {
            let SlotValue::Boolean(value) = v else {
                return None;
            };
            let changed = o.accessibility.reduce_motion != Some(*value);
            o.accessibility.reduce_motion = Some(*value);
            Some(changed)
        },
    },
    Field {
        resolved_slot: "accessibility.reduceMotionFollowsSystem",
        working_copy: None,
        key: keys::REDUCE_MOTION,
        resolved: |r| SlotValue::Boolean(r.reduce_motion_follows_system),
        apply: no_working_copy,
    },
    Field {
        resolved_slot: "accessibility.screenShakeScale",
        working_copy: Some("options.screenShakeScale"),
        key: keys::SCREEN_SHAKE_SCALE,
        resolved: |r| SlotValue::Number(r.screen_shake_scale),
        apply: |o, v| apply_number(&mut o.accessibility.screen_shake_scale, v),
    },
    Field {
        resolved_slot: "accessibility.viewFeelScale",
        working_copy: Some(super::VIEW_FEEL_SCALE_SLOT),
        key: keys::VIEW_FEEL_SCALE,
        resolved: |r| SlotValue::Number(r.view_feel_scale),
        apply: |o, v| apply_number(&mut o.view_feel_scale, v),
    },
    Field {
        resolved_slot: "accessibility.flashLimiter",
        working_copy: None,
        key: keys::FLASH_LIMITER,
        resolved: |r| SlotValue::Boolean(r.flash_limiter),
        apply: no_working_copy,
    },
    Field {
        resolved_slot: "accessibility.masterVolume",
        working_copy: Some("options.masterVolume"),
        key: keys::MASTER_VOLUME,
        resolved: |r| SlotValue::Number(r.master_volume),
        apply: |o, v| apply_number(&mut o.accessibility.master_volume, v),
    },
    Field {
        resolved_slot: "accessibility.sfxVolume",
        working_copy: Some("options.sfxVolume"),
        key: keys::SFX_VOLUME,
        resolved: |r| SlotValue::Number(r.sfx_volume),
        apply: |o, v| apply_number(&mut o.accessibility.sfx_volume, v),
    },
    Field {
        resolved_slot: "accessibility.musicVolume",
        working_copy: Some("options.musicVolume"),
        key: keys::MUSIC_VOLUME,
        resolved: |r| SlotValue::Number(r.music_volume),
        apply: |o, v| apply_number(&mut o.accessibility.music_volume, v),
    },
    Field {
        resolved_slot: "accessibility.uiVolume",
        working_copy: Some("options.uiVolume"),
        key: keys::UI_VOLUME,
        resolved: |r| SlotValue::Number(r.ui_volume),
        apply: |o, v| apply_number(&mut o.accessibility.ui_volume, v),
    },
    Field {
        resolved_slot: "accessibility.holdTimingScale",
        working_copy: Some("options.holdTimingScale"),
        key: keys::HOLD_TIMING_SCALE,
        resolved: |r| SlotValue::Number(r.hold_timing_scale),
        apply: |o, v| apply_number(&mut o.accessibility.hold_timing_scale, v),
    },
    Field {
        resolved_slot: "accessibility.monoAudio",
        working_copy: Some("options.monoAudio"),
        key: keys::MONO_AUDIO,
        resolved: |r| SlotValue::Boolean(r.mono_audio),
        apply: |o, v| apply_bool(&mut o.accessibility.mono_audio, v),
    },
];

const FIELD_COUNT: usize = FIELDS.len();

/// Per-field observed working-copy generations plus the last projected
/// resolution. All frame work is compare-then-write, so a settled frame writes
/// no slot and allocates nothing.
#[derive(Default)]
pub(super) struct AccessibilitySync {
    observed: [u64; FIELD_COUNT],
    last_resolved: Option<ResolvedAccessibility>,
}

impl AccessibilitySync {
    pub(super) fn resolved(&self) -> Option<ResolvedAccessibility> {
        self.last_resolved
    }

    /// Apply menu writes observed since the last frame. Returns whether a
    /// stored field changed.
    pub(super) fn observe(&mut self, table: &SlotTable, options: &mut PlayerOptions) -> bool {
        let mut changed = false;
        for (index, field) in FIELDS.iter().enumerate() {
            let Some(copy) = field.working_copy else {
                continue;
            };
            let slot = table
                .get(copy)
                .expect("accessibility working copy must exist in the engine-state catalog");
            let generation = slot.write_generation();
            if generation == self.observed[index] {
                continue;
            }
            self.observed[index] = generation;
            let Some(value) = slot.value.as_ref() else {
                continue;
            };
            if let Some(field_changed) = (field.apply)(options, value) {
                options.mark_written(field.key);
                changed |= field_changed;
            }
        }
        changed
    }

    /// Project the resolved values into `accessibility.*` and reseed any
    /// working copy an engine write moved. A working copy holding a write the
    /// bridge has not yet observed is left for `observe`, so a reseed never
    /// discards a player's write. Returns the resolution when it changed.
    pub(super) fn sync(
        &mut self,
        table: &mut SlotTable,
        options: &PlayerOptions,
        os: &OsPreferences,
    ) -> Option<ResolvedAccessibility> {
        let resolved = ResolvedAccessibility::resolve(options, os);
        if self.last_resolved == Some(resolved) {
            return None;
        }
        for (index, field) in FIELDS.iter().enumerate() {
            let value = (field.resolved)(&resolved);
            write_if_changed(table, field.resolved_slot, &value);
            if let Some(copy) = field.working_copy {
                let slot = table
                    .get_mut(copy)
                    .expect("accessibility working copy must exist in the engine-state catalog");
                if slot.write_generation() == self.observed[index]
                    && slot.value.as_ref() != Some(&value)
                {
                    slot.write_value(Some(value));
                    self.observed[index] = slot.write_generation();
                }
            }
        }
        self.last_resolved = Some(resolved);
        Some(resolved)
    }

    /// Seed every working copy and projection from the store, as a menu open
    /// and session build do. Seeds advance the observed generations, so they
    /// never read back as menu writes.
    pub(super) fn seed_all(
        &mut self,
        table: &mut SlotTable,
        options: &PlayerOptions,
        os: &OsPreferences,
    ) -> ResolvedAccessibility {
        let resolved = ResolvedAccessibility::resolve(options, os);
        for (index, field) in FIELDS.iter().enumerate() {
            let value = (field.resolved)(&resolved);
            write_if_changed(table, field.resolved_slot, &value);
            if let Some(copy) = field.working_copy {
                self.observed[index] = super::seed_slot(table, copy, value);
            }
        }
        self.last_resolved = Some(resolved);
        resolved
    }
}

fn write_if_changed(table: &mut SlotTable, name: &str, value: &SlotValue) {
    let slot = table
        .get_mut(name)
        .expect("accessibility slot must exist in the engine-state catalog");
    if slot.value.as_ref() != Some(value) {
        slot.write_value(Some(value.clone()));
    }
}
