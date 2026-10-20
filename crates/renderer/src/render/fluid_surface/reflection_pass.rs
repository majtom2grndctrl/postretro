// Fluid reflection pass: each reflecting fluid fragment's ray marched across the
// frame's opaque depth into the reflection target, before the fluid pass
// composites it over the probe term; the target's lifetime and group 5 binding.
// See: context/lib/rendering_pipeline.md §7.3a, §10

use bytemuck::{Pod, Zeroable};
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;

// The parent's imports included: the reflection pass reads the installed
// level's buffers and shares its vertex streams and primitive state.
use super::*;

const REFLECTION_SHADER_SOURCE: &str = concat!(
    include_str!("../../shaders/fluid_reflection_pass.wgsl"),
    "\n",
    include_str!("../../shaders/fluid_motion.wgsl"),
    "\n",
    include_str!("../../shaders/fluid_reflection_ray.wgsl"),
    "\n",
    include_str!("../../shaders/screen_march.wgsl"),
    "\n",
);

/// Pipeline group of the pass's own inputs: scene depth, scene colour and
/// the reflection uniform.
pub(crate) const FLUID_REFLECTION_INPUTS_GROUP: u32 = 2;
/// Pipeline group of the fluid pass's group 4 buffers this pass reads: the
/// parameter window and the probe table, never the probe arrays.
pub(crate) const FLUID_REFLECTION_PROBE_GROUP: u32 = 3;
/// Groups the reflection pipeline layout composes, in order: 0 camera, 1
/// world material, 2 the pass's inputs, 3 the parameter window and probe
/// table.
#[cfg(test)]
pub(crate) const FLUID_REFLECTION_BIND_GROUP_COUNT: usize = 4;

/// The target holds premultiplied (c × SSR, c): bilinear filtering then
/// interpolates both linearly, so a footprint straddling a written and a
/// cleared texel lands between the probe-only and the SSR composite.
pub(crate) const FLUID_REFLECTION_TARGET_FORMAT: wgpu::TextureFormat =
    wgpu::TextureFormat::Rgba16Float;
/// March steps per ray, and metres of view depth the near end of a step's
/// view-depth interval may lie behind the depth buffer for a hit. Dev tuning
/// may override both for the session.
const FLUID_REFLECTION_DEFAULT_STEP_BUDGET: u32 = 64;
const FLUID_REFLECTION_DEFAULT_THICKNESS: f32 = 0.5;
/// Scene rows per full-resolution pixel of march stride: 1 below 1080 rows,
/// 2 at 1080p, 4 at 2160p, so a ray's reach covers about the same share of
/// the screen at every resolution, and every target extent marches the same
/// stride. Dev tuning may override it for the session.
const FLUID_REFLECTION_ROWS_PER_STRIDE_PIXEL: u32 = 540;

/// The march's dev tuning (the diagnostics panel's "SDF / Fog Quality"
/// section), never player-facing: steps per ray, the hit band's thickness,
/// and scene rows per pixel of stride. Every tier marches the same values.
/// Changing one writes the reflection uniform on its next upload and nothing
/// else: no bind group, target or pipeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FluidReflectionMarchTuning {
    pub step_budget: u32,
    /// Metres the near end of a step's view-depth interval may lie behind the
    /// depth buffer for a hit. The band bounds the interval's nearer end: the
    /// previous sample for a receding ray, the current one for an approaching
    /// ray.
    pub thickness: f32,
    pub rows_per_stride_pixel: u32,
}

impl Default for FluidReflectionMarchTuning {
    fn default() -> Self {
        Self {
            step_budget: FLUID_REFLECTION_DEFAULT_STEP_BUDGET,
            thickness: FLUID_REFLECTION_DEFAULT_THICKNESS,
            rows_per_stride_pixel: FLUID_REFLECTION_ROWS_PER_STRIDE_PIXEL,
        }
    }
}

impl FluidReflectionMarchTuning {
    /// At least one step and one row per stride pixel, and a finite,
    /// non-negative thickness; a non-finite thickness keeps `current`'s.
    fn sanitized(self, current: Self) -> Self {
        Self {
            step_budget: self.step_budget.max(1),
            thickness: if self.thickness.is_finite() {
                self.thickness.max(0.0)
            } else {
                current.thickness
            },
            rows_per_stride_pixel: self.rows_per_stride_pixel.max(1),
        }
    }
}

/// The reflection uniform (group 2 binding 2), the shader's
/// `FluidReflectionInputs` member for member.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct FluidReflectionUniform {
    pub reflection_style: u32,
    pub step_budget: u32,
    pub thickness: f32,
    pub stride: f32,
    pub target_scale: [f32; 2],
    /// Padding to the uniform's 32 bytes; carries nothing.
    pub spare: [f32; 2],
}
pub(crate) const FLUID_REFLECTION_UNIFORM_BYTES: u64 =
    std::mem::size_of::<FluidReflectionUniform>() as u64;

/// Group 2: scene depth and scene colour, both read by `textureLoad` at full
/// resolution and neither attached by this pass, and the reflection uniform.
pub(crate) fn fluid_reflection_inputs_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 3]
{
    [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(FLUID_REFLECTION_UNIFORM_BYTES),
            },
            count: None,
        },
    ]
}

/// Group 3: the fluid pass's group 4 entries for the parameter window and the
/// probe table, at their own bindings, over the same buffers. The probe
/// colour and distance arrays stay out: the pass reads no capture.
pub(crate) fn fluid_reflection_probe_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 2]
{
    let [window, _probe_array, probe_table, _probe_distance] =
        fluid_params_bind_group_layout_entries();
    [window, probe_table]
}

/// Every BGL entry set the reflection pipeline layout composes, in group
/// order, from the builders the layout is created from.
#[cfg(test)]
pub(crate) fn fluid_reflection_pipeline_bind_group_layout_entries()
-> [Vec<wgpu::BindGroupLayoutEntry>; FLUID_REFLECTION_BIND_GROUP_COUNT] {
    [
        uniform_bind_group_layout_entries().to_vec(),
        material_bind_group_layout_entries().to_vec(),
        fluid_reflection_inputs_bind_group_layout_entries().to_vec(),
        fluid_reflection_probe_bind_group_layout_entries().to_vec(),
    ]
}

/// Tests read the target back to see which texels a frame wrote; the
/// production target carries no copy usage.
const TARGET_TEST_COPY_USAGE: wgpu::TextureUsages = if cfg!(test) {
    wgpu::TextureUsages::COPY_SRC
} else {
    wgpu::TextureUsages::empty()
};

/// The reflection target and the pass's own depth, at the target's extent.
/// The depth orders fluid against fluid, so the nearest surface owns a texel;
/// scene depth never occludes, so water hidden behind an object standing in
/// it is still shaded.
struct FluidReflectionTarget {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    extent: (u32, u32),
}

impl FluidReflectionTarget {
    fn new(device: &wgpu::Device, extent: (u32, u32)) -> Self {
        let size = wgpu::Extent3d {
            width: extent.0,
            height: extent.1,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Fluid Reflection Target"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FLUID_REFLECTION_TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | TARGET_TEST_COPY_USAGE,
            view_formats: &[],
        });
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Fluid Reflection Depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        Self {
            view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
            _texture: texture,
            depth_view: depth.create_view(&wgpu::TextureViewDescriptor::default()),
            _depth: depth,
            extent,
        }
    }
}

/// The reflection pipeline, its inputs, and the target with its placeholder.
/// The pipeline and placeholder exist from full init, at every tier; the
/// target exists only while a level with a probe-bound fluid face is
/// installed at a tier other than Off.
pub(crate) struct FluidReflectionPass {
    pipeline: wgpu::RenderPipeline,
    inputs_layout: wgpu::BindGroupLayout,
    probe_layout: wgpu::BindGroupLayout,
    /// Written only when the style, the march or the target scale changes.
    uniform_buffer: wgpu::Buffer,
    inputs_bind_group: wgpu::BindGroup,
    reflection_style: FluidReflectionStyle,
    tuning: FluidReflectionMarchTuning,
    /// The scene extent the inputs were last built over.
    scene_extent: (u32, u32),
    /// The player's tier: whether a target may exist, and its fraction of
    /// the scene extent. Kept here so every target creation, the scene-target
    /// rebuild's included, sizes by the current tier.
    quality: FluidReflectionQuality,
    /// The uniform as last written.
    uploaded: FluidReflectionUniform,
    /// 1×1, zero-initialised: c = 0 wherever group 5 binds it.
    _placeholder: wgpu::Texture,
    placeholder_view: wgpu::TextureView,
    target: Option<FluidReflectionTarget>,
    /// Target creations so far; tests read it.
    #[cfg(test)]
    target_builds: u32,
    /// Uniform writes so far; tests read it.
    #[cfg(test)]
    uploads: u32,
    /// Reflection passes recorded so far; tests read it. A cell, because the
    /// pass records through a shared borrow of the renderer.
    #[cfg(test)]
    pass_records: std::cell::Cell<u32>,
    /// The scene depth and colour the inputs were last built over; tests
    /// compare them with the current scene targets.
    #[cfg(test)]
    inputs_views: (wgpu::TextureView, wgpu::TextureView),
}

fn create_inputs_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    scene_depth: &wgpu::TextureView,
    scene_color: &wgpu::TextureView,
    uniform_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Fluid Reflection Inputs Bind Group (group 2)"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(scene_depth),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(scene_color),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniform_buffer.as_entire_binding(),
            },
        ],
    })
}

fn texture_extent(view: &wgpu::TextureView) -> (u32, u32) {
    let size = view.texture().size();
    (size.width, size.height)
}

impl FluidReflectionPass {
    /// Build the pipeline over the shared camera and world-material layouts,
    /// the inputs over the current scene depth and colour, and the
    /// placeholder, at full init and at every tier.
    pub fn new(
        device: &wgpu::Device,
        camera_bgl: &wgpu::BindGroupLayout,
        material_bgl: &wgpu::BindGroupLayout,
        scene_depth: &wgpu::TextureView,
        scene_color: &wgpu::TextureView,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Fluid Reflection Shader"),
            source: wgpu::ShaderSource::Wgsl(REFLECTION_SHADER_SOURCE.into()),
        });
        let inputs_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Fluid Reflection Inputs BGL (group 2)"),
            entries: &fluid_reflection_inputs_bind_group_layout_entries(),
        });
        let probe_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Fluid Reflection Probe Table BGL (group 3)"),
            entries: &fluid_reflection_probe_bind_group_layout_entries(),
        });
        // Same group order as `fluid_reflection_pipeline_bind_group_layout_entries`.
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Fluid Reflection Pipeline Layout"),
            bind_group_layouts: &[
                Some(camera_bgl),
                Some(material_bgl),
                Some(&inputs_layout),
                Some(&probe_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Fluid Reflection Pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_reflection"),
                buffers: &fluid_vertex_buffer_layouts(),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: fluid_primitive_state(),
            depth_stencil: Some(fluid_reflection_depth_stencil_state()),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_reflection"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FLUID_REFLECTION_TARGET_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let placeholder = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Fluid Reflection Target Placeholder"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FLUID_REFLECTION_TARGET_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let placeholder_view = placeholder.create_view(&wgpu::TextureViewDescriptor::default());
        let scene_extent = texture_extent(scene_color);
        let tuning = FluidReflectionMarchTuning::default();
        let initial = FluidReflectionUniform {
            reflection_style: FluidReflectionStyle::default().uniform_value(),
            step_budget: tuning.step_budget,
            thickness: tuning.thickness,
            stride: march_stride(scene_extent, tuning.rows_per_stride_pixel),
            target_scale: [0.0; 2],
            spare: [0.0; 2],
        };
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fluid Reflection Uniform"),
            contents: bytemuck::bytes_of(&initial),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let inputs_bind_group = create_inputs_bind_group(
            device,
            &inputs_layout,
            scene_depth,
            scene_color,
            &uniform_buffer,
        );
        Self {
            pipeline,
            inputs_layout,
            probe_layout,
            uniform_buffer,
            inputs_bind_group,
            reflection_style: FluidReflectionStyle::default(),
            tuning,
            scene_extent,
            quality: FluidReflectionQuality::default(),
            uploaded: initial,
            _placeholder: placeholder,
            placeholder_view,
            target: None,
            #[cfg(test)]
            target_builds: 0,
            #[cfg(test)]
            uploads: 0,
            #[cfg(test)]
            pass_records: std::cell::Cell::new(0),
            #[cfg(test)]
            inputs_views: (scene_depth.clone(), scene_color.clone()),
        }
    }

    /// Follow recreated scene targets: a live target is recreated first, at
    /// the kept tier's fraction of the new extent, then the inputs rebuild
    /// over the new depth and colour. Returns the superseded target: the
    /// caller rebuilds group 5 over the new one, then drops it, as
    /// `set_reflection_target` does.
    #[must_use]
    fn resize(
        &mut self,
        device: &wgpu::Device,
        scene_depth: &wgpu::TextureView,
        scene_color: &wgpu::TextureView,
    ) -> Option<FluidReflectionTarget> {
        self.scene_extent = texture_extent(scene_color);
        let superseded = self.target.take();
        if superseded.is_some()
            && let Some(extent) = self.tier_target_extent()
        {
            self.create_target(device, extent);
        }
        self.inputs_bind_group = create_inputs_bind_group(
            device,
            &self.inputs_layout,
            scene_depth,
            scene_color,
            &self.uniform_buffer,
        );
        #[cfg(test)]
        {
            self.inputs_views = (scene_depth.clone(), scene_color.clone());
        }
        superseded
    }

    /// The target extent the kept tier calls for at the current scene
    /// extent; `None` at Off.
    fn tier_target_extent(&self) -> Option<(u32, u32)> {
        self.quality.target_extent(self.scene_extent)
    }

    /// The live target's extent; `None` with only the placeholder.
    fn live_target_extent(&self) -> Option<(u32, u32)> {
        self.target.as_ref().map(|target| target.extent)
    }

    fn create_target(&mut self, device: &wgpu::Device, extent: (u32, u32)) {
        self.target = Some(FluidReflectionTarget::new(device, extent));
        #[cfg(test)]
        {
            self.target_builds += 1;
        }
    }

    /// What group 5 binds: the target, or the placeholder without one.
    pub(super) fn bound_view(&self) -> &wgpu::TextureView {
        self.target
            .as_ref()
            .map_or(&self.placeholder_view, |target| &target.view)
    }

    /// The target's extent over the scene's per axis; 0 without a target.
    fn target_scale(&self) -> [f32; 2] {
        self.target.as_ref().map_or([0.0; 2], |target| {
            [
                target.extent.0 as f32 / self.scene_extent.0.max(1) as f32,
                target.extent.1 as f32 / self.scene_extent.1.max(1) as f32,
            ]
        })
    }

    /// The uniform as the next upload writes it.
    fn pending(&self) -> FluidReflectionUniform {
        FluidReflectionUniform {
            reflection_style: self.reflection_style.uniform_value(),
            step_budget: self.tuning.step_budget,
            thickness: self.tuning.thickness,
            stride: march_stride(self.scene_extent, self.tuning.rows_per_stride_pixel),
            target_scale: self.target_scale(),
            spare: [0.0; 2],
        }
    }

    /// Write the uniform if the style, the march or the target scale
    /// changed since it last uploaded.
    fn upload(&mut self, queue: &crate::render::uploads::UploadQueue) {
        let uniform = self.pending();
        if uniform == self.uploaded {
            return;
        }
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));
        self.uploaded = uniform;
        #[cfg(test)]
        {
            self.uploads += 1;
        }
    }

    /// Group 3 over one level's parameter window and probe table, built at
    /// install for a level with a probe-bound face.
    fn probe_bind_group(
        &self,
        device: &wgpu::Device,
        params_buffer: &wgpu::Buffer,
        probe_table: wgpu::BindGroupEntry<'_>,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Fluid Reflection Probe Table Bind Group (group 3)"),
            layout: &self.probe_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: params_buffer,
                        offset: 0,
                        size: NonZeroU64::new(FLUID_PARAMS_WINDOW_BYTES),
                    }),
                },
                probe_table,
            ],
        })
    }
}

/// Full-resolution pixels per march step at `scene` extent: one per
/// `rows_per_stride_pixel` scene rows, at least one.
fn march_stride(scene: (u32, u32), rows_per_stride_pixel: u32) -> f32 {
    (scene.1 / rows_per_stride_pixel.max(1)).max(1) as f32
}

/// Depth tested and written against the pass's own depth, never scene depth:
/// the nearest fluid surface owns each texel, whatever order draws arrive in.
pub(crate) fn fluid_reflection_depth_stencil_state() -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(true),
        depth_compare: Some(wgpu::CompareFunction::Less),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

impl FluidSurfacePass {
    /// The reflection target extent the installed level and the kept tier
    /// call for: the tier's fraction of the scene extent on a level with a
    /// probe-bound fluid face, else none. Probe binding is fixed at install;
    /// reflectivity, retuned live, never enters it.
    ///
    /// The reflecting variant can still draw on a level with no target.
    /// `has_probe_bound_face` comes from the per-vertex probe indices, while
    /// render-cpu's `volume_has_bound_face`, which packs reflectivity and so
    /// picks the variant, comes from the probe bindings, and the two can
    /// disagree. That draw degrades safely: its vertices carry
    /// `FLUID_REFLECTION_NO_PROBE`, so the gate shuts, and the placeholder
    /// reads c = 0.
    pub(super) fn wanted_reflection_target(&self) -> Option<(u32, u32)> {
        let probe_bound = self
            .installed
            .as_ref()
            .is_some_and(InstalledFluidSurfaces::has_probe_bound_face);
        if !probe_bound {
            return None;
        }
        self.reflection.tier_target_extent()
    }

    /// Create, resize or release the reflection target to match `wanted`,
    /// its extent or none. Group 5 rebuilds around the change: over a new
    /// target once it exists, and over the placeholder before a released
    /// target drops, so no group 5 build references a dropped target. The
    /// extent it already has builds nothing.
    pub(super) fn set_reflection_target(
        &mut self,
        device: &wgpu::Device,
        wanted: Option<(u32, u32)>,
    ) {
        if wanted == self.reflection.live_target_extent() {
            return;
        }
        let released = self.reflection.target.take();
        if let Some(extent) = wanted {
            self.reflection.create_target(device, extent);
        }
        self.rebind_reflection_target(device);
        drop(released);
    }

    /// Apply the player's reflection tier. The tier it already holds builds
    /// nothing. Otherwise the target follows: created on a level with a
    /// probe-bound face when the tier leaves Off, resized when the fraction
    /// changes, released at Off, with group 5 and its scale rebuilt around
    /// it. With no such level it only keeps the tier for the next install.
    /// Creates no pipeline and no shader module: those are built at full
    /// init, at every tier.
    pub fn set_reflection_quality(
        &mut self,
        device: &wgpu::Device,
        quality: FluidReflectionQuality,
    ) {
        if self.reflection.quality == quality {
            return;
        }
        self.reflection.quality = quality;
        self.set_reflection_target(device, self.wanted_reflection_target());
    }

    /// Rebuild group 5 over the scene depth it holds and the current target
    /// or placeholder, with the matching scale.
    fn rebind_reflection_target(&mut self, device: &wgpu::Device) {
        let scene_depth = self.scene_inputs.bound_scene_depth().clone();
        self.scene_inputs
            .rebuild(device, &scene_depth, self.reflection.bound_view());
        self.scene_inputs
            .set_ssr_scale(self.reflection.target_scale());
    }

    /// Follow a recreated scene depth and colour, in order: a live target is
    /// recreated at the new extent, then the reflection inputs and group 5
    /// rebuild over the new resources, so none references a superseded one.
    /// The superseded target drops only after group 5 holds its successor.
    /// The scene-target rebuild is the only caller: the frame never builds a
    /// bind group.
    pub fn rebuild_scene_inputs(
        &mut self,
        device: &wgpu::Device,
        scene_depth: &wgpu::TextureView,
        scene_color: &wgpu::TextureView,
    ) {
        let superseded = self.reflection.resize(device, scene_depth, scene_color);
        self.scene_inputs
            .rebuild(device, scene_depth, self.reflection.bound_view());
        self.scene_inputs
            .set_ssr_scale(self.reflection.target_scale());
        drop(superseded);
    }

    /// Group 3 of the reflection pass for a level being installed.
    pub(super) fn reflection_probe_bind_group(
        &self,
        device: &wgpu::Device,
        params_buffer: &wgpu::Buffer,
        probe_table: wgpu::BindGroupEntry<'_>,
    ) -> wgpu::BindGroup {
        self.reflection
            .probe_bind_group(device, params_buffer, probe_table)
    }

    /// Forward the mod's reflection style to the reflection pass's uniform.
    pub(super) fn set_reflection_pass_style(&mut self, style: FluidReflectionStyle) {
        self.reflection.reflection_style = style;
    }

    /// The march's dev tuning as the next upload writes it.
    pub fn reflection_march_tuning(&self) -> FluidReflectionMarchTuning {
        self.reflection.tuning
    }

    /// Replace the march's dev tuning (sanitized: at least one step and one
    /// row per stride pixel, a finite non-negative thickness). It reaches
    /// the reflection uniform on the next upload, and only if it changed;
    /// it builds no bind group, target or pipeline.
    pub fn set_reflection_march_tuning(&mut self, tuning: FluidReflectionMarchTuning) {
        self.reflection.tuning = tuning.sanitized(self.reflection.tuning);
    }

    /// Write the reflection uniform if it changed; beside group 5's, so both
    /// passes of a frame draw one style.
    pub(super) fn upload_reflection_inputs(&mut self, queue: &crate::render::uploads::UploadQueue) {
        self.reflection.upload(queue);
    }

    /// Whether this frame records the reflection pass: a live target and a
    /// draw taking the reflecting variant under `light_term_mask` and
    /// `quality`, the frame's group-0 mask and tier, the ones the fluid pass
    /// picks variants by. Off, a level with no fluid, a frame with no
    /// reflecting draw and an isolating mask each record none.
    pub fn has_reflecting_draws(
        &self,
        light_term_mask: LightTermMask,
        quality: FluidReflectionQuality,
    ) -> bool {
        quality.reflects()
            && self.reflection.target.is_some()
            && self.installed.as_ref().is_some_and(|installed| {
                installed.reflection_probe_bind_group.is_some()
                    && installed
                        .slots
                        .frame_draws(light_term_mask.bits(), quality)
                        .any(|(_, variant)| variant.reflections())
            })
    }

    /// Record this frame's reflecting draws into the target. The caller binds
    /// group 0; this binds 1, 2 and 3, with the vertex streams, parameter
    /// windows and materials `record_draws` uses, and skips every
    /// reflection-free draw.
    pub fn record_reflection_draws(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        materials: &[GpuTexture],
        light_term_mask: LightTermMask,
        quality: FluidReflectionQuality,
    ) {
        let Some(installed) = self.installed.as_ref() else {
            return;
        };
        let Some(probe_bind_group) = installed.reflection_probe_bind_group.as_ref() else {
            return;
        };
        pass.set_pipeline(&self.reflection.pipeline);
        pass.set_bind_group(
            FLUID_REFLECTION_INPUTS_GROUP,
            &self.reflection.inputs_bind_group,
            &[],
        );
        pass.set_vertex_buffer(0, installed.vertex_buffer.slice(..));
        pass.set_vertex_buffer(1, installed.drift_buffer.slice(..));
        pass.set_vertex_buffer(2, installed.probe_index_buffer.slice(..));
        pass.set_index_buffer(installed.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

        let mut bound_window = None;
        let mut bound_material = None;
        for (slot, variant) in installed.slots.frame_draws(light_term_mask.bits(), quality) {
            if !variant.reflections() {
                continue;
            }
            let (window, element) = slot_window(slot);
            if bound_window != Some(window) {
                let offset = u64::from(window) * FLUID_PARAMS_WINDOW_BYTES;
                pass.set_bind_group(
                    FLUID_REFLECTION_PROBE_GROUP,
                    probe_bind_group,
                    &[offset as wgpu::DynamicOffset],
                );
                bound_window = Some(window);
            }
            for range in &installed.volumes[slot as usize].material_ranges {
                if bound_material != Some(range.material_index) {
                    let material = materials
                        .get(range.material_index)
                        .or_else(|| materials.first())
                        .expect("renderer always keeps a placeholder material bind group");
                    pass.set_bind_group(1, &material.bind_group, &[]);
                    bound_material = Some(range.material_index);
                }
                pass.draw_indexed(
                    range.index_start..range.index_start + range.index_count,
                    0,
                    element..element + 1,
                );
            }
        }
    }

    /// The target and the pass's depth, while a target is live.
    fn reflection_attachments(&self) -> Option<(&wgpu::TextureView, &wgpu::TextureView)> {
        self.reflection
            .target
            .as_ref()
            .map(|target| (&target.view, &target.depth_view))
    }

    /// The live target's extent; `None` with only the placeholder.
    #[cfg(test)]
    pub(crate) fn reflection_target_extent(&self) -> Option<(u32, u32)> {
        self.reflection.live_target_extent()
    }

    /// Target creations since full init.
    #[cfg(test)]
    pub(crate) fn reflection_target_builds(&self) -> u32 {
        self.reflection.target_builds
    }

    /// Reflection passes recorded since full init.
    #[cfg(test)]
    pub(crate) fn reflection_pass_records(&self) -> u32 {
        self.reflection.pass_records.get()
    }

    /// The tier the pass keeps.
    #[cfg(test)]
    pub(crate) fn kept_reflection_quality(&self) -> FluidReflectionQuality {
        self.reflection.quality
    }

    /// What group 5's binding 2 should hold now: the live target's view, or
    /// the placeholder's.
    #[cfg(test)]
    pub(crate) fn reflection_bound_view(&self) -> &wgpu::TextureView {
        self.reflection.bound_view()
    }

    /// The reflection inputs' scene depth and colour as last built.
    #[cfg(test)]
    pub(crate) fn reflection_inputs_views(&self) -> &(wgpu::TextureView, wgpu::TextureView) {
        &self.reflection.inputs_views
    }

    /// The reflection uniform as the next upload writes it.
    #[cfg(test)]
    pub(crate) fn reflection_pending_uniform(&self) -> FluidReflectionUniform {
        self.reflection.pending()
    }

    /// The reflection uniform as last written, and the writes so far.
    #[cfg(test)]
    pub(crate) fn reflection_uploaded_uniform(&self) -> (FluidReflectionUniform, u32) {
        (self.reflection.uploaded, self.reflection.uploads)
    }
}

impl Renderer {
    /// The reflection pass, between skinned meshes and the fluid pass: the
    /// frame's reflecting fluid draws under `light_term_mask` and `quality`,
    /// the frame's one mask snapshot and one tier value, marched into
    /// the target, cleared to c = 0 first so a texel no draw writes reads as
    /// a miss. The frame records it only when `has_reflecting_draws`.
    pub(in crate::render) fn record_fluid_reflection_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        light_term_mask: LightTermMask,
        quality: FluidReflectionQuality,
    ) {
        let full = self.full();
        let fluid = &full.fluid_surface;
        let Some((target, depth)) = fluid.reflection_attachments() else {
            return;
        };
        #[cfg(test)]
        fluid
            .reflection
            .pass_records
            .set(fluid.reflection.pass_records.get() + 1);
        let timestamp_writes = full
            .frame_timing
            .as_ref()
            .map(|t| t.render_pass_writes(TIMING_PAIR_FLUID_REFLECTION));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Fluid Reflection Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes,
            ..Default::default()
        });
        pass.set_bind_group(0, &full.uniform_bind_group, &[]);
        fluid.record_reflection_draws(&mut pass, &full.gpu_textures, light_term_mask, quality);
    }
}

#[cfg(test)]
pub(crate) fn composed_reflection_shader_source() -> &'static str {
    REFLECTION_SHADER_SOURCE
}
