// The frame's planar mirror step: after the camera walk, before any visibility consumer.
// See: context/lib/rendering_pipeline.md §7.3a

use postretro_render_cpu::planar_reflection::PlanarPreset;

use crate::App;
use crate::camera::RenderCamera;
use crate::render_preparation::VisibleRenderPreparation;

/// The camera walk (`main`) has run, and no consumer has read visibility
/// yet, so the main view's sets are the same with planar on and off. Read the
/// tier once, from the tier the renderer draws this frame with; then select,
/// seed from this frame's camera reach, walk the mirror on its own scratch,
/// and tag its mirror-only light reach. The walk's time lands in `main`'s
/// visibility stages, which the frame folds under `visibility`. Below very
/// high nothing runs.
pub(super) fn prepare_planar_mirror(
    app: &mut App,
    camera: &RenderCamera,
    main: &VisibleRenderPreparation,
) {
    let preset = app
        .renderer
        .as_ref()
        .and_then(|renderer| renderer.planar_reflection_preset());
    app.prepare_planar_mirror_at(camera, main, preset);
}

impl App {
    /// The mirror step at `preset`, the tier's planar preset read once this
    /// frame (`None` below very high). The play frame and Settling's held
    /// frame both run it after their camera walk, so a held pose selects,
    /// crops and walks exactly as the reveal frame at that pose does, and the
    /// settle set holds what the reveal frame's mirror draws.
    pub(crate) fn prepare_planar_mirror_at(
        &mut self,
        camera: &RenderCamera,
        main: &VisibleRenderPreparation,
        preset: Option<PlanarPreset>,
    ) {
        let Some(world) = self.level.as_ref() else {
            return;
        };
        // The diagnostics panel's crop switch; its planar-off switch already
        // reads as no preset.
        #[cfg(feature = "dev-tools")]
        if let Some(renderer) = self.renderer.as_ref() {
            self.planar_render.set_crop(renderer.planar_dev_crop());
        }
        self.planar_render.prepare(
            world,
            camera,
            main,
            preset,
            &self.blocked_portals,
            &main.stats.cpu,
        );
    }
}

#[cfg(test)]
#[path = "planar_tests.rs"]
mod tests;
