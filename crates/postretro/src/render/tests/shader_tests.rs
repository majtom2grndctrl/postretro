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
        crate::lighting::GPU_LIGHT_SIZE,
        "forward.wgsl GpuLight stride ({light_span}) must match GPU_LIGHT_SIZE ({})",
        crate::lighting::GPU_LIGHT_SIZE,
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

    let span = seen
        .get("ShGridInfo")
        .copied()
        .expect("forward shader should declare struct ShGridInfo");
    assert_eq!(
        span as usize,
        sh_volume::SH_GRID_INFO_SIZE,
        "forward.wgsl ShGridInfo stride ({span}) must match SH_GRID_INFO_SIZE ({})",
        sh_volume::SH_GRID_INFO_SIZE,
    );

    let desc_span = seen
        .get("AnimationDescriptor")
        .copied()
        .expect("forward shader should declare struct AnimationDescriptor");
    assert_eq!(
        desc_span as usize,
        sh_volume::ANIMATION_DESCRIPTOR_SIZE,
        "forward.wgsl AnimationDescriptor stride ({desc_span}) must match ANIMATION_DESCRIPTOR_SIZE ({})",
        sh_volume::ANIMATION_DESCRIPTOR_SIZE,
    );
}
