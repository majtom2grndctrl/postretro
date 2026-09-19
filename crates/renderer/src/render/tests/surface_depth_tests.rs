// Surface Depth (texel-space parallax) shader-contract tests.
// See: context/lib/testing_guide.md, context/lib/rendering_pipeline.md §7.3

use super::super::*;
use postretro_render_cpu::material_plan::MATERIAL_UNIFORM_SIZE;
use postretro_render_cpu::surface_depth as sd;

const SNIPPET: &str = include_str!("../../shaders/surface_depth.wgsl");
const FORWARD: &str = include_str!("../../shaders/forward.wgsl");
const KINEMATIC: &str = include_str!("../../shaders/kinematic_brush.wgsl");

/// The composed source of every pipeline that shades a world material bundle
/// through the group-1 layout.
fn world_pipeline_sources() -> [(&'static str, &'static str); 2] {
    [
        ("forward", SHADER_SOURCE),
        ("kinematic brush", kinematic_brush::composed_shader_source()),
    ]
}

/// Strip `//` line comments so a contract assertion cannot be satisfied by
/// prose that merely mentions the thing it is checking for.
fn strip_line_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn wgsl_struct_span(source: &str, name: &str) -> u32 {
    let module = naga::front::wgsl::parse_str(source).expect("composed shader must parse");
    module
        .types
        .iter()
        .find_map(|(_handle, ty)| match (&ty.name, &ty.inner) {
            (Some(ty_name), naga::TypeInner::Struct { span, .. }) if ty_name == name => Some(*span),
            _ => None,
        })
        .unwrap_or_else(|| panic!("shader must declare struct {name}"))
}

/// The DDA lives in ONE place. `kinematic_brush.wgsl` already keeps a duplicate
/// `sample_post_retro` body that can drift from `forward.wgsl`'s; the parallax
/// march must never become a second such copy. Both pipelines concatenate the
/// same snippet string, and neither declares a march function of its own.
#[test]
fn both_world_pipelines_share_one_surface_depth_march() {
    for (label, composed) in world_pipeline_sources() {
        assert!(
            composed.contains(SNIPPET),
            "{label} must concatenate shaders/surface_depth.wgsl verbatim",
        );
        // Exactly once: a double append would duplicate every symbol.
        assert_eq!(
            composed.matches("fn surface_depth_resolve(").count(),
            1,
            "{label} must append the surface-depth snippet exactly once",
        );
    }

    for (label, consumer) in [("forward", FORWARD), ("kinematic brush", KINEMATIC)] {
        assert!(
            !consumer.contains("fn surface_depth_"),
            "{label} must call the shared march, never redeclare it",
        );
        assert!(
            consumer.contains("surface_depth_resolve("),
            "{label} must run the shared march",
        );
        assert!(
            consumer.contains("surface_depth_indirect_ao("),
            "{label} must apply depth AO through the shared helper",
        );
        assert!(
            consumer.contains("surface_depth_light_visibility("),
            "{label} must self-shadow dynamic lights through the shared helper",
        );
    }
}

/// The per-material uniform is a 3-way contract: the Rust packer plus the two
/// world shaders. Both WGSL copies must be textually identical, not merely
/// compatible, and must span the full 32 bytes the CPU has always uploaded.
#[test]
fn material_uniform_layout_is_mirrored_by_both_world_shaders() {
    fn material_uniform_block(source: &str) -> &str {
        let start = source
            .find("struct MaterialUniform {")
            .expect("shader must declare struct MaterialUniform");
        let end = source[start..]
            .find("\n};")
            .map(|offset| start + offset + 3)
            .expect("MaterialUniform must close");
        &source[start..end]
    }

    assert_eq!(
        material_uniform_block(FORWARD),
        material_uniform_block(KINEMATIC),
        "the two world shaders' MaterialUniform declarations have drifted",
    );

    for (label, composed) in world_pipeline_sources() {
        assert_eq!(
            wgsl_struct_span(composed, "MaterialUniform") as usize,
            MATERIAL_UNIFORM_SIZE,
            "{label} MaterialUniform stride must match MATERIAL_UNIFORM_SIZE",
        );
    }
}

/// Every tuning constant exists twice — once as the GPU-free authority in
/// `postretro_render_cpu::surface_depth`, once in WGSL. Pin them together by
/// VALUE (parsing the declared literal, so a reformat is not a failure);
/// a silent divergence would make the CPU reference tests prove nothing.
#[test]
fn shader_constants_match_the_cpu_reference() {
    fn declared(name: &str, ty: &str) -> String {
        let needle = format!("const {name}: {ty} = ");
        let at = SNIPPET
            .find(&needle)
            .unwrap_or_else(|| panic!("surface_depth.wgsl must declare {name}: {ty}"));
        let rest = &SNIPPET[at + needle.len()..];
        let end = rest
            .find(';')
            .unwrap_or_else(|| panic!("{name} declaration must terminate"));
        rest[..end].trim().to_owned()
    }

    fn declared_u32(name: &str) -> u32 {
        let literal = declared(name, "u32");
        let digits = literal
            .strip_suffix('u')
            .unwrap_or_else(|| panic!("{name} must be a u32 literal, found `{literal}`"));
        match digits.strip_prefix("0x") {
            Some(hex) => u32::from_str_radix(hex, 16),
            None => digits.parse(),
        }
        .unwrap_or_else(|_| panic!("{name} literal `{literal}` must parse"))
    }

    fn declared_f32(name: &str) -> f32 {
        let literal = declared(name, "f32");
        literal
            .parse()
            .unwrap_or_else(|_| panic!("{name} literal `{literal}` must parse"))
    }

    assert_eq!(
        declared_u32("SURFACE_DEPTH_MAX_STEPS_MASK"),
        sd::SURFACE_DEPTH_MAX_STEPS
    );
    assert_eq!(
        declared_u32("SURFACE_DEPTH_BASE_MIP_SHIFT"),
        sd::SURFACE_DEPTH_BASE_MIP_SHIFT
    );
    assert_eq!(
        declared_u32("SURFACE_DEPTH_BASE_MIP_MASK"),
        sd::SURFACE_DEPTH_BASE_MIP_MASK
    );
    assert_eq!(
        declared_u32("SURFACE_DEPTH_HAS_DEPTH_BIT"),
        sd::SURFACE_DEPTH_HAS_DEPTH_BIT
    );
    // The self-shadow budget is NOT a shader constant: it rides the packed
    // march word so the player's quality tier can zero it by rewriting the
    // material uniform buffer. Pin the field's position instead.
    assert_eq!(
        declared_u32("SURFACE_DEPTH_SHADOW_BUDGET_SHIFT"),
        sd::SURFACE_DEPTH_SHADOW_BUDGET_SHIFT
    );
    assert_eq!(
        declared_u32("SURFACE_DEPTH_SHADOW_BUDGET_MASK"),
        sd::SURFACE_DEPTH_SHADOW_BUDGET_MASK
    );
    assert!(
        !SNIPPET.contains("const SURFACE_DEPTH_SHADOW_LIGHT_BUDGET"),
        "the budget must reach the shader as data, not as a constant the \
         player's quality tier cannot change",
    );
    // The AO gate is the `LightTermMask` bit, which skips the reserved
    // emissive bit 7.
    assert_eq!(
        declared_u32("LIGHT_TERM_DEPTH_AO"),
        postretro_render_cpu::frame_uniforms::LightTermMask::DEPTH_AMBIENT_OCCLUSION.bits()
    );

    for (name, expected) in [
        (
            "SURFACE_DEPTH_FADE_LOD_START",
            sd::SURFACE_DEPTH_FADE_LOD_START,
        ),
        (
            "SURFACE_DEPTH_FADE_LOD_RANGE",
            sd::SURFACE_DEPTH_FADE_LOD_RANGE,
        ),
        (
            "SURFACE_DEPTH_FADE_DISTANCE_FRACTION",
            sd::SURFACE_DEPTH_FADE_DISTANCE_FRACTION,
        ),
        ("SURFACE_DEPTH_AO_STRENGTH", sd::SURFACE_DEPTH_AO_STRENGTH),
        (
            "SURFACE_DEPTH_SHADOW_BIAS_M",
            sd::SURFACE_DEPTH_SHADOW_BIAS_M,
        ),
        (
            "SURFACE_DEPTH_SIDE_UV_BIAS_TEXELS",
            sd::SURFACE_DEPTH_SIDE_UV_BIAS_TEXELS,
        ),
        ("SURFACE_DEPTH_DET_EPS", sd::SURFACE_DEPTH_DET_EPS),
        (
            "SURFACE_DEPTH_MAX_UV_SCALE_M",
            sd::SURFACE_DEPTH_MAX_UV_SCALE_M,
        ),
        ("SURFACE_DEPTH_MIN_DESCENT", sd::SURFACE_DEPTH_MIN_DESCENT),
    ] {
        assert_eq!(
            declared_f32(name),
            expected,
            "{name} has drifted from the CPU reference"
        );
    }
}

/// D2's hard renderer constraints, restated as assertions. Each of these
/// breaks the build or the engine if violated, and none of them fails loudly
/// on its own — a depth write silently kills the fragment under
/// `depth_compare: Equal`, a lightmap offset silently samples a neighbouring
/// chart.
#[test]
fn surface_depth_honors_the_hard_renderer_constraints() {
    for (label, composed) in world_pipeline_sources() {
        let code = strip_line_comments(composed);
        assert!(
            !code.contains("builtin(frag_depth)"),
            "{label}: the depth pre-pass is vertex-only and the forward pass runs \
             depth_compare: Equal with writes disabled — a fragment that writes depth \
             fails its own equality test",
        );
    }

    let snippet_code = strip_line_comments(SNIPPET);
    for derivative in ["dpdx", "dpdy", "fwidth", "dpdxFine", "dpdyFine"] {
        assert!(
            !snippet_code.contains(&format!("{derivative}(")),
            "the march must consume pre-computed gradients: a {derivative} call inside it \
             would put a derivative in non-uniform control flow",
        );
    }
    assert!(
        snippet_code.contains("textureLoad(spec_texture,"),
        "height must be read with textureLoad at an explicit mip — no sampler, and the \
         exact quantized values survive",
    );
    assert!(
        !snippet_code.contains("textureSample"),
        "the march must not filter the depth field; filtering would destroy the \
         piecewise-constant property the DDA's exactness rests on",
    );

    // `lightmap_uv` is offset nowhere. Charts carry only CHART_PADDING_TEXELS = 2
    // of gutter, so a parallax offset would pull a neighbouring chart.
    for (label, consumer) in [("forward", FORWARD), ("kinematic brush", KINEMATIC)] {
        for line in strip_line_comments(consumer).lines() {
            assert!(
                !(line.contains("lightmap_uv") && line.contains("depth.")),
                "{label}: parallax must shift base_uv only — `{}`",
                line.trim(),
            );
        }
    }

    // Shadow-map receiver bias keeps the GEOMETRIC normal. The DDA face normal
    // is far bumpier than a normal map and would wobble shadow boundaries.
    for (label, consumer, receiver_calls) in [
        (
            "forward",
            FORWARD,
            ["sample_point_shadow(", "sample_spot_shadow("].as_slice(),
        ),
        (
            "kinematic brush",
            KINEMATIC,
            [
                "sample_point_shadow(",
                "sample_spot_shadow(",
                "sample_point_shadow_with_static(",
                "sample_spot_shadow_with_static(",
            ]
            .as_slice(),
        ),
    ] {
        let code = strip_line_comments(consumer);
        for call in receiver_calls {
            for (index, _) in code.match_indices(call) {
                let args_end = code[index..]
                    .find(");")
                    .map(|offset| index + offset)
                    .expect("shadow call must terminate");
                let args = &code[index..args_end];
                assert!(
                    args.contains("mesh_n"),
                    "{label}: {call} must receive the geometric normal, not the DDA face normal",
                );
                assert!(
                    !args.contains("N_shade") && !args.contains("depth.normal"),
                    "{label}: {call} must never receive the DDA face normal",
                );
                assert!(
                    !args.contains("depth.world_position"),
                    "{label}: {call} must sample from the TRUE plane — the depth map \
                     holds undisplaced geometry and its bias is tuned against it",
                );
            }
        }
    }
}

/// The SH indirect lookup is biased along the surface normal to reduce bleed
/// through thin walls. Surface Depth's side-wall normal is PERPENDICULAR to the
/// surface, so biasing along it would slide the lookup sideways across the face
/// instead of lifting it off — the opposite of what the bias is for. The two
/// normals must therefore stay separate arguments.
#[test]
fn the_sh_lookup_bias_never_uses_the_dda_face_normal() {
    for (label, consumer, shading, bias) in [
        ("forward", FORWARD, "N_shade", "N_bump"),
        ("kinematic brush", KINEMATIC, "n", "n_bump"),
    ] {
        let code = strip_line_comments(consumer);
        assert!(
            code.contains("let offset_world = world_pos + offset_normal * SH_NORMAL_OFFSET_M"),
            "{label}: the SH lookup must bias along its own offset normal",
        );
        assert!(
            code.contains(&format!(
                "sample_sh_indirect(in.world_position, {shading}, {bias}, mesh_n)"
            )),
            "{label}: SH indirect must evaluate for the DDA face normal but bias along              the bumped surface normal",
        );
    }
}

/// D6.2: the base mip must reach the shader as a parameter. A hardcoded 0
/// would silently read non-resident data once streaming drops top mips, and
/// the fade must key off the resident level's dimensions so a streamed-out
/// surface map flattens instead of popping.
#[test]
fn the_dda_base_mip_is_a_parameter_not_a_hardcoded_zero() {
    let code = strip_line_comments(SNIPPET);
    assert!(
        code.contains(
            "let base_mip = (packed >> SURFACE_DEPTH_BASE_MIP_SHIFT) & SURFACE_DEPTH_BASE_MIP_MASK;"
        ),
        "the base mip must be decoded from the material uniform",
    );
    assert!(
        code.contains("textureDimensions(spec_texture, base_mip)")
            && code.contains("textureDimensions(spec_texture, depth.base_mip)"),
        "both marches must measure the grid at the RESIDENT base level",
    );
    assert!(
        !code.contains("textureLoad(spec_texture, folded, 0)")
            && !code.contains("textureDimensions(spec_texture, 0)"),
        "no level-0 literal may reach the surface map",
    );
    // The LOD that drives the fade is measured against those same dimensions.
    assert!(
        code.contains("let footprint = max(length(ddx_uv * dims), length(ddy_uv * dims));"),
        "the fade LOD must be measured in resident base-mip texels",
    );
}

/// Depth AO applies to the SH indirect term and to nothing else. Its own mask
/// bit exists so it can be ruled out in isolation when a lighting bug is being
/// bisected; if it leaked onto a direct term that would be double-counting.
#[test]
fn depth_ambient_occlusion_touches_only_the_indirect_term() {
    for (label, consumer) in [("forward", FORWARD), ("kinematic brush", KINEMATIC)] {
        let code = strip_line_comments(consumer);
        let applications: Vec<&str> = code
            .lines()
            .filter(|line| line.contains("surface_depth_indirect_ao("))
            .collect();
        assert_eq!(
            applications.len(),
            1,
            "{label}: depth AO must be applied exactly once",
        );
        assert!(
            applications[0].contains("indirect = indirect *"),
            "{label}: depth AO must scale the indirect term and nothing else — `{}`",
            applications[0].trim(),
        );
    }
    assert!(
        strip_line_comments(SNIPPET).contains("(light_terms & LIGHT_TERM_DEPTH_AO) == 0u"),
        "depth AO must be gated by its own LightTermMask bit",
    );
}

/// Self-shadowing exists for DYNAMIC lights only: the bake knows nothing about
/// dynamic bodies, but the lightmap DOES own static-onto-static occlusion at
/// 4 cm/texel, and competing with it risks the no-double-counting invariant.
#[test]
fn self_shadowing_is_budgeted_and_dynamic_only() {
    let forward = strip_line_comments(FORWARD);
    assert!(
        forward.contains("depth_shadow_marches < depth.shadow_light_budget"),
        "forward must budget its self-shadow marches from the per-material word",
    );
    assert!(
        forward.contains("NdotL > 0.0 && depth_shadow_marches"),
        "forward must skip the march when the face is not lit",
    );

    let kinematic = strip_line_comments(KINEMATIC);
    assert!(
        kinematic.contains("&& i < kinematic_light_params.dynamic_light_count"),
        "the mover must self-shadow the DYNAMIC prefix only — the animated-baked tail \
         and selected-static suffix are baked-tier records",
    );
    assert!(
        kinematic.contains("depth_shadow_marches < depth.shadow_light_budget"),
        "the mover must budget its self-shadow marches from the per-material word",
    );

    // The static-light loops must NOT march. `spec_lights` is the static/baked
    // side; only the runtime `lights` loop may.
    for (label, consumer) in [("forward", FORWARD), ("kinematic brush", KINEMATIC)] {
        for line in strip_line_comments(consumer).lines() {
            assert!(
                !(line.contains("spec_lights") && line.contains("surface_depth_light_visibility")),
                "{label}: baked static light must not be self-shadowed — `{}`",
                line.trim(),
            );
        }
    }
}

/// The flat path must be the pre-Surface-Depth path exactly. Both shaders take
/// the march's outputs through one named local each, so an inactive result
/// feeding through is provably the interpolated value.
#[test]
fn an_inactive_march_restores_the_pre_feature_inputs() {
    let code = strip_line_comments(SNIPPET);
    for restore in [
        "out.uv = uv;",
        "out.march_uv = uv;",
        "out.world_position = world_position;",
        "out.normal = geo_normal;",
        "out.hit_top = true;",
        "out.depth_m = 0.0;",
        "out.carved = false;",
    ] {
        assert!(
            code.contains(restore),
            "the flat result must restore the interpolated inputs: missing `{restore}`",
        );
    }
    // Every early-out in the resolver returns that same flat result.
    assert_eq!(
        code.matches("return flat_result;").count(),
        7,
        "each degenerate case (no map, faded out, zero depth, singular Jacobian, \
         zero UV scale, collapsed tangent plane, edge-on) must return the flat result",
    );

    for (label, consumer) in [("forward", FORWARD), ("kinematic brush", KINEMATIC)] {
        let code = strip_line_comments(consumer);
        assert!(
            code.contains("let shade_uv = depth.uv;"),
            "{label}: material sampling must go through one named marched UV",
        );
        assert!(
            !code.contains("sample_normal(t_normal, in.uv"),
            "{label}: the normal map must be sampled at the marched UV",
        );
    }
}

/// No GPU is available in CI, so a WGSL mistake in the MOVER pipeline would
/// otherwise surface only at pipeline creation on a real adapter. naga's
/// `Validator` (not `parse_str` alone) is the same control-flow-uniformity
/// analysis `forward_wgsl_passes_naga_validation` runs on the forward side;
/// both world pipelines concatenate the same march, so both need it.
#[test]
fn both_world_pipelines_pass_naga_validation() {
    for (label, composed) in world_pipeline_sources() {
        let module = naga::front::wgsl::parse_str(composed)
            .unwrap_or_else(|err| panic!("{label} composed source must parse: {err}"));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|err| panic!("{label} composed source must pass naga validation: {err}"));
    }
}

/// The player's quality tier (design D5) reaches the shader as DATA in the
/// per-material uniform, because the tier is applied by rewriting that buffer
/// — this engine has no shader-variant system, so anything a tier must switch
/// off has to be decodable from the packed march word.
#[test]
fn the_quality_tier_reaches_the_shader_through_the_packed_march_word() {
    let code = strip_line_comments(SNIPPET);
    assert!(
        code.contains(
            "(packed >> SURFACE_DEPTH_SHADOW_BUDGET_SHIFT) & SURFACE_DEPTH_SHADOW_BUDGET_MASK"
        ),
        "the shadow budget must be decoded from the material's packed word",
    );
    assert!(
        code.contains("out.shadow_light_budget = 0u;"),
        "the flat result must zero the budget, so a flat fragment never marches",
    );

    // The packed fields must not overlap: has-depth is one bit, the budget is
    // a nibble above it, and neither may disturb steps or base mip.
    let concrete = Material::Concrete.surface_depth();
    let high = sd::SurfaceDepthUniform::resolve(
        concrete,
        sd::SurfaceDepthQuality::High,
        true,
        11,
        sd::SURFACE_DEPTH_RESIDENT_BASE_MIP,
    );
    let low = sd::SurfaceDepthUniform::resolve(
        concrete,
        sd::SurfaceDepthQuality::Low,
        true,
        11,
        sd::SURFACE_DEPTH_RESIDENT_BASE_MIP,
    );
    let off = sd::SurfaceDepthUniform::resolve(
        concrete,
        sd::SurfaceDepthQuality::Off,
        true,
        11,
        sd::SURFACE_DEPTH_RESIDENT_BASE_MIP,
    );
    let decoded_high = sd::unpack_surface_depth_march(high.march_word());
    let decoded_low = sd::unpack_surface_depth_march(low.march_word());
    assert!(decoded_high.has_depth && decoded_low.has_depth);
    assert!(decoded_high.shadow_light_budget > 0);
    assert_eq!(decoded_low.shadow_light_budget, 0);
    assert!(decoded_low.max_steps < decoded_high.max_steps);
    assert_eq!(off.march_word(), 0, "Off must pack the all-zero march word");
}

/// The prefix-driven parameters reach the shader through the already-zeroed
/// second uniform row, and a material with no height sibling never sets the
/// has-depth flag however deep its prefix asks to carve.
#[test]
fn material_parameters_are_prefix_driven_and_gated_on_the_loaded_slot() {
    let concrete = Material::Concrete.surface_depth();
    assert!(concrete.is_enabled(), "the cobblestone case must carve");

    let with_map = sd::SurfaceDepthUniform::resolve(
        concrete,
        sd::SurfaceDepthQuality::High,
        true,
        11,
        sd::SURFACE_DEPTH_RESIDENT_BASE_MIP,
    );
    assert!(with_map.has_depth);
    let without_map = sd::SurfaceDepthUniform::resolve(
        concrete,
        sd::SurfaceDepthQuality::High,
        false,
        11,
        sd::SURFACE_DEPTH_RESIDENT_BASE_MIP,
    );
    assert!(!without_map.has_depth);
    assert_eq!(without_map.depth.depth_meters, 0.0);

    let carved = postretro_render_cpu::material_plan::build_material_uniform(
        Material::Concrete.shininess(),
        Material::Concrete.emissive_strength(),
        with_map,
    );
    let flat = postretro_render_cpu::material_plan::build_material_uniform(
        Material::Concrete.shininess(),
        Material::Concrete.emissive_strength(),
        without_map,
    );
    // Identical first row: Surface Depth changes nothing a pre-existing
    // material relied on.
    assert_eq!(carved[..16], flat[..16]);
    assert_ne!(carved[16..], flat[16..]);
    assert!(
        flat[16..].iter().all(|&byte| byte == 0),
        "a material without a surface map must upload the historical all-zero row",
    );
}
