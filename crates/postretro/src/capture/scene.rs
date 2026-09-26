// Tool-facing static frame-capture scene vocabulary and validation.
// See: context/lib/rendering_pipeline.md §7.8

use serde::Deserialize;
use thiserror::Error;

/// Default horizontal field of view in degrees, matching `camera::HFOV`.
pub(crate) const DEFAULT_FOV_DEG: f32 = 100.0;
use crate::camera::{MAX_FOV_DEG, MIN_FOV_DEG};
const MAX_ABS_PITCH_DEG: f32 = 89.0;
const MAX_CAPTURE_DIMENSION: u32 = 8192;
pub(super) const MAX_MEASUREMENT_WARMUP_FRAMES: u32 = 10_000;
pub(super) const MAX_MEASUREMENT_SAMPLE_FRAMES: u32 = 100_000;
// Capture overrides are linear HDR radiance. Six stops above unit white
// cover diagnostic lighting while leaving roughly 1023x headroom below the
// Rgba16Float atlas/scene ceiling (65504) for transport and accumulation.
// This is a capture-authoring budget, not a new scripting intensity limit.
const MAX_FORCED_RADIANCE: f32 = 64.0;

/// A deterministic capture of world geometry and authored receivers.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) struct CaptureScene {
    pub(crate) map: String,
    pub(crate) camera: CameraPose,
    pub(crate) resolution: [u32; 2],
    pub(crate) output: String,
    pub(crate) force_active: Option<Vec<ForcedAnimLight>>,
    /// Capture-only fixed crossfade values for animated-baked promotion. A
    /// one-frame capture cannot advance the normal ramp, so these expose known
    /// `(1 - w)` / `w` splits for same-adapter golden comparisons.
    pub(crate) force_promotion: Option<Vec<ForcedAnimatedPromotion>>,
    /// Exactness oracle for streamed SH compose. This is capture-only and
    /// defaults off so ordinary scenes exercise the shipped sampled-row gate.
    #[serde(default)]
    pub(crate) force_full_resident_sh_compose: bool,
    /// Optional stepped-frame measurement. Warmup and samples advance renderer
    /// animation at fixed 1/60-second steps; omission keeps the single readback.
    pub(crate) measurement: Option<CaptureMeasurement>,
}

/// Author-controlled output and bounds for a stepped capture measurement.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) struct CaptureMeasurement {
    pub(crate) report: String,
    pub(crate) warmup_frames: u32,
    pub(crate) sample_frames: u32,
}

/// An authored, single-instant active state for tagged baked animated lights.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) struct ForcedAnimLight {
    pub(crate) tag: String,
    pub(crate) radiance: [f32; 3],
}

/// A capture-only requested promotion weight for each animated-baked light
/// matching `tag`. It never changes gameplay state or normal pool ranking.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) struct ForcedAnimatedPromotion {
    pub(crate) tag: String,
    pub(crate) weight: f32,
}

/// Static camera pose expressed in degrees for author-facing JSON.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) struct CameraPose {
    pub(crate) position: [f32; 3],
    pub(crate) yaw_deg: f32,
    pub(crate) pitch_deg: f32,
    #[serde(default = "default_fov_deg")]
    pub(crate) fov_deg: f32,
}

const fn default_fov_deg() -> f32 {
    DEFAULT_FOV_DEG
}

#[derive(Debug, Error)]
pub(crate) enum SceneError {
    #[error("invalid capture scene: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("invalid capture scene: map must not be empty")]
    EmptyMap,
    #[error("invalid capture scene: output must not be empty")]
    EmptyOutput,
    #[error("invalid capture scene: measurement report must not be empty")]
    EmptyMeasurementReport,
    #[error("invalid capture scene: measurement warmup_frames must be in 1..=10000, got {value}")]
    MeasurementWarmupFramesOutOfRange { value: u32 },
    #[error("invalid capture scene: measurement sample_frames must be in 1..=100000, got {value}")]
    MeasurementSampleFramesOutOfRange { value: u32 },
    #[error("invalid capture scene: force_active tag must not be empty")]
    EmptyForcedAnimLightTag,
    #[error("invalid capture scene: force_promotion tag must not be empty")]
    EmptyForcedAnimatedPromotionTag,
    #[error("invalid capture scene: force_active radiance must be finite")]
    NonFiniteForcedAnimLightRadiance,
    #[error(
        "invalid capture scene: force_active radiance channels must be in 0..={MAX_FORCED_RADIANCE}, got {value}"
    )]
    ForcedAnimLightRadianceOutOfRange { value: f32 },
    #[error("invalid capture scene: force_promotion weight must be finite")]
    NonFiniteForcedAnimatedPromotionWeight,
    #[error("invalid capture scene: force_promotion weight must be in 0.0..=1.0, got {value}")]
    ForcedAnimatedPromotionWeightOutOfRange { value: f32 },
    #[error(
        "invalid capture scene: fov_deg must be between {MIN_FOV_DEG} and {MAX_FOV_DEG}, got {value}"
    )]
    FovOutOfRange { value: f32 },
    #[error(
        "invalid capture scene: resolution dimensions must be in 1..={MAX_CAPTURE_DIMENSION}, got {width}x{height}"
    )]
    ResolutionOutOfRange { width: u32, height: u32 },
    #[error("invalid capture scene: camera position, yaw_deg, and pitch_deg must be finite")]
    NonFiniteCamera,
    #[error(
        "invalid capture scene: pitch_deg must be between -{MAX_ABS_PITCH_DEG} and {MAX_ABS_PITCH_DEG}, got {value}"
    )]
    PitchOutOfRange { value: f32 },
    #[error("invalid capture scene: camera position is too large to form a stable view matrix")]
    DegenerateCamera,
}

/// Parse a scene document and validate all GPU-independent authoring limits.
pub(crate) fn parse_scene(json: &str) -> Result<CaptureScene, SceneError> {
    let scene: CaptureScene = serde_json::from_str(json)?;
    validate_scene(&scene)?;
    Ok(scene)
}

fn validate_scene(scene: &CaptureScene) -> Result<(), SceneError> {
    if scene.map.trim().is_empty() {
        return Err(SceneError::EmptyMap);
    }
    if scene.output.trim().is_empty() {
        return Err(SceneError::EmptyOutput);
    }
    if let Some(measurement) = &scene.measurement {
        if measurement.report.trim().is_empty() {
            return Err(SceneError::EmptyMeasurementReport);
        }
        if !(1..=MAX_MEASUREMENT_WARMUP_FRAMES).contains(&measurement.warmup_frames) {
            return Err(SceneError::MeasurementWarmupFramesOutOfRange {
                value: measurement.warmup_frames,
            });
        }
        if !(1..=MAX_MEASUREMENT_SAMPLE_FRAMES).contains(&measurement.sample_frames) {
            return Err(SceneError::MeasurementSampleFramesOutOfRange {
                value: measurement.sample_frames,
            });
        }
    }
    if let Some(forced_lights) = &scene.force_active {
        for light in forced_lights {
            if light.tag.trim().is_empty() {
                return Err(SceneError::EmptyForcedAnimLightTag);
            }
            if !light.radiance.into_iter().all(f32::is_finite) {
                return Err(SceneError::NonFiniteForcedAnimLightRadiance);
            }
            for value in light.radiance {
                if !(0.0..=MAX_FORCED_RADIANCE).contains(&value) {
                    return Err(SceneError::ForcedAnimLightRadianceOutOfRange { value });
                }
            }
        }
    }
    if let Some(forced_promotions) = &scene.force_promotion {
        for promotion in forced_promotions {
            if promotion.tag.trim().is_empty() {
                return Err(SceneError::EmptyForcedAnimatedPromotionTag);
            }
            if !promotion.weight.is_finite() {
                return Err(SceneError::NonFiniteForcedAnimatedPromotionWeight);
            }
            if !(0.0..=1.0).contains(&promotion.weight) {
                return Err(SceneError::ForcedAnimatedPromotionWeightOutOfRange {
                    value: promotion.weight,
                });
            }
        }
    }
    if !(MIN_FOV_DEG..=MAX_FOV_DEG).contains(&scene.camera.fov_deg) {
        return Err(SceneError::FovOutOfRange {
            value: scene.camera.fov_deg,
        });
    }
    let [width, height] = scene.resolution;
    if width == 0 || height == 0 || width > MAX_CAPTURE_DIMENSION || height > MAX_CAPTURE_DIMENSION
    {
        return Err(SceneError::ResolutionOutOfRange { width, height });
    }
    if !scene.camera.position.into_iter().all(f32::is_finite)
        || !scene.camera.yaw_deg.is_finite()
        || !scene.camera.pitch_deg.is_finite()
    {
        return Err(SceneError::NonFiniteCamera);
    }
    if !(-MAX_ABS_PITCH_DEG..=MAX_ABS_PITCH_DEG).contains(&scene.camera.pitch_deg) {
        return Err(SceneError::PitchOutOfRange {
            value: scene.camera.pitch_deg,
        });
    }

    // `look_at_rh` subtracts eye from center in f32. At very large finite
    // coordinates, adding a unit look vector can round back to the eye and
    // produce a zero basis. Reject that pose before visibility or GPU work.
    let yaw = scene.camera.yaw_deg.to_radians();
    let pitch = scene.camera.pitch_deg.to_radians();
    let look_dir = glam::Vec3::new(
        -yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    );
    let eye = glam::Vec3::from_array(scene.camera.position);
    let rounded_forward = (eye + look_dir) - eye;
    if !rounded_forward.is_finite()
        || rounded_forward.length_squared() <= f32::EPSILON
        || rounded_forward.cross(glam::Vec3::Y).length_squared() <= f32::EPSILON
    {
        return Err(SceneError::DegenerateCamera);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCENE_WITH_DEFAULT_FOV: &str = r#"
        {
          "map": "content/dev/maps/test.prl",
          "camera": { "position": [1.0, 2.0, 3.0], "yaw_deg": 45.0, "pitch_deg": -10.0 },
          "resolution": [1280, 720],
          "output": "capture.png"
        }
    "#;

    #[test]
    fn parse_scene_applies_default_fov() {
        let scene = parse_scene(SCENE_WITH_DEFAULT_FOV).expect("scene must parse");
        assert_eq!(scene.camera.fov_deg, DEFAULT_FOV_DEG);
        assert!(
            scene.measurement.is_none(),
            "an omitted measurement block must retain the legacy one-frame capture path"
        );
        assert!(!scene.force_full_resident_sh_compose);
    }

    #[test]
    fn parse_scene_rejects_malformed_json() {
        assert!(matches!(
            parse_scene("{ \"map\": "),
            Err(SceneError::Parse(_))
        ));
    }

    #[test]
    fn parse_scene_rejects_unknown_fields() {
        let json = SCENE_WITH_DEFAULT_FOV.replace(
            "\"output\": \"capture.png\"",
            "\"output\": \"capture.png\", \"unexpected\": true",
        );
        let err = parse_scene(&json).expect_err("unknown field must fail");
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn parse_scene_accepts_force_active_lights() {
        let json = SCENE_WITH_DEFAULT_FOV.replace(
            "\"output\": \"capture.png\"",
            "\"output\": \"capture.png\", \"force_active\": [{ \"tag\": \"alarm_light\", \"radiance\": [4.0, 0.0, 0.0] }]",
        );

        let scene = parse_scene(&json).expect("force_active scene must parse");
        assert_eq!(
            scene.force_active,
            Some(vec![ForcedAnimLight {
                tag: "alarm_light".into(),
                radiance: [4.0, 0.0, 0.0],
            }])
        );
    }

    #[test]
    fn parse_scene_accepts_capture_only_forced_promotion_weight() {
        let json = SCENE_WITH_DEFAULT_FOV.replace(
            "\"output\": \"capture.png\"",
            "\"output\": \"capture.png\", \"force_promotion\": [{ \"tag\": \"alarm_light\", \"weight\": 0.5 }]",
        );

        let scene = parse_scene(&json).expect("forced promotion scene must parse");
        assert_eq!(
            scene.force_promotion,
            Some(vec![ForcedAnimatedPromotion {
                tag: "alarm_light".into(),
                weight: 0.5,
            }])
        );
    }

    #[test]
    fn parse_scene_accepts_full_resident_compose_exactness_oracle() {
        let json = SCENE_WITH_DEFAULT_FOV.replace(
            "\"output\": \"capture.png\"",
            "\"output\": \"capture.png\", \"force_full_resident_sh_compose\": true",
        );

        let scene = parse_scene(&json).expect("full-resident oracle scene must parse");
        assert!(scene.force_full_resident_sh_compose);
    }

    #[test]
    fn sampled_row_exactness_scenes_pair_two_stepped_times_with_the_oracle() {
        let cases = [
            (
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../measurements/sh-probe-streaming/sampled-row-gating/animroom-gated-t050.scene.json"
                )),
                false,
                29,
            ),
            (
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../measurements/sh-probe-streaming/sampled-row-gating/animroom-full-t050.scene.json"
                )),
                true,
                29,
            ),
            (
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../measurements/sh-probe-streaming/sampled-row-gating/animroom-gated-t100.scene.json"
                )),
                false,
                59,
            ),
            (
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../measurements/sh-probe-streaming/sampled-row-gating/animroom-full-t100.scene.json"
                )),
                true,
                59,
            ),
        ];
        for (json, forced, sample_frames) in cases {
            let scene = parse_scene(json).expect("checked-in exactness scene must remain valid");
            assert_eq!(scene.force_full_resident_sh_compose, forced);
            assert_eq!(scene.measurement.unwrap().sample_frames, sample_frames);
        }
    }

    #[test]
    fn parse_scene_accepts_measurement_v1_fields() {
        let json = SCENE_WITH_DEFAULT_FOV.replace(
            "\"output\": \"capture.png\"",
            "\"output\": \"capture.png\", \"measurement\": { \"report\": \"capture.json\", \"warmup_frames\": 12, \"sample_frames\": 240 }",
        );

        let scene = parse_scene(&json).expect("measurement scene must parse");
        assert_eq!(
            scene.measurement,
            Some(CaptureMeasurement {
                report: "capture.json".into(),
                warmup_frames: 12,
                sample_frames: 240,
            })
        );
    }

    #[test]
    fn parse_scene_rejects_invalid_measurement_bounds_and_unknown_fields() {
        for (field, value) in [
            ("warmup_frames", "0"),
            ("warmup_frames", "10001"),
            ("sample_frames", "0"),
            ("sample_frames", "100001"),
        ] {
            let json = SCENE_WITH_DEFAULT_FOV.replace(
                "\"output\": \"capture.png\"",
                &format!(
                    "\"output\": \"capture.png\", \"measurement\": {{ \"report\": \"capture.json\", \"{field}\": {value}, \"{}\": {} }}",
                    if field == "warmup_frames" { "sample_frames" } else { "warmup_frames" },
                    if field == "warmup_frames" { "1" } else { "1" },
                ),
            );
            assert!(parse_scene(&json).is_err(), "{field}={value} must fail");
        }

        let empty = SCENE_WITH_DEFAULT_FOV.replace(
            "\"output\": \"capture.png\"",
            "\"output\": \"capture.png\", \"measurement\": { \"report\": \"  \", \"warmup_frames\": 1, \"sample_frames\": 1 }",
        );
        assert!(matches!(
            parse_scene(&empty),
            Err(SceneError::EmptyMeasurementReport)
        ));

        let unknown = SCENE_WITH_DEFAULT_FOV.replace(
            "\"output\": \"capture.png\"",
            "\"output\": \"capture.png\", \"measurement\": { \"report\": \"capture.json\", \"warmup_frames\": 1, \"sample_frames\": 1, \"unexpected\": true }",
        );
        let err = parse_scene(&unknown).expect_err("unknown measurement field must fail");
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn parse_scene_rejects_invalid_forced_promotion_weights() {
        for weight in [f32::NAN, -0.01, 1.01] {
            let json = SCENE_WITH_DEFAULT_FOV.replace(
                "\"output\": \"capture.png\"",
                &format!(
                    "\"output\": \"capture.png\", \"force_promotion\": [{{ \"tag\": \"alarm_light\", \"weight\": {weight} }}]"
                ),
            );
            assert!(
                parse_scene(&json).is_err(),
                "invalid forced-promotion weight {weight:?} must fail"
            );
        }
    }

    #[test]
    fn parse_scene_rejects_unknown_force_active_light_fields() {
        let json = SCENE_WITH_DEFAULT_FOV.replace(
            "\"output\": \"capture.png\"",
            "\"output\": \"capture.png\", \"force_active\": [{ \"tag\": \"alarm_light\", \"radiance\": [4.0, 0.0, 0.0], \"unexpected\": true }]",
        );
        let err = parse_scene(&json).expect_err("unknown nested field must fail");
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn parse_scene_rejects_empty_force_active_tag() {
        let json = SCENE_WITH_DEFAULT_FOV.replace(
            "\"output\": \"capture.png\"",
            "\"output\": \"capture.png\", \"force_active\": [{ \"tag\": \"  \", \"radiance\": [4.0, 0.0, 0.0] }]",
        );
        assert!(matches!(
            parse_scene(&json),
            Err(SceneError::EmptyForcedAnimLightTag)
        ));
    }

    #[test]
    fn force_active_radiance_respects_nonnegative_hdr_capture_budget() {
        for value in [-0.001, 64.01, f32::MAX] {
            let mut scene = parse_scene(SCENE_WITH_DEFAULT_FOV).unwrap();
            scene.force_active = Some(vec![ForcedAnimLight {
                tag: "alarm_light".into(),
                radiance: [0.0, value, 0.0],
            }]);
            assert!(matches!(
                validate_scene(&scene),
                Err(SceneError::ForcedAnimLightRadianceOutOfRange { .. })
            ));
        }
        let mut scene = parse_scene(SCENE_WITH_DEFAULT_FOV).unwrap();
        scene.force_active = Some(vec![ForcedAnimLight {
            tag: "alarm_light".into(),
            radiance: [0.0, 4.0, MAX_FORCED_RADIANCE],
        }]);
        assert!(validate_scene(&scene).is_ok());
    }

    #[test]
    fn validate_scene_rejects_non_finite_force_active_radiance() {
        let scene = CaptureScene {
            map: "content/dev/maps/test.prl".into(),
            camera: CameraPose {
                position: [1.0, 2.0, 3.0],
                yaw_deg: 45.0,
                pitch_deg: -10.0,
                fov_deg: DEFAULT_FOV_DEG,
            },
            resolution: [1280, 720],
            output: "capture.png".into(),
            force_active: Some(vec![ForcedAnimLight {
                tag: "alarm_light".into(),
                radiance: [f32::NAN, 0.0, 0.0],
            }]),
            force_promotion: None,
            force_full_resident_sh_compose: false,
            measurement: None,
        };
        assert!(matches!(
            validate_scene(&scene),
            Err(SceneError::NonFiniteForcedAnimLightRadiance)
        ));
    }

    #[test]
    fn parse_scene_rejects_missing_map() {
        let json = SCENE_WITH_DEFAULT_FOV.replace("\"map\": \"content/dev/maps/test.prl\",", "");
        let err = parse_scene(&json).expect_err("missing map must fail");
        assert!(err.to_string().contains("missing field `map`"));
    }

    #[test]
    fn parse_scene_rejects_fov_outside_configurable_range() {
        let json = SCENE_WITH_DEFAULT_FOV.replace(
            "\"pitch_deg\": -10.0 }",
            "\"pitch_deg\": -10.0, \"fov_deg\": 59.9 }",
        );
        assert!(matches!(
            parse_scene(&json),
            Err(SceneError::FovOutOfRange { .. })
        ));
    }

    #[test]
    fn parse_scene_rejects_zero_or_oversized_resolution() {
        let zero = SCENE_WITH_DEFAULT_FOV.replace("[1280, 720]", "[0, 720]");
        assert!(matches!(
            parse_scene(&zero),
            Err(SceneError::ResolutionOutOfRange {
                width: 0,
                height: 720
            })
        ));

        let oversized = SCENE_WITH_DEFAULT_FOV.replace("[1280, 720]", "[8193, 720]");
        assert!(matches!(
            parse_scene(&oversized),
            Err(SceneError::ResolutionOutOfRange {
                width: 8193,
                height: 720
            })
        ));
    }

    #[test]
    fn parse_scene_rejects_vertical_or_out_of_contract_pitch() {
        for pitch in [-90.0, 90.0, 180.0] {
            let json = SCENE_WITH_DEFAULT_FOV
                .replace("\"pitch_deg\": -10.0", &format!("\"pitch_deg\": {pitch}"));
            assert!(matches!(
                parse_scene(&json),
                Err(SceneError::PitchOutOfRange { .. })
            ));
        }
    }

    #[test]
    fn parse_scene_rejects_position_that_collapses_the_look_vector() {
        let json = SCENE_WITH_DEFAULT_FOV
            .replace("[1.0, 2.0, 3.0]", "[3.402823e38, 3.402823e38, 3.402823e38]");
        assert!(matches!(
            parse_scene(&json),
            Err(SceneError::DegenerateCamera)
        ));
    }
}
