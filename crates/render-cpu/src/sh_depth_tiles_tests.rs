use super::*;
use postretro_level_format::probe_depth_tiles::{
    MomentSecondChannel, PROBE_DEPTH_TILE_TEXELS, decode_moment_second_channel,
    probe_depth_tile_texture_bytes,
};

fn section(probe_indices: &[u32]) -> ProbeDepthTilesSection {
    let texels = (0..probe_indices.len() * PROBE_DEPTH_TILE_TEXELS)
        .map(|i| [i as u16, (i as u16) ^ 0x5555])
        .collect();
    ProbeDepthTilesSection {
        probe_indices: probe_indices.to_vec(),
        texels,
    }
}

#[test]
fn references_pack_tiled_probes_as_flagged_tile_and_untiled_as_their_mean_square() {
    let references = ProbeDepthTileReferences::from_section(Some(&section(&[3, 9, 40])), 0);
    assert_eq!(references.tile_count(), 3);
    assert_eq!(
        decode_moment_second_channel(references.second_channel(9, 0x4900)),
        MomentSecondChannel::Tile(1)
    );
    assert_eq!(
        decode_moment_second_channel(references.second_channel(40, 0x4900)),
        MomentSecondChannel::Tile(2)
    );
    assert_eq!(references.second_channel(4, 0x4900), 0x4900);
    // +0 stays untiled: the flag is the raw sign bit, not the float's sign.
    assert_eq!(
        decode_moment_second_channel(references.second_channel(4, 0x0000)),
        MomentSecondChannel::Averaged(0)
    );
}

#[test]
fn absent_and_zero_tile_sections_give_identical_references_words_and_texture() {
    let absent = ProbeDepthTileReferences::from_section(None, 0);
    let zero = ProbeDepthTileReferences::from_section(Some(&section(&[])), 0);
    assert_eq!(absent, zero);
    assert_eq!(absent.texture_layout(), None);
    assert_eq!(
        DepthTileGridWords::for_layout(absent.texture_layout()),
        DepthTileGridWords::NONE
    );
    let absent_plan = depth_tile_texture_plan(None, 0);
    let zero_plan = depth_tile_texture_plan(Some(&section(&[])), 0);
    assert_eq!(absent_plan.extent, [1, 1]);
    assert_eq!(absent_plan.extent, zero_plan.extent);
    assert_eq!(absent_plan.texels, zero_plan.texels);
}

#[test]
fn texture_plan_places_each_bordered_tile_at_its_layout_origin() {
    let tiles = section(&[0, 5, 6, 7, 11]);
    let layout = tiles.texture_layout().unwrap();
    let plan = depth_tile_texture_plan(Some(&tiles), 0);
    assert_eq!(plan.extent, layout.extent());
    assert_eq!(
        plan.texels.len() as u64 * 4,
        probe_depth_tile_texture_bytes(5),
        "the plan uploads exactly the bytes the ledger prices"
    );
    let width = plan.extent[0] as usize;
    let edge = PROBE_DEPTH_TILE_BORDERED_EDGE as usize;
    for tile in 0..tiles.tile_count() {
        let [ox, oy] = layout.tile_origin(tile as u32);
        for row in 0..edge {
            for column in 0..edge {
                assert_eq!(
                    plan.texels[(oy as usize + row) * width + ox as usize + column],
                    tiles.tile(tile)[row * edge + column],
                    "tile {tile} texel ({column}, {row})"
                );
            }
        }
    }
}

/// Mover live tiles (id 57) follow id 59's in one texture sized from their
/// sum: id 59's tiles keep their origins, live slots start zero, and a
/// reference may name any of them. Live tiles alone size the texture too.
#[test]
fn live_tiles_follow_id_59s_in_one_texture_sized_from_their_sum() {
    let tiles = section(&[0, 5, 6]);
    let plan = depth_tile_texture_plan(Some(&tiles), 4);
    let layout = ProbeDepthTileTextureLayout::for_tile_count(7).unwrap();
    assert_eq!(plan.extent, layout.extent());
    assert_eq!(
        plan.texels.len() as u64 * 4,
        probe_depth_tile_texture_bytes(7)
    );
    let width = plan.extent[0] as usize;
    let edge = PROBE_DEPTH_TILE_BORDERED_EDGE as usize;
    let texel = |tile: u32, row: usize, column: usize| {
        let [ox, oy] = layout.tile_origin(tile);
        plan.texels[(oy as usize + row) * width + ox as usize + column]
    };
    for row in 0..edge {
        for column in 0..edge {
            assert_eq!(texel(2, row, column), tiles.tile(2)[row * edge + column]);
            for live in 3..7 {
                assert_eq!(texel(live, row, column), [0, 0], "live tile {live}");
            }
        }
    }
    let mut references = ProbeDepthTileReferences::from_section(Some(&tiles), 4);
    assert_eq!(references.tile_count(), 7);
    assert_eq!(references.tile_of(5), Some(1), "id 59's references stand");
    assert_eq!(
        references.tile_of(9),
        None,
        "no probe starts on a live tile"
    );
    references.set_reference(9, Some(6)).unwrap();
    assert_eq!(
        references.set_reference(9, Some(7)),
        Err(DepthTileReferenceError::TileOutOfRange {
            tile: 7,
            tile_count: 7
        })
    );

    let live_only = ProbeDepthTileReferences::from_section(None, 2);
    assert_eq!(live_only.tile_count(), 2);
    assert_eq!(
        live_only.texture_layout(),
        ProbeDepthTileTextureLayout::for_tile_count(2)
    );
    assert_eq!(
        depth_tile_texture_plan(None, 2).extent,
        ProbeDepthTileTextureLayout::for_tile_count(2)
            .unwrap()
            .extent()
    );
}

/// Renderer side: id 59's tiles plus live tiles exactly at the
/// reference's range lay out, and the last live tile is referenceable.
#[test]
fn id_59_and_live_tiles_exactly_at_the_range_lay_out_and_the_last_is_referenceable() {
    let id59 = MAX_PROBE_DEPTH_TILES - 3;
    let tiles = ProbeDepthTilesSection {
        probe_indices: (0..id59).collect(),
        texels: vec![[0, 0]; id59 as usize * PROBE_DEPTH_TILE_TEXELS],
    };
    let mut references = ProbeDepthTileReferences::from_section(Some(&tiles), 3);
    assert_eq!(references.tile_count(), MAX_PROBE_DEPTH_TILES);
    assert_eq!(
        references.texture_layout(),
        ProbeDepthTileTextureLayout::for_tile_count(MAX_PROBE_DEPTH_TILES)
    );
    let last = (MAX_PROBE_DEPTH_TILES - 1) as u16;
    references.set_reference(id59 + 7, Some(last)).unwrap();
    assert_eq!(
        decode_moment_second_channel(references.second_channel(id59 + 7, 0x4000)),
        MomentSecondChannel::Tile(last)
    );
    let plan = depth_tile_texture_plan(Some(&tiles), 3);
    assert_eq!(
        plan.texels.len() as u64 * 4,
        probe_depth_tile_texture_bytes(MAX_PROBE_DEPTH_TILES)
    );
}

#[test]
fn grid_words_carry_row_shift_and_reciprocal_extent() {
    let layout = ProbeDepthTileTextureLayout::for_tile_count(MAX_PROBE_DEPTH_TILES).unwrap();
    assert_eq!(layout.extent(), [2560, 1280]);
    let words = DepthTileGridWords::for_layout(Some(layout));
    assert_eq!(words.row_shift, 8);
    assert_eq!(words.inv_width, 1.0 / 2560.0);
    assert_eq!(words.inv_height, 1.0 / 1280.0);
}

#[test]
fn rewrite_redirects_adds_and_clearing_restores_the_mean_square() {
    let mut references = ProbeDepthTileReferences::from_section(Some(&section(&[3, 9])), 0);
    references.set_reference(9, Some(0)).unwrap();
    assert_eq!(references.tile_of(9), Some(0));
    // A probe that had no tile takes one.
    references.set_reference(5, Some(1)).unwrap();
    assert_eq!(references.tile_of(5), Some(1));
    references.set_reference(3, None).unwrap();
    assert_eq!(references.second_channel(3, 0x4321), 0x4321);
    assert_eq!(
        references.set_reference(5, Some(2)),
        Err(DepthTileReferenceError::TileOutOfRange {
            tile: 2,
            tile_count: 2
        })
    );
    assert_eq!(
        references.tile_of(5),
        Some(1),
        "a refused rewrite changes nothing"
    );
}

#[test]
fn grid_info_carries_tile_words_in_the_former_pad_slots_and_keeps_96_bytes() {
    use crate::sh_volume::{SH_GRID_INFO_SIZE, ShGridInfoParams, build_grid_info_bytes};
    let words = DepthTileGridWords::for_layout(ProbeDepthTileTextureLayout::for_tile_count(5));
    let params = |depth_tiles| ShGridInfoParams {
        grid_origin: [0.0; 3],
        cell_size: [1.0; 3],
        grid_dimensions: [2, 2, 2],
        atlas_dimensions: [6, 6],
        tile_dimension: 6,
        tile_border: 1,
        atlas_tiles_per_row: 1,
        physical_tile_stride: 6,
        tiles_per_layer: 1,
        atlas_layer_count: 1,
        present: true,
        probe_occlusion_enabled: true,
        depth_tiles,
    };
    let bytes = build_grid_info_bytes(params(words));
    assert_eq!(bytes.len(), SH_GRID_INFO_SIZE);
    assert_eq!(u32::from_ne_bytes(bytes[28..32].try_into().unwrap()), 2);
    assert_eq!(
        f32::from_ne_bytes(bytes[44..48].try_into().unwrap()),
        1.0 / 40.0
    );
    assert_eq!(
        f32::from_ne_bytes(bytes[92..96].try_into().unwrap()),
        1.0 / 20.0
    );
    let none = build_grid_info_bytes(params(DepthTileGridWords::NONE));
    for range in [28..32, 44..48, 92..96] {
        assert!(none[range].iter().all(|&b| b == 0));
    }
    assert_eq!(bytes[..28], none[..28]);
    assert_eq!(bytes[32..44], none[32..44]);
    assert_eq!(bytes[48..92], none[48..92]);
}
