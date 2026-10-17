// SPIKE (cell-rate spike, never merged): the pixel-style reflection cell
// atlas, its install-time resources, and the Fluid Reflection Cell Pass that
// evaluates every drawn reflecting cell once per frame.
// See: context/lib/rendering_pipeline.md §7.3a (Reflection → Style)

use std::num::NonZeroU64;

use wgpu::util::DeviceExt;

use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_level_format::reflection_probes::ReflectionProbesSection;
use postretro_render_cpu::fluid_surface::reflection::{
    FLUID_PROBE_TABLE_BYTES, FLUID_REFLECTION_NO_PROBE, fill_probe_indices,
};
use postretro_render_cpu::fluid_surface::reflection_cells::{
    FLUID_CELL_FACE_STRIDE, FLUID_CELL_PASS_UNIFORM_BYTES, FLUID_CELL_TILE, FLUID_CELL_TILE_STRIDE,
    FLUID_CELL_VERTEX_STRIDE, FluidCellAtlas, FluidCellPassUniform, FluidCellTileGpu,
    FluidReflectionCellLevers, build_cell_atlas, cell_dispatch, frustum_planes,
};
use postretro_render_cpu::fluid_surface::{FluidShaderVariant, FluidSurfaceMesh, FluidSurfaceSlots};

use super::fluid_reflection_probes::{
    FLUID_PROBE_ARRAY_BINDING, FLUID_PROBE_DISTANCE_BINDING, FLUID_PROBE_TABLE_BINDING,
    FluidProbeResources,
};

/// Group 4 binding of the sampled cell atlas the cells fragment loads.
pub(crate) const FLUID_CELL_ATLAS_BINDING: u32 = 4;
/// Compute-only group 4 bindings, past every binding the fluid module declares.
const FLUID_CELL_STORE_BINDING: u32 = 8;
const FLUID_CELL_FACES_BINDING: u32 = 9;
const FLUID_CELL_TILES_BINDING: u32 = 10;
const FLUID_CELL_PASS_BINDING: u32 = 11;
const FLUID_CELL_STATS_BINDING: u32 = 12;
/// The compute's group in the fluid module, and its pipeline layout's group count.
const FLUID_CELL_COMPUTE_GROUP: u32 = 4;

pub(crate) const FLUID_CELL_ATLAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The fourth vertex buffer's shader location.
pub(crate) const FLUID_CELL_VERTEX_LOCATION: u32 = 5;
/// Occlusion queries the stats frame can hold: wgpu's per-set maximum.
const FLUID_CELL_STATS_MAX_QUERIES: u32 = 4096;

/// The shader's override names for the debug levers.
pub(crate) const FLUID_REFLECTION_CELLS_OVERRIDE: &str = "FLUID_REFLECTION_CELLS";
pub(crate) const FLUID_REFLECTION_RATIO_ONE_OVERRIDE: &str = "FLUID_REFLECTION_RATIO_ONE";
const FLUID_CELLS_FRUSTUM_CULL_OVERRIDE: &str = "FLUID_CELLS_FRUSTUM_CULL";

pub(crate) const COMPUTE_SOURCE: &str = include_str!("../shaders/fluid_reflection_cells.wgsl");

pub(crate) const CELL_VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 1] = [wgpu::VertexAttribute {
    offset: 0,
    shader_location: FLUID_CELL_VERTEX_LOCATION,
    format: wgpu::VertexFormat::Uint32x4,
}];

/// The fragment-visible sampled atlas entry of group 4.
pub(crate) fn fluid_cell_atlas_layout_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: FLUID_CELL_ATLAS_BINDING,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// The reflecting pipeline's fragment overrides beyond `FLUID_REFLECTIONS`:
/// none for the shipped variant, so the default pipelines keep main's
/// constants exactly.
pub(crate) fn lever_constants(cells: bool, ratio_one: bool) -> Vec<(&'static str, f64)> {
    let mut constants = Vec::new();
    if cells {
        constants.push((FLUID_REFLECTION_CELLS_OVERRIDE, 1.0));
    }
    if ratio_one {
        constants.push((FLUID_REFLECTION_RATIO_ONE_OVERRIDE, 1.0));
    }
    constants
}

/// Index of a lever reflecting pipeline: (cells, ratio_one) other than the
/// shipped (false, false).
pub(crate) fn lever_pipeline_index(cells: bool, ratio_one: bool) -> Option<usize> {
    match (cells, ratio_one) {
        (false, false) => None,
        (true, false) => Some(0),
        (false, true) => Some(1),
        (true, true) => Some(2),
    }
}

/// Compute-visible group 4: the probe resources the fragment reads, then the
/// pass's own atlas store, faces, tiles, uniform and stats counter.
fn compute_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 8] {
    let array = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    };
    let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let uniform = |binding, size: usize| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(size as u64),
        },
        count: None,
    };
    [
        array(FLUID_PROBE_ARRAY_BINDING),
        uniform(FLUID_PROBE_TABLE_BINDING, FLUID_PROBE_TABLE_BYTES),
        array(FLUID_PROBE_DISTANCE_BINDING),
        wgpu::BindGroupLayoutEntry {
            binding: FLUID_CELL_STORE_BINDING,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: FLUID_CELL_ATLAS_FORMAT,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        },
        storage(FLUID_CELL_FACES_BINDING, true),
        storage(FLUID_CELL_TILES_BINDING, true),
        uniform(FLUID_CELL_PASS_BINDING, FLUID_CELL_PASS_UNIFORM_BYTES),
        storage(FLUID_CELL_STATS_BINDING, false),
    ]
}

/// The pass's pipelines, built on first use: one compute pipeline per
/// (ratio, cull) lever pair over one layout of groups 0 (camera), 3 (SH,
/// already compute-visible) and the compute-only group 4.
pub(crate) struct FluidCellPipelines {
    bind_group_layout: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    module: Option<wgpu::ShaderModule>,
    /// Indexed `ratio_one * 2 + frustum_cull`.
    compute: [Option<wgpu::ComputePipeline>; 4],
}

impl FluidCellPipelines {
    pub(crate) fn new(
        device: &wgpu::Device,
        camera_bgl: &wgpu::BindGroupLayout,
        sh_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Fluid Reflection Cell BGL (group 4, compute)"),
            entries: &compute_bind_group_layout_entries(),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Fluid Reflection Cell Pipeline Layout"),
            bind_group_layouts: &[
                Some(camera_bgl),
                None,
                None,
                Some(sh_bgl),
                Some(&bind_group_layout),
            ],
            immediate_size: 0,
        });
        Self {
            bind_group_layout,
            layout,
            module: None,
            compute: [None, None, None, None],
        }
    }

    /// Build the compute pipeline for `levers` if it is not yet built.
    pub(crate) fn ensure(&mut self, device: &wgpu::Device, shader_source: &str, levers: FluidReflectionCellLevers) {
        let index = usize::from(levers.ratio_one) * 2 + usize::from(levers.frustum_cull);
        if self.compute[index].is_some() {
            return;
        }
        let module = self.module.get_or_insert_with(|| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Fluid Reflection Cell Shader"),
                source: wgpu::ShaderSource::Wgsl(
                    format!("{shader_source}\n{COMPUTE_SOURCE}\n").into(),
                ),
            })
        });
        let constants = [
            (
                FLUID_REFLECTION_RATIO_ONE_OVERRIDE,
                if levers.ratio_one { 1.0 } else { 0.0 },
            ),
            (
                FLUID_CELLS_FRUSTUM_CULL_OVERRIDE,
                if levers.frustum_cull { 1.0 } else { 0.0 },
            ),
        ];
        self.compute[index] = Some(device.create_compute_pipeline(
            &wgpu::ComputePipelineDescriptor {
                label: Some("Fluid Reflection Cell Pipeline"),
                layout: Some(&self.layout),
                module,
                entry_point: Some("cs_reflection_cells"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
                cache: None,
            },
        ));
    }

    fn pipeline(&self, levers: FluidReflectionCellLevers) -> Option<&wgpu::ComputePipeline> {
        self.compute[usize::from(levers.ratio_one) * 2 + usize::from(levers.frustum_cull)].as_ref()
    }
}

/// This frame's cell pass, as prepared: nothing dispatches at zero tiles.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct FluidCellFrame {
    pub tiles: u32,
    pub groups: (u32, u32),
    pub cells: u64,
    /// Reflecting draws this frame, the stats frame's occlusion queries.
    pub reflecting_draws: u32,
    /// The frame's reflecting draws take the cells pipeline.
    pub cells_variant: bool,
}

/// One installed level's atlas, vertex words and pass resources. Drops with
/// the level's other surface resources.
pub(crate) struct InstalledFluidCells {
    layout: FluidCellAtlas,
    // Held so the level owns the texture's life; tests write it.
    _atlas: wgpu::Texture,
    atlas_view: wgpu::TextureView,
    vertex_buffer: wgpu::Buffer,
    tiles_buffer: wgpu::Buffer,
    pass_buffer: wgpu::Buffer,
    stats_buffer: wgpu::Buffer,
    /// `None` without an atlas face: the pass never runs.
    compute_bind_group: Option<wgpu::BindGroup>,
    /// Reused per frame.
    frame_tiles: Vec<FluidCellTileGpu>,
    frame: FluidCellFrame,
    /// Test hook: the atlas holds test-written texels; no pass runs.
    #[cfg(test)]
    skip_pass: bool,
}

impl InstalledFluidCells {
    /// Lay out and allocate the atlas for `mesh`. Called from the surface
    /// install, beside the probe resources it references.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        device: &wgpu::Device,
        volumes: &[EnvironmentVolumeRecord],
        mesh: &FluidSurfaceMesh,
        reflection_probes: Option<&ReflectionProbesSection>,
        probes: &FluidProbeResources,
        texture_dimensions: &[[u32; 2]],
        material_count: usize,
        pipelines: &FluidCellPipelines,
    ) -> Self {
        let mut probe_indices = vec![FLUID_REFLECTION_NO_PROBE; mesh.vertices.len()];
        fill_probe_indices(volumes, &mesh.face_drift_maps, reflection_probes, &mut probe_indices);
        let layout = build_cell_atlas(
            volumes,
            mesh,
            &probe_indices,
            texture_dimensions,
            material_count,
            device.limits().max_texture_dimension_2d,
        );
        let (width, height) = if layout.is_empty() {
            (1, 1)
        } else {
            (layout.width, layout.height)
        };
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Fluid Reflection Cell Atlas"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FLUID_CELL_ATLAS_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_view = atlas.create_view(&wgpu::TextureViewDescriptor::default());
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fluid Reflection Cell Vertex Buffer"),
            contents: bytemuck::cast_slice(&layout.vertex_words),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let faces_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fluid Reflection Cell Faces"),
            contents: if layout.faces.is_empty() {
                &[0u8; FLUID_CELL_FACE_STRIDE]
            } else {
                bytemuck::cast_slice(&layout.faces)
            },
            usage: wgpu::BufferUsages::STORAGE,
        });
        // Sized for every face's tiles drawn at once, so a frame never grows it.
        let tiles_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fluid Reflection Cell Tiles"),
            size: (layout.tiles.len().max(1) * FLUID_CELL_TILE_STRIDE) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pass_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fluid Reflection Cell Pass Uniform"),
            size: FLUID_CELL_PASS_UNIFORM_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let stats_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fluid Reflection Cell Stats"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let compute_bind_group = (!layout.is_empty()).then(|| {
            let [probe_array, probe_table, probe_distance] = probes.bind_group_entries();
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Fluid Reflection Cell Bind Group (group 4, compute)"),
                layout: &pipelines.bind_group_layout,
                entries: &[
                    probe_array,
                    probe_table,
                    probe_distance,
                    wgpu::BindGroupEntry {
                        binding: FLUID_CELL_STORE_BINDING,
                        resource: wgpu::BindingResource::TextureView(&atlas_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: FLUID_CELL_FACES_BINDING,
                        resource: faces_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: FLUID_CELL_TILES_BINDING,
                        resource: tiles_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: FLUID_CELL_PASS_BINDING,
                        resource: pass_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: FLUID_CELL_STATS_BINDING,
                        resource: stats_buffer.as_entire_binding(),
                    },
                ],
            })
        });
        log::info!(
            "[Renderer] Fluid cell spike atlas: {}×{} ({} B), {} faces, {} skipped, {} tiles",
            layout.width,
            layout.height,
            layout.bytes(),
            layout.faces.len(),
            layout.skipped_faces,
            layout.tiles.len(),
        );
        let frame_tiles = Vec::with_capacity(layout.tiles.len());
        Self {
            layout,
            _atlas: atlas,
            atlas_view,
            vertex_buffer,
            tiles_buffer,
            pass_buffer,
            stats_buffer,
            compute_bind_group,
            frame_tiles,
            frame: FluidCellFrame::default(),
            #[cfg(test)]
            skip_pass: false,
        }
    }

    /// Group 4's sampled atlas entry.
    pub(crate) fn atlas_entry(&self) -> wgpu::BindGroupEntry<'_> {
        wgpu::BindGroupEntry {
            binding: FLUID_CELL_ATLAS_BINDING,
            resource: wgpu::BindingResource::TextureView(&self.atlas_view),
        }
    }

    pub(crate) fn vertex_buffer(&self) -> &wgpu::Buffer {
        &self.vertex_buffer
    }

    pub(crate) fn frame(&self) -> FluidCellFrame {
        self.frame
    }

    pub(crate) fn layout(&self) -> &FluidCellAtlas {
        &self.layout
    }

    /// Prepare this frame's pass from the frame's draws: the reflecting slots'
    /// tiles, each with its reflectivity, and the pass uniform. `cells_on` is
    /// the cells lever in pixel style. Allocation-free once warm.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
        slots: &FluidSurfaceSlots,
        light_term_mask: u32,
        cells_on: bool,
        view_proj: glam::Mat4,
        extent: [u32; 2],
        max_workgroups: u32,
        stats: bool,
    ) {
        let reflecting = || {
            slots
                .frame_draws(light_term_mask)
                .filter(|(_, variant)| *variant == FluidShaderVariant::Reflecting)
        };
        let reflecting_draws = reflecting().count() as u32;
        self.frame = FluidCellFrame {
            reflecting_draws,
            cells_variant: cells_on,
            ..FluidCellFrame::default()
        };
        #[cfg(test)]
        if self.skip_pass {
            return;
        }
        if !cells_on || self.compute_bind_group.is_none() || reflecting_draws == 0 {
            return;
        }
        let cells = self.layout.fill_frame_tiles(
            reflecting().map(|(slot, _)| (slot, slots.row(slot).reflectivity())),
            &mut self.frame_tiles,
        );
        let tiles = self.frame_tiles.len() as u32;
        if tiles == 0 {
            return;
        }
        let groups = cell_dispatch(tiles, max_workgroups);
        queue.write_buffer(&self.tiles_buffer, 0, bytemuck::cast_slice(&self.frame_tiles));
        let uniform = FluidCellPassUniform {
            planes: frustum_planes(view_proj).map(|plane| plane.to_array()),
            extent: [extent[0] as f32, extent[1] as f32],
            tile_count: tiles,
            groups_x: groups.0,
            stats: u32::from(stats),
            _pad: [0; 3],
        };
        queue.write_buffer(&self.pass_buffer, 0, bytemuck::bytes_of(&uniform));
        self.frame.tiles = tiles;
        self.frame.groups = groups;
        self.frame.cells = cells;
    }

    /// Record the pass, if this frame prepared one, as its own compute pass.
    /// `stats` clears the evaluated-cell counter first.
    pub(crate) fn record(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipelines: &FluidCellPipelines,
        levers: FluidReflectionCellLevers,
        camera_bind_group: &wgpu::BindGroup,
        sh_bind_group: &wgpu::BindGroup,
        stats: bool,
    ) {
        if stats {
            encoder.clear_buffer(&self.stats_buffer, 0, None);
        }
        let (Some(bind_group), Some(pipeline)) =
            (self.compute_bind_group.as_ref(), pipelines.pipeline(levers))
        else {
            return;
        };
        if self.frame.tiles == 0 {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Fluid Reflection Cell Pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, camera_bind_group, &[]);
        pass.set_bind_group(3, sh_bind_group, &[]);
        pass.set_bind_group(FLUID_CELL_COMPUTE_GROUP, bind_group, &[]);
        pass.dispatch_workgroups(self.frame.groups.0, self.frame.groups.1, 1);
    }

    /// Test hook: write `texels` (RGBA16F, row-major over the whole atlas)
    /// into the atlas and stop the pass from overwriting them.
    #[cfg(test)]
    pub(crate) fn write_atlas_for_test(&mut self, queue: &wgpu::Queue, texels: &[u8]) {
        let size = self._atlas.size();
        queue.write_texture(
            self._atlas.as_image_copy(),
            texels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.width * 8),
                rows_per_image: Some(size.height),
            },
            size,
        );
        self.skip_pass = true;
    }
}

/// The stats frame's GPU side: an occlusion query per reflecting draw, their
/// resolve, and one readback of the counter and the query results.
pub(crate) struct FluidCellStatsGpu {
    query_set: wgpu::QuerySet,
    query_count: u32,
    resolve_buffer: wgpu::Buffer,
    readback_buffer: wgpu::Buffer,
    /// Queries the armed frame wrote, and whether its readback is encoded.
    written: u32,
    encoded: bool,
}

impl FluidCellStatsGpu {
    pub(crate) fn new(device: &wgpu::Device, slot_count: u32) -> Self {
        let query_count = slot_count.clamp(1, FLUID_CELL_STATS_MAX_QUERIES);
        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("Fluid Reflection Cell Stats Occlusion"),
            ty: wgpu::QueryType::Occlusion,
            count: query_count,
        });
        let resolve_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fluid Reflection Cell Stats Resolve"),
            size: u64::from(query_count) * 8,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fluid Reflection Cell Stats Readback"),
            size: 8 + u64::from(query_count) * 8,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            query_set,
            query_count,
            resolve_buffer,
            readback_buffer,
            written: 0,
            encoded: false,
        }
    }

    pub(crate) fn query_set(&self) -> &wgpu::QuerySet {
        &self.query_set
    }

    pub(crate) fn query_count(&self) -> u32 {
        self.query_count
    }

    /// Resolve the frame's queries and copy them and the counter out.
    pub(crate) fn encode_readback(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        cells: &InstalledFluidCells,
        written: u32,
    ) {
        let written = written.min(self.query_count);
        encoder.copy_buffer_to_buffer(&cells.stats_buffer, 0, &self.readback_buffer, 0, 4);
        if written > 0 {
            encoder.resolve_query_set(&self.query_set, 0..written, &self.resolve_buffer, 0);
            encoder.copy_buffer_to_buffer(
                &self.resolve_buffer,
                0,
                &self.readback_buffer,
                8,
                u64::from(written) * 8,
            );
        }
        self.written = written;
        self.encoded = true;
    }

    /// Map the readback after the frame's submission: the evaluated-cell
    /// count and the summed occlusion samples. `None` if nothing was encoded.
    pub(crate) fn read(&mut self, device: &wgpu::Device) -> Option<(u32, u64)> {
        if !self.encoded {
            return None;
        }
        self.encoded = false;
        let slice = self.readback_buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let (evaluated, samples) = {
            let data = slice.get_mapped_range().ok()?;
            let evaluated = u32::from_le_bytes(data[0..4].try_into().ok()?);
            let samples = data[8..8 + self.written as usize * 8]
                .chunks_exact(8)
                .map(|chunk| u64::from_le_bytes(chunk.try_into().unwrap_or([0; 8])))
                .sum::<u64>();
            (evaluated, samples)
        };
        self.readback_buffer.unmap();
        Some((evaluated, samples))
    }
}

/// The stats a stats-armed frame reports (capture `fluid_reflection_cell_stats`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidReflectionCellStats {
    pub levers: FluidReflectionCellLevers,
    /// The frame drew pixel style (the cells lever acts only there).
    pub pixel_style: bool,
    pub atlas_width: u32,
    pub atlas_height: u32,
    pub atlas_bytes: u64,
    pub faces_in_atlas: u32,
    pub faces_skipped: u32,
    /// Reflecting draws (volumes) this frame.
    pub reflecting_draws: u32,
    /// Tiles and threads the pass dispatched (0 when it did not run).
    pub tiles_dispatched: u32,
    pub threads_dispatched: u64,
    /// Cells of the drawn reflecting faces: the sum of their rect areas.
    pub cells_dispatched: u64,
    /// Cells the pass evaluated, counted on the GPU: equals dispatched cells
    /// without frustum culling.
    pub cells_evaluated: u32,
    /// Samples passing the depth test over the reflecting draws, by
    /// occlusion query: reflecting fluid fragments (overlapping faces count
    /// once each).
    pub reflecting_fragments: u64,
    /// Reflecting draws past the query set's capacity, uncounted above.
    pub uncounted_draws: u32,
}

impl FluidReflectionCellStats {
    pub(crate) fn threads(tiles: u32) -> u64 {
        u64::from(tiles) * u64::from(FLUID_CELL_TILE * FLUID_CELL_TILE)
    }
}

/// The fourth vertex buffer's layout.
pub(crate) fn cell_vertex_buffer_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: FLUID_CELL_VERTEX_STRIDE as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &CELL_VERTEX_ATTRIBUTES,
    }
}
