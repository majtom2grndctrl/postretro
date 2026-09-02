// Renderer unit tests (split from the original `mod tests`).
// See: context/lib/testing_guide.md

use super::super::*;

#[test]
fn world_material_uniform_mirrors_shininess_and_emissive_strength_packing() {
    let forward = include_str!("../../shaders/forward.wgsl");
    let kinematic = include_str!("../../shaders/kinematic_brush.wgsl");
    for shader in [forward, kinematic] {
        assert!(shader.contains("shininess: f32,"));
        assert!(shader.contains("emissive_strength: f32,"));
        assert!(shader.contains("_pad: vec2<f32>,"));
    }
    assert!(forward.contains("@group(1) @binding(1) var emissive_texture"));
    assert!(kinematic.contains("@group(1) @binding(1) var emissive_texture"));
}

#[test]
fn zero_strength_materials_skip_emissive_texture_sampling() {
    let forward = include_str!("../../shaders/forward.wgsl");
    let kinematic = include_str!("../../shaders/kinematic_brush.wgsl");
    for (label, shader, sample) in [
        (
            "forward",
            forward,
            "emissive = sample_color(emissive_texture",
        ),
        (
            "kinematic",
            kinematic,
            "emissive = sample_post_retro(emissive_texture",
        ),
    ] {
        let derivatives = shader
            .find("let ddx = dpdx(in.uv);")
            .unwrap_or_else(|| panic!("{label} shader must derive texture gradients"));
        let black = shader
            .find("var emissive = vec3<f32>(0.0);")
            .unwrap_or_else(|| panic!("{label} shader must initialize emissive to black"));
        let guard = shader
            .find("if material.emissive_strength > 0.0")
            .unwrap_or_else(|| panic!("{label} shader must guard the emissive texture fetch"));
        let sample = shader
            .find(sample)
            .unwrap_or_else(|| panic!("{label} shader must retain positive emissive sampling"));
        let additive = shader
            .find("+ emissive * material.emissive_strength")
            .unwrap_or_else(|| panic!("{label} shader must retain additive HDR emissive"));

        assert!(
            derivatives < black && black < guard && guard < sample && sample < additive,
            "{label} shader must derive gradients, initialize black, guard sampling, then add emissive",
        );
    }
}

/// Regression: every storage/uniform buffer binding in `forward.wgsl` must
/// receive a payload large enough to satisfy wgpu's minimum-binding-size
/// validation. The original bug was `anim_descriptors` bound with 16 B while
/// `array<AnimationDescriptor>` requires ≥ 48 B (one full element stride).
///
/// Strategy: parse the live shader with naga, derive the minimum required
/// size for every buffer binding from the WGSL type information, then check
/// that the Rust-side dummy payloads (empty-map / no-SH-section case) are
/// at least that large. Catches mismatches at `cargo test` time, not at
/// draw time on real hardware.
#[test]
fn forward_wgsl_dummy_buffers_meet_shader_min_binding_size() {
    use std::collections::HashMap;

    let module =
        naga::front::wgsl::parse_str(SHADER_SOURCE).expect("forward shader should parse as WGSL");

    // Build (group, binding) → minimum byte count required by the shader.
    // Only storage and uniform address spaces produce buffer bindings.
    let mut min_sizes: HashMap<(u32, u32), u64> = HashMap::new();
    for (_handle, var) in module.global_variables.iter() {
        let is_buffer = matches!(
            var.space,
            naga::AddressSpace::Storage { .. } | naga::AddressSpace::Uniform
        );
        if !is_buffer {
            continue;
        }
        let Some(rb) = &var.binding else { continue };
        let ty = &module.types[var.ty];
        let min: u64 = match &ty.inner {
            // Unbounded array<T> — shader needs at least one element.
            naga::TypeInner::Array {
                stride,
                size: naga::ArraySize::Dynamic,
                ..
            } => *stride as u64,
            // Bounded array<T, N> — shader needs all N elements.
            naga::TypeInner::Array {
                stride,
                size: naga::ArraySize::Constant(n),
                ..
            } => n.get() as u64 * *stride as u64,
            // Struct — shader needs the full declared span.
            naga::TypeInner::Struct { span, .. } => *span as u64,
            // Scalars / vectors / matrices: trivially satisfied; skip.
            _ => continue,
        };
        min_sizes.insert((rb.group, rb.binding), min);
    }

    // Verify that the empty-map dummy animation buffers (no SH section)
    // satisfy the shader's per-binding size requirements.
    //
    // binding 11: array<AnimationDescriptor> — stride = ANIMATION_DESCRIPTOR_SIZE
    // binding 12: array<f32>                 — stride = 4
    let (anim_desc, anim_samples, _count) = sh_volume::build_animation_buffers(None);

    for (label, binding, buf) in [
        (
            "anim_descriptors",
            sh_volume::BIND_ANIM_DESCRIPTORS,
            anim_desc.as_slice(),
        ),
        (
            "anim_samples",
            sh_volume::BIND_ANIM_SAMPLES,
            anim_samples.as_slice(),
        ),
    ] {
        if let Some(&min) = min_sizes.get(&(3, binding)) {
            assert!(
                buf.len() as u64 >= min,
                "dummy {label} buffer (group=3, binding={binding}): Rust side \
                     produces {} B but forward.wgsl min binding size is {min} B \
                     (array element stride — at least one element required)",
                buf.len(),
            );
        } else {
            panic!(
                "forward.wgsl has no buffer at group=3 binding={binding}; \
                        check BIND_* constants match shader @binding decorators"
            );
        }
    }

    // Verify the ShGridInfo uniform payload size.
    let sh_grid_binding = sh_volume::BIND_SH_GRID_INFO;
    let grid_info = sh_volume::build_grid_info_bytes(sh_volume::ShGridInfoParams {
        grid_origin: [0.0; 3],
        cell_size: [1.0; 3],
        grid_dimensions: [1, 1, 1],
        atlas_dimensions: [1, 1],
        tile_dimension: 1,
        tile_border: 0,
        atlas_tiles_per_row: 1,
        tiles_per_layer: 1,
        atlas_layer_count: 1,
        present: false,
        probe_occlusion_enabled: true,
    });
    if let Some(&min) = min_sizes.get(&(3, sh_grid_binding)) {
        assert!(
            grid_info.len() as u64 >= min,
            "sh_grid uniform (group=3, binding={sh_grid_binding}): Rust side \
                 produces {} B but forward.wgsl struct span is {min} B",
            grid_info.len(),
        );
    } else {
        panic!(
            "forward.wgsl has no uniform at group=3 binding={sh_grid_binding}; \
                    check BIND_SH_GRID_INFO matches shader @binding decorators"
        );
    }
}

/// Validates that `forward.wgsl` passes naga's full uniformity analysis.
/// Implicit derivatives (`dpdx`/`dpdy`) and `textureSample` must stay in
/// uniform control flow; the anisotropic filtering branches must use only
/// `textureSampleGrad` (which is safe under non-uniform flow).  Naga's
/// `Validator` enforces this property — `parse_str` alone does not.
/// A future edit that moves a derivative call under a non-uniform branch
/// would silently pass `parse_str` but will be caught here at `cargo test`
/// time, before reaching GPU pipeline creation.
#[test]
fn forward_wgsl_passes_naga_validation() {
    let module = naga::front::wgsl::parse_str(SHADER_SOURCE).expect("forward.wgsl must parse");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("forward.wgsl must pass naga validation (control-flow uniformity)");
}

/// The no-`CUBE_ARRAY_TEXTURES` variant of the forward shader, derived from the
/// single source via `strip_point_shadow_cube`, must (1) drop the
/// `point_shadow_cube` binding entirely so it matches a group-5 BGL that omits
/// binding 5, and (2) still parse + pass naga validation. This is what ships on
/// an adapter without the feature — point shadows cleanly off, no panic.
#[test]
fn forward_wgsl_no_cube_variant_strips_binding_and_validates() {
    let stripped = strip_point_shadow_cube(SHADER_SOURCE);
    // The `point_shadow_cube` binding DECLARATION is gone (comments mentioning
    // the name in prose are harmless; naga validation below proves there is no
    // dangling code reference).
    assert!(
        !stripped.contains("var point_shadow_cube:"),
        "no-cube forward variant must not declare the point_shadow_cube binding"
    );
    // The body markers (and everything between them, including the cube
    // sample) are gone, replaced by the no-shadow constant. naga validation
    // below is the real guarantee that no code references the absent binding.
    assert!(
        !stripped.contains("CUBE_SHADOW_BODY_BEGIN") && !stripped.contains("CUBE_SHADOW_BODY_END"),
        "no-cube forward variant must consume the sample_point_shadow body markers"
    );
    // The supported variant keeps the declaration (sanity that the transform
    // actually removed something).
    assert!(SHADER_SOURCE.contains("var point_shadow_cube:"));

    let module =
        naga::front::wgsl::parse_str(&stripped).expect("no-cube forward variant must parse");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("no-cube forward variant must pass naga validation");

    let two_pair_source = "@group(0) @binding(0) var cube: texture_depth_cube_array; // CUBE_SHADOW_BINDING\n\
fn first() -> f32 {\n\
// CUBE_SHADOW_BODY_BEGIN\n\
return textureDimensions(cube).x;\n\
// CUBE_SHADOW_BODY_END\n\
}\n\
fn second() -> f32 {\n\
// CUBE_SHADOW_BODY_BEGIN\n\
return textureDimensions(cube).y;\n\
// CUBE_SHADOW_BODY_END\n\
}";
    let two_pair_stripped = strip_point_shadow_cube(two_pair_source);
    assert_eq!(
        two_pair_stripped.matches("return 1.0;").count(),
        2,
        "every cube-only helper body must be neutralized, not just the first",
    );
    assert!(
        !two_pair_stripped.contains("CUBE_SHADOW_BODY") && !two_pair_stripped.contains("cube"),
        "all cube markers and the tagged binding must be removed from every pair",
    );
}

/// The depth pre-pass shader must parse as valid WGSL and declare
/// the same `Uniforms` struct binding as `forward.wgsl` (only the
/// leading `view_proj` field is referenced, but the shader still
/// needs to compile cleanly).
#[test]
fn depth_prepass_wgsl_parses() {
    let module = naga::front::wgsl::parse_str(DEPTH_PREPASS_SHADER_SOURCE)
        .expect("depth_prepass.wgsl should parse as WGSL");
    // Sanity: the vertex entry point must be named `vs_main` so the
    // pipeline's `entry_point: Some("vs_main")` resolves.
    let has_vs_main = module
        .entry_points
        .iter()
        .any(|ep| ep.name == "vs_main" && ep.stage == naga::ShaderStage::Vertex);
    assert!(
        has_vs_main,
        "depth_prepass.wgsl must export @vertex vs_main"
    );
    // Vertex-only: the lightmap-UV gbuffer MRT was removed with the animated
    // dominant-direction trace, so there must be NO fragment stage.
    let has_fs = module
        .entry_points
        .iter()
        .any(|ep| ep.stage == naga::ShaderStage::Fragment);
    assert!(
        !has_fs,
        "depth_prepass.wgsl must be vertex-only — the gbuffer MRT was removed"
    );
}

/// The depth pre-pass attachment is recreated at the surface size on resize.
/// Actual texture creation needs a GPU device (unavailable in `cargo test`);
/// the size decision is factored into `prepass_attachment_extent`, asserted
/// here. Zero-size transients clamp to 1 so texture creation stays valid.
#[test]
fn prepass_attachment_extent_matches_surface_size() {
    let e = prepass_attachment_extent(1920, 1080);
    assert_eq!(
        (e.width, e.height, e.depth_or_array_layers),
        (1920, 1080, 1)
    );
    // Zero-size transients clamp to 1 so texture creation stays valid.
    assert_eq!(
        prepass_attachment_extent(0, 0),
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
}

/// Ensure the wireframe shader's `Uniforms` struct stays in sync with
/// the forward shader's — they share a single uniform buffer binding.
#[test]
fn wireframe_wgsl_uniforms_match_forward_layout() {
    let module = naga::front::wgsl::parse_str(WIREFRAME_SHADER_SOURCE)
        .expect("wireframe shader should parse as WGSL");

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
        .expect("wireframe shader should declare struct Uniforms");
    assert_eq!(
        uniforms_span as usize, UNIFORM_SIZE,
        "wireframe.wgsl Uniforms stride ({uniforms_span}) must match UNIFORM_SIZE ({UNIFORM_SIZE})",
    );
}

/// Billboard shares the camera uniform buffer with the forward pass, so its
/// WGSL mirror must retain the 128-byte `Uniforms` span.
#[test]
fn billboard_wgsl_uniforms_match_forward_layout() {
    const BILLBOARD_SHADER_SOURCE: &str = concat!(
        include_str!("../../shaders/billboard.wgsl"),
        "\n",
        include_str!("../../shaders/sh_sample.wgsl"),
    );

    let module = naga::front::wgsl::parse_str(BILLBOARD_SHADER_SOURCE)
        .expect("billboard shader should parse as WGSL");
    let uniforms_span = module
        .types
        .iter()
        .find_map(|(_, ty)| match (&ty.name, &ty.inner) {
            (Some(name), naga::TypeInner::Struct { span, .. }) if name == "Uniforms" => Some(*span),
            _ => None,
        })
        .expect("billboard shader should declare struct Uniforms");

    assert_eq!(
        uniforms_span, 128,
        "billboard.wgsl Uniforms span ({uniforms_span}) must remain 128 bytes",
    );
}
