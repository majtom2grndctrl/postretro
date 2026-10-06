// Settings persistence: tolerant per-field load and atomic round-trip save of
// `settings.toml`.
// See: context/lib/player_options.md §2

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::document::{DocumentWriter, FieldReader, StoredDocument};
use super::{AccessibilityOptions, DisplayMode, PlayerOptions, keys};

impl PlayerOptions {
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

    pub(super) fn unavailable() -> Self {
        Self {
            stored: StoredDocument::read_only(),
            ..Self::default()
        }
    }

    pub(super) fn from_table(table: toml::Table, path: &Path) -> Self {
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
            game_binding_writes: Default::default(),
            stored: StoredDocument::default(),
        };
        Self { stored, ..options }
    }

    /// The document a save writes: the loaded table with every known field set
    /// to its current value. An unset OS-seedable field writes no key.
    pub(super) fn to_document(&self) -> toml::Table {
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
        if let Some(mode) = &self.display_mode {
            mode.write(&mut writer);
        }
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
        self.write_game_bindings(&mut writer);
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
pub(super) fn tmp_path_for(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}
