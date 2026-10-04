// Per-human runtime preferences and their persistence to a human-editable
// `settings.toml`. Pure data + filesystem — no wgpu, renderer, or UI coupling.
// See: context/lib/player_options.md

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::input::DEFAULT_MOUSE_SENSITIVITY;

mod accessibility;
pub(crate) mod boot;
mod bridge;
mod document;
mod graphics;
mod panel_actions;
mod resolved;
mod window;

pub use accessibility::AccessibilityOptions;
pub(crate) use bridge::OptionsBridge;
use document::{DocumentWriter, FieldReader, StoredDocument};
pub use graphics::{FogQuality, RenderResolution, ShadowQuality, SurfaceDepthQuality};
pub(crate) use panel_actions::{PanelActionOutcome, apply_panel_action, is_numeric_field};
pub(crate) use resolved::{OsPreferences, apply_to_audio, reduce_motion_from_slots};
pub(crate) use window::{DisplayMode, WindowMode};

/// Registered dev-mod options tree whose open/close boundaries seed and flush
/// the session-owned settings bridge.
pub(crate) const OPTIONS_MENU_TREE_NAME: &str = "frontend.options";

/// `settings.toml` field keys, dotted for fields inside a table. A bridge or
/// panel write marks its key so the next save replaces a stored value this
/// build could not read.
pub(crate) mod keys {
    pub(crate) use super::accessibility::keys::*;

    pub(crate) const PLAYER_ID: &str = "player_id";
    pub(crate) const MOUSE_SENSITIVITY: &str = "mouse_sensitivity";
    pub(crate) const INVERT_Y: &str = "invert_y";
    pub(crate) const VIEW_FEEL_SCALE: &str = "view_feel_scale";
    pub(crate) const CROUCH_MODE: &str = "crouch_mode";
    pub(crate) const SHADOW_QUALITY: &str = "shadow_quality";
    pub(crate) const FOG_QUALITY: &str = "fog_quality";
    pub(crate) const SURFACE_DEPTH_QUALITY: &str = "surface_depth_quality";
    pub(crate) const WINDOW_MODE: &str = "window_mode";
    pub(crate) const RENDER_RESOLUTION: &str = "render_resolution";
    pub(crate) const SWITCH_CYCLE_DWELL_MS: &str = "switch_cycle_dwell_ms";
    pub(crate) const SCROLL_NOTCH_PIXELS: &str = "scroll_notch_pixels";
    pub(crate) const ACCESSIBILITY_PANEL_SHOWN: &str = "accessibility_panel_shown";
}

/// Filename written into the platform config directory
/// (`startup::app_dirs::AppDirs::settings_path`).
pub(crate) const SETTINGS_FILENAME: &str = "settings.toml";
const DEFAULT_SCROLL_NOTCH_PIXELS: f32 = 120.0;
const MAX_SCROLL_NOTCH_PIXELS: f32 = 4_096.0;
const MAX_SWITCH_CYCLE_DWELL_MS: u32 = 60_000;

/// How the crouch action is interpreted by the input layer. Resolved upstream
/// of the movement intent: the movement intent only ever sees the single
/// resolved per-tick bit (`MovementInput::crouch_intent`), never this mode.
///
/// Wire format is snake_case (matching the rest of `PlayerOptions`): TOML values
/// are `"hold"` / `"toggle"`. Defaults to `Hold` — hold-to-crouch is the
/// boomer-shooter baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrouchMode {
    /// One press latches crouch on, a second press latches it off (edge-driven).
    Toggle,
    /// Crouch intent is active only while the button is held (level signal).
    #[default]
    Hold,
}

impl CrouchMode {
    fn slot_value(self) -> &'static str {
        match self {
            Self::Hold => "hold",
            Self::Toggle => "toggle",
        }
    }

    fn from_slot_value(value: &str) -> Option<Self> {
        match value {
            "hold" => Some(Self::Hold),
            "toggle" => Some(Self::Toggle),
            _ => None,
        }
    }
}

/// Per-human runtime preferences, persisted as TOML.
///
/// Wire format is deliberately snake_case: this is a human-editable config,
/// distinct from the project's camelCase script-facing surface. Each field
/// loads on its own: an absent key takes its default, and a value this build
/// cannot read falls back for that field alone, with a warning, and stays in
/// the file until the player writes that field.
#[derive(Debug, Clone)]
pub struct PlayerOptions {
    /// Device-local player identity asserted when connecting to a multiplayer
    /// host. `None` is the anonymous fallback when this installation cannot
    /// persist an identity.
    pub player_id: Option<[u8; 16]>,

    /// Radians per raw mouse unit. Must be finite and > 0; defaults to the
    /// input subsystem's `DEFAULT_MOUSE_SENSITIVITY`.
    pub mouse_sensitivity: f32,

    /// When true, the pitch (Y) mouse axis is negated.
    pub invert_y: bool,

    /// Accessibility scale for view feel (head bob, kick, sway). Clamped to
    /// `[0, 1]` on load rather than rejected, so a hand-edited out-of-range
    /// value degrades gracefully. Part of the accessibility group, but keeps
    /// its top-level key.
    pub view_feel_scale: f32,

    /// How the crouch action is interpreted by the input layer (hold vs toggle).
    /// Engine-internal config — no SDK type / scripting surface (see
    /// player_options.md §6).
    pub crouch_mode: CrouchMode,

    /// Spot-shadow map resolution tier, applied on renderer full-init or the
    /// next level install.
    pub shadow_quality: ShadowQuality,

    /// Volumetric-fog march density tier, applied live and on renderer full-init.
    pub fog_quality: FogQuality,

    /// Surface Depth (texel-space parallax) switch, applied live by rewriting
    /// the per-material uniform buffers, and re-applied on renderer full-init.
    pub surface_depth_quality: SurfaceDepthQuality,

    /// Scene render resolution, applied live and re-applied on renderer
    /// full-init before the scene targets are built.
    pub render_resolution: RenderResolution,

    pub(crate) window_mode: WindowMode,
    pub(crate) display_mode: Option<DisplayMode>,

    /// Optional local override for the mod's cycle-selection dwell. `None`
    /// preserves the mod policy; an explicit zero selects immediately.
    pub switch_cycle_dwell_ms: Option<u32>,

    /// Pixel distance treated as one scroll-wheel notch. This is a concrete
    /// per-device setting, not a policy override; 120 matches the OS-standard
    /// wheel quantum.
    pub scroll_notch_pixels: f32,

    /// The `[accessibility]` table.
    pub accessibility: AccessibilityOptions,

    /// Whether the player has closed the engine accessibility panel once. The
    /// first-launch hold shows the panel until this is written.
    pub accessibility_panel_shown: bool,

    /// The loaded document a save rewrites.
    stored: StoredDocument,
}

/// Preference equality: the loaded document is persistence state, not a
/// preference.
impl PartialEq for PlayerOptions {
    fn eq(&self, other: &Self) -> bool {
        self.player_id == other.player_id
            && self.mouse_sensitivity == other.mouse_sensitivity
            && self.invert_y == other.invert_y
            && self.view_feel_scale == other.view_feel_scale
            && self.crouch_mode == other.crouch_mode
            && self.shadow_quality == other.shadow_quality
            && self.fog_quality == other.fog_quality
            && self.surface_depth_quality == other.surface_depth_quality
            && self.render_resolution == other.render_resolution
            && self.window_mode == other.window_mode
            && self.display_mode == other.display_mode
            && self.switch_cycle_dwell_ms == other.switch_cycle_dwell_ms
            && self.scroll_notch_pixels == other.scroll_notch_pixels
            && self.accessibility == other.accessibility
            && self.accessibility_panel_shown == other.accessibility_panel_shown
    }
}

fn default_mouse_sensitivity() -> f32 {
    DEFAULT_MOUSE_SENSITIVITY
}

fn default_invert_y() -> bool {
    false
}

fn default_view_feel_scale() -> f32 {
    1.0
}

fn default_scroll_notch_pixels() -> f32 {
    DEFAULT_SCROLL_NOTCH_PIXELS
}

/// Warn that the stored value for `field` (dotted key) is not finite and is
/// about to fall back to its default. TOML's `nan`/`inf` float literals parse
/// cleanly, so this is the only place a hand-edited non-finite value surfaces.
fn warn_non_finite(field: &str, value: f32) {
    log::warn!("[Options] `{field}` is not a finite number ({value}); using its default");
}

impl Default for PlayerOptions {
    fn default() -> Self {
        Self {
            player_id: None,
            mouse_sensitivity: default_mouse_sensitivity(),
            invert_y: default_invert_y(),
            view_feel_scale: default_view_feel_scale(),
            crouch_mode: CrouchMode::default(),
            shadow_quality: ShadowQuality::default(),
            fog_quality: FogQuality::default(),
            surface_depth_quality: SurfaceDepthQuality::default(),
            render_resolution: RenderResolution::default(),
            window_mode: WindowMode::default(),
            display_mode: None,
            switch_cycle_dwell_ms: None,
            scroll_notch_pixels: default_scroll_notch_pixels(),
            accessibility: AccessibilityOptions::default(),
            accessibility_panel_shown: false,
            stored: StoredDocument::default(),
        }
    }
}

impl PlayerOptions {
    /// Clamp loaded values into their valid ranges. Applied after
    /// deserialization so hand-edited out-of-range values are corrected rather
    /// than rejected. Every f32 field falls back to its default when not
    /// finite: TOML accepts `nan`/`inf` as valid float literals, so a
    /// hand-edited value sails past deserialization, and `f32::clamp` (unlike
    /// a `<`/`>` fallback check) leaves NaN untouched rather than clamping it —
    /// an unclamped NaN then fails every downstream `PartialEq` comparison,
    /// which is what let a NaN `view_feel_scale` keep the options bridge from
    /// ever settling. `mouse_sensitivity` also falls back when non-positive (a
    /// zero/negative sensitivity would break look input).
    fn sanitize(&mut self) {
        if self.view_feel_scale.is_finite() {
            self.view_feel_scale = self.view_feel_scale.clamp(0.0, 1.0);
        } else {
            warn_non_finite(keys::VIEW_FEEL_SCALE, self.view_feel_scale);
            self.view_feel_scale = default_view_feel_scale();
        }

        if !self.mouse_sensitivity.is_finite() {
            warn_non_finite(keys::MOUSE_SENSITIVITY, self.mouse_sensitivity);
        }
        if !(self.mouse_sensitivity.is_finite() && self.mouse_sensitivity > 0.0) {
            self.mouse_sensitivity = default_mouse_sensitivity();
        }

        self.switch_cycle_dwell_ms = self
            .switch_cycle_dwell_ms
            .map(|dwell| dwell.min(MAX_SWITCH_CYCLE_DWELL_MS));

        if !self.scroll_notch_pixels.is_finite() {
            warn_non_finite(keys::SCROLL_NOTCH_PIXELS, self.scroll_notch_pixels);
        }
        if !self.scroll_notch_pixels.is_finite() || self.scroll_notch_pixels <= 0.0 {
            self.scroll_notch_pixels = default_scroll_notch_pixels();
        } else {
            self.scroll_notch_pixels = self.scroll_notch_pixels.min(MAX_SCROLL_NOTCH_PIXELS);
        }
        self.accessibility.sanitize();
    }

    /// Record that the player (or the engine on the player's behalf) wrote the
    /// field at dotted `key`, so the next save replaces a stored value this
    /// build could not read.
    pub(crate) fn mark_written(&mut self, key: &str) {
        self.stored.clear_unrecognized(key);
    }

    /// False when the settings file exists but could not be read or parsed.
    /// Nothing replaces such a file.
    pub(crate) fn can_persist(&self) -> bool {
        !self.stored.is_read_only()
    }

    /// Load options from `path`.
    ///
    /// - File not found: returns defaults silently (normal first-run path).
    /// - Other read error (permission denied, I/O): logs a `[Options]` warning
    ///   and returns defaults; the file is never replaced.
    /// - Not valid TOML: logs a warning and returns defaults *without* touching
    ///   the file, so a malformed hand edit is preserved for the human to fix.
    /// - Otherwise each field loads on its own: an absent key takes its
    ///   default, and an unreadable value warns and falls back for that field
    ///   alone. Values are then sanitized.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn load(path: &Path) -> Self {
        Self::load_with_status(path).0
    }

    /// Load options and report whether it is safe for boot to replace the file.
    /// A malformed or unreadable file stays untouched, preserving the public
    /// `load` degradation contract while letting Session keep its identity
    /// generation anonymous in that case.
    pub(crate) fn load_with_status(path: &Path) -> (Self, PlayerOptionsLoadStatus) {
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return (Self::default(), PlayerOptionsLoadStatus::Missing);
            }
            Err(err) => {
                // Unexpected read failure (permission denied, I/O error, etc.).
                log::warn!(
                    "[Options] failed to read {}: {err}; using defaults (file will not be replaced)",
                    path.display()
                );
                return (Self::unavailable(), PlayerOptionsLoadStatus::Unavailable);
            }
        };

        match contents.parse::<toml::Table>() {
            Ok(table) => {
                let mut options = Self::from_table(table, path);
                options.sanitize();
                (options, PlayerOptionsLoadStatus::Loaded)
            }
            Err(err) => {
                log::warn!(
                    "[Options] failed to parse {}: {err}; using defaults (file left unmodified)",
                    path.display()
                );
                (Self::unavailable(), PlayerOptionsLoadStatus::Unavailable)
            }
        }
    }

    fn unavailable() -> Self {
        Self {
            stored: StoredDocument::read_only(),
            ..Self::default()
        }
    }

    fn from_table(table: toml::Table, path: &Path) -> Self {
        let mut stored = StoredDocument::from_table(table);
        let mut reader = FieldReader::new(&mut stored, path);
        let defaults = Self::default();
        let options = Self {
            player_id: reader.read(keys::PLAYER_ID),
            mouse_sensitivity: reader
                .read(keys::MOUSE_SENSITIVITY)
                .unwrap_or(defaults.mouse_sensitivity),
            invert_y: reader.read(keys::INVERT_Y).unwrap_or(defaults.invert_y),
            view_feel_scale: reader
                .read(keys::VIEW_FEEL_SCALE)
                .unwrap_or(defaults.view_feel_scale),
            crouch_mode: reader
                .read(keys::CROUCH_MODE)
                .unwrap_or(defaults.crouch_mode),
            shadow_quality: reader
                .read(keys::SHADOW_QUALITY)
                .unwrap_or(defaults.shadow_quality),
            fog_quality: reader
                .read(keys::FOG_QUALITY)
                .unwrap_or(defaults.fog_quality),
            surface_depth_quality: reader
                .read(keys::SURFACE_DEPTH_QUALITY)
                .unwrap_or(defaults.surface_depth_quality),
            window_mode: reader.read(keys::WINDOW_MODE).unwrap_or_default(),
            display_mode: DisplayMode::read(&mut reader),
            render_resolution: reader
                .read(keys::RENDER_RESOLUTION)
                .unwrap_or(defaults.render_resolution),
            switch_cycle_dwell_ms: reader.read(keys::SWITCH_CYCLE_DWELL_MS),
            scroll_notch_pixels: reader
                .read(keys::SCROLL_NOTCH_PIXELS)
                .unwrap_or(defaults.scroll_notch_pixels),
            accessibility: AccessibilityOptions::read(&mut reader),
            accessibility_panel_shown: reader
                .read(keys::ACCESSIBILITY_PANEL_SHOWN)
                .unwrap_or(defaults.accessibility_panel_shown),
            stored: StoredDocument::default(),
        };
        Self { stored, ..options }
    }

    /// The document a save writes: the loaded table with every known field set
    /// to its current value. An unset OS-seedable field writes no key.
    fn to_document(&self) -> toml::Table {
        let mut writer = DocumentWriter::new(&self.stored);
        writer.put(keys::PLAYER_ID, self.player_id.as_ref());
        writer.put_f32(keys::MOUSE_SENSITIVITY, Some(&self.mouse_sensitivity));
        writer.put(keys::INVERT_Y, Some(&self.invert_y));
        writer.put_f32(keys::VIEW_FEEL_SCALE, Some(&self.view_feel_scale));
        writer.put(keys::CROUCH_MODE, Some(&self.crouch_mode));
        writer.put(keys::SHADOW_QUALITY, Some(&self.shadow_quality));
        writer.put(keys::FOG_QUALITY, Some(&self.fog_quality));
        writer.put(
            keys::SURFACE_DEPTH_QUALITY,
            Some(&self.surface_depth_quality),
        );
        writer.put(keys::RENDER_RESOLUTION, Some(&self.render_resolution));
        writer.put(keys::WINDOW_MODE, Some(&self.window_mode));
        if let Some(mode) = &self.display_mode { mode.write(&mut writer); }
        writer.put(
            keys::SWITCH_CYCLE_DWELL_MS,
            self.switch_cycle_dwell_ms.as_ref(),
        );
        writer.put_f32(keys::SCROLL_NOTCH_PIXELS, Some(&self.scroll_notch_pixels));
        // Written only once true: a first launch's file holds no record until
        // the player closes the panel.
        writer.put(
            keys::ACCESSIBILITY_PANEL_SHOWN,
            self.accessibility_panel_shown.then_some(&true),
        );
        self.accessibility.write(&mut writer);
        writer.finish()
    }

    /// Serialize to TOML and write atomically to `path`.
    ///
    /// Rewrites the loaded document, so keys and values this build cannot read
    /// survive. Writes to a sibling `settings.toml.tmp` in the same directory,
    /// then renames over the target. Same-filesystem rename is atomic, so a
    /// reader never observes a partially written file. The temp file is a
    /// sibling (not `std::env::temp_dir()`) specifically so the rename stays
    /// within one filesystem.
    ///
    /// A file that could not be read or parsed at load is never replaced: the
    /// save is skipped and reports success, so the in-memory value stays live
    /// for the session.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if !self.can_persist() {
            return Ok(());
        }
        let serialized = toml::to_string_pretty(&self.to_document()).map_err(io::Error::other)?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let tmp_path = tmp_path_for(path);
        fs::write(&tmp_path, serialized)?;
        fs::rename(&tmp_path, path)?;
        Ok(())
    }
}

/// Whether a settings file was loaded normally, was absent, or must not be
/// replaced after an I/O/parse failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlayerOptionsLoadStatus {
    Missing,
    Loaded,
    Unavailable,
}

/// Sibling temp path (`<target>.tmp`) used by the atomic save. Kept next to the
/// target so the final `rename` stays on one filesystem.
fn tmp_path_for(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;
    use postretro_test_log_capture::LogCapture;
    use tempfile::tempdir;

    const EPSILON: f32 = 1e-6;

    /// The TOML a save would write.
    fn to_toml(options: &PlayerOptions) -> String {
        toml::to_string_pretty(&options.to_document()).unwrap()
    }

    /// Options as `load` would read `text`.
    fn from_toml(text: &str) -> PlayerOptions {
        let mut options =
            PlayerOptions::from_table(text.parse().unwrap(), Path::new("settings.toml"));
        options.sanitize();
        options
    }

    fn assert_options_eq(a: &PlayerOptions, b: &PlayerOptions) {
        assert_eq!(a.player_id, b.player_id);
        assert!(
            (a.mouse_sensitivity - b.mouse_sensitivity).abs() < EPSILON,
            "mouse_sensitivity: {} vs {}",
            a.mouse_sensitivity,
            b.mouse_sensitivity
        );
        assert_eq!(a.invert_y, b.invert_y);
        assert!(
            (a.view_feel_scale - b.view_feel_scale).abs() < EPSILON,
            "view_feel_scale: {} vs {}",
            a.view_feel_scale,
            b.view_feel_scale
        );
        assert_eq!(a.crouch_mode, b.crouch_mode);
        assert_eq!(a.shadow_quality, b.shadow_quality);
        assert_eq!(a.fog_quality, b.fog_quality);
        assert_eq!(a.surface_depth_quality, b.surface_depth_quality);
        assert_eq!(a.render_resolution, b.render_resolution);
        assert_eq!(a.switch_cycle_dwell_ms, b.switch_cycle_dwell_ms);
        assert!(
            (a.scroll_notch_pixels - b.scroll_notch_pixels).abs() < EPSILON,
            "scroll_notch_pixels: {} vs {}",
            a.scroll_notch_pixels,
            b.scroll_notch_pixels
        );
    }

    #[test]
    fn player_options_roundtrips_through_toml() {
        let original = PlayerOptions {
            player_id: Some([0x7c; 16]),
            mouse_sensitivity: 0.0035,
            invert_y: true,
            view_feel_scale: 0.5,
            crouch_mode: CrouchMode::Toggle,
            shadow_quality: ShadowQuality::Low,
            fog_quality: FogQuality::High,
            surface_depth_quality: SurfaceDepthQuality::Off,
            render_resolution: RenderResolution::Third,
            switch_cycle_dwell_ms: Some(250),
            scroll_notch_pixels: 96.0,
            ..PlayerOptions::default()
        };
        let serialized = to_toml(&original);
        let restored = from_toml(&serialized);
        assert_options_eq(&original, &restored);
    }

    #[test]
    fn load_returns_defaults_when_file_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        assert!(!path.exists());

        let loaded = PlayerOptions::load(&path);
        assert_options_eq(&loaded, &PlayerOptions::default());
        assert_eq!(loaded.player_id, None, "loading does not generate identity");
        // Load must not create the file; the caller owns writing defaults.
        assert!(!path.exists());
    }

    #[test]
    fn save_then_load_yields_equal_options() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");

        let options = PlayerOptions {
            player_id: Some([0x8d; 16]),
            mouse_sensitivity: 0.0042,
            invert_y: true,
            view_feel_scale: 0.25,
            crouch_mode: CrouchMode::Toggle,
            shadow_quality: ShadowQuality::Medium,
            fog_quality: FogQuality::Low,
            surface_depth_quality: SurfaceDepthQuality::On,
            switch_cycle_dwell_ms: Some(400),
            scroll_notch_pixels: 100.0,
            ..PlayerOptions::default()
        };
        options.save(&path).unwrap();

        let loaded = PlayerOptions::load(&path);
        assert_options_eq(&loaded, &options);
    }

    #[test]
    fn load_applies_present_keys_and_defaults_absent_keys() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        // Only `invert_y` present; the other two must fall back to defaults.
        fs::write(&path, "invert_y = true\n").unwrap();

        let loaded = PlayerOptions::load(&path);
        assert!(loaded.invert_y);
        assert!((loaded.mouse_sensitivity - DEFAULT_MOUSE_SENSITIVITY).abs() < EPSILON);
        assert!((loaded.view_feel_scale - 1.0).abs() < EPSILON);
        assert_eq!(loaded.player_id, None, "an absent key stays absent on load");
        // Schema evolution: a settings.toml written before Surface Depth
        // shipped must load with the feature ON, not silently disabled.
        assert_eq!(loaded.surface_depth_quality, SurfaceDepthQuality::On);
    }

    #[test]
    fn surface_depth_quality_persists_as_a_snake_case_state_name() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        for (state, wire) in [
            (SurfaceDepthQuality::Off, "off"),
            (SurfaceDepthQuality::On, "on"),
        ] {
            let options = PlayerOptions {
                surface_depth_quality: state,
                ..PlayerOptions::default()
            };
            options.save(&path).unwrap();
            let text = fs::read_to_string(&path).unwrap();
            assert!(
                text.contains(&format!("surface_depth_quality = \"{wire}\"")),
                "{state:?} must persist as `{wire}`; got:\n{text}"
            );
            assert_eq!(PlayerOptions::load(&path).surface_depth_quality, state);
            assert_eq!(state.slot_value(), wire);
            assert_eq!(SurfaceDepthQuality::from_slot_value(wire), Some(state));
        }
        assert_eq!(SurfaceDepthQuality::from_slot_value("ultra"), None);
        // The retired tier names are accepted only at the TOML file-reading
        // boundary (serde aliases on `On`), never through the slot layer.
        assert_eq!(SurfaceDepthQuality::from_slot_value("high"), None);
        assert_eq!(SurfaceDepthQuality::from_slot_value("low"), None);
    }

    #[test]
    fn the_retired_surface_depth_tiers_load_as_on_and_are_rewritten() {
        // A settings.toml written before D5 collapsed to off/on carries "low"
        // or "high" — "high" in every file that ever saved the default. Both
        // named a state that DID march, so both load as `On`. Letting either
        // fail the parse would discard the rest of the file with it, which is
        // the real cost: this is player data, not a code API.
        for retired in ["low", "high"] {
            let dir = tempdir().unwrap();
            let path = dir.path().join("settings.toml");
            fs::write(
                &path,
                format!("invert_y = true\nsurface_depth_quality = \"{retired}\"\n"),
            )
            .unwrap();

            let loaded = PlayerOptions::load(&path);
            assert_eq!(
                loaded.surface_depth_quality,
                SurfaceDepthQuality::On,
                "a saved `{retired}` must load as On, not Off",
            );
            assert!(
                loaded.invert_y,
                "`{retired}` must not take the rest of the file down with it",
            );

            // The next save normalizes the file onto the live vocabulary.
            loaded.save(&path).unwrap();
            let text = fs::read_to_string(&path).unwrap();
            assert!(
                text.contains("surface_depth_quality = \"on\""),
                "the retired name must not survive a save; got:\n{text}"
            );
        }
    }

    #[test]
    fn an_out_of_vocabulary_surface_depth_state_falls_back_for_that_field_alone() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(
            &path,
            "invert_y = true\nsurface_depth_quality = \"ultra\"\nfog_quality = \"low\"\n",
        )
        .unwrap();

        // A value that never named a real state falls back for its own field;
        // every other setting in the file loads intact.
        let loaded = PlayerOptions::load(&path);
        assert_eq!(loaded.surface_depth_quality, SurfaceDepthQuality::On);
        assert!(loaded.invert_y);
        assert_eq!(loaded.fog_quality, FogQuality::Low);
    }

    #[test]
    fn render_resolution_persists_each_value_as_its_snake_case_name() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        for (value, wire) in [
            (RenderResolution::Auto, "auto"),
            (RenderResolution::Native, "native"),
            (RenderResolution::Half, "half"),
            (RenderResolution::Third, "third"),
            (RenderResolution::Quarter, "quarter"),
        ] {
            let options = PlayerOptions {
                render_resolution: value,
                ..PlayerOptions::default()
            };
            options.save(&path).unwrap();
            let text = fs::read_to_string(&path).unwrap();
            assert!(
                text.contains(&format!("render_resolution = \"{wire}\"")),
                "{value:?} must persist as `{wire}`; got:\n{text}"
            );
            assert_eq!(PlayerOptions::load(&path).render_resolution, value);
            // The live slot shares the TOML vocabulary.
            assert_eq!(value.slot_value(), wire);
            assert_eq!(RenderResolution::from_slot_value(wire), Some(value));
        }
        assert_eq!(RenderResolution::from_slot_value("eighth"), None);
    }

    #[test]
    fn settings_without_render_resolution_load_as_auto() {
        let loaded = from_toml("invert_y = true\nfog_quality = \"low\"\n");
        assert_eq!(loaded.render_resolution, RenderResolution::Auto);
        assert_eq!(
            PlayerOptions::default().render_resolution,
            RenderResolution::Auto
        );
    }

    #[test]
    fn an_unknown_render_resolution_falls_back_to_auto_alone_and_survives_until_written() {
        let capture = LogCapture::start();
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(
            &path,
            "invert_y = true\nrender_resolution = \"eighth\"\nfog_quality = \"low\"\n",
        )
        .unwrap();

        let mut loaded = PlayerOptions::load(&path);
        assert_eq!(loaded.render_resolution, RenderResolution::Auto);
        assert!(loaded.invert_y);
        assert_eq!(loaded.fog_quality, FogQuality::Low);
        capture.assert_logged_once(Level::Warn, "unrecognized value for `render_resolution`");

        // A save of another field keeps the unrecognized text (round-trip).
        loaded.invert_y = false;
        loaded.mark_written(keys::INVERT_Y);
        loaded.save(&path).unwrap();
        let saved: toml::Table = fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(saved["render_resolution"].as_str(), Some("eighth"));

        // Writing the field replaces it.
        let mut reloaded = PlayerOptions::load(&path);
        reloaded.render_resolution = RenderResolution::Half;
        reloaded.mark_written(keys::RENDER_RESOLUTION);
        reloaded.save(&path).unwrap();
        let saved: toml::Table = fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(saved["render_resolution"].as_str(), Some("half"));
        assert_eq!(
            PlayerOptions::load(&path).render_resolution,
            RenderResolution::Half
        );
    }

    #[test]
    fn load_returns_defaults_and_preserves_file_when_malformed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let malformed = "this is not valid toml = = =\n";
        fs::write(&path, malformed).unwrap();

        let loaded = PlayerOptions::load(&path);
        assert_options_eq(&loaded, &PlayerOptions::default());

        // The malformed file must be left untouched for the human to fix.
        let on_disk = fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, malformed);
    }

    #[test]
    fn save_leaves_no_tmp_artifact_and_writes_parseable_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");

        let options = PlayerOptions {
            player_id: Some([0x9e; 16]),
            mouse_sensitivity: 0.003,
            invert_y: false,
            view_feel_scale: 0.75,
            crouch_mode: CrouchMode::Toggle,
            shadow_quality: ShadowQuality::Low,
            fog_quality: FogQuality::High,
            surface_depth_quality: SurfaceDepthQuality::On,
            switch_cycle_dwell_ms: Some(500),
            scroll_notch_pixels: 80.0,
            ..PlayerOptions::default()
        };
        options.save(&path).unwrap();

        // No `.tmp` sibling should survive a successful save.
        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !entries.iter().any(|name| name.ends_with(".tmp")),
            "found tmp artifact in {entries:?}"
        );

        let restored = from_toml(&fs::read_to_string(&path).unwrap());
        assert_options_eq(&restored, &options);
    }

    #[test]
    fn load_clamps_view_feel_scale_into_unit_range() {
        let dir = tempdir().unwrap();

        let high = dir.path().join("high.toml");
        fs::write(&high, "view_feel_scale = 2.0\n").unwrap();
        let loaded_high = PlayerOptions::load(&high);
        assert!((loaded_high.view_feel_scale - 1.0).abs() < EPSILON);

        let low = dir.path().join("low.toml");
        fs::write(&low, "view_feel_scale = -1.0\n").unwrap();
        let loaded_low = PlayerOptions::load(&low);
        assert!((loaded_low.view_feel_scale - 0.0).abs() < EPSILON);
    }

    #[test]
    fn crouch_mode_defaults_to_hold() {
        assert_eq!(CrouchMode::default(), CrouchMode::Hold);
        assert_eq!(PlayerOptions::default().crouch_mode, CrouchMode::Hold);
    }

    #[test]
    fn crouch_mode_roundtrips_through_toml() {
        for mode in [CrouchMode::Hold, CrouchMode::Toggle] {
            let original = PlayerOptions {
                crouch_mode: mode,
                ..PlayerOptions::default()
            };
            let serialized = to_toml(&original);
            let restored = from_toml(&serialized);
            assert_eq!(restored.crouch_mode, mode);
        }
    }

    #[test]
    fn crouch_mode_wire_format_is_snake_case() {
        // The TOML value must be the snake_case `"toggle"`, not `"Toggle"`.
        let toggle = PlayerOptions {
            crouch_mode: CrouchMode::Toggle,
            ..PlayerOptions::default()
        };
        let serialized = to_toml(&toggle);
        assert!(
            serialized.contains("crouch_mode = \"toggle\""),
            "expected snake_case crouch_mode key/value, got:\n{serialized}"
        );
    }

    #[test]
    fn load_defaults_crouch_mode_to_hold_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        // A file with no `crouch_mode` key must load as the `Hold` default.
        fs::write(&path, "invert_y = true\n").unwrap();

        let loaded = PlayerOptions::load(&path);
        assert_eq!(loaded.crouch_mode, CrouchMode::Hold);
    }

    #[test]
    fn load_applies_explicit_crouch_mode_toggle() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, "crouch_mode = \"toggle\"\n").unwrap();

        let loaded = PlayerOptions::load(&path);
        assert_eq!(loaded.crouch_mode, CrouchMode::Toggle);
    }

    #[test]
    fn player_options_graphics_quality_roundtrips_and_defaults() {
        let original = PlayerOptions {
            shadow_quality: ShadowQuality::Low,
            fog_quality: FogQuality::High,
            ..PlayerOptions::default()
        };
        let serialized = to_toml(&original);
        assert!(serialized.contains("shadow_quality = \"low\""));
        assert!(serialized.contains("fog_quality = \"high\""));

        let restored = from_toml(&serialized);
        assert_eq!(restored.shadow_quality, ShadowQuality::Low);
        assert_eq!(restored.fog_quality, FogQuality::High);

        let absent = from_toml("invert_y = true\n");
        assert_eq!(absent.shadow_quality, ShadowQuality::High);
        assert_eq!(absent.fog_quality, FogQuality::Medium);
    }

    #[test]
    fn an_unknown_graphics_quality_falls_back_alone_and_keeps_every_other_setting() {
        let capture = LogCapture::start();
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let text = "invert_y = true\nshadow_quality = \"ultra\"\nfog_quality = \"high\"\n\
                    [accessibility]\nflash_limiter = false\nsfx_volume = 0.5\n";
        fs::write(&path, text).unwrap();

        let (loaded, status) = PlayerOptions::load_with_status(&path);

        assert_eq!(status, PlayerOptionsLoadStatus::Loaded);
        assert_eq!(
            loaded.shadow_quality,
            ShadowQuality::High,
            "bad field takes its default"
        );
        assert!(loaded.invert_y);
        assert_eq!(loaded.fog_quality, FogQuality::High);
        assert!(!loaded.accessibility.flash_limiter);
        assert!((loaded.accessibility.sfx_volume - 0.5).abs() < EPSILON);
        capture.assert_logged_once(Level::Warn, "unrecognized value for `shadow_quality`");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            text,
            "loading never writes"
        );
    }

    #[test]
    fn an_unrecognized_accessibility_value_falls_back_alone_with_a_warning() {
        let capture = LogCapture::start();
        let loaded = from_toml(
            "fog_quality = \"low\"\n[accessibility]\nscreen_shake_scale = \"lots\"\n\
             mono_audio = true\nreduce_motion = \"maybe\"\n",
        );
        assert!((loaded.accessibility.screen_shake_scale - 1.0).abs() < EPSILON);
        assert!(loaded.accessibility.mono_audio);
        assert_eq!(
            loaded.accessibility.reduce_motion, None,
            "an unrecognized OS-seedable value resolves as unset"
        );
        assert_eq!(loaded.fog_quality, FogQuality::Low);
        capture.assert_logged_once(
            Level::Warn,
            "unrecognized value for `accessibility.screen_shake_scale`",
        );
        capture.assert_logged_once(
            Level::Warn,
            "unrecognized value for `accessibility.reduce_motion`",
        );
    }

    #[test]
    fn a_file_with_no_accessibility_group_loads_unset_and_defaults() {
        let loaded = from_toml("invert_y = true\n");
        assert_eq!(loaded.accessibility, AccessibilityOptions::default());
        assert_eq!(loaded.accessibility.reduce_motion, None);
        assert!(loaded.accessibility.flash_limiter, "limiter defaults on");
        assert!(!loaded.accessibility_panel_shown);
    }

    #[test]
    fn saving_writes_no_key_for_an_unset_os_seedable_field() {
        let text = to_toml(&PlayerOptions::default());
        assert!(
            !text.contains("reduce_motion"),
            "unset writes no key:\n{text}"
        );
        assert!(
            !text.contains("accessibility_panel_shown"),
            "no first-launch record until the panel closes:\n{text}"
        );
        assert!(text.contains("flash_limiter = true"));

        let set = PlayerOptions {
            accessibility: AccessibilityOptions {
                reduce_motion: Some(false),
                ..AccessibilityOptions::default()
            },
            accessibility_panel_shown: true,
            ..PlayerOptions::default()
        };
        let text = to_toml(&set);
        assert!(text.contains("reduce_motion = false"), "{text}");
        assert!(text.contains("accessibility_panel_shown = true"), "{text}");
        assert_eq!(from_toml(&text).accessibility.reduce_motion, Some(false));
    }

    #[test]
    fn save_keeps_unrecognized_values_and_unknown_keys_until_the_player_writes_them() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(
            &path,
            "fog_quality = \"ultra\"\nfuture_option = 7\n[accessibility]\n\
             reduce_motion = \"sometimes\"\nfuture_flag = true\n",
        )
        .unwrap();

        let mut loaded = PlayerOptions::load(&path);
        loaded.invert_y = true;
        loaded.mark_written(keys::INVERT_Y);
        loaded.save(&path).unwrap();

        let saved: toml::Table = fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(saved["fog_quality"].as_str(), Some("ultra"));
        assert_eq!(saved["future_option"].as_integer(), Some(7));
        assert_eq!(saved["invert_y"].as_bool(), Some(true));
        let group = saved["accessibility"].as_table().unwrap();
        assert_eq!(group["reduce_motion"].as_str(), Some("sometimes"));
        assert_eq!(group["future_flag"].as_bool(), Some(true));

        // A player write to the unrecognized field replaces it.
        let mut reloaded = PlayerOptions::load(&path);
        reloaded.fog_quality = FogQuality::Low;
        reloaded.mark_written(keys::FOG_QUALITY);
        reloaded.accessibility.reduce_motion = Some(true);
        reloaded.mark_written(keys::REDUCE_MOTION);
        reloaded.save(&path).unwrap();
        let saved: toml::Table = fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(saved["fog_quality"].as_str(), Some("low"));
        assert_eq!(
            saved["accessibility"].as_table().unwrap()["reduce_motion"].as_bool(),
            Some(true)
        );
        assert_eq!(saved["future_option"].as_integer(), Some(7));
    }

    #[test]
    fn a_file_that_is_not_valid_toml_is_never_replaced() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let malformed = "this is not valid toml = = =\n";
        fs::write(&path, malformed).unwrap();

        let (mut loaded, status) = PlayerOptions::load_with_status(&path);
        assert_eq!(status, PlayerOptionsLoadStatus::Unavailable);
        assert!(!loaded.can_persist());

        loaded.accessibility.flash_limiter = false;
        loaded.accessibility_panel_shown = true;
        loaded.save(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), malformed);
    }

    #[test]
    fn accessibility_scales_and_volumes_clamp_into_unit_range() {
        let loaded = from_toml(
            "[accessibility]\nscreen_shake_scale = 3.0\nmaster_volume = -1.0\nui_volume = 0.25\n",
        );
        assert!((loaded.accessibility.screen_shake_scale - 1.0).abs() < EPSILON);
        assert!((loaded.accessibility.master_volume - 0.0).abs() < EPSILON);
        assert!((loaded.accessibility.ui_volume - 0.25).abs() < EPSILON);
    }

    #[test]
    fn player_id_roundtrips_and_defaults_to_none_when_absent() {
        let original = PlayerOptions {
            player_id: Some([0x3f; 16]),
            ..PlayerOptions::default()
        };
        let serialized = to_toml(&original);
        let restored = from_toml(&serialized);
        assert_eq!(restored.player_id, original.player_id);

        let without_id = from_toml("invert_y = true\n");
        assert_eq!(without_id.player_id, None);
    }

    #[test]
    fn wieldable_input_options_default_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, "invert_y = true\n").unwrap();

        let loaded = PlayerOptions::load(&path);
        assert_eq!(loaded.switch_cycle_dwell_ms, None);
        assert!((loaded.scroll_notch_pixels - DEFAULT_SCROLL_NOTCH_PIXELS).abs() < EPSILON);
    }

    #[test]
    fn wieldable_input_options_sanitize_to_supported_ranges() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(
            &path,
            "switch_cycle_dwell_ms = 999999\nscroll_notch_pixels = -1.0\n",
        )
        .unwrap();

        let loaded = PlayerOptions::load(&path);
        assert_eq!(
            loaded.switch_cycle_dwell_ms,
            Some(MAX_SWITCH_CYCLE_DWELL_MS)
        );
        assert!((loaded.scroll_notch_pixels - DEFAULT_SCROLL_NOTCH_PIXELS).abs() < EPSILON);
    }

    #[test]
    fn a_nan_view_feel_scale_falls_back_to_default() {
        // `clamp` leaves NaN untouched (it fails every `<`/`>` comparison), so
        // a hand-edited `nan` — a valid TOML float literal — used to sail past
        // `sanitize` and land in `ResolvedAccessibility`, where `PartialEq`
        // treats NaN as never equal to itself: the options bridge would then
        // never see two equal snapshots and re-apply every frame.
        let capture = LogCapture::start();
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, "view_feel_scale = nan\ninvert_y = true\n").unwrap();

        let loaded = PlayerOptions::load(&path);
        assert!((loaded.view_feel_scale - 1.0).abs() < EPSILON);
        assert!(
            loaded.invert_y,
            "a non-finite field must not take the rest of the file down with it"
        );
        capture.assert_logged_once(Level::Warn, "`view_feel_scale` is not a finite number");
    }

    #[test]
    fn every_non_finite_f32_field_falls_back_to_default_with_a_warning() {
        let capture = LogCapture::start();
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(
            &path,
            "mouse_sensitivity = inf\nscroll_notch_pixels = -nan\ninvert_y = true\n",
        )
        .unwrap();

        let loaded = PlayerOptions::load(&path);
        assert!((loaded.mouse_sensitivity - DEFAULT_MOUSE_SENSITIVITY).abs() < EPSILON);
        assert!((loaded.scroll_notch_pixels - DEFAULT_SCROLL_NOTCH_PIXELS).abs() < EPSILON);
        assert!(loaded.invert_y);
        capture.assert_logged_once(Level::Warn, "`mouse_sensitivity` is not a finite number");
        capture.assert_logged_once(Level::Warn, "`scroll_notch_pixels` is not a finite number");
    }

    #[test]
    fn default_options_save_mouse_sensitivity_at_full_f32_precision() {
        let text = to_toml(&PlayerOptions::default());
        assert!(
            text.contains("mouse_sensitivity = 0.002\n"),
            "Value::try_from(f32) widens through f64 and would write \
             0.0020000000949949026 instead; got:\n{text}"
        );
    }

    #[test]
    fn a_stepped_f32_value_saves_at_full_precision_not_f64_widened_noise() {
        // Regression for a panel step (e.g. a slider settling on 0.85): the
        // generic `Value::try_from` path serializes f32 through
        // `serialize_f64(value as f64)`, so 0.85 would round-trip to
        // 0.8500000238418579 and get rewritten on every subsequent save.
        let stepped = PlayerOptions {
            view_feel_scale: 0.85,
            ..PlayerOptions::default()
        };
        let text = to_toml(&stepped);
        assert!(text.contains("view_feel_scale = 0.85\n"), "got:\n{text}");
    }
}
