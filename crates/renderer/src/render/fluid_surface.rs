// Fluid surface pass: environment-volume surface faces drawn translucent,
// two-sided and moving in whole texels, after skinned meshes and before billboards.
// See: context/lib/rendering_pipeline.md §7.3a

use std::num::NonZeroU64;

use wgpu::util::DeviceExt;

use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_level_format::reflection_probes::ReflectionProbesSection;
use postretro_render_cpu::fluid_surface::{
    FLUID_DRIFT_RATE_STRIDE, FLUID_PARAMS_STRIDE, FLUID_PARAMS_WINDOW_BYTES,
    FLUID_PROBE_INDEX_STRIDE, FLUID_SURFACE_VERTEX_STRIDE, FluidFaceDriftMap, FluidShaderVariant,
    FluidSurfaceDraw, FluidSurfaceSlots, FluidSurfaceVolume, fill_drift_rates, pack_fluid_surfaces,
    slot_window,
};
use postretro_render_cpu::frame_uniforms::LightTermMask;

use super::fluid_reflection_cells::{
    FluidCellPipelines, FluidCellStatsGpu, FluidReflectionCellStats, InstalledFluidCells,
    cell_vertex_buffer_layout, fluid_cell_atlas_layout_entry, lever_constants,
    lever_pipeline_index,
};
use super::fluid_reflection_probes::{
    FluidProbeResources, create_probe_index_buffer, fluid_probe_bind_group_layout_entries,
    installable_probe_section,
};
use postretro_render_cpu::fluid_surface::reflection_cells::FluidReflectionCellLevers;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;

#[cfg(test)]
pub(crate) use super::fluid_scene_inputs::FLUID_SCENE_UNIFORM_BYTES;
#[cfg(any(test, debug_assertions))]
pub(crate) use super::fluid_scene_inputs::fluid_scene_inputs_bind_group_layout_entries;
pub(crate) use super::fluid_scene_inputs::{FLUID_SCENE_INPUTS_GROUP, FluidSceneInputs};

use super::*;

/// Pipeline group of the per-volume parameter window and the reflection probe
/// resources (probe array, share table, distance array).
pub(crate) const FLUID_PARAMS_GROUP: u32 = 4;
/// Groups the pipeline layout composes, in order: 0 camera, 1 world material,
/// 2 lighting, 3 SH volume, 4 per-volume parameters and reflection probes,
/// 5 scene inputs.
#[cfg(any(test, debug_assertions))]
pub(crate) const FLUID_BIND_GROUP_COUNT: usize = 6;

const SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/fluid_surface.wgsl"),
    "\n",
    include_str!("../shaders/sh_indirection.wgsl"),
    "\n",
    include_str!("../shaders/sh_sample.wgsl"),
    "\n",
    include_str!("../shaders/curve_eval.wgsl"),
    "\n",
    include_str!("../shaders/light_falloff.wgsl"),
    "\n",
    include_str!("../shaders/light_eval.wgsl"),
    "\n",
);

/// The shader's pipeline-overridable constant that selects its variant.
pub(crate) const FLUID_REFLECTIONS_OVERRIDE: &str = "FLUID_REFLECTIONS";

/// The fragment stage's override values for `variant`.
pub(crate) fn fluid_variant_constants(variant: FluidShaderVariant) -> [(&'static str, f64); 1] {
    [(
        FLUID_REFLECTIONS_OVERRIDE,
        if variant.reflections() { 1.0 } else { 0.0 },
    )]
}

/// Group 4, level-scoped: one uniform window (at most 64 KiB) of per-volume
/// rows selected by a dynamic offset, then the reflection probe array, share
/// table and distance array, and (SPIKE) the sampled cell atlas. Textures,
/// samplers and uniforms only: the shared groups already spend the whole
/// FRAGMENT storage budget.
pub(crate) fn fluid_params_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 5] {
    let [probe_array, probe_table, probe_distance] = fluid_probe_bind_group_layout_entries();
    [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: NonZeroU64::new(FLUID_PARAMS_WINDOW_BYTES),
            },
            count: None,
        },
        probe_array,
        probe_table,
        probe_distance,
        fluid_cell_atlas_layout_entry(),
    ]
}

/// Every BGL entry set the fluid pipeline layout composes, in group order.
/// GPU-free, so the budget tests and the init-time debug assert derive the
/// per-stage inventory from the same builders the layout is created from.
#[cfg(any(test, debug_assertions))]
pub(crate) fn fluid_pipeline_bind_group_layout_entries()
-> [Vec<wgpu::BindGroupLayoutEntry>; FLUID_BIND_GROUP_COUNT] {
    [
        uniform_bind_group_layout_entries().to_vec(),
        material_bind_group_layout_entries().to_vec(),
        lighting_bind_group_layout_entries().to_vec(),
        sh_volume::sh_bind_group_layout_entries(),
        fluid_params_bind_group_layout_entries().to_vec(),
        fluid_scene_inputs_bind_group_layout_entries().to_vec(),
    ]
}

/// Sum one per-stage binding count over the fluid pipeline's groups.
#[cfg(debug_assertions)]
pub(crate) fn fluid_pipeline_stage_count(count: fn(&[wgpu::BindGroupLayoutEntry]) -> u32) -> u32 {
    fluid_pipeline_bind_group_layout_entries()
        .iter()
        .map(|entries| count(entries))
        .sum()
}

/// Straight alpha over the scene. The fragment's alpha is the composite's
/// coverage: the surface over fog, plus the probe reflection's union coverage
/// F + (1 - F)·a from `fluid_reflection_composite`. Murk alone still hides the
/// scene.
pub(crate) fn fluid_blend_state() -> wgpu::BlendState {
    wgpu::BlendState {
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
    }
}

/// Depth tested like the billboard pass and never written: a fluid face hides
/// nothing drawn after it, so smoke above the water stays undimmed and fog
/// composites over it. Never writing is also what lets the pass open depth
/// read-only and load it in group 5.
pub(crate) fn fluid_depth_stencil_state(format: wgpu::TextureFormat) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format,
        depth_write_enabled: Some(false),
        depth_compare: Some(wgpu::CompareFunction::Less),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

/// Two-sided: a fluid face is seen from inside the volume as well as outside.
pub(crate) fn fluid_primitive_state() -> wgpu::PrimitiveState {
    wgpu::PrimitiveState {
        topology: wgpu::PrimitiveTopology::TriangleList,
        front_face: wgpu::FrontFace::Ccw,
        cull_mode: None,
        ..Default::default()
    }
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 3] = [
    wgpu::VertexAttribute {
        offset: 0,
        shader_location: 0,
        format: wgpu::VertexFormat::Float32x3,
    },
    wgpu::VertexAttribute {
        offset: 12,
        shader_location: 1,
        format: wgpu::VertexFormat::Float32x3,
    },
    wgpu::VertexAttribute {
        offset: 24,
        shader_location: 2,
        format: wgpu::VertexFormat::Float32x2,
    },
];

/// The second vertex buffer: one drift rate per vertex, texture periods per
/// second, rewritten in place whenever drift re-resolves.
const DRIFT_ATTRIBUTES: [wgpu::VertexAttribute; 1] = [wgpu::VertexAttribute {
    offset: 0,
    shader_location: 3,
    format: wgpu::VertexFormat::Float32x2,
}];

/// The third vertex buffer: the face's reflection probe index, written once
/// at install. Bindings are per face and rows per volume, so the index rides
/// the vertex stream, never a row.
const PROBE_INDEX_ATTRIBUTES: [wgpu::VertexAttribute; 1] = [wgpu::VertexAttribute {
    offset: 0,
    shader_location: 4,
    format: wgpu::VertexFormat::Uint32,
}];

/// One installed level's fluid geometry and per-volume parameter buffer.
struct InstalledFluidSurfaces {
    vertex_buffer: wgpu::Buffer,
    /// Zeroed at install: a level draws still until drift resolves, and
    /// capture, which resolves none, always draws still.
    drift_buffer: wgpu::Buffer,
    /// One map per surface face of every volume, in record order.
    face_drift_maps: Vec<Option<FluidFaceDriftMap>>,
    /// Reused per re-resolve.
    drift_rates: Vec<[f32; 2]>,
    /// One probe index per vertex; the no-probe value for faces no binding
    /// names, every face of a level without probes included.
    probe_index_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    params_buffer: wgpu::Buffer,
    /// The level's probe array, share table and distance array, or their
    /// dummies. Held so they drop with the level; the frame reaches them only
    /// through `params_bind_group`.
    _probes: FluidProbeResources,
    params_bind_group: wgpu::BindGroup,
    /// SPIKE (cell-rate spike): the cell atlas, its vertex words and pass
    /// resources.
    cells: InstalledFluidCells,
    /// Indexed by draw slot.
    volumes: Vec<FluidSurfaceVolume>,
    slots: FluidSurfaceSlots,
    /// Parameter-row uploads so far; tests read it.
    #[cfg(test)]
    param_uploads: u32,
}

pub(crate) struct FluidSurfacePass {
    /// One pipeline per `FluidShaderVariant`, in `FluidShaderVariant::ALL`
    /// order, over one layout: the reflection-free variant and the
    /// reflecting one. Each draw takes its row's variant.
    pipelines: [wgpu::RenderPipeline; 2],
    params_bind_group_layout: wgpu::BindGroupLayout,
    /// Group 5. Built at full init, level or not, and rebuilt only by the
    /// scene-target rebuild; level install and clear never touch it.
    scene_inputs: FluidSceneInputs,
    /// `None` until a level with drawable fluid surfaces installs: a level
    /// without them allocates nothing and never opens the pass.
    installed: Option<InstalledFluidSurfaces>,
    /// SPIKE (cell-rate spike): what the lever pipelines build from, the
    /// reflecting lever pipelines (built on first use, indexed by
    /// `lever_pipeline_index`), the cell pass pipelines, the levers, and the
    /// stats frame's resources while armed.
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    depth_format: wgpu::TextureFormat,
    lever_pipelines: [Option<wgpu::RenderPipeline>; 3],
    cell_pipelines: FluidCellPipelines,
    cell_levers: FluidReflectionCellLevers,
    cell_stats: Option<FluidCellStatsGpu>,
}

impl FluidSurfacePass {
    /// Build both shader-variant pipelines over the renderer-owned shared
    /// layouts: camera (group 0), world material (1), lighting (2) and SH
    /// volume (3), and group 5 over the current scene depth.
    pub fn new(
        device: &wgpu::Device,
        depth_format: wgpu::TextureFormat,
        camera_bgl: &wgpu::BindGroupLayout,
        material_bgl: &wgpu::BindGroupLayout,
        lighting_bgl: &wgpu::BindGroupLayout,
        sh_bgl: &wgpu::BindGroupLayout,
        scene_depth: &wgpu::TextureView,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Fluid Surface Shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SOURCE.into()),
        });
        let params_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Fluid Surface Params BGL (group 4)"),
                entries: &fluid_params_bind_group_layout_entries(),
            });
        let scene_inputs = FluidSceneInputs::new(device, scene_depth);
        // Same group order as `fluid_pipeline_bind_group_layout_entries`.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Fluid Surface Pipeline Layout"),
            bind_group_layouts: &[
                Some(camera_bgl),
                Some(material_bgl),
                Some(lighting_bgl),
                Some(sh_bgl),
                Some(&params_bind_group_layout),
                Some(scene_inputs.layout()),
            ],
            immediate_size: 0,
        });
        let pipelines = FluidShaderVariant::ALL.map(|variant| {
            create_fluid_pipeline(
                device,
                &pipeline_layout,
                &shader,
                depth_format,
                variant,
                &fluid_variant_constants(variant),
            )
        });
        let cell_pipelines = FluidCellPipelines::new(device, camera_bgl, sh_bgl);
        Self {
            pipelines,
            params_bind_group_layout,
            scene_inputs,
            installed: None,
            shader,
            pipeline_layout,
            depth_format,
            lever_pipelines: [None, None, None],
            cell_pipelines,
            cell_levers: FluidReflectionCellLevers::default(),
            cell_stats: None,
        }
    }

    /// SPIKE (cell-rate spike): set the debug levers, building any pipeline
    /// they need now, so a frame never compiles one.
    pub fn set_cell_levers(&mut self, device: &wgpu::Device, levers: FluidReflectionCellLevers) {
        self.cell_levers = levers;
        if let Some(index) = lever_pipeline_index(levers.cells, levers.ratio_one)
            && self.lever_pipelines[index].is_none()
        {
            let mut constants = fluid_variant_constants(FluidShaderVariant::Reflecting).to_vec();
            constants.extend(lever_constants(levers.cells, levers.ratio_one));
            self.lever_pipelines[index] = Some(create_fluid_pipeline(
                device,
                &self.pipeline_layout,
                &self.shader,
                self.depth_format,
                FluidShaderVariant::Reflecting,
                &constants,
            ));
        }
        if levers.cells {
            self.cell_pipelines.ensure(device, SHADER_SOURCE, levers);
        }
    }

    pub fn cell_levers(&self) -> FluidReflectionCellLevers {
        self.cell_levers
    }

    /// The cells lever acts only in pixel style.
    fn cells_on(&self) -> bool {
        self.cell_levers.cells && self.scene_inputs.reflection_style() == FluidReflectionStyle::Pixel
    }

    /// SPIKE: prepare this frame's cell pass (tile list and uniform) from the
    /// frame's draws. Called beside the scene-input upload, before the pass.
    pub fn prepare_cell_pass(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
        light_term_mask: LightTermMask,
        view_proj: glam::Mat4,
        extent: [u32; 2],
        max_workgroups: u32,
    ) {
        let cells_on = self.cells_on();
        let stats = self.cell_stats.is_some();
        let Some(installed) = self.installed.as_mut() else {
            return;
        };
        installed.cells.prepare(
            queue,
            &installed.slots,
            light_term_mask.bits(),
            cells_on,
            view_proj,
            extent,
            max_workgroups,
            stats,
        );
    }

    /// SPIKE: record the Fluid Reflection Cell Pass, its own compute pass,
    /// if this frame prepared one. The caller passes the frame's camera
    /// (group 0) and SH (group 3) bind groups.
    pub fn record_cell_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        camera_bind_group: &wgpu::BindGroup,
        sh_bind_group: &wgpu::BindGroup,
    ) {
        let Some(installed) = self.installed.as_ref() else {
            return;
        };
        installed.cells.record(
            encoder,
            &self.cell_pipelines,
            self.cell_levers,
            camera_bind_group,
            sh_bind_group,
            self.cell_stats.is_some(),
        );
    }

    /// SPIKE: the stats frame's occlusion query set, for the fluid pass.
    pub fn cell_stats_query_set(&self) -> Option<&wgpu::QuerySet> {
        self.cell_stats
            .as_ref()
            .filter(|_| self.installed.is_some())
            .map(FluidCellStatsGpu::query_set)
    }

    /// SPIKE: arm the next frame's stats; reading them disarms.
    pub fn arm_cell_stats(&mut self, device: &wgpu::Device) {
        let slot_count = self
            .installed
            .as_ref()
            .map_or(1, |installed| installed.slots.slot_count());
        self.cell_stats = Some(FluidCellStatsGpu::new(device, slot_count));
    }

    /// SPIKE: after the fluid pass, resolve the stats frame's queries and copy
    /// them and the evaluated-cell counter to the readback.
    pub fn encode_cell_stats_readback(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let (Some(stats), Some(installed)) = (self.cell_stats.as_mut(), self.installed.as_ref())
        else {
            return;
        };
        let written = installed.cells.frame().reflecting_draws;
        stats.encode_readback(encoder, &installed.cells, written);
    }

    /// SPIKE: read the armed frame's stats after its submission and disarm.
    pub fn take_cell_stats(&mut self, device: &wgpu::Device) -> Option<FluidReflectionCellStats> {
        let mut gpu = self.cell_stats.take()?;
        let installed = self.installed.as_ref()?;
        let (evaluated, samples) = gpu.read(device)?;
        let frame = installed.cells.frame();
        let layout = installed.cells.layout();
        Some(FluidReflectionCellStats {
            levers: self.cell_levers,
            pixel_style: self.scene_inputs.reflection_style() == FluidReflectionStyle::Pixel,
            atlas_width: layout.width,
            atlas_height: layout.height,
            atlas_bytes: layout.bytes(),
            faces_in_atlas: layout.faces.len() as u32,
            faces_skipped: layout.skipped_faces,
            reflecting_draws: frame.reflecting_draws,
            tiles_dispatched: frame.tiles,
            threads_dispatched: FluidReflectionCellStats::threads(frame.tiles),
            cells_dispatched: frame.cells,
            cells_evaluated: evaluated,
            reflecting_fragments: samples,
            uncounted_draws: frame.reflecting_draws.saturating_sub(gpu.query_count()),
        })
    }

    /// SPIKE test hook: fill the atlas with `texels` and run no cell pass.
    #[cfg(test)]
    pub(crate) fn write_cell_atlas_for_test(&mut self, queue: &wgpu::Queue, texels: &[u8]) {
        if let Some(installed) = self.installed.as_mut() {
            installed.cells.write_atlas_for_test(queue, texels);
        }
    }

    /// SPIKE test hook: the installed atlas layout.
    #[cfg(test)]
    pub(crate) fn cell_atlas_layout(
        &self,
    ) -> Option<&postretro_render_cpu::fluid_surface::reflection_cells::FluidCellAtlas> {
        self.installed
            .as_ref()
            .map(|installed| installed.cells.layout())
    }

    /// Rebuild group 5 over a recreated scene depth. The scene-target rebuild
    /// is its only caller: the frame never builds a bind group.
    pub fn rebuild_scene_inputs(&mut self, device: &wgpu::Device, scene_depth: &wgpu::TextureView) {
        self.scene_inputs.rebuild(device, scene_depth);
    }

    /// Store the frame's eye fog weight; it uploads before the next fluid pass.
    pub fn set_fog_weight(&mut self, weight: f32) {
        self.scene_inputs.set_fog_weight(weight);
    }

    /// Store the mod's reflection style; it uploads before the next fluid pass.
    pub fn set_reflection_style(
        &mut self,
        style: postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle,
    ) {
        self.scene_inputs.set_reflection_style(style);
    }

    /// Write the scene uniform if the fog weight or reflection style changed.
    /// Called just before the fluid pass opens.
    pub fn upload_scene_inputs(&mut self, queue: &crate::render::uploads::UploadQueue) {
        self.scene_inputs.upload(queue);
    }

    #[cfg(test)]
    pub(crate) fn scene_inputs(&self) -> &FluidSceneInputs {
        &self.scene_inputs
    }

    /// Drop the installed level's fluid geometry. Level install and unload
    /// both start here.
    pub fn clear(&mut self) {
        self.installed = None;
    }

    /// Upload every drawable surface face of `volumes`, triangulated by
    /// `pack_fluid_surfaces` into one shared buffer with per-volume material
    /// ranges. UVs normalize by `texture_dimensions` (indexed by texture
    /// index); faces whose texture is past `material_count` draw with the
    /// placeholder material. `reflection_probes` (section 54) uploads whole
    /// with the surfaces, and its bindings write each face's probe index; a
    /// level without it binds dummies.
    pub fn install(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::uploads::UploadQueue,
        volumes: &[EnvironmentVolumeRecord],
        reflection_probes: Option<&ReflectionProbesSection>,
        texture_dimensions: &[[u32; 2]],
        material_count: usize,
    ) {
        self.installed = None;
        let mesh = pack_fluid_surfaces(volumes, texture_dimensions, material_count);
        if mesh.indices.is_empty() {
            return;
        }
        let reflection_probes = installable_probe_section(reflection_probes);
        let probe_index_buffer = create_probe_index_buffer(
            device,
            volumes,
            &mesh.face_drift_maps,
            mesh.vertices.len(),
            reflection_probes,
        );
        // An install-time upload takes the raw queue, as the SH atlases do.
        let probes = FluidProbeResources::new(device, queue.raw(), reflection_probes);
        let slots = FluidSurfaceSlots::new(&mesh.volumes);
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fluid Surface Vertex Buffer"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let drift_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fluid Surface Drift Buffer"),
            size: (mesh.vertices.len() * FLUID_DRIFT_RATE_STRIDE) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fluid Surface Index Buffer"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        // Zeroed at creation; each slot uploads its row on first draw.
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fluid Surface Params"),
            size: u64::from(slots.window_count()) * FLUID_PARAMS_WINDOW_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cells = InstalledFluidCells::new(
            device,
            volumes,
            &mesh,
            reflection_probes,
            &probes,
            texture_dimensions,
            material_count,
            &self.cell_pipelines,
        );
        let [probe_array, probe_table, probe_distance] = probes.bind_group_entries();
        let params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Fluid Surface Params Bind Group (group 4)"),
            layout: &self.params_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &params_buffer,
                        offset: 0,
                        size: NonZeroU64::new(FLUID_PARAMS_WINDOW_BYTES),
                    }),
                },
                probe_array,
                probe_table,
                probe_distance,
                cells.atlas_entry(),
            ],
        });
        log::info!(
            "[Renderer] Fluid surfaces installed: {} volumes, {} triangles",
            mesh.volumes.len(),
            mesh.indices.len() / 3,
        );
        self.installed = Some(InstalledFluidSurfaces {
            vertex_buffer,
            drift_buffer,
            drift_rates: vec![[0.0; 2]; mesh.vertices.len()],
            face_drift_maps: mesh.face_drift_maps,
            probe_index_buffer,
            index_buffer,
            params_buffer,
            _probes: probes,
            params_bind_group,
            cells,
            volumes: mesh.volumes,
            slots,
            #[cfg(test)]
            param_uploads: 0,
        });
    }

    /// Replace every face's drift: `drifts` holds one world drift per surface
    /// face of every installed volume, in record order. Only the drift buffer
    /// is rewritten, so draws, slots and parameter rows carry over untouched.
    pub fn set_drift(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
        drifts: &[glam::Vec3],
    ) {
        let Some(installed) = self.installed.as_mut() else {
            return;
        };
        if drifts.len() != installed.face_drift_maps.len() {
            log::warn!(
                "[Renderer] Fluid surface drift count {} does not match {} surface faces; \
                 water holds still",
                drifts.len(),
                installed.face_drift_maps.len(),
            );
        }
        fill_drift_rates(
            &installed.face_drift_maps,
            drifts,
            &mut installed.drift_rates,
        );
        // Drift re-resolves at install, reload commit and fluid-table or
        // tuning refreshes. Some of those points require an empty upload
        // batch. Nothing ever batch-writes this buffer, so a direct write,
        // which lands now, is safe at every one.
        queue.direct_write_buffer(
            &installed.drift_buffer,
            0,
            bytemuck::cast_slice(&installed.drift_rates),
        );
    }

    /// Layers of the installed probe array: 1 for the dummy.
    #[cfg(test)]
    pub(crate) fn probe_array_layers(&self) -> Option<u32> {
        self.probe_array_size()
            .map(|size| size.depth_or_array_layers)
    }

    /// The installed probe array's tile width, height and layer count: 1×1
    /// and one layer for the dummy.
    #[cfg(test)]
    pub(crate) fn probe_array_size(&self) -> Option<wgpu::Extent3d> {
        self.installed
            .as_ref()
            .map(|installed| installed._probes.array_size())
    }

    /// The installed distance array's tile width, height and layer count:
    /// 1×1 and one layer for the dummy.
    #[cfg(test)]
    pub(crate) fn probe_distance_size(&self) -> Option<wgpu::Extent3d> {
        self.installed
            .as_ref()
            .map(|installed| installed._probes.distance_size())
    }

    /// Parameter-row uploads since install.
    #[cfg(test)]
    pub(crate) fn param_uploads(&self) -> Option<u32> {
        self.installed
            .as_ref()
            .map(|installed| installed.param_uploads)
    }

    /// Replace this frame's draws, uploading only parameter rows that changed.
    pub fn set_draws(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
        draws: &[FluidSurfaceDraw],
    ) {
        let Some(installed) = self.installed.as_mut() else {
            return;
        };
        installed.slots.set_draws(draws);
        if let Some((range, rows)) = installed.slots.take_dirty() {
            #[cfg(test)]
            {
                installed.param_uploads += 1;
            }
            queue.write_buffer(
                &installed.params_buffer,
                u64::from(range.start) * FLUID_PARAMS_STRIDE as u64,
                bytemuck::cast_slice(rows),
            );
        }
    }

    /// The pass's own gate: any visible fluid surface this frame.
    pub fn has_draws(&self) -> bool {
        self.installed
            .as_ref()
            .is_some_and(|installed| installed.slots.has_draws())
    }

    /// Record this frame's draws. The caller binds groups 0, 2 and 3; this
    /// binds 1, 4 and the scene inputs built with the scene targets (5).
    /// Each draw takes its row's shader variant under `light_term_mask`, the
    /// frame's group-0 mask; the pipeline is set only where it changes
    /// between consecutive draws.
    pub fn record_draws(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        materials: &[GpuTexture],
        light_term_mask: LightTermMask,
    ) {
        // SPIKE: the frame's reflecting draws take the lever pipeline, if
        // any; the stats frame counts each one's fragments by occlusion query.
        let lever_index = self.installed.as_ref().and_then(|installed| {
            lever_pipeline_index(
                installed.cells.frame().cells_variant,
                self.cell_levers.ratio_one,
            )
        });
        let reflecting_pipeline = lever_index
            .and_then(|index| self.lever_pipelines[index].as_ref())
            .unwrap_or_else(|| self.pipeline(FluidShaderVariant::Reflecting));
        let query_capacity = self
            .cell_stats
            .as_ref()
            .map_or(0, FluidCellStatsGpu::query_count);
        let mut queries = 0u32;
        let Some(installed) = self.installed.as_ref() else {
            return;
        };
        if !installed.slots.has_draws() {
            return;
        }
        pass.set_bind_group(
            FLUID_SCENE_INPUTS_GROUP,
            self.scene_inputs.bind_group(),
            &[],
        );
        pass.set_vertex_buffer(0, installed.vertex_buffer.slice(..));
        pass.set_vertex_buffer(1, installed.drift_buffer.slice(..));
        pass.set_vertex_buffer(2, installed.probe_index_buffer.slice(..));
        pass.set_vertex_buffer(3, installed.cells.vertex_buffer().slice(..));
        pass.set_index_buffer(installed.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

        let mut bound_variant = None;
        let mut bound_window = None;
        let mut bound_material = None;
        for (slot, variant) in installed.slots.frame_draws(light_term_mask.bits()) {
            if bound_variant != Some(variant) {
                pass.set_pipeline(match variant {
                    FluidShaderVariant::Reflecting => reflecting_pipeline,
                    FluidShaderVariant::ReflectionFree => self.pipeline(variant),
                });
                bound_variant = Some(variant);
            }
            let query = variant == FluidShaderVariant::Reflecting && queries < query_capacity;
            if query {
                pass.begin_occlusion_query(queries);
                queries += 1;
            }
            let (window, element) = slot_window(slot);
            if bound_window != Some(window) {
                let offset = u64::from(window) * FLUID_PARAMS_WINDOW_BYTES;
                pass.set_bind_group(
                    FLUID_PARAMS_GROUP,
                    &installed.params_bind_group,
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
            if query {
                pass.end_occlusion_query();
            }
        }
    }

    fn pipeline(&self, variant: FluidShaderVariant) -> &wgpu::RenderPipeline {
        &self.pipelines[variant as usize]
    }

    /// The variant each of this frame's draws takes, in draw order: what
    /// `record_draws` binds.
    #[cfg(test)]
    pub(crate) fn frame_variants(&self, light_term_mask: LightTermMask) -> Vec<FluidShaderVariant> {
        self.installed.as_ref().map_or_else(Vec::new, |installed| {
            installed
                .slots
                .frame_draws(light_term_mask.bits())
                .map(|(_, variant)| variant)
                .collect()
        })
    }
}

/// One fluid pipeline: `variant` sets the shader's `FLUID_REFLECTIONS`
/// override in the fragment stage. Everything else is shared, so both
/// variants draw under one layout, vertex layout, blend and depth state.
fn create_fluid_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    depth_format: wgpu::TextureFormat,
    variant: FluidShaderVariant,
    constants: &[(&str, f64)],
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(match variant {
            FluidShaderVariant::ReflectionFree => "Fluid Surface Pipeline (reflection-free)",
            FluidShaderVariant::Reflecting => "Fluid Surface Pipeline (reflecting)",
        }),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[
                Some(wgpu::VertexBufferLayout {
                    array_stride: FLUID_SURFACE_VERTEX_STRIDE as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &VERTEX_ATTRIBUTES,
                }),
                Some(wgpu::VertexBufferLayout {
                    array_stride: FLUID_DRIFT_RATE_STRIDE as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &DRIFT_ATTRIBUTES,
                }),
                Some(wgpu::VertexBufferLayout {
                    array_stride: FLUID_PROBE_INDEX_STRIDE as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &PROBE_INDEX_ATTRIBUTES,
                }),
                // SPIKE: the cell atlas entry (location 5).
                Some(cell_vertex_buffer_layout()),
            ],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: fluid_primitive_state(),
        depth_stencil: Some(fluid_depth_stencil_state(depth_format)),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: SCENE_COLOR_FORMAT,
                blend: Some(fluid_blend_state()),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions {
                constants,
                ..Default::default()
            },
        }),
        multiview_mask: None,
        cache: None,
    })
}

impl Renderer {
    /// Install the level's fluid surfaces and their reflection probes. Call
    /// after `install_textures` and `install_level_geometry` (which drops the
    /// previous level's): UVs normalize by the installed diffuse dimensions,
    /// as `normalize_world_uvs` does for world faces. Volumes without surface
    /// faces allocate nothing, probes included; a level without any never
    /// opens the pass. `reflection_probes` is the level's section 54.
    pub fn install_fluid_surfaces(
        &mut self,
        volumes: &[EnvironmentVolumeRecord],
        reflection_probes: Option<&ReflectionProbesSection>,
    ) {
        let Self {
            device,
            queue,
            full,
            ..
        } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let texture_dimensions =
            super::renderer_models::loaded_texture_dimensions(&full.loaded_textures);
        let material_count = full.gpu_textures.len();
        full.fluid_surface.install(
            device,
            queue,
            volumes,
            reflection_probes,
            &texture_dimensions,
            material_count,
        );
    }

    /// Every installed fluid face's world drift, one per surface face of every
    /// volume in record order (`EnvironmentVolumeRecord::surface_faces`). Call
    /// after `install_fluid_surfaces` and whenever drift re-resolves; a slice
    /// of the wrong length draws every face still.
    pub fn set_fluid_surface_drift(&mut self, drifts: &[glam::Vec3]) {
        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        full.fluid_surface.set_drift(queue, drifts);
    }

    /// This frame's visible fluid draws, far to near, from
    /// `FluidSurfaceRenderCollector`. Call before `render_frame_indirect`; an
    /// empty slice closes the pass.
    pub fn set_fluid_surface_draws(&mut self, draws: &[FluidSurfaceDraw]) {
        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        full.fluid_surface.set_draws(queue, draws);
    }

    /// Store the frame's eye fog weight (0..1): 0 while the eye is inside any
    /// fluid, else one minus the view tint's ramp. Send it every presented
    /// frame beside `set_fluid_view_tint`. Held until replaced; before the
    /// first write it is 1, so a renderer recreated mid-level still fogs.
    pub fn set_fluid_fog_weight(&mut self, weight: f32) {
        self.full_mut().fluid_surface.set_fog_weight(weight);
    }

    /// Commit the mod's fluid reflection style (`render.reflections.style`).
    /// Retained in boot state, so a full init after surface recovery keeps it,
    /// and safe before full init; it uploads before the next fluid pass, and
    /// only when it changed. A renderer never told draws pixel, the default,
    /// as frame capture does.
    pub fn set_fluid_reflection_style(
        &mut self,
        style: postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle,
    ) {
        self.fluid_reflection_style = style;
        if let Some(full) = self.full.as_mut() {
            full.fluid_surface.set_reflection_style(style);
        }
    }

    /// SPIKE (cell-rate spike): set the debug cell levers. Safe before full
    /// init (then dropped); builds any pipeline the levers need now.
    pub fn set_fluid_reflection_cell_levers(&mut self, levers: FluidReflectionCellLevers) {
        let Self { device, full, .. } = self;
        if let Some(full) = full.as_mut() {
            full.fluid_surface.set_cell_levers(device, levers);
        }
    }

    /// SPIKE: the debug cell levers, once fully initialized.
    pub fn fluid_reflection_cell_levers(&self) -> Option<FluidReflectionCellLevers> {
        self.full
            .as_ref()
            .map(|full| full.fluid_surface.cell_levers())
    }

    /// SPIKE: count the next frame's cell stats (an occlusion query per
    /// reflecting draw and the pass's evaluated-cell counter).
    pub fn arm_fluid_reflection_cell_stats(&mut self) {
        let Self { device, full, .. } = self;
        if let Some(full) = full.as_mut() {
            full.fluid_surface.arm_cell_stats(device);
        }
    }

    /// SPIKE: the armed frame's stats, after it completed; disarms.
    pub fn take_fluid_reflection_cell_stats(&mut self) -> Option<FluidReflectionCellStats> {
        let Self { device, full, .. } = self;
        full.as_mut()?.fluid_surface.take_cell_stats(device)
    }

    /// The reflection style the next fluid pass draws.
    pub fn fluid_reflection_style(
        &self,
    ) -> postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle {
        self.fluid_reflection_style
    }
}

#[cfg(test)]
pub(crate) fn composed_shader_source() -> &'static str {
    SHADER_SOURCE
}

#[cfg(test)]
#[path = "fluid_surface_tests.rs"]
mod tests;
