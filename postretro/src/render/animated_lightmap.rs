// Animated-lightmap compose compute pass: per-frame clear + compose of the
// animated-light contribution atlas. Samples the same descriptor and
// `anim_samples` buffers as the SH path (`sh_volume.rs`) via
// `AnimatedLightBuffers`, so scripting toggles to `is_active` affect both
// consumers in one upload.
//
// See: context/plans/in-progress/animated-light-weight-maps/index.md §Task 5
//      context/lib/rendering_pipeline.md §4
//
// Dispatch-limit choice: this module asserts at map load that the total
// 8×8 tile count fits in `max_compute_workgroups_per_dimension` (65535 at
// wgpu defaults). Bundled maps stay far below that; the 2D-dispatch
// fallback described in the plan is intentionally not wired up. If a
// future map trips the cap, extend the dispatch here and compute the flat
// index in `animated_lightmap_compose.wgsl::compose_main`.

use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;

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

/// Matches Spec 1's proposed `MAX_ANIMATED_LIGHTS_PER_CHUNK = 4`. Used as
/// the heatmap denominator in debug mode 1 — saturating at 1.0 when a
/// covered texel references the cap. Duplicated here (rather than imported
/// from the format crate) because Spec 1 has not yet exported the
/// constant; update this if/when it does.
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
    clear_pipeline: wgpu::ComputePipeline,
    compose_pipeline: wgpu::ComputePipeline,
    compute_bind_group: wgpu::BindGroup,
    /// Number of compose workgroups (one per `DispatchTile` in
    /// `dispatch_tiles`). Checked against `MAX_WORKGROUPS_PER_DIM` at
    /// construction so the dispatch call can't trigger a wgpu validation
    /// error at frame time.
    compose_workgroup_count: u32,
    /// Atlas dimensions, cached for the clear dispatch grid calculation.
    atlas_size: u32,
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
    /// - `uniform_bind_group_layout`: group-0 layout from the renderer. Must
    ///   have COMPUTE in `visibility` so the same layout works here.
    ///
    /// Returns an error string when cross-section validation fails; the
    /// caller should log and refuse to load the map.
    pub fn new(
        device: &wgpu::Device,
        weight_maps: Option<&AnimatedLightWeightMapsSection>,
        animated_chunks: Option<&AnimatedLightChunksSection>,
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
        // no `COPY_DST` because the clear is done via compute dispatch.
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
        let dispatch_tiles_buffer =
            create_storage_buffer(device, "Animated LM Dispatch Tiles", &dispatch_tiles_bytes);

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

        let clear_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Animated LM Clear Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("clear_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
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

        Ok(Self {
            atlas_texture: Some(atlas_texture),
            dummy_texture,
            forward_view,
            dispatch_state: Some(DispatchState {
                clear_pipeline,
                compose_pipeline,
                compute_bind_group,
                compose_workgroup_count,
                atlas_size: ANIMATED_ATLAS_SIZE,
            }),
        })
    }

    /// Dispatch the per-frame clear + compose passes. No-op when the map
    /// carries no animated weight maps (the forward pass reads the dummy
    /// zero texture in that case).
    ///
    /// `uniform_bind_group` must be the renderer's group-0 uniform bind
    /// group; this pass consumes `uniforms.time` to drive the curves.
    ///
    /// `timestamp_writes`: single pair covering clear + compose folded
    /// together, allocated via `FrameTiming::compute_pass_writes`. Folding
    /// both passes under one timing pair keeps the telemetry compact and
    /// still isolates the animated-LM work from the BVH cull.
    /// Whether a real compose dispatch will run. `false` for maps with no
    /// animated weight maps — callers skip allocating a GPU timing pair in
    /// that case so the timestamp slot isn't marked-but-unwritten.
    pub fn is_active(&self) -> bool {
        self.dispatch_state.is_some()
    }

    pub fn dispatch(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        let Some(state) = &self.dispatch_state else {
            return;
        };

        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Animated LM Compose"),
            timestamp_writes,
        });
        pass.set_bind_group(0, uniform_bind_group, &[]);
        pass.set_bind_group(1, &state.compute_bind_group, &[]);

        // Clear: one invocation per atlas texel, 8×8 workgroup.
        let clear_groups = state.atlas_size.div_ceil(8);
        pass.set_pipeline(&state.clear_pipeline);
        pass.dispatch_workgroups(clear_groups, clear_groups, 1);

        // Compose: one workgroup per dispatch tile, flat in x.
        pass.set_pipeline(&state.compose_pipeline);
        pass.dispatch_workgroups(state.compose_workgroup_count, 1, 1);
    }
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
    // rejecting empty `chunk_rects`), so a debug-assert suffices.
    debug_assert!(!bytes.is_empty(), "{label} storage buffer would be empty");
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
    for t in tiles {
        bytes.extend_from_slice(&t.chunk_idx.to_ne_bytes());
        bytes.extend_from_slice(&t.tile_origin_x.to_ne_bytes());
        bytes.extend_from_slice(&t.tile_origin_y.to_ne_bytes());
        bytes.extend_from_slice(&t._pad.to_ne_bytes());
    }
    bytes
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
    // The AnimatedLightChunks runtime loader is optional today. When the
    // chunks section is missing we still validate the internal
    // prefix-sum + light-index invariants; only the cross-section length
    // check is skipped.
    if let Some(chunks) = animated_chunks {
        if section.chunk_rects.len() != chunks.chunks.len() {
            return Err(format!(
                "chunk_rects.len() ({}) != AnimatedLightChunks.chunks.len() ({})",
                section.chunk_rects.len(),
                chunks.chunks.len(),
            ));
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
    use postretro_level_format::animated_light_weight_maps::{
        ChunkAtlasRect, TexelLight, TexelLightEntry,
    };

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
        // Both entry points exist.
        let has_clear = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "clear_main" && ep.stage == naga::ShaderStage::Compute);
        let has_compose = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "compose_main" && ep.stage == naga::ShaderStage::Compute);
        assert!(has_clear, "clear_main missing");
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
        assert!(validate_cross_section(&section, None, 1).is_ok());
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
        let err = validate_cross_section(&section, None, 0).unwrap_err();
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
        let err = validate_cross_section(&section, None, 5).unwrap_err();
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
        let err = validate_cross_section(&section, None, 1).unwrap_err();
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
        let err = validate_cross_section(&section, None, 0).unwrap_err();
        assert!(err.contains("offset_counts.len"), "unexpected error: {err}");
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
