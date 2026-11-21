// Contract tests for the mover pass: WGSL shape and composition pins, the
// group-2 layout and params row, material ranges and the mirror cull.
// See: context/lib/rendering_pipeline.md §7.3 · context/lib/testing_guide.md

use super::*;
use postretro_level_format::geometry::{FaceMeta, Vertex};

// `abs` replaced `fwidth` here: the seam width now comes from the
// pre-march ddx/ddy the caller already passes, because under Surface Depth
// `uv` is the marched UV and its fwidth measures the parallax step rather
// than the pixel footprint. Tracking the call that actually runs is what
// keeps this fingerprint a real parity check.
const POST_RETRO_SAMPLING_CALLS: &[&str] = &[
    "textureDimensions",
    "floor",
    "max",
    "abs",
    "clamp",
    "textureSampleGrad",
];

fn extract_wgsl_fn<'a>(source: &'a str, name: &str) -> &'a str {
    let needle = format!("fn {name}");
    let start = source
        .find(&needle)
        .unwrap_or_else(|| panic!("shader should declare fn {name}"));
    let body_start = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("fn {name} should have a body"));
    let mut depth = 0i32;
    for (offset, ch) in source[body_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..body_start + offset + 1];
                }
            }
            _ => {}
        }
    }
    panic!("fn {name} should close its body");
}

fn wgsl_call_fingerprint(source: &str, keep: &[&str]) -> Vec<String> {
    let source = strip_wgsl_comments(source);
    let mut calls = Vec::new();
    for (start, ident) in wgsl_identifiers(&source) {
        let after_ident = &source[start + ident.len()..];
        if !after_ident.trim_start().starts_with('(') {
            continue;
        }
        if keep.contains(&ident) {
            calls.push(ident.to_owned());
        }
    }
    calls
}

fn strip_wgsl_comments(source: &str) -> String {
    let mut stripped = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '/' {
            stripped.push(ch);
            continue;
        }
        match chars.peek() {
            Some('/') => {
                chars.next();
                for comment_ch in chars.by_ref() {
                    if comment_ch == '\n' {
                        stripped.push('\n');
                        break;
                    }
                }
            }
            Some('*') => {
                chars.next();
                let mut prev = '\0';
                for comment_ch in chars.by_ref() {
                    if comment_ch == '\n' {
                        stripped.push('\n');
                    }
                    if prev == '*' && comment_ch == '/' {
                        break;
                    }
                    prev = comment_ch;
                }
            }
            _ => stripped.push(ch),
        }
    }
    stripped
}

fn wgsl_identifiers(source: &str) -> Vec<(usize, &str)> {
    let mut idents = Vec::new();
    let mut iter = source.char_indices().peekable();
    while let Some((start, ch)) = iter.next() {
        if !is_wgsl_ident_start(ch) {
            continue;
        }
        let mut end = start + ch.len_utf8();
        while let Some(&(next, next_ch)) = iter.peek() {
            if !is_wgsl_ident_continue(next_ch) {
                break;
            }
            iter.next();
            end = next + next_ch.len_utf8();
        }
        idents.push((start, &source[start..end]));
    }
    idents
}

fn is_wgsl_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_wgsl_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn vertex(lightmap_uv: [f32; 2], lightmap_block: u16) -> Vertex {
    Vertex::new(
        [1.0, 2.0, 3.0],
        [4.0, 5.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        lightmap_uv,
        lightmap_block,
    )
}

#[test]
fn pack_vertex_consumes_zeroed_lightmap_fields_without_rewriting_them() {
    let packed = pack_vertex(&vertex([0.0, 0.0], 0));
    assert_eq!(packed.position, [1.0, 2.0, 3.0]);
    assert_eq!(packed.base_uv, [4.0, 5.0]);
    assert_eq!(packed.lightmap_uv, [0, 0]);
    assert_eq!(packed.lightmap_block, 0);
}

// The ghost sort's CPU copy of a mover's triangles: each index triple
// becomes its three local positions, and one indexing past the mover's
// vertices is dropped rather than panicking.
#[test]
fn mover_triangle_positions_follow_the_indices_and_drop_one_past_the_vertices() {
    let corner = |position: [f32; 3]| {
        Vertex::new(
            position,
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        )
    };
    let vertices = [
        corner([0.0, 0.0, 0.0]),
        corner([1.0, 0.0, 0.0]),
        corner([0.0, 0.0, 1.0]),
    ];
    assert_eq!(
        triangle_positions(&vertices, &[2, 0, 1]),
        Some([glam::Vec3::Z, glam::Vec3::ZERO, glam::Vec3::X])
    );
    assert_eq!(triangle_positions(&vertices, &[0, 1, 3]), None);
}

#[test]
fn kinematic_brush_shader_matches_forward_post_retro_sampling() {
    let forward = include_str!("../shaders/forward.wgsl");
    let kinematic = include_str!("../shaders/kinematic_brush.wgsl");

    // Both copies reconstruct through the shared `post_retro_texel_uv`, so
    // the call lists compare directly; the reconstruction itself is one
    // definition (`forward_and_mover_share_the_specular_texel_functions`).
    let calls = |source| {
        wgsl_call_fingerprint(
            extract_wgsl_fn(source, "sample_post_retro"),
            &[POST_RETRO_SAMPLING_CALLS, &["post_retro_texel_uv"]].concat(),
        )
    };
    assert_eq!(
        calls(kinematic),
        calls(forward),
        "kinematic brush movers must keep the same Post Retro sampling operations as static world geometry",
    );
    assert_eq!(
        calls(forward),
        [
            "textureDimensions",
            "post_retro_texel_uv",
            "textureSampleGrad"
        ],
        "the sample reconstructs through the shared function, then samples with the original derivatives",
    );

    // Both entries share one prelude: it takes the UV derivatives before its
    // first sample, and each entry samples only after it, with its derivatives.
    let surface_calls = wgsl_call_fingerprint(
        extract_wgsl_fn(kinematic, "mover_surface"),
        &["dpdx", "dpdy", "sample_post_retro", "textureSample"],
    );
    let ddx_index = surface_calls
        .iter()
        .position(|call| call == "dpdx")
        .expect("the mover prelude must compute a UV ddx");
    let ddy_index = surface_calls
        .iter()
        .position(|call| call == "dpdy")
        .expect("the mover prelude must compute a UV ddy");
    let sample_index = surface_calls
        .iter()
        .position(|call| call == "sample_post_retro")
        .expect("the mover prelude samples through the Post Retro helper");
    assert!(
        ddx_index < sample_index && ddy_index < sample_index,
        "the mover prelude must compute UV derivatives before sampling",
    );
    for entry in ["fs_pixel", "fs_smooth"] {
        let calls = wgsl_call_fingerprint(
            extract_wgsl_fn(kinematic, entry),
            &[
                "mover_surface",
                "dpdx",
                "dpdy",
                "sample_post_retro",
                "textureSample",
            ],
        );
        assert_eq!(
            calls.first().map(String::as_str),
            Some("mover_surface"),
            "{entry} takes the prelude's derivatives before any sample: {calls:?}"
        );
        assert!(
            calls.iter().any(|call| call == "sample_post_retro"),
            "{entry} samples its albedo through the Post Retro helper"
        );
    }
    for function in ["mover_surface", "fs_pixel", "fs_smooth", "mover_emissive"] {
        assert!(
            !wgsl_call_fingerprint(extract_wgsl_fn(kinematic, function), &["textureSample"])
                .iter()
                .any(|call| call == "textureSample"),
            "{function} must not bypass the Post Retro helper",
        );
    }
}

// The texel-centre functions live once, in `material_shading.wgsl`, and
// both world-material pipelines compose that one copy: the mover
// reconstructs its texture reads and its pixel-style specular point with
// forward's own functions, so the two cannot drift (rendering_pipeline.md
// §4, Texel-centre highlight; §8).
#[test]
fn forward_and_mover_share_the_specular_texel_functions() {
    let material_shading = include_str!("../shaders/material_shading.wgsl");
    let squashed = |source: &str, name: &str| -> String {
        strip_wgsl_comments(extract_wgsl_fn(source, name))
            .split_whitespace()
            .collect()
    };
    for (label, consumer, composed) in [
        (
            "forward",
            include_str!("../shaders/forward.wgsl"),
            crate::render::pipeline_layout::SHADER_SOURCE,
        ),
        (
            "mover",
            include_str!("../shaders/kinematic_brush.wgsl"),
            SHADER_SOURCE,
        ),
    ] {
        for declaration in [
            "fn post_retro_texel_uv(",
            "fn specular_texel_point(",
            "struct SpecularPoint {",
            "const SPECULAR_POINT_AXES_EPSILON:",
            "fn specular_texel(",
            "fn specular_texel_exponent(",
            "fn specular_tint(",
            "struct SpecularTexel {",
            "const SPECULAR_ROUGHNESS4_FLOOR:",
            "const SPECULAR_SMOOTHNESS_MARKER:",
            "const SPECULAR_TINT_MARKER:",
        ] {
            assert!(
                material_shading.contains(declaration),
                "{declaration} lives in material_shading.wgsl"
            );
            assert!(
                !consumer.contains(declaration),
                "{label} must not redeclare {declaration}"
            );
            assert_eq!(
                composed.matches(declaration).count(),
                1,
                "{label} composes {declaration} once"
            );
        }
        assert!(
            squashed(consumer, "sample_post_retro")
                .contains("letuv_recon=post_retro_texel_uv(uv,dims,ddx,ddy);"),
            "{label}'s texture reads reconstruct through the shared function"
        );
    }

    // Each pass builds its point through the shared builder, from the
    // fragment's own UV, the marched UV, the albedo lattice and the
    // fragment's hoisted derivatives.
    let forward_fs = squashed(include_str!("../shaders/forward.wgsl"), "fs_main");
    assert_eq!(forward_fs.matches("specular_texel_point(").count(), 1);
    let mover = include_str!("../shaders/kinematic_brush.wgsl");
    let surface = squashed(mover, "mover_surface");
    let pixel_fs = squashed(mover, "fs_pixel");
    let smooth_fs = squashed(mover, "fs_smooth");
    for fs in [&surface, &pixel_fs, &smooth_fs] {
        assert!(!fs.contains("specular_texel_point("));
    }
    assert!(pixel_fs.contains(
        "letspec_point=mover_specular_point(s.depth,in.uv,in.world_position,\
         s.mesh_n,s.view,camera.camera_position,\
         vec2<f32>(textureDimensions(base_texture,0)),s.ddx,s.ddy,s.ddx_world,s.ddy_world,);"
    ));
    assert!(
        !smooth_fs.contains("mover_specular_point("),
        "smooth evaluates every highlight input per fragment"
    );
    let mover_point = squashed(mover, "mover_specular_point");
    assert_eq!(mover_point.matches("specular_texel_point(").count(), 1);
    assert!(
        mover_point.contains(
            "returnspecular_texel_point(false,\
             SpecularPoint(depth.world_position,view_to_eye),uv,depth.uv,\
             world_position+geo_normal*top_height_m,eye,albedo_dims,ddx,ddy,\
             ddx_world,ddy_world,);"
        ),
        "the mover anchors on the fragment's own UV and plane position, raised by the \
         struck texel's height, and reconstructs the marched UV"
    );
    // `depth.uv` is the mover's marched UV, the one its texture reads use.
    assert!(surface.contains("letshade_uv=depth.uv;"));
    for fs in [&pixel_fs, &smooth_fs] {
        assert!(
            fs.contains("sample_post_retro(base_texture,aniso_sampler,s.depth.uv,s.ddx,s.ddy)")
        );
        assert!(fs.contains("mover_emissive(s.depth.uv,s.ddx,s.ddy)"));
    }

    // Sharpness and tint: each pass reads the slot once and decodes it
    // through the shared functions, so the mover reads them exactly as
    // forward does.
    let forward_slot_read = "letspec_texel=specular_texel(sample_color(spec_texture,\
                             shade_uv,ddx,ddy),max(material.shininess,1.0),);";
    let mover_slot_read = "letspec_texel=specular_texel(sample_post_retro(spec_texture,\
                           aniso_sampler,shade_uv,ddx,ddy),max(material.shininess,1.0),);";
    assert!(forward_fs.contains(forward_slot_read));
    assert!(surface.contains(mover_slot_read));
    assert_eq!(
        forward_fs.matches("spec_texture").count(),
        1,
        "one read of the slot"
    );
    assert_eq!(
        surface.matches("spec_texture").count(),
        1,
        "one read of the slot"
    );
    for fs in [&pixel_fs, &smooth_fs] {
        assert!(
            !fs.contains("spec_texture"),
            "the prelude's read is the only one"
        );
    }
    assert_eq!(
        forward_fs
            .matches("specular_tint(base_color.rgb,spec_texel.tint)")
            .count(),
        1
    );
    assert!(pixel_fs.contains("s.spec_texel.exponent,s.spec_texel.intensity,s.spec_texel.tinted,"));
    assert!(pixel_fs.contains(
        "returnvec4<f32>(mover_pixel_shade(base_color.rgb,s.lighting,dynamic,emissive,\
         s.spec_texel),base_color.a,);"
    ));
    assert!(smooth_fs.contains(
        "returnvec4<f32>(mover_smooth_shade(base_color.rgb,s.lighting,dynamic,emissive,\
         s.spec_texel),base_color.a,);"
    ));
    // Pixel mixes the tint into its tinted sum alone, the tinted term last,
    // so an untinted fragment keeps its pre-tint expression; smooth gives
    // its one specular sum the albedo or the tint colour.
    assert_eq!(
        squashed(mover, "mover_pixel_shade"),
        "fnmover_pixel_shade(base_color:vec3<f32>,lighting:vec3<f32>,dynamic:MoverPixelDirect,\
         emissive:vec3<f32>,spec_texel:SpecularTexel,)->vec3<f32>{letlit=lighting+dynamic.lit;\
         lettinted_specular=dynamic.tinted_specular*specular_tint(base_color,spec_texel.tint);\
         returnbase_color*lit+emissive+tinted_specular;}"
    );
    assert_eq!(
        squashed(mover, "mover_smooth_shade"),
        "fnmover_smooth_shade(base_color:vec3<f32>,lighting:vec3<f32>,dynamic:MoverSmoothDirect,\
         emissive:vec3<f32>,spec_texel:SpecularTexel,)->vec3<f32>{letlit=lighting+dynamic.diffuse;\
         letspec_color=select(base_color,specular_tint(base_color,spec_texel.tint),\
         spec_texel.tinted,);returnbase_color*lit+emissive+dynamic.specular*spec_color;}"
    );
    assert!(forward_fs.contains(
        "specular_texel_point(specular_style_smooth(),SpecularPoint(in.world_position,V),\
         in.uv,shade_uv,"
    ));

    // The reconstruction's seam width stays on the passed derivatives.
    assert_eq!(
        wgsl_call_fingerprint(
            extract_wgsl_fn(material_shading, "post_retro_texel_uv"),
            POST_RETRO_SAMPLING_CALLS,
        ),
        ["floor", "max", "abs", "abs", "clamp"],
    );
}

#[test]
fn material_ranges_preserve_face_texture_order_and_index_spans() {
    let indices = [0, 1, 2, 0, 2, 3, 4, 5, 6];
    let face_meta = [
        FaceMeta {
            leaf_index: 0,
            texture_index: 2,
        },
        FaceMeta {
            leaf_index: 0,
            texture_index: 1,
        },
    ];
    let ranges = derive_material_ranges(&indices, &face_meta, 4);
    assert_eq!(
        ranges,
        vec![
            MaterialRange {
                material_index: 2,
                index_start: 0,
                index_count: 6,
            },
            MaterialRange {
                material_index: 1,
                index_start: 6,
                index_count: 3,
            },
        ],
    );
}

#[test]
fn material_ranges_merge_adjacent_faces_with_same_material() {
    let indices = [0, 1, 2, 3, 4, 5];
    let face_meta = [
        FaceMeta {
            leaf_index: 0,
            texture_index: 1,
        },
        FaceMeta {
            leaf_index: 0,
            texture_index: 1,
        },
    ];
    assert_eq!(
        derive_material_ranges(&indices, &face_meta, 2),
        vec![MaterialRange {
            material_index: 1,
            index_start: 0,
            index_count: 6,
        }],
    );
}

#[test]
fn material_range_uses_placeholder_for_missing_or_out_of_range_texture() {
    assert_eq!(
        material_index(postretro_level_format::geometry::NO_TEXTURE, 3),
        0
    );
    assert_eq!(material_index(9, 3), 0);
    assert_eq!(material_index(2, 3), 2);
}

#[test]
fn kinematic_pass_uses_existing_bind_group_budget() {
    assert_eq!(KINEMATIC_BIND_GROUP_COUNT, 5);
    const { assert!(KINEMATIC_BIND_GROUP_COUNT <= 8) };
}

#[test]
fn kinematic_pipeline_fragment_texture_budget_includes_emissive() {
    let total = |cube_array_supported| {
        let per_group = [
            fragment_sampled_textures(&uniform_bind_group_layout_entries()),
            fragment_sampled_textures(&material_bind_group_layout_entries()),
            fragment_sampled_textures(&light_bind_group_layout_entries(cube_array_supported)),
            0, // group 3 instance storage buffer
            // Group 4 is the mesh superset full init builds this pass
            // against: total atlas, depth moments, depth tiles and the
            // fragment-visible direct atlas.
            fragment_sampled_textures(&sh_volume::mesh_bind_group_layout_entries()),
        ];
        (per_group, per_group.iter().sum::<u32>())
    };

    let (cube_groups, cube_total) = total(true);
    assert_eq!(cube_groups, [0, 4, 4, 0, 4]);
    assert_eq!(cube_total, 12);
    assert!(cube_total <= 16);

    let (no_cube_groups, no_cube_total) = total(false);
    assert_eq!(no_cube_groups, [0, 4, 2, 0, 4]);
    assert_eq!(no_cube_total, 10);
    assert!(no_cube_total <= 16);
}

// Mover fragments weigh probes with the smooth, floored backface form on
// both SH reads: a ramp of the probe's signed distance from the surface
// point, 0.4 cell wide, with the lookup lifted 0.4 cell, and that form
// never weighs a probe 0 (ip-372 levers 3 and 5; owner Ruling A,
// mover-light-flow--mid-travel-poses Amendment 2). The form's values are
// proven on the GPU in `backface_tests`.
#[test]
fn mover_sh_reads_take_the_smooth_floored_backface_weight() {
    let source = composed_shader_source();
    let smooth_calls = source
        .matches(", SH_BACKFACE_SMOOTH, sh_grid.probe_occlusion")
        .count();
    assert_eq!(smooth_calls, 2, "direct and indirect SH reads");
    for other in ["SH_BACKFACE_HARD, sh_grid", "SH_BACKFACE_NONE, sh_grid"] {
        assert!(!source.contains(other), "mover pass passes {other}");
    }
    let floor = source
        .lines()
        .find_map(|line| line.strip_prefix("const SH_BACKFACE_SMOOTH_FLOOR: f32 = "))
        .and_then(|rest| rest.trim_end_matches(';').parse::<f32>().ok())
        .expect("smooth backface floor constant");
    assert!(floor > 0.0, "the smooth form's floor stays non-zero");
    assert_eq!(floor, 0.03, "the floor is the depth weight's 0.03");
    for line in [
        "const SH_MOVER_LOOKUP_OFFSET_CELLS: f32 = 0.4;",
        "const SH_MOVER_BACKFACE_RAMP_CELLS: f32 = SH_MOVER_LOOKUP_OFFSET_CELLS;",
        "    const SH_NORMAL_OFFSET_M: f32 = SH_MOVER_LOOKUP_OFFSET_CELLS;",
        "return r * r * (3.0 - 2.0 * r) + SH_BACKFACE_SMOOTH_FLOOR;",
    ] {
        assert!(source.contains(line), "the ruled smooth form: {line}");
    }
    assert_eq!(
        source
            .matches("sh_receiver_lookup_lift = dot(offset_world - world_pos, geo_normal);")
            .count(),
        2,
        "both SH reads record their lookup's lift before reading"
    );
}

#[test]
fn kinematic_material_uniform_mirrors_emissive_layout() {
    let shader = include_str!("../shaders/kinematic_brush.wgsl");
    assert!(shader.contains("shininess: f32,"));
    assert!(shader.contains("emissive_strength: f32,"));
    assert!(shader.contains("emissive_texture"));
    assert!(shader.contains("return emissive * material.emissive_strength;"));
    assert_eq!(
        shader.matches("base_color * lit + emissive + ").count(),
        2,
        "both compositions add the glow after the lit albedo"
    );
}

#[test]
fn kinematic_light_layout_keeps_storage_fragment_only() {
    let entries = light_bind_group_layout_entries(true);
    let storage_count = entries
        .iter()
        .filter(|entry| {
            entry.visibility.contains(wgpu::ShaderStages::FRAGMENT)
                && matches!(
                    entry.ty,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { .. },
                        ..
                    }
                )
        })
        .count();
    assert_eq!(storage_count, 4);
    assert!(
        entries
            .iter()
            .all(|entry| !entry.visibility.contains(wgpu::ShaderStages::VERTEX)),
        "runtime-light bindings must not spend vertex-stage storage budget",
    );
}

#[test]
fn kinematic_light_layout_requires_32_byte_params_uniform() {
    let entries = light_bind_group_layout_entries(true);
    let params_entry = entries
        .iter()
        .find(|entry| entry.binding == 4)
        .expect("kinematic light layout must bind params at binding 4");
    let wgpu::BindingType::Buffer {
        ty,
        has_dynamic_offset,
        min_binding_size,
    } = params_entry.ty
    else {
        panic!("kinematic light binding 4 must be a uniform buffer");
    };

    assert_eq!(ty, wgpu::BufferBindingType::Uniform);
    assert!(!has_dynamic_offset);
    assert_eq!(
        min_binding_size.map(std::num::NonZeroU64::get),
        Some(KINEMATIC_LIGHT_PARAMS_SIZE as u64),
    );
}

#[test]
fn kinematic_light_params_uploads_counts_in_second_uniform_row_and_pads_its_last_word() {
    let bytes = build_light_params_bytes(KinematicLightParams {
        light_count: 11,
        time: 1.5,
        light_term_mask: 0x7F,
        ambient_floor: 0.375,
        dynamic_light_count: 7,
        scripted_light_count: 10,
        indirect_scale: 0.625,
    });

    assert_eq!(bytes.len(), KINEMATIC_LIGHT_PARAMS_SIZE);
    assert_eq!(bytes[0..4], 11u32.to_ne_bytes());
    assert_eq!(bytes[4..8], 1.5f32.to_ne_bytes());
    assert_eq!(bytes[8..12], 0x7Fu32.to_ne_bytes());
    assert_eq!(bytes[12..16], 0.375f32.to_ne_bytes());
    assert_eq!(bytes[16..20], 7u32.to_ne_bytes());
    assert_eq!(bytes[20..24], 10u32.to_ne_bytes());
    assert_eq!(bytes[24..28], 0.625f32.to_ne_bytes());
    // The specular style picks a pipeline; no word carries it.
    assert_eq!(bytes[28..], [0; 4]);
}

#[test]
fn kinematic_light_params_wgsl_layout_matches_rust_upload() {
    let module = naga::front::wgsl::parse_str(SHADER_SOURCE)
        .expect("composed kinematic brush shader should parse as WGSL");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("composed kinematic brush shader should pass Naga validation");

    let (span, members) = module
        .types
        .iter()
        .find_map(|(_handle, ty)| match (&ty.name, &ty.inner) {
            (Some(name), naga::TypeInner::Struct { span, members, .. })
                if name == "KinematicLightParams" =>
            {
                Some((*span, members))
            }
            _ => None,
        })
        .expect("kinematic brush shader should declare KinematicLightParams");

    assert_eq!(span as usize, KINEMATIC_LIGHT_PARAMS_SIZE);
    assert_eq!(
        members
            .iter()
            .map(|member| member.name.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("light_count"),
            Some("time"),
            Some("light_term_mask"),
            Some("ambient_floor"),
            Some("dynamic_light_count"),
            Some("scripted_light_count"),
            Some("indirect_scale"),
            Some("_pad"),
        ],
    );
    assert_eq!(
        members
            .iter()
            .map(|member| member.offset)
            .collect::<Vec<_>>(),
        vec![0, 4, 8, 12, 16, 20, 24, 28],
    );
}

// The colour draw tests `Equal` against the depth `vs_depth` wrote in the
// pre-pass, so both entries must reach clip space by the same data flow
// (one helper pair, no inline `view_proj` multiply) with the position
// `@invariant`; otherwise FMA reassociation can drop mover pixels.
#[test]
fn mover_depth_and_colour_entries_share_one_invariant_clip_path() {
    let module =
        naga::front::wgsl::parse_str(SHADER_SOURCE).expect("kinematic brush shader parses");
    for entry_name in ["vs_depth", "vs_main"] {
        let entry = module
            .entry_points
            .iter()
            .find(|entry| entry.name == entry_name)
            .unwrap_or_else(|| panic!("kinematic brush shader declares {entry_name}"));
        assert_eq!(entry.stage, naga::ShaderStage::Vertex, "{entry_name}");
        let result = entry.function.result.as_ref().expect("returns a position");
        let naga::TypeInner::Struct { members, .. } = &module.types[result.ty].inner else {
            panic!("{entry_name} returns a struct");
        };
        assert!(
            members.iter().any(|member| matches!(
                member.binding,
                Some(naga::Binding::BuiltIn(naga::BuiltIn::Position {
                    invariant: true
                }))
            )),
            "{entry_name}: clip position is @invariant"
        );
        let body: String = strip_wgsl_comments(extract_wgsl_fn(SHADER_SOURCE, entry_name))
            .split_whitespace()
            .collect();
        assert_eq!(
            body.matches("out.clip_position=").count(),
            1,
            "{entry_name}: one clip-position write"
        );
        assert!(
            !body.contains("view_proj"),
            "{entry_name}: clip space only through mover_clip_position"
        );
    }
    let code = |name| -> String {
        strip_wgsl_comments(extract_wgsl_fn(SHADER_SOURCE, name))
            .split_whitespace()
            .collect()
    };
    assert!(code("vs_depth").contains(
        "out.clip_position=mover_clip_position(mover_world_position(in.position,instance_index));"
    ));
    let main = code("vs_main");
    assert!(main.contains("letworld_pos=mover_world_position(in.position,instance_index);"));
    assert!(main.contains("out.clip_position=mover_clip_position(world_pos);"));
    assert!(code("mover_clip_position").contains("returncamera.view_proj*world_pos;"));
    assert!(
        code("mover_world_position")
            .contains("returninstances[instance_index].model*vec4<f32>(position,1.0);")
    );
}

#[test]
fn kinematic_shader_source_validates_without_cube_arrays() {
    let source = shader_source(false);
    for declaration in [
        "@group(2) @binding(8) var point_shadow_cube",
        "@group(2) @binding(10) var promoted_cube_depth_cache",
    ] {
        assert!(
            !source.contains(declaration),
            "no-cube kinematic source must strip cube binding: {declaration}",
        );
    }
    let module = naga::front::wgsl::parse_str(&source)
        .expect("no-cube kinematic brush shader should parse as WGSL");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("no-cube kinematic brush shader should pass Naga validation");
    // Validation covers every entry; both beauty entries are among them.
    for style in SpecularStyle::ALL {
        let entry = beauty_fragment_entry(style);
        assert!(
            module
                .entry_points
                .iter()
                .any(|e| e.name == entry && e.stage == naga::ShaderStage::Fragment),
            "{style:?}: the no-cube source declares fragment entry {entry}"
        );
    }
}

#[test]
fn kinematic_light_bgl_matches_both_cube_variants() {
    let no_cube_bindings: Vec<u32> = light_bind_group_layout_entries(false)
        .iter()
        .map(|entry| entry.binding)
        .collect();
    assert_eq!(no_cube_bindings, vec![0, 1, 2, 3, 4, 5, 6, 7, 9]);

    let cube = light_bind_group_layout_entries(true);
    let cube_bindings: Vec<u32> = cube.iter().map(|entry| entry.binding).collect();
    assert_eq!(cube_bindings, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    assert!(matches!(
        cube[9].ty,
        wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        }
    ));
    assert!(matches!(
        cube[10].ty,
        wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::CubeArray,
            multisampled: false,
        }
    ));
}

#[test]
fn kinematic_shader_uses_shared_material_helpers_for_promoted_static_specular() {
    assert!(
        SHADER_SOURCE.contains("fn blinn_phong(")
            && SHADER_SOURCE.contains("fn sample_normal(")
            && SHADER_SOURCE.contains("fn reconstruct_tbn_normal("),
        "the composed mover shader must append the shared material shading snippet",
    );

    let squashed = |name| -> String {
        strip_wgsl_comments(extract_wgsl_fn(
            include_str!("../shaders/kinematic_brush.wgsl"),
            name,
        ))
        .split_whitespace()
        .collect()
    };
    let pixel_loop = squashed("accumulate_pixel_direct");
    let smooth_loop = squashed("accumulate_smooth_direct");
    assert!(
        pixel_loop.contains(
            "lettexel_specular=use_specular&&i>=kinematic_light_params.scripted_light_count;"
        ) && pixel_loop.contains("iftexel_specular{"),
        "only promoted records with LightTermMask specular enabled may add pixel mover specular",
    );
    let mover_src = include_str!("../shaders/kinematic_brush.wgsl");
    assert!(
        mover_src.contains("let use_dynamic = (light_terms & 0x20u) != 0u;")
            && mover_src.contains("let use_specular = (light_terms & 0x40u) != 0u;"),
        "mover dynamic and specular gates must derive independently from LightTermMask bits 5 and 6",
    );
    // Smooth: one specular sum, every promoted record's term behind the
    // front-lit gate, tinted or not.
    assert!(
        smooth_loop.contains(
            "ifuse_specular&&i>=kinematic_light_params.scripted_light_count&&n_dot_l>0.0{\
             specular=specular+blinn_phong(L,view,n,drive.color,spec_exp,spec_int)*attenuation;}"
        ),
        "smooth mover specular keeps the front-lit gate and the runtime attenuation, cone, \
         shadow, and promotion color factors",
    );
    assert!(
        !smooth_loop.contains("tinted"),
        "smooth keeps one specular sum"
    );
    // Pixel: an untinted texel adds its term into `total` right after the
    // light's diffuse, as before tint; a tinted texel's term joins its own sum.
    let split = "iftinted{tinted_total=tinted_total+term;}else{total=total+term;}";
    assert!(
        pixel_loop.contains(&format!(
            "letterm=mover_texel_specular(light,drive.color,drive.aim,spec_point,\
             n,mesh_n,depth.hit_top,shadow,spec_exp,spec_int,);{split}"
        )),
        "pixel mover specular takes the texel's point, this fragment's shadow and the \
         promotion color",
    );
    assert_eq!(pixel_loop.matches(split).count(), 1);
    assert!(
        squashed("mover_texel_specular")
            .contains("blinn_phong(L,point.view,n,color,spec_exp,spec_int)*(geometry.w*shadow)"),
        "pixel mover specular keeps the point's attenuation and cone, times the shadow",
    );
    // Both loops share every per-record helper, so their diffuse, shadows
    // and animation cannot drift apart.
    for helper in [
        "mover_light_cache_layer(i)",
        "mover_light_drive(i,light)",
        "mover_light_geometry(light,drive.aim,depth.world_position)",
        "mover_light_shadow(i,light,cache_layer,world_pos,mesh_n)",
        "mover_light_n_dot_l(depth,n,above_plane,L)",
        "mover_light_diffuse(i,depth,L,n_dot_l,above_plane,attenuation,drive.color,\
         &depth_shadow_marches,)",
    ] {
        for (name, body) in [("pixel", &pixel_loop), ("smooth", &smooth_loop)] {
            assert_eq!(
                body.matches(helper).count(),
                1,
                "{name} loop calls {helper}"
            );
        }
    }
}

#[test]
fn kinematic_animated_descriptors_cover_the_raw_animated_tail_only() {
    let mover = include_str!("../shaders/kinematic_brush.wgsl");
    let drive = extract_wgsl_fn(mover, "mover_light_drive");
    assert!(
        drive.contains("if i < kinematic_light_params.scripted_light_count {")
            && drive.contains("let scripted_desc = scripted_light_descriptors[i];"),
        "only selected-static records append after the descriptor-upload prefix and must not read stale descriptor tail bytes",
    );
    let cache_layer = extract_wgsl_fn(mover, "mover_light_cache_layer");
    let shadow = extract_wgsl_fn(mover, "mover_light_shadow");
    assert!(
        cache_layer.contains("if i >= kinematic_light_params.dynamic_light_count {")
            && cache_layer
                .contains("let promoted_index = i - kinematic_light_params.dynamic_light_count;")
            && shadow.contains("if i >= kinematic_light_params.dynamic_light_count {")
            && shadow.contains("sample_spot_shadow_with_static(")
            && shadow.contains("sample_point_shadow_with_static("),
        "raw section-45 mover tails must keep descriptors while reading their static-world cache metadata after the dynamic prefix",
    );
}

// The mirror's mover draws skip a mover whose world bounds (local bounds
// under its model transform) miss the mirror's cull frustum or lie
// wholly below its clip plane; one above the water in the reflected view
// still draws.
#[test]
fn mover_mirror_cull_skips_movers_off_the_mirror_view_or_under_the_water() {
    use glam::{Mat4, Vec3};
    use postretro_render_cpu::planar_reflection::MirrorCamera;
    let eye = Vec3::new(0.0, 2.0, 0.0);
    let view = glam::camera::rh::view::look_at_mat4(eye, Vec3::new(0.0, 0.0, -10.0), Vec3::Y);
    let proj = glam::camera::rh::proj::directx::perspective(60.0_f32.to_radians(), 1.0, 0.1, 100.0);
    let mirror = MirrorCamera::new(eye, view, proj, 0.0, None).unwrap();
    let planes = mirror.cull_planes();
    let local = Aabb {
        min: Vec3::splat(-0.5),
        max: Vec3::splat(0.5),
    };
    let reaches = |centre: Vec3| {
        mover_reaches_mirror(
            local,
            Mat4::from_translation(centre),
            &planes,
            mirror.clip_plane,
        )
    };
    assert!(reaches(Vec3::new(0.0, 1.0, -10.0)), "above the water");
    assert!(
        !reaches(Vec3::new(0.0, -1.5, -10.0)),
        "inside the frustum but wholly under the water"
    );
    assert!(!reaches(Vec3::new(50.0, 1.0, -10.0)), "off to the side");
    assert!(!reaches(Vec3::new(0.0, 1.0, 10.0)), "behind the eye");
}

/// `fn name(`'s source, matched exactly so a name that prefixes another
/// (`sample_point_shadow`, `sample_point_shadow_with_static`) finds its own.
fn wgsl_fn_exact<'a>(source: &'a str, name: &str) -> &'a str {
    let needle = format!("fn {name}(");
    let start = source
        .find(&needle)
        .unwrap_or_else(|| panic!("shader should declare fn {name}"));
    extract_wgsl_fn(&source[start..], name)
}

// FXC refuses an implicit-derivative sample inside a varying-length loop or a
// helper such a loop calls (rendering_pipeline.md §8, DX12 portability), and
// only a Windows DX12 build would catch it. Walk the smooth loop's whole call
// graph in the composed source instead.
#[test]
fn smooth_light_loop_and_its_helpers_take_no_implicit_derivative_sample() {
    let source = strip_wgsl_comments(SHADER_SOURCE);
    let mut pending = vec!["accumulate_smooth_direct".to_owned()];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(name) = pending.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let body = wgsl_fn_exact(&source, &name);
        for (start, ident) in wgsl_identifiers(body) {
            if !body[start + ident.len()..].trim_start().starts_with('(') {
                continue;
            }
            assert!(
                !matches!(
                    ident,
                    "textureSample" | "textureSampleBias" | "textureSampleCompare"
                ),
                "{name}, reached from the smooth light loop, calls {ident}"
            );
            if ident != name && source.contains(&format!("fn {ident}(")) {
                pending.push(ident.to_owned());
            }
        }
    }
    for helper in [
        "mover_light_shadow",
        "sample_spot_shadow_with_static",
        "sample_point_shadow_with_static",
        "surface_depth_light_visibility",
        "blinn_phong",
    ] {
        assert!(seen.contains(helper), "the walk reaches {helper}: {seen:?}");
    }
}

// Each beauty pipeline is built with its own style's fragment entry, and
// `record_draws` reports the very style it selects the pipeline by, so a
// report cannot name a style the bound pipeline does not draw.
#[test]
fn each_beauty_pipeline_builds_its_styles_entry_and_reports_the_style_it_selected_by() {
    assert_eq!(beauty_fragment_entry(SpecularStyle::Pixel), "fs_pixel");
    assert_eq!(beauty_fragment_entry(SpecularStyle::Smooth), "fs_smooth");
    let module = naga::front::wgsl::parse_str(SHADER_SOURCE).expect("the mover shader parses");
    for style in SpecularStyle::ALL {
        let entry = beauty_fragment_entry(style);
        assert!(
            module
                .entry_points
                .iter()
                .any(|e| e.name == entry && e.stage == naga::ShaderStage::Fragment),
            "{style:?}: the mover shader declares fragment entry {entry}"
        );
    }
    let squashed = |text: &str| -> String { text.split_whitespace().collect() };
    let pass = squashed(include_str!("kinematic_brush.rs"));
    for pin in [
        // Creation: each style's pipeline takes that style's entry.
        "pixel:build(SpecularStyle::Pixel),smooth:build(SpecularStyle::Smooth),",
        "entry_point:Some(beauty_fragment_entry(style)),",
        "SpecularStyle::Pixel=>&self.pixel,SpecularStyle::Smooth=>&self.smooth,",
        // Selection: one style both binds the pipeline and is reported.
        "pass.set_pipeline(self.beauty_pipelines.for_style(style));#[cfg(test)]\
         self.bound_styles.lock().expect(\"bound_styles\").push((view.kind(),style));",
    ] {
        assert_eq!(
            pass.matches(pin).count(),
            1,
            "kinematic_brush.rs pins {pin}"
        );
    }
    let record = pass
        .split("pub(crate)fnrecord_draws(")
        .nth(1)
        .and_then(|rest| rest.split("fninstance_model(").next())
        .expect("record_draws");
    assert_eq!(record.matches("set_pipeline(").count(), 1);
}

// The mover pass builds both beauty pipelines and its depth pipeline when it
// is built, at full init, and nothing later: exactly three render pipelines,
// with cube-array shadows on and off (the no-cube source carries both
// fragment entries).
#[test]
fn building_the_mover_pass_creates_both_beauty_pipelines_and_its_depth_pipeline() {
    use crate::render::gpu_create::gpu_creation_counts;
    let test = "building_the_mover_pass_creates_both_beauty_pipelines_and_its_depth_pipeline";
    let Some(renderer) = crate::render::gpu_test_harness::offscreen_renderer_or_skip(test, 16, 16)
    else {
        return;
    };
    let device = &renderer.device;
    let layout = |entries: &[wgpu::BindGroupLayoutEntry]| {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries,
        })
    };
    let camera = layout(&crate::render::pipeline_layout::uniform_bind_group_layout_entries());
    let material = layout(&crate::render::pipeline_layout::material_bind_group_layout_entries());
    let sh = layout(&crate::render::sh_volume::mesh_bind_group_layout_entries());
    for cube_array_supported in [true, false] {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let before = gpu_creation_counts();
        let _pass = KinematicBrushPass::new(
            device,
            crate::render::DEPTH_FORMAT,
            &camera,
            &material,
            &sh,
            cube_array_supported,
        );
        let built = gpu_creation_counts().since(before);
        let error = pollster::block_on(scope.pop());
        assert!(
            error.is_none(),
            "cube arrays {cube_array_supported}: the mover pipelines build: {error:?}"
        );
        assert_eq!(
            built.pipelines, 3,
            "cube arrays {cube_array_supported}: smooth beauty, pixel beauty and depth"
        );
    }
}
