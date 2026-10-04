// Persisted window preferences; no windowing or GPU dependencies.
// See: context/lib/player_options.md §7

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WindowMode {
    #[default]
    Windowed,
    Borderless,
    Exclusive,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{PlayerOptions, keys};
    #[test]
    fn window_mode_round_trip_and_field_fallback_preserve_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        for mode in [
            WindowMode::Windowed,
            WindowMode::Borderless,
            WindowMode::Exclusive,
        ] {
            let options = PlayerOptions {
                window_mode: mode,
                invert_y: true,
                ..Default::default()
            };
            options.save(&path).unwrap();
            assert_eq!(PlayerOptions::load(&path), options);
        }
        for value in ["\"future\"", "42", "[]"] {
            std::fs::write(&path, format!("invert_y = true\nwindow_mode = {value}\n")).unwrap();
            let mut options = PlayerOptions::load(&path);
            assert_eq!(options.window_mode, WindowMode::Windowed);
            assert!(options.invert_y);
            options.save(&path).unwrap();
            assert!(std::fs::read_to_string(&path).unwrap().contains(value));
            options.window_mode = WindowMode::Borderless;
            options.mark_written(keys::WINDOW_MODE);
            options.save(&path).unwrap();
            assert_eq!(
                PlayerOptions::load(&path).window_mode,
                WindowMode::Borderless
            );
        }
    }
}
