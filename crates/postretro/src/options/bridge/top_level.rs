// The top-level option working copies: seed on menu open, observe menu
// writes, apply live input effects. Accessibility fields sync in `accessibility.rs`.
// See: context/lib/player_options.md §4

use postretro_entities::slot_table::{SlotTable, SlotValue};

use super::super::{
    CrouchMode, FogQuality, PlayerOptions, RenderResolution, ShadowQuality, SurfaceDepthQuality,
    keys,
};
use super::{OptionsApplyEffects, changed_value, seed_slot};
use crate::input::InputSystem;

pub(crate) const MOUSE_SENSITIVITY_SLOT: &str = "options.mouseSensitivity";
pub(crate) const INVERT_Y_SLOT: &str = "options.invertY";
pub(crate) const VIEW_FEEL_SCALE_SLOT: &str = "options.viewFeelScale";
pub(crate) const CROUCH_MODE_SLOT: &str = "options.crouchMode";
pub(crate) const SHADOW_QUALITY_SLOT: &str = "options.shadowQuality";
pub(crate) const FOG_QUALITY_SLOT: &str = "options.fogQuality";
pub(crate) const SURFACE_DEPTH_QUALITY_SLOT: &str = "options.surfaceDepthQuality";
pub(crate) const RENDER_RESOLUTION_SLOT: &str = "options.renderResolution";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ObservedGenerations {
    mouse_sensitivity: u64,
    invert_y: u64,
    crouch_mode: u64,
    shadow_quality: u64,
    fog_quality: u64,
    surface_depth_quality: u64,
    render_resolution: u64,
    window_mode: u64,
}

/// Change tracking for the top-level `options.*` working copies.
#[derive(Default)]
pub(super) struct TopLevelSync {
    observed: ObservedGenerations,
}

impl TopLevelSync {
    pub(super) fn reseed_window_mode(
        &mut self,
        table: &mut SlotTable,
        mode: super::super::WindowMode,
    ) {
        self.observed.window_mode = seed_slot(
            table,
            "options.windowMode",
            SlotValue::Enum(mode.slot_value().into()),
        );
    }

    /// Seed every top-level working copy from the in-memory settings.
    pub(super) fn seed(&mut self, table: &mut SlotTable, options: &PlayerOptions) {
        self.observed.mouse_sensitivity = seed_slot(
            table,
            MOUSE_SENSITIVITY_SLOT,
            SlotValue::Number(options.mouse_sensitivity),
        );
        self.observed.invert_y =
            seed_slot(table, INVERT_Y_SLOT, SlotValue::Boolean(options.invert_y));
        self.observed.crouch_mode = seed_slot(
            table,
            CROUCH_MODE_SLOT,
            SlotValue::Enum(options.crouch_mode.slot_value().to_string()),
        );
        self.observed.shadow_quality = seed_slot(
            table,
            SHADOW_QUALITY_SLOT,
            SlotValue::Enum(options.shadow_quality.slot_value().to_string()),
        );
        self.observed.fog_quality = seed_slot(
            table,
            FOG_QUALITY_SLOT,
            SlotValue::Enum(options.fog_quality.slot_value().to_string()),
        );
        self.observed.surface_depth_quality = seed_slot(
            table,
            SURFACE_DEPTH_QUALITY_SLOT,
            SlotValue::Enum(options.surface_depth_quality.slot_value().to_string()),
        );
        self.observed.render_resolution = seed_slot(
            table,
            RENDER_RESOLUTION_SLOT,
            SlotValue::Enum(options.render_resolution.slot_value().to_string()),
        );
        self.reseed_window_mode(table, options.window_mode);
    }

    pub(super) fn observe(
        &mut self,
        table: &SlotTable,
        options: &mut PlayerOptions,
        input: &mut InputSystem,
        effects: &mut OptionsApplyEffects,
    ) -> bool {
        let mut changed = false;
        if let Some((generation, SlotValue::Enum(value))) =
            changed_value(table, "options.windowMode", &mut self.observed.window_mode)
        {
            effects.window_mode = super::super::WindowMode::from_slot_value(value);
            self.observed.window_mode = generation;
        }

        if let Some((generation, SlotValue::Number(value))) = changed_value(
            table,
            MOUSE_SENSITIVITY_SLOT,
            &mut self.observed.mouse_sensitivity,
        ) {
            options.mark_written(keys::MOUSE_SENSITIVITY);
            if options.mouse_sensitivity != *value {
                options.mouse_sensitivity = *value;
                input.set_mouse_sensitivity(*value);
                effects.mouse_sensitivity = Some(*value);
                changed = true;
            }
            self.observed.mouse_sensitivity = generation;
        }

        if let Some((generation, SlotValue::Boolean(value))) =
            changed_value(table, INVERT_Y_SLOT, &mut self.observed.invert_y)
        {
            options.mark_written(keys::INVERT_Y);
            if options.invert_y != *value {
                options.invert_y = *value;
                input.set_invert_y(*value);
                effects.invert_y = Some(*value);
                changed = true;
            }
            self.observed.invert_y = generation;
        }

        if let Some((generation, SlotValue::Enum(value))) =
            changed_value(table, CROUCH_MODE_SLOT, &mut self.observed.crouch_mode)
        {
            if let Some(mode) = CrouchMode::from_slot_value(value) {
                options.mark_written(keys::CROUCH_MODE);
                if options.crouch_mode != mode {
                    options.crouch_mode = mode;
                    changed = true;
                }
            }
            self.observed.crouch_mode = generation;
        }

        if let Some((generation, SlotValue::Enum(value))) = changed_value(
            table,
            SHADOW_QUALITY_SLOT,
            &mut self.observed.shadow_quality,
        ) {
            if let Some(quality) = ShadowQuality::from_slot_value(value) {
                options.mark_written(keys::SHADOW_QUALITY);
                if options.shadow_quality != quality {
                    options.shadow_quality = quality;
                    changed = true;
                }
            }
            self.observed.shadow_quality = generation;
        }

        if let Some((generation, SlotValue::Enum(value))) =
            changed_value(table, FOG_QUALITY_SLOT, &mut self.observed.fog_quality)
        {
            if let Some(quality) = FogQuality::from_slot_value(value) {
                options.mark_written(keys::FOG_QUALITY);
                if options.fog_quality != quality {
                    options.fog_quality = quality;
                    effects.fog_quality = Some(quality);
                    changed = true;
                }
            }
            self.observed.fog_quality = generation;
        }

        if let Some((generation, SlotValue::Enum(value))) = changed_value(
            table,
            SURFACE_DEPTH_QUALITY_SLOT,
            &mut self.observed.surface_depth_quality,
        ) {
            if let Some(quality) = SurfaceDepthQuality::from_slot_value(value) {
                options.mark_written(keys::SURFACE_DEPTH_QUALITY);
                if options.surface_depth_quality != quality {
                    options.surface_depth_quality = quality;
                    effects.surface_depth_quality = Some(quality);
                    changed = true;
                }
            }
            self.observed.surface_depth_quality = generation;
        }

        if let Some((generation, SlotValue::Enum(value))) = changed_value(
            table,
            RENDER_RESOLUTION_SLOT,
            &mut self.observed.render_resolution,
        ) {
            if let Some(resolution) = RenderResolution::from_slot_value(value) {
                options.mark_written(keys::RENDER_RESOLUTION);
                if options.render_resolution != resolution {
                    options.render_resolution = resolution;
                    effects.render_resolution = Some(resolution);
                    changed = true;
                }
            }
            self.observed.render_resolution = generation;
        }

        changed
    }
}
