// Ghost pipeline state contract: each mode's colour draw and depth-only draw.
// See: context/lib/rendering_pipeline.md §7.9

use super::*;

const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

// Each mode's colour draw writes colour, tests `Equal` against the ghost
// depth target and never writes depth, so it draws only the ghost's
// nearest surface per pixel and leaves scene depth to the opaque passes.
#[test]
fn ghost_mode_selects_its_fragment_and_blend_and_tests_ghost_depth_equal_without_writing() {
    let blend = ghost_pipeline_state(GhostTransparency::Blend, DEPTH);
    assert_eq!(blend.fragment_entry, "fs_blend");
    let color = blend.blend.expect("blend mode blends").color;
    assert_eq!(color.src_factor, wgpu::BlendFactor::SrcAlpha);
    assert_eq!(color.dst_factor, wgpu::BlendFactor::OneMinusSrcAlpha);

    let dither = ghost_pipeline_state(GhostTransparency::Dither, DEPTH);
    assert_eq!(dither.fragment_entry, "fs_dither");
    assert_eq!(dither.blend, None, "screen-door draws opaque survivors");

    for state in [blend, dither] {
        assert_eq!(state.write_mask, wgpu::ColorWrites::ALL);
        assert_eq!(state.depth.depth_write_enabled, Some(false));
        assert_eq!(
            state.depth.depth_compare,
            Some(wgpu::CompareFunction::Equal)
        );
        assert_eq!(state.cull_mode, Some(wgpu::Face::Back));
    }
}

// A ghost's depth-only draw writes depth, tests `Less` so its nearest surface
// wins, writes no colour, and culls as its colour draw does; the colour draw
// then tests `Equal` against it without writing depth. Blend's depth entry
// writes every fragment; dither's discards the screen door's dropped pixels.
#[test]
fn ghost_depth_draw_writes_nearest_depth_and_no_colour_and_the_colour_draw_tests_it_equal() {
    for mode in GhostTransparency::ALL {
        let depth = ghost_depth_pipeline_state(mode, DEPTH);
        let colour = ghost_pipeline_state(mode, DEPTH);
        assert_eq!(depth.depth.format, DEPTH);
        assert_eq!(depth.depth.depth_write_enabled, Some(true), "{mode:?}");
        assert_eq!(
            depth.depth.depth_compare,
            Some(wgpu::CompareFunction::Less),
            "{mode:?}"
        );
        assert_eq!(depth.write_mask, wgpu::ColorWrites::empty(), "{mode:?}");
        assert_eq!(depth.blend, None, "{mode:?}");
        assert_eq!(depth.depth.bias, colour.depth.bias, "{mode:?}");
        assert_eq!(depth.cull_mode, colour.cull_mode, "{mode:?}");
        assert_eq!(colour.depth.format, depth.depth.format);
        assert_eq!(colour.depth.depth_write_enabled, Some(false), "{mode:?}");
        assert_eq!(
            colour.depth.depth_compare,
            Some(wgpu::CompareFunction::Equal),
            "{mode:?}"
        );
    }
    assert_eq!(
        ghost_depth_pipeline_state(GhostTransparency::Blend, DEPTH).fragment_entry,
        "fs_blend_depth"
    );
    assert_eq!(
        ghost_depth_pipeline_state(GhostTransparency::Dither, DEPTH).fragment_entry,
        "fs_dither_depth"
    );
}
