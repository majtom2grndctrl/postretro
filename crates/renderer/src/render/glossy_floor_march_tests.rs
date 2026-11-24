// Source gates and pure checks for the glossy floor layer's march and the
// overlay's floor composite: the march shader's composition and bindings,
// its IP fences (rows 40, 44, 53), the WGSL↔CPU pins of the blur at the hit
// and the layered composite, the floor layer's reach (row 45), and the
// recording order.
// See: context/lib/testing_guide.md · context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors) · context/plans/in-progress/glossy-world-reflections/ip-check.md §7

use super::*;
use crate::render::gpu_test_harness::{wgsl_const_line, wgsl_function};
use crate::render::pipeline_layout::{TIMING_PAIR_COUNT, TIMING_PAIR_GLOSSY_FLOOR_MARCH};

const MARCH_WGSL: &str = include_str!("../shaders/glossy_floor_march.wgsl");
const BLUR_WGSL: &str = include_str!("../shaders/glossy_floor_blur.wgsl");
const OVERLAY_WGSL: &str = include_str!("../shaders/glossy_overlay.wgsl");
const BLUR_CPU: &str = include_str!("../../../render-cpu/src/glossy_floor_blur.rs");

/// `src` with every `//` comment removed, then every whitespace character.
fn squashed(src: &str) -> String {
    src.lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<String>()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// `source`'s declared bindings, `(group, binding, name)`, sorted.
fn bindings(source: &str) -> Vec<(u32, u32, String)> {
    let module = naga::front::wgsl::parse_str(source).expect("WGSL parses");
    let mut declared: Vec<_> = module
        .global_variables
        .iter()
        .filter_map(|(_, global)| {
            let binding = global.binding.as_ref()?;
            Some((binding.group, binding.binding, global.name.clone()?))
        })
        .collect();
    declared.sort();
    declared
}

fn named(list: &[(u32, u32, &str)]) -> Vec<(u32, u32, String)> {
    list.iter()
        .map(|&(group, binding, name)| (group, binding, name.to_owned()))
        .collect()
}

#[test]
fn the_floor_march_shader_validates_with_its_composed_modules() {
    let module =
        naga::front::wgsl::parse_str(GLOSSY_FLOOR_MARCH_SHADER_SOURCE).expect("march WGSL parses");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("march WGSL validates");
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

// Acceptance row 40 (source gate) and the carried §7.3a IP rules: the floor
// layer's march is water's `screen_march`, composed unmodified from the one
// file water composes (`the_march_and_its_confidence_read_no_direction` pins
// its body and the floor's entry); the pass binds the camera, the world
// material, scene depth and the level's chain alone, so no probe colour,
// distance or sums array and no static diffuse; and no capture-shadow,
// probe-search or live-lighting function is composed into it (the ray file's
// box-projection helpers are geometry, composed as water's SSR pass composes
// them, and unreachable from the pinned march body).
#[test]
fn the_floor_march_is_waters_and_binds_no_probe_quantity() {
    assert!(
        GLOSSY_FLOOR_MARCH_SHADER_SOURCE.contains(include_str!("../shaders/screen_march.wgsl")),
        "the march composes screen_march.wgsl whole"
    );
    assert_eq!(
        GLOSSY_FLOOR_MARCH_SHADER_SOURCE
            .matches("fn screen_march(")
            .count(),
        1
    );
    assert_eq!(
        bindings(GLOSSY_FLOOR_MARCH_SHADER_SOURCE),
        named(&[
            (0, 0, "uniforms"),
            (1, 0, "base_texture"),
            (1, 2, "spec_texture"),
            (1, 3, "material"),
            (1, 4, "t_normal"),
            (1, 5, "aniso_sampler"),
            (2, 0, "scene_depth"),
            (3, 0, "floor_chain"),
            (3, 1, "floor_chain_sampler"),
            (3, 2, "floor_march"),
        ]),
        "the march's binding inventory"
    );
    let composed = squashed(GLOSSY_FLOOR_MARCH_SHADER_SOURCE);
    for forbidden in [
        "fluid_probe",
        "fluid_probes",
        "static_diffuse",
        "fluid_reflection_search",
        "fluid_reflection_shadow",
        "fluid_reflection_sample",
        "fluid_reflection_ratio",
        "fluid_reflection_layered_composite",
        "glossy_composite",
        "glossy_overlay_probe",
        "var<storage",
        "textureStore",
    ] {
        assert!(
            !composed.contains(forbidden),
            "the march reaches `{forbidden}`"
        );
    }
    for forbidden in [
        "fluid_reflection.wgsl",
        "fluid_reflection_lights.wgsl",
        "fluid_reflection_oracle.wgsl",
        "reflection_probe_lobes.wgsl",
        "sh_sample.wgsl",
        "lightmap_sample.wgsl",
        "shadow_sample.wgsl",
        "light_eval.wgsl",
        "glossy_overlay.wgsl",
    ] {
        assert!(
            !include_str!("glossy_floor/march.rs").contains(&format!("shaders/{forbidden}\")")),
            "the march composes {forbidden}"
        );
    }
    for entry in glossy_floor_scene_layout_entries()
        .iter()
        .chain(&glossy_floor_level_layout_entries())
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
    // Every march pipeline builds over the shared camera and material layouts
    // and the floor layer's own two.
    let pass = squashed(include_str!("glossy_floor.rs"));
    assert!(pass.contains(
        "bind_group_layouts:&[Some(uniform_layout),Some(material_layout),Some(&scene_layout),\
         Some(&level_layout),],"
    ));
}

// Acceptance rows 36 and 40, fence B1 (`ip-check.md` §7): the chain level a
// hit reads is one function of the texel's exponent, the hit length and the
// hit's view depth, with the camera's focal length and the chain's scale,
// and the minimum blur inside the probe term's own exponent → mip function.
// It mirrors render-cpu `glossy_floor_blur` statement for statement, its
// constants pinned; the hit's colour is one isotropic read at the hit's own
// screen position. Nothing in it names s, the distance tile, a
// capture-shadow flag, probe colour or the hit's confidence.
#[test]
fn the_floor_blur_reads_exponent_minimum_blur_and_hit_length_alone() {
    use postretro_render_cpu::glossy_floor_blur::{
        GLOSSY_FLOOR_NEAREST_DEPTH, GLOSSY_FLOOR_PROBE_MIPS, GLOSSY_FLOOR_WIDEST_HALF_ANGLE,
    };
    assert_eq!(
        squashed(wgsl_const_line(BLUR_WGSL, "GLOSSY_FLOOR_PROBE_MIPS")),
        format!("constGLOSSY_FLOOR_PROBE_MIPS:u32={GLOSSY_FLOOR_PROBE_MIPS}u;")
    );
    assert_eq!(
        squashed(wgsl_const_line(BLUR_WGSL, "GLOSSY_FLOOR_WIDEST_HALF_ANGLE")),
        format!("constGLOSSY_FLOOR_WIDEST_HALF_ANGLE:f32={GLOSSY_FLOOR_WIDEST_HALF_ANGLE:?};")
    );
    assert_eq!(
        squashed(wgsl_const_line(BLUR_WGSL, "GLOSSY_FLOOR_NEAREST_DEPTH")),
        format!("constGLOSSY_FLOOR_NEAREST_DEPTH:f32={GLOSSY_FLOOR_NEAREST_DEPTH:?};")
    );
    assert_eq!(
        squashed(wgsl_function(BLUR_WGSL, "glossy_floor_blur_half_angle")),
        "fnglossy_floor_blur_half_angle(exponent:f32)->f32{\
         returnmin(REFLECTION_PROBE_MIP0_TEXEL_ANGLE\
         *exp2(reflection_probe_exponent_mip(exponent,GLOSSY_FLOOR_PROBE_MIPS)),\
         GLOSSY_FLOOR_WIDEST_HALF_ANGLE);}"
    );
    assert_eq!(
        squashed(wgsl_function(BLUR_CPU, "glossy_floor_blur_half_angle")),
        "fnglossy_floor_blur_half_angle(exponent:f32)->f32{\
         (REFLECTION_PROBE_MIP0_TEXEL_ANGLE\
         *reflection_probe_exponent_mip(exponent,GLOSSY_FLOOR_PROBE_MIPS).exp2())\
         .min(GLOSSY_FLOOR_WIDEST_HALF_ANGLE)}"
    );
    assert_eq!(
        squashed(wgsl_function(BLUR_WGSL, "glossy_floor_blur_level")),
        "fnglossy_floor_blur_level(exponent:f32,hit_length:f32,hit_depth:f32,focal:f32,\
         chain_scale:f32,levels:u32)->f32{\
         lethalf_width=tan(glossy_floor_blur_half_angle(exponent))*hit_length*focal\
         /max(hit_depth,GLOSSY_FLOOR_NEAREST_DEPTH);\
         returnclamp(log2(2.0*half_width*chain_scale),0.0,f32(levels-1u));}"
    );
    assert_eq!(
        squashed(wgsl_function(BLUR_CPU, "glossy_floor_blur_level")),
        "fnglossy_floor_blur_level(exponent:f32,hit_length:f32,hit_depth:f32,focal:f32,\
         chain_scale:f32,levels:u32,)->f32{\
         lethalf_width=glossy_floor_blur_half_angle(exponent).tan()*hit_length*focal\
         /hit_depth.max(GLOSSY_FLOOR_NEAREST_DEPTH);\
         (2.0*half_width*chain_scale).log2().max(0.0).min(levels.saturating_sub(1)asf32)}"
    );
    assert_eq!(
        squashed(wgsl_function(BLUR_WGSL, "glossy_floor_blurred_hit")),
        "fnglossy_floor_blurred_hit(uv:vec2<f32>,level:f32)->vec3<f32>{\
         returnclamp(textureSampleLevel(floor_chain,floor_chain_sampler,uv,level).rgb,\
         vec3<f32>(0.0),vec3<f32>(65504.0));}"
    );
    // The march hands the level the exponent and the hit alone, and reads
    // at the hit's own screen position.
    assert_eq!(
        squashed(wgsl_function(MARCH_WGSL, "glossy_floor_hit_colour")),
        "fnglossy_floor_hit_colour(pixel:vec2<f32>,exponent:f32,hit:GlossyFloorHit,\
         extent:vec2<f32>,focal:f32)->vec3<f32>{\
         letchain_scale=f32(textureDimensions(floor_chain).y)/extent.y;\
         letlevel=glossy_floor_blur_level(exponent,hit.hit_length,hit.hit_depth,focal,\
         chain_scale,textureNumLevels(floor_chain));\
         returnglossy_floor_blurred_hit(pixel/extent,level);}"
    );
    assert_eq!(
        squashed(wgsl_function(MARCH_WGSL, "glossy_floor_focal")),
        "fnglossy_floor_focal(extent:vec2<f32>)->f32{\
         letrow_y=vec3<f32>(uniforms.view_proj[0].y,uniforms.view_proj[1].y,\
         uniforms.view_proj[2].y);\
         returnlength(row_y)*0.5*extent.y;}"
    );
    // The exponent is the one the overlay's texel reads.
    assert_eq!(
        squashed(wgsl_function(MARCH_WGSL, "glossy_floor_exponent")),
        "fnglossy_floor_exponent(uv:vec2<f32>,ddx:vec2<f32>,ddy:vec2<f32>)->f32{\
         letslot=sample_post_retro(spec_texture,aniso_sampler,uv,ddx,ddy);\
         returnspecular_texel(slot,SPECULAR_EXPONENT_MIN).exponent;}"
    );
    assert!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_texel"))
            .contains("letslot=sample_post_retro(spec_texture,aniso_sampler,uv,ddx,ddy);")
    );
    assert!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_texel"))
            .contains("lettexel=specular_texel(slot,SPECULAR_EXPONENT_MIN);")
    );
    for (file, code) in [
        ("glossy_floor_blur.wgsl", squashed(BLUR_WGSL)),
        (
            "glossy_floor_march.wgsl glossy_floor_hit_colour",
            squashed(wgsl_function(MARCH_WGSL, "glossy_floor_hit_colour")),
        ),
        (
            "glossy_floor_march.wgsl glossy_floor_hit",
            squashed(wgsl_function(MARCH_WGSL, "glossy_floor_hit")),
        ),
        (
            "render-cpu glossy_floor_blur.rs",
            squashed(wgsl_function(BLUR_CPU, "glossy_floor_blur_level")),
        ),
    ] {
        let lower = code.to_lowercase();
        for word in [
            "shadow",
            "probe_distance",
            "distance",
            "capture",
            "fluid_probe",
            "confidence",
            "march.",
            "static_diffuse",
            "normal",
            "noise",
        ] {
            assert!(!lower.contains(word), "{file} names `{word}`");
        }
    }
}

/// The floor march's entry function, code only: pinned whole beside water's
/// `fs_reflection` by the march's IP gate in `fluid_surface_tests`.
pub(crate) const GLOSSY_FLOOR_MARCH_BODY: &str = "fnglossy_floor_march(in:GlossyVertexOutput,\
     pixel:bool,carve:bool,normal_map:bool)->vec4<f32>{\
     letderivatives=glossy_overlay_derivatives(in);\
     letdepth=glossy_overlay_depth(in,derivatives,carve);\
     letexponent=glossy_floor_exponent(depth.uv,derivatives.ddx,derivatives.ddy);\
     letsurface=glossy_overlay_surface(in,derivatives,depth,pixel,normal_map);\
     if!fluid_reflection_seen_from_above(surface.outward,surface.view)\
     ||!fluid_reflection_mask_on(uniforms.light_term_mask){discard;}\
     letray=fluid_reflection_above_hemisphere(fluid_reflection_mirror(surface.view,\
     surface.normal),fluid_reflection_hemisphere_axis(surface.outward));\
     letextent=vec2<f32>(textureDimensions(scene_depth));\
     letfocal=glossy_floor_focal(extent);\
     letorigin=glossy_floor_origin(surface.origin,surface.outward,focal);\
     letmarch=screen_march(origin,ray,uniforms.view_proj,scene_depth,\
     floor_march.stride,floor_march.step_budget,floor_march.thickness);\
     letcolour=glossy_floor_hit_colour(march.pixel,exponent,\
     glossy_floor_hit(origin,ray,march.pixel,extent),extent,focal);\
     letpremultiplied=vec4<f32>(colour*march.confidence,march.confidence);\
     returnselect(vec4<f32>(0.0),premultiplied,march.confidence>0.0);}";

// The march starts where the probe term's lookup does and turns about the
// same normal: the overlay's shading inputs and its point, the face's own
// hemisphere, one ray per target pixel (each entry one style and form, the
// exponent setting the blur alone). Its origin lifts off the floor along the
// face normal by one pixel's footprint at its view depth, reading the point,
// the face normal and the projection alone; the hit length is solved from
// the stop pixel by least squares, choosing no axis.
#[test]
fn the_floor_march_starts_from_the_probe_terms_point_and_writes_premultiplied() {
    assert_eq!(
        squashed(wgsl_function(MARCH_WGSL, "glossy_floor_march")),
        GLOSSY_FLOOR_MARCH_BODY
    );
    assert_eq!(
        squashed(wgsl_function(MARCH_WGSL, "glossy_floor_origin")),
        "fnglossy_floor_origin(point:vec3<f32>,outward:vec3<f32>,focal:f32)->vec3<f32>{\
         letdepth=(uniforms.view_proj*vec4<f32>(point,1.0)).w;\
         returnpoint+outward*(GLOSSY_FLOOR_ORIGIN_LIFT_PIXELS*max(depth,0.0)/focal);}"
    );
    assert_eq!(
        squashed(wgsl_const_line(
            MARCH_WGSL,
            "GLOSSY_FLOOR_ORIGIN_LIFT_PIXELS"
        )),
        "constGLOSSY_FLOOR_ORIGIN_LIFT_PIXELS:f32=1.0;"
    );
    assert_eq!(
        squashed(wgsl_function(MARCH_WGSL, "glossy_floor_hit")),
        "fnglossy_floor_hit(origin:vec3<f32>,ray:vec3<f32>,pixel:vec2<f32>,\
         extent:vec2<f32>)->GlossyFloorHit{\
         letndc=vec2<f32>(pixel.x/extent.x*2.0-1.0,1.0-pixel.y/extent.y*2.0);\
         letstart=uniforms.view_proj*vec4<f32>(origin,1.0);\
         letalong=uniforms.view_proj*vec4<f32>(ray,0.0);\
         letnum=ndc*start.w-start.xy;\
         letden=along.xy-ndc*along.w;\
         lethit_length=max(dot(num,den)/max(dot(den,den),1.0e-12),0.0);\
         returnGlossyFloorHit(hit_length,start.w+hit_length*along.w);}"
    );
    // The overlay's probe term mirrors the same view about the same normal.
    assert!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_sample")).contains(
            "letray=fluid_reflection_above_hemisphere(fluid_reflection_mirror(view,normal),axis);"
        )
    );
    assert!(
        squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_probe")).contains(
            "returnglossy_overlay_sample(in.probe,surface.origin,surface.view,surface.normal,\
             fluid_reflection_hemisphere_axis(surface.outward),exponent);"
        )
    );
    for (entry, pixel, carve, normal_map) in [
        ("fs_pixel", "true", "true", "true"),
        ("fs_smooth", "false", "true", "true"),
        ("fs_pixel_flat", "true", "false", "true"),
        ("fs_smooth_flat", "false", "false", "true"),
        ("fs_pixel_plain", "true", "false", "false"),
        ("fs_smooth_plain", "false", "false", "false"),
    ] {
        assert_eq!(
            squashed(wgsl_function(MARCH_WGSL, entry)),
            format!(
                "fn{entry}(in:GlossyVertexOutput)->@location(0)vec4<f32>{{\
                 returnglossy_floor_march(in,{pixel},{carve},{normal_map});}}"
            )
        );
    }
    // The target and its own depth, as water's: premultiplied Rgba16Float,
    // no blend, the nearest floor owning a texel; cleared every recorded
    // frame, and timed as its own pair.
    assert_eq!(GLOSSY_FLOOR_LAYER_FORMAT, wgpu::TextureFormat::Rgba16Float);
    let depth = glossy_floor_depth_stencil_state();
    assert_eq!(depth.depth_write_enabled, Some(true));
    assert_eq!(depth.depth_compare, Some(wgpu::CompareFunction::Less));
    let march = squashed(include_str!("glossy_floor/march.rs"));
    assert!(march.contains("format:GLOSSY_FLOOR_LAYER_FORMAT,blend:None,"));
    let frame = squashed(include_str!("renderer_glossy_floor_frame.rs"));
    assert!(frame.contains(
        "view:targets.layer_view(),depth_slice:None,resolve_target:None,ops:wgpu::Operations{\
         load:wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),store:wgpu::StoreOp::Store,},"
    ));
    assert!(frame.contains(
        "view:targets.layer_depth_view(),depth_ops:Some(wgpu::Operations{\
         load:wgpu::LoadOp::Clear(1.0),store:wgpu::StoreOp::Discard,}),"
    ));
    assert!(frame.contains("render_pass_writes(TIMING_PAIR_GLOSSY_FLOOR_MARCH)"));
    const { assert!(TIMING_PAIR_GLOSSY_FLOOR_MARCH < TIMING_PAIR_COUNT) }
    assert_eq!(TIMING_PAIR_COUNT, 26);
    // The march uniform is the shader's struct, member for member.
    assert_eq!(GLOSSY_FLOOR_MARCH_UNIFORM_BYTES, 16);
    assert!(
        squashed(MARCH_WGSL).contains(
            "structGlossyFloorMarch{step_budget:u32,stride:f32,thickness:f32,spare:f32,};"
        )
    );
}

// Acceptance row 45 (source half): no face but a probe-bound, up-facing one
// takes the floor layer. The march's vertex stage drops every other face
// whole (water's up-facing test on the face's flat normal); the overlay
// reads the floor layer only through its `_floor` entries, and there only on
// an up-facing face under the probe term's gate. Every other overlay entry
// reaches no floor-layer binding, so its compiled form is the probe term's.
#[test]
fn only_probe_bound_up_facing_faces_take_the_floor_layer() {
    let vertex = squashed(wgsl_function(MARCH_WGSL, "vs_main"));
    assert!(vertex.contains(
        "lettakes_layer=in.probe!=FLUID_REFLECTION_NO_PROBE\
         &&out.world_normal.y>=REFLECTION_UP_FACING_MIN_COS;\
         out.clip_position=select(GLOSSY_FLOOR_CULLED,\
         uniforms.view_proj*vec4<f32>(in.position,1.0),takes_layer);"
    ));
    assert!(
        squashed(MARCH_WGSL)
            .contains("constGLOSSY_FLOOR_CULLED:vec4<f32>=vec4<f32>(2.0,2.0,2.0,1.0);")
    );
    let screen = squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_floor_screen"));
    assert!(screen.starts_with(
        "fnglossy_overlay_floor_screen(in:GlossyVertexOutput,surface:GlossySurface,\
         pixel:bool)->vec4<f32>{\
         ifin.probe==FLUID_REFLECTION_NO_PROBE\
         ||surface.outward.y<REFLECTION_UP_FACING_MIN_COS\
         ||!fluid_reflection_seen_from_above(surface.outward,surface.view)\
         ||!fluid_reflection_mask_on(uniforms.light_term_mask){returnvec4<f32>(0.0);}"
    ));
    let overlay = squashed(OVERLAY_WGSL);
    assert_eq!(
        overlay.matches("glossy_overlay_floor_screen(").count(),
        2,
        "declared, and read by the floor delta alone"
    );
    assert_eq!(overlay.matches("glossy_overlay_floor_delta(").count(), 2);
    assert_eq!(overlay.matches("glossy_overlay_shade_floor(").count(), 7);
    assert_eq!(
        overlay.matches("floor_layer").count(),
        4,
        "declared, and read by the floor screen's two reads and its size"
    );
    for entry in [
        "fs_pixel",
        "fs_smooth",
        "fs_pixel_flat",
        "fs_smooth_flat",
        "fs_pixel_plain",
        "fs_smooth_plain",
    ] {
        let body = squashed(wgsl_function(OVERLAY_WGSL, entry));
        assert!(body.contains("glossy_overlay_shade(in,"), "{entry}: {body}");
    }
    assert!(!squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_shade")).contains("floor"));
    assert!(!squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_delta")).contains("floor"));
}

// Acceptance rows 4, 28 and the guard (plan Corrections): on a floor the
// overlay's composite is water's layered form over the probe term, the same
// `glossy_composite` the CPU reference mirrors (c = 0 is the probe term's
// line), with F = Fresnel × `_s` × sharpness fade and the screen-space
// coefficient never gaining s. The floor layer is read behind the
// zero-weight early-out and after the coplanar-mover guard's mark, so a
// texel of zero weight and a pixel a mover covers take nothing from either
// layer. Fence B3: smooth reads at the fragment's own screen position, pixel
// at its texel centre's projection, with no normal- or noise-driven offset.
#[test]
fn the_floor_composite_is_the_layered_form_behind_both_early_outs() {
    let shade = squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_shade"));
    let floor_shade = squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_shade_floor"));
    assert_eq!(
        floor_shade,
        shade
            .replace("fnglossy_overlay_shade(", "fnglossy_overlay_shade_floor(")
            .replace(
                "returnglossy_overlay_delta(in,texel,surface);",
                "returnglossy_overlay_floor_delta(in,texel,surface,pixel);"
            )
    );
    assert!(floor_shade.contains("iftexel.mask<=0.0{returnvec3<f32>(0.0);}"));
    let delta = squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_delta"));
    let floor_delta = squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_floor_delta"));
    assert_eq!(
        floor_delta,
        delta
            .replace(
                "fnglossy_overlay_delta(in:GlossyVertexOutput,texel:GlossyTexel,\
                 surface:GlossySurface)",
                "fnglossy_overlay_floor_delta(in:GlossyVertexOutput,texel:GlossyTexel,\
                 surface:GlossySurface,pixel:bool)"
            )
            .replace(
                "returnglossy_composite(d_static.rgb,probe.capture,probe.weight,vec4<f32>(0.0),",
                "letscreen=glossy_overlay_floor_screen(in,surface,pixel);\
                 returnglossy_composite(d_static.rgb,probe.capture,probe.weight,screen,"
            )
    );
    let guard = floor_delta
        .find("ifglossy_overlay_mover_covered(d_static){returnvec3<f32>(0.0);}")
        .expect("the guard");
    let read = floor_delta
        .find("glossy_overlay_floor_screen(")
        .expect("the floor read");
    assert!(guard < read, "the guard's mark gates the floor layer");
    let screen = squashed(wgsl_function(OVERLAY_WGSL, "glossy_overlay_floor_screen"));
    assert!(screen.contains(
        "if!pixel{returntextureSampleLevel(floor_layer,sh_atlas_sampler,\
         in.clip_position.xy/extent,0.0);}"
    ));
    assert!(screen.contains("letclip=uniforms.view_proj*vec4<f32>(surface.origin,1.0);"));
    assert!(screen.contains(
        "returntextureLoad(floor_layer,min(vec2<u32>(at*vec2<f32>(size)/extent),\
         size-vec2<u32>(1u)),0);"
    ));
    for word in [
        "normal", "noise", "hash", "offset", "shadow", "probe.", "weight",
    ] {
        assert!(
            !screen.to_lowercase().contains(word),
            "the floor read names `{word}`"
        );
    }
    for (entry, pixel, march, normal_map) in [
        ("fs_pixel_floor", "true", "true", "true"),
        ("fs_smooth_floor", "false", "true", "true"),
        ("fs_pixel_flat_floor", "true", "false", "true"),
        ("fs_smooth_flat_floor", "false", "false", "true"),
        ("fs_pixel_plain_floor", "true", "false", "false"),
        ("fs_smooth_plain_floor", "false", "false", "false"),
    ] {
        assert_eq!(
            squashed(wgsl_function(OVERLAY_WGSL, entry)),
            format!(
                "fn{entry}(in:GlossyVertexOutput)->@location(0)vec4<f32>{{\
                 returnvec4<f32>(glossy_overlay_shade_floor(in,{pixel},{march},{normal_map}),\
                 0.0);}}"
            )
        );
    }
    // The overlay's inputs: D_static, the sampler and the floor layer, the
    // floor layer filterable for the smooth read; no planar layer.
    let inputs = crate::render::glossy_overlay::glossy_overlay_inputs_layout_entries();
    assert_eq!(inputs.len(), 3);
    assert!(matches!(
        inputs[2].ty,
        wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            ..
        }
    ));
    assert!(squashed(OVERLAY_WGSL).contains("@group(3)@binding(2)varfloor_layer:texture_2d<f32>;"));
}

// Acceptance row 53: the no-history gate's binding inventory covers the
// overlay and the floor march. Both inventories are closed, so a previous
// frame's target, a reprojection or an intersection table would be a new
// binding they refuse, and no source of either names one. Each target the
// floor layer writes clears every frame it records (the chain's and the
// highlight-free colour's: `the_floor_layers_sources_hold_no_history`).
#[test]
fn the_overlay_and_the_floor_march_hold_no_history() {
    assert_eq!(
        bindings(crate::render::glossy_overlay::GLOSSY_OVERLAY_SHADER_SOURCE),
        named(&[
            (0, 0, "uniforms"),
            (1, 0, "base_texture"),
            (1, 2, "spec_texture"),
            (1, 3, "material"),
            (1, 4, "t_normal"),
            (1, 5, "aniso_sampler"),
            (2, 0, "fluid_probe_array"),
            (2, 1, "fluid_probes"),
            (2, 2, "fluid_probe_distance"),
            (2, 3, "fluid_probe_sums"),
            (3, 0, "static_diffuse"),
            (3, 1, "sh_atlas_sampler"),
            (3, 2, "floor_layer"),
        ]),
        "the overlay's binding inventory"
    );
    let forbidden = [
        "history",
        "previous",
        "prev_",
        "last_frame",
        "reproject",
        "temporal",
        "intersection",
        "hit_buffer",
        "hit_table",
    ];
    for (file, source) in [
        ("glossy_floor_march.wgsl", GLOSSY_FLOOR_MARCH_SHADER_SOURCE),
        (
            "glossy_floor/march.rs",
            include_str!("glossy_floor/march.rs"),
        ),
        (
            "glossy_floor/targets.rs",
            include_str!("glossy_floor/targets.rs"),
        ),
        (
            "renderer_glossy_floor_frame.rs",
            include_str!("renderer_glossy_floor_frame.rs"),
        ),
        (
            "glossy_overlay.wgsl",
            crate::render::glossy_overlay::GLOSSY_OVERLAY_SHADER_SOURCE,
        ),
        ("glossy_overlay.rs", include_str!("glossy_overlay.rs")),
        (
            "glossy_overlay/level.rs",
            include_str!("glossy_overlay/level.rs"),
        ),
        (
            "renderer_glossy_frame.rs",
            include_str!("renderer_glossy_frame.rs"),
        ),
    ] {
        let code = squashed(source).to_lowercase();
        for word in forbidden {
            assert!(!code.contains(word), "{file} names `{word}`");
        }
    }
}
