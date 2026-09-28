// Accessibility resolution: player-set > OS value > engine default.
// See: context/lib/player_options.md §5

use super::PlayerOptions;

/// Engine default for reduce motion when neither the player nor the OS has a
/// preference.
const DEFAULT_REDUCE_MOTION: bool = false;

/// The OS accessibility readings the engine follows. `None` means the OS
/// reported no preference, or has not replied yet.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct OsPreferences {
    pub(crate) reduce_motion: Option<bool>,
}

/// Every accessibility field at its resolved value: what `accessibility.*`
/// slots carry and what presentation applies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResolvedAccessibility {
    pub(crate) reduce_motion: bool,
    /// True while reduce motion is unset and so follows the OS.
    pub(crate) reduce_motion_follows_system: bool,
    pub(crate) screen_shake_scale: f32,
    pub(crate) view_feel_scale: f32,
    pub(crate) flash_limiter: bool,
    pub(crate) master_volume: f32,
    pub(crate) sfx_volume: f32,
    pub(crate) music_volume: f32,
    pub(crate) ui_volume: f32,
    pub(crate) mono_audio: bool,
}

impl ResolvedAccessibility {
    /// The view-feel scale presentation applies: zero while reduce motion is
    /// on, the player's slider otherwise. The slider value itself is untouched,
    /// so it returns when the switch turns off.
    pub(crate) fn presented_view_feel_scale(&self) -> f32 {
        if self.reduce_motion {
            0.0
        } else {
            self.view_feel_scale
        }
    }

    pub(crate) fn resolve(options: &PlayerOptions, os: &OsPreferences) -> Self {
        let a = &options.accessibility;
        Self {
            reduce_motion: a
                .reduce_motion
                .or(os.reduce_motion)
                .unwrap_or(DEFAULT_REDUCE_MOTION),
            reduce_motion_follows_system: a.reduce_motion.is_none(),
            screen_shake_scale: a.screen_shake_scale,
            view_feel_scale: options.view_feel_scale,
            flash_limiter: a.flash_limiter,
            master_volume: a.master_volume,
            sfx_volume: a.sfx_volume,
            music_volume: a.music_volume,
            ui_volume: a.ui_volume,
            mono_audio: a.mono_audio,
        }
    }
}

/// The resolved reduce-motion switch as projected into the frame's slot
/// snapshot. Presentation that reads the snapshot rather than the store uses
/// this.
pub(crate) fn reduce_motion_from_slots(
    slots: &std::collections::HashMap<String, postretro_entities::SlotValue>,
) -> bool {
    matches!(
        slots.get("accessibility.reduceMotion"),
        Some(postretro_entities::SlotValue::Boolean(true))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::AccessibilityOptions;

    fn with_reduce_motion(stored: Option<bool>) -> PlayerOptions {
        PlayerOptions {
            accessibility: AccessibilityOptions {
                reduce_motion: stored,
                ..AccessibilityOptions::default()
            },
            ..PlayerOptions::default()
        }
    }

    #[test]
    fn resolution_prefers_player_then_os_then_default() {
        let os_on = OsPreferences {
            reduce_motion: Some(true),
        };
        let unset = with_reduce_motion(None);
        let resolved = ResolvedAccessibility::resolve(&unset, &os_on);
        assert!(resolved.reduce_motion, "unset follows the OS");
        assert!(resolved.reduce_motion_follows_system);

        let player_off = with_reduce_motion(Some(false));
        let resolved = ResolvedAccessibility::resolve(&player_off, &os_on);
        assert!(
            !resolved.reduce_motion,
            "an OS value never overrides the player"
        );
        assert!(!resolved.reduce_motion_follows_system);

        let resolved = ResolvedAccessibility::resolve(&unset, &OsPreferences::default());
        assert_eq!(resolved.reduce_motion, DEFAULT_REDUCE_MOTION);
    }

    #[test]
    fn reduce_motion_presents_zero_view_feel_and_keeps_the_slider() {
        let mut options = with_reduce_motion(Some(true));
        options.view_feel_scale = 0.7;
        options.accessibility.screen_shake_scale = 0.4;
        let on = ResolvedAccessibility::resolve(&options, &OsPreferences::default());
        assert_eq!(on.presented_view_feel_scale(), 0.0);
        // Each per-effect slot still carries its slider's own value.
        assert!((on.view_feel_scale - 0.7).abs() < 1e-6);
        assert!((on.screen_shake_scale - 0.4).abs() < 1e-6);

        options.accessibility.reduce_motion = Some(false);
        let off = ResolvedAccessibility::resolve(&options, &OsPreferences::default());
        assert!((off.presented_view_feel_scale() - 0.7).abs() < 1e-6);
    }
}
