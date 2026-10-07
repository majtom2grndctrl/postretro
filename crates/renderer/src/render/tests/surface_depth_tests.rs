// Surface Depth (texel-space parallax) shader-contract tests.
// See: context/lib/testing_guide.md, context/lib/rendering_pipeline.md §7.3

use super::super::*;
use postretro_render_cpu::material_plan::MATERIAL_UNIFORM_SIZE;
use postretro_render_cpu::surface_depth as sd;

const SNIPPET: &str = include_str!("../../shaders/surface_depth.wgsl");
const FORWARD: &str = include_str!("../../shaders/forward.wgsl");
const LIGHTMAP_SAMPLE: &str = include_str!("../../shaders/lightmap_sample.wgsl");
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

/// Same names and the same span do not pin the ORDER: swapping the peak and
/// trough declarations would still pass the test above while the shader read
/// each from the other's bytes. Read every member's offset from naga and the
/// value the CPU packer wrote at that offset.
#[test]
fn material_uniform_member_offsets_match_the_cpu_packer() {
    let uniform = sd::SurfaceDepthUniform {
        depth: postretro_render_data::material::SurfaceDepth {
            depth_meters: 5.0,
            quantize_levels: 3,
            max_steps: 40,
            fade_distance_meters: 9.0,
        },
        has_depth: true,
        base_mip: 2,
        shadow_light_budget: 1,
        relief: sd::SurfaceRelief {
            peak_raise: 0.5,
            trough: -0.25,
        },
    };
    let bytes = postretro_render_cpu::material_plan::build_material_uniform(7.0, 3.0, uniform);
    let word = |offset: u32| {
        let at = offset as usize;
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    };
    let expected_f32 = [
        ("shininess", 7.0f32),
        ("emissive_strength", 3.0),
        ("surface_depth_peak_raise", 0.5),
        ("surface_depth_trough", -0.25),
        ("surface_depth_meters", 5.0),
        ("surface_depth_fade_distance", 9.0),
        ("surface_depth_quantize_levels", 3.0),
    ];

    for (label, composed) in world_pipeline_sources() {
        let module = naga::front::wgsl::parse_str(composed).expect("composed shader must parse");
        let members = module
            .types
            .iter()
            .find_map(|(_handle, ty)| match (&ty.name, &ty.inner) {
                (Some(name), naga::TypeInner::Struct { members, .. })
                    if name == "MaterialUniform" =>
                {
                    Some(members.clone())
                }
                _ => None,
            })
            .expect("shader must declare struct MaterialUniform");
        let offset_of = |name: &str| {
            members
                .iter()
                .find(|member| member.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("{label}: MaterialUniform has no `{name}`"))
                .offset
        };
        for (name, value) in expected_f32 {
            assert_eq!(
                f32::from_bits(word(offset_of(name))),
                value,
                "{label}: `{name}` reads bytes the CPU packer did not write it to",
            );
        }
        assert_eq!(
            word(offset_of("surface_depth_march")),
            uniform.march_word(),
            "{label}: `surface_depth_march` reads bytes the CPU packer did not write it to",
        );
        assert_eq!(
            members.len(),
            expected_f32.len() + 1,
            "{label}: unpinned member"
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
    // march word so the player's on/off switch (D5) can zero it by rewriting the
    // material uniform buffer. Pin the field's position instead.
    assert_eq!(
        declared_u32("SURFACE_DEPTH_SHADOW_BUDGET_SHIFT"),
        sd::SURFACE_DEPTH_SHADOW_BUDGET_SHIFT
    );
    // The unit the authored depth carries. The shader converts a texel count
    // with the fragment's own texel rate; the CPU picks the matching authoring
    // table and the matching cap. A mismatch would make every material carve in
    // one unit and be tuned in the other, so pin it rather than trusting two
    // hand-edits to stay together.
    assert_eq!(
        declared_u32("SURFACE_DEPTH_TEXEL_MODE"),
        sd::SURFACE_DEPTH_TEXEL_MODE
    );
    // The ceiling on the RESOLVED depth. In texel mode the CPU caps a texel
    // count, so this is the only bound on the meters the march actually sees.
    assert_eq!(
        declared_f32("SURFACE_DEPTH_MAX_METERS"),
        sd::SURFACE_DEPTH_MAX_METERS
    );
    assert_eq!(
        declared_u32("SURFACE_DEPTH_SHADOW_BUDGET_MASK"),
        sd::SURFACE_DEPTH_SHADOW_BUDGET_MASK
    );
    assert!(
        !SNIPPET.contains("const SURFACE_DEPTH_SHADOW_LIGHT_BUDGET"),
        "the budget must reach the shader as data, not as a constant the \
         player's on/off switch cannot change",
    );
    // The AO gate is the `LightTermMask` bit, which skips the reserved
    // emissive bit 7.
    assert_eq!(
        declared_u32("LIGHT_TERM_DEPTH_AO"),
        postretro_render_cpu::frame_uniforms::LightTermMask::DEPTH_AMBIENT_OCCLUSION.bits()
    );

    // The signed encoding's two byte constants, and the expressions that use
    // them. The expressions are pinned as TEXT because they are where a
    // backend-dependent rounding or a sign slip would hide: the byte is
    // recovered with `floor(x + 0.5)` first, and quantization is `floor(x +
    // 0.5)` per direction, never the builtin that rounds half to even.
    assert_eq!(
        declared_f32("SURFACE_HEIGHT_BYTE_MAX"),
        sd::SURFACE_HEIGHT_BYTE_MAX
    );
    assert_eq!(
        declared_f32("SURFACE_HEIGHT_PLANE_BYTE"),
        sd::SURFACE_HEIGHT_PLANE_BYTE
    );
    let snippet_code = strip_line_comments(SNIPPET);
    for expression in [
        "let stored = floor(stored_g * SURFACE_HEIGHT_BYTE_MAX + 0.5);",
        "let authored = SURFACE_HEIGHT_BYTE_MAX - stored;",
        "return (authored - SURFACE_HEIGHT_PLANE_BYTE) / SURFACE_HEIGHT_PLANE_BYTE;",
        "if levels >= 1.0 {",
        "return clamp(floor(s * levels + 0.5) / levels, -1.0, 1.0);",
    ] {
        assert!(
            snippet_code.contains(expression),
            "the shader's signed-height encoding has drifted from the CPU reference: \
             missing `{expression}`",
        );
    }
    assert!(
        !snippet_code.contains("round("),
        "WGSL rounds half to even; the CPU authority uses `floor(x + 0.5)`",
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
        ("SURFACE_DEPTH_EPS", sd::SURFACE_DEPTH_EPS),
        ("SURFACE_DEPTH_FAR", sd::SURFACE_DEPTH_FAR),
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
    for derivative in [
        "dpdx",
        "dpdy",
        "fwidth",
        "dpdxFine",
        "dpdyFine",
        "dpdxCoarse",
        "dpdyCoarse",
        "fwidthFine",
        "fwidthCoarse",
    ] {
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

    // The lightmap texel is offset nowhere. Charts carry only
    // CHART_PADDING_TEXELS = 2 of gutter, so a parallax offset would pull a
    // neighbouring chart across it.
    //
    // Asserted POSITIVELY, per call site, because the negative form this
    // replaced — no line contains both `lightmap_uv` and `depth.` — was vacuous
    // against the one refactor that actually breaks the constraint:
    // `sample_lightmap_irradiance(shade_uv, ...)` in place of
    // `in.lightmap_texel` contains neither token, so the test stayed green
    // while the atlas was sampled at the marched UV. The shadowmask path is the worse half of that hole, since
    // it is a layered atlas and `shadowmask_union_subtraction` forwards one
    // UV to every promoted light on the fragment.
    // Forward plus the lightmap sampling helpers it composes.
    let forward_code = strip_line_comments(&format!("{FORWARD}\n{LIGHTMAP_SAMPLE}"));
    // The animated atlas is read at the block-remapped UV, a pure atlas
    // translation of the interpolated block-local lightmap texel:
    // `animated_block_uv` must receive that texel verbatim, and the animated
    // sample must take its result.
    for call in [
        "sample_lightmap_irradiance(",
        "sample_lightmap_direction(",
        "sample_lightmap_animated(",
        "animated_block_uv(",
        "sample_shadowmask_atlas(",
        "shadowmask_union_subtraction(",
    ] {
        let mut cursor = forward_code.as_str();
        let mut call_sites = 0usize;
        while let Some(at) = cursor.find(call) {
            let is_definition = cursor[..at].trim_end().ends_with("fn");
            cursor = &cursor[at + call.len()..];
            if is_definition {
                continue;
            }
            call_sites += 1;
            let args = &cursor[..cursor.find(')').unwrap_or(cursor.len())];
            // The texel must appear as a WHOLE argument, not merely somewhere
            // in the window: `sample_lightmap_irradiance(in.lightmap_texel +
            // parallax, ..)` contains the token but is exactly the offset this
            // forbids.
            // Its position varies — `shadowmask_union_subtraction` takes the
            // world position first — so match any argument, not the first.
            let accepted: &[&str] = if call == "sample_lightmap_animated(" {
                &["animated.uv"]
            } else {
                &["in.lightmap_texel", "lightmap_texel"]
            };
            let verbatim = args.split(',').any(|arg| accepted.contains(&arg.trim()));
            assert!(
                verbatim,
                "forward: `{call}` must sample the atlas at the interpolated lightmap texel, unmodified — got `{}`",
                args.trim(),
            );
            assert!(
                !args.contains("shade_uv") && !args.contains("depth."),
                "forward: `{call}` must never receive a marched UV — got `{}`",
                args.trim(),
            );
        }
        assert!(
            call_sites > 0,
            "forward: no call site for `{call}`, so this guard asserts nothing —              the call was renamed or removed and the assertion list is stale",
        );
    }

    // A floor under the per-call-site assertions above: any line naming the
    // lightmap UV or texel — a raw `textureSample` a future edit adds outside
    // the named helpers included — must not mix in a marched UV. Scanning every
    // line keeps those sites guarded without having to enumerate them.
    for line in forward_code.lines() {
        if !line.contains("lightmap_uv") && !line.contains("lightmap_texel") {
            continue;
        }
        assert!(
            !line.contains("shade_uv") && !line.contains("depth."),
            "forward: a lightmap-atlas read must not mix in a marched UV — `{}`",
            line.trim(),
        );
    }

    // The mover has no lightmap path at all, so it has nothing to offset. Pin
    // that fact rather than looping the checks above over it, which is what the
    // previous form did — vacuously, since the token never appears there.
    let kinematic_code = strip_line_comments(KINEMATIC);
    assert!(
        !kinematic_code.contains("lightmap_uv") && !kinematic_code.contains("lightmap_texel"),
        "the mover grew a lightmap path — extend the per-call-site assertions to it",
    );

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
/// through thin walls. Surface Depth's side-wall normal points ALONG the surface
/// (perpendicular to the surface NORMAL, lying in the tangent plane), so biasing
/// along it would slide the lookup sideways across the face instead of lifting
/// it off — the opposite of what the bias is for. The two
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
    let kinematic = strip_line_comments(KINEMATIC);

    // The march is skipped unless the light actually REACHES the fragment.
    //
    // Asserted on the `contributes` binding rather than on one inlined
    // expression, because the gate has to cover attenuation and color as well
    // as the Lambert term: a spot in range but aimed elsewhere, or a scripted
    // light whose descriptor is present but inactive, would otherwise burn one
    // of the two budget slots on a march whose result is multiplied by zero,
    // leaving the light that genuinely casts the shadow unshadowed.
    for (label, code, n_dot_l) in [
        ("forward", &forward, "NdotL > 0.0"),
        ("kinematic brush", &kinematic, "n_dot_l > 0.0"),
    ] {
        assert!(
            code.contains("contributes && depth_shadow_marches") || code.contains("&& contributes"),
            "{label}: the self-shadow march must be gated on the light contributing",
        );
        let at = code
            .find("let contributes =")
            .unwrap_or_else(|| panic!("{label}: no `contributes` gate for the march"));
        let rest = &code[at..];
        let gate = &rest[..rest.find(';').unwrap_or(rest.len())];
        for term in [n_dot_l, "attenuation > 0.0", "effective_color"] {
            assert!(
                gate.contains(term),
                "{label}: the march gate must account for `{term}` — `{}`",
                gate.split_whitespace().collect::<Vec<_>>().join(" "),
            );
        }
    }

    // A side hit's normal lies IN the tangent plane, so the Lambert term alone
    // admits a light behind the brush. Both consumers must also gate on the
    // geometric plane, and must leave TOP hits alone so an uncarved fragment
    // (and every fragment at `Off`) stays byte-identical.
    for (label, code) in [("forward", &forward), ("kinematic brush", &kinematic)] {
        assert!(
            code.contains("let plane_lit = depth.hit_top ||"),
            "{label}: the light gate must short-circuit on a TOP hit, which is what keeps \
             an uncarved fragment and every fragment at `Off` byte-identical",
        );
        assert!(
            code.contains("dot(mesh_n, L) > 0.0"),
            "{label}: a side hit must be gated on the GEOMETRIC plane too, or a light \
             behind opaque brush geometry lights its carved side walls at full strength",
        );
    }
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
        "out.height_m = 0.0;",
        "out.peak_raise = 0.0;",
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
        10,
        "each degenerate case (no map, faded out, zero authored depth, singular \
         Jacobian, zero UV scale, collapsed tangent plane, a resolved scale that is \
         not positive, an empty band (P4), edge-on, a starved march (D7)) must \
         return the flat result",
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

/// The player's Surface Depth switch (design D5) reaches the shader as DATA in
/// the per-material uniform, because the switch is applied by rewriting that
/// buffer — this engine has no shader-variant system, so anything the switch
/// must turn off has to be decodable from the packed march word.
#[test]
fn the_player_switch_reaches_the_shader_through_the_packed_march_word() {
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

    let concrete = Material::Concrete.surface_depth();
    let on = sd::SurfaceDepthUniform::resolve(
        concrete,
        sd::SurfaceDepthQuality::On,
        true,
        11,
        sd::SURFACE_DEPTH_RESIDENT_BASE_MIP,
        sd::SurfaceRelief::FULL_RANGE,
    );
    let off = sd::SurfaceDepthUniform::resolve(
        concrete,
        sd::SurfaceDepthQuality::Off,
        true,
        11,
        sd::SURFACE_DEPTH_RESIDENT_BASE_MIP,
        sd::SurfaceRelief::FULL_RANGE,
    );
    let decoded_on = sd::unpack_surface_depth_march(on.march_word());
    assert!(decoded_on.has_depth);
    assert!(
        decoded_on.shadow_light_budget > 0,
        "On must budget the self-shadow march",
    );
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
        sd::SurfaceDepthQuality::On,
        true,
        11,
        sd::SURFACE_DEPTH_RESIDENT_BASE_MIP,
        sd::SurfaceRelief::FULL_RANGE,
    );
    assert!(with_map.has_depth);
    let without_map = sd::SurfaceDepthUniform::resolve(
        concrete,
        sd::SurfaceDepthQuality::On,
        false,
        11,
        sd::SURFACE_DEPTH_RESIDENT_BASE_MIP,
        sd::SurfaceRelief::FULL_RANGE,
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
    // Identical specular and emissive words: Surface Depth changes nothing a
    // pre-existing material relied on. Bytes 8..16 carry the relief band.
    assert_eq!(carved[..8], flat[..8]);
    assert_ne!(carved[8..16], flat[8..16]);
    assert_ne!(carved[16..], flat[16..]);
    assert!(
        flat[8..].iter().all(|&byte| byte == 0),
        "a material without a surface map must upload the historical all-zero rows",
    );

    // The band is the QUANTIZED relief, peak first. FULL_RANGE at the
    // concrete's level count lands on a terrace edge, not the raw 127/128.
    let band = sd::SurfaceRelief::FULL_RANGE.quantized(concrete.quantize_levels as f32);
    assert_eq!(carved[8..12], band.peak_raise.to_le_bytes());
    assert_eq!(carved[12..16], band.trough.to_le_bytes());
    assert_eq!(with_map.relief, band);
}

/// `g = 0` is MAXIMUM RAISE under the signed encoding, so the has-depth bit is
/// the only thing keeping the R8 placeholder (and any other single-channel
/// slot) on its true plane. Whatever G byte such a slot would read as, a
/// non-`Rg8Unorm` slot must never set the bit and must upload no band.
#[test]
fn a_non_surface_map_slot_never_marches_even_when_its_g_would_read_as_max_raise() {
    // What the shader would decode from a stored G of 0.
    assert_eq!(
        sd::surface_height_fraction(0.0),
        sd::SURFACE_HEIGHT_MAX_RAISE
    );
    let max_raise = sd::SurfaceRelief {
        peak_raise: sd::SURFACE_HEIGHT_MAX_RAISE,
        trough: 0.0,
    };
    for format in [
        wgpu::TextureFormat::R8Unorm,
        wgpu::TextureFormat::Bc4RUnorm,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureFormat::Bc5RgUnorm,
    ] {
        let is_surface_map = specular_slot_is_surface_map(format);
        assert!(!is_surface_map, "{format:?} must not be a surface map");
        let plan = postretro_render_cpu::material_plan::MaterialUniformPlan::new(
            Material::Concrete,
            is_surface_map,
            11,
            max_raise,
        );
        for quality in sd::SurfaceDepthQuality::ALL {
            let bytes = plan.uniform_bytes(quality);
            let march = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
            assert_eq!(
                march & sd::SURFACE_DEPTH_HAS_DEPTH_BIT,
                0,
                "{format:?} at {quality:?} must not set the has-depth bit",
            );
            assert!(
                bytes[8..].iter().all(|&byte| byte == 0),
                "{format:?} at {quality:?} must upload the all-zero flat rows",
            );
        }
    }
    // And the shader refuses to march without the bit, whatever the band says.
    let code = strip_line_comments(SNIPPET);
    assert!(
        code.contains("(material.surface_depth_march & SURFACE_DEPTH_HAS_DEPTH_BIT) != 0u"),
        "surface_depth_has_map must test the has-depth bit",
    );
    assert!(
        code.contains("if !surface_depth_has_map() {\n        return flat_result;"),
        "the resolve must bail out flat when the has-depth bit is clear, before any fetch",
    );
}

/// The signed march's structure, pinned against the CPU authority's: it starts
/// at the peak (D6) or the eye, whichever is lower (the eye bound), measures
/// the band (P1/P4), resolves a starved march flat (D7), reports a signed
/// height along the view ray, measures AO from the peak (D5) and ends the
/// shadow march at the peak's clearance (P3). The CPU's
/// single-texel early-out (P2) has no GPU branch: it resolves exactly what the
/// loop's first iteration does, so parity holds on results.
#[test]
fn the_shader_march_mirrors_the_signed_cpu_march() {
    let code = strip_line_comments(SNIPPET);
    for expected in [
        "let peak = material.surface_depth_peak_raise;",
        "let trough = material.surface_depth_trough;",
        "let band_m = (peak - trough) * depth_scale_m;",
        "if !(band_m > 0.0) {",
        "let top = min(peak, view_distance * descent / depth_scale_m);",
        "let top_m = top * depth_scale_m;",
        "let start = p0 - dir * top_m;",
        "let solid = (top - surface_depth_texel(surface_depth_fold(dda.cell, dims_i), base_mip, levels))",
        "if walked + 1u >= max_steps {",
        "let height_m = top_m - z_hit;",
        "out.world_position = world_position + view_to_eye * (height_m / descent);",
        "clamp(depth.peak_raise - depth.height_m / depth.depth_scale_m, 0.0, 1.0)",
        "let clearance = depth.peak_raise * depth.depth_scale_m - depth.height_m;",
        "if !(clearance > SURFACE_DEPTH_SHADOW_BIAS_M) {",
        "if min(dda.t_max.x, dda.t_max.y) >= clearance {",
        "if clearance - risen > solid + SURFACE_DEPTH_SHADOW_BIAS_M {",
    ] {
        assert!(
            code.contains(expected),
            "the shader march has drifted from the CPU authority: missing `{expected}`",
        );
    }
    // The starved branch resolves flat: it must return the flat result, with
    // `carved = false`, rather than a hit at the last crossed boundary.
    let starved = code
        .find("if walked + 1u >= max_steps {")
        .expect("starved branch");
    assert!(
        code[starved..]
            .trim_start_matches(|c: char| c != '\n')
            .trim_start()
            .starts_with("return flat_result;"),
        "a starved march must resolve flat at the plane (D7)",
    );
    // The eye bound needs the fragment-to-eye distance: both consumers pass the
    // camera distance, not some other length.
    for (label, consumer, camera) in [
        ("forward", FORWARD, "uniforms.camera_position"),
        ("kinematic brush", KINEMATIC, "camera.camera_position"),
    ] {
        let consumer = normalized(&strip_line_comments(consumer));
        for expected in [
            format!("let view_vector = {camera} - in.world_position;"),
            "let view_distance = length(view_vector);".to_owned(),
            "surface_depth_resolve( in.uv, in.world_position, mesh_n, V, view_distance,".to_owned(),
        ] {
            assert!(
                consumer.contains(&expected),
                "{label}: the eye bound must receive the camera distance: missing `{expected}`",
            );
        }
    }
    // The old carve-only vocabulary must be gone so no consumer keeps the old
    // sign by accident.
    assert!(
        !code.contains(".depth_m") && !code.contains("    depth_m:"),
        "the signed field is `height_m`",
    );
    for (label, consumer) in [("forward", FORWARD), ("kinematic brush", KINEMATIC)] {
        let consumer = strip_line_comments(consumer);
        assert!(
            !consumer.contains(".depth_m"),
            "{label} must read `height_m`, not the renamed depth field",
        );
    }
}

/// The march shapes that were measured on AMD Metal (Metal System Trace,
/// `campaign-test`, GPU held at its top clock), pinned so a reshape is a
/// deliberate re-measurement rather than an accident.
///
/// No single-texel early-out branch: dropping it changes no result and made the
/// forward pass faster even with Surface Depth off. The light march folds once
/// and steps the folded coordinate: a signed `%` per fetch lowers to an integer
/// divide plus naga's guards, about sixty instructions a step.
#[test]
fn the_dda_shapes_hold_their_measured_cost() {
    let code = strip_line_comments(SNIPPET);
    assert!(
        !code.contains("surface_depth_single_texel_band"),
        "the GPU march has no single-texel early-out; the CPU keeps it as the \
         authority's proof that it is exact",
    );
    // A GPU early-out would compare the ray's first crossing against the band
    // height under some other name. `band_m` may only be declared and fed to
    // the empty-band test (P4).
    assert_eq!(
        code.matches("band_m").count(),
        2,
        "`band_m` is read only by the empty-band test; a second reader is a \
         single-texel early-out, which needs a re-measure on AMD Metal",
    );
    let fold = &code[code.find("fn surface_depth_fold(").expect("fold")..];
    assert!(
        fold[..fold.find('}').expect("fold body")].contains(" % "),
        "the modulo belongs to the fold",
    );
    assert_eq!(
        code.matches(" % ").count(),
        1,
        "only `surface_depth_fold` may take a modulo",
    );
    let light = &code[code
        .find("fn surface_depth_light_visibility(")
        .expect("light march")..];
    assert!(
        light.contains("var texel = surface_depth_fold(dda.cell, dims_i);")
            && light.matches("surface_depth_fold_step(texel.").count() == 2,
        "the light march steps a folded coordinate on both axes",
    );
}

/// Collapse every whitespace run to one space, so a pinned expression survives
/// a reflow across lines.
fn normalized(code: &str) -> String {
    code.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The body of `fn name(` up to the next `fn` in [`normalized`] code, for
/// pins that must hold in one march and not merely somewhere in the snippet.
fn function_body<'a>(code: &'a str, name: &str) -> &'a str {
    let at = code
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("snippet must declare {name}"));
    let rest = &code[at + 3..];
    &rest[..rest.find(" fn ").unwrap_or(rest.len())]
}

/// Every step of the two DDAs that a reshape could silently change, pinned as
/// shader text against the CPU authority, which proves each one by test
/// (`postretro_render_cpu::surface_depth`'s tests: DDA setup, corner
/// tie-break, the hit rules' equality cases, side-hit signs, the shadow budget,
/// the AO clamp). Shape, not bits: GPU parity is exact for the byte decode, the
/// half step and mid-gray, but the terrace division may differ by an ulp under
/// fast math, so no test here asserts GPU bit equality.
#[test]
fn the_shader_dda_steps_mirror_the_cpu_authority() {
    let code = normalized(&strip_line_comments(SNIPPET));
    let mut pins = vec![
        // DDA setup: start cell, sentinels, and per-axis step, first crossing
        // and spacing. The zero test is `abs(dir) > EPS`; the CPU writes it
        // `!above(|dir|, EPS)` so a NaN axis is zero on both sides.
        "dda.cell = vec2<i32>(floor(origin));".to_owned(),
        "dda.t_max = vec2<f32>(SURFACE_DEPTH_FAR, SURFACE_DEPTH_FAR);".to_owned(),
        "dda.t_delta = vec2<f32>(SURFACE_DEPTH_FAR, SURFACE_DEPTH_FAR);".to_owned(),
        "dda.step_dir = vec2<i32>(0, 0);".to_owned(),
        // The hit rules: `>=` on entry (side wall), strict `>` on exit (top).
        "let z_exit = min(dda.t_max.x, dda.t_max.y);".to_owned(),
        "if z_enter >= solid { z_hit = z_enter; hit_normal_ts = entry_normal_ts; hit_bias = entry_bias; break; }".to_owned(),
        "if z_exit > solid { z_hit = solid; break; }".to_owned(),
        // The view-march budget is floored at one iteration.
        "let max_steps = max(packed & SURFACE_DEPTH_MAX_STEPS_MASK, 1u);".to_owned(),
        // The resolved hit: unbiased march UV, biased sample UV, face normal
        // from the TBN, top test.
        "let hit_texel = start + dir * z_hit;".to_owned(),
        "out.uv = (hit_texel + hit_bias) / dims;".to_owned(),
        "out.march_uv = hit_texel / dims;".to_owned(),
        "out.normal = normalize( tangent * hit_normal_ts.x + bitangent * hit_normal_ts.y + geo_normal * hit_normal_ts.z );".to_owned(),
        "out.hit_top = hit_normal_ts.z > 0.5;".to_owned(),
        // The self-shadow budget: half the view budget, never zero.
        "out.shadow_steps = max(max_steps / 2u, 1u);".to_owned(),
        "for (var i: u32 = 0u; i < depth.shadow_steps; i = i + 1u) {".to_owned(),
        // The height is read from the G channel, at an explicit level.
        "let stored_g = textureLoad(spec_texture, folded, level).g;".to_owned(),
        // The fold and the folded step.
        "let folded = coord % dims;".to_owned(),
        "return select(folded + dims, folded, folded >= vec2<i32>(0, 0));".to_owned(),
        "let next = folded + step;".to_owned(),
        "return select(select(next, next - dim, next >= dim), next + dim, next < 0);".to_owned(),
        // Fade: distance ramp, LOD ramp, LOD measured in resident texels, and
        // the stricter of the two.
        "if fade_distance_m <= 0.0 { return 0.0; }".to_owned(),
        "let ramp = max(fade_distance_m * SURFACE_DEPTH_FADE_DISTANCE_FRACTION, SURFACE_DEPTH_EPS);".to_owned(),
        "return clamp((fade_distance_m - distance_m) / ramp, 0.0, 1.0);".to_owned(),
        "return clamp(1.0 - (lod - SURFACE_DEPTH_FADE_LOD_START) / SURFACE_DEPTH_FADE_LOD_RANGE, 0.0, 1.0);".to_owned(),
        "let lod = log2(max(footprint, SURFACE_DEPTH_EPS));".to_owned(),
        "let fade = min( surface_depth_distance_fade(view_distance, material.surface_depth_fade_distance), surface_depth_lod_fade(lod), );".to_owned(),
        // AO: gated on a carved hit and its own mask bit, divided by the
        // CLAMPED scale (the resolve clamps before both terms), and scaled by
        // the fade.
        "depth_scale_m = min(depth_scale_m, SURFACE_DEPTH_MAX_METERS * fade);".to_owned(),
        "out.depth_scale_m = depth_scale_m;".to_owned(),
        "if !depth.carved || (light_terms & LIGHT_TERM_DEPTH_AO) == 0u { return 1.0; }".to_owned(),
        "if !(depth.depth_scale_m > SURFACE_DEPTH_EPS) { return 1.0; }".to_owned(),
        "return 1.0 - SURFACE_DEPTH_AO_STRENGTH * depth.fade * clamp(depth.peak_raise - depth.height_m / depth.depth_scale_m, 0.0, 1.0);".to_owned(),
        // D7 for the self-shadow march: a flat (starved) result never marches.
        "fn surface_depth_light_visibility(depth: SurfaceDepthResult, to_light: vec3<f32>) -> f32 { if !depth.carved { return 1.0; }".to_owned(),
    ];
    for axis in ["x", "y"] {
        pins.extend([
            format!("if abs(dir.{axis}) > SURFACE_DEPTH_EPS {{"),
            format!("let positive = dir.{axis} > 0.0;"),
            format!("dda.step_dir.{axis} = select(-1, 1, positive);"),
            format!(
                "let boundary = select(f32(dda.cell.{axis}), f32(dda.cell.{axis} + 1), positive);"
            ),
            format!("dda.t_max.{axis} = (boundary - origin.{axis}) / dir.{axis};"),
            format!("dda.t_delta.{axis} = abs(1.0 / dir.{axis});"),
            // Both marches advance the crossed axis by its spacing.
            format!("dda.t_max.{axis} = dda.t_max.{axis} + dda.t_delta.{axis};"),
        ]);
    }
    // Side hits: the normal is the crossed axis negated, and the sample UV is
    // biased half a texel along the step, into the entered texel.
    pins.extend([
        "dda.cell.x = dda.cell.x + dda.step_dir.x; z_enter = dda.t_max.x; dda.t_max.x = dda.t_max.x + dda.t_delta.x; entry_normal_ts = vec3<f32>(-f32(dda.step_dir.x), 0.0, 0.0); entry_bias = vec2<f32>(f32(dda.step_dir.x) * SURFACE_DEPTH_SIDE_UV_BIAS_TEXELS, 0.0);".to_owned(),
        "dda.cell.y = dda.cell.y + dda.step_dir.y; z_enter = dda.t_max.y; dda.t_max.y = dda.t_max.y + dda.t_delta.y; entry_normal_ts = vec3<f32>(0.0, -f32(dda.step_dir.y), 0.0); entry_bias = vec2<f32>(0.0, f32(dda.step_dir.y) * SURFACE_DEPTH_SIDE_UV_BIAS_TEXELS);".to_owned(),
        "texel.x = surface_depth_fold_step(texel.x, dda.step_dir.x, dims_i.x); risen = dda.t_max.x;".to_owned(),
        "texel.y = surface_depth_fold_step(texel.y, dda.step_dir.y, dims_i.y); risen = dda.t_max.y;".to_owned(),
    ]);
    for pin in &pins {
        assert!(
            code.contains(pin.as_str()),
            "the shader DDA has drifted from the CPU authority: missing `{pin}`",
        );
    }

    // Tie-break: U is stepped on a tie, in BOTH marches.
    for march in ["surface_depth_resolve", "surface_depth_light_visibility"] {
        assert_eq!(
            function_body(&code, march)
                .matches("if dda.t_max.x <= dda.t_max.y {")
                .count(),
            1,
            "{march} must step U on a corner tie, as the CPU authority does",
        );
    }
}

/// The pinned fold and folded-step expressions, transcribed with WGSL's
/// semantics (`%` truncates like Rust's; `select(f, t, c)` is `if c { t } else
/// { f }`), wrap exactly like `AddressMode::Repeat`: stepping a folded
/// coordinate equals folding the stepped one.
#[test]
fn the_pinned_fold_expressions_wrap_like_repeat() {
    let select = |f: i32, t: i32, c: bool| if c { t } else { f };
    let fold = |coord: i32, dim: i32| {
        let folded = coord % dim;
        select(folded + dim, folded, folded >= 0)
    };
    let fold_step = |folded: i32, step: i32, dim: i32| {
        let next = folded + step;
        select(select(next, next - dim, next >= dim), next + dim, next < 0)
    };
    for dim in 1..=7 {
        for coord in -40..40 {
            assert_eq!(
                fold(coord, dim),
                coord.rem_euclid(dim),
                "fold {coord} in {dim}"
            );
            for step in -1..=1 {
                assert_eq!(
                    fold_step(fold(coord, dim), step, dim),
                    (coord + step).rem_euclid(dim),
                    "step {step} from {coord} in {dim}",
                );
            }
        }
    }
}
