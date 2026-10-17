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
    FLUID_CELL_ATLAS_BINDING, FLUID_CELL_VERTEX_ATTRIBUTES, FLUID_CELL_VERTEX_STRIDE,
    FluidCellLevers, FluidCellResources, FluidCellStats, build_cell_layout,
    cell_atlas_bind_group_layout_entry, cell_bind_group_layout_entries,
    cell_probe_bind_group_layout_entries, create_dummy_atlas,
};
use super::fluid_reflection_probes::{
    FluidProbeResources, create_probe_index_buffer, fluid_probe_bind_group_layout_entries,
    installable_probe_section,
};
use postretro_render_cpu::fluid_surface::reflection::face_probe_indices;

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
    include_str!("../shaders/fluid_reflection_cells.wgsl"),
    "\n",
);

/// The shader's pipeline-overridable constant that selects its variant.
pub(crate) const FLUID_REFLECTIONS_OVERRIDE: &str = "FLUID_REFLECTIONS";

/// Cell-rate spike overrides: read the cell atlas; skip the live-lighting ratio.
pub(crate) const FLUID_REFLECTION_CELLS_OVERRIDE: &str = "FLUID_REFLECTION_CELLS";
pub(crate) const FLUID_REFLECTION_RATIO_ONE_OVERRIDE: &str = "FLUID_REFLECTION_RATIO_ONE";

/// Render pipelines by role: the two shipped variants, then the spike's
/// reflecting variants (ratio one; cell atlas).
const PIPELINE_REFLECTION_FREE: usize = 0;
const PIPELINE_REFLECTING: usize = 1;
const PIPELINE_RATIO_ONE: usize = 2;
const PIPELINE_CELLS: usize = 3;
/// Two-draw cells: the premultiplied reflection overlay drawn after the
/// reflection-free surface.
const PIPELINE_CELL_OVERLAY: usize = 4;

/// The fragment stage's override values for `variant`.
pub(crate) fn fluid_variant_constants(variant: FluidShaderVariant) -> [(&'static str, f64); 1] {
    [(
        FLUID_REFLECTIONS_OVERRIDE,
        if variant.reflections() { 1.0 } else { 0.0 },
    )]
}

/// Group 4, level-scoped: one uniform window (at most 64 KiB) of per-volume
/// rows selected by a dynamic offset, then the reflection probe array, share
/// table and distance array. Textures, samplers and uniforms only: the shared
/// groups already spend the whole FRAGMENT storage budget.
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
        cell_atlas_bind_group_layout_entry(),
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
    /// Cell-rate spike: per-vertex cell offset and rect (fourth vertex buffer).
    cell_vertex_buffer: wgpu::Buffer,
    /// The level's cell atlas and compute resources, when it can read cells.
    cells: Option<FluidCellResources>,
    /// The 1×1 atlas group 4 binds without cells; held for the bind group.
    _dummy_atlas: Option<(wgpu::Texture, wgpu::TextureView)>,
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
    pipelines: [wgpu::RenderPipeline; 5],
    /// Cell-rate spike: the cell compute pipeline, then its ratio-one variant.
    cell_pipelines: [wgpu::ComputePipeline; 2],
    cell_probe_bind_group_layout: wgpu::BindGroupLayout,
    cell_bind_group_layout: wgpu::BindGroupLayout,
    levers: FluidCellLevers,
    /// This frame reads cells (set by `prepare_cells`).
    cells_active: bool,
    /// One occlusion query over the fluid draws, while counting.
    occlusion_query_set: wgpu::QuerySet,
    occlusion_resolve: wgpu::Buffer,
    /// The query was written and resolved since install.
    occlusion_resolved: std::cell::Cell<bool>,
    /// The counting frame's pass carries the query set.
    occlusion_this_frame: bool,
    params_bind_group_layout: wgpu::BindGroupLayout,
    /// Group 5. Built at full init, level or not, and rebuilt only by the
    /// scene-target rebuild; level install and clear never touch it.
    scene_inputs: FluidSceneInputs,
    /// `None` until a level with drawable fluid surfaces installs: a level
    /// without them allocates nothing and never opens the pass.
    installed: Option<InstalledFluidSurfaces>,
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
        let pipelines = [
            (FluidShaderVariant::ReflectionFree, false, false, false),
            (FluidShaderVariant::Reflecting, false, false, false),
            (FluidShaderVariant::Reflecting, false, true, false),
            (FluidShaderVariant::Reflecting, true, false, false),
            (FluidShaderVariant::Reflecting, true, false, true),
        ]
        .map(|(variant, cells, ratio_one, overlay)| {
            create_fluid_pipeline(
                device,
                &pipeline_layout,
                &shader,
                depth_format,
                variant,
                cells,
                ratio_one,
                overlay,
            )
        });
        let cell_probe_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Fluid Reflection Cell Probe BGL (compute group 4)"),
                entries: &cell_probe_bind_group_layout_entries(),
            });
        let cell_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Fluid Reflection Cell BGL (compute group 5)"),
                entries: &cell_bind_group_layout_entries(),
            });
        let cell_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Fluid Reflection Cell Pipeline Layout"),
            bind_group_layouts: &[
                Some(camera_bgl),
                None,
                None,
                Some(sh_bgl),
                Some(&cell_probe_bind_group_layout),
                Some(&cell_bind_group_layout),
            ],
            immediate_size: 0,
        });
        let cell_pipelines = [false, true].map(|ratio_one| {
            let constants = [(
                FLUID_REFLECTION_RATIO_ONE_OVERRIDE,
                if ratio_one { 1.0 } else { 0.0 },
            )];
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(if ratio_one {
                    "Fluid Reflection Cell Pipeline (ratio one)"
                } else {
                    "Fluid Reflection Cell Pipeline"
                }),
                layout: Some(&cell_layout),
                module: &shader,
                entry_point: Some("cs_fluid_reflection_cells"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
                cache: None,
            })
        });
        let occlusion_query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("Fluid Surface Occlusion Query"),
            ty: wgpu::QueryType::Occlusion,
            count: 1,
        });
        let occlusion_resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fluid Surface Occlusion Resolve"),
            size: 8,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Self {
            pipelines,
            cell_pipelines,
            cell_probe_bind_group_layout,
            cell_bind_group_layout,
            levers: FluidCellLevers::default(),
            cells_active: false,
            occlusion_query_set,
            occlusion_resolve,
            occlusion_resolved: std::cell::Cell::new(false),
            occlusion_this_frame: false,
            params_bind_group_layout,
            scene_inputs,
            installed: None,
        }
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
        let face_probes = face_probe_indices(volumes, &mesh.face_drift_maps, reflection_probes);
        let (cell_vertices, cell_layout) = build_cell_layout(
            volumes,
            &mesh,
            &face_probes,
            texture_dimensions,
            material_count,
        );
        let cell_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fluid Reflection Cell Vertex Buffer"),
            contents: bytemuck::cast_slice(&cell_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let cells = cell_layout.map(|layout| {
            FluidCellResources::new(
                device,
                layout,
                probes.bind_group_entries(),
                &self.cell_probe_bind_group_layout,
                &self.cell_bind_group_layout,
            )
        });
        let dummy_atlas = cells.is_none().then(|| create_dummy_atlas(device));
        let atlas_view = cells
            .as_ref()
            .map(FluidCellResources::atlas_view)
            .or(dummy_atlas.as_ref().map(|(_, view)| view))
            .expect("cells or the dummy atlas");
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
                wgpu::BindGroupEntry {
                    binding: FLUID_CELL_ATLAS_BINDING,
                    resource: wgpu::BindingResource::TextureView(atlas_view),
                },
            ],
        });
        log::info!(
            "[Renderer] Fluid surfaces installed: {} volumes, {} triangles",
            mesh.volumes.len(),
            mesh.indices.len() / 3,
        );
        self.occlusion_resolved.set(false);
        self.installed = Some(InstalledFluidSurfaces {
            cell_vertex_buffer,
            cells,
            _dummy_atlas: dummy_atlas,
            vertex_buffer,
            drift_buffer,
            drift_rates: vec![[0.0; 2]; mesh.vertices.len()],
            face_drift_maps: mesh.face_drift_maps,
            probe_index_buffer,
            index_buffer,
            params_buffer,
            _probes: probes,
            params_bind_group,
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
        pass.set_vertex_buffer(3, installed.cell_vertex_buffer.slice(..));
        pass.set_index_buffer(installed.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        if self.occlusion_this_frame {
            pass.begin_occlusion_query(0);
        }

        let mut bound_variant = None;
        let mut bound_window = None;
        let mut bound_material = None;
        for (slot, variant) in installed.slots.frame_draws(light_term_mask.bits()) {
            let pipeline = self.pipeline_index(installed, slot, variant);
            if bound_variant != Some(pipeline) {
                pass.set_pipeline(&self.pipelines[pipeline]);
                bound_variant = Some(pipeline);
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
            // Two-draw cells: the surface reflection-free, then the overlay.
            let two_draw = pipeline == PIPELINE_CELLS && self.levers.overlay;
            if two_draw {
                pass.set_pipeline(&self.pipelines[PIPELINE_REFLECTION_FREE]);
                bound_variant = Some(PIPELINE_REFLECTION_FREE);
            }
            let passes: &[bool] = if two_draw { &[false, true] } else { &[false] };
            for &overlay_draw in passes {
                if overlay_draw {
                    pass.set_pipeline(&self.pipelines[PIPELINE_CELL_OVERLAY]);
                    bound_variant = Some(PIPELINE_CELL_OVERLAY);
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
        if self.occlusion_this_frame {
            pass.end_occlusion_query();
        }
    }

    /// The render pipeline `slot`'s draw takes this frame: the cell-atlas
    /// variant for a reflecting, cell-capable draw while cells are active;
    /// else the ratio-one variant under that lever; else the row's variant.
    fn pipeline_index(
        &self,
        installed: &InstalledFluidSurfaces,
        slot: u32,
        variant: FluidShaderVariant,
    ) -> usize {
        match variant {
            FluidShaderVariant::ReflectionFree => PIPELINE_REFLECTION_FREE,
            FluidShaderVariant::Reflecting if self.levers.reflection_off => {
                PIPELINE_REFLECTION_FREE
            }
            FluidShaderVariant::Reflecting => {
                if self.cells_active
                    && installed
                        .cells
                        .as_ref()
                        .is_some_and(|cells| cells.slot_reads_cells(slot))
                {
                    PIPELINE_CELLS
                } else if self.levers.ratio_one {
                    PIPELINE_RATIO_ONE
                } else {
                    PIPELINE_REFLECTING
                }
            }
        }
    }

    pub(crate) fn levers(&self) -> FluidCellLevers {
        self.levers
    }

    pub(crate) fn set_levers(&mut self, levers: FluidCellLevers) {
        self.levers = levers;
    }

    /// Decide whether this frame reads cells and, if so, upload its face and
    /// tile lists. Call once per drawing frame, before the cell pass and the
    /// fluid pass. Cells read only under the buffer lever, in pixel style, on
    /// a level with an atlas.
    pub fn prepare_cells(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
        camera: glam::Vec3,
        viewport: [u32; 2],
        light_term_mask: LightTermMask,
    ) {
        use postretro_render_cpu::fluid_surface::reflection_style::FLUID_REFLECTION_STYLE_PIXEL;
        let pixel = self.scene_inputs.pending().reflection_style == FLUID_REFLECTION_STYLE_PIXEL;
        let levers = self.levers;
        self.occlusion_this_frame = levers.count && self.installed.is_some();
        self.cells_active = false;
        let Some(installed) = self.installed.as_mut() else {
            return;
        };
        let Some(cells) = installed.cells.as_mut() else {
            return;
        };
        if !(levers.buffer && pixel) || levers.reflection_off {
            cells.clear_frame();
            return;
        }
        self.cells_active = true;
        cells.prepare(
            queue,
            &installed.slots,
            light_term_mask.bits(),
            camera,
            viewport,
            levers,
        );
    }

    /// Record this frame's cell compute pass, if it dispatched any tile.
    pub fn record_cell_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        camera_bind_group: &wgpu::BindGroup,
        sh_bind_group: &wgpu::BindGroup,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        if !self.cells_active {
            return;
        }
        let Some(cells) = self.installed.as_ref().and_then(|i| i.cells.as_ref()) else {
            return;
        };
        let pipeline = &self.cell_pipelines[usize::from(self.levers.ratio_one)];
        cells.record(
            encoder,
            pipeline,
            camera_bind_group,
            sh_bind_group,
            timestamp_writes,
        );
    }

    /// Whether this frame's cell pass dispatches (it takes a timing pair).
    pub fn cell_pass_dispatches(&self) -> bool {
        self.cells_active
            && self
                .installed
                .as_ref()
                .and_then(|i| i.cells.as_ref())
                .is_some_and(|cells| cells.frame_tiles() > 0)
    }

    /// The occlusion query set while this frame counts.
    pub fn occlusion_query_set(&self) -> Option<&wgpu::QuerySet> {
        self.occlusion_this_frame
            .then_some(&self.occlusion_query_set)
    }

    /// Resolve the counting frame's occlusion query after the fluid pass.
    pub fn resolve_occlusion(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.occlusion_this_frame && self.has_draws() {
            encoder.resolve_query_set(&self.occlusion_query_set, 0..1, &self.occlusion_resolve, 0);
            self.occlusion_resolved.set(true);
        }
    }

    /// Encode copies of the GPU counters into `readback` (16 B: evaluated,
    /// tile count, then samples passed as u64) and return the CPU-side
    /// figures, whether the counter and whether the query were copied.
    pub(crate) fn encode_cell_stats(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        readback: &wgpu::Buffer,
    ) -> Option<(FluidCellStats, bool, bool)> {
        let installed = self.installed.as_ref()?;
        let mut stats = FluidCellStats::default();
        let mut counted = false;
        if let Some(cells) = installed.cells.as_ref() {
            stats = cells.stats();
            cells.copy_counter(encoder, readback);
            counted = true;
        }
        let occluded = self.occlusion_resolved.get();
        if occluded {
            encoder.copy_buffer_to_buffer(&self.occlusion_resolve, 0, readback, 8, 8);
        }
        Some((stats, counted, occluded))
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
    cells: bool,
    ratio_one: bool,
    overlay: bool,
) -> wgpu::RenderPipeline {
    let [reflections] = fluid_variant_constants(variant);
    let spike_constants = [
        reflections,
        (
            FLUID_REFLECTION_CELLS_OVERRIDE,
            if cells { 1.0 } else { 0.0 },
        ),
        (
            FLUID_REFLECTION_RATIO_ONE_OVERRIDE,
            if ratio_one { 1.0 } else { 0.0 },
        ),
    ];
    // The shipped variants set only their own override, as before the spike.
    let experiment: f64 = std::env::var("POSTRETRO_SPIKE_CELL_EXPERIMENT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    let cells_constants = [
        spike_constants[0],
        spike_constants[1],
        spike_constants[2],
        ("FLUID_CELL_EXPERIMENT", experiment),
    ];
    let constants: &[(&str, f64)] = if cells {
        &cells_constants
    } else if ratio_one {
        &spike_constants
    } else {
        &spike_constants[..1]
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(match (variant, cells, ratio_one) {
            _ if overlay => "Fluid Surface Pipeline (cell reflection overlay)",
            (FluidShaderVariant::ReflectionFree, _, _) => {
                "Fluid Surface Pipeline (reflection-free)"
            }
            (FluidShaderVariant::Reflecting, true, _) => {
                "Fluid Surface Pipeline (reflecting, cells)"
            }
            (FluidShaderVariant::Reflecting, false, true) => {
                "Fluid Surface Pipeline (reflecting, ratio one)"
            }
            (FluidShaderVariant::Reflecting, false, false) => "Fluid Surface Pipeline (reflecting)",
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
                Some(wgpu::VertexBufferLayout {
                    array_stride: FLUID_CELL_VERTEX_STRIDE as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &FLUID_CELL_VERTEX_ATTRIBUTES,
                }),
            ],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: fluid_primitive_state(),
        depth_stencil: Some(fluid_depth_stencil_state(depth_format)),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            // The cells variant draws through its own entry point, which
            // holds no per-fragment reflection code.
            entry_point: Some(if overlay {
                "fs_cells_overlay"
            } else if cells {
                "fs_cells"
            } else {
                "fs_main"
            }),
            targets: &[Some(wgpu::ColorTargetState {
                format: SCENE_COLOR_FORMAT,
                blend: Some(if overlay {
                    wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING
                } else {
                    fluid_blend_state()
                }),
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

    /// The reflection style the next fluid pass draws.
    pub fn fluid_reflection_style(
        &self,
    ) -> postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle {
        self.fluid_reflection_style
    }

    fn update_fluid_cell_levers(&mut self, update: impl FnOnce(&mut FluidCellLevers)) {
        let pass = &mut self.full_mut().fluid_surface;
        let mut levers = pass.levers();
        update(&mut levers);
        pass.set_levers(levers);
    }

    /// Cell-rate spike lever: pixel-style reflections read the cell atlas.
    pub fn fluid_cell_buffer(&self) -> bool {
        self.full().fluid_surface.levers().buffer
    }

    pub fn set_fluid_cell_buffer(&mut self, enabled: bool) {
        self.update_fluid_cell_levers(|levers| levers.buffer = enabled);
    }

    /// Cell-rate spike lever: the cell pass skips cells outside the frustum.
    pub fn fluid_cell_cull(&self) -> bool {
        self.full().fluid_surface.levers().cull
    }

    pub fn set_fluid_cell_cull(&mut self, enabled: bool) {
        self.update_fluid_cell_levers(|levers| levers.cull = enabled);
    }

    /// Cell-rate spike lever: the probe term skips its live-lighting ratio.
    pub fn fluid_ratio_one(&self) -> bool {
        self.full().fluid_surface.levers().ratio_one
    }

    pub fn set_fluid_ratio_one(&mut self, enabled: bool) {
        self.update_fluid_cell_levers(|levers| levers.ratio_one = enabled);
    }

    /// Cell-rate spike lever: count evaluated cells and fluid samples
    /// (untimed runs only; read with `read_fluid_cell_stats`).
    pub fn fluid_cell_count(&self) -> bool {
        self.full().fluid_surface.levers().count
    }

    pub fn set_fluid_cell_count(&mut self, enabled: bool) {
        self.update_fluid_cell_levers(|levers| levers.count = enabled);
    }

    /// Cell-rate spike baseline lever: every fluid draw takes the
    /// reflection-free pipeline under the frame's own mask (no cells either).
    pub fn set_fluid_reflection_off(&mut self, enabled: bool) {
        self.update_fluid_cell_levers(|levers| levers.reflection_off = enabled);
    }

    /// Cell-rate spike lever: with the cell buffer, draw the surface through
    /// the reflection-free pipeline and add the cell reflection in a second,
    /// premultiplied overlay draw.
    pub fn set_fluid_cell_overlay(&mut self, enabled: bool) {
        self.update_fluid_cell_levers(|levers| levers.overlay = enabled);
    }

    /// The last frame's cell and fluid-sample counts, read back with a
    /// blocking wait (capture only). `None` without installed fluid surfaces.
    pub fn read_fluid_cell_stats(&mut self) -> Option<FluidCellStats> {
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fluid Reflection Cell Stats Readback"),
            size: 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Fluid Reflection Cell Stats"),
            });
        let (mut stats, counted, occluded) = self
            .full()
            .fluid_surface
            .encode_cell_stats(&mut encoder, &readback)?;
        self.queue
            .submit_unbatched(std::iter::once(encoder.finish()), "fluid cell stats");
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        rx.recv().ok()?.ok()?;
        {
            let data = slice.get_mapped_range().ok()?;
            let words: &[u32] = bytemuck::cast_slice(&data[..8]);
            if counted {
                stats.cells_evaluated = words[0];
            }
            if occluded {
                stats.fluid_samples_passed =
                    u64::from_le_bytes(data[8..16].try_into().expect("8 bytes"));
            }
        }
        readback.unmap();
        Some(stats)
    }
}

#[cfg(test)]
pub(crate) fn composed_shader_source() -> &'static str {
    SHADER_SOURCE
}

#[cfg(test)]
#[path = "fluid_surface_tests.rs"]
mod tests;
