// Capture-only ghost presentation: a scene's ghost style table and transparency mode.
// See: context/lib/rendering_pipeline.md §7.8 (Frame capture), §7.9

use postretro_foundation::{GhostStyle, GhostStyleDescriptor, GhostStyleRegistry};
use postretro_render_cpu::ghost_transparency::GhostTransparency;
use serde::Deserialize;

use super::scene::SceneError;

/// One capture-scene ghost style. Capture loads no mod manifest, so a scene
/// declares the styles its links' `ghost_style` names resolve against, as a
/// manifest's `ghostStyles` would. Every field but `name` is optional and takes
/// the floor style's value; there are no sound flags, since capture plays
/// nothing.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) struct CaptureGhostStyle {
    pub(crate) name: String,
    pub(crate) tint: Option<[f32; 3]>,
    pub(crate) opacity: Option<f32>,
    pub(crate) saturation: Option<f32>,
}

impl CaptureGhostStyle {
    fn descriptor(&self) -> GhostStyleDescriptor {
        let floor = GhostStyle::FLOOR;
        GhostStyleDescriptor {
            name: self.name.clone(),
            style: GhostStyle {
                tint: self.tint.unwrap_or(floor.tint),
                opacity: self.opacity.unwrap_or(floor.opacity),
                saturation: self.saturation.unwrap_or(floor.saturation),
                audio: false,
                impact_audio: false,
            },
        }
    }
}

/// A capture scene's ghost transparency: the manifest's `render.ghosts`
/// vocabulary, which capture cannot read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CaptureGhostTransparency {
    Blend,
    Dither,
}

impl CaptureGhostTransparency {
    pub(crate) const fn renderer(self) -> GhostTransparency {
        match self {
            Self::Blend => GhostTransparency::Blend,
            Self::Dither => GhostTransparency::Dither,
        }
    }
}

/// Reject a malformed scene style instead of dropping it with a warning, as a
/// manifest would: a capture is a fixture, so a typo must fail it rather than
/// silently present the floor style. Ranges are the manifest's.
pub(super) fn validate_ghost_styles(styles: &[CaptureGhostStyle]) -> Result<(), SceneError> {
    for (index, style) in styles.iter().enumerate() {
        style
            .descriptor()
            .validate()
            .map_err(|error| SceneError::GhostStyleField {
                index,
                field: error.field,
                requirement: error.requirement,
                got: error.got,
            })?;
        if styles[..index]
            .iter()
            .any(|earlier| earlier.name == style.name)
        {
            return Err(SceneError::DuplicateGhostStyle {
                index,
                name: style.name.clone(),
            });
        }
    }
    Ok(())
}

/// The validated scene styles as the table a link's `ghost_style` resolves
/// against at install. A name the table lacks presents the floor style and
/// warns once, as at a level install.
pub(super) fn ghost_style_registry(styles: &[CaptureGhostStyle]) -> GhostStyleRegistry {
    GhostStyleRegistry::from_declarations(
        "capture scene",
        styles.iter().map(|style| Ok(style.descriptor())),
    )
}

#[cfg(test)]
mod tests {
    use super::super::scene::parse_scene;
    use super::*;

    fn scene(ghosts: &str) -> String {
        format!(
            r#"{{
              "map": "content/dev/maps/test.prl",
              "camera": {{ "position": [1.0, 2.0, 3.0], "yaw_deg": 0.0, "pitch_deg": 0.0 }},
              "resolution": [64, 48],
              "output": "capture.png"{ghosts}
            }}"#
        )
    }

    #[test]
    fn scene_ghost_styles_fill_omitted_fields_from_the_floor_and_resolve_by_name() {
        let parsed = parse_scene(&scene(
            r#", "ghost_styles": [{ "name": "echo", "tint": [0.6, 0.8, 1.0], "opacity": 0.45 }],
               "ghost_transparency": "dither""#,
        ))
        .expect("scene parses");
        assert_eq!(
            parsed
                .ghost_transparency
                .map(CaptureGhostTransparency::renderer),
            Some(GhostTransparency::Dither)
        );
        let registry = ghost_style_registry(parsed.ghost_styles.as_deref().unwrap());
        let (style, undeclared) = registry.resolve("echo");
        assert!(undeclared.is_none());
        assert_eq!(style.tint, [0.6, 0.8, 1.0]);
        assert_eq!(style.opacity, 0.45);
        assert_eq!(style.saturation, GhostStyle::FLOOR.saturation);
        assert!(!style.audio && !style.impact_audio);
    }

    #[test]
    fn scene_ghost_styles_reject_out_of_range_fields_and_duplicate_names() {
        let error = parse_scene(&scene(
            r#", "ghost_styles": [{ "name": "echo", "opacity": 1.5 }]"#,
        ))
        .expect_err("opacity past 1 fails the scene");
        assert!(
            matches!(
                error,
                SceneError::GhostStyleField {
                    index: 0,
                    field: "opacity",
                    ..
                }
            ),
            "{error}"
        );
        let error = parse_scene(&scene(
            r#", "ghost_styles": [{ "name": "echo" }, { "name": "echo" }]"#,
        ))
        .expect_err("a repeated name fails the scene");
        assert!(
            matches!(error, SceneError::DuplicateGhostStyle { index: 1, .. }),
            "{error}"
        );
        assert!(
            parse_scene(&scene(r#", "ghost_transparency": "stipple""#)).is_err(),
            "an unknown transparency fails the scene"
        );
    }
}
