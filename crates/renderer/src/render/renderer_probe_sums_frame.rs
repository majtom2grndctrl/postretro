// The frame's live-lighting sums dispatch: once, ahead of every pass that
// draws a reflection probe term, on a frame with any such draw.
// See: context/lib/rendering_pipeline.md §7.3a (Live lighting)

use super::*;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;

/// Whether the frame dispatches the sums: it draws a probe term somewhere,
/// through the fluid pass's reflecting draws or the glossy overlay's.
fn frame_dispatches_probe_sums(
    fluid_probe_term_draws: bool,
    overlay_probe_term_draws: bool,
) -> bool {
    fluid_probe_term_draws || overlay_probe_term_draws
}

impl Renderer {
    /// Record the sums pass over the active level's probes, once, on a frame
    /// that draws a probe term under `frame_light_term_mask` and
    /// `frame_reflection_quality`, the frame's one mask snapshot and tier.
    /// The caller records it after the pre-scene compute and before world
    /// forward, so every later probe-term pass reads this frame's sums. The
    /// frame's descriptor flush (`update_per_frame_uniforms`) and the light
    /// bridge's sample writes are queue writes that land before this
    /// submission; no pass recorded after it writes a descriptor or sample
    /// buffer.
    pub(super) fn record_reflection_probe_sums(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        render_world: bool,
        frame_light_term_mask: LightTermMask,
        frame_reflection_quality: FluidReflectionQuality,
    ) {
        if !render_world {
            return;
        }
        let fluid_probe_term_draws = self.full().fluid_surface.has_probe_term_draws(
            self.fluid_surfaces(),
            frame_light_term_mask,
            frame_reflection_quality,
        );
        let overlay_probe_term_draws = self.glossy_overlay_probe_term_draws(
            render_world,
            frame_light_term_mask,
            frame_reflection_quality,
        );
        if !frame_dispatches_probe_sums(fluid_probe_term_draws, overlay_probe_term_draws) {
            return;
        }
        let Some(probes) = self.reflection_probes() else {
            return;
        };
        let full = self.full();
        full.reflection_probe_sums.record(
            encoder,
            full.frame_timing.as_ref(),
            &full.uniform_bind_group,
            self.level().sh_bind_group(),
            probes,
        );
    }
}
