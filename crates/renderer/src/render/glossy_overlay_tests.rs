// Glossy overlay shader-contract, layout and gate tests: the overlay lands
// where forward shaded, reads only its own layout, and draws under its gate.
// See: context/lib/testing_guide.md · context/lib/rendering_pipeline.md §7.3a

use super::*;
use crate::render::gpu_test_harness::wgsl_function;

const OVERLAY_WGSL: &str = include_str!("../shaders/glossy_overlay.wgsl");
const FORWARD_WGSL: &str = include_str!("../shaders/forward.wgsl");

/// `src` with every `//` comment removed, then every whitespace character.
fn squashed(src: &str) -> String {
    src.lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<String>()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// The text of `struct name { ... };` in `src`, squashed.
fn wgsl_struct(src: &str, name: &str) -> String {
    let start = src
        .find(&format!("struct {name} {{"))
        .unwrap_or_else(|| panic!("struct {name}"));
    let end = start + src[start..].find("};").expect("struct end") + 2;
    squashed(&src[start..end])
}

#[test]
fn glossy_overlay_shader_validates_with_its_composed_modules() {
    let module =
        naga::front::wgsl::parse_str(GLOSSY_OVERLAY_SHADER_SOURCE).expect("overlay WGSL parses");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("overlay WGSL validates");
    let entries: Vec<_> = module
        .entry_points
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(entries, ["vs_main", "fs_main"]);
}

// The overlay lands on exactly the pixels forward shaded only while both
// compute clip position by the same expression over the same uniform, under
// `@invariant` (rendering_pipeline.md §7.2).
#[test]
fn overlay_vertex_stage_repeats_forwards_invariant_clip_position() {
    let clip = "out.clip_position=uniforms.view_proj*vec4<f32>(in.position,1.0);";
    assert!(squashed(wgsl_function(FORWARD_WGSL, "vs_main")).contains(clip));
    assert!(squashed(wgsl_function(OVERLAY_WGSL, "vs_main")).contains(clip));
    for (name, source) in [("forward", FORWARD_WGSL), ("overlay", OVERLAY_WGSL)] {
        assert!(
            squashed(source).contains("@invariant@builtin(position)clip_position:vec4<f32>"),
            "{name} declares its clip position invariant"
        );
    }
    // The same uniform, member for member: forward's is commented, the
    // overlay's (as the sums pass's) is not.
    assert_eq!(
        wgsl_struct(OVERLAY_WGSL, "Uniforms"),
        wgsl_struct(FORWARD_WGSL, "Uniforms")
    );
    assert_eq!(
        squashed(wgsl_function(OVERLAY_WGSL, "oct_decode")),
        squashed(wgsl_function(FORWARD_WGSL, "oct_decode"))
    );
    assert_eq!(
        squashed(wgsl_function(OVERLAY_WGSL, "oct_encode_unquantized")),
        squashed(wgsl_function(
            include_str!("../shaders/sh_sample.wgsl"),
            "oct_encode_unquantized"
        ))
    );
    let fluid = include_str!("../shaders/fluid_surface.wgsl");
    for name in [
        "FluidProbeLight",
        "FluidProbe",
        "FluidProbeTable",
        "FluidProbeSums",
    ] {
        assert_eq!(
            wgsl_struct(OVERLAY_WGSL, name),
            wgsl_struct(fluid, name),
            "{name} matches the fluid shader's"
        );
    }
}

// Forward's layout and budget stay untouched: the overlay binds the shared
// camera group and two groups of its own, textures, uniforms and one sampler
// only — no SH, lightmap, shadow, light, emissive or SDF binding, no storage
// buffer (rendering_pipeline.md §10; brief IP fences).
#[test]
fn overlay_layout_binds_only_its_probe_records_and_static_diffuse() {
    assert_eq!(GLOSSY_OVERLAY_BIND_GROUP_COUNT, 3);
    let groups = squashed(OVERLAY_WGSL);
    for group in 3..8 {
        assert!(
            !groups.contains(&format!("@group({group})")),
            "the overlay declares group {group}"
        );
    }
    assert!(!groups.contains("var<storage"), "no storage binding");
    assert!(
        !groups.contains("textureStore"),
        "the overlay writes no tile"
    );
    for entry in glossy_overlay_probe_layout_entries()
        .iter()
        .chain(&glossy_overlay_inputs_layout_entries())
    {
        assert_eq!(entry.visibility, wgpu::ShaderStages::FRAGMENT);
        assert!(!matches!(
            entry.ty,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { .. },
                ..
            } | wgpu::BindingType::StorageTexture { .. }
        ));
    }
    for forbidden in [
        "sh_sample.wgsl",
        "lightmap_sample.wgsl",
        "shadow_sample.wgsl",
        "light_eval.wgsl",
        "fluid_reflection_lights.wgsl",
        "fluid_reflection_oracle.wgsl",
        "curve_eval.wgsl",
        "sdf_light_select.wgsl",
        "screen_march.wgsl",
    ] {
        assert!(
            !include_str!("glossy_overlay.rs").contains(&format!("shaders/{forbidden}\")")),
            "the overlay composes {forbidden}"
        );
    }
}

// IP fences: the probe comes from the face's binding by position, the layer
// from the probe alone (inside the shared sample), and the live lighting is
// the sums' blend; no stored light is evaluated per fragment.
#[test]
fn overlay_reads_its_probe_from_the_binding_and_lights_through_the_sums() {
    let term = squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_term"));
    assert!(
        term.contains("letprobe=fluid_reflection_sample(in.probe,in.world_position,view,normal);")
    );
    assert_eq!(term.matches("fluid_reflection_sample(").count(), 1);
    assert!(squashed(wgsl_function(OVERLAY_WGSL, "vs_main")).contains("out.probe=in.probe;"));
    let sample = squashed(wgsl_function(
        include_str!("../shaders/fluid_reflection.wgsl"),
        "fluid_reflection_sample",
    ));
    assert!(sample.contains("letlayer=fluid_probes.probes[probe].layer;"));
    assert!(sample.contains("fluid_reflection_ratio_from_sums(probe,search.lookup)"));
    let composed = squashed(GLOSSY_OVERLAY_SHADER_SOURCE);
    assert!(!composed.contains("fnfluid_reflection_ratio("));
    assert!(!composed.contains("anim_descriptors"));
}

// The composite: forward + W·(R − D_static), D_static at the fragment's own
// pixel, W the Schlick weight on water's reflectance scale times the
// capture-shadow weight; additive over the scene colour.
#[test]
fn overlay_adds_the_weighted_reflection_minus_static_diffuse() {
    assert_eq!(
        squashed(wgsl_function(OVERLAY_WGSL, "fs_main")),
        "fnfs_main(in:GlossyVertexOutput)->@location(0)vec4<f32>{\
         letterm=glossy_overlay_term(in);\
         letd_static=glossy_overlay_static_diffuse(in.clip_position.xy);\
         returnvec4<f32>(term.weight*(term.reflection-d_static),0.0);}"
    );
    assert!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_term")).contains(
            "letfresnel=fluid_reflection_fresnel(dot(view,normal),GLOSSY_OVERLAY_REFLECTIVITY);\
             returnGlossyOverlayTerm(fresnel*probe.weight,probe.capture);"
        )
    );
    assert!(squashed(OVERLAY_WGSL).contains("constGLOSSY_OVERLAY_REFLECTIVITY:f32=2.0;"));
    assert_eq!(
        postretro_render_cpu::fluid_surface::reflection::reflection_fresnel(1.0, 2.0),
        0.04
    );
    let blend = glossy_overlay_blend_state();
    assert_eq!(blend.color.src_factor, wgpu::BlendFactor::One);
    assert_eq!(blend.color.dst_factor, wgpu::BlendFactor::One);
    assert_eq!(blend.color.operation, wgpu::BlendOperation::Add);
    assert_eq!(blend.alpha.src_factor, wgpu::BlendFactor::Zero);
}

// Forward's always-run entry shades exactly as the shared shading does and
// declares one output; the static-diffuse entry adds D_static as the second.
// D_static is taken before any highlight or dynamic light joins the light.
#[test]
fn forward_entries_share_shading_and_only_the_static_diffuse_entry_writes_it() {
    assert_eq!(
        squashed(wgsl_function(FORWARD_WGSL, "fs_main")),
        "fnfs_main(in:VertexOutput)->@location(0)vec4<f32>{returnforward_shade(in).colour;}"
    );
    assert!(squashed(FORWARD_WGSL).contains(
        "structForwardStaticDiffuseOutput{@location(0)colour:vec4<f32>,\
             @location(1)static_diffuse:vec4<f32>,};"
    ));
    let shade = squashed(wgsl_function(FORWARD_WGSL, "forward_shade"));
    let static_diffuse = shade
        .find("letstatic_diffuse=base_color.rgb*total_light;")
        .expect("D_static is the albedo times the static light");
    let total = shade
        .find("vartotal_light=select(vec3<f32>(0.0),vec3<f32>(uniforms.ambient_floor),use_ambient_floor)+indirect+static_direct;")
        .expect("static light");
    let specular = shade
        .find("total_light=total_light+specular_sum;")
        .expect("highlight joins later");
    assert!(total < static_diffuse && static_diffuse < specular);
    let returns = shade.matches("return").count();
    assert_eq!(
        shade.matches("returnForwardShaded(").count(),
        returns,
        "every early return writes both outputs"
    );
}

#[test]
fn static_diffuse_takes_the_four_byte_format_where_it_renders() {
    use crate::render::renderer_init_pipelines::static_diffuse_format;
    assert_eq!(
        static_diffuse_format(wgpu::Features::RG11B10UFLOAT_RENDERABLE),
        wgpu::TextureFormat::Rg11b10Ufloat
    );
    assert_eq!(
        static_diffuse_format(wgpu::Features::empty()),
        wgpu::TextureFormat::Rgba16Float
    );
    assert_eq!(
        postretro_level_loader::NO_WORLD_PROBE,
        postretro_render_cpu::fluid_surface::reflection::FLUID_REFLECTION_NO_PROBE
    );
}

// The gate: off, any all-terms bit off, a level not ready, or no glossy
// range draws nothing; all four on draws.
#[test]
fn overlay_gate_needs_tier_mask_ready_level_and_a_glossy_range() {
    use FluidReflectionQuality::*;
    let all = LightTermMask::ALL;
    for quality in [Low, Medium, High, VeryHigh, Ultra] {
        assert!(glossy_overlay_draws(quality, all, true, true));
        assert!(!glossy_overlay_draws(quality, all, false, true));
        assert!(!glossy_overlay_draws(quality, all, true, false));
        for term in LightTermMask::ALL_TERMS {
            let mut mask = all;
            mask.set_enabled(term, false);
            assert!(!glossy_overlay_draws(quality, mask, true, true), "{term:?}");
        }
    }
    assert!(!glossy_overlay_draws(Off, all, true, true));
}

// The overlay records after the skinned-mesh pass and before the Ghost Pass
// and every translucent pass; forward precedes it.
#[test]
fn overlay_records_after_the_skinned_mesh_pass_and_before_the_ghost_pass() {
    let frame = squashed(include_str!("renderer_render_frame.rs"));
    let at = |needle: &str| {
        frame
            .find(needle)
            .unwrap_or_else(|| panic!("the frame records `{needle}`"))
    };
    let order = [
        at("self.record_reflection_probe_sums("),
        at("self.record_world_forward_pass("),
        at("label:Some(\"SkinnedMeshPass\")"),
        at("self.record_glossy_overlay_pass("),
        at("label:Some(\"GhostPass\")"),
        at("self.record_planar_reflection_pass("),
    ];
    assert!(order.is_sorted(), "{order:?}");
    assert_eq!(frame.matches("self.record_glossy_overlay_pass(").count(), 1);
}
