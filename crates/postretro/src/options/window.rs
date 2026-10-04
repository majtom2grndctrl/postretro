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

impl WindowMode {
    pub(crate) fn slot_value(self) -> &'static str {
        match self {
            Self::Windowed => "windowed",
            Self::Borderless => "borderless",
            Self::Exclusive => "exclusive",
        }
    }
    pub(crate) fn from_slot_value(value: &str) -> Option<Self> {
        match value {
            "windowed" => Some(Self::Windowed),
            "borderless" => Some(Self::Borderless),
            "exclusive" => Some(Self::Exclusive),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DisplayMode {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) refresh_millihertz: u32,
    pub(crate) bit_depth: u16,
    pub(crate) monitor: String,
}

pub(super) const DISPLAY_KEYS: [&str; 5] = [
    "display_mode_width",
    "display_mode_height",
    "display_mode_refresh_millihertz",
    "display_mode_bit_depth",
    "display_mode_monitor",
];

impl DisplayMode {
    pub(super) fn read(reader: &mut super::document::FieldReader<'_>) -> Option<Self> {
        let width = reader.read::<u32>(DISPLAY_KEYS[0]);
        let height = reader.read::<u32>(DISPLAY_KEYS[1]);
        let refresh = reader.read::<u32>(DISPLAY_KEYS[2]);
        let depth = reader.read::<u16>(DISPLAY_KEYS[3]);
        let monitor = reader.read::<String>(DISPLAY_KEYS[4]);
        let mode = Self {
            width: width?,
            height: height?,
            refresh_millihertz: refresh?,
            bit_depth: depth?,
            monitor: monitor?,
        };
        if mode.width == 0 || mode.height == 0 || mode.bit_depth == 0 {
            log::warn!(
                "[Options] unusable stored display mode; keeping its text and using the session fallback"
            );
            return None;
        }
        Some(mode)
    }
    pub(super) fn write(&self, writer: &mut super::document::DocumentWriter<'_>) {
        writer.put(DISPLAY_KEYS[0], Some(&self.width));
        writer.put(DISPLAY_KEYS[1], Some(&self.height));
        writer.put(DISPLAY_KEYS[2], Some(&self.refresh_millihertz));
        writer.put(DISPLAY_KEYS[3], Some(&self.bit_depth));
        writer.put(DISPLAY_KEYS[4], Some(&self.monitor));
    }
}

impl super::PlayerOptions {
    pub(crate) fn set_display_mode(&mut self, mode: DisplayMode) {
        self.display_mode = Some(mode);
        for key in DISPLAY_KEYS {
            self.mark_written(key);
        }
    }
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

#[cfg(test)]
mod display_tests {
    use super::*;
    use crate::options::PlayerOptions;
    #[test]
    fn display_mode_round_trip_and_partial_tuple_preserve_loaded_document() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let mode = DisplayMode {
            width: 1920,
            height: 1080,
            refresh_millihertz: 59940,
            bit_depth: 32,
            monitor: "test monitor".into(),
        };
        let mut options = PlayerOptions::default();
        options.set_display_mode(mode.clone());
        options.save(&path).unwrap();
        assert_eq!(PlayerOptions::load(&path).display_mode, Some(mode.clone()));
        std::fs::write(
            &path,
            "invert_y = true\ndisplay_mode_width = 1920\ndisplay_mode_monitor = 42\n",
        )
        .unwrap();
        let mut loaded = PlayerOptions::load(&path);
        assert!(loaded.invert_y);
        assert_eq!(loaded.display_mode, None);
        loaded.save(&path).unwrap();
        let table: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(table["display_mode_width"].as_integer(), Some(1920));
        assert_eq!(table["display_mode_monitor"].as_integer(), Some(42));
        loaded.set_display_mode(mode.clone());
        loaded.save(&path).unwrap();
        assert_eq!(PlayerOptions::load(&path).display_mode, Some(mode));
    }

    #[test]
    fn window_mode_catalog_matches_persisted_vocabulary_and_capability() {
        use postretro_entities::{
            EngineStateCapability, EngineStateValueType, engine_state_catalog,
        };
        let catalog = engine_state_catalog().unwrap();
        let entry = catalog
            .entries()
            .iter()
            .find(|entry| entry.wire_name == "options.windowMode")
            .unwrap();
        let EngineStateValueType::Enum { values } = entry.value_type else {
            panic!("window mode is enum");
        };
        assert_eq!(entry.capability, EngineStateCapability::Writable);
        assert_eq!(values.len(), 3);
        for mode in [
            WindowMode::Windowed,
            WindowMode::Borderless,
            WindowMode::Exclusive,
        ] {
            let wire = match mode {
                WindowMode::Windowed => "windowed",
                WindowMode::Borderless => "borderless",
                WindowMode::Exclusive => "exclusive",
            };
            assert!(values.contains(&wire));
            assert_eq!(WindowMode::from_slot_value(wire), Some(mode));
            assert_eq!(mode.slot_value(), wire);
        }
        for entry in catalog
            .entries()
            .iter()
            .filter(|entry| entry.wire_name.starts_with("window.displayMode"))
        {
            assert_eq!(entry.capability, EngineStateCapability::Readonly);
            assert!(!entry.persist);
        }
    }
}
