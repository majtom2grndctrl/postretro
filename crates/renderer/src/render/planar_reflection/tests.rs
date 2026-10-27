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

/// Whether `ch` continues an identifier.
fn word_char(ch: Option<char>) -> bool {
    ch.is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
}

/// Whether `text` names `identifier` as a whole word.
fn names(text: &str, identifier: &str) -> bool {
    text.match_indices(identifier).any(|(at, _)| {
        !word_char(text[..at].chars().next_back())
            && !word_char(text[at + identifier.len()..].chars().next())
    })
}

/// The argument text of each call to `function` in `text`: the whole word,
/// then any whitespace, then `(`.
fn call_arguments<'a>(text: &'a str, function: &str) -> Vec<&'a str> {
    text.match_indices(function)
        .filter(|(at, _)| !word_char(text[..*at].chars().next_back()))
        .filter_map(|(at, _)| {
            let rest = text[at + function.len()..].trim_start();
            rest.starts_with('(').then_some(text.len() - rest.len() + 1)
        })
        .map(|open| {
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

/// The condition text of each `if` (the whole word, however spaced) in
/// `text`, up to its opening brace.
fn if_conditions(text: &str) -> Vec<&str> {
    text.match_indices("if")
        .filter(|(at, _)| {
            !word_char(text[..*at].chars().next_back()) && !word_char(text[at + 2..].chars().next())
        })
        .map(|(at, _)| {
            let start = at + 2;
            &text[start..start + text[start..].find('{').expect("if body")]
        })
        .collect()
}

/// The names `text` declares with `fn`, in order.
fn declared_functions(text: &str) -> Vec<&str> {
    text.match_indices("fn")
        .filter(|(at, _)| {
            !word_char(text[..*at].chars().next_back())
                && text[at + 2..].starts_with(char::is_whitespace)
        })
        .map(|(at, _)| {
            let rest = text[at + 2..].trim_start();
            &rest[..rest
                .find(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
                .unwrap_or(rest.len())]
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

/// The shared helpers planar code may compose (ip-check.md D4). Every other
/// `fluid_` function the composed modules declare stays out of planar text.
const D4_HELPERS: [&str; 6] = [
    "fluid_displayed_offset",
    "fluid_reflection_texel_axes",
    "fluid_reflection_cell_centre",
    "fluid_reflection_cell_normal",
    "fluid_reflection_mirror",
    "fluid_reflection_above_horizon",
];

/// The resolve's own direction-valued names, and generic names a later edit
/// might reach for. Eye positions are positions: R cuts on the eye's
/// distance.
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

/// The resolve's reprojected clip position and its NDC: a clamp may place
/// with them, no decision may read them.
const REPROJECTION: [&str; 2] = ["mirror_clip", "mirror_ndc"];

/// Built-ins whose arguments scale or shape a value: a direction quantity
/// inside one would weight by direction.
const WEIGHTING: [&str; 7] = ["max", "min", "clamp", "sign", "abs", "saturate", "pow"];

/// Every IP-scan finding in planar-authored `source`, empty when it passes.
/// `reprojects` marks the resolve, whose reprojected clip position stays out
/// of every decision; the mask's depth test reads the main view's own.
fn ip_findings(source: &str, reprojects: bool) -> Vec<String> {
    let code = code(source);
    let lower = code.to_lowercase();
    let mut findings = Vec::new();
    for banned in [
        "fresnel", "schlick", "probe", "capture", "tile", "ratio", "lookup",
    ] {
        if lower.contains(banned) {
            findings.push(format!("names `{banned}`"));
        }
    }
    if callees(&code, "dot").contains(&"dot") {
        findings.push("calls `dot`".to_string());
    }
    for callee in callees(&code, "fluid_") {
        if !D4_HELPERS.contains(&callee) {
            findings.push(format!("calls `{callee}`, not a D4 helper"));
        }
    }
    let mut decisions: Vec<String> = comparisons(&code);
    for function in ["select", "step", "smoothstep"] {
        decisions.extend(
            call_arguments(&code, function)
                .into_iter()
                .map(str::to_string),
        );
    }
    decisions.extend(if_conditions(&code).into_iter().map(str::to_string));
    let weights: Vec<&str> = WEIGHTING
        .iter()
        .flat_map(|function| call_arguments(&code, function))
        .collect();
    for text in decisions.iter().map(String::as_str).chain(weights) {
        for identifier in DIRECTION {
            if names(text, identifier) {
                findings.push(format!(
                    "`{identifier}` inside a decision or weight: {text}"
                ));
            }
        }
    }
    if reprojects {
        for text in &decisions {
            for identifier in REPROJECTION {
                if names(text, identifier) {
                    findings.push(format!(
                        "the reprojected `{identifier}` inside a decision: {text}"
                    ));
                }
            }
        }
    }
    // The full-screen view-ray reconstruction is gone: the resolve's world
    // position is the fragment's own.
    for retired in ["abs(ray", "inverse_main_vp"] {
        if code.contains(retired) {
            findings.push(format!("keeps `{retired}`"));
        }
    }
    findings
}

// AC 3: the IP gate on the planar-authored shader text (rendering_pipeline.md
// §7.3a Planar layer IP rules; ip-check.md D1-D9, path P2). No Fresnel term,
// no probe binding or name; every `fluid_` call names a D4 helper, so the
// reflecting gate, seen-from-above, the box pull, `fluid_reflection_point`,
// every capture-shadow, tile, lookup and ratio helper and the composed
// motion module's wave, drift, warp and hash functions never appear; no
// `dot` call (the reflection law's dot lives in the composed, pinned
// helpers). The direction-valued identifiers (view vector, reflected and
// tilted rays, cell and flat normals) appear in no comparison, `if`,
// `select`, `step` or `smoothstep`, nor inside `max`, `min`, `clamp`, `sign`,
// `abs`, `saturate` or `pow`; the resolve's reprojected clip position and NDC
// appear in no decision: the mask, resolve and weight cut on coverage,
// occlusion and distance alone, and an off-frustum reprojection clamps. Calls
// match however they are spaced. The scan cannot see a direction quantity
// under a new name: the snapshot and CPU pins below fix every function's
// text, and the GPU c proof reads c itself.
#[test]
fn planar_shaders_preserve_ip_boundaries() {
    for (name, source) in PLANAR_AUTHORED {
        let findings = ip_findings(source, name == "planar_resolve.wgsl");
        assert!(findings.is_empty(), "{name}: {findings:#?}");
    }
    // The scan catches each shape a later edit might take, spaced or not.
    for escape in [
        "let c = select(0.0, 1.0, tilted_ray.y > 0.0);",
        "let c = select (0.0, 1.0, tilted_ray.y);",
        "fn f() { if(all(view_vector)) { return; } }",
        "let c = step (0.0, reflected_ray.y);",
        "let w = dot (a, b);",
        "let c = max(tilted_ray.y, 0.0);",
        "let c = pow (abs(cell_normal.y), 5.0);",
        "let o = fluid_wave_offset(uv, t, wave);",
        "let g = fluid_reflection_gate(p, n, v, mask);",
    ] {
        assert!(
            !ip_findings(escape, false).is_empty(),
            "the scan misses: {escape}"
        );
    }
    for escape in [
        "fn f() { if mirror_clip.w <= 0.0 { return; } }",
        "let k = select(0.0, 1.0, mirror_ndc.x);",
    ] {
        assert!(
            !ip_findings(escape, true).is_empty(),
            "the scan misses: {escape}"
        );
    }
    // A clamp placing with the reprojection is no decision.
    assert!(ip_findings("let w = max(mirror_clip.w, PLANAR_W_FLOOR);", true).is_empty());
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
        "mirror_clip",
        "mirror_ndc",
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

// AC 3, AC 11 (ip-check.md D1-D2). The planar functions no CPU reference
// mirrors statement for statement, pinned whole as written: the layer value
// (c from coverage, D at the surface the texel shows and R at the receiving
// fragment, nothing else), the fade, the resolve's vertex entry and the whole
// mask. A direction factor in c under any name, e.g.
// `* max(normalize(planar.main_eye.xyz - receiver).y, 0.0)`, passes the IP
// scan but fails here; any edit to these bodies updates this pin under review
// against rendering_pipeline.md §7.3a's planar IP rules.
#[test]
fn planar_unmirrored_shader_bodies_match_their_snapshots() {
    let snapshots: [(&str, &str, &str); 5] = [
        (
            RESOLVE_AUTHORED_SOURCE,
            "planar_layer_value",
            "fn planar_layer_value(texel: vec2<i32>, receiver: vec3<f32>) -> vec4<f32> {
             let depth = textureLoad(mirror_depth, texel, 0);
             if !planar_covered(depth) { return vec4<f32>(0.0); }
             let surface = planar_mirror_surface(texel, depth);
             let c = planar_distance_fade(distance(planar.mirror_eye.xyz, surface), planar.limits.x,
             planar.limits.y)
             * planar_distance_fade(distance(planar.main_eye.xyz, receiver), planar.limits.z,
             planar.limits.w);
             let colour = clamp(textureLoad(mirror_color, texel, 0).rgb, vec3<f32>(0.0),
             vec3<f32>(65504.0));
             return vec4<f32>(colour * c, c); }",
        ),
        (
            RESOLVE_AUTHORED_SOURCE,
            "planar_distance_fade",
            "fn planar_distance_fade(metres: f32, limit: f32, band: f32) -> f32 {
             if limit <= 0.0 { return 1.0; }
             return clamp((limit - metres) / max(band, 0.001), 0.0, 1.0); }",
        ),
        (
            RESOLVE_AUTHORED_SOURCE,
            "vs_planar_resolve",
            "fn vs_planar_resolve(in: ResolveVertexInput) -> ResolveVertex {
             var out: ResolveVertex;
             out.clip_position = planar.main_vp * vec4<f32>(in.position, 1.0);
             out.world_position = in.position;
             out.world_normal = in.normal;
             out.uv = in.uv;
             return out; }",
        ),
        (
            MASK_SHADER_SOURCE,
            "vs_planar_mask",
            "fn vs_planar_mask(@location(0) position: vec3<f32>) -> MaskVertex {
             var out: MaskVertex;
             let clip = planar.main_vp * vec4<f32>(position, 1.0);
             out.clip_position = vec4<f32>(-clip.x, clip.y, clip.z, clip.w);
             out.world_position = position;
             return out; }",
        ),
        (
            MASK_SHADER_SOURCE,
            "fs_planar_mask",
            "fn fs_planar_mask(in: MaskVertex) -> MaskFragment {
             let clip = planar.main_vp * vec4<f32>(in.world_position, 1.0);
             let ndc = clip.xyz / max(clip.w, PLANAR_W_FLOOR);
             let scene = planar.extent.zw;
             let last = vec2<i32>(scene) - vec2<i32>(1);
             let pixel = clamp(vec2<i32>(floor(vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5)
             * scene)), vec2<i32>(0), last);
             if ndc.z > textureLoad(scene_depth, pixel, 0) + PLANAR_MASK_DEPTH_SLACK { discard; }
             if planar.limits.z > 0.0
             && distance(planar.main_eye.xyz, in.world_position) >= planar.limits.z { discard; }
             var out: MaskFragment;
             out.depth = 1.0;
             return out; }",
        ),
    ];
    for (source, name, body) in snapshots {
        assert_eq!(
            compact(wgsl_item(source, "fn", name)),
            pinned(body),
            "`{name}` drifted from its snapshot"
        );
    }
    // The fade follows render-cpu's `planar_fade`: the same clamped ramp; a
    // zero limit is the CPU's unlimited (the writer stores `unwrap_or(0.0)`);
    // the band is sanitized to at least 0.001, so the shader's
    // `max(band, 0.001)` never changes a band the writer sends.
    let cpu = include_str!("../../../../render-cpu/src/planar_reflection.rs");
    assert!(
        compact(wgsl_item(cpu, "pub fn", "planar_fade"))
            .contains("((limit-distance)/band).clamp(0.0,1.0)"),
        "render-cpu's planar_fade keeps the shader's ramp"
    );
    assert!(compact(wgsl_item(cpu, "pub fn", "sanitized")).contains("value.max(0.001)"));
    assert!(
        compact(include_str!("record.rs")).contains(
            "preset.d.unwrap_or(0.0),preset.band_d,preset.r.unwrap_or(0.0),preset.band_r,"
        )
    );
}

// AC 3. Every function the planar-authored text declares is on this reviewed
// list, and each is pinned whole: the tilt functions and the fragment against
// planar_tilt.rs, the rest by snapshot. A new helper fails here until it is
// reviewed against the planar IP rules and pinned.
#[test]
fn planar_shader_functions_match_the_reviewed_list() {
    assert_eq!(
        declared_functions(&code(MASK_SHADER_SOURCE)),
        ["vs_planar_mask", "fs_planar_mask"]
    );
    assert_eq!(
        declared_functions(&code(RESOLVE_AUTHORED_SOURCE)),
        [
            "vs_planar_resolve",
            "planar_distance_fade",
            "planar_tilt_ray",
            "planar_mirror_texel",
            "planar_mirror_surface",
            "planar_covered",
            "planar_tilted_point",
            "planar_layer_value",
            "fs_planar_resolve",
        ]
    );
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
