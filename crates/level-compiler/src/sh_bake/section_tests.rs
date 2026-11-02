// Tests for SH section assembly: compact atlas, layer-major spill, atlas cap.
// See: context/lib/build_pipeline.md §Compiler pipeline

use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION, MAX_SH_ATLAS_DIMENSION,
    MAX_SH_ATLAS_LAYERS, irradiance_array_tile_location, irradiance_atlas_array_layout,
};
use postretro_level_format::sh_volume::{
    OCTAHEDRAL_PROBE_STRIDE, OctahedralAtlasTexel, OctahedralShProbe, OctahedralShVolumeSection,
};

use super::section::*;
use super::*;

#[test]
fn compact_atlas_excludes_invalid_tiles_in_x_fastest_order() {
    let dense_layout = sh_atlas_array_layout([3, 1, 1], DEFAULT_IRRADIANCE_TILE_DIMENSION)
        .expect("three-probe dense layout");
    let dense_texel_count = dense_layout.layer_count as usize
        * dense_layout.atlas_width as usize
        * dense_layout.atlas_height as usize;
    let mut dense_atlas = vec![OctahedralAtlasTexel::default(); dense_texel_count];
    let dense_width = dense_layout.atlas_width as usize;
    let dense_layer_texels = dense_width * dense_layout.atlas_height as usize;
    let tile_dimension = DEFAULT_IRRADIANCE_TILE_DIMENSION as usize;

    for probe_index in 0..3usize {
        let [layer, tile_x, tile_y] = irradiance_array_tile_location(
            probe_index,
            dense_layout.tiles_per_layer,
            dense_layout.atlas_tiles_per_row,
        );
        let value = (probe_index as u16) + 10;
        let layer_offset = layer as usize * dense_layer_texels;
        for tile_y_offset in 0..tile_dimension {
            let row = layer_offset
                + (tile_y as usize * tile_dimension + tile_y_offset) * dense_width
                + tile_x as usize * tile_dimension;
            dense_atlas[row..row + tile_dimension].fill(OctahedralAtlasTexel {
                rgba: [value, value, value, value],
            });
        }
    }

    let section = OctahedralShVolumeSection {
        grid_origin: [0.0; 3],
        cell_size: [1.0; 3],
        grid_dimensions: [3, 1, 1],
        probe_stride: OCTAHEDRAL_PROBE_STRIDE,
        tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
        tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
        atlas_dimensions: [dense_layout.atlas_width, dense_layout.atlas_height],
        layer_count: dense_layout.layer_count,
        tiles_per_layer: dense_layout.tiles_per_layer,
        atlas_tiles_per_row: dense_layout.atlas_tiles_per_row,
        probes: vec![
            OctahedralShProbe {
                validity: 1,
                mean_distance: 0,
                mean_sq_distance: 0,
                density_level: 0,
                node_scale: 0,
            },
            OctahedralShProbe {
                validity: 0,
                mean_distance: 0,
                mean_sq_distance: 0,
                density_level: 0,
                node_scale: 0,
            },
            OctahedralShProbe {
                validity: 1,
                mean_distance: 0,
                mean_sq_distance: 0,
                density_level: 0,
                node_scale: 0,
            },
        ],
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        compact_atlas: Vec::new(),
        animation_descriptors: Vec::new(),
        slot_for_map_light: Vec::new(),
    };

    let compact = compact_section_from_dense_atlas(section, &dense_atlas);
    let tile_bytes = tile_dimension * tile_dimension * 8;
    assert_eq!(
        compact.compact_atlas.len(),
        2 * tile_bytes,
        "two valid probes must contribute exactly two 288-byte tiles before encoding",
    );
    assert_eq!(compact.atlas_dimensions, [12, 6]);
    assert_eq!(compact.atlas_tiles_per_row, 2);
    assert_eq!(compact.tiles_per_layer, 2);
    assert_eq!(compact.layer_count, 1);
    assert_eq!(
        u16::from_le_bytes([compact.compact_atlas[0], compact.compact_atlas[1]]),
        10,
        "first compact slot must retain global probe 0",
    );
    assert_eq!(
        u16::from_le_bytes([
            compact.compact_atlas[tile_dimension * 8],
            compact.compact_atlas[tile_dimension * 8 + 1],
        ]),
        12,
        "second compact slot must skip invalid global probe 1 and retain probe 2",
    );

    let bc6h = encode_sh_volume_section_bc6h(&compact, false);
    assert_eq!(bc6h.irradiance_format, IRRADIANCE_FORMAT_BC6H);
    assert_eq!(bc6h.compact_atlas.len(), 3 * 2 * 16);
    assert_eq!(encode_sh_volume_section_bc6h(&compact, true), compact);
}

#[test]
fn synthetic_probe_grid_spills_to_deterministic_layer_major_atlas() {
    let grid = [1_900_000, 1, 1];
    let tile_dimension = DEFAULT_IRRADIANCE_TILE_DIMENSION;
    let layout = sh_atlas_array_layout(grid, tile_dimension).unwrap();
    let direct_layout =
        irradiance_atlas_array_layout(grid, tile_dimension, MAX_SH_ATLAS_DIMENSION).unwrap();

    assert_eq!(layout, direct_layout);
    assert!(layout.layer_count > 1);
    assert!(layout.atlas_width <= MAX_SH_ATLAS_DIMENSION);
    assert!(layout.atlas_height <= MAX_SH_ATLAS_DIMENSION);

    let total = grid[0] as usize * grid[1] as usize * grid[2] as usize;
    let mut seen = vec![false; layout.layer_count as usize * layout.tiles_per_layer as usize];
    for probe_index in 0..total {
        let [layer, tile_x, tile_y] = irradiance_array_tile_location(
            probe_index,
            layout.tiles_per_layer,
            layout.atlas_tiles_per_row,
        );
        assert!(layer < layout.layer_count);
        assert!(tile_x * tile_dimension + tile_dimension <= layout.atlas_width);
        assert!(tile_y * tile_dimension + tile_dimension <= layout.atlas_height);

        let flat_tile = layer as usize * layout.tiles_per_layer as usize
            + tile_y as usize * layout.atlas_tiles_per_row as usize
            + tile_x as usize;
        assert!(
            !seen[flat_tile],
            "probe {probe_index} reused atlas tile {flat_tile}"
        );
        seen[flat_tile] = true;
    }

    let second = sh_atlas_array_layout(grid, tile_dimension).unwrap();
    assert_eq!(layout, second);
    assert_eq!(seen.into_iter().filter(|&used| used).count(), total);
}

#[test]
fn compiler_and_streaming_codec_share_the_format_owned_atlas_cap() {
    assert_eq!(
        MAX_SH_ATLAS_DIMENSION,
        postretro_level_format::octahedral::MAX_SH_ATLAS_DIMENSION
    );
    let slots = [257, 1, 1];
    assert_eq!(
        sh_atlas_array_layout(slots, 8).unwrap(),
        postretro_level_format::cluster_sh_payloads::cluster_sh_isolated_atlas_array_layout(
            slots[0]
        )
        .unwrap()
    );
}

#[test]
fn sh_atlas_array_layout_rejects_grids_above_layer_cap() {
    let max_tiles_per_axis = MAX_SH_ATLAS_DIMENSION / DEFAULT_IRRADIANCE_TILE_DIMENSION;
    let tiles_per_layer = max_tiles_per_axis * max_tiles_per_axis;
    let grid = [tiles_per_layer * MAX_SH_ATLAS_LAYERS + 1, 1, 1];

    let err = sh_atlas_array_layout(grid, DEFAULT_IRRADIANCE_TILE_DIMENSION).unwrap_err();
    match err {
        ShAtlasBakeError::LayerOverflow {
            grid_dimensions,
            max_dim,
            max_layers,
        } => {
            assert_eq!(grid_dimensions, grid);
            assert_eq!(max_dim, MAX_SH_ATLAS_DIMENSION);
            assert_eq!(max_layers, MAX_SH_ATLAS_LAYERS);
        }
    }
}
