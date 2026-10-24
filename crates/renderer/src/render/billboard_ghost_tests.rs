// Billboard ghost shader contract: naga validation, the instance stride, the quad and
// sampling copies pinned to the billboard pass's, and the order-independent blend.
// See: context/lib/rendering_pipeline.md §7.9, §7.4, §8

use super::super::kinematic_ghost::tests::{bound_groups, struct_span, validate, wgsl_fn};
use super::*;

const BILLBOARD_WGSL: &str = include_str!("../shaders/billboard.wgsl");

#[test]
fn billboard_ghost_wgsl_validates_and_binds_no_lighting() {
    let module = validate(BILLBOARD_GHOST_SHADER_SOURCE);
    assert_eq!(
        bound_groups(&module),
        [0, 1, 2].into_iter().collect(),
        "camera, sprite sheet and ghost sprites only"
    );
    assert_eq!(
        struct_span(&module, "GhostSpriteInstance") as usize,
        GHOST_SPRITE_INSTANCE_SIZE
    );
}

// A ghost quad covers and samples exactly what its projectile's billboard does.
#[test]
fn billboard_ghost_quad_and_sampling_match_the_billboard_pass() {
    for function in [
        "corner_for_vertex",
        "corner_data",
        "camera_right_up",
        "sample_post_retro",
    ] {
        assert_eq!(
            code_only(wgsl_fn(BILLBOARD_GHOST_SHADER_SOURCE, function)),
            code_only(wgsl_fn(BILLBOARD_WGSL, function)),
            "{function}"
        );
    }
    for line in [
        "let frame_count = max(bitcast<u32>(draw_params.params.x), 1u);",
        "let lifetime = max(draw_params.params.z, 1.0e-6);",
        "let frame_duration = lifetime / f32(frame_count);",
        "let frame_idx = u32(floor(age / max(frame_duration, 1.0e-6))) % frame_count;",
    ] {
        assert!(BILLBOARD_WGSL.contains(line), "billboard: {line}");
        assert!(
            BILLBOARD_GHOST_SHADER_SOURCE.contains(line),
            "ghost: {line}"
        );
    }
}

/// A function's code with comments and blank lines dropped.
fn code_only(source: &str) -> Vec<&str> {
    source
        .lines()
        .map(|line| line.split("//").next().unwrap_or("").trim_end())
        .filter(|line| !line.trim().is_empty())
        .collect()
}

// Projectile billboard ghosts blend in both `render.ghosts` modes, additively
// like every billboard, so instance order changes no pixel.
#[test]
fn billboard_ghosts_blend_additively_and_order_independently() {
    for component in [BILLBOARD_GHOST_BLEND.color, BILLBOARD_GHOST_BLEND.alpha] {
        assert_eq!(component.src_factor, wgpu::BlendFactor::One);
        assert_eq!(component.dst_factor, wgpu::BlendFactor::One);
        assert_eq!(component.operation, wgpu::BlendOperation::Add);
    }
}
