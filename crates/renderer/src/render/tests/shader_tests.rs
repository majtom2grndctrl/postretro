// Renderer unit tests (split from the original `mod tests`).
// See: context/lib/testing_guide.md

use super::super::*;

fn scripted_light_intensity_scalar_reference(
    premultiplied_color: [f32; 3],
    base_color: [f32; 3],
) -> f32 {
    let (premultiplied_channel, color_channel) =
        if base_color[0] >= base_color[1] && base_color[0] >= base_color[2] {
            (premultiplied_color[0], base_color[0])
        } else if base_color[1] >= base_color[2] {
            (premultiplied_color[1], base_color[1])
        } else {
            (premultiplied_color[2], base_color[2])
        };
    if color_channel <= 1.0e-6 {
        return 0.0;
    }
    premultiplied_channel / color_channel
}

fn scripted_color_curve_effective_color(
    premultiplied_color: [f32; 3],
    base_color: [f32; 3],
    color_sample: [f32; 3],
    brightness: f32,
) -> [f32; 3] {
    let intensity = scripted_light_intensity_scalar_reference(premultiplied_color, base_color);
    [
        color_sample[0].max(0.0) * intensity * brightness.max(0.0),
        color_sample[1].max(0.0) * intensity * brightness.max(0.0),
        color_sample[2].max(0.0) * intensity * brightness.max(0.0),
    ]
}

fn assert_vec3_near(actual: [f32; 3], expected: [f32; 3]) {
    for i in 0..3 {
        assert!(
            (actual[i] - expected[i]).abs() < 1.0e-6,
            "channel {i}: expected {}, got {}",
            expected[i],
            actual[i],
        );
    }
}

#[test]
fn forward_shader_color_curve_branch_reapplies_static_intensity() {
    let src = include_str!("../../shaders/forward.wgsl");
    let color_branch_start = src
        .find("if scripted_desc.color_count > 0u")
        .expect("forward shader should have a scripted color-curve branch");
    let brightness_branch_start = src[color_branch_start..]
        .find("} else if scripted_desc.brightness_count > 0u")
        .map(|offset| color_branch_start + offset)
        .expect("forward shader should keep a brightness-only branch");
    let color_branch = &src[color_branch_start..brightness_branch_start];

    assert!(
        color_branch.contains("let unit_sample = max("),
        "color branch should bind the clamped unit-RGB sample before applying intensity",
    );
    assert!(
        color_branch.contains("light_eval_scripted_intensity_scalar("),
        "color branch should recover the static intensity scalar",
    );
    assert!(
        color_branch.contains("effective_color = unit_sample * intensity * brightness;"),
        "color branch should apply unit sample, static intensity, and optional brightness multiplicatively",
    );
    assert!(
        !color_branch.contains("effective_color = max("),
        "color branch must not assign the raw clamped unit-RGB sample as final effective_color",
    );
}

#[test]
fn scripted_color_curve_white_sample_keeps_static_intensity() {
    let actual = scripted_color_curve_effective_color(
        [10.0, 10.0, 10.0],
        [1.0, 1.0, 1.0],
        [1.0, 1.0, 1.0],
        1.0,
    );
    assert_vec3_near(actual, [10.0, 10.0, 10.0]);
}

#[test]
fn scripted_color_curve_hue_sample_uses_static_intensity_as_magnitude() {
    let actual = scripted_color_curve_effective_color(
        [10.0, 10.0, 10.0],
        [1.0, 1.0, 1.0],
        [0.5, 0.0, 0.0],
        1.0,
    );
    assert_vec3_near(actual, [5.0, 0.0, 0.0]);
}

#[test]
fn scripted_color_curve_multiplies_optional_brightness_curve() {
    let actual = scripted_color_curve_effective_color(
        [10.0, 10.0, 10.0],
        [1.0, 1.0, 1.0],
        [1.0, 0.0, 0.0],
        0.5,
    );
    assert_vec3_near(actual, [5.0, 0.0, 0.0]);
}

/// Regression: both the CPU-side `build_uniform_data` packer and the
/// CPU-side `pack_light` packer must match the WGSL struct layouts
/// that the fragment shader compiles against. Parsing the live
/// shader source with naga catches drift before it reaches a GPU
/// round-trip (see the similar test in `compute_cull.rs`).
#[test]
fn forward_wgsl_struct_strides_match_cpu_layout() {
    let module =
        naga::front::wgsl::parse_str(SHADER_SOURCE).expect("forward shader should parse as WGSL");

    let mut seen = std::collections::HashMap::new();
    for (_handle, ty) in module.types.iter() {
        if let naga::TypeInner::Struct { span, .. } = &ty.inner
            && let Some(name) = &ty.name
        {
            seen.insert(name.clone(), *span);
        }
    }

    let uniforms_span = seen
        .get("Uniforms")
        .copied()
        .expect("forward shader should declare struct Uniforms");
    assert_eq!(
        uniforms_span as usize, UNIFORM_SIZE,
        "forward.wgsl Uniforms stride ({uniforms_span}) must match UNIFORM_SIZE ({UNIFORM_SIZE})",
    );

    let light_span = seen
        .get("GpuLight")
        .copied()
        .expect("forward shader should declare struct GpuLight");
    assert_eq!(
        light_span as usize,
        postretro_lighting::GPU_LIGHT_SIZE,
        "forward.wgsl GpuLight stride ({light_span}) must match GPU_LIGHT_SIZE ({})",
        postretro_lighting::GPU_LIGHT_SIZE,
    );

    let spec_light_span = seen
        .get("SpecLight")
        .copied()
        .expect("forward shader should declare struct SpecLight");
    assert_eq!(
        spec_light_span as usize, SPEC_LIGHT_SIZE,
        "forward.wgsl SpecLight stride ({spec_light_span}) must match SPEC_LIGHT_SIZE ({SPEC_LIGHT_SIZE})",
    );
}

#[test]
fn forward_light_term_mask_gates_each_world_term_without_overriding_scale() {
    let src = include_str!("../../shaders/forward.wgsl");

    for constant in [
        "const LIGHT_TERM_AMBIENT_FLOOR: u32 = 0x01u;",
        "const LIGHT_TERM_INDIRECT_STATIC: u32 = 0x02u;",
        "const LIGHT_TERM_INDIRECT_ANIMATED: u32 = 0x04u;",
        "const LIGHT_TERM_BAKED_DIRECT_STATIC: u32 = 0x08u;",
        "const LIGHT_TERM_BAKED_DIRECT_ANIMATED: u32 = 0x10u;",
        "const LIGHT_TERM_DYNAMIC_DIRECT: u32 = 0x20u;",
        "const LIGHT_TERM_SPECULAR: u32 = 0x40u;",
    ] {
        assert!(src.contains(constant), "missing {constant}");
    }
    assert!(
        src.contains("let use_ambient_floor = (light_terms & LIGHT_TERM_AMBIENT_FLOOR) != 0u;")
            && src.contains("let use_baked_direct_static = (light_terms & LIGHT_TERM_BAKED_DIRECT_STATIC) != 0u;")
            && src.contains("let use_baked_direct_animated = (light_terms & LIGHT_TERM_BAKED_DIRECT_ANIMATED) != 0u;")
            && src.contains("let use_dynamic = (light_terms & LIGHT_TERM_DYNAMIC_DIRECT) != 0u;")
            && src.contains("let use_specular = (light_terms & LIGHT_TERM_SPECULAR) != 0u;"),
        "the world shader must independently derive each direct gate from the mask",
    );
    assert!(
        src.contains(
            "if use_baked_direct_static {\n            lm_irr = sample_lightmap_irradiance"
        ) && src.contains("if use_baked_direct_animated && animated_slot != 0xffffffffu"),
        "static and animated world lightmap contributions must be independently sampled",
    );
    assert!(
        src.contains("* uniforms.indirect_scale;")
            && !src.contains("select(uniforms.indirect_scale, 1.0"),
        "term isolation must not force indirect_scale to 1.0",
    );
}

#[test]
fn count_split_shader_consumers_use_expected_loop_bounds() {
    let forward_src = include_str!("../../shaders/forward.wgsl");
    assert!(
        forward_src.contains("let light_count = select(0u, uniforms.light_count, use_dynamic);"),
        "forward world lighting must stay bounded by dynamic-only uniforms.light_count",
    );
    assert!(
        !forward_src.contains("uniforms.total_light_count, use_dynamic"),
        "forward world lighting must not evaluate promoted static records",
    );

    let billboard_src = include_str!("../../shaders/billboard.wgsl");
    assert!(
        billboard_src.contains(
            "select(uniforms.total_light_count, uniforms.light_count, uniforms.has_scatter != 0u)"
        ),
        "scatter billboards must stop their runtime loop at the dynamic prefix while legacy direct-SH still consumes the promoted tail",
    );

    let mesh_src = include_str!("../../shaders/skinned_mesh.wgsl");
    assert!(
        mesh_src.contains("select(0u, mesh_light_params.light_count, use_dynamic)"),
        "mesh lighting must use the renderer-provided total light count",
    );
}

#[test]
fn billboard_light_term_mask_gates_per_vertex_terms() {
    let src = include_str!("../../shaders/billboard.wgsl");

    for constant in [
        "const LIGHT_TERM_AMBIENT_FLOOR: u32 = 0x01u;",
        "const LIGHT_TERM_BAKED_DIRECT_STATIC: u32 = 0x08u;",
        "const LIGHT_TERM_BAKED_DIRECT_ANIMATED: u32 = 0x10u;",
        "const LIGHT_TERM_DYNAMIC_DIRECT: u32 = 0x20u;",
        "const LIGHT_TERM_SPECULAR: u32 = 0x40u;",
    ] {
        assert!(src.contains(constant), "missing {constant}");
    }
    assert!(
        src.contains("let light_terms = uniforms.light_term_mask;")
            && src.contains(
                "let use_ambient_floor = (light_terms & LIGHT_TERM_AMBIENT_FLOOR) != 0u;"
            )
            && src.contains(
                "let use_dynamic_direct = (light_terms & LIGHT_TERM_DYNAMIC_DIRECT) != 0u;"
            )
            && src.contains(
                "let use_baked_direct_static = (light_terms & LIGHT_TERM_BAKED_DIRECT_STATIC) != 0u;"
            )
            && src.contains(
                "let use_baked_direct_animated = (light_terms & LIGHT_TERM_BAKED_DIRECT_ANIMATED) != 0u;"
            )
            && src.contains("let use_specular = (light_terms & LIGHT_TERM_SPECULAR) != 0u;"),
        "billboard vertex lighting must derive every local term gate from group-0's mask",
    );
    assert!(
        src.contains("const SCATTER_MODE_COMPOSED_ANIMATED: u32 = 2u;")
            && src.contains("let use_baked_direct_scatter = use_baked_direct_static")
            && src.contains("|| (uniforms.has_scatter == SCATTER_MODE_COMPOSED_ANIMATED && use_baked_direct_animated);")
            && src.contains("if use_baked_direct_scatter {")
            && src.contains("if use_specular && chunk_grid.has_chunk_grid != 0u && spec_int > 0.0 {")
            && src.contains(
                "select(uniforms.total_light_count, uniforms.light_count, uniforms.has_scatter != 0u)"
            )
            && src.contains(
                "let ambient_floor = select(0.0, uniforms.ambient_floor, use_ambient_floor);"
            ),
        "billboard static specular, dynamic diffuse, and ambient floor must be independently gated in vs_main",
    );
    assert!(
        !src.contains("uniforms.dynamic_direct_isolation"),
        "billboard SH terms must rely only on the compose atlases",
    );
}

#[test]
fn billboard_scatter_shader_is_normal_free_and_keeps_legacy_direct_path() {
    let src = include_str!("../../shaders/billboard.wgsl");
    assert!(
        src.contains("@group(3) @binding(17) var billboard_direct_scatter: texture_3d<f32>;")
            && src
                .contains("fn sample_billboard_direct_scatter(world_pos: vec3<f32>) -> vec3<f32>")
            && src.contains("let is_valid = sample.a >= 0.5;")
            && src.contains("sh_probe_weight(")
            && src.contains("vec3<f32>(0.0),\n            is_valid,\n            false,"),
        "scatter must use the depth-aware SH weighting convention without supplying a sprite normal",
    );
    assert!(
        src.contains(
            "direct_scatter = uniforms.direct_scale * sample_billboard_direct_scatter(sprite_pos);"
        ) && src.contains("} else if uniforms.has_direct != 0u {")
            && src.contains("sh_direct = uniforms.direct_scale * sample_sh_direct(sprite_pos, N);"),
        "scatter must replace only the static direct-SH read; unavailable maps retain legacy direct SH",
    );
    assert!(
        src.contains("if uniforms.has_scatter != 0u && !spec_light_is_sdf(sl)")
            && src.contains("select(uniforms.total_light_count, uniforms.light_count, uniforms.has_scatter != 0u)"),
        "scatter must exclude static-light-map specular while the promoted tail remains available solely on the legacy path",
    );

    // Regression: section 47 bakes only static-light-map transport. Gating the
    // whole spec_lights loop on the legacy branch dropped static SDF lights.
    let static_specular = src
        .split("var static_specular = vec3<f32>(0.0);")
        .nth(1)
        .expect("billboard shader must retain the static specular loop")
        .split("// Dynamic direct")
        .next()
        .expect("static specular loop must precede dynamic direct");
    assert!(
        static_specular.contains("if uniforms.has_scatter != 0u && !spec_light_is_sdf(sl) {")
            && static_specular.contains("continue;")
            && src.contains("fn spec_light_is_sdf(sl: SpecLight) -> bool {")
            && src.contains("return sl.color_and_pad.w > 0.5;"),
        "scatter must retain SDF SpecLight handling and skip only static-light-map records",
    );

    let runtime_direct = src
        .split("for (var i: u32 = 0u; i < dynamic_count; i = i + 1u) {")
        .nth(1)
        .expect("billboard shader must retain the runtime direct loop");
    let scatter_dynamic = runtime_direct
        .split("if uniforms.has_scatter != 0u {")
        .nth(1)
        .expect("dynamic scatter branch must exist")
        .split("} else {")
        .next()
        .expect("dynamic scatter branch must close before legacy branch");
    assert!(
        scatter_dynamic.contains("light.color_and_falloff_model.xyz * attenuation")
            && !scatter_dynamic.contains("NdotL")
            && src.contains("let influence = light_influence[i];")
            && src.contains("if inf_radius <= 1.0e30 {")
            && src.contains("let cone = cone_attenuation("),
        "scatter dynamic lighting must preserve influence/range/cone rejection without a Lambert cosine",
    );
}

#[test]
fn billboard_scatter_sampling_accepts_animated_direct_without_static_direct() {
    // Regression: static-off/animated-on compose produced an animated scatter
    // texture, but the billboard discarded it behind the static-only gate.
    let src = include_str!("../../shaders/billboard.wgsl");
    assert!(
        src.contains("const LIGHT_TERM_BAKED_DIRECT_STATIC: u32 = 0x08u;")
            && src.contains("const LIGHT_TERM_BAKED_DIRECT_ANIMATED: u32 = 0x10u;")
            && src.contains(
                "|| (uniforms.has_scatter == SCATTER_MODE_COMPOSED_ANIMATED && use_baked_direct_animated);"
            )
            && src.contains("if use_baked_direct_scatter {")
            && src.contains(
                "direct_scatter = uniforms.direct_scale * sample_billboard_direct_scatter(sprite_pos);"
            ),
        "composed scatter must remain visible under animated-only direct while static-base mode cannot borrow that bit",
    );
}

#[test]
fn fog_dynamic_scatter_uses_group_zero_snapshot_loop_bounds() {
    // Ordering T10: fog reads the shared group-0 snapshot, never the live UI
    // mask, so its dynamic term cannot lead or lag the world path.
    let src = include_str!("../../shaders/fog_volume.wgsl");

    assert!(
        src.contains("@group(0) @binding(0) var<uniform> uniforms: Uniforms;")
            && src.contains("light_term_mask: u32,"),
        "fog must read the group-0 Uniforms prefix through the mask field",
    );
    assert!(
        src.contains("let spot_count = select(0u, fog.spot_count, use_dynamic_direct);")
            && src.contains("let point_count = select(0u, fog.point_count, use_dynamic_direct);")
            && src.contains("for (var li: u32 = 0u; li < spot_count; li = li + 1u)")
            && src.contains("for (var pi: u32 = 0u; pi < point_count; pi = pi + 1u)"),
        "fog must use the group-0 dynamic bit to bound both dynamic scatter loops",
    );
}

#[test]
fn skinned_shader_projects_and_shades_the_same_world_position() {
    // Regression: the viewmodel used to provide a camera-space model transform
    // to this shared shader while binding projection-only at group 0. Clip
    // placement looked correct, but SH, dynamic lights, and shadow receipt all
    // consumed the camera-space value as `world_position`.
    let mesh_src = include_str!("../../shaders/skinned_mesh.wgsl");
    assert!(mesh_src.contains("let world_pos = instance.model * skinned_pos;"));
    assert!(mesh_src.contains("out.clip_position = camera.view_proj * world_pos;"));
    assert!(mesh_src.contains("out.world_position = world_pos.xyz;"));
    assert!(mesh_src.contains("sample_sh_indirect(in.world_position"));
    assert!(mesh_src.contains("accumulate_dynamic_direct(\n        in.world_position,"));
}

#[test]
fn forward_shader_shadowmask_union_uses_promoted_count_and_safe_metadata_tail() {
    let src = include_str!("../../shaders/forward.wgsl");
    let start = src
        .find("fn shadowmask_union_subtraction(")
        .expect("forward shader must declare the shadowmask union helper");
    let helper = &src[start
        ..src
            .find("@fragment")
            .expect("fragment entry follows helpers")];

    assert!(
        helper.contains("if uniforms.total_light_count <= uniforms.light_count"),
        "no promoted lights must return before reading promoted metadata"
    );
    assert!(
        helper.contains("let promoted_count = uniforms.total_light_count - uniforms.light_count;"),
        "shadowmask loop must be bounded by promoted count"
    );
    assert!(
        helper.contains("let influence_index = uniforms.light_count + p;"),
        "influence-volume early-out must read the promoted influence before metadata"
    );
    assert!(
        helper.contains(
            "let meta_index = uniforms.total_light_count + p * SHADOWMASK_META_VEC4S_PER_RECORD;"
        ),
        "metadata must live after the dynamic+promoted influence prefix"
    );
    assert!(
        helper.contains("if meta_index + 1u >= influence_len"),
        "metadata reads must be bounds-guarded so stale tails are not read"
    );
    assert!(
        !helper.contains("bitcast<vec4<u32>>(meta"),
        "promoted metadata lives in a float storage tail and must be read as numeric f32 values, not raw u32 bit patterns"
    );
    assert!(
        src.contains("const SHADOWMASK_INVALID_INDEX_VALUE: f32 = -1.0;")
            && src.contains("const SHADOWMASK_CHANNEL_DROPPED: f32 = 4.0;"),
        "shadowmask metadata sentinels must be normal numeric floats"
    );
    assert!(
        helper.contains("channel_value >= SHADOWMASK_CHANNEL_DROPPED"),
        "dropped channels must use the float-safe 4.0 sentinel and skip the union term before u32 casts"
    );
    let spec_guard = helper
        .find("spec_idx_value <= SHADOWMASK_INVALID_INDEX_VALUE")
        .expect("invalid spec indices must be rejected as float metadata");
    let spec_cast = helper
        .find("let spec_idx = u32(spec_idx_value);")
        .expect("shader must consume the CPU-uploaded compact spec_lights index");
    assert!(
        spec_guard < spec_cast,
        "spec index metadata must be bounds-guarded before casting to u32"
    );
    let channel_guard = helper
        .find("channel_value >= SHADOWMASK_CHANNEL_DROPPED")
        .expect("dropped channel sentinel must be checked");
    let channel_cast = helper
        .find("let channel = u32(channel_value);")
        .expect("shader must cast the checked numeric channel");
    assert!(
        channel_guard < channel_cast,
        "channel metadata must be sentinel/range-guarded before casting to u32"
    );
    assert!(
        helper.contains("floor(spec_idx_value) != spec_idx_value")
            && helper.contains("floor(slot_value) != slot_value")
            && helper.contains("floor(channel_value) != channel_value"),
        "metadata values must be integer-valued floats before u32 casts"
    );
    assert!(
        helper.contains("slot_value >= f32(SHADOWMASK_SPOT_SLOT_COUNT)")
            && helper.contains("slot_value >= f32(SHADOWMASK_CUBE_SLOT_COUNT)"),
        "shadow pool slots must be range-guarded before indexing or sampling"
    );
    assert!(
        helper.contains("let spec_idx = u32(spec_idx_value);"),
        "shader must consume the CPU-uploaded compact spec_lights index"
    );
    assert!(
        helper.contains("let weight = clamp(meta0.w, 0.0, 1.0);"),
        "shader must use raw promoted-set w from metadata, not GpuLight color"
    );
    assert!(
        helper.contains("out.subtraction = vec3<f32>(0.0);")
            && helper.contains("if uniforms.total_light_count <= uniforms.light_count")
            && helper.contains("return out;"),
        "zero promoted lights must retain a zero union subtraction"
    );
}

// Regression: one bad point-shadow tap survived the spot-calibrated dead zone.
#[test]
fn forward_shader_shadowmask_dead_zone_matches_each_pool_kernel() {
    let src = include_str!("../../shaders/forward.wgsl");
    let shadow_src = include_str!("../../shaders/shadow_sample.wgsl");
    let point_shadow = &shadow_src[shadow_src
        .find("fn sample_point_shadow(")
        .expect("shared shadow sampler must declare sample_point_shadow")..];

    assert!(
        src.contains("const SHADOWMASK_SPOT_KERNEL_RADIUS: i32 = 2;")
            && src.contains(
                "for (var dy: i32 = -SHADOWMASK_SPOT_KERNEL_RADIUS; dy <= SHADOWMASK_SPOT_KERNEL_RADIUS;"
            )
            && src.contains(
                "for (var dx: i32 = -SHADOWMASK_SPOT_KERNEL_RADIUS; dx <= SHADOWMASK_SPOT_KERNEL_RADIUS;"
            )
            && src.contains(
                "const SHADOWMASK_SPOT_VISIBILITY_DEAD_ZONE: f32 = 1.0 / 25.0;"
            ),
        "the promoted spot union must ignore one tap of its 5x5 comparison kernel"
    );
    assert!(
        point_shadow.contains("for (var dy = -1; dy <= 1;")
            && point_shadow.contains("for (var dx = -1; dx <= 1;")
            && point_shadow.contains("return lit / 9.0;")
            && src.contains("const SHADOWMASK_POINT_VISIBILITY_DEAD_ZONE: f32 = 1.0 / 9.0;"),
        "the promoted point union must ignore one tap of its 3x3 comparison kernel"
    );
    assert!(
        src.contains("fn shadowmask_visibility_difference(\n    pool_kind: u32,")
            && src.contains("pool_kind == SHADOWMASK_POOL_CUBE,")
            && src.contains("max(difference - dead_zone, 0.0) / (1.0 - dead_zone)")
            && src
                .contains("shadowmask_visibility_difference(pool_kind, baked_vis, shadow_map_vis)"),
        "the continuous union difference must select and apply the validated pool-kind calibration"
    );

    let renormalized_difference =
        |difference: f32, dead_zone: f32| ((difference - dead_zone).max(0.0)) / (1.0 - dead_zone);
    for dead_zone in [1.0 / 25.0, 1.0 / 9.0] {
        assert_eq!(renormalized_difference(dead_zone, dead_zone), 0.0);
        assert!(renormalized_difference(dead_zone + 1.0e-3, dead_zone) > 0.0);
        assert!((renormalized_difference(1.0, dead_zone) - 1.0).abs() < f32::EPSILON);
    }
}

#[test]
fn forward_shader_shadowmask_visualization_mode_is_wired() {
    let src = include_str!("../../shaders/forward.wgsl");
    assert!(
        src.contains("@group(4) @binding(6) var shadowmask_atlas: texture_2d_array<f32>;"),
        "shadowmask atlas must be one sampled texture in the lightmap group"
    );
    assert!(
        src.contains("uniforms.sdf_shadow_mode == SHADOWMASK_VISUALIZE_MODE"),
        "mode 5 must visualize the union subtraction magnitude"
    );
    assert!(
        src.contains("return vec4<f32>(shadowmask_union, base_color.a);"),
        "visualization mode should show the union term directly"
    );
    assert!(
        src.contains("const SHADOWMASK_RAW_POOL_VISIBILITY_MODE: u32 = 6u;")
            && src.contains(
                "out.raw_pool_visibility = min(out.raw_pool_visibility, shadow_map_vis);"
            ),
        "mode 6 must show the darkest raw promoted-light pool visibility"
    );
    assert!(
        src.contains("uniforms.sdf_shadow_mode == SHADOWMASK_RAW_POOL_VISIBILITY_MODE")
            && src.contains("return vec4<f32>(g, g, g, base_color.a);")
            && src.contains(
                "if use_baked_direct_static || use_specular || uniforms.sdf_shadow_mode == SHADOWMASK_RAW_POOL_VISIBILITY_MODE"
            ),
        "raw pool visibility must be a grayscale early-return diagnostic independent of term isolation"
    );
}

#[test]
fn forward_shader_shadowmask_fallback_clamps_multilayer_indices() {
    let src = include_str!("../../shaders/forward.wgsl");
    let helper_start = src
        .find("fn sample_shadowmask_atlas(")
        .expect("forward shader must centralize shadowmask atlas sampling");
    let helper_end = src[helper_start..]
        .find("fn shadowmask_visibility_for_spec_light(")
        .map(|offset| helper_start + offset)
        .expect("shadowmask sampling helper must precede spec-light visibility");
    let helper = &src[helper_start..helper_end];

    assert!(
        helper.contains("textureNumLayers(shadowmask_atlas) - 1u")
            && helper.contains("min(lightmap_layer, last_layer)")
            && helper.contains("i32(safe_layer)"),
        "shadowmask sampling must clamp baked layer indices to the bound texture's last layer",
    );
    assert_eq!(
        src.matches("sample_shadowmask_atlas(").count(),
        3,
        "the helper definition plus union and specular call sites must be the only shadowmask samples",
    );
    assert_eq!(
        src.matches("textureSample(\n        shadowmask_atlas,")
            .count(),
        1,
        "all shadowmask reads must route through the layer-safe helper",
    );

    let fs = &src[src
        .find("fn fs_main(")
        .expect("forward shader must declare fs_main")..];
    assert!(
        fs.contains("sample_shadowmask_atlas(in.lightmap_uv, in.lightmap_layer)"),
        "world specular must use the layer-safe all-visible fallback sample",
    );
    assert!(
        src.contains("round(sl.cone_cos.z) >= SHADOWMASK_CHANNEL_DROPPED")
            && src.contains("return 1.0;"),
        "absent/dropped atlas channels must retain the fully-lit sentinel path",
    );
}

#[test]
fn direct_sh_compose_debug_override_isolates_single_selection() {
    let src = include_str!("../../shaders/direct_sh_compose.wgsl");
    let selection_weight_start = src
        .find("fn selection_weight(")
        .expect("direct_sh_compose.wgsl should declare selection_weight");
    let selection_weight = &src[selection_weight_start..];
    let live_weights_start = selection_weight
        .find("if (selection_index >= arrayLength(&selection_weights))")
        .expect("selection_weight should keep the live weights bounds check");
    let debug_branch = &selection_weight[..live_weights_start];

    assert!(
        debug_branch.contains("if (debug_override.enabled != 0u)"),
        "debug override should take over selection weighting when enabled",
    );
    assert!(
        debug_branch.contains("selection_index == debug_override.selection_index"),
        "debug override should apply the slider only to the selected light",
    );
    assert!(
        debug_branch.contains("return 0.0;"),
        "debug override must suppress all other selected lights",
    );
}

#[test]
fn animated_direct_sh_compose_debug_override_isolates_one_animated_baked_light() {
    let src = include_str!("../../shaders/animated_direct_sh_compose.wgsl");
    let scale_start = src
        .find("fn animated_light_scale(")
        .expect("animated compose shader should declare animated_light_scale");
    let scale = &src[scale_start..];

    assert!(
        src.contains("@group(1) @binding(26) var<uniform> debug_override: DebugOverride;"),
        "Pass B must use its own uniform override binding",
    );
    assert!(
        scale.contains("light_index != debug_override.light_index"),
        "Pass B override must suppress every non-selected AnimatedBakedLights entry",
    );
    assert!(
        scale.contains("clamp(debug_override.weight, 0.0, 1.0)"),
        "Pass B override must retain the selected light's inspectable weight",
    );
}

/// Task 5 (sdf-static-occluder-shadows): the forward shader must parse
/// cleanly with the new SDF shadow-factor bindings (`sdf_shadow_factor` and
/// `sdf_shadow_depth` on group 5 bindings 3 and 4) and must declare the
/// inline bilateral upsample helper. Mirrors the parse-and-binding shape of
/// Task 2b's `compose_shader_parses_and_declares_debug_binding`.
#[test]
fn forward_shader_parses_and_declares_sdf_shadow_upsample() {
    let src = SHADER_SOURCE;
    let module = naga::front::wgsl::parse_str(src)
        .expect("forward.wgsl should parse as WGSL after Task 5 plumbing");

    // The upsample function is the public surface of the bilateral filter.
    let has_upsample = module
        .functions
        .iter()
        .any(|(_h, f)| f.name.as_deref() == Some("upsample_shadow_factor"));
    assert!(
        has_upsample,
        "forward.wgsl must declare `upsample_shadow_factor` (Task 5 bilateral upsample)",
    );

    // The bilateral filter is depth-aware — both the factor target and
    // the scene depth texture must be declared.
    assert!(
        src.contains("sdf_shadow_factor"),
        "forward.wgsl must bind the half-res SDF shadow factor target",
    );
    assert!(
        src.contains("sdf_shadow_depth"),
        "forward.wgsl must bind the scene depth texture for the depth-aware bilateral",
    );

    // The fragment entry point must reference the upsample helper — else
    // the wiring is dead and the multiply never lands.
    let fs = src
        .find("fn fs_main(")
        .expect("forward.wgsl must declare fs_main");
    let fs_tail = &src[fs..];
    assert!(
        fs_tail.contains("upsample_shadow_factor("),
        "fs_main must call upsample_shadow_factor (otherwise the multiply is dead)",
    );

    // The gating bitset must be wired into the Uniforms struct.
    assert!(
        src.contains("sdf_shadow_flags"),
        "forward.wgsl Uniforms must include the `sdf_shadow_flags` gate field",
    );
}

/// Guards that the forward shader composes `sdf_light_select.wgsl` and
/// validates end-to-end: `select_sdf_lights` (K-selection parity seam with
/// the visibility pass) and `slice_for_visibility` (per-light diffuse
/// multiply via R/B/A slices) must be declared and called from `fs_main`.
/// Also confirms the bilateral upsample wiring is intact. Full naga
/// validation — not just parse — catches type/binding errors.
#[test]
fn forward_shader_composes_sdf_light_selection_and_reads_slices() {
    let src = SHADER_SOURCE;
    let module = naga::front::wgsl::parse_str(src)
        .expect("forward + sdf_light_select must parse as one composed WGSL module");
    // Full validation catches type/binding errors a bare parse misses.
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("forward + sdf_light_select composed source should validate");

    // The shared selection helper must be present as a function — proving
    // the helper source was concatenated, not reimplemented inline.
    let has_select = module
        .functions
        .iter()
        .any(|(_h, f)| f.name.as_deref() == Some("select_sdf_lights"));
    assert!(
        has_select,
        "forward must compose the shared `select_sdf_lights` helper (K-selection parity seam)",
    );

    // The slice→channel mapper must exist — it is how the forward reads a
    // selection slot's visibility (slot 0→R, 1→B, 2→A).
    let has_slice_map = module
        .functions
        .iter()
        .any(|(_h, f)| f.name.as_deref() == Some("slice_for_visibility"));
    assert!(
        has_slice_map,
        "forward must declare `slice_for_visibility` to read per-light slices from R/B/A",
    );

    // fs_main must actually drive the per-light path: select the lights and
    // read each one's slice — else the diffuse term attaches to nothing.
    let fs = src
        .find("fn fs_main(")
        .expect("forward.wgsl must declare fs_main");
    let fs_tail = &src[fs..];
    assert!(
        fs_tail.contains("select_sdf_lights("),
        "fs_main must call select_sdf_lights (parity with the visibility pass)",
    );
    assert!(
        fs_tail.contains("slice_for_visibility("),
        "fs_main must read per-light visibility via slice_for_visibility (else slices are dead)",
    );

    // The dev force-visibility-1.0 toggle must be wired into the Uniforms
    // struct (drives the no-double-count A/B).
    assert!(
        src.contains("sdf_force_visibility_one"),
        "forward.wgsl Uniforms must include the `sdf_force_visibility_one` dev toggle",
    );
}

/// Pins Task 5's headline contract (invariant 9): an `sdf`-typed light's
/// SPECULAR term reads the SAME per-light visibility slice as its diffuse.
/// The specular loop walks the chunk list in chunk order, so it resolves the
/// slice through `sdf_visibility_for_light`, which finds the light's slot in
/// the shared `sdf_sel` selection and maps it via `slice_for_visibility` —
/// the same selection and slot→channel mapping the diffuse loop uses, so the
/// two terms read the same slice by construction. Full naga validation plus
/// structural assertions that the resolver exists, is composed, and is
/// actually applied to the specular contribution in `fs_main`.
#[test]
fn forward_shader_specular_reads_sdf_visibility_slice() {
    let src = SHADER_SOURCE;
    let module = naga::front::wgsl::parse_str(src)
        .expect("forward + sdf_light_select must parse as one composed WGSL module");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("forward + sdf_light_select composed source should validate");

    // The specular slice resolver must exist as a function.
    let has_resolver = module
        .functions
        .iter()
        .any(|(_h, f)| f.name.as_deref() == Some("sdf_visibility_for_light"));
    assert!(
        has_resolver,
        "forward must declare `sdf_visibility_for_light` (specular reads the per-light slice)",
    );

    let fs = src
        .find("fn fs_main(")
        .expect("forward.wgsl must declare fs_main");
    let fs_tail = &src[fs..];

    // The specular loop must drive the resolver — else specular is unshadowed
    // for sdf lights and Task 5's headline contract is unmet.
    assert!(
        fs_tail.contains("sdf_visibility_for_light("),
        "fs_main must call sdf_visibility_for_light so sdf specular reads its visibility slice",
    );

    // Diffuse and specular must read off the SAME selection: one shared
    // `sdf_sel` (single `select_sdf_lights` call), not two. A second call
    // could drift the slot ordering and break diffuse/specular parity.
    // Count against forward.wgsl ALONE — `SHADER_SOURCE` appends the helper
    // file, whose `fn select_sdf_lights(` definition would otherwise count.
    let forward_only = include_str!("../../shaders/forward.wgsl");
    assert_eq!(
        forward_only.matches("select_sdf_lights(").count(),
        1,
        "forward.wgsl must call select_sdf_lights exactly once (diffuse + specular share one selection)",
    );
    assert!(
        fs_tail.contains("sdf_visibility_for_light(sdf_sel,"),
        "specular must resolve visibility through the shared `sdf_sel` selection",
    );

    // The specular contribution must actually be multiplied by the resolved
    // visibility (gated through the sdf tag), proving the slice reaches the
    // blinn-phong term and is not dead.
    assert!(
        fs_tail.contains("sdf_select_is_sdf("),
        "specular must gate visibility on the sdf tag via sdf_select_is_sdf",
    );
}

/// Regression: the SH volume's `ShGridInfo` uniform struct must have
/// matching byte stride on both sides of the bind group — CPU packer
/// (`sh_volume::build_grid_info_bytes`) and the fragment shader's
/// declaration in `forward.wgsl`.
#[test]
fn forward_wgsl_sh_grid_info_matches_cpu_layout() {
    let span = wgsl_struct_span(SHADER_SOURCE, "ShGridInfo", "forward shader");
    assert_eq!(
        span as usize,
        sh_volume::SH_GRID_INFO_SIZE,
        "forward.wgsl ShGridInfo stride ({span}) must match SH_GRID_INFO_SIZE ({})",
        sh_volume::SH_GRID_INFO_SIZE,
    );

    let desc_span = wgsl_struct_span(SHADER_SOURCE, "AnimationDescriptor", "forward shader");
    assert_eq!(
        desc_span as usize,
        sh_volume::ANIMATION_DESCRIPTOR_SIZE,
        "forward.wgsl AnimationDescriptor stride ({desc_span}) must match ANIMATION_DESCRIPTOR_SIZE ({})",
        sh_volume::ANIMATION_DESCRIPTOR_SIZE,
    );
}

#[test]
fn sh_grid_info_consumer_shaders_match_cpu_layout() {
    const BILLBOARD_SHADER_SOURCE: &str = concat!(
        include_str!("../../shaders/billboard.wgsl"),
        "\n",
        include_str!("../../shaders/sh_sample.wgsl"),
    );
    const FOG_SHADER_SOURCE: &str = concat!(
        include_str!("../../shaders/fog_volume.wgsl"),
        "\n",
        include_str!("../../shaders/sh_sample.wgsl"),
    );
    const SKINNED_MESH_SHADER_SOURCE: &str = concat!(
        include_str!("../../shaders/skinned_mesh.wgsl"),
        "\n",
        include_str!("../../shaders/sh_sample.wgsl"),
        "\n",
        include_str!("../../shaders/curve_eval.wgsl"),
        "\n",
        include_str!("../../shaders/light_eval.wgsl"),
        "\n",
        include_str!("../../shaders/shadow_sample.wgsl"),
    );

    for (label, source) in [
        ("forward", SHADER_SOURCE),
        ("billboard", BILLBOARD_SHADER_SOURCE),
        ("fog_volume", FOG_SHADER_SOURCE),
        ("skinned_mesh", SKINNED_MESH_SHADER_SOURCE),
    ] {
        let span = wgsl_struct_span(source, "ShGridInfo", label);
        assert_eq!(
            span as usize,
            sh_volume::SH_GRID_INFO_SIZE,
            "{label}.wgsl ShGridInfo stride ({span}) must match SH_GRID_INFO_SIZE ({})",
            sh_volume::SH_GRID_INFO_SIZE,
        );
    }
}

#[test]
fn sh_compose_grid_dims_shader_layouts_match_cpu_packer() {
    const SH_COMPOSE_SHADER_SOURCE: &str = concat!(
        include_str!("../../shaders/sh_compose.wgsl"),
        "\n",
        include_str!("../../shaders/curve_eval.wgsl"),
    );
    const DIRECT_SH_COMPOSE_SHADER_SOURCE: &str =
        include_str!("../../shaders/direct_sh_compose.wgsl");

    for (label, source) in [
        ("sh_compose", SH_COMPOSE_SHADER_SOURCE),
        ("direct_sh_compose", DIRECT_SH_COMPOSE_SHADER_SOURCE),
    ] {
        let span = wgsl_struct_span(source, "GridDims", label);
        assert_eq!(
            span, 64,
            "{label}.wgsl GridDims stride ({span}) must match build_compose_grid_bytes",
        );
    }
}

fn wgsl_struct_span(source: &str, name: &str, label: &str) -> u32 {
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|err| panic!("{label} should parse as WGSL: {err}"));
    for (_handle, ty) in module.types.iter() {
        if let naga::TypeInner::Struct { span, .. } = &ty.inner
            && ty.name.as_deref() == Some(name)
        {
            return *span;
        }
    }
    panic!("{label} should declare struct {name}");
}
