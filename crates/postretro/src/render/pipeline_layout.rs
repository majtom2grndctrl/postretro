// Bind-group-layout entry builders, shader source consts, depth helpers, and
// the no-CUBE_ARRAY shader-variant transform for the renderer pipelines.
// See: context/lib/rendering_pipeline.md

use super::*;

// `curve_eval.wgsl` reads `anim_samples`; `sh_sample.wgsl` reads
// `sh_total_atlas`, `sh_depth_moments`, and `sh_grid`, all declared in
// `forward.wgsl`. WGSL resolves module-scope names regardless of textual order,
// so appending after is safe. `sh_sample.wgsl` owns the SH reconstruction +
// 8-corner blend symbols (`sample_sh_indirect_corners_pair`,
// `sample_sh_indirect_direct_corners`, `sample_sh_direct_corners_depth_aware`,
// `sample_sh_indirect_corners_depth_aware`, `sample_sh_indirect_corners_without_depth`,
// `sample_sh_indirect_corners_two_without_depth`) — forward must not redeclare them.
//
// `sdf_light_select.wgsl` is the LOAD-BEARING K-selection parity seam: the same
// source string is concatenated into the half-res SDF visibility pass
// (`sdf_shadow.rs`) so both pick the same `sdf`-tagged lights in the same order.
// It reads `spec_lights` / `chunk_grid` / `chunk_offsets` / `chunk_indices` by
// name — all already declared in `forward.wgsl` for the static-light loop — and
// declares no buffers of its own. Never reimplement the selection here.
//
// `light_eval.wgsl` owns the dynamic-tier per-light evaluation helpers
// (`light_eval_falloff`, `light_eval_cone_attenuation`,
// `light_eval_animated_direction`, `light_eval_scripted_intensity_scalar`) the
// runtime light loop calls — extracted so the skinned-mesh pass can mirror the
// same loop against its own group-2 bindings. It declares no buffers. Append
// ORDER dependency: `light_eval_animated_direction` calls
// `sample_color_catmull_rom` from `curve_eval.wgsl`, so the consumer must also
// append curve_eval (forward does, above). WGSL resolves module-scope names
// regardless of textual order, so the relative order of these two is free.
//
// `shadow_sample.wgsl` owns the runtime shadow-map samplers (`sample_spot_shadow`
// spot 2D-array PCF, `sample_point_shadow` point cube-array PCF) plus their
// bias/resolution constants and `cube_face_ndc_depth` — extracted so the
// skinned-mesh pass can mirror the same calls against its own group-2 b5–b8
// shadow bindings. It declares no bindings: it reads the group-5
// `spot_shadow_depth`, `spot_shadow_compare`, `light_space_matrices`, and
// `point_shadow_cube` declared in `forward.wgsl` by lexical name. The
// `// CUBE_SHADOW_BODY_BEGIN` / `// CUBE_SHADOW_BODY_END` markers around
// `sample_point_shadow`'s body travel WITH the body into this snippet, so
// `strip_point_shadow_cube` still neutralizes the cube path in the composed
// no-`CUBE_ARRAY_TEXTURES` source; the `// CUBE_SHADOW_BINDING` binding
// declaration stays with the consumer in `forward.wgsl`.
pub(crate) const SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/forward.wgsl"),
    "\n",
    include_str!("../shaders/curve_eval.wgsl"),
    "\n",
    include_str!("../shaders/sh_sample.wgsl"),
    "\n",
    include_str!("../shaders/sdf_light_select.wgsl"),
    "\n",
    include_str!("../shaders/light_eval.wgsl"),
    "\n",
    include_str!("../shaders/shadow_sample.wgsl"),
);

/// Derive the no-`CUBE_ARRAY_TEXTURES` variant of a group-5 shader (forward or
/// fog) from its single canonical source, so there is no second hand-maintained
/// copy to drift. Two localized edits, both keyed off marker comments embedded in
/// the WGSL:
///
/// 1. Strip the `point_shadow_cube` binding-5 declaration (the line tagged
///    `// CUBE_SHADOW_BINDING`), so the shader matches a group-5 BGL that omits
///    binding 5 — required, since a `CubeArray` BGL entry needs the feature.
/// 2. Neutralize `sample_point_shadow`: replace its body (delimited by
///    `// CUBE_SHADOW_BODY_BEGIN` / `// CUBE_SHADOW_BODY_END`) with `return 1.0;`
///    so the function references no stripped binding and every point light reads
///    as unshadowed. The fog shader has no such body (it never samples the cube),
///    so the body transform is a no-op there.
///
/// Panics (init-time, acceptable per the panic policy) if the binding marker is
/// absent — that means the shader and this transform have drifted, which must
/// fail loudly rather than ship a mis-bound pipeline. The body markers are
/// optional (fog omits them). `pub(super)` so the fog pass (`fog_pass.rs`) derives
/// its own no-cube variant from the SAME transform.
pub(crate) fn strip_point_shadow_cube(source: &str) -> String {
    // 1. Drop the marked binding-5 declaration line.
    let without_binding: String = {
        let kept: Vec<&str> = source
            .lines()
            .filter(|line| !line.contains("// CUBE_SHADOW_BINDING"))
            .collect();
        assert!(
            kept.len() < source.lines().count(),
            "strip_point_shadow_cube: no `// CUBE_SHADOW_BINDING` line found — \
             shader and transform have drifted"
        );
        kept.join("\n")
    };

    // 2. Replace the cube-sampling function body with a no-shadow constant.
    const BEGIN: &str = "// CUBE_SHADOW_BODY_BEGIN";
    const END: &str = "// CUBE_SHADOW_BODY_END";
    match (without_binding.find(BEGIN), without_binding.find(END)) {
        (Some(begin), Some(end)) => {
            // `end` indexes the start of the END marker; include the marker line
            // itself in the replaced span so it does not linger.
            let end_line_end = without_binding[end..]
                .find('\n')
                .map(|n| end + n)
                .unwrap_or(without_binding.len());
            let mut out = String::with_capacity(without_binding.len());
            out.push_str(&without_binding[..begin]);
            out.push_str("return 1.0;");
            out.push_str(&without_binding[end_line_end..]);
            out
        }
        // Fog: no body markers, declaration strip alone suffices.
        (None, None) => without_binding,
        _ => panic!(
            "strip_point_shadow_cube: exactly one of the CUBE_SHADOW_BODY markers \
             is present — shader and transform have drifted"
        ),
    }
}

pub(crate) const WIREFRAME_SHADER_SOURCE: &str = include_str!("../shaders/wireframe.wgsl");

// Depth pre-pass: writes depth only (enables Equal depth compare → zero shading
// overdraw). The full-res lightmap-UV gbuffer MRT it once wrote was freed with
// the animated dominant-direction trace; the per-light SDF visibility pass keys
// on light position, not lightmap UV, so it has no color attachment now.
pub(crate) const DEPTH_PREPASS_SHADER_SOURCE: &str = include_str!("../shaders/depth_prepass.wgsl");

// Spot shadow: vertex-only; per-slot matrix selected via dynamic-offset uniform.
pub(crate) const SPOT_SHADOW_SHADER_SOURCE: &str = include_str!("../shaders/spot_shadow.wgsl");

// Pair index i → query slots [2i, 2i+1]. Labels vec keeps ordering and callsite indices in sync.
pub(crate) const TIMING_PAIR_CULL: usize = 0;
pub(crate) const TIMING_PAIR_ANIMATED_LM_COMPOSE: usize = 1;
pub(crate) const TIMING_PAIR_DEPTH_PREPASS: usize = 2;
pub(crate) const TIMING_PAIR_SDF_SHADOW: usize = 3;
pub(crate) const TIMING_PAIR_FORWARD: usize = 4;
pub(crate) const TIMING_PAIR_SH_COMPOSE: usize = 5;
pub(crate) const TIMING_PAIR_SMOKE: usize = 6;
pub(crate) const TIMING_PAIR_COUNT: usize = 7;

// Must match `Uniforms` in forward.wgsl and wireframe.wgsl (both bind the same buffer).
// std140: vec3<f32> aligns to 16 bytes; camera_position and ambient_floor share a slot.
//   0..64    view_proj  64..76   camera_position  76..80   ambient_floor
//   80..84   light_count  84..88  time  88..92   lighting_isolation  92..96  indirect_scale
//   96..100  sdf_shadow_flags  100..104 sdf_shadow_mode
//   104..108 sdf_force_visibility_one  108..112 dynamic_direct_scale
//   112..116 dynamic_direct_isolation  116..120 has_direct  120..128 _pad
// `sdf_shadow_flags` gates whether the forward samples the half-res SDF
// visibility target at all:
//   bit 0 = a baked SDF atlas is loaded, so the four RGBA channels hold valid
//           per-light visibility slices (K = 4). Set whenever the atlas loads.
// The per-light sdf-tag diffuse/specular terms read their visibility slices
// directly (no per-slice flag) — gated instead by `select_sdf_lights` returning
// lights for the fragment.
// `sdf_shadow_mode` overlays the debug selector; `sdf_force_visibility_one`
// is the dev "force visibility to 1.0" toggle for the no-double-count A/B.
// The dynamic-direct tail (Task 6 of baked-static-direct-sh): repurposes the
// old `_sdf_pad1` slot (108..112) for `dynamic_direct_scale`, then a fresh
// 16-byte row carries `dynamic_direct_isolation` + `has_direct` + padding.
// Only billboard.wgsl reads these (the mesh path uses its own group-4
// `DynamicDirectParams`); forward/wireframe declare them as inert tail so the
// shared 3-way byte contract (Rust writer + forward.wgsl + billboard.wgsl)

pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Extent for the full-res depth pre-pass attachment. Recreated at the surface
/// size on resize. `0` is clamped to `1` to keep texture creation valid during
/// transient zero-size resize events.
pub(crate) fn prepass_attachment_extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: width.max(1),
        height: height.max(1),
        depth_or_array_layers: 1,
    }
}

pub(crate) fn create_depth_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let size = prepass_attachment_extent(width, height);

    let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Depth Texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });

    let view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
    (depth_texture, view)
}

// Group 0: per-frame uniforms (view/proj/time). One buffer entry, no textures.
// COMPUTE required: animated-lightmap compose reuses this BGL (same buffer;
// `uniforms.time` drives curve sampling). Dropping COMPUTE fails wgpu validation
// at compute pipeline creation.
pub(crate) fn uniform_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 1] {
    [wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::VERTEX
            | wgpu::ShaderStages::FRAGMENT
            | wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }]
}

// Group 1: 0=diffuse(sRGB), 2=specular(R8), 3=shininess, 4=normal(Rgba8Unorm,
// NOT sRGB; n = sample.rgb*2-1), 5=aniso_sampler (linear+anisotropic).
// Binding 1 is intentionally vacated (former nearest sampler); the aniso sampler
// stays at 5 — non-contiguous bindings are valid.
pub(crate) fn material_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 5] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 5,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
    ]
}

// Group 2: 0=dynamic lights, 1=influence volumes, 2=spec-only statics,
//          3=ChunkGridInfo, 4=chunk offsets, 5=chunk indices. All buffers, no
// textures.
pub(crate) fn lighting_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 6] {
    // Billboard hoists its static-specular and dynamic-light loops into the
    // vertex stage, so group 2 must be VERTEX-visible too. This is additive —
    // the forward (FRAGMENT) and fog (COMPUTE) pipelines still bind the same
    // group; wgpu validates the widened visibility at pipeline creation. The
    // mesh pipeline reuses only groups 0 and 1, so it is unaffected.
    let storage_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    [
        storage_entry(0),
        storage_entry(1),
        storage_entry(2),
        wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        storage_entry(4),
        storage_entry(5),
    ]
}

/// Count BGL entries that consume a `max_sampled_textures_per_shader_stage` slot
/// for the FRAGMENT stage: `BindingType::Texture` entries whose visibility
/// includes FRAGMENT. wgpu charges the limit against the BGL *entry* set of a
/// pipeline layout per stage, not against how many textures a shader actually
/// samples — so a fragment-visible texture entry counts even if no fragment
/// shader reads it. Example: billboard samples the SH direct atlas in the
/// VERTEX stage, but its BGL entry carries `VERTEX | FRAGMENT` visibility, so
/// it still counts against the fragment texture budget here.
#[cfg(debug_assertions)]
pub(crate) fn fragment_sampled_textures(entries: &[wgpu::BindGroupLayoutEntry]) -> u32 {
    entries
        .iter()
        .filter(|e| {
            e.visibility.contains(wgpu::ShaderStages::FRAGMENT)
                && matches!(e.ty, wgpu::BindingType::Texture { .. })
        })
        .count() as u32
}

/// Count BGL entries that consume a `max_storage_buffers_per_shader_stage` slot
/// for the VERTEX stage: `BindingType::Buffer { ty: Storage, .. }` entries whose
/// visibility includes VERTEX. wgpu charges this limit against the BGL *entry* set
/// of a pipeline layout per stage — a vertex-visible storage entry counts even if
/// no vertex shader actually reads it (exactly the over-broad-visibility trap that
/// hoisting billboard lighting into `vs_main` fell into). The downlevel/WebGPU
/// default ceiling is 8.
#[cfg(debug_assertions)]
pub(crate) fn vertex_storage_buffers(entries: &[wgpu::BindGroupLayoutEntry]) -> u32 {
    entries
        .iter()
        .filter(|e| {
            e.visibility.contains(wgpu::ShaderStages::VERTEX)
                && matches!(
                    e.ty,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { .. },
                        ..
                    }
                )
        })
        .count() as u32
}

/// Single source of truth for the billboard pipeline's VERTEX-stage storage-buffer
/// budget. Sums the vertex-visible storage entries across the exact BGLs that
/// compose the Billboard Pipeline Layout (see `SmokePass::new` for the matching
/// group order: 0 camera, 1 sheet, 2 lighting, 3 SH volume, 6 instance). GPU-free,
/// so it runs in unit tests and `Renderer::new` without a device.
///
/// Billboard lighting runs in `vs_main` (per-vertex SH indirect+direct,
/// static-specular, dynamic-diffuse); the group-6 instance storage buffer is
/// VERTEX-read. The genuinely vertex-read storage buffers are: group 2's five
/// (`lights`, `light_influence`, `spec_lights`, `chunk_offsets`, `chunk_indices`)
/// and group 6's one (`sprites`) — six total. The three group-3 anim/scripted-light
/// storage buffers are read only in the fragment/compute stages, so they must NOT
/// carry VERTEX visibility (see `sh_bind_group_layout_entries`); if they did, this
/// would report 9 and pipeline creation would fail on real GPUs with the
/// downlevel-default limit of 8.
#[cfg(debug_assertions)]
pub(crate) fn billboard_pipeline_vertex_storage_buffer_count() -> u32 {
    vertex_storage_buffers(&uniform_bind_group_layout_entries())
        + vertex_storage_buffers(&smoke::sprite_sheet_bind_group_layout_entries())
        + vertex_storage_buffers(&lighting_bind_group_layout_entries())
        + vertex_storage_buffers(&sh_volume::sh_bind_group_layout_entries())
        + vertex_storage_buffers(&smoke::sprite_instance_bind_group_layout_entries())
}

/// Single source of truth for the forward ("Textured") pipeline's sampled-texture
/// budget. Sums the fragment-visible texture entries across the exact BGLs that
/// compose the forward pipeline layout (see `create_pipeline_layout` for the
/// matching group order). GPU-free: every builder returns plain CPU structs, so
/// this runs in unit tests and at init without a device. Keeping the layout
/// creation and this count reading from the same builders prevents the two
/// sources of truth from drifting (the bug this guards against). Asserted in
/// `Renderer::new` and the
/// `forward_pipeline_sampled_texture_request_matches_bgl_definitions` test.
#[cfg(debug_assertions)]
pub(crate) fn forward_pipeline_sampled_texture_count(cube_array_supported: bool) -> u32 {
    // Groups 0 (uniform) and 2 (lighting) carry no textures, but include them so
    // adding a texture entry to either BGL is caught here automatically. Group 5's
    // count is feature-conditional: the cube-array point-shadow texture (binding 5)
    // is present only when `cube_array_supported` (14 total with it, 13 without).
    fragment_sampled_textures(&uniform_bind_group_layout_entries())
        + fragment_sampled_textures(&material_bind_group_layout_entries())
        + fragment_sampled_textures(&lighting_bind_group_layout_entries())
        + fragment_sampled_textures(&sh_volume::sh_bind_group_layout_entries())
        + fragment_sampled_textures(&crate::lighting::lightmap::bind_group_layout_entries())
        + fragment_sampled_textures(&SpotShadowPool::bind_group_layout_entries(
            cube_array_supported,
        ))
}
