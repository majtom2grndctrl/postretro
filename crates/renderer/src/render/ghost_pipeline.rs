// Ghost draw vocabulary both ghost kinds share: the two steps of one ghost's
// draw pair, a skinned ghost's instance span, each step's fixed-function
// state per transparency mode, and the pipeline set built from them.
// See: context/lib/rendering_pipeline.md §7.9

use postretro_render_cpu::ghost_transparency::GhostTransparency;

/// Which half of one ghost's draw pair to record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GhostDrawStep {
    /// Colour writes off; depth written where nearer than everything drawn
    /// so far, so the target holds this ghost's nearest surface.
    Depth,
    /// Colour where the fragment equals the ghost depth; no depth write.
    Colour,
}

/// One skinned plan group's instances that belong to one ghost, as an
/// absolute instance-buffer range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SkinnedGhostSpan {
    pub(super) group: u32,
    pub(super) start: u32,
    pub(super) end: u32,
}

impl SkinnedGhostSpan {
    pub(super) fn instances(&self) -> std::ops::Range<u32> {
        self.start..self.end
    }
}

/// The fixed-function state one ghost pipeline draws with: a mode's colour
/// draw in [`ghost_pipeline_state`], its depth-only draw in
/// [`ghost_depth_pipeline_state`]. Pure data, so the mode → state mapping is
/// testable without a device.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct GhostPipelineState {
    pub(super) fragment_entry: &'static str,
    pub(super) blend: Option<wgpu::BlendState>,
    pub(super) write_mask: wgpu::ColorWrites,
    pub(super) depth: wgpu::DepthStencilState,
    pub(super) cull_mode: Option<wgpu::Face>,
}

/// Blend: straight alpha over the scene, the fluid surface's blend.
/// Dither: blend off; the fragment discards pixels the screen-door pattern
/// drops. Both test `Equal` against the depth the ghost's own depth-only
/// draw just wrote ([`ghost_depth_pipeline_state`]) and never write depth, so
/// each pixel shows only the ghost's nearest surface, scene depth stays
/// untouched and a ghost hides nothing drawn after it. `Equal` is exact: both
/// draws run one `@invariant` vertex stage over the same triangles. A ghost
/// face exactly coplanar with the opaque scene draws, and z-fights with it
/// either way. Both cull back faces like the skinned pass.
pub(super) fn ghost_pipeline_state(
    mode: GhostTransparency,
    depth_format: wgpu::TextureFormat,
) -> GhostPipelineState {
    let (fragment_entry, blend) = match mode {
        GhostTransparency::Blend => (
            "fs_blend",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
        ),
        GhostTransparency::Dither => ("fs_dither", None),
    };
    GhostPipelineState {
        fragment_entry,
        blend,
        write_mask: wgpu::ColorWrites::ALL,
        depth: wgpu::DepthStencilState {
            format: depth_format,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Equal),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        },
        cull_mode: Some(wgpu::Face::Back),
    }
}

/// A ghost's depth-only draw, run just before its colour draw in the same
/// pass: it writes each of the ghost's surfaces where it is nearer than the
/// opaque scene and everything drawn so far, so the target holds the ghost's
/// nearest surface per pixel when its colour draw tests `Equal`. Ghosts draw
/// farthest first, so a farther ghost's colour is already down where a
/// nearer one then writes over its depth. It targets the scene colour with
/// colour writes off, so it can share the colour draws' pass. It culls
/// exactly as the colour draw does, so both rasterize the same triangles.
///
/// Dither discards the screen door's dropped pixels here too: a pixel a near
/// ghost drops must not hide a farther ghost that keeps it. Within one ghost
/// the drop is the same for every surface at a pixel, so this changes nothing
/// there.
///
/// A ghost face exactly coplanar with the opaque scene fails `Less`, leaves
/// the seeded depth, and then passes the colour draw's `Equal`: it draws, and
/// z-fights with the scene either way.
pub(super) fn ghost_depth_pipeline_state(
    mode: GhostTransparency,
    depth_format: wgpu::TextureFormat,
) -> GhostPipelineState {
    GhostPipelineState {
        fragment_entry: match mode {
            GhostTransparency::Blend => "fs_blend_depth",
            GhostTransparency::Dither => "fs_dither_depth",
        },
        blend: None,
        write_mask: wgpu::ColorWrites::empty(),
        depth: wgpu::DepthStencilState {
            format: depth_format,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        },
        cull_mode: Some(wgpu::Face::Back),
    }
}

/// One ghost kind's colour and depth-only pipeline per transparency mode,
/// built once; a frame draws every ghost through the mode the mod's render
/// profile picks. Skinned and mover ghosts each hold one; each kind's module
/// supplies its own constructor over [`GhostPipelines::build`].
pub(super) struct GhostPipelines {
    blend: wgpu::RenderPipeline,
    dither: wgpu::RenderPipeline,
    depth_blend: wgpu::RenderPipeline,
    depth_dither: wgpu::RenderPipeline,
}

impl GhostPipelines {
    /// All four pipelines of one ghost `kind` (its label prefix): `create`
    /// builds one from its draw step, fixed-function state and label.
    pub(super) fn build(
        kind: &str,
        depth_format: wgpu::TextureFormat,
        mut create: impl FnMut(GhostDrawStep, GhostPipelineState, &str) -> wgpu::RenderPipeline,
    ) -> Self {
        let mut pipeline = |step: GhostDrawStep, mode: GhostTransparency| {
            let (state, step_label) = match step {
                GhostDrawStep::Depth => (ghost_depth_pipeline_state(mode, depth_format), " Depth"),
                GhostDrawStep::Colour => (ghost_pipeline_state(mode, depth_format), ""),
            };
            let mode_label = match mode {
                GhostTransparency::Blend => "Blend",
                GhostTransparency::Dither => "Dither",
            };
            create(
                step,
                state,
                &format!("{kind} Ghost {mode_label}{step_label} Pipeline"),
            )
        };
        Self {
            blend: pipeline(GhostDrawStep::Colour, GhostTransparency::Blend),
            dither: pipeline(GhostDrawStep::Colour, GhostTransparency::Dither),
            depth_blend: pipeline(GhostDrawStep::Depth, GhostTransparency::Blend),
            depth_dither: pipeline(GhostDrawStep::Depth, GhostTransparency::Dither),
        }
    }

    /// The pipeline that records `step` of a ghost's draw pair under `mode`.
    pub(super) fn for_step(
        &self,
        mode: GhostTransparency,
        step: GhostDrawStep,
    ) -> &wgpu::RenderPipeline {
        match (step, mode) {
            (GhostDrawStep::Depth, GhostTransparency::Blend) => &self.depth_blend,
            (GhostDrawStep::Depth, GhostTransparency::Dither) => &self.depth_dither,
            (GhostDrawStep::Colour, GhostTransparency::Blend) => &self.blend,
            (GhostDrawStep::Colour, GhostTransparency::Dither) => &self.dither,
        }
    }
}

#[cfg(test)]
#[path = "ghost_pipeline_tests.rs"]
mod tests;
