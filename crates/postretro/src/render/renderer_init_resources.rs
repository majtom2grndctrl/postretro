// GPU device acquisition and lighting bind-group construction, factored out of
// `Renderer::new` to keep the constructor within the module size budget.
// See: context/lib/rendering_pipeline.md §4

use super::*;

/// Minimum `max_texture_array_layers` the engine requires. The lightmap
/// irradiance + direction atlases are `texture_2d_array`; charts that overflow
/// one atlas layer spill into additional layers. 256 is wgpu's default for this
/// limit and the WebGPU spec floor, comfortably above the layer counts the bake
/// produces. Requested in `required_limits` (the hard backstop) and pre-checked
/// against the adapter so an under-spec adapter fails with a named diagnostic
/// before `request_device`.
const REQUIRED_MAX_TEXTURE_ARRAY_LAYERS: u32 = 256;

/// Whether an adapter's granted `max_texture_array_layers` clears the engine's
/// floor. Pure comparison, factored out so the abort path is unit-testable —
/// no real adapter exposes a limit below 256 to exercise the full bail.
fn array_layers_sufficient(limit: u32) -> bool {
    limit >= REQUIRED_MAX_TEXTURE_ARRAY_LAYERS
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn request_renderer_device(
    adapter: &wgpu::Adapter,
    cube_array_supported: bool,
    enable_gpu_timing: bool,
    gpu_timing_requested: bool,
    gpu_timing_supported: bool,
) -> Result<(wgpu::Device, wgpu::Queue)> {
    let adapter_features = adapter.features();
    let mut required_features = wgpu::Features::TEXTURE_COMPRESSION_BC;
    if enable_gpu_timing {
        required_features |= wgpu::Features::TIMESTAMP_QUERY;
    } else if gpu_timing_requested && !gpu_timing_supported {
        log::warn!(
            "[Renderer] POSTRETRO_GPU_TIMING=1 requested but adapter \
                 lacks TIMESTAMP_QUERY support — running without GPU timing"
        );
    }

    // The forward pass binds more sampled textures per stage than wgpu's
    // *default* request (4 bind groups) would carry, so we request the exact
    // count the pipelines need. This stays under the WebGPU spec floor of 16
    // (`wgpu::Limits::defaults().max_sampled_textures_per_shader_stage`), and
    // every targeted backend reports far higher (Metal/AMD = 128) — the
    // adapter pre-check below confirms the granted maximum still covers it.
    //
    // Derived (14 when CUBE_ARRAY is supported, 13 without) from the actual
    // BGLs that compose the forward pipeline layout, so it can never drift from
    // the real binding count:
    //   Group 1 — material (3): diffuse, specular, normal
    //   Group 3 — SH volume (3): octahedral atlas + depth-moments
    //                            + direct static-light atlas (billboard samples it in
    //                              the VERTEX stage; entry is VERTEX | FRAGMENT so it
    //                              counts against the fragment budget; forward/fog
    //                              carry the entry but never sample it)
    //   Group 4 — lightmap (4): static irradiance, static dominant-direction,
    //                           animated-contribution atlas, animated dominant-direction
    //   Group 5 — shadow (4 with CUBE_ARRAY, else 3): spot-shadow depth array (binding 0),
    //                           SDF shadow factor (binding 3), scene depth (binding 4),
    //                           point-light cube-array depth (binding 5; present only when
    //                           CUBE_ARRAY_TEXTURES is supported)
    // 16 is the WebGPU spec floor and wgpu's `Limits::default()` value; it is
    // also the hard ceiling on Metal (macOS) and is universally supported on
    // all desktop adapters. We use it as a fixed design budget rather than
    // deriving the exact binding count here — the unit test
    // `forward_pipeline_sampled_texture_request_matches_bgl_definitions`
    // verifies that the derived count stays within this budget independently.
    const REQUIRED_SAMPLED_TEXTURES: u32 = 16;
    // Pull the count helpers onto the runtime path (they are otherwise
    // test-only), so overflowing the budget trips here in debug builds.
    // debug-only because CI has no GPU: a release panic at pipeline creation
    // would be uncatchable, and the headless test covers the same invariant.
    // `#[cfg(debug_assertions)]` on the statement: the count helper is itself
    // debug-only, so referencing it must vanish from release builds too (a bare
    // `debug_assert!` still *compiles* its arguments in release).
    #[cfg(debug_assertions)]
    debug_assert!(
        forward_pipeline_sampled_texture_count(cube_array_supported) <= REQUIRED_SAMPLED_TEXTURES,
        "forward pipeline sampled-texture count ({}) exceeds the requested \
             budget ({}); switch to bindless (TEXTURE_BINDING_ARRAY) rather than \
             raising the limit (16 is Metal's hard ceiling)",
        forward_pipeline_sampled_texture_count(cube_array_supported),
        REQUIRED_SAMPLED_TEXTURES
    );
    // Billboard lighting runs in `vs_main` (per-vertex SH indirect+direct,
    // static-specular, dynamic-diffuse); the group-6 instance storage buffer is
    // VERTEX-read (see §7.4). wgpu charges `max_storage_buffers_per_shader_stage` against
    // the BGL *entry* set per stage — every VERTEX-visible storage entry across the
    // Billboard Pipeline Layout's groups counts, read or not. The downlevel/WebGPU
    // default ceiling (we do not raise it — broad hardware compat for a
    // modder-friendly retro FPS) is 8. Six are genuinely vertex-read; if a shared
    // BGL re-widens an unused storage entry to VERTEX the count hits 9 and pipeline
    // creation fails on real GPUs (headless CI never triggers it). debug-only for
    // the same reason as the texture budget above.
    // Gated as a block: both the helper and the budget const are debug-only,
    // so neither is referenced in release (where the helper does not exist).
    #[cfg(debug_assertions)]
    {
        const MAX_VERTEX_STORAGE_BUFFERS: u32 = 8;
        debug_assert!(
            billboard_pipeline_vertex_storage_buffer_count() <= MAX_VERTEX_STORAGE_BUFFERS,
            "billboard pipeline VERTEX-visible storage-buffer count ({}) exceeds the \
                 downlevel-default max_storage_buffers_per_shader_stage ({}); trim VERTEX \
                 visibility from storage entries vs_main does not read, or consolidate \
                 buffers — do NOT raise the device limit (it breaks modest-spec adapters)",
            billboard_pipeline_vertex_storage_buffer_count(),
            MAX_VERTEX_STORAGE_BUFFERS
        );
    }
    const REQUIRED_STORAGE_TEXTURES: u32 = 4;
    // Stopgap: SH compose's flat delta-probe storage buffer outgrows the
    // WebGPU spec floor (128 MiB) on maps with many animated lights because
    // it bakes a dense AABB grid per light. 512 MiB covers current maps on
    // mainstream desktop adapters (which report 2 GiB+), but it is a
    // load-bearing dependency on above-spec hardware.
    // context/plans/drafts/perf-animated-sh-light-culling/index.md
    // tracks the fix: sparse per-light delta storage that keeps the total
    // binding under the 128 MiB spec floor regardless of light count.
    const REQUIRED_STORAGE_BUFFER_BINDING_SIZE: u64 = 512 * 1024 * 1024;
    // Lightmap atlases bake up to 8192² (see
    // `crates/level-compiler/src/lightmap_bake.rs::MAX_ATLAS_DIMENSION`).
    // The bake is a CLI with no GPU device, so its cap is a fixed constant —
    // the runtime makes that requirement explicit by requesting the limit
    // here and refusing under-spec adapters in the pre-check below. wgpu's
    // default for this field is already 8192; setting it explicitly
    // formalizes the dependency.
    const REQUIRED_MAX_TEXTURE_DIMENSION_2D: u32 = 8192;
    // Single-buffer ceiling. wgpu defaults to 256 MiB; the dev-tools SH
    // irradiance readback (full-atlas Rgba16Float copy) overruns it on large
    // maps (~327 MiB on stress-warren-crates). 2 GiB clears that with headroom
    // yet stays low enough to flag a runaway allocation here rather than balloon
    // silently. It also sits within reach of real adapters: the dev Apple-Silicon
    // device caps below 3 GiB, so a higher floor (e.g. 4 GiB) rejects it.
    // Revisit against a known target-hardware floor once shipping maps exist.
    const REQUIRED_MAX_BUFFER_SIZE: u64 = 2 * 1024 * 1024 * 1024;
    let adapter_limits = adapter.limits();
    let required_limits = wgpu::Limits {
        max_bind_groups: 8,
        max_sampled_textures_per_shader_stage: REQUIRED_SAMPLED_TEXTURES,
        max_storage_textures_per_shader_stage: REQUIRED_STORAGE_TEXTURES,
        max_storage_buffer_binding_size: REQUIRED_STORAGE_BUFFER_BINDING_SIZE,
        max_texture_dimension_2d: REQUIRED_MAX_TEXTURE_DIMENSION_2D,
        max_texture_array_layers: REQUIRED_MAX_TEXTURE_ARRAY_LAYERS,
        max_buffer_size: REQUIRED_MAX_BUFFER_SIZE,
        ..wgpu::Limits::default()
    };

    // Pre-check so an under-spec adapter fails with a named error here
    // rather than an opaque `request_device` rejection or a deferred
    // pipeline-creation crash.
    if !adapter_features.contains(wgpu::Features::TEXTURE_COMPRESSION_BC) {
        anyhow::bail!(
            "GPU adapter lacks required feature TEXTURE_COMPRESSION_BC \
                 (needed for BC5-compressed normal maps); this engine requires \
                 a desktop GPU with BC texture support"
        );
    }
    if adapter_limits.max_sampled_textures_per_shader_stage < REQUIRED_SAMPLED_TEXTURES {
        anyhow::bail!(
            "GPU adapter supports only {} sampled textures per shader stage; \
                 the forward pass requires {}",
            adapter_limits.max_sampled_textures_per_shader_stage,
            REQUIRED_SAMPLED_TEXTURES
        );
    }
    if adapter_limits.max_storage_textures_per_shader_stage < REQUIRED_STORAGE_TEXTURES {
        anyhow::bail!(
            "GPU adapter supports only {} storage textures per shader stage; \
                 the SH compose pass requires {}",
            adapter_limits.max_storage_textures_per_shader_stage,
            REQUIRED_STORAGE_TEXTURES
        );
    }
    if adapter_limits.max_storage_buffer_binding_size < REQUIRED_STORAGE_BUFFER_BINDING_SIZE {
        anyhow::bail!(
            "GPU adapter supports only {} bytes per storage buffer binding; \
                 the SH compose delta-probe buffer requires {} (stopgap limit — \
                 see context/plans/drafts/perf-animated-sh-light-culling/index.md \
                 for the sparse-storage fix that removes this requirement)",
            adapter_limits.max_storage_buffer_binding_size,
            REQUIRED_STORAGE_BUFFER_BINDING_SIZE
        );
    }
    if adapter_limits.max_buffer_size < REQUIRED_MAX_BUFFER_SIZE {
        anyhow::bail!(
            "GPU adapter allows a maximum single buffer of {} bytes; this engine \
                 requires {} (2 GiB) for large scene and diagnostic buffers",
            adapter_limits.max_buffer_size,
            REQUIRED_MAX_BUFFER_SIZE
        );
    }
    // The lightmap irradiance + animated atlases (`Rgba16Float`) are sampled
    // with hardware linear filtering (group-4 BGL declares `filterable:true`).
    // Linear filtering of 16-bit-float textures is core WebGPU and mandated
    // on every targeted backend (Vulkan/Metal/DX12), but check anyway so a
    // non-filterable adapter fails here with a named message rather than an
    // opaque `create_bind_group` crash later. See context/lib/rendering_pipeline.md §4.
    if !crate::lighting::lightmap::atlas_format_filterable(adapter) {
        anyhow::bail!(
            "[Renderer] GPU adapter does not support linear filtering of \
                 Rgba16Float; PostRetro requires it for lightmap irradiance \
                 sampling. All supported backends (Vulkan/Metal/DX12) provide \
                 this — an adapter lacking it is below the supported floor"
        );
    }
    // BC6H is the default irradiance storage at rest — the bake compresses
    // the irradiance atlas to `Bc6hRgbUfloat` and the runtime uploads it
    // through the same `Float { filterable: true }` BGL slot as the
    // uncompressed debug variant. `TEXTURE_COMPRESSION_BC` is already
    // required above; this fail-fast sibling check confirms the adapter
    // also advertises `FILTERABLE` for `Bc6hRgbUfloat` specifically, so a
    // misconfigured adapter fails here with a named message instead of an
    // opaque `create_bind_group` crash later. Matches the
    // `atlas_format_filterable` (`Rgba16Float`) check above.
    if !crate::lighting::lightmap::bc6h_irradiance_filterable(adapter) {
        anyhow::bail!(
            "[Renderer] GPU adapter does not support linear filtering of \
                 Bc6hRgbUfloat; PostRetro requires it for the compressed \
                 lightmap irradiance atlas. All supported backends \
                 (Vulkan/Metal/DX12) provide this — an adapter lacking it is \
                 below the supported floor"
        );
    }
    // The lightmap bake's `MAX_ATLAS_DIMENSION` (8192) is a fixed CLI-side
    // constant chosen to match guaranteed device support. Mirror that
    // requirement here: a baked atlas can be up to 8192² in either axis, so
    // an adapter that grants less cannot host one. Fail-fast with a named
    // message rather than a deferred texture-creation crash. wgpu's default
    // floor is 8192, so any in-spec desktop adapter satisfies this.
    if adapter_limits.max_texture_dimension_2d < REQUIRED_MAX_TEXTURE_DIMENSION_2D {
        anyhow::bail!(
            "[Renderer] GPU adapter grants max_texture_dimension_2d = {}; \
                 PostRetro requires at least {} to host the lightmap atlas at \
                 its baked ceiling. All supported backends (Vulkan/Metal/DX12) \
                 provide this — an adapter granting less is below the supported floor",
            adapter_limits.max_texture_dimension_2d,
            REQUIRED_MAX_TEXTURE_DIMENSION_2D,
        );
    }
    // The lightmap irradiance + direction atlases are `texture_2d_array`; a baked
    // section can carry multiple layers when its charts overflow one atlas layer.
    // Mirror the dimension pre-check: an adapter granting fewer array layers than
    // we request cannot host a multi-layer atlas, so fail-fast with a named
    // message rather than a deferred texture-creation crash. wgpu's default floor
    // is 256, so any in-spec desktop adapter satisfies this.
    if !array_layers_sufficient(adapter_limits.max_texture_array_layers) {
        anyhow::bail!(
            "[Renderer] GPU adapter grants max_texture_array_layers = {}; \
                 PostRetro requires at least {} to host the multi-layer lightmap \
                 atlas. All supported backends (Vulkan/Metal/DX12) provide this — \
                 an adapter granting less is below the supported floor",
            adapter_limits.max_texture_array_layers,
            REQUIRED_MAX_TEXTURE_ARRAY_LAYERS,
        );
    }

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("Postretro Device"),
        required_features,
        required_limits,
        ..Default::default()
    }))
    .context("failed to create GPU device")?;

    Ok((device, queue))
}

pub(crate) struct LightingResources {
    pub lights_buffer: wgpu::Buffer,
    pub influence_buffer: wgpu::Buffer,
    pub spec_lights_buffer: wgpu::Buffer,
    pub chunk_grid_info_buffer: wgpu::Buffer,
    pub chunk_grid_offsets_buffer: wgpu::Buffer,
    pub chunk_grid_indices_buffer: wgpu::Buffer,
    pub lighting_bind_group: wgpu::BindGroup,
}

pub(crate) fn build_lighting_bind_group(
    device: &wgpu::Device,
    lighting_bind_group_layout: &wgpu::BindGroupLayout,
    level_lights: &[MapLight],
    dynamic_influences: &[LightInfluence],
    geometry: Option<&LevelGeometry>,
) -> LightingResources {
    // wgpu rejects zero-size storage buffers — pad to one dummy; light_count stays 0.
    let lights_data = if !level_lights.is_empty() {
        pack_lights(level_lights)
    } else {
        vec![0u8; GPU_LIGHT_SIZE]
    };
    let lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Direct Lights Storage Buffer"),
        contents: &lights_data,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });

    // Influence volume buffer — same dummy strategy as lights.
    let influence_data = if !dynamic_influences.is_empty() {
        influence::pack_influence(dynamic_influences)
    } else {
        vec![0u8; 16]
    };
    let influence_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Light Influence Storage Buffer"),
        contents: &influence_data,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });

    // Specular-only static lights; 1-record dummy avoids zero-size storage binding.
    let spec_lights_data = {
        let packed = geometry
            .map(|g| pack_spec_lights(g.lights))
            .unwrap_or_default();
        if packed.is_empty() {
            vec![0u8; SPEC_LIGHT_SIZE]
        } else {
            packed
        }
    };
    let spec_lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Spec-Only Lights Storage Buffer"),
        contents: &spec_lights_data,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });

    // Absent → fallback payload with has_chunk_grid=0; shader iterates full spec buffer.
    let chunk_grid = match geometry.and_then(|g| g.chunk_light_list) {
        Some(sec) => ChunkGrid::from_section(sec),
        None => ChunkGrid::fallback(),
    };
    if chunk_grid.present {
        log::info!("[Renderer] ChunkLightList active (spec-only path is spatially partitioned)");
    } else {
        log::info!(
            "[Renderer] ChunkLightList absent — specular path iterates the full spec buffer"
        );
    }
    let chunk_grid_info_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Chunk Grid Info Uniform"),
        contents: &chunk_grid.grid_info,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let chunk_grid_offsets_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Chunk Grid Offset Table"),
        contents: &chunk_grid.offset_table,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    let chunk_grid_indices_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Chunk Grid Index List"),
        contents: &chunk_grid.index_list,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });

    let lighting_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Lighting Bind Group"),
        layout: lighting_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: lights_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: influence_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: spec_lights_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: chunk_grid_info_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: chunk_grid_offsets_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: chunk_grid_indices_buffer.as_entire_binding(),
            },
        ],
    });

    LightingResources {
        lights_buffer,
        influence_buffer,
        spec_lights_buffer,
        chunk_grid_info_buffer,
        chunk_grid_offsets_buffer,
        chunk_grid_indices_buffer,
        lighting_bind_group,
    }
}

pub(crate) struct ShadowVsResources {
    pub shadow_vs_stride: u32,
    pub shadow_vs_uniform_buffer: wgpu::Buffer,
    pub shadow_vs_bind_group: wgpu::BindGroup,
    pub cube_shadow_vs_uniform_buffer: wgpu::Buffer,
    pub cube_shadow_vs_bind_group: wgpu::BindGroup,
}

pub(crate) fn build_shadow_vs_resources(
    device: &wgpu::Device,
    shadow_vs_bgl: &wgpu::BindGroupLayout,
) -> ShadowVsResources {
    // min_uniform_buffer_offset_alignment required for dynamic-offset bindings.
    let min_ubo_align = device.limits().min_uniform_buffer_offset_alignment.max(64);
    let shadow_vs_stride = min_ubo_align;
    let shadow_vs_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Spot Shadow VS Uniforms"),
        size: (shadow_vs_stride as u64) * (crate::lighting::spot_shadow::SHADOW_POOL_SIZE as u64),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let shadow_vs_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Spot Shadow VS Bind Group"),
        layout: shadow_vs_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &shadow_vs_uniform_buffer,
                offset: 0,
                size: std::num::NonZeroU64::new(64),
            }),
        }],
    });

    // --- Cube point-shadow VS uniforms -----------------------------------
    // The cube pool itself was built earlier (its sampling view feeds the
    // group-5 BGL). Its per-face light-space matrices ride a dynamic-offset
    // uniform buffer shaped exactly like `shadow_vs_*` (reusing
    // `shadow_vs_bgl`), one slot per `(cube slot, face)` pair. The
    // skinned-depth pipeline binds it at group 0 just like the spot path,
    // proving the cube-ready contract.
    //
    // Total capacity = `shadow_vs_stride × CUBE_COUNT × CUBE_FACES` (every face
    // of every slot gets its own dynamic-offset slot). A render selects a face
    // via dynamic offset = `layer * shadow_vs_stride`, where the layer index is
    // `slot * CUBE_FACES + face` (matching `CubeShadowPool::face_layer`).
    let cube_face_count =
        crate::lighting::cube_shadow::CUBE_COUNT * crate::lighting::cube_shadow::CUBE_FACES;
    let cube_shadow_vs_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Cube Shadow VS Uniforms"),
        size: (shadow_vs_stride as u64) * (cube_face_count as u64),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let cube_shadow_vs_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Cube Shadow VS Bind Group"),
        layout: shadow_vs_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &cube_shadow_vs_uniform_buffer,
                offset: 0,
                size: std::num::NonZeroU64::new(64),
            }),
        }],
    });

    ShadowVsResources {
        shadow_vs_stride,
        shadow_vs_uniform_buffer,
        shadow_vs_bind_group,
        cube_shadow_vs_uniform_buffer,
        cube_shadow_vs_bind_group,
    }
}

pub(crate) struct UniformBindGroups {
    pub uniform_buffer: wgpu::Buffer,
    pub uniform_bind_group_layout: wgpu::BindGroupLayout,
    pub uniform_bind_group: wgpu::BindGroup,
    pub texture_bind_group_layout: wgpu::BindGroupLayout,
    pub lighting_bind_group_layout: wgpu::BindGroupLayout,
}

pub(crate) fn build_uniform_bind_groups(
    device: &wgpu::Device,
    uniform_data: &[u8],
) -> UniformBindGroups {
    let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Uniform Buffer"),
        contents: uniform_data,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });

    let uniform_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Uniform Bind Group Layout"),
            entries: &uniform_bind_group_layout_entries(),
        });

    let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Uniform Bind Group"),
        layout: &uniform_bind_group_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buffer.as_entire_binding(),
        }],
    });

    // Group 1: 0=diffuse(sRGB), 2=specular(R8), 3=shininess,
    //          4=normal(Rgba8Unorm, NOT sRGB; n = sample.rgb*2-1),
    //          5=aniso_sampler (linear+anisotropic, Post Retro).
    // Binding 1 is intentionally vacated (former nearest sampler); the
    // aniso sampler stays at 5 — non-contiguous bindings are valid.
    let texture_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Material Bind Group Layout"),
            entries: &material_bind_group_layout_entries(),
        });

    let lighting_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lighting Bind Group Layout"),
            entries: &lighting_bind_group_layout_entries(),
        });

    UniformBindGroups {
        uniform_buffer,
        uniform_bind_group_layout,
        uniform_bind_group,
        texture_bind_group_layout,
        lighting_bind_group_layout,
    }
}

pub(crate) fn build_frame_timing(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    enable_gpu_timing: bool,
) -> Option<FrameTiming> {
    if enable_gpu_timing {
        log::info!("[Renderer] GPU timing enabled (POSTRETRO_GPU_TIMING=1)");
        let mut pass_labels = vec![""; TIMING_PAIR_COUNT];
        pass_labels[TIMING_PAIR_CULL] = "cull";
        pass_labels[TIMING_PAIR_ANIMATED_LM_COMPOSE] = "animated_lm_compose";
        pass_labels[TIMING_PAIR_DEPTH_PREPASS] = "depth_prepass";
        pass_labels[TIMING_PAIR_SDF_SHADOW] = "sdf_shadow";
        pass_labels[TIMING_PAIR_FORWARD] = "forward";
        pass_labels[TIMING_PAIR_SH_COMPOSE] = "sh_compose";
        pass_labels[TIMING_PAIR_SMOKE] = "smoke";
        Some(FrameTiming::new(device, queue, pass_labels))
    } else {
        None
    }
}

pub(crate) fn build_initial_uniform_data(
    view_proj: Mat4,
    ambient_floor: f32,
    light_count: u32,
) -> [u8; UNIFORM_SIZE] {
    build_uniform_data(&FrameUniforms {
        view_proj,
        camera_position: Vec3::ZERO,
        ambient_floor,
        light_count,
        time: 0.0,
        lighting_isolation: LightingIsolation::Normal,
        indirect_scale: DEFAULT_INDIRECT_SCALE,
        // No level loaded yet — per-frame uniform upload in
        // `update_per_frame_uniforms` reflects `has_sdf_atlas()` +
        // `lightmap_mode()` once geometry installs.
        sdf_shadow_flags: 0,
        sdf_shadow_mode: SdfShadowMode::On,
        sdf_force_visibility_one: false,
        dynamic_direct_scale: DEFAULT_DYNAMIC_DIRECT_SCALE,
        dynamic_direct_isolation: DynamicDirectIsolation::Combined,
        // No level loaded yet — `has_direct` reflects the direct SH section
        // once geometry installs (see `update_per_frame_uniforms`).
        has_direct: false,
    })
}

pub(crate) fn build_placeholder_textures(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture_bind_group_layout: &wgpu::BindGroupLayout,
    mip_count_aniso_samplers: &std::collections::HashMap<u32, wgpu::Sampler>,
) -> (Vec<LoadedTexture>, Vec<GpuTexture>) {
    let mut loaded_textures: Vec<LoadedTexture> = Vec::new();
    let mut gpu_textures: Vec<GpuTexture> = Vec::new();
    {
        let placeholder = placeholder_loaded_texture(device, queue);
        let aniso_sampler = mip_count_aniso_samplers
            .get(&1)
            .expect("mip_count 1 aniso seeded above");
        let bind_group = build_material_bind_group(
            device,
            texture_bind_group_layout,
            &placeholder,
            aniso_sampler,
            Material::Default,
            "Placeholder Material",
        );
        loaded_textures.push(placeholder);
        gpu_textures.push(GpuTexture { bind_group });
    }
    (loaded_textures, gpu_textures)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Array-layer floor guard. No real adapter exposes `max_texture_array_layers`
    /// below 256, so the full bail in `request_renderer_device` can't be exercised
    /// against hardware — this pins the pure predicate it pivots on instead.
    #[test]
    fn array_layers_sufficient_floor() {
        assert!(
            !array_layers_sufficient(255),
            "below the 256-layer floor must be rejected",
        );
        assert!(
            array_layers_sufficient(REQUIRED_MAX_TEXTURE_ARRAY_LAYERS),
            "exactly at the floor must be accepted",
        );
        assert!(
            array_layers_sufficient(256),
            "256 (the floor) must be accepted",
        );
        assert!(
            array_layers_sufficient(2048),
            "well above the floor must be accepted",
        );
    }
}
