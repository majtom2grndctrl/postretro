// Launch seeds for the diagnostics panel's planar reflection controls; dev-tools only.
// See: context/lib/rendering_pipeline.md §7.3a · context/lib/development_guide.md §6.4

use std::sync::OnceLock;

use postretro_render_cpu::planar_reflection::PlanarPreset;

/// The panel's launch state from `POSTRETRO_PLANAR_*`. Each preset field is
/// `None` where its variable is unset or malformed, leaving the tier's value;
/// the preset half applies to the first planar tier the renderer keeps, and
/// a later tier change resets to that tier's preset. Never a player setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PlanarLaunchSeed {
    /// `POSTRETRO_PLANAR_SCALE`: `0.25`, `0.5` or `1`.
    pub scale: Option<f32>,
    /// `POSTRETRO_PLANAR_D`: metres in 1..=100, or `off` for unlimited.
    pub d: Option<Option<f32>>,
    /// `POSTRETRO_PLANAR_BAND_D`: fade metres in 0.1..=16.
    pub band_d: Option<f32>,
    /// `POSTRETRO_PLANAR_R`: metres in 1..=100, or `off` for unlimited.
    pub r: Option<Option<f32>>,
    /// `POSTRETRO_PLANAR_BAND_R`: fade metres in 0.1..=16.
    pub band_r: Option<f32>,
    /// `POSTRETRO_PLANAR_OFF`: `1` routes the selected plane to SSR.
    pub off: bool,
    /// `POSTRETRO_PLANAR_CROP`: `0` walks the whole mirror frustum.
    pub crop: bool,
}

impl Default for PlanarLaunchSeed {
    fn default() -> Self {
        Self {
            scale: None,
            d: None,
            band_d: None,
            r: None,
            band_r: None,
            off: false,
            crop: true,
        }
    }
}

impl PlanarLaunchSeed {
    /// Whether any preset field is seeded.
    pub fn overrides_preset(&self) -> bool {
        self.scale.is_some()
            || self.d.is_some()
            || self.band_d.is_some()
            || self.r.is_some()
            || self.band_r.is_some()
    }

    /// `preset` with every seeded field in place of its own.
    pub fn apply(&self, preset: PlanarPreset) -> PlanarPreset {
        PlanarPreset {
            scale: self.scale.unwrap_or(preset.scale),
            d: self.d.unwrap_or(preset.d),
            band_d: self.band_d.unwrap_or(preset.band_d),
            r: self.r.unwrap_or(preset.r),
            band_r: self.band_r.unwrap_or(preset.band_r),
        }
    }
}

/// The process's launch seed, read once: a malformed variable warns once and
/// leaves its control at the panel default. Never blocks boot.
pub(super) fn from_environment() -> PlanarLaunchSeed {
    static SEED: OnceLock<PlanarLaunchSeed> = OnceLock::new();
    *SEED.get_or_init(|| {
        let seed = from_values(|key| std::env::var(key).ok());
        if seed != PlanarLaunchSeed::default() {
            log::info!("[Renderer] Planar launch seed: {seed:?}");
        }
        seed
    })
}

fn from_values(read: impl Fn(&str) -> Option<String>) -> PlanarLaunchSeed {
    let mut seed = PlanarLaunchSeed::default();
    let read_valid = |key: &str, parse: &dyn Fn(&str) -> bool| {
        let value = read(key)?;
        if parse(&value) {
            Some(value)
        } else {
            log::warn!("[Renderer] invalid {key}={value:?}; keeping the panel default");
            None
        }
    };
    let metres = |value: &str, range: std::ops::RangeInclusive<f32>| {
        value
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite() && range.contains(v))
    };
    seed.scale = read_valid("POSTRETRO_PLANAR_SCALE", &|v| {
        metres(v, 0.25..=1.0).is_some_and(|v| [0.25, 0.5, 1.0].contains(&v))
    })
    .and_then(|v| metres(&v, 0.25..=1.0));
    for (key, field) in [
        ("POSTRETRO_PLANAR_D", &mut seed.d),
        ("POSTRETRO_PLANAR_R", &mut seed.r),
    ] {
        *field = read_valid(key, &|v| v == "off" || metres(v, 1.0..=100.0).is_some())
            .map(|v| metres(&v, 1.0..=100.0));
    }
    for (key, field) in [
        ("POSTRETRO_PLANAR_BAND_D", &mut seed.band_d),
        ("POSTRETRO_PLANAR_BAND_R", &mut seed.band_r),
    ] {
        *field = read_valid(key, &|v| metres(v, 0.1..=16.0).is_some())
            .and_then(|v| metres(&v, 0.1..=16.0));
    }
    for (key, field) in [
        ("POSTRETRO_PLANAR_OFF", &mut seed.off),
        ("POSTRETRO_PLANAR_CROP", &mut seed.crop),
    ] {
        if let Some(value) = read_valid(key, &|v| v == "0" || v == "1") {
            *field = value == "1";
        }
    }
    seed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Renderer;
    use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
    use postretro_render_cpu::planar_reflection::planar_preset;

    fn seeded(values: &[(&str, &str)]) -> PlanarLaunchSeed {
        from_values(|key| values.iter().find(|v| v.0 == key).map(|v| v.1.to_owned()))
    }

    fn very_high() -> PlanarPreset {
        planar_preset(FluidReflectionQuality::VeryHigh).expect("very high runs planar")
    }

    #[test]
    fn planar_launch_seeds_the_same_controls_as_the_panel() {
        let unset = seeded(&[]);
        assert_eq!(unset, PlanarLaunchSeed::default());
        assert!(!unset.overrides_preset() && !unset.off && unset.crop);
        assert_eq!(unset.apply(very_high()), very_high());
        let seed = seeded(&[
            ("POSTRETRO_PLANAR_SCALE", "0.25"),
            ("POSTRETRO_PLANAR_D", "15"),
            ("POSTRETRO_PLANAR_R", "30"),
            ("POSTRETRO_PLANAR_BAND_D", "8"),
            ("POSTRETRO_PLANAR_BAND_R", "0.5"),
            ("POSTRETRO_PLANAR_OFF", "1"),
            ("POSTRETRO_PLANAR_CROP", "0"),
        ]);
        assert!(seed.overrides_preset() && seed.off && !seed.crop);
        assert_eq!(
            seed.apply(very_high()),
            PlanarPreset {
                scale: 0.25,
                d: Some(15.0),
                band_d: 8.0,
                r: Some(30.0),
                band_r: 0.5,
            }
        );
        // `off` seeds an unlimited distance over a tier that limits it; a
        // field left unset keeps the tier's.
        let limited = PlanarPreset {
            d: Some(20.0),
            ..very_high()
        };
        let unlimited = seeded(&[("POSTRETRO_PLANAR_D", "off")]);
        assert!(unlimited.overrides_preset());
        assert_eq!(unlimited.apply(limited).d, None);
        assert_eq!(
            seeded(&[("POSTRETRO_PLANAR_R", "12")]).apply(limited).d,
            Some(20.0)
        );
    }

    #[test]
    fn planar_launch_bad_fields_warn_once_and_keep_the_panel_default() {
        let logs = postretro_test_log_capture::LogCapture::start();
        for (key, value) in [
            ("POSTRETRO_PLANAR_SCALE", "0.4"),
            ("POSTRETRO_PLANAR_D", "NaN"),
            ("POSTRETRO_PLANAR_R", "101"),
            ("POSTRETRO_PLANAR_BAND_D", "0"),
            ("POSTRETRO_PLANAR_BAND_R", "inf"),
            ("POSTRETRO_PLANAR_OFF", "yes"),
            ("POSTRETRO_PLANAR_CROP", ""),
        ] {
            let seed = seeded(&[(key, value), ("POSTRETRO_PLANAR_BAND_D", "2")]);
            let expected = PlanarLaunchSeed {
                band_d: (key != "POSTRETRO_PLANAR_BAND_D").then_some(2.0),
                ..PlanarLaunchSeed::default()
            };
            assert_eq!(seed, expected, "{key}={value:?} leaves its control default");
            logs.assert_logged_once(log::Level::Warn, &format!("invalid {key}="));
        }
    }

    // The seed's preset half lands on the first planar tier the renderer
    // keeps, and a tier change resets to that tier's own preset, as an
    // override set from the panel does. An explicit override discards a seed
    // not yet taken.
    #[test]
    fn planar_launch_seed_presets_the_first_planar_tier_and_a_tier_change_resets_it() {
        let mut renderer = match Renderer::new_offscreen(16, 16) {
            Ok(renderer) => renderer,
            Err(error) if error.to_string().contains("requires a GPU adapter") => {
                eprintln!("[PlanarLaunch] skipped: no adapter ({error:#})");
                return;
            }
            Err(error) => panic!("adapter present but renderer initialization failed: {error:#}"),
        };
        let seed = seeded(&[
            ("POSTRETRO_PLANAR_SCALE", "1"),
            ("POSTRETRO_PLANAR_D", "15"),
        ]);
        renderer.set_reflection_quality(FluidReflectionQuality::High);
        renderer.full_mut().planar_reflection.dev.launch_seed = Some(seed);
        renderer.set_reflection_quality(FluidReflectionQuality::Medium);
        assert_eq!(renderer.planar_tuned_preset(), None);
        assert_eq!(
            renderer.full().planar_reflection.dev.launch_seed,
            Some(seed),
            "a tier with no planar layer leaves the seed pending"
        );
        renderer.set_reflection_quality(FluidReflectionQuality::VeryHigh);
        let tuned = seed.apply(very_high());
        assert_eq!(renderer.planar_tuned_preset(), Some(tuned));
        assert_eq!(renderer.planar_reflection_preset(), Some(tuned));
        renderer.set_reflection_quality(FluidReflectionQuality::Ultra);
        assert_eq!(
            renderer.planar_tuned_preset(),
            planar_preset(FluidReflectionQuality::Ultra),
            "a tier change resets to the new tier's preset"
        );
        assert!(renderer.set_planar_preset_override(Some(tuned)));
        assert_eq!(renderer.planar_tuned_preset(), Some(tuned));
        renderer.set_reflection_quality(FluidReflectionQuality::VeryHigh);
        assert_eq!(renderer.planar_tuned_preset(), Some(very_high()));
        renderer.set_reflection_quality(FluidReflectionQuality::High);
        assert!(!renderer.set_planar_preset_override(Some(tuned)));
        renderer.set_reflection_quality(FluidReflectionQuality::VeryHigh);
        assert_eq!(renderer.planar_tuned_preset(), Some(very_high()));

        renderer.full_mut().planar_reflection.dev.launch_seed = Some(seed);
        assert!(renderer.set_planar_preset_override(None));
        renderer.set_reflection_quality(FluidReflectionQuality::Ultra);
        assert_eq!(
            renderer.planar_tuned_preset(),
            planar_preset(FluidReflectionQuality::Ultra),
            "an explicit override discards the pending seed"
        );

        // Planar off routes the plane to SSR through the app's preset read
        // while the panel keeps the preset it edits.
        renderer.set_planar_dev_off(true);
        assert_eq!(renderer.planar_reflection_preset(), None);
        assert!(renderer.planar_tuned_preset().is_some());
        renderer.set_planar_dev_off(false);
        assert!(renderer.planar_reflection_preset().is_some());
    }
}
