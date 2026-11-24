// Source gates and pure checks for the glossy floor layer's source half: the
// IP fences over the scene-colour chain, the highlight-free outputs' shape,
// and where its pipelines are built.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors) ·
// context/plans/in-progress/glossy-world-reflections/ip-check.md §7

use super::*;
use crate::render::gpu_test_harness::{wgsl_function, wgsl_item};

const FORWARD_WGSL: &str = include_str!("../shaders/forward.wgsl");
const MOVER_WGSL: &str = include_str!("../shaders/kinematic_brush.wgsl");
const SKINNED_WGSL: &str = include_str!("../shaders/skinned_mesh.wgsl");

/// `src` with every `//` comment removed, then every whitespace character.
fn squashed(src: &str) -> String {
    src.lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<String>()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// Every source the floor layer's blur reads or writes through, code only:
/// the chain's downsample shader, the targets it builds its groups over, and
/// its recording; the blur at the march's hit, the march's hit and read, and
/// the march's recording. One gate covers the layer. Names a scan may skip
/// are stripped first: the blur width's probe-term exponent → mip function
/// and its constants (the width reads the exponent and the minimum blur, as
/// B1 allows), and the world vertex stream of probe indices the march's
/// vertex stage culls by.
fn floor_layer_blur_sources() -> Vec<(&'static str, String)> {
    let march = include_str!("../shaders/glossy_floor_march.wgsl");
    let strip = |code: String| {
        [
            "reflection_probe_exponent_mip",
            "reflection_probe_mip0_texel_angle",
            "glossy_floor_probe_mips",
            "world_probes",
        ]
        .into_iter()
        .fold(code, |code, name| {
            code.replace(name, "").replace(&name.to_uppercase(), "")
        })
    };
    let sources = vec![
        (
            "scene_colour_chain.wgsl",
            squashed(SCENE_COLOUR_CHAIN_SHADER),
        ),
        (
            "glossy_floor/targets.rs build",
            squashed(wgsl_function(
                include_str!("glossy_floor/targets.rs"),
                "build",
            )),
        ),
        (
            "glossy_floor/chain.rs",
            squashed(include_str!("glossy_floor/chain.rs")),
        ),
        (
            "renderer_glossy_floor_frame.rs record_glossy_scene_colour_chain",
            squashed(wgsl_function(
                include_str!("renderer_glossy_floor_frame.rs"),
                "record_glossy_scene_colour_chain",
            )),
        ),
        (
            "glossy_floor_blur.wgsl",
            squashed(include_str!("../shaders/glossy_floor_blur.wgsl")),
        ),
        (
            "glossy_floor_march.wgsl glossy_floor_hit",
            squashed(wgsl_function(march, "glossy_floor_hit")),
        ),
        (
            "glossy_floor_march.wgsl glossy_floor_hit_colour",
            squashed(wgsl_function(march, "glossy_floor_hit_colour")),
        ),
        (
            "glossy_floor/march.rs record_march",
            squashed(wgsl_item(
                include_str!("glossy_floor/march.rs"),
                "fn record_march<'a>(",
            )),
        ),
        (
            "renderer_glossy_floor_frame.rs record_glossy_floor_march",
            squashed(wgsl_function(
                include_str!("renderer_glossy_floor_frame.rs"),
                "record_glossy_floor_march",
            )),
        ),
    ];
    sources
        .into_iter()
        .map(|(file, code)| (file, strip(code)))
        .collect()
}

/// The words the blur at the hit may read beyond the chain's: the texel's
/// exponent, which sets its width (B1).
const BLUR_AT_THE_HIT: [&str; 3] = [
    "glossy_floor_blur.wgsl",
    "glossy_floor_march.wgsl glossy_floor_hit_colour",
    "glossy_floor/march.rs record_march",
];

// Acceptance row 44, fences B2 and B4 (`ip-check.md` §7), B1's inputs. No
// target the blur reads or writes holds the probe term: the chain's one
// source is the highlight-free colour, which every opaque pass writes before
// the overlay adds a probe term to scene colour, and neither the downsample,
// the blur at the hit nor the march's recording names a probe array, a
// capture-shadow quantity, scene colour or D_static (the march's bindings:
// `the_floor_march_is_waters_and_binds_no_probe_quantity`; its read at the
// hit's own screen position, B3:
// `the_floor_blur_reads_exponent_minimum_blur_and_hit_length_alone`). The filter is isotropic: four taps at fixed quarter-texel offsets
// on each axis, from the destination texel's own size alone, equal weights,
// no weight or offset from a neighbour's exponent or normal.
#[test]
fn the_floor_layers_blur_reads_no_probe_term_and_filters_isotropically() {
    let forbidden = [
        "probe",
        "capture",
        "shadow",
        "distance",
        "sums",
        "exponent",
        "smoothness",
        "normal",
        "lobe",
        "noise",
        "static_diffuse",
        "scene_color",
        "scene_colour_view",
        "overlay_output",
    ];
    for (file, code) in floor_layer_blur_sources() {
        let lower = code.to_lowercase();
        for word in forbidden {
            if word == "exponent" && BLUR_AT_THE_HIT.contains(&file) {
                continue;
            }
            assert!(!lower.contains(word), "{file} names `{word}`");
        }
    }

    // The downsample's whole interface: one sampled level and a sampler.
    let module = naga::front::wgsl::parse_str(SCENE_COLOUR_CHAIN_SHADER).expect("chain WGSL");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("chain WGSL validates");
    let mut bindings: Vec<_> = module
        .global_variables
        .iter()
        .filter_map(|(_, global)| {
            let binding = global.binding.as_ref()?;
            Some((binding.group, binding.binding, global.name.clone()?))
        })
        .collect();
    bindings.sort();
    assert_eq!(
        bindings,
        [
            (0, 0, "chain_source".to_owned()),
            (0, 1, "chain_sampler".to_owned()),
        ]
    );
    assert_eq!(
        squashed(wgsl_function(
            SCENE_COLOUR_CHAIN_SHADER,
            "fs_chain_downsample"
        )),
        "fnfs_chain_downsample(in:ChainVertex)->@location(0)vec4<f32>{\
         letquarter=0.25*vec2<f32>(dpdx(in.uv).x,dpdy(in.uv).y);\
         letsum=textureSampleLevel(chain_source,chain_sampler,in.uv+vec2<f32>(-quarter.x,-quarter.y),0.0)\
         +textureSampleLevel(chain_source,chain_sampler,in.uv+vec2<f32>(quarter.x,-quarter.y),0.0)\
         +textureSampleLevel(chain_source,chain_sampler,in.uv+vec2<f32>(-quarter.x,quarter.y),0.0)\
         +textureSampleLevel(chain_source,chain_sampler,in.uv+vec2<f32>(quarter.x,quarter.y),0.0);\
         returnvec4<f32>(sum.rgb*0.25,1.0);}",
        "four equal taps at the destination texel's quarter offsets"
    );

    // Level 0 reads the highlight-free colour, every other level the one
    // above; each level is cleared and written whole every recorded frame.
    let build = squashed(wgsl_function(
        include_str!("glossy_floor/targets.rs"),
        "build",
    ));
    assert!(build.contains(
        "letsource=matchlevels.last(){Some(above)=>&above.target,None=>&highlight_free_view,};"
    ));
    let chain = squashed(include_str!("glossy_floor/chain.rs"));
    assert!(chain.contains("for(target,source)intargets.levels(){"));
    assert!(chain.contains("load:wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),"));
    assert!(chain.contains("pass.set_bind_group(0,source,&[]);pass.draw(0..3,0..1);"));
    assert_eq!(chain.matches("set_bind_group(").count(), 1);
}

// Row 53's inventory, extended to the chain: the downsample's bindings name
// no history, reprojection or intersection resource, and the highlight-free
// colour and every chain level are cleared each frame they are written (by
// forward and the chain), so no texel survives a frame.
#[test]
fn the_floor_layers_sources_hold_no_history() {
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
    for (file, code) in floor_layer_blur_sources() {
        let lower = code.to_lowercase();
        for word in forbidden {
            assert!(!lower.contains(word), "{file} names `{word}`");
        }
    }
    let forward = squashed(include_str!("renderer_forward_frame.rs"));
    assert!(forward.contains(
        "lethighlight_free_attachment=highlight_free.map(|view|wgpu::RenderPassColorAttachment{\
         view,depth_slice:None,resolve_target:None,ops:wgpu::Operations{\
         load:wgpu::LoadOp::Clear(clear_color.into()),store:wgpu::StoreOp::Store,},});"
    ));
}

// Acceptance rows 41 and 51: forward's highlight-free entries return the very
// colour `forward_shade` computes, as `fs_main` does, and beside it that
// colour less the highlight forward adds (the untinted specular times albedo
// plus the tinted specular), taken after the colour's own expression, which
// is unchanged.
#[test]
fn forward_writes_its_colour_and_leaves_its_highlight_out() {
    assert_eq!(
        squashed(wgsl_function(FORWARD_WGSL, "fs_main")),
        "fnfs_main(in:VertexOutput)->@location(0)vec4<f32>{returnforward_shade(in).colour;}"
    );
    assert_eq!(
        squashed(wgsl_function(FORWARD_WGSL, "fs_main_highlight_free")),
        "fnfs_main_highlight_free(in:VertexOutput)->ForwardHighlightFreeOutput{\
         letshaded=forward_shade(in);\
         returnForwardHighlightFreeOutput(shaded.colour,forward_highlight_free(shaded));}"
    );
    assert_eq!(
        squashed(wgsl_function(
            FORWARD_WGSL,
            "fs_main_static_diffuse_highlight_free"
        )),
        "fnfs_main_static_diffuse_highlight_free(in:VertexOutput)\
         ->ForwardStaticDiffuseHighlightFreeOutput{letshaded=forward_shade(in);\
         returnForwardStaticDiffuseHighlightFreeOutput(shaded.colour,\
         vec4<f32>(shaded.static_diffuse,0.0),forward_highlight_free(shaded),);}"
    );
    assert_eq!(
        squashed(wgsl_function(FORWARD_WGSL, "forward_highlight_free")),
        "fnforward_highlight_free(shaded:ForwardShaded)->vec4<f32>{\
         returnvec4<f32>(max(shaded.colour.rgb-shaded.highlight,vec3<f32>(0.0)),shaded.colour.a);}"
    );
    let shade = squashed(wgsl_function(FORWARD_WGSL, "forward_shade"));
    let folded = shade
        .find("total_light=total_light+specular_sum;")
        .expect("the untinted highlight folds into the light");
    let highlight = shade
        .find("lethighlight=base_color.rgb*specular_sum+tinted_specular;")
        .expect("the highlight");
    let colour = shade
        .find("letrgb=base_color.rgb*total_light+emissive+tinted_specular;")
        .expect("the colour's expression, unchanged");
    assert!(folded < highlight && highlight < colour);
    for sum in ["specular_sum=", "tinted_specular="] {
        assert!(
            shade.rfind(sum).expect("assigned") < folded,
            "nothing changes {sum} once the highlight is taken"
        );
    }
    // Debug views leave no highlight; the shaded colour carries its own.
    assert_eq!(shade.matches(",static_diffuse,vec3<f32>(0.0));").count(), 5);
    assert!(
        shade.ends_with(
            "returnForwardShaded(vec4<f32>(rgb,base_color.a),static_diffuse,highlight);}"
        )
    );
}

// Acceptance rows 41 and 42: the mover's highlight-free value is its colour
// less exactly the highlight that colour adds (pixel: albedo times the
// untinted sum plus the tinted sum's tint; smooth: the one specular sum
// times its colour), and the skinned mesh, which draws none, writes its
// colour as the highlight-free colour.
#[test]
fn movers_leave_their_highlight_out_and_skinned_meshes_write_their_colour() {
    let pixel = squashed(wgsl_function(MOVER_WGSL, "mover_pixel_fragment"));
    assert!(pixel.contains(
        "lethighlight=base_color.rgb*dynamic.untinted_specular\
         +dynamic.tinted_specular*specular_tint(base_color.rgb,s.spec_texel.tint);"
    ));
    let smooth = squashed(wgsl_function(MOVER_WGSL, "mover_smooth_fragment"));
    assert!(smooth.contains("dynamic.specular*spec_color,);"));
    assert_eq!(
        squashed(wgsl_function(MOVER_WGSL, "mover_smooth_shade"))
            .matches("dynamic.specular*spec_color")
            .count(),
        1,
        "the colour adds the same specular term"
    );
    assert_eq!(
        squashed(wgsl_function(MOVER_WGSL, "mover_highlight_free")),
        "fnmover_highlight_free(shaded:MoverShaded)->vec4<f32>{\
         returnvec4<f32>(max(shaded.colour.rgb-shaded.highlight,vec3<f32>(0.0)),shaded.colour.a);}"
    );
    assert_eq!(
        squashed(wgsl_function(SKINNED_WGSL, "fs_main_highlight_free")),
        "fnfs_main_highlight_free(in:VertexOutput)->SkinnedHighlightFreeOutput{\
         letcolour=skinned_shade(in);returnSkinnedHighlightFreeOutput(colour,colour);}"
    );
    assert_eq!(
        squashed(wgsl_function(SKINNED_WGSL, "fs_main")),
        "fnfs_main(in:VertexOutput)->@location(0)vec4<f32>{returnskinned_shade(in);}"
    );
}

// Acceptance row 43, pin P28 (source half): every forward, mover and
// skinned-mesh form, with and without the highlight-free output, and the
// chain's downsample, is built at full init: each constructor is reached
// from one full-init site alone, so a tier change can build none. The
// planar mirror's world forward binds the one-output form, its movers and
// meshes their colour form. The GPU half counts the pipelines a tier change
// builds (`glossy_floor_frame_test.rs`).
#[test]
fn every_highlight_free_form_builds_at_full_init() {
    let init = squashed(include_str!("renderer_init_pipelines.rs"));
    assert_eq!(
        init.matches("ForwardStaticDiffusePipelines::new(").count(),
        1
    );
    let overlay = squashed(include_str!("glossy_overlay.rs"));
    assert_eq!(overlay.matches("GlossyFloorPass::new(").count(), 1);
    assert!(overlay.contains("pub(crate)fnnew("));
    let full_init = squashed(include_str!("renderer_full_init.rs"));
    for constructor in [
        "GlossyOverlayPass::new(",
        "KinematicBrushPass::new(",
        "MeshPass::new(",
    ] {
        assert_eq!(full_init.matches(constructor).count(), 1, "{constructor}");
    }
    let forms = squashed(include_str!("forward_pipelines.rs"));
    for entry in [
        "fs_main_static_diffuse_highlight_free",
        "fs_main_highlight_free",
    ] {
        assert_eq!(forms.matches(&format!("\"{entry}\"")).count(), 1, "{entry}");
    }
    let mesh = squashed(include_str!("mesh_pass/pipeline.rs"));
    assert!(mesh.contains("\"fs_main_highlight_free\","));
    let mirror = squashed(include_str!("planar_reflection/record.rs"));
    assert!(mirror.contains("ColourOutputs::Colour,full.frame_specular_style,"));
    assert!(!mirror.contains("HighlightFree"));
    assert!(!mirror.contains("highlight_free"));
}

#[test]
fn the_chain_runs_to_one_texel_along_its_longer_axis() {
    use super::targets::chain_mip_count;
    assert_eq!(chain_mip_count(1, 1), 1);
    assert_eq!(chain_mip_count(2, 1), 2);
    assert_eq!(chain_mip_count(16, 16), 5);
    assert_eq!(chain_mip_count(24, 20), 5);
    assert_eq!(chain_mip_count(960, 540), 10);
    assert_eq!(chain_mip_count(1920, 1080), 11);
}

#[test]
fn the_highlight_free_colour_takes_four_bytes_where_the_adapter_renders_them() {
    assert_eq!(
        highlight_free_format(wgpu::Features::RG11B10UFLOAT_RENDERABLE),
        wgpu::TextureFormat::Rg11b10Ufloat
    );
    assert_eq!(
        highlight_free_format(wgpu::Features::empty()),
        SCENE_COLOR_FORMAT
    );
    assert_eq!(ColourOutputs::of(true), ColourOutputs::HighlightFree);
    assert_eq!(ColourOutputs::of(false), ColourOutputs::Colour);
}
