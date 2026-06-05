// Per-human runtime preferences and their persistence to a human-editable
// `settings.toml`. Pure data + filesystem — no wgpu, renderer, or UI coupling.
// See: context/lib/player_options.md

// The store ships ahead of its boot wiring (Task 2): `load`/`save`/
// `settings_path` have no caller yet. Allow at the module level until the boot
// sequence consumes them.
#![allow(dead_code)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::input::DEFAULT_MOUSE_SENSITIVITY;

/// Filename written into the platform config directory.
const SETTINGS_FILENAME: &str = "settings.toml";

/// Per-human runtime preferences, persisted as TOML.
///
/// Wire format is deliberately snake_case (serde default, no `rename_all`):
/// this is a human-editable config, distinct from the project's camelCase
/// script-facing surface. Every field carries a `serde(default = ...)` so that
/// partial or older files load cleanly — missing keys fall back to defaults
/// rather than failing the whole parse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerOptions {
    /// Radians per raw mouse unit. Must be finite and > 0; defaults to the
    /// input subsystem's `DEFAULT_MOUSE_SENSITIVITY`.
    #[serde(default = "default_mouse_sensitivity")]
    pub mouse_sensitivity: f32,

    /// When true, the pitch (Y) mouse axis is negated.
    #[serde(default = "default_invert_y")]
    pub invert_y: bool,

    /// Accessibility scale for view feel (head bob, kick, sway). Clamped to
    /// `[0, 1]` on load rather than rejected, so a hand-edited out-of-range
    /// value degrades gracefully.
    #[serde(default = "default_view_feel_scale")]
    pub view_feel_scale: f32,
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

impl Default for PlayerOptions {
    fn default() -> Self {
        Self {
            mouse_sensitivity: default_mouse_sensitivity(),
            invert_y: default_invert_y(),
            view_feel_scale: default_view_feel_scale(),
        }
    }
}

impl PlayerOptions {
    /// Clamp loaded values into their valid ranges. Applied after
    /// deserialization so hand-edited out-of-range values are corrected rather
    /// than rejected. `mouse_sensitivity` falls back to its default when not
    /// finite-positive (a zero/negative/NaN sensitivity would break look input).
    fn sanitize(&mut self) {
        self.view_feel_scale = self.view_feel_scale.clamp(0.0, 1.0);
        if !(self.mouse_sensitivity.is_finite() && self.mouse_sensitivity > 0.0) {
            self.mouse_sensitivity = default_mouse_sensitivity();
        }
    }

    /// Load options from `path`.
    ///
    /// - Missing file (read error): returns defaults. The caller is expected to
    ///   write them back so the human gets an editable starting file.
    /// - Parse error: logs a warning and returns defaults *without* touching the
    ///   file, so a malformed hand edit is preserved for the human to fix.
    /// - Success: deserializes, then sanitizes ranges.
    pub fn load(path: &Path) -> Self {
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            // Missing/unreadable file is the normal first-run path; the caller
            // writes defaults. Not a warning.
            Err(_) => return Self::default(),
        };

        match toml::from_str::<PlayerOptions>(&contents) {
            Ok(mut options) => {
                options.sanitize();
                options
            }
            Err(err) => {
                log::warn!(
                    "[Options] failed to parse {}: {err}; using defaults (file left unmodified)",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Serialize to TOML and write atomically to `path`.
    ///
    /// Writes to a sibling `settings.toml.tmp` in the same directory, then
    /// renames over the target. Same-filesystem rename is atomic, so a reader
    /// never observes a partially written file. The temp file is a sibling (not
    /// `std::env::temp_dir()`) specifically so the rename stays within one
    /// filesystem.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let serialized = toml::to_string_pretty(self).map_err(io::Error::other)?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let tmp_path = tmp_path_for(path);
        fs::write(&tmp_path, serialized)?;
        fs::rename(&tmp_path, path)?;
        Ok(())
    }
}

/// Sibling temp path (`<target>.tmp`) used by the atomic save. Kept next to the
/// target so the final `rename` stays on one filesystem.
fn tmp_path_for(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Resolve the on-disk settings path: `<platform config dir>/settings.toml`.
///
/// Kept separate from `load`/`save` so tests inject a temp path instead of
/// touching the real user config directory. Returns `None` if the platform
/// provides no config directory (rare; caller decides the fallback).
pub fn settings_path() -> Option<PathBuf> {
    ProjectDirs::from("", "", "postretro").map(|dirs| dirs.config_dir().join(SETTINGS_FILENAME))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const EPSILON: f32 = 1e-6;

    fn assert_options_eq(a: &PlayerOptions, b: &PlayerOptions) {
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
    }

    #[test]
    fn player_options_roundtrips_through_toml() {
        let original = PlayerOptions {
            mouse_sensitivity: 0.0035,
            invert_y: true,
            view_feel_scale: 0.5,
        };
        let serialized = toml::to_string_pretty(&original).unwrap();
        let restored: PlayerOptions = toml::from_str(&serialized).unwrap();
        assert_options_eq(&original, &restored);
    }

    #[test]
    fn load_returns_defaults_when_file_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        assert!(!path.exists());

        let loaded = PlayerOptions::load(&path);
        assert_options_eq(&loaded, &PlayerOptions::default());
        // Load must not create the file; the caller owns writing defaults.
        assert!(!path.exists());
    }

    #[test]
    fn save_then_load_yields_equal_options() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");

        let options = PlayerOptions {
            mouse_sensitivity: 0.0042,
            invert_y: true,
            view_feel_scale: 0.25,
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
            mouse_sensitivity: 0.003,
            invert_y: false,
            view_feel_scale: 0.75,
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

        let restored: PlayerOptions = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
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
}
