// The frame's glossy floor layer, source half: its gate, the highlight-free
// colour the opaque passes attach, and the scene-colour chain recorded after
// the coplanar-mover guard and before the overlay.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors)

use super::*;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;

impl Renderer {
    /// Whether this frame runs the floor layer under `frame_light_term_mask`
    /// and `frame_reflection_quality`, the frame's one mask snapshot and
    /// tier: the tier runs it, the active level's floor targets exist (an
    /// up-facing glossy face at that tier), and the frame records the
    /// overlay. Forward, the mover pass and the skinned-mesh pass write the
    /// highlight-free colour, and the chain records, on exactly these frames;
    /// on every other frame none of them pays for it.
    pub(super) fn glossy_floor_layer_draws(
        &self,
        render_world: bool,
        frame_light_term_mask: LightTermMask,
        frame_reflection_quality: FluidReflectionQuality,
    ) -> bool {
        frame_reflection_quality.runs_floor_layer()
            && self.glossy_floor_targets().is_some()
            && self.glossy_overlay_probe_term_draws(
                render_world,
                frame_light_term_mask,
                frame_reflection_quality,
            )
    }

    /// The active level's floor-layer targets, while allocated.
    pub(super) fn glossy_floor_targets(&self) -> Option<&glossy_floor::GlossyFloorTargets> {
        self.full().level.as_ref()?.glossy.as_ref()?.floor()
    }

    /// The highlight-free colour an opaque pass attaches on a frame that
    /// runs the floor layer (`floor_layer`), else `None`: that pass then
    /// keeps its one-output form.
    pub(super) fn glossy_highlight_free_view(
        &self,
        floor_layer: bool,
    ) -> Option<&wgpu::TextureView> {
        self.glossy_floor_targets()
            .filter(|_| floor_layer)
            .map(|targets| targets.highlight_free_view())
    }

    /// Record the scene-colour chain from this frame's highlight-free colour,
    /// on a frame that runs the floor layer: after every opaque pass that
    /// writes it and the coplanar-mover guard, before the overlay adds a
    /// probe term to scene colour.
    pub(super) fn record_glossy_scene_colour_chain(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        floor_layer: bool,
    ) {
        let Some(targets) = self.glossy_floor_targets().filter(|_| floor_layer) else {
            return;
        };
        let full = self.full();
        // Encoder-level timestamps: the chain is one pass per level.
        if let Some(timing) = &full.frame_timing {
            timing.write_encoder_start(encoder, TIMING_PAIR_GLOSSY_SCENE_COLOUR_CHAIN);
        }
        full.glossy_overlay.floor().record_chain(encoder, targets);
        if let Some(timing) = &full.frame_timing {
            timing.write_encoder_end(encoder, TIMING_PAIR_GLOSSY_SCENE_COLOUR_CHAIN);
        }
    }
}
