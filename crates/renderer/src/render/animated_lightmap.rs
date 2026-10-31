// Animated-lightmap compose compute pass.
// See: context/lib/rendering_pipeline.md §4, §7.1
//
// The atlas is zero-initialized by wgpu at creation; the compose pass writes
// every texel the forward pass samples, so no per-frame clear is needed.
//
// Visibility: only tiles whose owning chunk belongs to a visible cell are
// dispatched. Any future pass that samples this atlas (reflection probes,
// alternate cameras) must share the same `VisibleCells` or skip animated-lit
// chunks — otherwise it reads stale atlas contents for invisible cells.
//
// Dispatch shape: one workgroup per 8×8 tile, laid out as a row-major 2D grid
// (`compose_grid`) so tile counts past `max_compute_workgroups_per_dimension`
// still dispatch. The last row is padded with `PADDING_TILE` records.

use crate::render::gpu_create::GpuCreate;
#[cfg(test)]
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
#[cfg(test)]
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
pub use postretro_render_cpu::animated_lightmap::AnimatedLmDebugConfig;
#[cfg(test)]
use postretro_render_cpu::animated_lightmap::plan::installed_block_table_section;
#[cfg(test)]
use postretro_render_cpu::animated_lightmap::plan::plan_animated_lightmap;
use postretro_render_cpu::animated_lightmap::plan::{
    AnimatedLightmapPlan, DispatchTile, compose_grid, pack_dispatch_tiles_into, pad_tiles_to_grid,
};
#[cfg(test)]
use postretro_render_cpu::animated_lightmap::plan::{
    PADDING_TILE, animated_atlas_extent, animated_atlas_preflight, build_chunk_cell_ids,
    expand_dispatch_tiles, validate_weight_map_section,
};

use crate::compute_cull::{MAX_VISIBLE_CELLS, VISIBLE_CELLS_WORDS};
#[cfg(test)]
use postretro_render_data::geometry::BvhLeaf;
use postretro_visibility::VisibleCells;

use super::install_pipelines::new_install_compute_pipeline;
use super::residency::{ResidencyAllocation, ResidencyAllocationState, texture_row};
use super::sh_volume::AnimatedLightBuffers;
use super::{LIGHTMAP_ANIMATED_DIRECTION, LIGHTMAP_ANIMATED_IRRADIANCE};
use postretro_level_format::SectionId;

fn animated_atlas_view_dimension() -> wgpu::TextureViewDimension {
    wgpu::TextureViewDimension::D2Array
}

/// The compose pass's device-only half, built once per renderer (see
/// `InstallPipelines`). Level halves hold clones of these handles.
#[derive(Clone)]
pub(crate) struct AnimatedLightmapPipelines {
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}

impl AnimatedLightmapPipelines {
    /// `uniform_bind_group_layout` must include `wgpu::ShaderStages::COMPUTE` —
    /// the compose pipeline is a compute pipeline and wgpu validation fails at
    /// `create_compute_pipeline` time otherwise. The canonical BGL in
    /// `render/mod.rs` declares `VERTEX | FRAGMENT | COMPUTE` for this reason;
    /// dropping COMPUTE there would break this pass. `wgpu::BindGroupLayout` is
    /// opaque so this cannot be runtime-checked — it must be preserved at the
    /// call site.
    pub(crate) fn new(
        device: &wgpu::Device,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Animated LM Compute BGL"),
            entries: &compute_bgl_entries(),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Animated LM Pipeline Layout"),
            bind_group_layouts: &[Some(uniform_bind_group_layout), Some(&bind_group_layout)],
            immediate_size: 0,
        });

        // curve_eval.wgsl is appended rather than imported; matches the pattern in forward.wgsl.
        let shader_source = concat!(
            include_str!("../shaders/animated_lightmap_compose.wgsl"),
            "\n",
            include_str!("../shaders/curve_eval.wgsl"),
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Animated LM Compose Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        let pipeline = new_install_compute_pipeline(
            device,
            &wgpu::ComputePipelineDescriptor {
                label: Some("Animated LM Compose Pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("compose_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            },
        );
        Self {
            bind_group_layout,
            pipeline,
        }
    }
}

/// Compose-pass resources. Always allocated — maps with no
/// `AnimatedLightWeightMaps` section get a 1×1 zero dummy atlas and skip the
/// per-frame dispatch.
pub struct AnimatedLightmapResources {
    /// `None` when no weight-map section is present; the dummy view is bound instead.
    atlas_texture: Option<wgpu::Texture>,
    /// Per-texel fused dominant-direction atlas. `None` on the no-weight-maps
    /// path; the direction dummy view is bound instead.
    direction_atlas_texture: Option<wgpu::Texture>,
    dummy_texture: wgpu::Texture,
    /// 1×1 zero direction placeholder, its own texture so the meter reports
    /// the direction slot's placeholder bytes apart from the irradiance one.
    dummy_direction_texture: wgpu::Texture,
    /// Meter state of the bound pair, on the SH ledger's conventions: Data for
    /// a real atlas or an accepted section with nothing to compose, Fallback
    /// when a supplied section could not be used, Dummy when none was supplied.
    residency_state: ResidencyAllocationState,
    /// Whether the level supplied section 25; the rows then cite it.
    section_supplied: bool,
    /// Bound to the forward-pass lightmap bind group. Points at `atlas_texture`
    /// when present, otherwise at `dummy_texture` — keeps the bind-group layout constant.
    pub forward_view: wgpu::TextureView,
    /// Bound to the forward-pass lightmap bind group alongside `forward_view`.
    /// Points at `direction_atlas_texture` when present, otherwise at the
    /// direction dummy view — keeps the bind-group layout constant.
    pub direction_forward_view: wgpu::TextureView,

    /// `None` on maps with no weight maps; `dispatch` is a no-op in that case.
    dispatch_state: Option<DispatchState>,
}

struct DispatchState {
    compose_pipeline: wgpu::ComputePipeline,
    compute_bind_group: wgpu::BindGroup,
    /// Sized to the master tile count plus the largest grid padding; updated
    /// each frame with the visibility-culled, grid-padded prefix via
    /// `queue.write_buffer`. Needs `COPY_DST`.
    dispatch_tiles_buffer: wgpu::Buffer,
    /// Device `max_compute_workgroups_per_dimension`, the per-axis bound for
    /// `compose_grid`.
    max_workgroups_per_dim: u32,
    /// Full unfiltered tile list built at load time. Per-frame cull walks
    /// this and pushes only tiles in visible cells.
    master_tiles: Vec<DispatchTile>,
    /// `cell_id` of the BVH leaf that owns each animated chunk (indexed
    /// parallel to `section.chunk_rects`). Built from `BvhLeaf.chunk_range_*`
    /// at load time; unreferenced chunks keep `u32::MAX` and are always culled.
    chunk_cell_ids: Vec<u32>,
    /// Reused each frame to avoid per-frame allocation.
    scratch_tiles: Vec<DispatchTile>,
    scratch_bytes: Vec<u8>,
    /// Previous frame's kept-tile count. Deduplicates the debug log.
    /// `u32::MAX` sentinel forces the first frame to log.
    prev_kept: u32,
    /// Cached master tile count so the logger and `DrawAll` path skip `.len()`.
    total_tiles: u32,
}

impl AnimatedLightmapResources {
    /// Build the non-dispatching dummy: the absent-section plan. Load
    /// failures use this instead of retaining a prior level's atlas views or
    /// compose state. It needs no pipeline: the dummy never dispatches.
    pub(crate) fn dummy(
        device: &wgpu::Device,
        animation: &AnimatedLightBuffers,
        debug_config: AnimatedLmDebugConfig,
    ) -> Self {
        Self::from_plan(
            device,
            AnimatedLightmapPlan::Absent,
            animation,
            None,
            debug_config,
        )
        .expect("the absent-section plan is the non-failing animated-lightmap dummy path")
    }

    /// Plan on this device's limits, then build: the main-thread oracle path.
    ///
    /// `static_blocks` — each installed lightmap cell block's `(width,
    /// height)` by block id, from `StaticPool::static_block_extents`.
    /// Section 25's blocks must lie inside their cell blocks; the compact
    /// atlas itself is `page_size² × compact_layers`. `None` is placeholder
    /// mode (no id 22, zero blocks, or a pool the device rejected): the
    /// animated path takes the dummy-atlas early-out, since the block table
    /// has no block frame to translate from.
    ///
    /// Returns `Err` on validation or allocation preflight failure; callers log
    /// and bind the non-dispatching dummy resource for this level.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        weight_maps: Option<&AnimatedLightWeightMapsSection>,
        animated_chunks: Option<&AnimatedLightChunksSection>,
        bvh_leaves: &[BvhLeaf],
        animation: &AnimatedLightBuffers,
        pipelines: &AnimatedLightmapPipelines,
        static_blocks: Option<&[(u32, u32)]>,
        debug_config: AnimatedLmDebugConfig,
    ) -> Result<Self, String> {
        let plan = plan_animated_lightmap(
            weight_maps,
            animated_chunks,
            bvh_leaves,
            animation.animated_light_count(),
            static_blocks,
            &super::install_prep::device_limits(&device.limits()),
        )?;
        Self::from_plan(device, plan, animation, Some(pipelines), debug_config)
    }

    /// The GPU half: create the atlases, buffers and bind group `plan` sized.
    /// Only a dispatching atlas reaches the pipeline, and with none it errs.
    pub(crate) fn from_plan(
        device: &wgpu::Device,
        plan: AnimatedLightmapPlan,
        animation: &AnimatedLightBuffers,
        pipelines: Option<&AnimatedLightmapPipelines>,
        debug_config: AnimatedLmDebugConfig,
    ) -> Result<Self, String> {
        let dummy_texture = create_zero_texture(
            device,
            "Animated LM Dummy",
            wgpu::TextureFormat::Rgba16Float,
        );
        let dummy_view = dummy_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Animated LM Dummy Forward View"),
            dimension: Some(animated_atlas_view_dimension()),
            ..Default::default()
        });
        // Separate 1×1 zero texture for the direction atlas slot so the forward
        // bind group stays valid on the empty-map path.
        let dummy_direction_texture = create_zero_texture(
            device,
            "Animated LM Dummy Direction",
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let dummy_direction_view =
            dummy_direction_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("Animated LM Dummy Direction Forward View"),
                dimension: Some(animated_atlas_view_dimension()),
                ..Default::default()
            });

        let dummy_path = |residency_state, section_supplied| Self {
            atlas_texture: None,
            direction_atlas_texture: None,
            dummy_texture: dummy_texture.clone(),
            dummy_direction_texture: dummy_direction_texture.clone(),
            residency_state,
            section_supplied,
            forward_view: dummy_view.clone(),
            direction_forward_view: dummy_direction_view.clone(),
            dispatch_state: None,
        };
        let atlas = match plan {
            AnimatedLightmapPlan::Absent => {
                return Ok(dummy_path(ResidencyAllocationState::Dummy, false));
            }
            AnimatedLightmapPlan::Empty => {
                return Ok(dummy_path(ResidencyAllocationState::Data, true));
            }
            AnimatedLightmapPlan::NoBlockFrame => {
                return Ok(dummy_path(ResidencyAllocationState::Fallback, true));
            }
            AnimatedLightmapPlan::Atlas(atlas) => *atlas,
        };
        let atlas_extent = atlas.extent;
        let atlas_bytes = atlas.atlas_bytes;
        let tile_capacity = atlas.tile_capacity;

        // No `COPY_DST` needed — wgpu zero-initializes and the compose pass
        // overwrites every texel the forward pass will sample.
        let atlas_texture = device.new_texture(&wgpu::TextureDescriptor {
            label: Some("Animated LM Atlas"),
            // One array layer per compact page; pages are square.
            size: wgpu::Extent3d {
                width: atlas_extent.width,
                height: atlas_extent.height,
                depth_or_array_layers: atlas_extent.depth_or_array_layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let forward_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Animated LM Forward View"),
            dimension: Some(animated_atlas_view_dimension()),
            ..Default::default()
        });
        let storage_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Animated LM Storage View"),
            dimension: Some(animated_atlas_view_dimension()),
            ..Default::default()
        });

        // Per-texel fused dominant-direction atlas: octahedral direction in `.rg`
        // (matching the static direction atlas) + coverage flag in `.a`, so
        // `Rgba8Unorm` (4 B/texel) suffices — half the VRAM of the irradiance
        // atlas. `direction_forward_view` is bound at group-4 binding 5 (forward
        // pass); the compose-side storage binding 8 writes this same atlas —
        // independent numbering spaces.
        let direction_atlas_texture = device.new_texture(&wgpu::TextureDescriptor {
            label: Some("Animated LM Direction Atlas"),
            size: wgpu::Extent3d {
                width: atlas_extent.width,
                height: atlas_extent.height,
                depth_or_array_layers: atlas_extent.depth_or_array_layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        // VRAM footprint of the two compose-target atlases (irradiance 8 B/texel
        // + direction 4 B/texel).
        log::info!(
            "[Renderer] Animated lightmap atlases {}x{}x{} pages, ~{} MiB VRAM (Rgba16Float irradiance + Rgba8Unorm direction)",
            atlas_extent.width,
            atlas_extent.height,
            atlas_extent.depth_or_array_layers,
            atlas_bytes / (1024 * 1024),
        );

        let direction_forward_view =
            direction_atlas_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("Animated LM Direction Forward View"),
                dimension: Some(animated_atlas_view_dimension()),
                ..Default::default()
            });
        let direction_storage_view =
            direction_atlas_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("Animated LM Direction Storage View"),
                dimension: Some(animated_atlas_view_dimension()),
                ..Default::default()
            });

        let chunk_rects_buffer =
            create_storage_buffer(device, "Animated LM Chunk Rects", &atlas.chunk_rects_bytes);
        let offset_counts_buffer = create_storage_buffer(
            device,
            "Animated LM Offset Counts",
            &atlas.offset_counts_bytes,
        );
        let texel_lights_buffer = create_storage_buffer(
            device,
            "Animated LM Texel Lights",
            &atlas.texel_lights_bytes,
        );
        // Seeded with the full master list padded to capacity; every frame
        // overwrites the prefix its grid reads, so no separate clear is needed.
        let dispatch_tiles_buffer = {
            device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Animated LM Dispatch Tiles"),
                contents: &atlas.dispatch_tiles_bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            })
        };

        let debug_buffer = {
            device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Animated LM Debug Config"),
                contents: &debug_config.to_uniform_bytes(),
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };

        let Some(pipelines) = pipelines else {
            return Err("animated lightmap compose was built without its pipeline".to_owned());
        };
        let compose_pipeline = pipelines.pipeline.clone();

        let compute_bind_group = device.new_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Animated LM Compute Bind Group"),
            layout: &pipelines.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: chunk_rects_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: offset_counts_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: texel_lights_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: dispatch_tiles_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: animation.descriptors.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: animation.anim_samples.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&storage_view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: debug_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(&direction_storage_view),
                },
            ],
        });

        log::info!(
            "[Renderer] Animated lightmap: {} chunks, {} covered texels, {} weight entries, {} dispatch tiles",
            atlas.chunk_count,
            atlas.covered_texels,
            atlas.weight_entries,
            atlas.master_tiles.len(),
        );

        // The plan proved the master count fits `u32` and its grid.
        let total_tiles = atlas.master_tiles.len() as u32;
        Ok(Self {
            atlas_texture: Some(atlas_texture),
            direction_atlas_texture: Some(direction_atlas_texture),
            dummy_texture,
            dummy_direction_texture,
            residency_state: ResidencyAllocationState::Data,
            section_supplied: true,
            forward_view,
            direction_forward_view,
            dispatch_state: Some(DispatchState {
                compose_pipeline,
                compute_bind_group,
                dispatch_tiles_buffer,
                max_workgroups_per_dim: atlas.max_workgroups_per_dim,
                // Pre-sized so `DrawAll` on the first frame doesn't realloc.
                scratch_tiles: Vec::with_capacity(tile_capacity),
                scratch_bytes: Vec::with_capacity(atlas.dispatch_tiles_bytes.len()),
                master_tiles: atlas.master_tiles,
                chunk_cell_ids: atlas.chunk_cell_ids,
                prev_kept: u32::MAX,
                total_tiles,
            }),
        })
    }

    /// The meter state, section flag, atlas extent and dispatch tables this
    /// resource was built with, for the prep-equivalence tests.
    #[cfg(test)]
    pub(crate) fn plan_summary(&self) -> String {
        format!(
            "{:?}",
            (
                self.residency_state,
                self.section_supplied,
                self.atlas_texture.as_ref().map(wgpu::Texture::size),
                self.dispatch_state.as_ref().map(|state| {
                    (
                        &state.master_tiles,
                        &state.chunk_cell_ids,
                        state.total_tiles,
                        state.max_workgroups_per_dim,
                    )
                }),
            )
        )
    }

    /// Mark a dummy installed because construction of a supplied section
    /// failed, so the meter reports the placeholder as that section's
    /// fallback rather than an absent section.
    pub(crate) fn into_fallback(mut self) -> Self {
        self.residency_state = ResidencyAllocationState::Fallback;
        self.section_supplied = true;
        self
    }

    /// Animated irradiance and direction meter rows for the pair actually
    /// bound: the real atlases when present, otherwise their placeholders.
    pub(crate) fn residency_rows(&self) -> [ResidencyAllocation; 2] {
        let sources: &[u16] = if self.section_supplied {
            &[SectionId::AnimatedLightWeightMaps as u16]
        } else {
            &[]
        };
        [
            texture_row(
                LIGHTMAP_ANIMATED_IRRADIANCE,
                self.atlas_texture.as_ref().unwrap_or(&self.dummy_texture),
                sources,
                self.residency_state,
            ),
            texture_row(
                LIGHTMAP_ANIMATED_DIRECTION,
                self.direction_atlas_texture
                    .as_ref()
                    .unwrap_or(&self.dummy_direction_texture),
                sources,
                self.residency_state,
            ),
        ]
    }

    /// Returns `false` on maps with no animated weight maps. Callers skip
    /// allocating a GPU timing pair so the timestamp slot isn't left
    /// marked-but-unwritten.
    pub fn is_active(&self) -> bool {
        self.dispatch_state.is_some()
    }

    /// Dispatch the per-frame compose pass.
    ///
    /// No-op when the map has no animated weight maps. Filters the master tile
    /// list against `visible`; skips encoding entirely when all animated chunks
    /// are off-screen (safe because the forward pass won't sample those texels).
    ///
    /// `uniform_bind_group` is the renderer's group-0 bind group; this pass
    /// reads `uniforms.time` to drive animation curves.
    ///
    /// When the dispatch is skipped, `timestamp_writes` goes
    /// marked-but-unwritten; the frame's timing prefill makes that pair decode
    /// as absent.
    pub fn dispatch(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        visible: &VisibleCells,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        let Some(state) = &mut self.dispatch_state else {
            return;
        };

        state.scratch_tiles.clear();
        match visible {
            VisibleCells::DrawAll => {
                state.scratch_tiles.extend_from_slice(&state.master_tiles);
            }
            VisibleCells::Culled(cells) => {
                // Local bitmask is cheaper than a HashSet for typical cell counts (dozens).
                let mut bitmask = [0u32; VISIBLE_CELLS_WORDS];
                for &cell in cells {
                    if cell >= MAX_VISIBLE_CELLS {
                        // `compute_cull::write_bitmask_from_cells` already logs this; stay quiet.
                        continue;
                    }
                    let word = (cell >> 5) as usize;
                    let bit = 1u32 << (cell & 31);
                    bitmask[word] |= bit;
                }
                for tile in &state.master_tiles {
                    let cell = state.chunk_cell_ids[tile.chunk_idx as usize];
                    if cell >= MAX_VISIBLE_CELLS {
                        continue;
                    }
                    let word = (cell >> 5) as usize;
                    let bit = 1u32 << (cell & 31);
                    if bitmask[word] & bit != 0 {
                        state.scratch_tiles.push(*tile);
                    }
                }
            }
        }

        let kept = state.scratch_tiles.len() as u32;
        let total = state.total_tiles;

        if kept != state.prev_kept {
            log::debug!("[Renderer] animated_lm tiles: {}/{} visible", kept, total);
            state.prev_kept = kept;
        }

        if kept == 0 {
            return;
        }

        // `kept` ≤ the master count validated at load, so the grid exists and
        // its padded length fits the tile buffer.
        let grid = compose_grid(kept, state.max_workgroups_per_dim)
            .expect("visible tile count is bounded by the load-validated master grid");
        pad_tiles_to_grid(&mut state.scratch_tiles, grid);

        state.scratch_bytes.clear();
        pack_dispatch_tiles_into(&state.scratch_tiles, &mut state.scratch_bytes);
        queue.write_buffer(&state.dispatch_tiles_buffer, 0, &state.scratch_bytes);

        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Animated LM Compose"),
            timestamp_writes,
        });
        pass.set_bind_group(0, uniform_bind_group, &[]);
        pass.set_bind_group(1, &state.compute_bind_group, &[]);

        pass.set_pipeline(&state.compose_pipeline);
        pass.dispatch_workgroups(grid.0, grid.1, 1);
    }
}

/// Resolve a construction attempt through the common renderer degradation
/// policy. The caller supplies the `weight_maps: None` dummy constructor, so
/// every failed load replaces its old views and dispatch state before rebinding
/// the new level's lightmap group.
pub(crate) fn with_dummy_fallback<T>(
    resources: Result<T, String>,
    dummy: impl FnOnce() -> T,
    install_context: &str,
) -> T {
    match resources {
        Ok(resources) => resources,
        Err(msg) => {
            log::error!(
                "[Renderer] {install_context} failed: {msg}; disabling animated-light contribution for this level"
            );
            dummy()
        }
    }
}

fn compute_bgl_entries() -> [wgpu::BindGroupLayoutEntry; 9] {
    let storage_read = wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Storage { read_only: true },
        has_dynamic_offset: false,
        min_binding_size: None,
    };
    [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: storage_read,
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: storage_read,
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: storage_read,
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: storage_read,
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: storage_read,
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 5,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: storage_read,
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 6,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba16Float,
                view_dimension: wgpu::TextureViewDimension::D2Array,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 7,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 8,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                // Direction atlas: octahedral in `.rg` + coverage in `.a`, so 8-bit
                // unorm suffices (half the irradiance atlas's footprint).
                format: wgpu::TextureFormat::Rgba8Unorm,
                view_dimension: wgpu::TextureViewDimension::D2Array,
            },
            count: None,
        },
    ]
}

fn create_zero_texture(
    device: &wgpu::Device,
    label: &str,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.new_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        // wgpu zero-initializes to (0,0,0,0); no upload needed.
        // `STORAGE_BINDING` required so the bind-group layout is compatible
        // with the real atlas slot when weight maps are absent.
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    })
}

fn create_storage_buffer(device: &wgpu::Device, label: &str, bytes: &[u8]) -> wgpu::Buffer {
    // wgpu rejects zero-sized storage buffers. All callers are gated behind the
    // early-out in `new` that bails when `chunk_rects` OR `texel_lights` is
    // empty, so by here every packed buffer is non-empty. (An all-SDF map emits
    // chunk rects + offset_counts but zero texel_lights — caught by that gate.)
    // Use `assert!` (not `debug_assert!`) so a future regression surfaces in
    // release builds.
    assert!(!bytes.is_empty(), "{label} storage buffer would be empty");
    device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytes,
        usage: wgpu::BufferUsages::STORAGE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;
    use postretro_level_format::animated_light_weight_maps::{
        AnimatedBlock, AnimatedLightWeightMapsSection, ChunkAtlasRect, TexelLight, TexelLightEntry,
    };
    use postretro_test_log_capture::LogCapture;

    fn mk_rect(w: u32, h: u32, offset: u32) -> ChunkAtlasRect {
        ChunkAtlasRect {
            compact_x: 0,
            compact_y: 0,
            width: w,
            height: h,
            texel_offset: offset,
            block: 0,
        }
    }

    fn mk_block(lightmap_block: u32, compact_layer: u32) -> AnimatedBlock {
        AnimatedBlock {
            lightmap_block,
            block_x: 0,
            block_y: 0,
            compact_x: 0,
            compact_y: 0,
            compact_layer,
            width: 16,
            height: 16,
        }
    }

    /// Two 1×1 chunks in two animated blocks keyed on lightmap cell blocks 2
    /// and 9, packed on pages 0 and 1 of a 16² atlas.
    fn two_page_weight_maps() -> AnimatedLightWeightMapsSection {
        let first = mk_rect(1, 1, 0);
        let second = ChunkAtlasRect {
            block: 1,
            ..mk_rect(1, 1, 1)
        };
        AnimatedLightWeightMapsSection {
            page_size: 16,
            compact_layers: 2,
            blocks: vec![mk_block(2, 0), mk_block(9, 1)],
            chunk_rects: vec![first, second],
            offset_counts: vec![
                TexelLightEntry {
                    offset: 0,
                    count: 0,
                };
                2
            ],
            texel_lights: Vec::new(),
        }
    }

    #[test]
    fn runtime_preflight_rejects_malformed_block_tables() {
        let mut past_table = two_page_weight_maps();
        past_table.chunk_rects[1].block = 2;
        let mut empty_page = two_page_weight_maps();
        empty_page.compact_layers = 3;
        let mut overlapping = two_page_weight_maps();
        overlapping.blocks[1].compact_layer = 0;
        overlapping.compact_layers = 1;
        for (label, section) in [
            ("block past the table", past_table),
            ("empty page", empty_page),
            ("overlapping blocks", overlapping),
        ] {
            let parsed = AnimatedLightWeightMapsSection::from_bytes(&section.to_bytes())
                .expect("malformed semantic metadata remains structurally parseable");
            assert!(
                validate_weight_map_section(&parsed).is_err(),
                "{label} must fail before renderer allocation",
            );
        }
        assert_eq!(validate_weight_map_section(&two_page_weight_maps()), Ok(()));
    }

    // Regression: a section over the block cap passed every load check and
    // then panicked the binding-7 table builder at level install.
    #[test]
    fn runtime_preflight_rejects_a_block_count_over_the_table_cap() {
        use postretro_level_format::animated_lightmap_atlas::ANIMATED_BLOCK_CAP;
        let mut section = two_page_weight_maps();
        let extra = section.blocks[1];
        section
            .blocks
            .resize(ANIMATED_BLOCK_CAP as usize + 1, extra);
        let error = validate_weight_map_section(&section)
            .expect_err("an over-cap section must fail before the block table is built");
        assert!(error.contains("block-table cap"), "{error}");
    }

    // Regression: a zero-area chunk with an unreferenced weight pool reached
    // storage-buffer allocation with no dispatch tiles and panicked at level load.
    #[test]
    fn runtime_preflight_rejects_zero_area_chunk_with_unreferenced_weight() {
        let section = AnimatedLightWeightMapsSection {
            page_size: 16,
            compact_layers: 1,
            blocks: vec![mk_block(0, 0)],
            chunk_rects: vec![mk_rect(0, 8, 0)],
            offset_counts: Vec::new(),
            texel_lights: vec![TexelLight {
                light_index: 0,
                weight: 1.0,
                direction_oct: [0, 0],
            }],
        };
        let parsed = AnimatedLightWeightMapsSection::from_bytes(&section.to_bytes())
            .expect("malformed geometry remains structurally parseable");

        assert!(
            parsed.is_consistent(),
            "fixture must pass format consistency"
        );
        assert!(
            validate_weight_map_section(&parsed).is_err(),
            "renderer validation must reject zero-area chunks before GPU allocation",
        );
    }

    #[test]
    fn compose_shader_parses_and_declares_debug_binding() {
        // Use naga to validate the same concatenated source the runtime builds,
        // so shader changes are caught without a GPU.
        let src = concat!(
            include_str!("../shaders/animated_lightmap_compose.wgsl"),
            "\n",
            include_str!("../shaders/curve_eval.wgsl"),
        );
        let module =
            naga::front::wgsl::parse_str(src).expect("compose shader should parse as WGSL");
        let has_clear = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "clear_main" && ep.stage == naga::ShaderStage::Compute);
        let has_compose = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "compose_main" && ep.stage == naga::ShaderStage::Compute);
        assert!(!has_clear, "clear_main should have been removed");
        assert!(has_compose, "compose_main missing");
        let has_debug_struct = module.types.iter().any(|(_, ty)| {
            matches!(&ty.inner, naga::TypeInner::Struct { .. })
                && ty.name.as_deref() == Some("DebugConfig")
        });
        assert!(has_debug_struct, "DebugConfig struct missing from shader");
    }

    /// `parse_str` skips uniformity analysis; the padding early-return must
    /// still pass full validation.
    #[test]
    fn compose_shader_passes_naga_validation() {
        let src = concat!(
            include_str!("../shaders/animated_lightmap_compose.wgsl"),
            "\n",
            include_str!("../shaders/curve_eval.wgsl"),
        );
        let module = naga::front::wgsl::parse_str(src).expect("compose shader must parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("compose shader must pass naga validation");
    }

    #[test]
    fn compose_shader_indexes_tiles_by_flattened_grid_and_skips_padding() {
        let src = include_str!("../shaders/animated_lightmap_compose.wgsl");
        assert!(src.contains("dispatch_tiles[wg.y * grid.x + wg.x]"));
        assert!(src.contains("const PADDING_TILE: u32 = 0xFFFFFFFFu;"));
        assert_eq!(PADDING_TILE.chunk_idx, 0xFFFF_FFFF);
        assert!(src.contains("if (tile.chunk_idx == PADDING_TILE)"));
    }

    #[test]
    fn compose_grid_is_one_row_within_the_per_dimension_limit() {
        assert_eq!(compose_grid(1, 65_535), Some((1, 1)));
        assert_eq!(compose_grid(65_535, 65_535), Some((65_535, 1)));
    }

    #[test]
    fn compose_grid_balances_rows_past_the_per_dimension_limit() {
        // The stress-warren-mini 0.08 m/texel bake that tripped the old cap.
        assert_eq!(compose_grid(242_074, 65_535), Some((60_519, 4)));
        assert_eq!(compose_grid(65_536, 65_535), Some((32_768, 2)));

        for (count, max) in [
            (242_074, 65_535),
            (65_536, 65_535),
            (1_000, 32),
            (40, 7),
            (49, 7),
        ] {
            let (columns, rows) = compose_grid(count, max).expect("count fits max²");
            assert!(columns <= max && rows <= max);
            let cells = columns * rows;
            assert!(cells >= count, "grid must hold every tile");
            assert!(cells - count < rows, "padding stays below one tile per row");
        }
    }

    #[test]
    fn compose_grid_rejects_counts_past_the_square_limit() {
        assert_eq!(compose_grid(49, 7), Some((7, 7)));
        assert_eq!(compose_grid(50, 7), None);
    }

    #[test]
    fn padding_fills_the_grid_after_every_real_tile() {
        let real = DispatchTile {
            chunk_idx: 3,
            tile_origin_x: 8,
            tile_origin_y: 16,
            target_page: 1,
        };
        let mut tiles = vec![real; 10];
        let grid = compose_grid(10, 4).expect("10 tiles fit a 4x4 limit");
        assert_eq!(grid, (4, 3));

        pad_tiles_to_grid(&mut tiles, grid);

        assert_eq!(tiles.len(), 12);
        assert!(tiles[..10].iter().all(|tile| *tile == real));
        assert!(tiles[10..].iter().all(|tile| *tile == PADDING_TILE));
    }

    #[test]
    fn compose_shader_uses_array_storage_and_page_indexed_stores() {
        let src = include_str!("../shaders/animated_lightmap_compose.wgsl");
        assert!(
            src.contains("texture_storage_2d_array<rgba16float, write>"),
            "animated irradiance compose target must be a storage texture array",
        );
        assert!(
            src.contains("texture_storage_2d_array<rgba8unorm, write>"),
            "animated direction compose target must be a storage texture array",
        );
        assert_eq!(
            src.matches("i32(tile.target_page)").count(),
            3,
            "debug, irradiance, and direction textureStore calls must use the target page",
        );
        assert_eq!(
            src.matches("i32(rect.compact_x + rect_x), i32(rect.compact_y + rect_y)")
                .count(),
            3,
            "every store writes at the chunk's compact-atlas position",
        );

        let entries = compute_bgl_entries();
        for binding in [6, 8] {
            assert!(matches!(
                entries.iter().find(|entry| entry.binding == binding),
                Some(wgpu::BindGroupLayoutEntry {
                    ty: wgpu::BindingType::StorageTexture {
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        ..
                    },
                    ..
                })
            ));
        }
    }

    #[test]
    fn forward_shader_samples_animated_arrays_through_the_block_table() {
        let src = include_str!("../shaders/forward.wgsl");
        assert!(src.contains("animated_lm_atlas: texture_2d_array<f32>"));
        assert!(src.contains("animated_lm_direction: texture_2d_array<f32>"));
        assert!(src.contains("@location(7) @interpolate(flat) animated_block: u32"));
        assert!(src.contains("@location(5) lightmap_block_ids: vec2<u32>"));
        assert!(src.contains("animated_block_uv(in.lightmap_texel, in.animated_block)"));
        assert!(src.contains("sample_lightmap_animated(animated.uv, animated.page)"));
        assert!(src.contains("i32(animated.page)"));
        let helpers = include_str!("../shaders/lightmap_sample.wgsl");
        assert!(
            helpers.contains("block_id == 0u || block_id > animated_block_table.block_count"),
            "id 0 and ids past an empty (inactive) table must resolve to no block",
        );
        assert!(
            !src.contains("lightmap_layer_flags == 0u"),
            "animated sampling must not be limited to pool layer zero",
        );
    }

    #[test]
    fn dispatch_tile_expansion_small_rect() {
        let tiles = expand_dispatch_tiles(&[mk_rect(5, 5, 0)], &[mk_block(0, 0)]);
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].chunk_idx, 0);
        assert_eq!(tiles[0].tile_origin_x, 0);
        assert_eq!(tiles[0].tile_origin_y, 0);
        assert_eq!(tiles[0].target_page, 0);
    }

    #[test]
    fn dispatch_tile_expansion_exact_tile_boundary() {
        let tiles = expand_dispatch_tiles(&[mk_rect(16, 8, 0)], &[mk_block(0, 0)]);
        assert_eq!(tiles.len(), 2);
        assert_eq!(tiles[0].tile_origin_x, 0);
        assert_eq!(tiles[1].tile_origin_x, 8);
    }

    #[test]
    fn dispatch_tile_expansion_partial_tile() {
        let tiles = expand_dispatch_tiles(&[mk_rect(9, 9, 0)], &[mk_block(0, 0)]);
        assert_eq!(tiles.len(), 4);
    }

    #[test]
    fn dispatch_tile_expansion_multiple_chunks_preserves_index() {
        let tiles =
            expand_dispatch_tiles(&[mk_rect(8, 8, 0), mk_rect(12, 8, 64)], &[mk_block(0, 0)]);
        assert_eq!(tiles.len(), 3);
        assert_eq!(tiles[0].chunk_idx, 0);
        assert_eq!(tiles[1].chunk_idx, 1);
        assert_eq!(tiles[2].chunk_idx, 1);
    }

    #[test]
    fn dispatch_tile_expansion_skips_zero_area_and_blocks_past_the_table() {
        let past_table = ChunkAtlasRect {
            block: 5,
            ..mk_rect(8, 8, 0)
        };
        let tiles = expand_dispatch_tiles(
            &[
                mk_rect(0, 8, 0),
                mk_rect(8, 0, 0),
                past_table,
                mk_rect(8, 8, 0),
            ],
            &[mk_block(0, 0)],
        );
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].chunk_idx, 3);
    }

    /// Pin P9: blocks spilled across two pages compose onto the page the
    /// forward block table names for the same block, at the same offset.
    #[test]
    fn compose_and_forward_resolve_every_block_to_the_same_page_and_offset() {
        use crate::lighting::lightmap::animated_block_table_bytes;
        use postretro_level_format::animated_lightmap_atlas::{
            ANIMATED_BLOCK_TABLE_BYTES_PER_BLOCK, ANIMATED_BLOCK_TABLE_HEADER_BYTES,
        };
        let block = |lightmap_block, block_x, compact_x, compact_layer| AnimatedBlock {
            lightmap_block,
            block_x,
            block_y: 40,
            compact_x,
            compact_y: 0,
            compact_layer,
            width: 16,
            height: 16,
        };
        let section = AnimatedLightWeightMapsSection {
            page_size: 64,
            compact_layers: 2,
            blocks: vec![block(2, 100, 0, 0), block(9, 7, 16, 0), block(4, 300, 0, 1)],
            chunk_rects: (0..3)
                .map(|index| ChunkAtlasRect {
                    compact_x: [0, 16, 0][index] + 2,
                    compact_y: 2,
                    width: 8,
                    height: 8,
                    texel_offset: index as u32 * 64,
                    block: index as u32,
                })
                .collect(),
            offset_counts: vec![
                TexelLightEntry {
                    offset: 0,
                    count: 0,
                };
                192
            ],
            texel_lights: Vec::new(),
        };
        assert_eq!(section.consistency_error(), None);

        let tiles = expand_dispatch_tiles(&section.chunk_rects, &section.blocks);
        let table = animated_block_table_bytes(Some(&section)).expect("a valid section packs");
        let word = |at: usize| u32::from_ne_bytes(table[at..at + 4].try_into().unwrap());
        for tile in &tiles {
            let rect = section.chunk_rects[tile.chunk_idx as usize];
            let block = section.blocks[rect.block as usize];
            let at = ANIMATED_BLOCK_TABLE_HEADER_BYTES as usize
                + rect.block as usize * ANIMATED_BLOCK_TABLE_BYTES_PER_BLOCK as usize;
            let packed = word(at);
            let (dx, dy) = (
                i64::from(packed as u16 as i16),
                i64::from((packed >> 16) as u16 as i16),
            );
            assert_eq!(
                tile.target_page,
                word(at + 4),
                "compose and forward share the page"
            );
            // A block-local texel of this chunk lands where compose wrote it.
            let (_, block_x, block_y) =
                section.chunk_block_origin(tile.chunk_idx as usize).unwrap();
            assert_eq!(
                (i64::from(block_x) + dx, i64::from(block_y) + dy),
                (i64::from(rect.compact_x), i64::from(rect.compact_y)),
                "block {} offsets agree",
                rect.block,
            );
            assert_eq!(tile.target_page, block.compact_layer);
        }
        assert!(
            tiles.iter().any(|tile| tile.target_page == 1),
            "page 1 is exercised"
        );
    }

    fn mk_leaf(cell_id: u32, chunk_range_start: u32, chunk_range_count: u32) -> BvhLeaf {
        BvhLeaf {
            aabb_min: [0.0; 3],
            material_bucket_id: 0,
            aabb_max: [1.0; 3],
            index_offset: 0,
            index_count: 0,
            cell_id,
            chunk_range_start,
            chunk_range_count,
        }
    }

    #[test]
    fn build_chunk_cell_ids_stamps_each_leaf_range() {
        let leaves = [mk_leaf(7, 0, 2), mk_leaf(9, 2, 3)];
        let ids = build_chunk_cell_ids(&leaves, 5);
        assert_eq!(ids, vec![7, 7, 9, 9, 9]);
    }

    #[test]
    fn build_chunk_cell_ids_leaves_unreferenced_chunks_as_sentinel() {
        let leaves = [mk_leaf(3, 0, 1)];
        let ids = build_chunk_cell_ids(&leaves, 2);
        assert_eq!(ids, vec![3, u32::MAX]);
    }

    #[test]
    fn build_chunk_cell_ids_clamps_out_of_range_leaf() {
        let leaves = [mk_leaf(5, 0, 10)];
        let ids = build_chunk_cell_ids(&leaves, 3);
        assert_eq!(ids, vec![5, 5, 5]);
    }

    /// The compose pass fuses each texel's per-light incoming directions into a
    /// dominant-direction atlas so style-animated lights receive the same
    /// bumped-Lambert normal-map correction the static lightmap gets. Guard the
    /// binding and store against silent removal. The atlas stores an octahedral
    /// direction in `.rg` (via `encode_direction_oct`, the inverse of forward's
    /// `decode_lightmap_direction`) plus a coverage flag in `.a`.
    #[test]
    fn compose_shader_emits_dominant_direction_atlas() {
        let src = include_str!("../shaders/animated_lightmap_compose.wgsl");
        assert!(
            src.contains("@group(1) @binding(8)"),
            "the direction atlas binding (binding 8) must be declared",
        );
        assert!(
            src.contains("animated_lm_direction_atlas"),
            "the direction atlas must be referenced by the compose shader",
        );
        // One main-path store into the direction atlas. Assert the binding +
        // write exist rather than pinning the exact encoding, so the test
        // survives octahedral-encoding tweaks.
        let dir_stores = src.matches("animated_lm_direction_atlas").count();
        assert!(
            dir_stores >= 2,
            "expected the direction atlas declaration plus at least one store, found {dir_stores}",
        );
    }

    #[test]
    fn animated_atlas_extent_is_page_count_square_pages() {
        let extent = animated_atlas_extent(1024, 3).expect("nonzero page count allocates");
        assert_eq!(extent.width, 1024);
        assert_eq!(extent.height, 1024);
        assert_eq!(extent.depth_or_array_layers, 3);
        assert_eq!(
            animated_atlas_extent(1024, 0),
            None,
            "page count zero must select the dummy path instead of depth zero",
        );
    }

    #[test]
    fn animated_atlas_preflight_rejects_device_layer_and_dimension_overflow() {
        let err = animated_atlas_preflight(64, 5, 8192, 4).unwrap_err();
        assert!(err.contains("maxTextureArrayLayers 4"), "{err}");
        let err = animated_atlas_preflight(16384, 1, 8192, 256).unwrap_err();
        assert!(err.contains("maxTextureDimension2D 8192"), "{err}");
    }

    /// Pin P6: the forward table describes the atlas actually bound. A dummy
    /// atlas gets no section, so every vertex resolves to no block.
    #[test]
    fn inactive_resource_installs_an_empty_block_table_for_a_valid_section() {
        let decoded = two_page_weight_maps();
        assert_eq!(
            installed_block_table_section(false, Some(&decoded)),
            None,
            "a valid no-texel-lights section binds the dummy atlas and must expose no blocks",
        );
        assert_eq!(
            installed_block_table_section(true, Some(&decoded)),
            Some(&decoded),
            "an active atlas keeps the decoded block table",
        );
        assert_eq!(installed_block_table_section(true, None), None);
    }

    #[test]
    fn dummy_animated_views_are_array_compatible() {
        assert_eq!(
            animated_atlas_view_dimension(),
            wgpu::TextureViewDimension::D2Array,
        );
    }

    #[test]
    fn construction_failure_replaces_resources_with_the_dummy_path() {
        let mut dummy_built = false;
        let resource = with_dummy_fallback(
            Err("over budget".to_owned()),
            || {
                dummy_built = true;
                ("dummy forward view", None::<u32>)
            },
            "animated lightmap test install",
        );

        assert!(dummy_built);
        assert_eq!(resource.0, "dummy forward view");
        assert_eq!(resource.1, None, "dummy path has no dispatch state");
        let decoded = two_page_weight_maps();
        assert_eq!(
            installed_block_table_section(resource.1.is_some(), Some(&decoded)),
            None,
            "an error fallback must not leave the decoded blocks bound beside the dummy atlas",
        );
    }

    #[test]
    fn over_budget_fallback_logs_renderer_error() {
        let construction = animated_atlas_preflight(8192, 2, 8192, 256);
        assert!(
            construction.is_err(),
            "fixture must exceed the 1 GiB budget"
        );

        let capture = LogCapture::start();
        with_dummy_fallback(construction.map(|_| ()), || (), "animated lightmap install");

        capture.assert_logged_once(
            Level::Error,
            "[Renderer] animated lightmap install failed: animated lightmap atlas 8192x8192x2",
        );
    }
}
