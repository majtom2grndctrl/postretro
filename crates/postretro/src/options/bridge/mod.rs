// App-side bridge from writable option slots to session-owned player settings.
// See: context/lib/player_options.md §4

mod accessibility;
#[cfg(test)]
mod accessibility_tests;
mod save_schedule;
mod top_level;

use std::io;
use std::path::Path;

use postretro_entities::slot_table::{SlotTable, SlotValue};

use super::resolved::{OsPreferences, ResolvedAccessibility};
use super::{FogQuality, PlayerOptions, RenderResolution, SurfaceDepthQuality};
use crate::input::InputSystem;
use accessibility::AccessibilitySync;
use save_schedule::SaveSchedule;
use top_level::TopLevelSync;
pub(crate) use top_level::VIEW_FEEL_SCALE_SLOT;
#[cfg(test)]
use top_level::{
    CROUCH_MODE_SLOT, FOG_QUALITY_SLOT, INVERT_Y_SLOT, MOUSE_SENSITIVITY_SLOT,
    RENDER_RESOLUTION_SLOT, SHADOW_QUALITY_SLOT, SURFACE_DEPTH_QUALITY_SLOT,
};

/// Live subsystem effects produced by accepted option-slot changes.
///
/// Input effects are applied by the bridge before this report is returned.
/// Fog, Surface Depth and render resolution stay typed until the app-side
/// render-profile chokepoint translates them into renderer parameters. Shadow
/// intentionally has no live effect.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct OptionsApplyEffects {
    pub(crate) mouse_sensitivity: Option<f32>,
    pub(crate) invert_y: Option<bool>,
    pub(crate) fog_quality: Option<FogQuality>,
    pub(crate) surface_depth_quality: Option<SurfaceDepthQuality>,
    pub(crate) render_resolution: Option<RenderResolution>,
    pub(crate) window_mode: Option<super::WindowMode>,
    /// The resolved accessibility preferences, when they changed this frame.
    pub(crate) accessibility: Option<ResolvedAccessibility>,
}

/// Deterministic, session-lifetime synchronization state for `options.*`.
#[derive(Default)]
pub(crate) struct OptionsBridge {
    top_level: TopLevelSync,
    accessibility: AccessibilitySync,
    /// Latest OS accessibility readings; unset fields follow them.
    os: OsPreferences,
    save: SaveSchedule,
}

impl OptionsBridge {
    pub(crate) fn reseed_window_mode(&mut self, table: &mut SlotTable, mode: super::WindowMode) {
        self.top_level.reseed_window_mode(table, mode);
    }

    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Seed the complete UI working copy from the current in-memory settings.
    /// The observed generations advance with these engine writes so reopening a
    /// menu never feeds the seed back through apply/save as a user change.
    pub(crate) fn seed_on_open(&mut self, table: &mut SlotTable, options: &PlayerOptions) {
        self.top_level.seed(table, options);
        self.accessibility.seed_all(table, options, &self.os);
    }

    /// Seed every accessibility working copy and `accessibility.*` slot once at
    /// session build, so a mod menu under any tree name shows resolved values.
    pub(crate) fn seed_accessibility(
        &mut self,
        table: &mut SlotTable,
        options: &PlayerOptions,
    ) -> ResolvedAccessibility {
        self.accessibility.seed_all(table, options, &self.os)
    }

    /// The current resolution, as last projected into `accessibility.*`.
    /// `None` only before the session-build seed.
    pub(crate) fn resolved(&self) -> Option<ResolvedAccessibility> {
        self.accessibility.resolved()
    }

    /// Record the OS's latest accessibility readings. Unset fields follow them
    /// from the next `update`, which reseeds their working copies.
    pub(crate) fn set_os_preferences(&mut self, os: OsPreferences) {
        self.os = os;
    }

    /// Schedule the settled save for an engine-side store write (a panel
    /// action), exactly as an accepted menu change does.
    pub(crate) fn schedule_save(&mut self, settings_path: Option<&Path>) {
        self.save.arm(settings_path);
    }

    /// Observe writes once per app frame, apply live input changes immediately,
    /// and settle disk persistence after 250 ms of no further accepted change.
    pub(crate) fn update(
        &mut self,
        frame_dt_seconds: f32,
        table: &mut SlotTable,
        options: &mut PlayerOptions,
        input: &mut InputSystem,
        settings_path: Option<&Path>,
    ) -> OptionsApplyEffects {
        self.update_with_save(
            frame_dt_seconds,
            table,
            options,
            input,
            settings_path,
            PlayerOptions::save,
        )
    }

    /// Flush a pending debounced write when the options modal closes.
    pub(crate) fn flush_on_options_close(
        &mut self,
        options: &PlayerOptions,
        settings_path: Option<&Path>,
    ) {
        self.flush_with_save(options, settings_path, PlayerOptions::save);
    }

    /// Flush a pending debounced write during normal application teardown.
    pub(crate) fn flush_on_clean_exit(
        &mut self,
        options: &PlayerOptions,
        settings_path: Option<&Path>,
    ) {
        self.flush_with_save(options, settings_path, PlayerOptions::save);
    }

    fn update_with_save<F>(
        &mut self,
        frame_dt_seconds: f32,
        table: &mut SlotTable,
        options: &mut PlayerOptions,
        input: &mut InputSystem,
        settings_path: Option<&Path>,
        mut save: F,
    ) -> OptionsApplyEffects
    where
        F: FnMut(&PlayerOptions, &Path) -> io::Result<()>,
    {
        let mut effects = OptionsApplyEffects::default();
        let mut changed = self.top_level.observe(table, options, input, &mut effects);
        changed |= self.accessibility.observe(table, options);
        // Projection and engine-write reseeds run every frame, but write only
        // what changed.
        effects.accessibility = self.accessibility.sync(table, options, &self.os);

        if changed {
            self.save.arm(settings_path);
        } else {
            self.save
                .tick(frame_dt_seconds, options, settings_path, &mut save);
        }

        effects
    }

    fn flush_with_save<F>(
        &mut self,
        options: &PlayerOptions,
        settings_path: Option<&Path>,
        mut save: F,
    ) where
        F: FnMut(&PlayerOptions, &Path) -> io::Result<()>,
    {
        self.save.flush(options, settings_path, &mut save);
    }
}

pub(super) fn seed_slot(table: &mut SlotTable, name: &str, value: SlotValue) -> u64 {
    let slot = table
        .get_mut(name)
        .expect("built-in options slot must exist in the engine-state catalog");
    slot.write_value(Some(value));
    slot.write_generation()
}

pub(super) fn changed_value<'a>(
    table: &'a SlotTable,
    name: &str,
    observed_generation: &mut u64,
) -> Option<(u64, &'a SlotValue)> {
    let slot = table
        .get(name)
        .expect("built-in options slot must exist in the engine-state catalog");
    let generation = slot.write_generation();
    if generation == *observed_generation {
        return None;
    }
    Some((
        generation,
        slot.value.as_ref().expect("options slots carry defaults"),
    ))
}

#[cfg(test)]
mod tests;
