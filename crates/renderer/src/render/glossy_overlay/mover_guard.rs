// The overlay's coplanar-mover guard: the frame's movers mark D_static's alpha
// where they shaded, and the overlay adds no term on a marked pixel.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces)

use crate::render::DEPTH_FORMAT;
use crate::render::frame_timing::FrameTiming;
use crate::render::kinematic_brush::{KinematicBrushPass, KinematicLevelState};
use crate::render::pipeline_layout::TIMING_PAIR_GLOSSY_MOVER_GUARD;

/// The guard's fragment: alpha 1, written through an alpha-only mask so the
/// static diffuse forward wrote in RGB stays as it is.
pub(crate) const GLOSSY_MOVER_GUARD_SHADER: &str = "\
// Marks a mover-covered pixel in D_static's alpha (glossy_overlay.wgsl,
// `glossy_overlay_mover_covered`).
@fragment
fn fs_mover_guard() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}
";

/// The guard pipeline: the mover pass's own clip path (`vs_depth`), `Equal`
/// against scene depth, so it lands on exactly the pixels the mover colour
/// pass shaded, a mover face coplanar with a glossy world face included.
///
/// D_static carries the mark in its alpha, which forward clears to 0 and
/// writes as 0 every frame it writes D_static, before the mover pass: the
/// mark is this frame's movers alone, with no history. A pixel a mover covers
/// in front of a glossy face is marked too; the overlay's `Equal` already
/// refuses it.
pub(crate) struct GlossyMoverGuard {
    pipeline: wgpu::RenderPipeline,
    /// Guard passes recorded so far; tests read it.
    #[cfg(test)]
    recorded: std::cell::Cell<u32>,
}

impl GlossyMoverGuard {
    pub(crate) fn new(
        device: &wgpu::Device,
        movers: &KinematicBrushPass,
        static_diffuse_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Glossy Mover Guard Shader"),
            source: wgpu::ShaderSource::Wgsl(GLOSSY_MOVER_GUARD_SHADER.into()),
        });
        let pipeline = movers.coverage_pipeline(
            device,
            "Glossy Mover Guard Pipeline",
            DEPTH_FORMAT,
            wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_mover_guard"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: static_diffuse_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALPHA,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
        );
        Self {
            pipeline,
            #[cfg(test)]
            recorded: std::cell::Cell::new(0),
        }
    }

    /// Whether this frame records the guard: a main-view mover draws. The
    /// caller asks before it opens the guard's CPU stage, so a frame with no
    /// guard reports the stage absent.
    pub(crate) fn records(movers: &KinematicBrushPass, level: &KinematicLevelState) -> bool {
        movers.has_draws(level)
    }

    /// Mark this frame's main-view movers in `static_diffuse`, after forward
    /// wrote it and before the overlay reads it, on a frame the overlay draws
    /// and [`Self::records`] holds. Its own pass, timed as its own pair:
    /// D_static loaded and stored, scene depth read-only.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        static_diffuse: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        camera: &wgpu::BindGroup,
        movers: &KinematicBrushPass,
        level: &KinematicLevelState,
        timing: Option<&FrameTiming>,
    ) {
        if !Self::records(movers, level) {
            return;
        }
        #[cfg(test)]
        self.recorded.set(self.recorded.get() + 1);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Glossy Mover Guard Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: static_diffuse,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: None,
                stencil_ops: None,
            }),
            timestamp_writes: timing
                .map(|timing| timing.render_pass_writes(TIMING_PAIR_GLOSSY_MOVER_GUARD)),
            ..Default::default()
        });
        pass.set_bind_group(0, camera, &[]);
        movers.record_coverage(&mut pass, level, &self.pipeline);
    }

    /// Guard passes recorded so far.
    #[cfg(test)]
    pub(crate) fn recorded(&self) -> u32 {
        self.recorded.get()
    }
}
