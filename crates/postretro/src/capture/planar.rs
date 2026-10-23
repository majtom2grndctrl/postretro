// Capture-only controls for repeatable planar spike measurements.
// See: context/plans/in-progress/planar-water-spike-contract.md

use postretro_render_cpu::planar_reflection::PlanarReflectionSettings;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct CapturePlanarReflection {
    pub on: bool,
    pub scale: f32,
    pub d: Option<f32>,
    pub band_d: f32,
    pub r: Option<f32>,
    pub band_r: f32,
}

impl Default for CapturePlanarReflection {
    fn default() -> Self {
        Self {
            on: true,
            scale: 0.5,
            d: None,
            band_d: 4.0,
            r: None,
            band_r: 4.0,
        }
    }
}

impl CapturePlanarReflection {
    pub fn validate(self) -> Result<(), &'static str> {
        if ![0.25, 0.5, 1.0].contains(&self.scale) {
            return Err("scale must be 0.25, 0.5 or 1");
        }
        for distance in [self.d, self.r].into_iter().flatten() {
            if !distance.is_finite() || !(1.0..=100.0).contains(&distance) {
                return Err("D/R must be null (unlimited) or finite metres in 1..=100");
            }
        }
        for band in [self.band_d, self.band_r] {
            if !band.is_finite() || !(0.1..=16.0).contains(&band) {
                return Err("fade widths must be finite metres in 0.1..=16");
            }
        }
        Ok(())
    }

    pub fn settings(self) -> PlanarReflectionSettings {
        PlanarReflectionSettings {
            on: self.on,
            scale: self.scale,
            d: self.d,
            band_d: self.band_d,
            r: self.r,
            band_r: self.band_r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::scene::parse_scene;
    use serde_json::json;

    fn scene(value: serde_json::Value) -> String {
        json!({"map":"test.prl", "camera":{"position":[0,2,0],"yaw_deg":0,"pitch_deg":0},
            "resolution":[960,540],"output":"test.png","planar_reflection":value})
        .to_string()
    }

    #[test]
    fn planar_capture_defaults_and_explicit_knobs_reach_settings() {
        let parsed = parse_scene(&scene(json!({}))).unwrap();
        let settings = parsed.planar_reflection.unwrap().settings();
        assert!(settings.on);
        assert_eq!(settings.scale, 0.5);
        assert_eq!(settings.d, None);
        let parsed = parse_scene(&scene(json!({"on":false,"scale":0.25,"d":15,"r":8,
            "band_d":2,"band_r":1})))
        .unwrap();
        assert_eq!(
            parsed.planar_reflection.unwrap().settings(),
            PlanarReflectionSettings {
                on: false,
                scale: 0.25,
                d: Some(15.0),
                r: Some(8.0),
                band_d: 2.0,
                band_r: 1.0
            }
        );
    }

    #[test]
    fn planar_capture_rejects_invalid_knobs_before_gpu_initialization() {
        for value in [
            json!({"scale":0.3}),
            json!({"d":0}),
            json!({"r":101}),
            json!({"band_d":0}),
            json!({"band_r":17}),
            json!({"typo":1}),
        ] {
            assert!(parse_scene(&scene(value)).is_err());
        }
    }
}
