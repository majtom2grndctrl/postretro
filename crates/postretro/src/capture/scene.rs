// Tool-facing static frame-capture scene vocabulary and validation.
// See: context/plans/in-progress/E20--frame-capture

use serde::Deserialize;
use thiserror::Error;

/// Default horizontal field of view in degrees, matching `camera::HFOV`.
pub(crate) const DEFAULT_FOV_DEG: f32 = 100.0;
const MIN_FOV_DEG: f32 = 60.0;
const MAX_FOV_DEG: f32 = 130.0;
const MAX_ABS_PITCH_DEG: f32 = 89.0;
const MAX_CAPTURE_DIMENSION: u32 = 8192;

/// A deterministic, world-only frame capture request.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) struct CaptureScene {
    pub(crate) map: String,
    pub(crate) camera: CameraPose,
    pub(crate) resolution: [u32; 2],
    pub(crate) output: String,
    pub(crate) force_active: Option<Vec<ForcedAnimLight>>,
}

/// An authored, single-instant active state for tagged baked animated lights.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) struct ForcedAnimLight {
    pub(crate) tag: String,
    pub(crate) radiance: [f32; 3],
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
    #[error("invalid capture scene: force_active tag must not be empty")]
    EmptyForcedAnimLightTag,
    #[error("invalid capture scene: force_active radiance must be finite")]
    NonFiniteForcedAnimLightRadiance,
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
    if let Some(forced_lights) = &scene.force_active {
        for light in forced_lights {
            if light.tag.trim().is_empty() {
                return Err(SceneError::EmptyForcedAnimLightTag);
            }
            if !light.radiance.into_iter().all(f32::is_finite) {
                return Err(SceneError::NonFiniteForcedAnimLightRadiance);
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
