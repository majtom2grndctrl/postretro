// GPU-free planar proofs: shader validation, IP shape and the frame's planar call order.
// See: context/lib/rendering_pipeline.md §7.3a

use super::pipelines::{MASK_SHADER_SOURCE, RESOLVE_AUTHORED_SOURCE, RESOLVE_SHADER_SOURCE};

/// Every module a planar pipeline compiles: the mask, and the resolve
/// composed with the shared motion and ray helpers.
const PLANAR_SHADERS: [(&str, &str); 2] = [
    ("planar_mask.wgsl", MASK_SHADER_SOURCE),
    ("planar_resolve.wgsl (composed)", RESOLVE_SHADER_SOURCE),
];

/// The planar-authored shader text the IP scan reads. The composed helpers
/// (fluid_motion.wgsl, fluid_reflection_ray.wgsl) are pinned where SSR's
/// pass composes them, and stay out of the scan.
const PLANAR_AUTHORED: [(&str, &str); 2] = [
    ("planar_mask.wgsl", MASK_SHADER_SOURCE),
    ("planar_resolve.wgsl", RESOLVE_AUTHORED_SOURCE),
];

/// Code only: comments removed, whitespace kept.
fn code(source: &str) -> String {
    source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `text` names `identifier` as a whole word.
fn names(text: &str, identifier: &str) -> bool {
    text.match_indices(identifier).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + identifier.len()..].chars().next();
        let word = |ch: Option<char>| ch.is_some_and(|ch| ch.is_alphanumeric() || ch == '_');
        !word(before) && !word(after)
    })
}

/// The argument text of each call to `function` in `text`.
fn call_arguments<'a>(text: &'a str, function: &str) -> Vec<&'a str> {
    let needle = format!("{function}(");
    text.match_indices(&needle)
        .filter(|(at, _)| {
            !text[..*at]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
        })
        .map(|(at, _)| {
            let open = at + needle.len();
            let mut depth = 1;
            let close = text[open..]
                .char_indices()
                .find_map(|(offset, ch)| {
                    match ch {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    (depth == 0).then_some(open + offset)
                })
                .expect("call closes");
            &text[open..close]
        })
        .collect()
}

/// The condition text of each `if` in `text`, up to its opening brace.
fn if_conditions(text: &str) -> Vec<&str> {
    text.match_indices("if ")
        .filter(|(at, _)| {
            !text[..*at]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
        })
        .map(|(at, _)| {
            let start = at + 3;
            &text[start..start + text[start..].find('{').expect("if body")]
        })
        .collect()
}

#[test]
fn planar_shaders_validate_without_an_adapter() {
    for (name, source) in PLANAR_SHADERS {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|error| panic!("{name} parses: {error}"));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("{name} validates: {error:?}"));
    }
}

/// The calls in `text` whose callee starts with `prefix`, by callee name.
fn callees<'a>(text: &'a str, prefix: &str) -> Vec<&'a str> {
    text.match_indices(prefix)
        .filter(|(at, _)| {
            !text[..*at]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
        })
        .filter_map(|(at, _)| {
            let name_end = at
                + text[at..]
                    .find(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
                    .unwrap_or(text.len() - at);
            text[name_end..]
                .trim_start()
                .starts_with('(')
                .then_some(&text[at..name_end])
        })
        .collect()
}

/// `text` with WGSL type templates (`vec3<f32>`, `var<uniform>`, ...) and
/// return arrows removed, so every `<` or `>` left is a comparison.
fn without_templates(text: &str) -> String {
    const TEMPLATED: [&str; 9] = [
        "vec2",
        "vec3",
        "vec4",
        "mat4x4",
        "mat3x3",
        "texture_2d",
        "array",
        "var",
        "ptr",
    ];
    let mut out = String::with_capacity(text.len());
    let mut rest = text.replace("->", " ");
    while let Some(open) = rest.find('<') {
        let head = &rest[..open];
        let templated = TEMPLATED.iter().any(|name| {
            head.ends_with(name)
                && !head[..head.len() - name.len()]
                    .chars()
                    .next_back()
                    .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
        });
        if !templated {
            out.push_str(&rest[..=open]);
            rest = rest[open + 1..].to_string();
            continue;
        }
        out.push_str(head);
        let mut depth = 0;
        let close = rest[open..]
            .char_indices()
            .find_map(|(offset, ch)| {
                match ch {
                    '<' => depth += 1,
                    '>' => depth -= 1,
                    _ => {}
                }
                (depth == 0).then_some(open + offset)
            })
            .expect("template closes");
        rest = rest[close + 1..].to_string();
    }
    out.push_str(&rest);
    out
}

/// The statements and conditions of `text` that compare: split at `;`, `{`
/// and `}`, each piece holding `<`, `>`, `==` or `!=` once templates are gone.
fn comparisons(text: &str) -> Vec<String> {
    without_templates(text)
        .split([';', '{', '}'])
        .filter(|piece| {
            piece.contains('<')
                || piece.contains('>')
                || piece.contains("==")
                || piece.contains("!=")
        })
        .map(str::to_string)
        .collect()
}

// AC 3: the IP gate on the planar-authored shader text (rendering_pipeline.md
// §7.3a Planar layer IP rules; ip-check.md D1-D9, path P2). No Fresnel term,
// no probe binding or name; every `fluid_reflection_` or `fluid_displayed_`
// call names a D4 helper, so the reflecting gate, seen-from-above, the box
// pull, `fluid_reflection_point` and every capture-shadow, tile, lookup and
// ratio helper never appear; no `dot(` (the reflection law's dot lives in the
// composed, pinned helpers). The direction-valued identifiers (view vector,
// reflected and tilted rays, cell and flat normals) and the reprojected clip
// w appear in no comparison, `if`, `select(`, `step(` or `smoothstep(`: the
// mask, resolve and weight cut on coverage, occlusion and distance alone, and
// an off-frustum reprojection clamps.
#[test]
fn planar_shaders_preserve_ip_boundaries() {
    const D4_HELPERS: [&str; 6] = [
        "fluid_displayed_offset",
        "fluid_reflection_texel_axes",
        "fluid_reflection_cell_centre",
        "fluid_reflection_cell_normal",
        "fluid_reflection_mirror",
        "fluid_reflection_above_horizon",
    ];
    // The resolve's own direction-valued names, and generic names a later
    // edit might reach for. Eye positions are positions: R cuts on the eye's
    // distance.
    const DIRECTION: [&str; 17] = [
        "view_vector",
        "reflected_ray",
        "tilted_ray",
        "cell_normal",
        "flat_normal",
        "world_normal",
        "tilt",
        "normal",
        "view",
        "view_dir",
        "ray",
        "reflected",
        "tilted",
        "outward",
        "direction",
        "dir",
        "incident",
    ];
    const CLIP_W: [&str; 2] = ["mirror_clip.w", "clip.w"];
    for (name, source) in PLANAR_AUTHORED {
        let code = code(source);
        let lower = code.to_lowercase();
        for banned in [
            "fresnel", "schlick", "probe", "capture", "tile", "ratio", "lookup", "dot(",
        ] {
            assert!(!lower.contains(banned), "{name} names `{banned}`");
        }
        for prefix in ["fluid_reflection_", "fluid_displayed_"] {
            for callee in callees(&code, prefix) {
                assert!(
                    D4_HELPERS.contains(&callee),
                    "{name} calls `{callee}`, not a D4 helper"
                );
            }
        }
        let mut guarded: Vec<String> = comparisons(&code);
        for function in ["select", "step", "smoothstep"] {
            guarded.extend(
                call_arguments(&code, function)
                    .into_iter()
                    .map(str::to_string),
            );
        }
        guarded.extend(if_conditions(&code).into_iter().map(str::to_string));
        for text in &guarded {
            for identifier in DIRECTION {
                assert!(
                    !names(text, identifier),
                    "{name}: `{identifier}` inside a decision: {text}"
                );
            }
            for clip_w in CLIP_W {
                assert!(
                    !text.contains(clip_w),
                    "{name}: the reprojected clip w inside a decision: {text}"
                );
            }
        }
        // The full-screen view-ray reconstruction is gone: the resolve's world
        // position is the fragment's own.
        for retired in ["abs(ray", "inverse_main_vp"] {
            assert!(!code.contains(retired), "{name} keeps `{retired}`");
        }
    }
    // The scan names what the resolve really computes, so a rename cannot
    // slip past it, and the resolve composes every D4 helper.
    let resolve = code(RESOLVE_AUTHORED_SOURCE);
    for identifier in [
        "view_vector",
        "reflected_ray",
        "tilted_ray",
        "cell_normal",
        "flat_normal",
        "world_normal",
    ] {
        assert!(
            names(&resolve, identifier),
            "the resolve names `{identifier}`"
        );
    }
    assert!(resolve.contains("mirror_clip.w"));
    for helper in D4_HELPERS {
        assert!(
            callees(&resolve, helper).contains(&helper),
            "the resolve composes `{helper}`"
        );
    }
    // The comparison finder sees through templates and arrows, and catches a
    // bare comparison.
    assert!(comparisons("let a = vec3<f32>(0.0); fn f() -> vec2<i32> {").is_empty());
    assert_eq!(comparisons("let b = tilted_ray.y < 0.0;").len(), 1);
    // The resolve binds the camera, the face's material and its own inputs,
    // and no probe resource; the composed helpers bind nothing.
    let bindings: Vec<&str> = resolve
        .match_indices("@group(")
        .map(|(at, _)| {
            let line = &resolve[at..];
            line[..line.find(';').expect("binding ends")].trim()
        })
        .collect();
    assert_eq!(
        bindings,
        [
            "@group(0) @binding(0) var<uniform> uniforms: Uniforms",
            "@group(1) @binding(0) var base_texture: texture_2d<f32>",
            "@group(2) @binding(0) var<uniform> planar: PlanarInputs",
            "@group(2) @binding(1) var mirror_color: texture_2d<f32>",
            "@group(2) @binding(2) var mirror_depth: texture_depth_2d",
        ]
    );
    let composed = &RESOLVE_SHADER_SOURCE[RESOLVE_AUTHORED_SOURCE.len()..];
    assert!(!code(composed).contains("@group("));
}

/// `name`'s whole declaration in `source`, from `prefix name(` (or `prefix
/// name {`) to its closing brace.
fn wgsl_item<'a>(source: &'a str, prefix: &str, name: &str) -> &'a str {
    let start = [format!("{prefix} {name}("), format!("{prefix} {name} {{")]
        .iter()
        .find_map(|needle| source.find(needle.as_str()))
        .unwrap_or_else(|| panic!("declares {prefix} {name}"));
    let body = start + source[start..].find('{').expect("body");
    let mut depth = 0i32;
    for (offset, ch) in source[body..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..body + offset + 1];
                }
            }
            _ => {}
        }
    }
    panic!("{name} closes");
}

/// Code only, comments and whitespace removed.
fn compact(source: &str) -> String {
    code(source).split_whitespace().collect()
}

/// A pinned body as written: whitespace removed.
fn pinned(body: &str) -> String {
    body.split_whitespace().collect()
}

// The resolve's group-0 uniform is the camera uniform the reflection pass
// redeclares, member for member, and both planar shaders declare one
// PlanarInputs, the Rust writer's 68 words.
#[test]
fn planar_resolve_redeclares_the_shared_uniforms_member_for_member() {
    let ssr = include_str!("../../shaders/fluid_reflection_pass.wgsl");
    assert_eq!(
        compact(wgsl_item(RESOLVE_AUTHORED_SOURCE, "struct", "Uniforms")),
        compact(wgsl_item(ssr, "struct", "Uniforms"))
    );
    assert_eq!(
        compact(wgsl_item(RESOLVE_AUTHORED_SOURCE, "struct", "PlanarInputs")),
        compact(wgsl_item(MASK_SHADER_SOURCE, "struct", "PlanarInputs"))
    );
    let module = naga::front::wgsl::parse_str(RESOLVE_SHADER_SOURCE).expect("parses");
    let mut layouter = naga::proc::Layouter::default();
    layouter.update(module.to_ctx()).expect("layout");
    let (inputs, _) = module
        .types
        .iter()
        .find(|(_, ty)| ty.name.as_deref() == Some("PlanarInputs"))
        .expect("PlanarInputs");
    assert_eq!(u64::from(layouter[inputs].size), super::PLANAR_INPUT_BYTES);
    assert_eq!(super::PLANAR_INPUT_BYTES, 68 * 4);
}

// The resolve's pixel-style tilt is the CPU reference in render-cpu's
// planar_tilt.rs, statement for statement, pinned on both sides: the cell
// ray from the D4 helpers, the clamped mirror texel, the surface a texel
// shows, coverage, X = P + r'·d, and the fragment's style switch and lookup.
#[test]
fn planar_tilt_wgsl_bodies_match_the_cpu_reference() {
    let wgsl = RESOLVE_AUTHORED_SOURCE;
    let cpu = include_str!("../../../../render-cpu/src/planar_tilt.rs");
    let bodies: [(&str, &str, &str); 5] = [
        (
            "planar_tilt_ray",
            "fn planar_tilt_ray(texel_uv: vec2<f32>, world_position: vec3<f32>,
             flat_normal: vec3<f32>, axes: FluidTexelAxes, eye: vec3<f32>, time: f32,
             texels_per_pixel: f32, offset_scale: i32) -> PlanarTiltRay {
             let offset = fluid_displayed_offset(vec2<i32>(floor(texel_uv)), time,
             texels_per_pixel) * offset_scale;
             let cell_centre = fluid_reflection_cell_centre(texel_uv, world_position, axes);
             let cell_normal = fluid_reflection_cell_normal(flat_normal, axes, offset);
             let view_vector = normalize(eye - cell_centre);
             let reflected_ray = fluid_reflection_mirror(view_vector, cell_normal);
             let tilted_ray = fluid_reflection_above_horizon(reflected_ray);
             return PlanarTiltRay(cell_centre, cell_normal, tilted_ray); }",
            "pub fn planar_tilt_ray(texel_uv: Vec2, world_position: Vec3, flat_normal: Vec3,
             axes: FluidTexelAxes, eye: Vec3, time: f32, texels_per_pixel: f32,
             offset_scale: i32,) -> PlanarTiltRay {
             let offset = displayed_offset(texel_cell(texel_uv), time, texels_per_pixel)
             * offset_scale;
             let cell_centre = reflection_cell_centre(texel_uv, world_position, axes);
             let cell_normal = reflection_cell_normal(flat_normal, axes, offset);
             let view_vector = (eye - cell_centre).normalize();
             let reflected_ray = reflection_mirror(view_vector, cell_normal);
             let tilted_ray = reflection_above_horizon(reflected_ray);
             PlanarTiltRay { cell_centre, cell_normal, tilted_ray, } }",
        ),
        (
            "planar_mirror_texel",
            "fn planar_mirror_texel(point: vec3<f32>) -> vec2<i32> {
             let mirror_clip = planar.mirror_vp * vec4<f32>(point, 1.0);
             let mirror_ndc = mirror_clip.xy / max(mirror_clip.w, PLANAR_W_FLOOR);
             let size = planar.extent.xy;
             let at = floor(vec2<f32>(mirror_ndc.x * 0.5 + 0.5, 0.5 - mirror_ndc.y * 0.5) * size);
             return vec2<i32>(clamp(at, vec2<f32>(0.0), size - vec2<f32>(1.0))); }",
            "pub fn planar_mirror_texel(point: Vec3, mirror: &PlanarMirrorView) -> IVec2 {
             let mirror_clip = mirror.raster_vp * point.extend(1.0);
             let mirror_ndc = Vec2::new(mirror_clip.x, mirror_clip.y)
             / mirror_clip.w.max(PLANAR_W_FLOOR);
             let size = mirror.extent;
             let at = (Vec2::new(mirror_ndc.x * 0.5 + 0.5, 0.5 - mirror_ndc.y * 0.5) * size)
             .floor();
             at.clamp(Vec2::ZERO, size - Vec2::ONE).as_ivec2() }",
        ),
        (
            "planar_mirror_surface",
            "fn planar_mirror_surface(texel: vec2<i32>, depth: f32) -> vec3<f32> {
             let centre = (vec2<f32>(texel) + vec2<f32>(0.5)) / planar.extent.xy;
             let surface_h = planar.inverse_mirror_vp
             * vec4<f32>(centre.x * 2.0 - 1.0, 1.0 - centre.y * 2.0, depth, 1.0);
             return surface_h.xyz / surface_h.w; }",
            "pub fn planar_mirror_surface(texel: IVec2, depth: f32, mirror: &PlanarMirrorView)
             -> Vec3 {
             let centre = (texel.as_vec2() + Vec2::splat(0.5)) / mirror.extent;
             let surface_h = mirror.inverse_raster_vp
             * Vec4::new(centre.x * 2.0 - 1.0, 1.0 - centre.y * 2.0, depth, 1.0);
             surface_h.truncate() / surface_h.w }",
        ),
        (
            "planar_covered",
            "fn planar_covered(depth: f32) -> bool { return depth > 0.0 && depth < 1.0; }",
            "pub fn planar_covered(depth: f32) -> bool { depth > 0.0 && depth < 1.0 }",
        ),
        (
            "planar_tilted_point",
            "fn planar_tilted_point(tilt: PlanarTiltRay, untilted: vec2<i32>,
             untilted_depth: f32) -> vec3<f32> {
             let reach = distance(planar_mirror_surface(untilted, untilted_depth),
             tilt.cell_centre);
             return tilt.cell_centre + tilt.tilted_ray * reach; }",
            "pub fn planar_tilted_point(tilt: &PlanarTiltRay, untilted: IVec2,
             untilted_depth: f32, mirror: &PlanarMirrorView,) -> Vec3 {
             let reach = planar_mirror_surface(untilted, untilted_depth, mirror)
             .distance(tilt.cell_centre);
             tilt.cell_centre + tilt.tilted_ray * reach }",
        ),
    ];
    for (name, wgsl_body, cpu_body) in bodies {
        assert_eq!(
            compact(wgsl_item(wgsl, "fn", name)),
            pinned(wgsl_body),
            "planar_resolve.wgsl `{name}` drifted from its pin"
        );
        assert_eq!(
            compact(wgsl_item(cpu, "pub fn", name)),
            pinned(cpu_body),
            "planar_tilt.rs `{name}` drifted from its pin"
        );
    }
    // The fragment, whole: derivatives first; texels per pixel scaled to
    // scene resolution as the reflection pass scales its own; smooth reads
    // the fragment's own texel; pixel reads where the cell's tilted ray
    // reaches at the untilted texel's depth, or nothing where that texel is
    // empty. No statement between may read a direction quantity.
    assert_eq!(
        compact(wgsl_item(wgsl, "fn", "fs_planar_resolve")),
        pinned(
            "fn fs_planar_resolve(in: ResolveVertex) -> @location(0) vec4<f32> {
             let ddx = dpdx(in.uv); let ddy = dpdy(in.uv);
             let ddx_world = dpdx(in.world_position); let ddy_world = dpdy(in.world_position);
             let dims = vec2<f32>(textureDimensions(base_texture, 0));
             let scale = planar.extent.xy / planar.extent.zw;
             let texels_per_pixel = max(length(ddx * dims * scale.x),
             length(ddy * dims * scale.y));
             if planar.reflection_style == FLUID_REFLECTION_STYLE_SMOOTH {
             return planar_layer_value(planar_mirror_texel(in.world_position),
             in.world_position); }
             let axes = fluid_reflection_texel_axes(ddx * dims, ddy * dims, ddx_world, ddy_world);
             let tilt = planar_tilt_ray(in.uv * dims, in.world_position,
             normalize(in.world_normal), axes, uniforms.camera_position, uniforms.time,
             texels_per_pixel, planar.offset_scale);
             let untilted = planar_mirror_texel(tilt.cell_centre);
             let untilted_depth = textureLoad(mirror_depth, untilted, 0);
             if !planar_covered(untilted_depth) { return vec4<f32>(0.0); }
             return planar_layer_value(planar_mirror_texel(planar_tilted_point(tilt, untilted,
             untilted_depth)), in.world_position); }"
        )
    );
    // Its CPU mirror: the same switch and lookup after the derivatives.
    assert_eq!(
        compact(wgsl_item(cpu, "pub fn", "planar_resolve_texel")),
        pinned(
            "pub fn planar_resolve_texel(style: u32, texel_uv: Vec2, world_position: Vec3,
             world_normal: Vec3, axes: FluidTexelAxes, eye: Vec3, time: f32,
             texels_per_pixel: f32, offset_scale: i32, mirror: &PlanarMirrorView,
             depth_at: impl Fn(IVec2) -> f32,) -> Option<IVec2> {
             if style == FLUID_REFLECTION_STYLE_SMOOTH {
             return Some(planar_mirror_texel(world_position, mirror)); }
             let tilt = planar_tilt_ray(texel_uv, world_position, world_normal.normalize(), axes,
             eye, time, texels_per_pixel, offset_scale,);
             let untilted = planar_mirror_texel(tilt.cell_centre, mirror);
             let untilted_depth = depth_at(untilted);
             if !planar_covered(untilted_depth) { return None; }
             Some(planar_mirror_texel(
             planar_tilted_point(&tilt, untilted, untilted_depth, mirror), mirror,)) }"
        )
    );
    // One floor on both sides.
    assert!(wgsl.contains("const PLANAR_W_FLOOR: f32 = 1.0e-6;"));
    assert!(cpu.contains("pub const PLANAR_W_FLOOR: f32 = 1.0e-6;"));
}

// The frame records the planar pass once, after the ghost pass and before
// the fluid gate, on the frame's encoder; the call reads no tier, so the
// frame's one tier read stays the fluid passes' snapshot.
#[test]
fn planar_pass_records_between_the_ghost_pass_and_the_fluid_gate() {
    let frame = include_str!("../renderer_render_frame.rs");
    let ghost = frame.find(".ghost_order.record(").expect("ghost pass");
    let planar = frame
        .find("self.record_planar_reflection_pass(")
        .expect("planar pass");
    let gate = frame
        .find("if render_world && self.full().fluid_surface.has_draws()")
        .expect("fluid gate");
    assert!(ghost < planar && planar < gate, "{ghost} {planar} {gate}");
    assert_eq!(
        frame.matches("self.record_planar_reflection_pass(").count(),
        1
    );
    let call = &frame[planar..planar + frame[planar..].find(';').expect("call ends")];
    assert_eq!(
        call,
        "self.record_planar_reflection_pass(encoder, render_world, world_mesh_frame_plan)"
    );
    let record = include_str!("record.rs");
    for second_encoder in [
        "create_command_encoder",
        "RenderBundle",
        "thread::spawn",
        "rayon",
    ] {
        assert!(
            !record.contains(second_encoder),
            "planar recording stays on the frame encoder: `{second_encoder}`"
        );
    }
}

// Animated-lightmap compose covers the mirror's drawn cells beside the main
// view's, ascending and without repeats, so a reflected animated face shows
// its current light; a draw-all side keeps the union draw-all.
#[test]
fn planar_compose_union_adds_mirror_cells_to_the_main_view() {
    use postretro_visibility::VisibleCells;
    let mut out = VisibleCells::DrawAll;
    super::union_cells(
        &VisibleCells::Culled(vec![4, 0]),
        &VisibleCells::Culled(vec![6, 0, 5]),
        &mut out,
    );
    let VisibleCells::Culled(union) = &out else {
        panic!("two culled sets compose a culled union");
    };
    assert_eq!(union, &[0, 4, 5, 6]);
    super::union_cells(
        &VisibleCells::DrawAll,
        &VisibleCells::Culled(vec![1]),
        &mut out,
    );
    assert!(matches!(out, VisibleCells::DrawAll));
}
