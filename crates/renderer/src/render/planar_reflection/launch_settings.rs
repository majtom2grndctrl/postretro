// Launch seeds for the existing planar spike sliders; never player settings.
// See: context/plans/in-progress/planar-water-spike-contract.md

use postretro_render_cpu::planar_reflection::PlanarReflectionSettings;

pub(super) fn from_environment() -> PlanarReflectionSettings {
    let settings = from_values(|key| std::env::var(key).ok());
    log::info!("[Renderer] Planar launch settings: {settings:?}");
    settings
}

fn from_values(read: impl Fn(&str) -> Option<String>) -> PlanarReflectionSettings {
    let mut settings = PlanarReflectionSettings {
        on: read("POSTRETRO_PLANAR_REFLECTION").as_deref() == Some("1"),
        ..Default::default()
    };
    for (key, field, low, high) in [
        ("POSTRETRO_PLANAR_SCALE", &mut settings.scale, 0.25, 1.0),
        ("POSTRETRO_PLANAR_BAND_D", &mut settings.band_d, 0.1, 16.0),
        ("POSTRETRO_PLANAR_BAND_R", &mut settings.band_r, 0.1, 16.0),
    ] {
        let Some(value) = read(key) else { continue };
        let parsed = value.parse::<f32>().ok().filter(|v| {
            v.is_finite()
                && (low..=high).contains(v)
                && (key != "POSTRETRO_PLANAR_SCALE" || [0.25, 0.5, 1.0].contains(v))
        });
        if let Some(value) = parsed {
            *field = value;
        } else {
            log::warn!("[Renderer] invalid {key}={value:?}; keeping planar slider default");
        }
    }
    for (key, field) in [
        ("POSTRETRO_PLANAR_D", &mut settings.d),
        ("POSTRETRO_PLANAR_R", &mut settings.r),
    ] {
        let Some(value) = read(key) else { continue };
        if value == "off" {
            continue;
        }
        if let Some(value) = value
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite() && (1.0..=100.0).contains(v))
        {
            *field = Some(value);
        } else {
            log::warn!("[Renderer] invalid {key}={value:?}; keeping distance unlimited");
        }
    }
    settings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded(values: &[(&str, &str)]) -> PlanarReflectionSettings {
        from_values(|key| values.iter().find(|v| v.0 == key).map(|v| v.1.to_owned()))
    }

    fn assert_defaults(settings: PlanarReflectionSettings, on: bool) {
        let defaults = PlanarReflectionSettings::default();
        assert_eq!(settings.on, on);
        assert!(settings.d.is_none() && settings.r.is_none());
        assert!((settings.scale - defaults.scale).abs() < f32::EPSILON);
        assert!((settings.band_d - defaults.band_d).abs() < f32::EPSILON);
        assert!((settings.band_r - defaults.band_r).abs() < f32::EPSILON);
    }

    #[test]
    fn planar_launch_seeds_the_same_controls_as_the_panel() {
        assert_defaults(seeded(&[]), false);
        let settings = seeded(&[
            ("POSTRETRO_PLANAR_REFLECTION", "1"),
            ("POSTRETRO_PLANAR_SCALE", "0.25"),
            ("POSTRETRO_PLANAR_D", "15"),
            ("POSTRETRO_PLANAR_R", "30"),
            ("POSTRETRO_PLANAR_BAND_D", "8"),
            ("POSTRETRO_PLANAR_BAND_R", "0.5"),
        ]);
        assert!(settings.on);
        assert!((settings.scale - 0.25).abs() < f32::EPSILON);
        assert!((settings.d.unwrap() - 15.0).abs() < f32::EPSILON);
        assert!((settings.r.unwrap() - 30.0).abs() < f32::EPSILON);
        assert!((settings.band_d - 8.0).abs() < f32::EPSILON);
        assert!((settings.band_r - 0.5).abs() < f32::EPSILON);
        assert_eq!(seeded(&[("POSTRETRO_PLANAR_D", "off")]).d, None);
    }

    #[test]
    fn planar_launch_bad_fields_warn_without_overriding_other_fields() {
        let logs = postretro_test_log_capture::LogCapture::start();
        for (key, value) in [
            ("POSTRETRO_PLANAR_SCALE", "0.4"),
            ("POSTRETRO_PLANAR_D", "NaN"),
            ("POSTRETRO_PLANAR_R", "101"),
            ("POSTRETRO_PLANAR_BAND_D", "0"),
            ("POSTRETRO_PLANAR_BAND_R", "inf"),
        ] {
            let settings = seeded(&[("POSTRETRO_PLANAR_REFLECTION", "1"), (key, value)]);
            assert_defaults(settings, true);
            logs.assert_logged_once(log::Level::Warn, &format!("invalid {key}="));
        }
    }
}
