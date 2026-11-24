// Glossy overlay source gates for its lifecycle: the probe layer from the
// face's binding, the coplanar-mover guard's mark and clip path, and no
// planar layer on a world face.
// See: context/lib/testing_guide.md §3 (Source-scanning tests) · context/lib/rendering_pipeline.md §7.3a

use super::*;
use crate::render::gpu_test_harness::wgsl_function;

const OVERLAY_WGSL: &str = include_str!("../shaders/glossy_overlay.wgsl");
const FORWARD_WGSL: &str = include_str!("../shaders/forward.wgsl");
const MOVER_WGSL: &str = include_str!("../shaders/kinematic_brush.wgsl");

/// `src` with every `//` comment removed, then every whitespace character.
fn squashed(src: &str) -> String {
    src.lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<String>()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// `name`'s body in the overlay's own source, squashed.
fn overlay_fn(name: &str) -> String {
    squashed(wgsl_function(OVERLAY_WGSL, name))
}

/// Whether `text` names planar as a word of its own: "coplanar" does not
/// count, `planar`, `_planar` and `Planar` do.
fn names_planar(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.match_indices("planar").any(|(at, _)| {
        !lower[..at]
            .chars()
            .next_back()
            .is_some_and(|before| before.is_ascii_alphabetic())
    })
}

// Acceptance row 54: the scan that pins water's probe layer to its face's
// binding covers the overlay. The probe index rides the per-vertex world
// stream, written once at install from id 54's world bindings alone and read
// flat; the fragment hands that varying to the probe term and nothing else;
// every table read is indexed by the `probe` parameter; the layer is bound
// once from the probe alone and every function below only passes it on. The
// search, h and fade functions are water's, composed unmodified, so water's
// scan (`fluid_probe_layer_derives_from_the_face_binding_alone`) pins them.
#[test]
fn the_overlay_probe_layer_derives_from_the_face_binding_alone() {
    let code = squashed(OVERLAY_WGSL);
    assert!(code.contains("@location(3)probe:u32,"), "the vertex stream");
    assert!(code.contains("@location(2)@interpolate(flat)probe:u32,"));
    assert!(overlay_fn("vs_main").contains("out.probe=in.probe;"));
    assert_eq!(
        code.matches("in.probe").count(),
        3,
        "the vertex stage's copy and the probe term's two reads"
    );
    assert!(!squashed(include_str!("../shaders/glossy_overlay_shading.wgsl")).contains("probe"));
    let term = overlay_fn("glossy_overlay_probe");
    assert_eq!(term.matches("in.probe").count(), 2, "{term}");
    assert!(term.contains("ifin.probe==FLUID_REFLECTION_NO_PROBE"));
    assert!(term.contains("returnglossy_overlay_sample(in.probe,"));

    // Every table read is the face's probe.
    let table_reads = code.matches("fluid_probes.probes[").count();
    assert_eq!(
        code.matches("fluid_probes.probes[probe]").count(),
        table_reads
    );
    assert_eq!(table_reads, 4, "the layer, the box and the capture point");
    let sample = overlay_fn("glossy_overlay_sample");
    assert!(sample.starts_with("fnglossy_overlay_sample(probe:u32,"));
    for bound in ["letprobe", "varprobe", "probe="] {
        assert!(!sample.contains(bound), "the sample binds `{bound}`");
    }
    let layer_at = sample.find("letlayer=").expect("layer binding");
    let layer = &sample[layer_at..layer_at + sample[layer_at..].find(';').expect(";") + 1];
    assert_eq!(layer, "letlayer=fluid_probes.probes[probe].layer;");
    assert_eq!(code.matches("letlayer=").count(), 1, "one layer binding");

    // Below the sample the layer is a parameter, passed on to one colour tap.
    for function in ["glossy_overlay_probe_colour", "glossy_overlay_probe_mip"] {
        let body = overlay_fn(function);
        assert!(body.contains("layer:u32,"), "{body}");
        for bound in ["letlayer", "varlayer", "layer="] {
            assert!(!body.contains(bound), "`{function}` binds `{bound}`");
        }
    }
    assert_eq!(
        code.matches("fluid_probe_array").count(),
        3,
        "declaration, level count, tap"
    );
    assert!(
        overlay_fn("glossy_overlay_probe_mip")
            .contains("textureSampleLevel(fluid_probe_array,sh_atlas_sampler,uv,layer,f32(mip))")
    );
    assert!(
        overlay_fn("glossy_overlay_probe_colour").contains("textureNumLevels(fluid_probe_array)")
    );
    assert_eq!(
        code.matches("fluid_probe_distance").count(),
        1,
        "the overlay's own source only declares the distance array"
    );
    for water in [
        include_str!("../shaders/fluid_reflection.wgsl"),
        include_str!("../shaders/fluid_reflection_ray.wgsl"),
    ] {
        assert!(
            GLOSSY_OVERLAY_SHADER_SOURCE.contains(water),
            "water's module unmodified"
        );
    }

    // The stream: id 54's world bindings at install, a vertex buffer never
    // written after.
    let level = squashed(include_str!("glossy_overlay/level.rs"));
    assert!(level.contains("&section.world_bindings,"));
    assert!(level.contains(
        "label:Some(\"WorldProbeIndexBuffer\"),contents:&bytes,usage:wgpu::BufferUsages::VERTEX,"
    ));
    for source in [
        include_str!("glossy_overlay.rs"),
        include_str!("glossy_overlay/level.rs"),
        include_str!("renderer_glossy_frame.rs"),
    ] {
        assert!(
            !squashed(source).contains("write_buffer(&self.world_probes")
                && !squashed(source).contains("world_probes(),0,")
        );
    }
}

// Acceptance row 49 (source half), pins P15 and P23. The guard covers the
// pixels the mover colour pass shades: the mover pass's own `vs_depth` from
// its own module and layout, Equal without a depth write. It marks D_static's
// alpha alone, which forward clears to 0 and writes as 0 on every frame it
// writes D_static, so the mark is this frame's movers and nothing earlier.
// The overlay reads the mark at its fragment's own pixel. The guard binds
// nothing but the camera and the mover transforms: no probe, no sums (IP
// fence: no sums on movers).
#[test]
fn the_mover_guard_marks_this_frames_movers_through_their_own_clip_path() {
    let coverage = squashed(include_str!("kinematic_brush/coverage.rs"));
    for pin in [
        "layout:Some(&self.clip_layout),",
        "module:&self.clip_module,entry_point:Some(\"vs_depth\"),",
        "buffers:&[Some(mover_vertex_layout())],",
        "primitive:mover_primitive_state(),",
        "depth_write_enabled:Some(false),depth_compare:Some(wgpu::CompareFunction::Equal),",
        "self.record_clip(pass,level,MoverView::Main,pipeline);",
    ] {
        assert!(coverage.contains(pin), "coverage.rs pins {pin}");
    }
    let movers = squashed(include_str!("kinematic_brush.rs"));
    assert!(movers.contains("clip_module:shader,clip_layout:depth_pipeline_layout,"));
    assert!(movers.contains("module:&shader,entry_point:Some(\"vs_depth\"),"));
    assert!(movers.contains("self.record_clip(pass,level,view,&self.depth_pipeline);"));

    let guard = squashed(include_str!("glossy_overlay/mover_guard.rs"));
    assert!(guard.contains("write_mask:wgpu::ColorWrites::ALPHA,"));
    assert!(guard.contains("blend:None,"));
    assert!(guard.contains("load:wgpu::LoadOp::Load,store:wgpu::StoreOp::Store,"));
    assert!(guard.contains("depth_ops:None,stencil_ops:None,"));
    let shader = squashed(GLOSSY_MOVER_GUARD_SHADER);
    assert_eq!(
        shader,
        "@fragmentfnfs_mover_guard()->@location(0)vec4<f32>{returnvec4<f32>(0.0,0.0,0.0,1.0);}"
    );
    assert!(
        !shader.contains("@group"),
        "the guard's fragment binds nothing"
    );

    let forward = squashed(wgsl_function(FORWARD_WGSL, "fs_main_static_diffuse"));
    assert!(forward.contains("vec4<f32>(shaded.static_diffuse,0.0)"));
    let forward_frame = squashed(include_str!("renderer_forward_frame.rs"));
    assert!(forward_frame.contains("load:wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),"));
    assert_eq!(
        overlay_fn("glossy_overlay_mover_covered"),
        "fnglossy_overlay_mover_covered(d_static:vec4<f32>)->bool{returnd_static.a>0.5;}"
    );
    for name in ["fluid_probe", "probe_sums", "reflection"] {
        assert!(
            !squashed(MOVER_WGSL).contains(name),
            "the mover shader reads {name}"
        );
    }
}

// Acceptance row 47 (source half): no planar layer reaches a world face at
// any tier. The overlay's shader, recording and level state name no planar
// resource, and the production composite takes no screen-space share
// (`vec4(0.0)`) until the floor layer lands.
#[test]
fn no_planar_layer_reaches_the_overlay() {
    for (file, source) in [
        ("glossy_overlay.wgsl", OVERLAY_WGSL),
        (
            "glossy_overlay_shading.wgsl",
            include_str!("../shaders/glossy_overlay_shading.wgsl"),
        ),
        ("glossy_overlay.rs", include_str!("glossy_overlay.rs")),
        (
            "glossy_overlay/level.rs",
            include_str!("glossy_overlay/level.rs"),
        ),
        (
            "glossy_overlay/pipeline.rs",
            include_str!("glossy_overlay/pipeline.rs"),
        ),
        (
            "glossy_overlay/mover_guard.rs",
            include_str!("glossy_overlay/mover_guard.rs"),
        ),
        (
            "renderer_glossy_frame.rs",
            include_str!("renderer_glossy_frame.rs"),
        ),
    ] {
        for (index, line) in source.lines().enumerate() {
            assert!(
                !names_planar(line),
                "{file}:{}: the overlay names a planar resource: `{}`",
                index + 1,
                line.trim()
            );
        }
    }
    assert!(overlay_fn("glossy_overlay_delta").contains(
        "returnglossy_composite(d_static.rgb,probe.capture,probe.weight,vec4<f32>(0.0),"
    ));
    assert!(names_planar("planar_layer") && !names_planar("coplanar mover"));
}
