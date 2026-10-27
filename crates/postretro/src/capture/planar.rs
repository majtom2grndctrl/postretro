// Capture's planar mirror: scene preset overrides, crop switch and the play frame's mirror step.
// See: context/lib/rendering_pipeline.md §7.3a, §7.8

use postretro_level_loader::LevelWorld;
use postretro_render_cpu::planar_reflection::{PlanarPreset, planar_preset};
use postretro_visibility::{MirrorVisibilityPath, VisibleCells};
use serde::{Deserialize, Deserializer, Serialize};

use super::scene::CaptureScene;
use crate::camera::RenderCamera;
use crate::render::Renderer;
use crate::render_preparation::{
    PlanarRenderPreparation, VisibleRenderPreparation, probe_bound_fluid_volumes,
};

/// A capture scene's `planar_reflection` block: overrides on the tier's
/// engine-constant planar preset, each optional, and the mirror crop switch.
/// `scale` is 0.25, 0.5 or 1; `d` and `r` are metres in 1..=100, or `null`
/// for unlimited; `band_d` and `band_r` are fade metres in 0.1..=16; `crop`
/// defaults true. There is no on switch: planar runs exactly when the
/// scene's reflection tier is very high or ultra, as in play.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct CapturePlanarReflection {
    pub(crate) scale: Option<f32>,
    #[serde(deserialize_with = "present")]
    pub(crate) d: Option<Option<f32>>,
    pub(crate) band_d: Option<f32>,
    #[serde(deserialize_with = "present")]
    pub(crate) r: Option<Option<f32>>,
    pub(crate) band_r: Option<f32>,
    pub(crate) crop: bool,
}

impl Default for CapturePlanarReflection {
    fn default() -> Self {
        Self {
            scale: None,
            d: None,
            band_d: None,
            r: None,
            band_r: None,
            crop: true,
        }
    }
}

/// A present field, `null` included: absent stays the tier's (`None`),
/// `null` is unlimited (`Some(None)`).
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<f32>>, D::Error> {
    Option::<f32>::deserialize(deserializer).map(Some)
}

impl CapturePlanarReflection {
    /// GPU-independent limits, checked at scene parse.
    pub(crate) fn validate(self) -> Result<(), &'static str> {
        if self
            .scale
            .is_some_and(|scale| ![0.25, 0.5, 1.0].contains(&scale))
        {
            return Err("scale must be 0.25, 0.5 or 1");
        }
        for distance in [self.d, self.r].into_iter().flatten().flatten() {
            if !distance.is_finite() || !(1.0..=100.0).contains(&distance) {
                return Err("d and r must be null (unlimited) or finite metres in 1..=100");
            }
        }
        for band in [self.band_d, self.band_r].into_iter().flatten() {
            if !band.is_finite() || !(0.1..=16.0).contains(&band) {
                return Err("band_d and band_r must be finite metres in 0.1..=16");
            }
        }
        Ok(())
    }

    /// `tier` with every field this block sets in place of its own.
    pub(crate) fn preset(self, tier: PlanarPreset) -> PlanarPreset {
        PlanarPreset {
            scale: self.scale.unwrap_or(tier.scale),
            d: self.d.unwrap_or(tier.d),
            band_d: self.band_d.unwrap_or(tier.band_d),
            r: self.r.unwrap_or(tier.r),
            band_r: self.band_r.unwrap_or(tier.band_r),
        }
    }
}

/// The planar state of one capture: the play frame's mirror step, from the
/// capture camera, at the scene's tier. Below very high it runs nothing and
/// every set it hands out is the main view's, so a capture there records,
/// uploads and reports exactly what it did without planar.
pub(super) struct CapturePlanar {
    render: PlanarRenderPreparation,
    preset: Option<PlanarPreset>,
    crop: bool,
}

impl CapturePlanar {
    /// Apply the scene's planar block over the kept tier's preset, then run
    /// the play frame's mirror step after the camera walk (`main`): install
    /// the level's candidates, select, crop, seed and walk, tag the mirror's
    /// light reach, and hand the renderer its mirror view. The light reach is
    /// handed over before each world frame instead (`mirror_light_cell_aabbs`),
    /// since each frame's light ranking consumes it. Call after the fluid
    /// install and before the first frame, preloads included.
    pub(super) fn prepare(
        world: &LevelWorld,
        renderer: &mut Renderer,
        scene: &CaptureScene,
        camera: &RenderCamera,
        main: &VisibleRenderPreparation,
    ) -> Self {
        let crop = scene.planar_reflection.is_none_or(|planar| planar.crop);
        let mut render = PlanarRenderPreparation::default();
        let preset = apply_scene_planar(renderer, scene);
        let Some(preset) = preset else {
            return Self {
                render,
                preset,
                crop,
            };
        };
        render.set_crop(crop);
        // Capture loads no manifest, so every probe-bound fluid volume
        // reflects (its surface look).
        render.install(world, &probe_bound_fluid_volumes(world));
        renderer.set_planar_candidate(render.has_candidate());
        render.prepare(world, camera, main, Some(preset), &[], &main.stats.cpu);
        renderer.set_planar_reflection_view(render.camera(), render.visible());
        Self {
            render,
            preset: Some(preset),
            crop,
        }
    }

    /// Bounds of the mirror-only light reach, which the renderer's light
    /// ranking consumes each world frame.
    pub(super) fn mirror_light_cell_aabbs(&self) -> &[(glam::Vec3, glam::Vec3)] {
        self.render.mirror_light_cell_aabbs()
    }

    /// Every drawable cell the capture draws: the main view's plus the mirror's.
    pub(super) fn drawn_cells<'a>(&'a self, main: &'a VisibleCells) -> &'a VisibleCells {
        self.render.drawn_cells(main)
    }

    /// The cells SH visible targeting reads (the play frame's gate).
    pub(super) fn sh_visible_cells<'a>(&'a self, main: &'a VisibleCells) -> &'a VisibleCells {
        self.render.sh_visible_cells(main)
    }

    /// SH sample regions' fog cells, keeping the unisolated sentinel.
    pub(super) fn sample_fog_cells<'a>(&'a self, main: &'a [u32]) -> &'a [u32] {
        self.render.sample_fog_cells(main)
    }

    /// The mirror's visible cells for mesh collection; `None` with no mirror.
    pub(super) fn mirror_cells(&self) -> Option<&VisibleCells> {
        self.render.camera().map(|_| self.render.visible())
    }

    /// The report's planar block; `None` where planar did not run.
    pub(super) fn report(&self, main: &VisibleCells) -> Option<CapturePlanarReport> {
        let preset = self.preset?;
        Some(CapturePlanarReport {
            scale: preset.scale,
            d: preset.d,
            band_d: preset.band_d,
            r: preset.r,
            band_r: preset.band_r,
            crop: self.crop,
            selected_plane_height: self.render.plane().map(|plane| plane.h),
            mirror_path: self.render.path().map(path_label),
            mirror_visible_cells: match self.render.visible() {
                VisibleCells::Culled(cells) if self.render.camera().is_some() => cells.len(),
                _ => 0,
            },
            mirror_reach_cells: self.render.reach().len(),
            mirror_only_cells: self.render.mirror_only_cells(main),
        })
    }
}

/// The capture report's planar block: the preset and crop the capture ran,
/// the selected plane's height (`None` with no plane selected), how the
/// mirror's visibility formed, and its cell counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct CapturePlanarReport {
    pub scale: f32,
    pub d: Option<f32>,
    pub band_d: f32,
    pub r: Option<f32>,
    pub band_r: f32,
    pub crop: bool,
    pub selected_plane_height: Option<f32>,
    pub mirror_path: Option<String>,
    pub mirror_visible_cells: usize,
    pub mirror_reach_cells: usize,
    pub mirror_only_cells: usize,
}

fn path_label(path: MirrorVisibilityPath) -> String {
    match path {
        MirrorVisibilityPath::Walked => "walked".into(),
        MirrorVisibilityPath::Tripped { considered } => format!("tripped after {considered}"),
        MirrorVisibilityPath::NoSeeds => "no_seeds".into(),
        MirrorVisibilityPath::NoPortals => "no_portals".into(),
    }
}

/// The scene's planar block over the kept tier's preset, then the preset the
/// capture draws by: `None` below very high, where a block is ignored with a
/// warning and nothing planar is touched. A capture's planar controls are
/// its scene alone: with dev-tools it also clears the diagnostics panel's
/// launch seed and planar-off switch.
fn apply_scene_planar(renderer: &mut Renderer, scene: &CaptureScene) -> Option<PlanarPreset> {
    let quality = renderer.reflection_quality();
    let Some(tier) = planar_preset(quality) else {
        if scene.planar_reflection.is_some() {
            log::warn!(
                "[Capture] planar_reflection ignored: reflection tier {quality:?} runs no planar \
                 layer"
            );
        }
        return None;
    };
    #[cfg(feature = "dev-tools")]
    renderer.set_planar_dev_off(false);
    let applied = renderer.set_planar_preset_override(
        scene
            .planar_reflection
            .map(|planar| planar.preset(tier).sanitized()),
    );
    debug_assert!(applied, "a planar tier takes an override");
    let preset = renderer.planar_reflection_preset();
    log::info!(
        "[Capture] Planar reflection: {quality:?}, {preset:?}, crop {}",
        scene.planar_reflection.is_none_or(|planar| planar.crop)
    );
    preset
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::scene::parse_scene;
    use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
    use serde_json::json;

    fn scene(value: serde_json::Value) -> String {
        json!({"map":"test.prl", "camera":{"position":[0,2,0],"yaw_deg":0,"pitch_deg":0},
            "resolution":[960,540],"output":"test.png","reflection_quality":"very_high",
            "planar_reflection":value})
        .to_string()
    }

    fn very_high() -> PlanarPreset {
        planar_preset(FluidReflectionQuality::VeryHigh).expect("very high runs planar")
    }

    // An empty block keeps the tier's preset and crops; explicit knobs
    // replace their fields, `null` lifts a tier's distance limit, and an
    // absent block parses to none. No knob switches planar on or off: the
    // tier does.
    #[test]
    fn planar_capture_defaults_and_explicit_knobs_reach_settings() {
        let empty = parse_scene(&scene(json!({}))).unwrap();
        let planar = empty.planar_reflection.unwrap();
        assert!(planar.crop);
        assert_eq!(planar.preset(very_high()), very_high());
        let explicit = parse_scene(&scene(json!({"scale":0.25,"d":15,"r":8,
            "band_d":2,"band_r":1,"crop":false})))
        .unwrap()
        .planar_reflection
        .unwrap();
        assert!(!explicit.crop);
        assert_eq!(
            explicit.preset(very_high()),
            PlanarPreset {
                scale: 0.25,
                d: Some(15.0),
                band_d: 2.0,
                r: Some(8.0),
                band_r: 1.0,
            }
        );
        let limited = PlanarPreset {
            d: Some(20.0),
            r: Some(10.0),
            ..very_high()
        };
        let unlimited = parse_scene(&scene(json!({"d":null})))
            .unwrap()
            .planar_reflection
            .unwrap();
        assert_eq!(unlimited.preset(limited).d, None);
        assert_eq!(unlimited.preset(limited).r, Some(10.0));
        let absent = json!({"map":"test.prl",
            "camera":{"position":[0,2,0],"yaw_deg":0,"pitch_deg":0},
            "resolution":[960,540],"output":"test.png"})
        .to_string();
        assert_eq!(parse_scene(&absent).unwrap().planar_reflection, None);
    }

    #[test]
    fn planar_capture_rejects_invalid_knobs_before_gpu_initialization() {
        for value in [
            json!({"scale":0.3}),
            json!({"d":0}),
            json!({"r":101}),
            json!({"d":"off"}),
            json!({"band_d":0}),
            json!({"band_r":17}),
            json!({"on":true}),
            json!({"typo":1}),
        ] {
            assert!(parse_scene(&scene(value.clone())).is_err(), "{value}");
        }
    }
}
