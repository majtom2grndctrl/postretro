// The glossy world overlay pass: its pipelines (one per reflection style) over
// its own layout, forward's static-diffuse forms, the coplanar-mover guard,
// the floor layer's pass, and the frame's draw gate.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces), §10

mod level;
mod mover_guard;
mod pipeline;

pub use level::GLOSSY_STATIC_DIFFUSE;
pub(crate) use level::{GlossyLevelLayout, GlossyLevelState};
#[cfg(test)]
pub(crate) use mover_guard::GLOSSY_MOVER_GUARD_SHADER;
pub(crate) use mover_guard::GlossyMoverGuard;
#[cfg(test)]
pub(crate) use pipeline::glossy_overlay_blend_state;
pub(crate) use pipeline::{
    glossy_overlay_inputs_layout_entries, glossy_overlay_probe_layout_entries,
};

use postretro_render_cpu::fluid_surface::reflection::reflection_mask_on;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;
use postretro_render_cpu::frame_uniforms::LightTermMask;

use super::forward_pipelines::ForwardStaticDiffusePipelines;
use super::glossy_floor::GlossyFloorPass;
use super::kinematic_brush::KinematicBrushPass;
use crate::compute_cull::{ComputeCullPipeline, SetTextureFn};
use pipeline::overlay_pipeline;

/// The overlay's shader: its own bindings and entries, the glossy surface
/// (the camera and world-material groups and the world vertex), its shading
/// inputs (the style's point, forward's march and normal map), the tint rule
/// forward's highlight reads, forward's Surface Depth march, the exponent →
/// mip function, then the probe term, hemisphere and Fresnel it reuses from
/// the fluid shader's modules, by name.
pub(crate) const GLOSSY_OVERLAY_SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/glossy_overlay.wgsl"),
    "\n",
    include_str!("../shaders/glossy_surface.wgsl"),
    "\n",
    include_str!("../shaders/glossy_overlay_shading.wgsl"),
    "\n",
    include_str!("../shaders/material_shading.wgsl"),
    "\n",
    include_str!("../shaders/surface_depth.wgsl"),
    "\n",
    include_str!("../shaders/reflection_probe_blur.wgsl"),
    "\n",
    include_str!("../shaders/reflection_probe_lobes.wgsl"),
    "\n",
    include_str!("../shaders/fluid_motion.wgsl"),
    "\n",
    include_str!("../shaders/fluid_reflection_ray.wgsl"),
    "\n",
    include_str!("../shaders/fluid_reflection.wgsl"),
    "\n",
);

/// The overlay's groups: 0 the shared camera group, 1 the face's world
/// material (forward's group 1 layout and bind groups, so a material's live
/// uniform reaches both), 2 the level's probe records, 3 the frame's inputs
/// (forward's static diffuse and a sampler). Forward is at 16/16 sampled
/// textures, so the overlay never binds through forward's whole layout.
pub(crate) const GLOSSY_OVERLAY_MATERIAL_GROUP: u32 = 1;
pub(crate) const GLOSSY_OVERLAY_PROBE_GROUP: u32 = 2;
pub(crate) const GLOSSY_OVERLAY_INPUTS_GROUP: u32 = 3;
#[cfg(test)]
pub(crate) const GLOSSY_OVERLAY_BIND_GROUP_COUNT: usize = 4;

/// Whether the frame records the overlay: a tier above off, every bit of
/// the all-terms mask on, a level whose glossy faces are ready to draw (probe
/// records bound and D_static allocated), and a camera range over a glossy
/// bucket. The same condition puts the overlay among the sums' readers.
pub(crate) fn glossy_overlay_draws(
    quality: FluidReflectionQuality,
    light_term_mask: LightTermMask,
    level_ready: bool,
    glossy_range_drawn: bool,
) -> bool {
    quality != FluidReflectionQuality::Off
        && reflection_mask_on(light_term_mask.bits())
        && level_ready
        && glossy_range_drawn
}

/// Which overlay shading a glossy bucket's material draws through. Each form
/// draws exactly what the full one would for the materials it takes, with
/// less work: the march-free forms skip registers and instructions the march
/// holds, and the plain form reads the neutral normal placeholder's one texel
/// where the others filter it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlossyOverlayForm {
    /// The material can march at some Surface Depth state
    /// (`MaterialUniformPlan::can_march`): forward's march and normal map.
    March,
    /// It never marches, and samples its own normal map.
    NormalMapped,
    /// It never marches, and binds the neutral normal placeholder.
    Plain,
}

impl GlossyOverlayForm {
    /// Every form, in the order the overlay draws them.
    pub(crate) const ALL: [Self; 3] = [Self::Plain, Self::NormalMapped, Self::March];

    /// The form of a material that can march (`can_march`) and binds its
    /// own normal map (`normal_mapped`).
    pub(crate) fn of(can_march: bool, normal_mapped: bool) -> Self {
        match (can_march, normal_mapped) {
            (true, _) => Self::March,
            (false, true) => Self::NormalMapped,
            (false, false) => Self::Plain,
        }
    }

    /// The fragment entry drawing this form in `style`.
    fn entry(self, style: FluidReflectionStyle) -> &'static str {
        match (style, self) {
            (FluidReflectionStyle::Pixel, Self::March) => "fs_pixel",
            (FluidReflectionStyle::Smooth, Self::March) => "fs_smooth",
            (FluidReflectionStyle::Pixel, Self::NormalMapped) => "fs_pixel_flat",
            (FluidReflectionStyle::Smooth, Self::NormalMapped) => "fs_smooth_flat",
            (FluidReflectionStyle::Pixel, Self::Plain) => "fs_pixel_plain",
            (FluidReflectionStyle::Smooth, Self::Plain) => "fs_smooth_plain",
        }
    }
}

/// The overlay's pipelines, one per `render.reflections.style` and
/// [`GlossyOverlayForm`], all built at full init so a style change compiles
/// nothing.
pub(crate) struct GlossyOverlayPipelines {
    /// Indexed `[style][form]`, styles pixel then smooth, forms as
    /// [`GlossyOverlayForm::ALL`].
    pipelines: [[wgpu::RenderPipeline; 3]; 2],
}

impl GlossyOverlayPipelines {
    const STYLES: [FluidReflectionStyle; 2] =
        [FluidReflectionStyle::Pixel, FluidReflectionStyle::Smooth];

    fn new(build: impl Fn(&str) -> wgpu::RenderPipeline) -> Self {
        Self {
            pipelines: Self::STYLES
                .map(|style| GlossyOverlayForm::ALL.map(|form| build(form.entry(style)))),
        }
    }

    fn for_style(
        &self,
        style: FluidReflectionStyle,
        form: GlossyOverlayForm,
    ) -> &wgpu::RenderPipeline {
        let style = match style {
            FluidReflectionStyle::Pixel => 0,
            FluidReflectionStyle::Smooth => 1,
        };
        let form = match form {
            GlossyOverlayForm::Plain => 0,
            GlossyOverlayForm::NormalMapped => 1,
            GlossyOverlayForm::March => 2,
        };
        &self.pipelines[style][form]
    }

    /// `pipeline` in every style's and form's place: a test-only variant.
    #[cfg(test)]
    pub(crate) fn both(pipeline: wgpu::RenderPipeline) -> Self {
        Self::new(|_| pipeline.clone())
    }
}

/// The overlay pass, built at full init: its pipelines, its two own layouts,
/// the probe sampler, forward's static-diffuse forms, the coplanar-mover
/// guard and the floor layer's pass. Level state (the probe stream, the probe
/// group, the D_static target and the floor layer's targets) belongs to the
/// level set.
pub(crate) struct GlossyOverlayPass {
    pipelines: GlossyOverlayPipelines,
    guard: GlossyMoverGuard,
    floor: GlossyFloorPass,
    #[cfg(test)]
    pipeline_layout: wgpu::PipelineLayout,
    level_layout: GlossyLevelLayout,
    forward: ForwardStaticDiffusePipelines,
    /// Overlay passes recorded so far; tests read it.
    #[cfg(test)]
    recorded: std::cell::Cell<u32>,
}

impl GlossyOverlayPass {
    pub(crate) fn new(
        device: &wgpu::Device,
        uniform_layout: &wgpu::BindGroupLayout,
        material_layout: &wgpu::BindGroupLayout,
        forward: ForwardStaticDiffusePipelines,
        movers: &KinematicBrushPass,
    ) -> Self {
        let probe_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Glossy Overlay Probe BGL"),
            entries: &glossy_overlay_probe_layout_entries(),
        });
        let inputs_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Glossy Overlay Inputs BGL"),
            entries: &glossy_overlay_inputs_layout_entries(),
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Glossy Overlay Probe Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Glossy Overlay Pipeline Layout"),
            bind_group_layouts: &[
                Some(uniform_layout),
                Some(material_layout),
                Some(&probe_layout),
                Some(&inputs_layout),
            ],
            immediate_size: 0,
        });
        let pipelines = GlossyOverlayPipelines::new(|entry| {
            overlay_pipeline(
                device,
                &pipeline_layout,
                GLOSSY_OVERLAY_SHADER_SOURCE,
                entry,
            )
        });
        let guard = GlossyMoverGuard::new(device, movers, forward.format);
        let floor = GlossyFloorPass::new(device, forward.highlight_free_format);
        let level_layout = GlossyLevelLayout {
            probe_layout,
            inputs_layout,
            sampler,
            static_diffuse_format: forward.format,
            floor: floor.level_layout(),
        };
        Self {
            pipelines,
            guard,
            floor,
            #[cfg(test)]
            pipeline_layout,
            level_layout,
            forward,
            #[cfg(test)]
            recorded: std::cell::Cell::new(0),
        }
    }

    /// What a level build binds from the pass, cloned for any thread.
    pub(crate) fn level_layout(&self) -> GlossyLevelLayout {
        self.level_layout.clone()
    }

    /// Forward's two-output forms: glossy buckets write D_static, every
    /// other bucket masks it.
    pub(crate) fn forward_forms(&self) -> &ForwardStaticDiffusePipelines {
        &self.forward
    }

    /// The coplanar-mover guard, recorded ahead of the overlay.
    pub(crate) fn guard(&self) -> &GlossyMoverGuard {
        &self.guard
    }

    /// The floor layer's pass: its scene-colour chain, recorded after the
    /// guard and ahead of the overlay.
    pub(crate) fn floor(&self) -> &GlossyFloorPass {
        &self.floor
    }

    /// Record the overlay's draws over the frame's glossy camera ranges in
    /// `style`, the renderer's committed reflection style this frame. The
    /// caller opens the pass (scene colour loaded, depth read-only) and binds
    /// group 0; `glossy` names the glossy buckets, `form` each bucket's
    /// [`GlossyOverlayForm`], which picks its pipeline, and `bind_material`
    /// binds a bucket's world material at [`GLOSSY_OVERLAY_MATERIAL_GROUP`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        style: FluidReflectionStyle,
        state: &'a GlossyLevelState,
        vertex_buffer: &'a wgpu::Buffer,
        index_buffer: &'a wgpu::Buffer,
        cull: &'a ComputeCullPipeline,
        glossy: &dyn Fn(u32) -> bool,
        form: &dyn Fn(u32) -> GlossyOverlayForm,
        bind_material: &SetTextureFn<'a>,
    ) {
        let (Some(probe_group), Some(inputs_group)) = (state.probe_group(), state.inputs_group())
        else {
            return;
        };
        #[cfg(test)]
        self.recorded.set(self.recorded.get() + 1);
        pass.set_bind_group(GLOSSY_OVERLAY_PROBE_GROUP, probe_group, &[]);
        pass.set_bind_group(GLOSSY_OVERLAY_INPUTS_GROUP, inputs_group, &[]);
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        pass.set_vertex_buffer(1, state.world_probes().slice(..));
        pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        // One sweep per form. The blend is additive and a pixel takes one
        // glossy face's fragment, so the split changes no sum: only the order
        // two overlapping coplanar faces of different materials add in.
        for drawn in GlossyOverlayForm::ALL {
            let keep = |bucket: u32| glossy(bucket) && form(bucket) == drawn;
            if !cull.draws_bucket(keep) {
                continue;
            }
            pass.set_pipeline(self.pipelines.for_style(style, drawn));
            cull.draw_indirect_subset(pass, Some(bind_material), &keep);
        }
    }

    /// Overlay passes recorded so far.
    #[cfg(test)]
    pub(crate) fn recorded(&self) -> u32 {
        self.recorded.get()
    }

    /// An overlay pipeline over the production layout whose fragment entry
    /// `entry` comes from `extra` appended to the production shader: the
    /// test-only variants (a constant per fragment, R forced to D_static).
    #[cfg(test)]
    pub(crate) fn variant_pipeline(
        &self,
        device: &wgpu::Device,
        extra: &str,
        entry: &str,
    ) -> wgpu::RenderPipeline {
        let source = format!("{GLOSSY_OVERLAY_SHADER_SOURCE}\n{extra}\n");
        overlay_pipeline(device, &self.pipeline_layout, &source, entry)
    }

    /// Put `forms` in forward's static-diffuse forms' place, returning them.
    #[cfg(test)]
    pub(crate) fn swap_forward_forms(
        &mut self,
        forms: ForwardStaticDiffusePipelines,
    ) -> ForwardStaticDiffusePipelines {
        std::mem::replace(&mut self.forward, forms)
    }

    /// Put `pipelines` in the production pipelines' place, returning them.
    #[cfg(test)]
    pub(crate) fn swap_pipelines(
        &mut self,
        pipelines: GlossyOverlayPipelines,
    ) -> GlossyOverlayPipelines {
        std::mem::replace(&mut self.pipelines, pipelines)
    }
}

#[cfg(test)]
#[path = "glossy_overlay_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "glossy_overlay_mip_tests.rs"]
mod mip_tests;

#[cfg(test)]
#[path = "glossy_overlay_gate_tests.rs"]
mod gate_tests;

#[cfg(test)]
#[path = "glossy_overlay_register_screen_test.rs"]
mod register_screen_test;
