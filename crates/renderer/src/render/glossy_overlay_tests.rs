// Glossy overlay shader-contract, layout and gate tests: the overlay lands
// where forward shaded, reads only its own layout, and draws under its gate.
// See: context/lib/testing_guide.md · context/lib/rendering_pipeline.md §7.3a

use super::*;
use crate::render::SCENE_COLOR_FORMAT;
use crate::render::gpu_test_harness::wgsl_function;

const OVERLAY_WGSL: &str = include_str!("../shaders/glossy_overlay.wgsl");
const SHADING_WGSL: &str = include_str!("../shaders/glossy_overlay_shading.wgsl");
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
    assert_eq!(
        entries,
        [
            "vs_main",
            "fs_pixel",
            "fs_smooth",
            "fs_pixel_flat",
            "fs_smooth_flat",
            "fs_pixel_plain",
            "fs_smooth_plain"
        ]
    );
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
    // The material group declares albedo, the specular slot, the material
    // uniform (Surface Depth's march words), the normal map and the sampler,
    // forward's bindings by name: never the glow.
    let material: Vec<_> = OVERLAY_WGSL
        .lines()
        .filter(|line| line.starts_with("@group(1)"))
        .collect();
    assert_eq!(
        material,
        [
            "@group(1) @binding(0) var base_texture: texture_2d<f32>;",
            "@group(1) @binding(2) var spec_texture: texture_2d<f32>;",
            "@group(1) @binding(3) var<uniform> material: MaterialUniform;",
            "@group(1) @binding(4) var t_normal: texture_2d<f32>;",
            "@group(1) @binding(5) var aniso_sampler: sampler;",
        ]
    );
    for line in &material {
        assert!(FORWARD_WGSL.contains(line), "forward declares `{line}`");
    }
    assert_eq!(
        wgsl_struct(OVERLAY_WGSL, "MaterialUniform"),
        wgsl_struct(FORWARD_WGSL, "MaterialUniform"),
        "the material uniform is forward's, member for member"
    );
    assert!(
        !SHADING_WGSL.contains("@group("),
        "the shading inputs bind nothing"
    );
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
// from the probe alone (inside the overlay's sample, water's with the ray
// kept in the face's hemisphere and the colour tap blurred), the hemisphere
// from the face's flat normal (never a lookup direction), and the live
// lighting is the sums' blend; no stored light is evaluated per fragment.
#[test]
fn overlay_reads_its_probe_from_the_binding_and_lights_through_the_sums() {
    let term = squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_probe"));
    assert!(term.contains(
        "returnglossy_overlay_sample(in.probe,surface.origin,surface.view,surface.normal,\
         fluid_reflection_hemisphere_axis(surface.outward),exponent);"
    ));
    assert_eq!(term.matches("glossy_overlay_sample(").count(), 1);
    assert!(!squashed(OVERLAY_WGSL).contains("fluid_reflection_sample("));
    assert!(squashed(wgsl_function(OVERLAY_WGSL, "vs_main")).contains("out.probe=in.probe;"));
    let water = squashed(wgsl_function(
        include_str!("../shaders/fluid_reflection.wgsl"),
        "fluid_reflection_sample",
    ));
    let sample = squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_sample"));
    assert!(sample.contains("letlayer=fluid_probes.probes[probe].layer;"));
    assert!(sample.contains("fluid_reflection_ratio_from_sums(probe,search.lookup)"));
    // Water's sample up to its colour tap: the same ray, layer and search,
    // statement for statement, with the ray kept in the face's hemisphere
    // where water floors it above the horizon (`FLUID_REFLECTION_UP`), so the
    // binding and search fences carry over.
    let prefix = |body: &str| {
        body[body.find('{').expect("body") + 1..]
            .split("letuv=")
            .next()
            .expect("prefix")
            .split("letcapture=")
            .next()
            .expect("prefix")
            .to_string()
    };
    let water_about_axis = prefix(&water)
        .replace(
            "fluid_reflection_above_horizon(fluid_reflection_mirror(view,outward))",
            "fluid_reflection_above_hemisphere(fluid_reflection_mirror(view,normal),axis)",
        )
        .replace("search(world_position,", "search(origin,")
        .replace("FLUID_REFLECTION_UP", "axis");
    assert_eq!(prefix(&sample), water_about_axis);
    assert!(
        squashed(wgsl_function(
            include_str!("../shaders/fluid_reflection_ray.wgsl"),
            "fluid_reflection_hemisphere_axis"
        ))
        .contains("select(normal,FLUID_REFLECTION_UP,normal.y>=REFLECTION_UP_FACING_MIN_COS)")
    );
    assert!(
        sample.contains("letcapture=glossy_overlay_probe_colour(search.lookup,layer,exponent);")
    );
    let composed = squashed(GLOSSY_OVERLAY_SHADER_SOURCE);
    assert!(!composed.contains("fnfluid_reflection_ratio("));
    assert!(!composed.contains("anim_descriptors"));
}

// The composite: forward + W·(R − D_static), D_static at the fragment's own
// pixel, W the texel's Fresnel × `_s` × sharpness fade times the
// capture-shadow weight, R coloured as the highlight; additive over the scene
// colour. A texel whose weight is 0 adds nothing and reads no normal map and
// runs no probe search; a pixel this frame's mover guard marked adds
// nothing, its mark read after the weight with D_static, in the one load
// (pins P15, P23). One entry per reflection style and material form over one
// shading.
#[test]
fn overlay_adds_the_weighted_reflection_minus_static_diffuse() {
    for (entry, pixel, march, normal_map) in [
        ("fs_pixel", "true", "true", "true"),
        ("fs_smooth", "false", "true", "true"),
        ("fs_pixel_flat", "true", "false", "true"),
        ("fs_smooth_flat", "false", "false", "true"),
        ("fs_pixel_plain", "true", "false", "false"),
        ("fs_smooth_plain", "false", "false", "false"),
    ] {
        assert_eq!(
            squashed(wgsl_function(OVERLAY_WGSL, entry)),
            format!(
                "fn{entry}(in:GlossyVertexOutput)->@location(0)vec4<f32>{{\
                 returnvec4<f32>(glossy_overlay_shade(in,{pixel},{march},{normal_map}),0.0);}}"
            )
        );
    }
    assert_eq!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_shade")),
        "fnglossy_overlay_shade(in:GlossyVertexOutput,pixel:bool,march:bool,\
         normal_map:bool)->vec3<f32>{\
         letderivatives=glossy_overlay_derivatives(in);\
         letdepth=glossy_overlay_depth(in,derivatives,march);\
         lettexel=glossy_overlay_texel(depth.uv,derivatives.ddx,derivatives.ddy);\
         iftexel.mask<=0.0{returnvec3<f32>(0.0);}\
         letsurface=glossy_overlay_surface(in,derivatives,depth,pixel,normal_map);\
         returnglossy_overlay_delta(in,texel,surface);}"
    );
    assert_eq!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_delta")),
        "fnglossy_overlay_delta(in:GlossyVertexOutput,texel:GlossyTexel,\
         surface:GlossySurface)->vec3<f32>{\
         letfresnel=glossy_fresnel(texel,dot(surface.view,surface.outward));\
         letprobe=glossy_overlay_probe(in,surface,texel.exponent);\
         letd_static=glossy_overlay_static_diffuse(in.clip_position.xy);\
         ifglossy_overlay_mover_covered(d_static){returnvec3<f32>(0.0);}\
         returnglossy_composite(d_static.rgb,probe.capture,probe.weight,vec4<f32>(0.0),\
         fresnel,texel.colour);}"
    );
    assert_eq!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_static_diffuse")),
        "fnglossy_overlay_static_diffuse(frag_xy:vec2<f32>)->vec4<f32>{\
         returntextureLoad(static_diffuse,vec2<u32>(frag_xy),0);}"
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
         select(albedo,specular_tint(albedo,texel.tint),texel.tinted),\
         texel.exponent,);}"
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

// Acceptance row 46 (recording order), pin P12: the overlay records after
// forward, the mover pass and the skinned-mesh pass, and before the Ghost
// Pass, the planar mirror, the fluid passes, billboards, billboard ghosts
// and the fog composite, so each of those draws over its result and nothing
// it draws is dimmed by the reflection weight. The coplanar-mover guard
// records inside the overlay's recording, ahead of the overlay's pass.
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
        at("self.record_fluid_surface_passes("),
        at("label:Some(\"BillboardSpritePass\")"),
        at("label:Some(\"BillboardGhostPass\")"),
        at("label:Some(\"FogCompositePass\")"),
    ];
    assert!(order.is_sorted(), "{order:?}");
    assert!(at("label:Some(\"KinematicBrushPass\")") < at("self.record_glossy_overlay_pass("));
    assert_eq!(frame.matches("self.record_glossy_overlay_pass(").count(), 1);
    let glossy = squashed(include_str!("renderer_glossy_frame.rs"));
    let guard = glossy
        .find(".guard().record(")
        .expect("the overlay records the guard");
    let overlay = glossy
        .find("label:Some(\"GlossyOverlayPass\")")
        .expect("the overlay's pass");
    assert!(
        guard < overlay,
        "the guard marks D_static before the overlay reads it"
    );
    assert_eq!(glossy.matches(".guard().record(").count(), 1);
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

// Acceptance rows 35, 67: the overlay shades from forward's own inputs. Its
// vertex stage decodes the tangent as forward does and interpolates position,
// UV, normal and tangent as forward's do; its march is forward's Surface Depth
// snippet over forward's arguments (the eye vector, its length, the
// derivatives), so it reads the texel forward shaded, and a live switch
// reaches both through the one material uniform; the normal it mirrors about
// is forward's shading normal (the march's face normal on a side hit, the
// normal map's on a top); and pixel style takes the texel-centre point
// forward's highlight takes, smooth the fragment (rendering_pipeline.md §4,
// Texel-centre highlight; §7.3, Surface Depth).
#[test]
fn overlay_shades_from_forwards_march_normal_and_texel_point() {
    let vertex = squashed(wgsl_function(OVERLAY_WGSL, "vs_main"));
    let forward_vertex = squashed(wgsl_function(FORWARD_WGSL, "vs_main"));
    for statement in [
        "letsign_bit=in.tangent_packed.y&0x8000u;",
        "letv_15bit=in.tangent_packed.y&0x7FFFu;",
        "letv_16bit=v_15bit*65535u/32767u;",
        "out.world_tangent=oct_decode(vec2<u32>(in.tangent_packed.x,v_16bit));",
        "out.bitangent_sign=select(-1.0,1.0,sign_bit!=0u);",
        "out.world_normal=oct_decode(in.normal_oct);",
        "out.uv=in.base_uv;",
        "out.world_position=in.position;",
    ] {
        assert!(forward_vertex.contains(statement), "forward: {statement}");
        assert!(vertex.contains(statement), "overlay: {statement}");
    }
    let output = wgsl_struct(OVERLAY_WGSL, "GlossyVertexOutput");
    for interpolated in [
        "@location(0)world_position:vec3<f32>",
        "@location(1)world_normal:vec3<f32>",
        "@location(3)uv:vec2<f32>",
        "@location(4)world_tangent:vec3<f32>",
        "@location(5)bitangent_sign:f32",
    ] {
        assert!(output.contains(interpolated), "{interpolated}: {output}");
    }

    let shade = squashed(wgsl_function(FORWARD_WGSL, "forward_shade"));
    for statement in [
        "letddx=dpdx(in.uv);",
        "letddy=dpdy(in.uv);",
        "letddx_world=dpdx(in.world_position);",
        "letddy_world=dpdy(in.world_position);",
        "letmesh_n=normalize(in.world_normal);",
        "letview_vector=uniforms.camera_position-in.world_position;",
        "letview_distance=length(view_vector);",
        "letV=normalize(view_vector);",
        "letdepth=surface_depth_resolve(in.uv,in.world_position,mesh_n,V,view_distance,ddx,\
         ddy,ddx_world,ddy_world,);",
        "letshade_uv=depth.uv;",
        "letn_ts=sample_normal(t_normal,shade_uv,ddx,ddy);",
        "letN_bump=reconstruct_tbn_normal(mesh_n,in.world_tangent,in.bitangent_sign,n_ts);",
        "letN_shade=select(depth.normal,N_bump,depth.hit_top);",
        "letspec_point=specular_texel_point(specular_style_smooth(),\
         SpecularPoint(in.world_position,V),in.uv,shade_uv,in.world_position,\
         uniforms.camera_position,vec2<f32>(textureDimensions(base_texture,0)),ddx,ddy,\
         ddx_world,ddy_world,);",
    ] {
        assert!(shade.contains(statement), "forward: {statement}");
    }
    assert_eq!(
        squashed(wgsl_function(SHADING_WGSL, "glossy_overlay_derivatives")),
        "fnglossy_overlay_derivatives(in:GlossyVertexOutput)->GlossyDerivatives{\
         returnGlossyDerivatives(dpdx(in.uv),dpdy(in.uv),dpdx(in.world_position),\
         dpdy(in.world_position));}"
    );
    assert_eq!(
        squashed(wgsl_function(SHADING_WGSL, "glossy_overlay_march")),
        "fnglossy_overlay_march(in:GlossyVertexOutput,d:GlossyDerivatives)->SurfaceDepthResult{\
         letmesh_n=normalize(in.world_normal);\
         letview_vector=uniforms.camera_position-in.world_position;\
         returnsurface_depth_resolve(in.uv,in.world_position,mesh_n,normalize(view_vector),\
         length(view_vector),d.ddx,d.ddy,d.ddx_world,d.ddy_world);}"
    );
    assert_eq!(
        squashed(wgsl_function(SHADING_WGSL, "glossy_overlay_surface")),
        "fnglossy_overlay_surface(in:GlossyVertexOutput,d:GlossyDerivatives,\
         depth:SurfaceDepthResult,pixel:bool,normal_map:bool)->GlossySurface{\
         letmesh_n=normalize(in.world_normal);\
         letn_ts=glossy_overlay_tangent_normal(depth.uv,d,normal_map);\
         letn_bump=reconstruct_tbn_normal(mesh_n,in.world_tangent,in.bitangent_sign,n_ts);\
         letnormal=select(depth.normal,n_bump,depth.hit_top);\
         letview=normalize(uniforms.camera_position-in.world_position);\
         letpoint=specular_texel_point(!pixel,SpecularPoint(in.world_position,view),in.uv,\
         depth.uv,in.world_position,uniforms.camera_position,\
         vec2<f32>(textureDimensions(base_texture,0)),d.ddx,d.ddy,d.ddx_world,d.ddy_world);\
         returnGlossySurface(point.position,point.view,normal,mesh_n);}"
    );
    // The march-free pipelines' depth is the march's own first early-out
    // over the same inputs: `surface_depth_flat` of the fragment's UV,
    // position and normalized normal, returned before any march when the
    // material has no map. Only materials that can never march draw through
    // them (`MaterialUniformPlan::can_march`, the overlay's record).
    assert_eq!(
        squashed(wgsl_function(SHADING_WGSL, "glossy_overlay_depth")),
        "fnglossy_overlay_depth(in:GlossyVertexOutput,d:GlossyDerivatives,\
         march:bool)->SurfaceDepthResult{\
         ifmarch{returnglossy_overlay_march(in,d);}\
         returnsurface_depth_flat(in.uv,in.world_position,normalize(in.world_normal));}"
    );
    let resolve = squashed(wgsl_function(
        include_str!("../shaders/surface_depth.wgsl"),
        "surface_depth_resolve",
    ));
    assert!(resolve.contains(
        "{letflat_result=surface_depth_flat(uv,world_position,geo_normal);\
         if!surface_depth_has_map(){returnflat_result;}"
    ));
    let overlay = squashed(include_str!("glossy_overlay.rs"));
    assert!(overlay.contains("letkeep=|bucket:u32|glossy(bucket)&&form(bucket)==drawn;"));
    assert!(overlay.contains("pass.set_pipeline(self.pipelines.for_style(style,drawn));"));
    assert!(squashed(include_str!("level_resource_set.rs")).contains(
        "GlossyOverlayForm::of(material.uniform_plan.can_march(),texture.normal_mapped)"
    ));
    // The plain form reads the neutral placeholder's one texel and decodes
    // it as forward's `sample_normal` decodes a sample; a material binds the
    // placeholder exactly when its normal slot was not consumed or did not
    // read.
    let sample_normal = squashed(wgsl_function(
        include_str!("../shaders/material_shading.wgsl"),
        "sample_normal",
    ));
    for statement in [
        "letz=sqrt(max(0.0,1.0-dot(rg,rg)));",
        "returnnormalize(vec3<f32>(rg,z));",
        ".rg*2.0-1.0;",
    ] {
        assert!(
            sample_normal.contains(statement),
            "sample_normal: {statement}"
        );
    }
    assert_eq!(
        squashed(wgsl_function(SHADING_WGSL, "glossy_overlay_tangent_normal")),
        "fnglossy_overlay_tangent_normal(uv:vec2<f32>,d:GlossyDerivatives,\
         normal_map:bool)->vec3<f32>{\
         ifnormal_map{returnsample_normal(t_normal,uv,d.ddx,d.ddy);}\
         letrg=textureLoad(t_normal,vec2<i32>(0),0).rg*2.0-1.0;\
         letz=sqrt(max(0.0,1.0-dot(rg,rg)));\
         returnnormalize(vec3<f32>(rg,z));}"
    );
    assert!(
        squashed(include_str!("loaded_texture.rs"))
            .contains("normal_mapped:plan.consume[2]&&slots[2].is_ok(),")
    );
    assert!(
        squashed(include_str!("loaded_texture/placeholders.rs")).contains("normal_mapped:false,")
    );
    // The march is forward's one snippet, composed whole, never a copy.
    assert!(GLOSSY_OVERLAY_SHADER_SOURCE.contains(include_str!("../shaders/surface_depth.wgsl")));
    assert!(!SHADING_WGSL.contains("fn surface_depth_"));
    assert!(!OVERLAY_WGSL.contains("fn surface_depth_"));
}

// The overlay's probe term is render-cpu `glossy_probe_term` statement for
// statement: the view mirrored about the shading normal, kept in the face's
// hemisphere about its axis, searched with that axis, then the exponent's
// colour mips toward the lookup times the sums' ratio toward it. The room
// gates compare the GPU against this reference per pixel.
#[test]
fn overlay_probe_term_mirrors_the_cpu_reference() {
    let cpu = include_str!("../../../render-cpu/src/glossy_reflection.rs");
    assert_eq!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_sample")),
        "fnglossy_overlay_sample(probe:u32,origin:vec3<f32>,view:vec3<f32>,normal:vec3<f32>,\
         axis:vec3<f32>,exponent:f32)->FluidReflectionSample{\
         letray=fluid_reflection_above_hemisphere(fluid_reflection_mirror(view,normal),axis);\
         letlayer=fluid_probes.probes[probe].layer;\
         letsearch=fluid_reflection_search(origin,ray,fluid_probes.probes[probe].box_min,\
         fluid_probes.probes[probe].box_max,fluid_probes.probes[probe].capture_point,axis,\
         fluid_probes.tile.xy,layer);\
         letcapture=glossy_overlay_probe_colour(search.lookup,layer,exponent);\
         returnFluidReflectionSample(\
         capture*fluid_reflection_ratio_from_sums(probe,search.lookup),search.weight);}"
    );
    assert_eq!(
        squashed(wgsl_function(cpu, "glossy_probe_term")),
        "fnglossy_probe_term(probe:&ReflectionProbeRecord,\
         sums:&[Vec3;REFLECTION_PROBE_LOBE_COUNT],origin:Vec3,view:Vec3,normal:Vec3,axis:Vec3,\
         tile:[u32;2],colour:implFn(Vec3)->Vec3,distance:implFn(Vec2)->f32,)->(Vec3,f32){\
         letray=reflection_above_hemisphere(reflection_mirror(view,normal),axis);\
         letsearch=reflection_search_about(origin,ray,Vec3::from(probe.box_min),\
         Vec3::from(probe.box_max),Vec3::from(probe.capture_point),axis,tile,distance,);\
         (colour(search.lookup)*reflection_ratio_from_sums(sums,search.lookup),\
         search.weight,)}"
    );
    // The axis is the face's, from its flat normal, never the lookup's.
    assert!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_probe"))
            .contains("fluid_reflection_hemisphere_axis(surface.outward)")
    );
}

// The form a bucket draws through: any material that can march takes the
// full shading; one that never marches takes the march-free shading, and
// the plain form when it binds the neutral normal placeholder.
#[test]
fn a_bucket_draws_through_the_form_its_material_needs() {
    assert_eq!(GlossyOverlayForm::of(true, true), GlossyOverlayForm::March);
    assert_eq!(GlossyOverlayForm::of(true, false), GlossyOverlayForm::March);
    assert_eq!(
        GlossyOverlayForm::of(false, true),
        GlossyOverlayForm::NormalMapped
    );
    assert_eq!(
        GlossyOverlayForm::of(false, false),
        GlossyOverlayForm::Plain
    );
}
