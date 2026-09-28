// The player's accessibility preferences: the `[accessibility]` table of
// `settings.toml`. Presentation-only and client-local; simulation never reads it.
// See: context/lib/player_options.md §5

use super::document::{DocumentWriter, FieldReader};

/// TOML table holding the group. `view_feel_scale` belongs to the group but
/// keeps its top-level key.
pub(crate) const GROUP: &str = "accessibility";

/// Dotted field keys, used to mark a player write so a save replaces a stored
/// value this build could not read.
pub(crate) mod keys {
    pub(crate) const REDUCE_MOTION: &str = "accessibility.reduce_motion";
    pub(crate) const SCREEN_SHAKE_SCALE: &str = "accessibility.screen_shake_scale";
    pub(crate) const FLASH_LIMITER: &str = "accessibility.flash_limiter";
    pub(crate) const MASTER_VOLUME: &str = "accessibility.master_volume";
    pub(crate) const SFX_VOLUME: &str = "accessibility.sfx_volume";
    pub(crate) const MUSIC_VOLUME: &str = "accessibility.music_volume";
    pub(crate) const UI_VOLUME: &str = "accessibility.ui_volume";
    pub(crate) const MONO_AUDIO: &str = "accessibility.mono_audio";
}

const DEFAULT_SCALE: f32 = 1.0;
const DEFAULT_VOLUME: f32 = 1.0;

/// Stored accessibility preferences.
///
/// An OS-seedable field is `None` while the player has never set it: it then
/// follows the OS value, else the engine default, and saving writes no key for
/// it. Resolution lives with the OS reader, not here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AccessibilityOptions {
    /// OS-seedable. Engine default off.
    pub reduce_motion: Option<bool>,
    /// Screen-shake presentation scale, `[0, 1]`.
    pub screen_shake_scale: f32,
    /// Photosensitivity flash limiter. On by default; only the engine panel
    /// changes it.
    pub flash_limiter: bool,
    /// Linear bus volumes, `[0, 1]`; mapped to decibels at the audio seam.
    pub master_volume: f32,
    pub sfx_volume: f32,
    pub music_volume: f32,
    pub ui_volume: f32,
    /// Fold left and right together after spatialization.
    pub mono_audio: bool,
}

impl Default for AccessibilityOptions {
    fn default() -> Self {
        Self {
            reduce_motion: None,
            screen_shake_scale: DEFAULT_SCALE,
            flash_limiter: true,
            master_volume: DEFAULT_VOLUME,
            sfx_volume: DEFAULT_VOLUME,
            music_volume: DEFAULT_VOLUME,
            ui_volume: DEFAULT_VOLUME,
            mono_audio: false,
        }
    }
}

impl AccessibilityOptions {
    pub(super) fn read(reader: &mut FieldReader<'_>) -> Self {
        reader.check_group(GROUP);
        let defaults = Self::default();
        let mut field = |key: &str| reader.read_in::<f32>(GROUP, key);
        let screen_shake_scale = field("screen_shake_scale").unwrap_or(defaults.screen_shake_scale);
        let master_volume = field("master_volume").unwrap_or(defaults.master_volume);
        let sfx_volume = field("sfx_volume").unwrap_or(defaults.sfx_volume);
        let music_volume = field("music_volume").unwrap_or(defaults.music_volume);
        let ui_volume = field("ui_volume").unwrap_or(defaults.ui_volume);
        Self {
            reduce_motion: reader.read_in(GROUP, "reduce_motion"),
            screen_shake_scale,
            flash_limiter: reader
                .read_in(GROUP, "flash_limiter")
                .unwrap_or(defaults.flash_limiter),
            master_volume,
            sfx_volume,
            music_volume,
            ui_volume,
            mono_audio: reader
                .read_in(GROUP, "mono_audio")
                .unwrap_or(defaults.mono_audio),
        }
    }

    pub(super) fn write(&self, writer: &mut DocumentWriter<'_>) {
        writer.put_in(GROUP, "reduce_motion", self.reduce_motion.as_ref());
        writer.put_in(GROUP, "screen_shake_scale", Some(&self.screen_shake_scale));
        writer.put_in(GROUP, "flash_limiter", Some(&self.flash_limiter));
        writer.put_in(GROUP, "master_volume", Some(&self.master_volume));
        writer.put_in(GROUP, "sfx_volume", Some(&self.sfx_volume));
        writer.put_in(GROUP, "music_volume", Some(&self.music_volume));
        writer.put_in(GROUP, "ui_volume", Some(&self.ui_volume));
        writer.put_in(GROUP, "mono_audio", Some(&self.mono_audio));
    }

    /// Clamp scales and volumes into `[0, 1]`; a non-finite value takes its
    /// default.
    pub(super) fn sanitize(&mut self) {
        for (value, default) in [
            (&mut self.screen_shake_scale, DEFAULT_SCALE),
            (&mut self.master_volume, DEFAULT_VOLUME),
            (&mut self.sfx_volume, DEFAULT_VOLUME),
            (&mut self.music_volume, DEFAULT_VOLUME),
            (&mut self.ui_volume, DEFAULT_VOLUME),
        ] {
            *value = if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                default
            };
        }
    }
}
