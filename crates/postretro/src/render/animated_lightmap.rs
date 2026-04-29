// Animated-lightmap compose compute pass: per-frame compose of the
// animated-light contribution atlas. The atlas texture is zero-initialized
// by wgpu at creation and the compose pass writes every texel the forward
// pass samples, so no per-frame clear is needed. Samples the same
// descriptor and `anim_samples` buffers as the SH path (`sh_volume.rs`) via
// `AnimatedLightBuffers`, so scripting toggles to `is_active` affect both
// consumers in one upload.
//
// Visibility invariant: `VisibleCells` is the single source of truth for
// which tiles are dispatched each frame. The per-frame filter in
// `dispatch()` pushes only tiles whose owning chunk belongs to a visible
// cell; tiles belonging to invisible cells retain their prior-frame atlas
// contents, which is fine because the forward pass will not sample them.
// Any future pass that samples the animated lightmap atlas (reflection
// probes, alternate cameras) must either share the same `VisibleCells` or
// skip animated-lit chunks entirely — otherwise it will read stale
// atlas contents for cells the current frame's visibility considers
// invisible.
//
// See: context/lib/rendering_pipeline.md §4, §7.1
//
// Dispatch-limit choice: this module asserts at map load that the total
// 8×8 tile count fits in `max_compute_workgroups_per_dimension` (65535 at
// wgpu defaults). Bundled maps stay far below that; the 2D-dispatch
// fallback described in the plan is intentionally not wired up. If a
// future map trips the cap, extend the dispatch here and compute the flat
// index in `animated_lightmap_compose.wgsl::compose_main`.

use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;

use crate::compute_cull::{MAX_VISIBLE_CELLS, VISIBLE_CELLS_WORDS};
use crate::geometry::BvhLeaf;
use crate::visibility::VisibleCells;

use super::sh_volume::AnimatedLightBuffers;

/// Animated-lightmap atlas resolution. Matches the static lightmap atlas
/// (1024²); same UV drives both samples in the forward pass. A future
/// halve-to-512 experiment would change this value and the compose
/// dispatch shape — nothing else.
pub const ANIMATED_ATLAS_SIZE: u32 = 1024;

/// wgpu default `max_compute_workgroups_per_dimension`. The plan allows
/// either a 2D dispatch fallback or an outright refusal when the tile
/// count exceeds this; we pick refusal for simplicity.
const MAX_WORKGROUPS_PER_DIM: u32 = 65535;

/// Matches `postretro_level_format::animated_light_chunks::MAX_ANIMATED_LIGHTS_PER_CHUNK = 4`.
/// Used as the heatmap denominator in debug mode 1 — saturating at 1.0
/// when a covered texel references the cap. Kept as a local `u32` constant
/// because the exported symbol is `usize`; casting at every use site adds
/// noise without benefit.
const DEBUG_MAX_LIGHTS_PER_CHUNK: u32 = 4;

/// Env var selecting a compose-side debug visualization. Parsed once at
/// renderer init; unset / empty → normal path.
const DEBUG_ENV_VAR: &str = "POSTRETRO_ANIMATED_LM_DEBUG";

/// CPU-side mirror of the `DebugConfig` uniform consumed by
/// `animated_lightmap_compose.wgsl`. See the shader's `DebugConfig` struct
/// for field semantics.
#[derive(Debug, Clone, Copy, Default)]
pub struct AnimatedLmDebugConfig {
    /// 0 = off, 1 = count heatmap, 2 = isolate a single descriptor slot.
    pub mode: u32,
    /// Descriptor slot to isolate when `mode == 2`. Ignored otherwise.
    pub isolate_slot: u32,
}

impl AnimatedLmDebugConfig {
    /// Parse `POSTRETRO_ANIMATED_LM_DEBUG`. Recognized values:
    /// - unset / empty → off.
    /// - `count` → mode 1.
    /// - `isolate=<u32>` → mode 2 with the given descriptor slot.
    ///
    /// Anything else logs a warning and falls back to off so a typo doesn't
    /// silently change rendering.
    pub fn from_env() -> Self {
        let Ok(raw) = std::env::var(DEBUG_ENV_VAR) else {
            return Self::default();
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Self::default();
        }
        if trimmed.eq_ignore_ascii_case("count") {
            log::info!("[Renderer] Animated LM debug: count heatmap (mode 1)");
            return Self {
                mode: 1,
                isolate_slot: 0,
            };
        }
        if let Some(rest) = trimmed.strip_prefix("isolate=") {
            match rest.parse::<u32>() {
                Ok(slot) => {
                    log::info!("[Renderer] Animated LM debug: isolate slot {slot} (mode 2)");
                    return Self {
                        mode: 2,
                        isolate_slot: slot,
                    };
                }
                Err(err) => {
                    log::warn!(
                        "[Renderer] {DEBUG_ENV_VAR}='{raw}' has invalid slot: {err}; debug off",
                    );
                    return Self::default();
                }
            }
        }
        log::warn!(
            "[Renderer] {DEBUG_ENV_VAR}='{raw}' not recognized (expected 'count' or \
             'isolate=<u32>'); debug off",
        );
        Self::default()
    }

    fn to_uniform_bytes(self) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[0..4].copy_from_slice(&self.mode.to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.isolate_slot.to_ne_bytes());
        bytes[8..12].copy_from_slice(&DEBUG_MAX_LIGHTS_PER_CHUNK.to_ne_bytes());
        // bytes[12..16] = padding, already zero.
        bytes
    }
}

/// One 8×8 atlas tile assigned to a chunk. `workgroup_id.x` indexes this
/// array in the compose shader.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DispatchTile {
    chunk_idx: u32,
    tile_origin_x: u32,
    tile_origin_y: u32,
    _pad: u32,
}

/// GPU storage-buffer layout for one `ChunkAtlasRect`. Matches the format
/// crate's `ChunkAtlasRect` and the WGSL `ChunkAtlasRect` struct.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct GpuChunkRect {
    atlas_x: u32,
    atlas_y: u32,
    width: u32,
    height: u32,
    texel_offset: u32,
}

/// GPU storage-buffer layout for one `(offset, count)` pair.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct GpuOffsetCount {
    offset: u32,
    count: u32,
}

/// GPU storage-buffer layout for one `(light_index, weight)` pair.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct GpuTexelLight {
    light_index: u32,
    weight: f32,
}

/// Compose-pass resources. Always allocated — when the PRL has no
/// `AnimatedLightWeightMaps` section (or zero animated lights) the module
/// allocates a 1×1 zero dummy atlas and skips the per-frame dispatch.
pub struct AnimatedLightmapResources {
    /// 1024² `Rgba16Float` storage texture the compose pass writes into.
    /// Sampled by the forward pass through the lightmap bind group.
    /// `None` when no weight-map section is present — the dummy 1×1 view
    /// below is bound to group 4 instead.
    #[allow(dead_code)]
    atlas_texture: Option<wgpu::Texture>,
    /// 1×1 zero `Rgba16Float` texture used when no weight maps are present.
    #[allow(dead_code)]
    dummy_texture: wgpu::Texture,
    /// Forward-pass-facing view. Points at `atlas_texture` when present,
    /// otherwise at `dummy_texture` — the bind-group layout stays constant.
    pub forward_view: wgpu::TextureView,

    /// Present when we have real weight maps and a compute pipeline.
    /// When `None`, `dispatch` is a no-op.
    dispatch_state: Option<DispatchState>,
}

struct DispatchState {
    compose_pipeline: wgpu::ComputePipeline,
    compute_bind_group: wgpu::BindGroup,
    /// GPU buffer holding the per-frame trimmed dispatch tile list. Sized
    /// at creation to the master (unfiltered) tile count and updated each
    /// frame via `queue.write_buffer` with the trimmed prefix.
    /// `STORAGE | COPY_DST` so the per-frame upload is legal.
    dispatch_tiles_buffer: wgpu::Buffer,
    /// Master (unfiltered) tile list, built once at load time. The
    /// per-frame filter walks this and pushes tiles whose owning chunk's
    /// cell is visible.
    master_tiles: Vec<DispatchTile>,
    /// One entry per animated chunk (same indexing as
    /// `section.chunk_rects`): the `cell_id` of the BVH leaf that owns the
    /// chunk. Derived from `BvhLeaf.chunk_range_start/count` at load time.
    chunk_cell_ids: Vec<u32>,
    /// Persistent scratch buffers reused each frame to avoid per-frame
    /// allocation. `scratch_tiles` holds the filtered tile list;
    /// `scratch_bytes` holds its packed GPU-layout bytes.
    scratch_tiles: Vec<DispatchTile>,
    scratch_bytes: Vec<u8>,
    /// Previous frame's trimmed tile count. Used by the debug-level
    /// `kept/total` logger to deduplicate identical frames. Sentinel
    /// `u32::MAX` forces the first frame to log.
    prev_kept: u32,
    /// Total (unfiltered) tile count. Cached so the logger and the
    /// `DrawAll` fast path don't need to call `master_tiles.len()`.
    total_tiles: u32,
}

impl AnimatedLightmapResources {
    /// Build the compose pass. Inputs:
    /// - `weight_maps`: the loaded `AnimatedLightWeightMaps` PRL section, or
    ///   `None` when the map has no animated lights.
    /// - `animated_chunks`: the loaded `AnimatedLightChunks` PRL section.
    ///   Required for the `chunk_rects.len() == chunks.len()` cross-section
    ///   check when `weight_maps` is `Some`. May be `None` only when
    ///   `weight_maps` is also `None`.
    /// - `animation`: shared animated-light buffers (descriptors +
    ///   anim_samples). Borrowed — this module does not upload its own copy.
    /// - `uniform_bind_group_layout`: group-0 layout from the renderer. **Must
    ///   include `wgpu::ShaderStages::COMPUTE`** in its visibility flags — the
    ///   compose pipeline below is a compute pipeline and will fail
    ///   wgpu validation at `create_compute_pipeline` time otherwise. The
    ///   canonical BGL in `render/mod.rs` (search for "Uniform Bind Group
    ///   Layout") declares `VERTEX | FRAGMENT | COMPUTE` specifically so this
    ///   pass can reuse it; if a future change drops COMPUTE there, either
    ///   re-add it or switch this pass to its own BGL. `wgpu::BindGroupLayout`
    ///   is opaque and does not expose its visibility flags, so this contract
    ///   cannot be runtime-checked — it must be preserved at the call site.
    ///
    /// Returns an error string when cross-section validation fails; the
    /// caller should log and refuse to load the map.
    pub fn new(
        device: &wgpu::Device,
        weight_maps: Option<&AnimatedLightWeightMapsSection>,
        animated_chunks: Option<&AnimatedLightChunksSection>,
        bvh_leaves: &[BvhLeaf],
        animation: &AnimatedLightBuffers,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        debug_config: AnimatedLmDebugConfig,
    ) -> Result<Self, String> {
        // 1×1 zero dummy used either as the sole binding (no weight maps
        // present) or as a placeholder until the real atlas view is built.
        let dummy_texture = create_zero_texture(device, 1, 1, "Animated LM Dummy");
        let dummy_view = dummy_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let Some(section) = weight_maps else {
            // No weight maps — forward pass sees the 1×1 zero atlas.
            return Ok(Self {
                atlas_texture: None,
                dummy_texture,
                forward_view: dummy_view,
                dispatch_state: None,
            });
        };

        if section.chunk_rects.is_empty() {
            // Weight-maps section is present but empty (no covered chunks).
            // Treat the same as missing — nothing to compose.
            return Ok(Self {
                atlas_texture: None,
                dummy_texture,
                forward_view: dummy_view,
                dispatch_state: None,
            });
        }

        validate_cross_section(section, animated_chunks, animation.animated_light_count())?;

        // Build the dispatch-tile list: one 8×8 tile per `ceil(w/8)*ceil(h/8)`
        // slot inside each chunk rect.
        let dispatch_tiles = expand_dispatch_tiles(&section.chunk_rects);
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

        // Real 1024² storage texture. `STORAGE_BINDING | TEXTURE_BINDING` —
        // no `COPY_DST` because wgpu zero-initializes the texture at
        // creation and the compose pass overwrites every sampled texel.
        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Animated LM Atlas"),
            size: wgpu::Extent3d {
                width: ANIMATED_ATLAS_SIZE,
                height: ANIMATED_ATLAS_SIZE,
                depth_or_array_layers: 1,
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
            ..Default::default()
        });
        let storage_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Animated LM Storage View"),
            ..Default::default()
        });

        // Pack the three storage buffers. All three use `to_le_bytes` via
        // the repr(C) `#[derive(Copy)]` structs above and the contents are
        // bitcast-safe — same approach as the rest of the renderer's
        // packing helpers (see `sh_volume::write_descriptor_bytes`).
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
        // Dispatch tiles buffer is sized to the master (unfiltered) tile
        // count at creation and only partially filled each frame. Needs
        // `COPY_DST` so `queue.write_buffer` can upload the trimmed slice
        // every frame. Seed it with the master tile bytes so the first
        // frame's `DrawAll` path (same-count upload) is bitwise-identical
        // to the pre-cull shape.
        let dispatch_tiles_buffer = {
            use wgpu::util::DeviceExt;
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Animated LM Dispatch Tiles"),
                contents: &dispatch_tiles_bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            })
        };

        // Build the chunk → cell id table. One entry per animated chunk,
        // populated by walking each BVH leaf's `chunk_range_start..start+count`
        // and stamping the leaf's `cell_id`. Chunks not covered by any BVH
        // leaf keep `u32::MAX` and are always filtered out by the per-frame
        // cull — a defensive choice; in a valid bake every animated chunk
        // belongs to exactly one leaf.
        let chunk_cell_ids = build_chunk_cell_ids(bvh_leaves, section.chunk_rects.len());

        // Small uniform carrying the debug-viz selection. One-time upload;
        // nothing reads or writes it after init.
        let debug_buffer = {
            use wgpu::util::DeviceExt;
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Animated LM Debug Config"),
                contents: &debug_config.to_uniform_bytes(),
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };

        // Pipeline + bind groups.
        let compute_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Animated LM Compute BGL"),
            entries: &compute_bgl_entries(),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Animated LM Pipeline Layout"),
            bind_group_layouts: &[Some(uniform_bind_group_layout), Some(&compute_bgl)],
            immediate_size: 0,
        });

        // Concatenate curve helpers after the compose shader, matching the
        // pattern used for `forward.wgsl`.
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
            dummy_texture,
            forward_view,
            dispatch_state: Some(DispatchState {
                compose_pipeline,
                compute_bind_group,
                dispatch_tiles_buffer,
                master_tiles: dispatch_tiles,
                chunk_cell_ids,
                // Pre-size scratch buffers to the master count so the
                // `DrawAll` path — which pushes every tile — doesn't
                // realloc on the first frame.
                scratch_tiles: Vec::with_capacity(total_tiles as usize),
                scratch_bytes: Vec::with_capacity(dispatch_tiles_bytes.len()),
                prev_kept: u32::MAX,
                total_tiles,
            }),
        })
    }

    /// Whether a real compose dispatch will run. `false` for maps with no
    /// animated weight maps — callers skip allocating a GPU timing pair in
    /// that case so the timestamp slot isn't marked-but-unwritten.
    pub fn is_active(&self) -> bool {
        self.dispatch_state.is_some()
    }

    /// Dispatch the per-frame compose pass. No-op when the map carries no
    /// animated weight maps (the forward pass reads the dummy zero texture
    /// in that case). The atlas is zero-initialized by wgpu at creation and
    /// the compose pass writes every texel the forward pass samples, so no
    /// per-frame clear is required.
    ///
    /// Before encoding, the master dispatch-tile list is filtered against
    /// `visible`: tiles whose owning chunk belongs to an invisible cell are
    /// skipped. When every animated chunk is off-screen the compute pass
    /// is not encoded at all — the atlas keeps its prior-frame contents,
    /// which is safe because the forward pass will not sample any tile
    /// from an invisible cell this frame.
    ///
    /// `uniform_bind_group` must be the renderer's group-0 uniform bind
    /// group; this pass consumes `uniforms.time` to drive the curves.
    ///
    /// `timestamp_writes`: single pair covering the compose dispatch,
    /// allocated via `FrameTiming::compute_pass_writes`. When the dispatch
    /// is skipped (all invisible) the caller's timing pair goes
    /// marked-but-unwritten — that is acceptable because the higher-level
    /// `is_active()` gate still reports the pass as active and the frame
    /// may go several frames without a write; the timing window averages
    /// over a rolling buffer and tolerates missing samples.
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

        // Build the trimmed tile list. `DrawAll` pushes everything (same
        // behavior as the pre-cull shape); `Culled` pushes only tiles
        // whose owning chunk is in a visible cell.
        state.scratch_tiles.clear();
        match visible {
            VisibleCells::DrawAll => {
                state.scratch_tiles.extend_from_slice(&state.master_tiles);
            }
            VisibleCells::Culled(cells) => {
                // Build a local bitmask from the visible-cell list. For
                // typical cell counts (dozens) this is cheaper than a
                // HashSet lookup per tile and has no allocation.
                let mut bitmask = [0u32; VISIBLE_CELLS_WORDS];
                for &cell in cells {
                    if cell >= MAX_VISIBLE_CELLS {
                        // Out-of-range cell ids are silently skipped here;
                        // `compute_cull::write_bitmask_from_cells` already
                        // logs on the same condition so this path stays
                        // quiet.
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
            // Nothing to compose — don't upload and don't begin a pass.
            // The atlas keeps its prior-frame contents; the forward pass
            // will not sample any of those texels this frame.
            return;
        }

        // Pack the trimmed tile list into the reusable byte scratch and
        // upload. Use `queue.write_buffer`; the dispatch-tiles buffer was
        // created with `COPY_DST`.
        state.scratch_bytes.clear();
        pack_dispatch_tiles_into(&state.scratch_tiles, &mut state.scratch_bytes);
        queue.write_buffer(&state.dispatch_tiles_buffer, 0, &state.scratch_bytes);

        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Animated LM Compose"),
            timestamp_writes,
        });
        pass.set_bind_group(0, uniform_bind_group, &[]);
        pass.set_bind_group(1, &state.compute_bind_group, &[]);

        // Compose: one workgroup per kept dispatch tile, flat in x.
        pass.set_pipeline(&state.compose_pipeline);
        pass.dispatch_workgroups(kept, 1, 1);
    }
}

/// Walk the BVH leaves and stamp each chunk's owning cell id into
/// `chunk_cell_ids`. Chunks never referenced by any leaf retain `u32::MAX`
/// as a sentinel — those tiles can never be kept by the per-frame filter
/// (the cap check in `dispatch()` rejects them). In a valid PRL every
/// animated chunk belongs to exactly one BVH leaf's range, so the sentinel
/// path is defensive only.
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

fn compute_bgl_entries() -> [wgpu::BindGroupLayoutEntry; 8] {
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
                view_dimension: wgpu::TextureViewDimension::D2,
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
        // Dummy just needs to be bindable on the forward pass; no upload
        // path is required because `Rgba16Float` zero-initializes to zero
        // half-floats, which the forward shader reads as (0, 0, 0, 0).
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    })
}

fn create_storage_buffer(device: &wgpu::Device, label: &str, bytes: &[u8]) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    // wgpu rejects zero-sized storage-buffer bindings. All callers pass
    // at least one packed record (guaranteed by the dispatch-state path
    // rejecting empty `chunk_rects`). Use assert! (not debug_assert!) so a
    // future regression is caught in release builds before producing an
    // invalid wgpu buffer.
    assert!(!bytes.is_empty(), "{label} storage buffer would be empty");
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytes,
        usage: wgpu::BufferUsages::STORAGE,
    })
}

/// Expand `chunk_rects` into a flat `Vec<DispatchTile>` covering every
/// chunk rect with `ceil(w/8) × ceil(h/8)` 8×8 tiles. Tile iteration order
/// is y-major, x-minor — doesn't affect correctness but keeps debugging
/// predictable.
fn expand_dispatch_tiles(
    chunk_rects: &[postretro_level_format::animated_light_weight_maps::ChunkAtlasRect],
) -> Vec<DispatchTile> {
    let mut tiles = Vec::new();
    for (chunk_idx, rect) in chunk_rects.iter().enumerate() {
        if rect.width == 0 || rect.height == 0 {
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
                    _pad: 0,
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
    }
    bytes
}

fn pack_dispatch_tiles(tiles: &[DispatchTile]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(tiles));
    pack_dispatch_tiles_into(tiles, &mut bytes);
    bytes
}

/// Same layout as `pack_dispatch_tiles`, but appends into a caller-owned
/// buffer. Used by the per-frame filter path to avoid a `Vec` allocation
/// every frame.
fn pack_dispatch_tiles_into(tiles: &[DispatchTile], bytes: &mut Vec<u8>) {
    bytes.reserve(std::mem::size_of_val(tiles));
    for t in tiles {
        bytes.extend_from_slice(&t.chunk_idx.to_ne_bytes());
        bytes.extend_from_slice(&t.tile_origin_x.to_ne_bytes());
        bytes.extend_from_slice(&t.tile_origin_y.to_ne_bytes());
        bytes.extend_from_slice(&t._pad.to_ne_bytes());
    }
}

/// Run the spec's three cross-section invariants. Returns `Err` with a
/// descriptive message on the first failure; the caller logs and refuses
/// to load the map.
fn validate_cross_section(
    section: &AnimatedLightWeightMapsSection,
    animated_chunks: Option<&AnimatedLightChunksSection>,
    animated_light_count: u32,
) -> Result<(), String> {
    // Invariant 1: chunk_rects.len() == AnimatedLightChunks.chunks.len().
    // The compiler emits weight-maps and chunks together (see `main.rs`), so
    // a present weight-maps section with an absent chunks section is a
    // malformed PRL — hard-error rather than quietly skipping this check.
    match animated_chunks {
        Some(chunks) => {
            if section.chunk_rects.len() != chunks.chunks.len() {
                return Err(format!(
                    "chunk_rects.len() ({}) != AnimatedLightChunks.chunks.len() ({})",
                    section.chunk_rects.len(),
                    chunks.chunks.len(),
                ));
            }
        }
        None => {
            if !section.chunk_rects.is_empty() {
                return Err(format!(
                    "AnimatedLightWeightMaps present ({} chunk_rects) but \
                     AnimatedLightChunks section is missing — PRL is malformed",
                    section.chunk_rects.len(),
                ));
            }
        }
    }

    // Invariant 2: texel_offset prefix-sum.
    let mut running: u32 = 0;
    for (i, rect) in section.chunk_rects.iter().enumerate() {
        if rect.texel_offset != running {
            return Err(format!(
                "chunk_rects[{}].texel_offset ({}) != prefix sum ({})",
                i, rect.texel_offset, running,
            ));
        }
        running = running
            .checked_add(rect.width.checked_mul(rect.height).ok_or_else(|| {
                format!(
                    "chunk_rects[{}] width*height overflow ({} * {})",
                    i, rect.width, rect.height,
                )
            })?)
            .ok_or_else(|| format!("chunk_rects prefix sum overflow at index {i}"))?;
    }
    if section.offset_counts.len() as u32 != running {
        return Err(format!(
            "offset_counts.len() ({}) != Σ width×height ({})",
            section.offset_counts.len(),
            running,
        ));
    }

    // Invariant 3: every light_index is < animated_light_count.
    // Also verify each (offset, count) slice is in bounds.
    for (i, tl) in section.texel_lights.iter().enumerate() {
        if tl.light_index >= animated_light_count {
            return Err(format!(
                "texel_lights[{}].light_index ({}) >= animated_light_count ({})",
                i, tl.light_index, animated_light_count,
            ));
        }
    }
    for (i, oc) in section.offset_counts.iter().enumerate() {
        let end = (oc.offset as usize)
            .checked_add(oc.count as usize)
            .ok_or_else(|| format!("offset_counts[{i}] end overflow"))?;
        if end > section.texel_lights.len() {
            return Err(format!(
                "offset_counts[{}] range {}..{} exceeds texel_lights.len() ({})",
                i,
                oc.offset,
                end,
                section.texel_lights.len(),
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::animated_light_chunks::{
        AnimatedLightChunk, AnimatedLightChunksSection,
    };
    use postretro_level_format::animated_light_weight_maps::{
        ChunkAtlasRect, TexelLight, TexelLightEntry,
    };

    /// Build a stub `AnimatedLightChunksSection` with `n` empty chunks, so
    /// the validator's cross-section length check passes.
    fn mk_chunks(n: usize) -> AnimatedLightChunksSection {
        AnimatedLightChunksSection {
            chunks: (0..n)
                .map(|_| AnimatedLightChunk {
                    aabb_min: [0.0, 0.0, 0.0],
                    face_index: 0,
                    aabb_max: [1.0, 1.0, 1.0],
                    index_offset: 0,
                    uv_min: [0.0, 0.0],
                    uv_max: [1.0, 1.0],
                    index_count: 0,
                    _padding: 0,
                })
                .collect(),
            light_indices: Vec::new(),
        }
    }

    fn mk_rect(w: u32, h: u32, offset: u32) -> ChunkAtlasRect {
        ChunkAtlasRect {
            atlas_x: 0,
            atlas_y: 0,
            width: w,
            height: h,
            texel_offset: offset,
        }
    }

    #[test]
    fn compose_shader_parses_and_declares_debug_binding() {
        // Parse the concatenated compose + curve_eval source (same concat
        // the runtime does) with naga so the debug binding addition stays
        // syntactically sound without needing a GPU.
        let src = concat!(
            include_str!("../shaders/animated_lightmap_compose.wgsl"),
            "\n",
            include_str!("../shaders/curve_eval.wgsl"),
        );
        let module =
            naga::front::wgsl::parse_str(src).expect("compose shader should parse as WGSL");
        // Compose entry point exists; clear entry point has been removed
        // (the atlas is zero-initialized by wgpu and fully overwritten each
        // frame by the compose pass).
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
        // DebugConfig struct is declared.
        let has_debug_struct = module.types.iter().any(|(_, ty)| {
            matches!(&ty.inner, naga::TypeInner::Struct { .. })
                && ty.name.as_deref() == Some("DebugConfig")
        });
        assert!(has_debug_struct, "DebugConfig struct missing from shader");
    }

    #[test]
    fn debug_config_uniform_bytes_layout() {
        let cfg = AnimatedLmDebugConfig {
            mode: 2,
            isolate_slot: 7,
        };
        let bytes = cfg.to_uniform_bytes();
        assert_eq!(&bytes[0..4], &2u32.to_ne_bytes());
        assert_eq!(&bytes[4..8], &7u32.to_ne_bytes());
        assert_eq!(&bytes[8..12], &DEBUG_MAX_LIGHTS_PER_CHUNK.to_ne_bytes());
        assert_eq!(&bytes[12..16], &[0, 0, 0, 0]);
    }

    #[test]
    fn dispatch_tile_expansion_small_rect() {
        // 5×5 rect → single 8×8 tile.
        let tiles = expand_dispatch_tiles(&[mk_rect(5, 5, 0)]);
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].chunk_idx, 0);
        assert_eq!(tiles[0].tile_origin_x, 0);
        assert_eq!(tiles[0].tile_origin_y, 0);
    }

    #[test]
    fn dispatch_tile_expansion_exact_tile_boundary() {
        // 16×8 rect → two tiles in a single row.
        let tiles = expand_dispatch_tiles(&[mk_rect(16, 8, 0)]);
        assert_eq!(tiles.len(), 2);
        assert_eq!(tiles[0].tile_origin_x, 0);
        assert_eq!(tiles[1].tile_origin_x, 8);
    }

    #[test]
    fn dispatch_tile_expansion_partial_tile() {
        // 9×9 rect → ceil(9/8) × ceil(9/8) = 4 tiles.
        let tiles = expand_dispatch_tiles(&[mk_rect(9, 9, 0)]);
        assert_eq!(tiles.len(), 4);
    }

    #[test]
    fn dispatch_tile_expansion_multiple_chunks_preserves_index() {
        // Two rects — 8×8 (one tile) and 12×8 (two tiles).
        let tiles = expand_dispatch_tiles(&[mk_rect(8, 8, 0), mk_rect(12, 8, 64)]);
        assert_eq!(tiles.len(), 3);
        assert_eq!(tiles[0].chunk_idx, 0);
        assert_eq!(tiles[1].chunk_idx, 1);
        assert_eq!(tiles[2].chunk_idx, 1);
    }

    #[test]
    fn dispatch_tile_expansion_skips_zero_area() {
        let tiles = expand_dispatch_tiles(&[mk_rect(0, 8, 0), mk_rect(8, 0, 0), mk_rect(8, 8, 0)]);
        assert_eq!(tiles.len(), 1);
        // Index survives the skip — the non-empty chunk keeps its input position.
        assert_eq!(tiles[0].chunk_idx, 2);
    }

    fn mk_section(
        chunk_rects: Vec<ChunkAtlasRect>,
        offset_counts: Vec<TexelLightEntry>,
        texel_lights: Vec<TexelLight>,
    ) -> AnimatedLightWeightMapsSection {
        AnimatedLightWeightMapsSection {
            chunk_rects,
            offset_counts,
            texel_lights,
        }
    }

    #[test]
    fn validator_accepts_valid_section() {
        let section = mk_section(
            vec![mk_rect(2, 2, 0)],
            vec![
                TexelLightEntry {
                    offset: 0,
                    count: 1,
                },
                TexelLightEntry {
                    offset: 1,
                    count: 0,
                },
                TexelLightEntry {
                    offset: 1,
                    count: 0,
                },
                TexelLightEntry {
                    offset: 1,
                    count: 0,
                },
            ],
            vec![TexelLight {
                light_index: 0,
                weight: 0.5,
            }],
        );
        let chunks = mk_chunks(1);
        assert!(validate_cross_section(&section, Some(&chunks), 1).is_ok());
    }

    #[test]
    fn validator_rejects_bad_prefix_sum() {
        let section = mk_section(
            vec![mk_rect(2, 2, 0), mk_rect(1, 1, 5)], // expected offset 4, got 5
            vec![
                TexelLightEntry {
                    offset: 0,
                    count: 0
                };
                5
            ],
            vec![],
        );
        let chunks = mk_chunks(2);
        let err = validate_cross_section(&section, Some(&chunks), 0).unwrap_err();
        assert!(err.contains("prefix sum"), "unexpected error: {err}");
    }

    #[test]
    fn validator_rejects_out_of_range_light_index() {
        let section = mk_section(
            vec![mk_rect(1, 1, 0)],
            vec![TexelLightEntry {
                offset: 0,
                count: 1,
            }],
            vec![TexelLight {
                light_index: 42,
                weight: 1.0,
            }],
        );
        let chunks = mk_chunks(1);
        let err = validate_cross_section(&section, Some(&chunks), 5).unwrap_err();
        assert!(err.contains("light_index"), "unexpected error: {err}");
    }

    #[test]
    fn validator_rejects_offset_count_out_of_range() {
        let section = mk_section(
            vec![mk_rect(1, 1, 0)],
            vec![TexelLightEntry {
                offset: 0,
                count: 5, // but texel_lights only has 1 entry
            }],
            vec![TexelLight {
                light_index: 0,
                weight: 1.0,
            }],
        );
        let chunks = mk_chunks(1);
        let err = validate_cross_section(&section, Some(&chunks), 1).unwrap_err();
        assert!(err.contains("texel_lights.len"), "unexpected error: {err}");
    }

    #[test]
    fn validator_rejects_offset_counts_length_mismatch() {
        let section = mk_section(
            vec![mk_rect(2, 2, 0)],
            // Only 3 entries, should be 4.
            vec![
                TexelLightEntry {
                    offset: 0,
                    count: 0
                };
                3
            ],
            vec![],
        );
        let chunks = mk_chunks(1);
        let err = validate_cross_section(&section, Some(&chunks), 0).unwrap_err();
        assert!(err.contains("offset_counts.len"), "unexpected error: {err}");
    }

    /// Regression: a non-empty weight-map section with a missing chunks
    /// section is a malformed PRL (`main.rs` emits the two together). The
    /// validator must hard-error rather than quietly skipping the
    /// cross-section length check.
    #[test]
    fn validator_rejects_missing_chunks_when_weight_maps_present() {
        let section = mk_section(
            vec![mk_rect(1, 1, 0)],
            vec![TexelLightEntry {
                offset: 0,
                count: 0,
            }],
            vec![],
        );
        let err = validate_cross_section(&section, None, 0).unwrap_err();
        assert!(
            err.contains("AnimatedLightChunks") && err.contains("malformed"),
            "unexpected error: {err}",
        );
    }

    /// An empty weight-map section (no chunk rects) combined with a missing
    /// chunks section is still valid — the degradation path for maps with
    /// zero animated lights.
    #[test]
    fn validator_accepts_empty_weight_maps_without_chunks() {
        let section = mk_section(vec![], vec![], vec![]);
        assert!(validate_cross_section(&section, None, 0).is_ok());
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
        // Two leaves covering 5 total chunks: leaf 0 cell 7 owns chunks
        // 0..2, leaf 1 cell 9 owns chunks 2..5.
        let leaves = [mk_leaf(7, 0, 2), mk_leaf(9, 2, 3)];
        let ids = build_chunk_cell_ids(&leaves, 5);
        assert_eq!(ids, vec![7, 7, 9, 9, 9]);
    }

    #[test]
    fn build_chunk_cell_ids_leaves_unreferenced_chunks_as_sentinel() {
        // Leaf covers chunk 0 only; chunk 1 is unreferenced and must
        // stay at the `u32::MAX` sentinel so the filter rejects it.
        let leaves = [mk_leaf(3, 0, 1)];
        let ids = build_chunk_cell_ids(&leaves, 2);
        assert_eq!(ids, vec![3, u32::MAX]);
    }

    #[test]
    fn build_chunk_cell_ids_clamps_out_of_range_leaf() {
        // Defensive: a malformed leaf claiming more chunks than exist
        // must not panic or write out of bounds.
        let leaves = [mk_leaf(5, 0, 10)];
        let ids = build_chunk_cell_ids(&leaves, 3);
        assert_eq!(ids, vec![5, 5, 5]);
    }

    /// Compose-pass output atlas dimensions match the static lightmap atlas.
    /// Both atlases share one UV in the forward shader, so a mismatch would
    /// silently corrupt sampling. No wgpu device required — compare constants.
    #[test]
    fn compose_atlas_dimensions_match_static_lightmap() {
        assert_eq!(
            ANIMATED_ATLAS_SIZE, 1024,
            "animated lightmap atlas must match the 1024² static lightmap atlas"
        );
    }
}
