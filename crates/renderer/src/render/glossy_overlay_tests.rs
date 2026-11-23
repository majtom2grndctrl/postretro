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

// Acceptance row 32 (source gate): forward's layout and budget stay
// untouched, and the overlay's layout refuses every lighting binding: it
// binds the shared camera group, forward's material group (read for albedo
// and the specular slot only), and two groups of its own, textures, uniforms
// and samplers only — no SH, lightmap, animated-weight, shadowmask,
// shadow-pool, dynamic light, emissive or emissive-volume, or SDF binding, no
// storage buffer (rendering_pipeline.md §10; brief IP fences).
#[test]
fn overlay_layout_binds_only_its_probe_records_and_static_diffuse() {
    assert_eq!(GLOSSY_OVERLAY_BIND_GROUP_COUNT, 4);
    assert_eq!(
        (
            GLOSSY_OVERLAY_MATERIAL_GROUP,
            GLOSSY_OVERLAY_PROBE_GROUP,
            GLOSSY_OVERLAY_INPUTS_GROUP
        ),
        (1, 2, 3)
    );
    let groups = squashed(OVERLAY_WGSL);
    // The material group declares albedo, the specular slot and the sampler,
    // forward's bindings by name: never the glow, normal map or uniform.
    let material: Vec<_> = OVERLAY_WGSL
        .lines()
        .filter(|line| line.starts_with("@group(1)"))
        .collect();
    assert_eq!(
        material,
        [
            "@group(1) @binding(0) var base_texture: texture_2d<f32>;",
            "@group(1) @binding(2) var spec_texture: texture_2d<f32>;",
            "@group(1) @binding(5) var aniso_sampler: sampler;",
        ]
    );
    for line in &material {
        assert!(FORWARD_WGSL.contains(line), "forward declares `{line}`");
    }
    let composed = squashed(GLOSSY_OVERLAY_SHADER_SOURCE);
    for lighting in [
        "emissive_texture",
        "emissive_volume_table",
        "GpuLight",
        "light_influence",
        "spec_lights",
        "sh_total_atlas",
        "lightmap_irradiance",
        "animated_lm_atlas",
        "shadowmask_atlas",
        "spot_shadow_depth",
        "point_shadow_cube",
        "sdf_shadow_factor",
    ] {
        assert!(!composed.contains(lighting), "the overlay reads {lighting}");
    }
    for group in 4..8 {
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
    let term = squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_probe"));
    assert!(
        term.contains("returnfluid_reflection_sample(in.probe,in.world_position,view,normal);")
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
// pixel, W the texel's Fresnel × `_s` × sharpness fade times the
// capture-shadow weight, R coloured as the highlight; additive over the scene
// colour. A texel whose weight is 0 adds nothing and runs no probe search.
#[test]
fn overlay_adds_the_weighted_reflection_minus_static_diffuse() {
    assert_eq!(
        squashed(wgsl_function(OVERLAY_WGSL, "fs_main")),
        "fnfs_main(in:GlossyVertexOutput)->@location(0)vec4<f32>{\
         letddx=dpdx(in.uv);letddy=dpdy(in.uv);\
         lettexel=glossy_overlay_texel(in.uv,ddx,ddy);\
         returnvec4<f32>(glossy_overlay_delta(in,texel),0.0);}"
    );
    assert_eq!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_delta")),
        "fnglossy_overlay_delta(in:GlossyVertexOutput,texel:GlossyTexel)->vec3<f32>{\
         letnormal=normalize(in.normal);\
         letview=normalize(uniforms.camera_position-in.world_position);\
         letfresnel=glossy_fresnel(texel,dot(view,normal));\
         iffresnel<=0.0{returnvec3<f32>(0.0);}\
         letprobe=glossy_overlay_probe(in,view,normal);\
         letd_static=glossy_overlay_static_diffuse(in.clip_position.xy);\
         returnglossy_composite(d_static,probe.capture,probe.weight,vec4<f32>(0.0),fresnel,\
         texel.colour);}"
    );
    let blend = glossy_overlay_blend_state();
    assert_eq!(blend.color.src_factor, wgpu::BlendFactor::One);
    assert_eq!(blend.color.dst_factor, wgpu::BlendFactor::One);
    assert_eq!(blend.color.operation, wgpu::BlendOperation::Add);
    assert_eq!(blend.alpha.src_factor, wgpu::BlendFactor::Zero);
}

// Acceptance rows 4, 27, 34: the shader's weight and colour mirror render-cpu
// `glossy_reflection` statement for statement, with its constants pinned to
// the reference's (the sharpness fade's to level-format's, the reflectivities
// to water's Fresnel scale), and the texel is read as forward reads its
// highlight's: the same reconstruction and the tint rule forward composes.
#[test]
fn overlay_weight_and_colour_mirror_the_cpu_reference() {
    use postretro_render_cpu::fluid_surface::reflection::{
        FLUID_REFLECTION_BASE_F0, FLUID_REFLECTIVITY_MAX,
    };
    use postretro_render_cpu::glossy_reflection::*;
    let source = squashed(OVERLAY_WGSL);
    for (name, value) in [
        ("GLOSSY_REFLECTIVITY_UNTINTED", GLOSSY_REFLECTIVITY_UNTINTED),
        (
            "GLOSSY_REFLECTIVITY_FULL_TINT",
            GLOSSY_REFLECTIVITY_FULL_TINT,
        ),
        (
            "GLOSSY_SHARPNESS_FLOOR_EXPONENT",
            GLOSSY_SHARPNESS_FLOOR_EXPONENT,
        ),
        ("GLOSSY_FULL_WEIGHT_EXPONENT", GLOSSY_FULL_WEIGHT_EXPONENT),
    ] {
        assert!(
            source.contains(&format!("const{name}:f32={value:?};")),
            "{name} = {value:?}"
        );
    }
    assert_eq!(GLOSSY_REFLECTIVITY_FULL_TINT, FLUID_REFLECTIVITY_MAX);
    assert_eq!(
        FLUID_REFLECTION_BASE_F0 * GLOSSY_REFLECTIVITY_UNTINTED,
        0.04
    );
    assert_eq!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_sharpness_fade")),
        "fnglossy_sharpness_fade(exponent:f32)->f32{\
         letfloor=log2(GLOSSY_SHARPNESS_FLOOR_EXPONENT);\
         letfull=log2(GLOSSY_FULL_WEIGHT_EXPONENT);\
         returnclamp((log2(exponent)-floor)/(full-floor),0.0,1.0);}"
    );
    assert_eq!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_texel")),
        "fnglossy_texel(slot:vec4<f32>,albedo:vec3<f32>)->GlossyTexel{\
         lettexel=specular_texel(slot,SPECULAR_EXPONENT_MIN);\
         returnGlossyTexel(\
         texel.intensity*glossy_sharpness_fade(texel.exponent),\
         select(GLOSSY_REFLECTIVITY_UNTINTED,\
         mix(GLOSSY_REFLECTIVITY_UNTINTED,GLOSSY_REFLECTIVITY_FULL_TINT,texel.tint),\
         texel.tinted),\
         select(albedo,specular_tint(albedo,texel.tint),texel.tinted),);}"
    );
    assert_eq!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_fresnel")),
        "fnglossy_fresnel(texel:GlossyTexel,cos_theta:f32)->f32{\
         returnfluid_reflection_fresnel(cos_theta,texel.reflectivity)*texel.mask;}"
    );
    assert_eq!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_composite")),
        "fnglossy_composite(static_diffuse:vec3<f32>,probe:vec3<f32>,shadow:f32,\
         screen:vec4<f32>,fresnel:f32,colour:vec3<f32>)->vec3<f32>{\
         letconfidence=screen.a;\
         letprobe_share=(1.0-confidence)*shadow;\
         letreflected=colour*(fresnel*(screen.rgb+probe*probe_share));\
         letweight=min(fresnel*(confidence+probe_share),1.0);\
         returnreflected-static_diffuse*weight;}"
    );
    // The texel forward's highlight reads: forward's reconstruction, and the
    // tint rule composed from the one module both read.
    assert_eq!(
        squashed(wgsl_function(OVERLAY_WGSL, "sample_post_retro")),
        squashed(wgsl_function(FORWARD_WGSL, "sample_post_retro"))
    );
    assert!(
        GLOSSY_OVERLAY_SHADER_SOURCE.contains(include_str!("../shaders/material_shading.wgsl"))
    );
    assert!(
        squashed(wgsl_function(FORWARD_WGSL, "forward_shade"))
            .contains("letspec_texel=specular_texel(sample_color(spec_texture,shade_uv,ddx,ddy),")
    );
    assert_eq!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_texel")),
        "fnglossy_overlay_texel(uv:vec2<f32>,ddx:vec2<f32>,ddy:vec2<f32>)->GlossyTexel{\
         letalbedo=sample_post_retro(base_texture,aniso_sampler,uv,ddx,ddy).rgb;\
         letslot=sample_post_retro(spec_texture,aniso_sampler,uv,ddx,ddy);\
         returnglossy_texel(slot,albedo);}"
    );
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

// Acceptance row 30: D_static is stored by the scene colour's own
// conversion, so it never exceeds forward's stored colour and the composite
// cannot undershoot (`static_diffuse_format`).
#[test]
fn static_diffuse_takes_the_scene_colours_format() {
    use crate::render::renderer_init_pipelines::static_diffuse_format;
    assert_eq!(static_diffuse_format(), SCENE_COLOR_FORMAT);
    assert_eq!(static_diffuse_format(), wgpu::TextureFormat::Rgba16Float);
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

// Acceptance row 30: a character's promoted-light shadow is forward's
// shadowmask union subtraction from the static direct light, taken before
// D_static, so it is part of what the overlay replaces and fades by (1 − W)
// with the rest of the static diffuse.
#[test]
fn the_entity_shadow_subtraction_lands_in_static_diffuse() {
    let shade = squashed(wgsl_function(FORWARD_WGSL, "forward_shade"));
    let subtraction = shade
        .find("static_direct=max(static_direct-shadowmask_union,vec3<f32>(0.0));")
        .expect("the entity shadow subtracts from the static direct light");
    let total = shade
        .find("vartotal_light=select(vec3<f32>(0.0),vec3<f32>(uniforms.ambient_floor),use_ambient_floor)+indirect+static_direct;")
        .expect("static light");
    let static_diffuse = shade
        .find("letstatic_diffuse=base_color.rgb*total_light;")
        .expect("D_static");
    assert!(subtraction < total && total < static_diffuse);
    assert!(
        shade.rfind("static_direct=").expect("assigned") < total,
        "nothing changes the static direct light after the static light sums it"
    );
}
