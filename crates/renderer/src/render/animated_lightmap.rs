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
// Dispatch limit: tile count is validated against
// `max_compute_workgroups_per_dimension` (65535) at map load. The 2D-dispatch
// fallback is not implemented — a map that trips the cap must be rebaked with
// fewer/smaller animated chunks.

use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::animated_lightmap_atlas::{
    ANIMATED_ATLAS_VRAM_BUDGET_BYTES, animated_atlas_byte_estimate, animated_atlas_fits_budget,
};
pub use postretro_render_cpu::animated_lightmap::AnimatedLmDebugConfig;
use postretro_render_cpu::animated_lightmap::validate_cross_section;

use crate::compute_cull::{MAX_VISIBLE_CELLS, VISIBLE_CELLS_WORDS};
use postretro_render_data::geometry::BvhLeaf;
use postretro_visibility::VisibleCells;

use crate::lighting::lightmap::{
    INVALID_SLOT, StaticLayerToAnimatedSlot, animated_slot_for_static_layer,
    static_layer_to_animated_slot,
};

use super::sh_volume::AnimatedLightBuffers;

/// wgpu default `max_compute_workgroups_per_dimension`.
const MAX_WORKGROUPS_PER_DIM: u32 = 65535;

/// Array-atlas dimensions derived from the static lightmap dimensions and the
/// section-25 animated slot count. `None` is the no-animated-atlas path: wgpu
/// rejects a texture whose array depth is zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AnimatedAtlasExtent {
    width: u32,
    height: u32,
    depth_or_array_layers: u32,
}

fn animated_atlas_extent(width: u32, height: u32, slot_count: u32) -> Option<AnimatedAtlasExtent> {
    (slot_count > 0).then_some(AnimatedAtlasExtent {
        width,
        height,
        depth_or_array_layers: slot_count,
    })
}

fn animated_atlas_view_dimension() -> wgpu::TextureViewDimension {
    wgpu::TextureViewDimension::D2Array
}

/// One 8×8 atlas tile assigned to a chunk. Indexed by `workgroup_id.x` in the compose shader.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DispatchTile {
    chunk_idx: u32,
    tile_origin_x: u32,
    tile_origin_y: u32,
    target_slot: u32,
}

/// GPU storage-buffer layout for the compose-relevant prefix of
/// `ChunkAtlasRect`. The v3 static layer resolves to `DispatchTile.target_slot`
/// on the CPU, so it intentionally stays out of this buffer and WGSL struct.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct GpuChunkRect {
    atlas_x: u32,
    atlas_y: u32,
    width: u32,
    height: u32,
    texel_offset: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct GpuOffsetCount {
    offset: u32,
    count: u32,
}

/// GPU storage-buffer layout for `TexelLight`. The two `u16` octahedral
/// direction components are packed into one `u32` (low 16 bits = x, high 16
/// bits = y) so the struct stays naturally aligned for storage-buffer access
/// (12 bytes; the WGSL `TexelLight` mirrors the same packing).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct GpuTexelLight {
    light_index: u32,
    weight: f32,
    direction_oct_packed: u32,
}

/// Compose-pass resources. Always allocated — maps with no
/// `AnimatedLightWeightMaps` section get a 1×1 zero dummy atlas and skip the
/// per-frame dispatch.
pub struct AnimatedLightmapResources {
    /// `None` when no weight-map section is present; the dummy view is bound instead.
    #[allow(dead_code)]
    atlas_texture: Option<wgpu::Texture>,
    /// Per-texel fused dominant-direction atlas. `None` on the no-weight-maps
    /// path; the direction dummy view is bound instead.
    #[allow(dead_code)]
    direction_atlas_texture: Option<wgpu::Texture>,
    #[allow(dead_code)]
    dummy_texture: wgpu::Texture,
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
    /// Sized to the master tile count; updated each frame with the
    /// visibility-culled prefix via `queue.write_buffer`. Needs `COPY_DST`.
    dispatch_tiles_buffer: wgpu::Buffer,
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
    /// Build the non-dispatching dummy path through `new`'s `weight_maps: None`
    /// early-out. Load failures use this instead of retaining a prior level's
    /// atlas views or compose state.
    pub(crate) fn dummy(
        device: &wgpu::Device,
        animation: &AnimatedLightBuffers,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        debug_config: AnimatedLmDebugConfig,
    ) -> Self {
        Self::new(
            device,
            None,
            None,
            &[],
            animation,
            uniform_bind_group_layout,
            None,
            debug_config,
        )
        .expect("weight_maps: None is the non-failing animated-lightmap dummy path")
    }

    /// Build the compose pass resources.
    ///
    /// `uniform_bind_group_layout` must include `wgpu::ShaderStages::COMPUTE` —
    /// the compose pipeline is a compute pipeline and wgpu validation fails at
    /// `create_compute_pipeline` time otherwise. The canonical BGL in
    /// `render/mod.rs` declares `VERTEX | FRAGMENT | COMPUTE` for this reason;
    /// dropping COMPUTE there would break this pass. `wgpu::BindGroupLayout` is
    /// opaque so this cannot be runtime-checked — it must be preserved at the
    /// call site.
    ///
    /// `atlas_dimensions` — `(width, height)` from `lightmap::usable_atlas_dimensions`.
    /// The animated irradiance and direction atlases are created at exactly these
    /// dimensions: compose writes at absolute static-atlas coordinates, and the
    /// forward pass samples all three atlases with one normalized `lightmap_uv`.
    /// `None` means the static atlas degraded to a 1×1 placeholder (absent,
    /// zero-area, or oversize section); the animated path takes the dummy-atlas
    /// early-out — no valid coordinate space to write into.
    ///
    /// Returns `Err` on validation or allocation preflight failure; callers log
    /// and bind the non-dispatching dummy resource for this level.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        weight_maps: Option<&AnimatedLightWeightMapsSection>,
        animated_chunks: Option<&AnimatedLightChunksSection>,
        bvh_leaves: &[BvhLeaf],
        animation: &AnimatedLightBuffers,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        atlas_dimensions: Option<(u32, u32)>,
        debug_config: AnimatedLmDebugConfig,
    ) -> Result<Self, String> {
        let dummy_texture = create_zero_texture(device, 1, 1, "Animated LM Dummy");
        let dummy_view = dummy_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Animated LM Dummy Forward View"),
            dimension: Some(animated_atlas_view_dimension()),
            ..Default::default()
        });
        // Separate 1×1 zero view for the direction atlas slot so the forward
        // bind group stays valid on the empty-map path.
        let dummy_direction_view = dummy_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Animated LM Dummy Direction Forward View"),
            dimension: Some(animated_atlas_view_dimension()),
            ..Default::default()
        });

        let Some(section) = weight_maps else {
            return Ok(Self {
                atlas_texture: None,
                direction_atlas_texture: None,
                dummy_texture,
                forward_view: dummy_view,
                direction_forward_view: dummy_direction_view,
                dispatch_state: None,
            });
        };

        // A v3 empty section has no animated slots. Guard before any atlas
        // allocation: wgpu rejects a D2 array texture with depth zero.
        let slot_count = section.slot_to_static_layer.len() as u32;
        if animated_atlas_extent(1, 1, slot_count).is_none() {
            return Ok(Self {
                atlas_texture: None,
                direction_atlas_texture: None,
                dummy_texture,
                forward_view: dummy_view,
                direction_forward_view: dummy_direction_view,
                dispatch_state: None,
            });
        }

        let Some((atlas_width, atlas_height)) = atlas_dimensions else {
            // The static lightmap atlas degraded to the 1×1 placeholder (absent,
            // zero-area, or oversize section), so the absolute coordinates the
            // baked weight maps reference have no valid target. Compose would
            // write off-atlas and the forward pass would sample the placeholder.
            // Take the dummy-atlas path: the animated term contributes nothing,
            // which matches the static term already being neutral.
            log::warn!(
                "[Renderer] Animated lightmap present but the static lightmap atlas \
                 is unavailable; skipping animated-light compose for this level."
            );
            return Ok(Self {
                atlas_texture: None,
                direction_atlas_texture: None,
                dummy_texture,
                forward_view: dummy_view,
                direction_forward_view: dummy_direction_view,
                dispatch_state: None,
            });
        };

        validate_cross_section(
            section,
            animated_chunks,
            animation.animated_light_count(),
            &section.slot_to_static_layer,
            (atlas_width, atlas_height),
        )?;

        if section.chunk_rects.is_empty() || section.texel_lights.is_empty() {
            // Nothing to compose. Either no animated chunks, or every animated
            // light is SDF-typed so the baker emitted zero baked direct weight
            // (the disjoint-direct split — sdf-per-light-shadows Task 1). The
            // chunk rects still exist (they pair 1:1 with AnimatedLightChunks
            // for the SH delta bake), but with no texel-lights there is no
            // direct term to composite: the forward pass falls back to the
            // static lightmap and runtime SDF resolves these lights' direct
            // term. Takes the same no-atlas path as a map with no weight maps.
            return Ok(Self {
                atlas_texture: None,
                direction_atlas_texture: None,
                dummy_texture,
                forward_view: dummy_view,
                direction_forward_view: dummy_direction_view,
                dispatch_state: None,
            });
        }

        let atlas_extent = animated_atlas_extent(atlas_width, atlas_height, slot_count)
            .expect("slot-count guard above rejects zero-depth animated atlases");

        let atlas_bytes = animated_atlas_byte_estimate(atlas_width, atlas_height, slot_count);
        if !animated_atlas_fits_budget(
            atlas_width,
            atlas_height,
            slot_count,
            ANIMATED_ATLAS_VRAM_BUDGET_BYTES,
        ) {
            return Err(format!(
                "animated lightmap atlas {atlas_width}x{atlas_height}x{slot_count} requires \
                 {atlas_bytes} bytes, exceeding the {}-byte VRAM budget",
                ANIMATED_ATLAS_VRAM_BUDGET_BYTES,
            ));
        }

        let static_layer_to_slot = static_layer_to_animated_slot(&section.slot_to_static_layer);
        let dispatch_tiles = expand_dispatch_tiles(&section.chunk_rects, &static_layer_to_slot);
        if dispatch_tiles.len() as u32 > MAX_WORKGROUPS_PER_DIM {
            return Err(format!(
                "[AnimatedLightmap] dispatch tile count {} exceeds wgpu \
                 max_compute_workgroups_per_dimension ({}); 2D-dispatch \
                 fallback is not implemented — rebake with fewer / smaller \
                 animated chunks.",
                dispatch_tiles.len(),
                MAX_WORKGROUPS_PER_DIM,
            ));
        }
        let compose_workgroup_count = dispatch_tiles.len() as u32;

        // No `COPY_DST` needed — wgpu zero-initializes and the compose pass
        // overwrites every texel the forward pass will sample.
        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Animated LM Atlas"),
            // Sized to match the static lightmap atlas (see `atlas_dimensions`
            // doc on `new`); width and height are independent — the static atlas
            // is shelf-packed and need not be square.
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
        let direction_atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
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
            "[Renderer] Animated lightmap atlases {atlas_width}x{atlas_height}x{slot_count}, ~{} MiB VRAM (Rgba16Float irradiance + Rgba8Unorm direction)",
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

        let chunk_rects_bytes = pack_chunk_rects(&section.chunk_rects);
        let offset_counts_bytes = pack_offset_counts(section);
        let texel_lights_bytes = pack_texel_lights(section);
        let dispatch_tiles_bytes = pack_dispatch_tiles(&dispatch_tiles);

        let chunk_rects_buffer =
            create_storage_buffer(device, "Animated LM Chunk Rects", &chunk_rects_bytes);
        let offset_counts_buffer =
            create_storage_buffer(device, "Animated LM Offset Counts", &offset_counts_bytes);
        let texel_lights_buffer =
            create_storage_buffer(device, "Animated LM Texel Lights", &texel_lights_bytes);
        // Seeded with the full master list; the first frame's `DrawAll` path
        // uploads an identical slice without needing a separate clear.
        let dispatch_tiles_buffer = {
            use wgpu::util::DeviceExt;
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Animated LM Dispatch Tiles"),
                contents: &dispatch_tiles_bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            })
        };

        let chunk_cell_ids = build_chunk_cell_ids(bvh_leaves, section.chunk_rects.len());

        let debug_buffer = {
            use wgpu::util::DeviceExt;
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Animated LM Debug Config"),
                contents: &debug_config.to_uniform_bytes(),
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };

        let compute_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Animated LM Compute BGL"),
            entries: &compute_bgl_entries(),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Animated LM Pipeline Layout"),
            bind_group_layouts: &[Some(uniform_bind_group_layout), Some(&compute_bgl)],
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

        let compose_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Animated LM Compose Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("compose_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let compute_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Animated LM Compute Bind Group"),
            layout: &compute_bgl,
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
            section.chunk_rects.len(),
            section.offset_counts.len(),
            section.texel_lights.len(),
            compose_workgroup_count,
        );

        let total_tiles = compose_workgroup_count;
        Ok(Self {
            atlas_texture: Some(atlas_texture),
            direction_atlas_texture: Some(direction_atlas_texture),
            dummy_texture,
            forward_view,
            direction_forward_view,
            dispatch_state: Some(DispatchState {
                compose_pipeline,
                compute_bind_group,
                dispatch_tiles_buffer,
                master_tiles: dispatch_tiles,
                chunk_cell_ids,
                // Pre-sized so `DrawAll` on the first frame doesn't realloc.
                scratch_tiles: Vec::with_capacity(total_tiles as usize),
                scratch_bytes: Vec::with_capacity(dispatch_tiles_bytes.len()),
                prev_kept: u32::MAX,
                total_tiles,
            }),
        })
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
    /// marked-but-unwritten. The timing window averages over a rolling buffer
    /// and tolerates missing samples.
    pub fn dispatch(
        &mut self,
        queue: &wgpu::Queue,
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
        pass.dispatch_workgroups(kept, 1, 1);
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

/// Build the chunk → cell-id table from BVH leaf ranges. Chunks not covered
/// by any leaf keep `u32::MAX` so the per-frame filter always rejects them.
/// In a valid PRL every animated chunk belongs to exactly one leaf; the
/// sentinel is a defensive fallback.
fn build_chunk_cell_ids(bvh_leaves: &[BvhLeaf], chunk_count: usize) -> Vec<u32> {
    let mut chunk_cell_ids = vec![u32::MAX; chunk_count];
    for leaf in bvh_leaves {
        let start = leaf.chunk_range_start as usize;
        let count = leaf.chunk_range_count as usize;
        let end = start.saturating_add(count).min(chunk_count);
        for slot in chunk_cell_ids.iter_mut().take(end).skip(start) {
            *slot = leaf.cell_id;
        }
    }
    chunk_cell_ids
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
    width: u32,
    height: u32,
    label: &str,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        // `Rgba16Float` zero-initializes to (0,0,0,0); no upload needed.
        // `STORAGE_BINDING` required so the bind-group layout is compatible
        // with the real atlas slot when weight maps are absent.
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    })
}

fn create_storage_buffer(device: &wgpu::Device, label: &str, bytes: &[u8]) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    // wgpu rejects zero-sized storage buffers. All callers are gated behind the
    // early-out in `new` that bails when `chunk_rects` OR `texel_lights` is
    // empty, so by here every packed buffer is non-empty. (An all-SDF map emits
    // chunk rects + offset_counts but zero texel_lights — caught by that gate.)
    // Use `assert!` (not `debug_assert!`) so a future regression surfaces in
    // release builds.
    assert!(!bytes.is_empty(), "{label} storage buffer would be empty");
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytes,
        usage: wgpu::BufferUsages::STORAGE,
    })
}

/// Expand each chunk rect into `ceil(w/8) × ceil(h/8)` 8×8 dispatch tiles.
/// Tile order is y-major, x-minor; order doesn't affect correctness.
fn expand_dispatch_tiles(
    chunk_rects: &[postretro_level_format::animated_light_weight_maps::ChunkAtlasRect],
    static_layer_to_slot: &StaticLayerToAnimatedSlot,
) -> Vec<DispatchTile> {
    let mut tiles = Vec::new();
    for (chunk_idx, rect) in chunk_rects.iter().enumerate() {
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        let target_slot = animated_slot_for_static_layer(static_layer_to_slot, rect.layer);
        if target_slot == INVALID_SLOT {
            // Section-25 validation rejects this malformed record at load, but
            // never compose it into slot 0 if a caller bypasses that boundary.
            continue;
        }
        let tiles_x = rect.width.div_ceil(8);
        let tiles_y = rect.height.div_ceil(8);
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                tiles.push(DispatchTile {
                    chunk_idx: chunk_idx as u32,
                    tile_origin_x: tx * 8,
                    tile_origin_y: ty * 8,
                    target_slot,
                });
            }
        }
    }
    tiles
}

fn pack_chunk_rects(
    chunk_rects: &[postretro_level_format::animated_light_weight_maps::ChunkAtlasRect],
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(chunk_rects.len() * std::mem::size_of::<GpuChunkRect>());
    for r in chunk_rects {
        bytes.extend_from_slice(&r.atlas_x.to_ne_bytes());
        bytes.extend_from_slice(&r.atlas_y.to_ne_bytes());
        bytes.extend_from_slice(&r.width.to_ne_bytes());
        bytes.extend_from_slice(&r.height.to_ne_bytes());
        bytes.extend_from_slice(&r.texel_offset.to_ne_bytes());
    }
    bytes
}

fn pack_offset_counts(section: &AnimatedLightWeightMapsSection) -> Vec<u8> {
    let mut bytes =
        Vec::with_capacity(section.offset_counts.len() * std::mem::size_of::<GpuOffsetCount>());
    for oc in &section.offset_counts {
        bytes.extend_from_slice(&oc.offset.to_ne_bytes());
        bytes.extend_from_slice(&oc.count.to_ne_bytes());
    }
    bytes
}

fn pack_texel_lights(section: &AnimatedLightWeightMapsSection) -> Vec<u8> {
    let mut bytes =
        Vec::with_capacity(section.texel_lights.len() * std::mem::size_of::<GpuTexelLight>());
    for tl in &section.texel_lights {
        bytes.extend_from_slice(&tl.light_index.to_ne_bytes());
        bytes.extend_from_slice(&tl.weight.to_ne_bytes());
        // Pack the two octahedral u16s into one u32. WGSL unpacks via
        // `direction_oct_packed & 0xFFFF` / `>> 16` — see compose shader.
        let packed: u32 = (tl.direction_oct[0] as u32) | ((tl.direction_oct[1] as u32) << 16);
        bytes.extend_from_slice(&packed.to_ne_bytes());
    }
    bytes
}

fn pack_dispatch_tiles(tiles: &[DispatchTile]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(tiles));
    pack_dispatch_tiles_into(tiles, &mut bytes);
    bytes
}

/// Appends packed tile bytes into a caller-owned buffer to avoid per-frame allocation.
fn pack_dispatch_tiles_into(tiles: &[DispatchTile], bytes: &mut Vec<u8>) {
    bytes.reserve(std::mem::size_of_val(tiles));
    for t in tiles {
        bytes.extend_from_slice(&t.chunk_idx.to_ne_bytes());
        bytes.extend_from_slice(&t.tile_origin_x.to_ne_bytes());
        bytes.extend_from_slice(&t.tile_origin_y.to_ne_bytes());
        bytes.extend_from_slice(&t.target_slot.to_ne_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::animated_light_weight_maps::ChunkAtlasRect;

    fn mk_rect(w: u32, h: u32, offset: u32) -> ChunkAtlasRect {
        ChunkAtlasRect {
            atlas_x: 0,
            atlas_y: 0,
            width: w,
            height: h,
            texel_offset: offset,
            layer: 0,
        }
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

    #[test]
    fn compose_shader_uses_array_storage_and_slot_indexed_stores() {
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
            src.matches("i32(tile.target_slot)").count(),
            3,
            "debug, irradiance, and direction textureStore calls must use the target slot",
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
    fn forward_shader_samples_animated_arrays_by_slot_without_layer_zero_guard() {
        let src = include_str!("../shaders/forward.wgsl");
        assert!(src.contains("animated_lm_atlas: texture_2d_array<f32>"));
        assert!(src.contains("animated_lm_direction: texture_2d_array<f32>"));
        assert!(src.contains("sample_lightmap_animated(in.lightmap_uv, animated_slot)"));
        assert!(src.contains("i32(animated_slot)"));
        assert!(
            !src.contains("in.lightmap_layer == 0u"),
            "animated sampling must not be limited to static layer zero",
        );
    }

    #[test]
    fn dispatch_tile_expansion_small_rect() {
        let tiles =
            expand_dispatch_tiles(&[mk_rect(5, 5, 0)], &static_layer_to_animated_slot(&[0]));
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].chunk_idx, 0);
        assert_eq!(tiles[0].tile_origin_x, 0);
        assert_eq!(tiles[0].tile_origin_y, 0);
        assert_eq!(tiles[0].target_slot, 0);
    }

    #[test]
    fn dispatch_tile_expansion_exact_tile_boundary() {
        let tiles =
            expand_dispatch_tiles(&[mk_rect(16, 8, 0)], &static_layer_to_animated_slot(&[0]));
        assert_eq!(tiles.len(), 2);
        assert_eq!(tiles[0].tile_origin_x, 0);
        assert_eq!(tiles[1].tile_origin_x, 8);
    }

    #[test]
    fn dispatch_tile_expansion_partial_tile() {
        let tiles =
            expand_dispatch_tiles(&[mk_rect(9, 9, 0)], &static_layer_to_animated_slot(&[0]));
        assert_eq!(tiles.len(), 4);
    }

    #[test]
    fn dispatch_tile_expansion_multiple_chunks_preserves_index() {
        let tiles = expand_dispatch_tiles(
            &[mk_rect(8, 8, 0), mk_rect(12, 8, 64)],
            &static_layer_to_animated_slot(&[0]),
        );
        assert_eq!(tiles.len(), 3);
        assert_eq!(tiles[0].chunk_idx, 0);
        assert_eq!(tiles[1].chunk_idx, 1);
        assert_eq!(tiles[2].chunk_idx, 1);
    }

    #[test]
    fn dispatch_tile_expansion_skips_zero_area() {
        let tiles = expand_dispatch_tiles(
            &[mk_rect(0, 8, 0), mk_rect(8, 0, 0), mk_rect(8, 8, 0)],
            &static_layer_to_animated_slot(&[0]),
        );
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].chunk_idx, 2);
    }

    #[test]
    fn compose_and_forward_resolve_each_static_layer_to_the_same_slot() {
        let slot_to_static_layer = [2, 9];
        let static_layer_to_slot = static_layer_to_animated_slot(&slot_to_static_layer);
        let mut first = mk_rect(8, 8, 0);
        first.layer = 2;
        let mut second = mk_rect(8, 8, 64);
        second.layer = 9;

        let tiles = expand_dispatch_tiles(&[first, second], &static_layer_to_slot);
        assert_eq!(
            tiles[0].target_slot,
            animated_slot_for_static_layer(&static_layer_to_slot, first.layer),
        );
        assert_eq!(
            tiles[1].target_slot,
            animated_slot_for_static_layer(&static_layer_to_slot, second.layer),
        );
        assert_eq!(tiles[0].target_slot, 0);
        assert_eq!(tiles[1].target_slot, 1);
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

    /// The animated irradiance/direction atlases must be created at the same
    /// dimensions the static lightmap atlas is created at: compose writes at
    /// absolute static-atlas coordinates and the forward pass samples all three
    /// atlases with one normalized `lightmap_uv`. The static atlas is
    /// dynamically sized (shelf-packed, up to 8192 per dimension, width may
    /// differ from height — it is not the fixed 1024² this code once assumed),
    /// so the size is sourced from the loaded `LightmapSection` via
    /// `lightmap::usable_atlas_dimensions` — the same resolver the static
    /// texture creation uses. This guards that both paths read from one source.
    #[test]
    fn animated_atlas_dimensions_track_static_lightmap() {
        use crate::lighting::lightmap::usable_atlas_dimensions;
        use postretro_level_format::lightmap::{
            IRRADIANCE_FORMAT_RGBA16F, LightmapMode, LightmapSection,
        };

        // A non-square, non-1024 section — what a real shelf-packed atlas looks
        // like — resolves to the section's own width/height under a generous
        // device limit.
        let section = LightmapSection {
            layer_count: 1,
            irr_width: 4096,
            irr_height: 2048,
            irr_texel_density: 1.0,
            irradiance: vec![0u8; 4096 * 2048 * 8],
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            dir_width: 4096,
            dir_height: 2048,
            dir_texel_density: 1.0,
            direction: vec![0u8; 4096 * 2048 * 4],
            mode: LightmapMode::Shadowed,
        };
        assert_eq!(
            usable_atlas_dimensions(Some(&section), 8192, 256),
            Some((4096, 2048)),
            "animated atlas size must equal the loaded lightmap dimensions",
        );

        // Absent / oversize sections resolve to `None`, which drives the
        // animated path to its dummy-atlas early-out (no valid coordinate space).
        assert_eq!(usable_atlas_dimensions(None, 8192, 256), None);
        assert_eq!(usable_atlas_dimensions(Some(&section), 1024, 256), None);
    }

    #[test]
    fn animated_atlas_extent_uses_slot_count_for_array_depth() {
        let extent = animated_atlas_extent(4096, 2048, 3).expect("nonzero slot count allocates");
        assert_eq!(extent.width, 4096);
        assert_eq!(extent.height, 2048);
        assert_eq!(extent.depth_or_array_layers, 3);
        assert_eq!(
            animated_atlas_extent(4096, 2048, 0),
            None,
            "slot count zero must select the dummy path instead of depth zero",
        );
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
    }
}
