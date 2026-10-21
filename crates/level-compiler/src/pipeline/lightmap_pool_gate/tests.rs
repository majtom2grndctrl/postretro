//! Runtime pool count and compiler/section/renderer placement seam proofs.

use glam::Vec3;
use log::Level;
use postretro_level_format::lightmap::LightmapBlockIndex;
use postretro_test_log_capture::LogCapture;

use super::*;
use crate::lightmap_bake::{BlockSectionBuilder, CellBlock, CompositedAtlas};

fn prepared(extents: &[(u32, u32)], scale: u32, bake_layers: u32) -> PreparedAtlas {
    PreparedAtlas {
        charts: Vec::new(),
        placements: Vec::new(),
        atlas_width: LIGHTMAP_POOL_LAYER_EDGE,
        atlas_height: LIGHTMAP_POOL_LAYER_EDGE,
        layer_count: bake_layers,
        layout: BlockLayout {
            direction_texel_scale: scale,
            blocks: extents
                .iter()
                .enumerate()
                .map(|(id, &(width, height))| CellBlock {
                    cell_id: id as u32,
                    width,
                    height,
                    layer: id as u32,
                    x: 0,
                    y: 0,
                })
                .collect(),
            chart_blocks: Vec::new(),
        },
        cut_parents: Vec::new(),
    }
}

#[test]
fn pool_gate_refuses_unstreamable_257_and_warns_streamable_257_by_name() {
    let extents = vec![(LIGHTMAP_POOL_LAYER_EDGE, LIGHTMAP_POOL_LAYER_EDGE); 257];
    let prepared = prepared(&extents, 8, 300);
    let capture = LogCapture::start();
    let error = gate_runtime_pool(&prepared, true, false)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Lightmap runtime all-resident pool"),
        "{error}"
    );
    assert!(error.contains("257 pool layers"), "{error}");
    assert!(
        error.contains("256-layer runtime array-layer floor"),
        "{error}"
    );
    assert!(error.contains("cannot stream"), "{error}");
    assert!(capture.records().is_empty(), "refusal does not also warn");

    assert_eq!(gate_runtime_pool(&prepared, true, true).unwrap(), 257);
    capture.assert_logged_once(Level::Warn, "Lightmap runtime all-resident pool");
    capture.assert_logged(Level::Warn, "257 pool layers");
    capture.assert_logged(Level::Warn, "256-layer runtime array-layer floor");
    capture.assert_logged(Level::Warn, "POSTRETRO_LIGHTMAP_STREAMING=all-resident");
}

#[test]
fn exact_runtime_floor_is_quiet_even_past_256_internal_bake_layers() {
    let extents = vec![(LIGHTMAP_POOL_LAYER_EDGE, LIGHTMAP_POOL_LAYER_EDGE); 256];
    let prepared = prepared(&extents, 8, 300);
    assert!(prepared.layer_count > RUNTIME_MAX_TEXTURE_ARRAY_LAYERS);
    let capture = LogCapture::start();
    for can_stream in [false, true] {
        assert_eq!(gate_runtime_pool(&prepared, true, can_stream).unwrap(), 256);
    }
    assert!(capture.records().is_empty());
}

#[test]
fn pool_gate_is_quiet_without_static_lights_or_blocks() {
    let extents = vec![(LIGHTMAP_POOL_LAYER_EDGE, LIGHTMAP_POOL_LAYER_EDGE); 257];
    let unconsumed = prepared(&extents, 8, 300);
    let empty = prepared(&[], 8, 300);
    let capture = LogCapture::start();
    for can_stream in [false, true] {
        assert_eq!(
            gate_runtime_pool(&unconsumed, false, can_stream).unwrap(),
            0
        );
        assert_eq!(gate_runtime_pool(&empty, true, can_stream).unwrap(), 0);
    }
    assert!(capture.records().is_empty());
}

#[test]
fn scale8_unaligned_extents_match_rounded_placements_and_require_four_layers() {
    let unaligned = [(21, 64); 7];
    let rounded = [(24, 64); 7];
    let actual = place_all_resident(&unaligned, 8, 64).unwrap();
    assert_eq!(actual, place_all_resident(&rounded, 8, 64).unwrap());
    assert_eq!(actual.layer_count, 4);
    assert_eq!(
        place_all_resident(&unaligned, 1, 64).unwrap().layer_count,
        3
    );
}

#[test]
fn compiler_count_matches_renderer_on_id22_built_from_its_finalized_scale8_layout() {
    // Seven blocks deliberately occupy seven internal bake layers, while the
    // runtime shelf allocator fits two per layer. Confusing the two counts
    // fails this seam proof. Only one raw plane is held, with no ray work.
    let mut prepared = prepared(&[(688, LIGHTMAP_POOL_LAYER_EDGE); 7], 8, 7);
    prepared.atlas_width = 688;
    let count = runtime_pool_layers(&prepared.layout).unwrap();
    assert_eq!(count, 4);
    assert_ne!(count, prepared.layer_count);
    let texels = prepared.atlas_width as usize * prepared.atlas_height as usize;
    let plane = CompositedAtlas {
        irradiance: vec![0.0; texels * 4],
        direction: vec![Vec3::Y; texels],
        coverage: vec![true; texels],
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layer_count: 1,
    };
    let mut builder = BlockSectionBuilder::new(&prepared.layout, true);
    for layer in 0..prepared.layer_count {
        builder.push_layer(layer, &plane);
    }
    let section = builder.finish();
    drop(plane);
    let bytes = section.to_bytes();
    let index = LightmapBlockIndex::from_prefix(&bytes, bytes.len() as u64).unwrap();
    assert_eq!(index.header.direction_texel_scale, 8);
    let extents: Vec<_> = index
        .records
        .iter()
        .map(|b| (u32::from(b.width), u32::from(b.height)))
        .collect();
    assert_eq!(extents, vec![(688, LIGHTMAP_POOL_LAYER_EDGE); 7]);
    let renderer = postretro_render_cpu::lightmap_pool::place_all_resident(
        &extents,
        index.header.block_alignment(),
        LIGHTMAP_POOL_LAYER_EDGE,
    )
    .unwrap();
    assert_eq!(count, renderer.layer_count);
    assert_eq!(renderer.layer_count, 4);
}
