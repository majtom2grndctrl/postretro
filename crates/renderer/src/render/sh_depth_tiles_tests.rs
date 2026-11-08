use glam::Vec3;
use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::octahedral::irradiance_interior_texel_direction;
use postretro_level_format::probe_depth_tiles::{
    MAX_PROBE_DEPTH_TILES, MomentSecondChannel, PROBE_DEPTH_TILE_BORDER,
    PROBE_DEPTH_TILE_BORDERED_EDGE, PROBE_DEPTH_TILE_EDGE, PROBE_DEPTH_TILE_INTERIOR_TEXELS,
    PROBE_DEPTH_TILE_TEXELS, ProbeDepthTilesSection, bordered_tile, decode_moment_second_channel,
    f16_bits_to_f32, probe_depth_tile_texture_bytes, sample_tile_moments,
};
use postretro_level_format::sh_volume::OctahedralShProbe;
use postretro_render_cpu::sh_depth_tiles::{
    DepthTileGridWords, ProbeDepthTileReferences, depth_tile_texture_plan,
};
use postretro_render_cpu::sh_volume::{ShGridInfoParams, build_grid_info_bytes};

use super::harness::{Lattice, Query, run};
use crate::render::gpu_test_harness::{gpu_or_skip, offscreen_renderer_or_skip};
use crate::render::sh_volume::pack_probe_depth_moments;

fn probe(mean: f32, mean_sq_bits: u16) -> OctahedralShProbe {
    OctahedralShProbe {
        validity: 1,
        mean_distance: f32_to_f16_bits(mean),
        mean_sq_distance: mean_sq_bits,
        ..Default::default()
    }
}

/// `[R, G]` of each probe's moment texel, as the whole-load packer writes it.
fn packed_rg(
    probes: &[OctahedralShProbe],
    grid: [u32; 3],
    tiles: Option<&ProbeDepthTilesSection>,
) -> Vec<[u16; 2]> {
    let words: Vec<u32> = probes
        .iter()
        .map(|probe| if probe.validity != 0 { 4 } else { 0 })
        .collect();
    pack_probe_depth_moments(
        probes,
        grid,
        &words,
        &ProbeDepthTileReferences::from_section(tiles),
    )
    .as_chunks::<4>()
    .0
    .iter()
    .map(|&[r, g, _, _]| [r, g])
    .collect()
}

/// A tile whose bordered texels all hold `(mean, mean_sq)`.
fn uniform_tile(mean: f32, mean_sq: f32) -> Vec<[u16; 2]> {
    vec![[f32_to_f16_bits(mean), f32_to_f16_bits(mean_sq)]; PROBE_DEPTH_TILE_TEXELS]
}

/// A tile whose interior texel `(x, y)` has mean `0.25 · (1 + x + 8y)`.
fn directional_tile() -> Vec<[u16; 2]> {
    let interior: Vec<[u16; 2]> = (0..PROBE_DEPTH_TILE_INTERIOR_TEXELS)
        .map(|i| {
            let mean = 0.25 * (1 + i) as f32;
            [f32_to_f16_bits(mean), f32_to_f16_bits(mean * mean + 0.5)]
        })
        .collect();
    bordered_tile(&interior)
}

fn section(probe_indices: Vec<u32>, tiles: Vec<Vec<[u16; 2]>>) -> ProbeDepthTilesSection {
    ProbeDepthTilesSection {
        probe_indices,
        texels: tiles.concat(),
    }
}

#[test]
fn whole_load_packer_points_a_tiled_probe_at_its_tile_and_keeps_e_d_in_r() {
    let mut probes = vec![probe(1.0, 0x3d00), probe(2.0, 0x4400), probe(3.0, 0x4880)];
    probes.push(OctahedralShProbe {
        validity: 0,
        ..probe(4.0, 0x4c00)
    });
    let tiles = section(vec![1, 2], vec![uniform_tile(1.0, 1.0); 2]);
    let packed = packed_rg(&probes, [4, 1, 1], Some(&tiles));

    assert_eq!(packed[0], [probes[0].mean_distance, 0x3d00], "untiled");
    for (probe_index, tile) in [(1usize, 0u16), (2, 1)] {
        let [r, g] = packed[probe_index];
        assert_eq!(r, probes[probe_index].mean_distance, "R stays id-34 E[d]");
        assert_eq!(
            decode_moment_second_channel(g),
            MomentSecondChannel::Tile(tile)
        );
    }
    assert_eq!(packed[3], [0, 0], "an invalid probe packs zero");
}

/// A PRL without id 59 and one with a zero-tile id 59 hand the GPU the same
/// moment texels, the same `ShGridInfo` bytes and the same 1×1 tile dummy.
#[test]
fn absent_and_zero_tile_id59_produce_identical_gpu_inputs() {
    let probes = vec![probe(1.0, 0x3d00), probe(0.5, 0x0000), probe(2.0, 0x4400)];
    let grid = [3, 1, 1];
    let zero = section(Vec::new(), Vec::new());
    assert_eq!(
        packed_rg(&probes, grid, None),
        packed_rg(&probes, grid, Some(&zero))
    );
    let grid_bytes = |tiles: Option<&ProbeDepthTilesSection>| {
        build_grid_info_bytes(ShGridInfoParams {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: grid,
            atlas_dimensions: [6, 6],
            tile_dimension: 6,
            tile_border: 1,
            atlas_tiles_per_row: 1,
            physical_tile_stride: 6,
            tiles_per_layer: 1,
            atlas_layer_count: 1,
            present: true,
            probe_occlusion_enabled: true,
            depth_tiles: DepthTileGridWords::for_layout(
                ProbeDepthTileReferences::from_section(tiles).texture_layout(),
            ),
        })
    };
    assert_eq!(grid_bytes(None), grid_bytes(Some(&zero)));
    let (absent, zero_plan) = (
        depth_tile_texture_plan(None),
        depth_tile_texture_plan(Some(&zero)),
    );
    assert_eq!(
        (absent.extent, &absent.texels),
        (zero_plan.extent, &zero_plan.texels)
    );
    assert_eq!(absent.extent, [1, 1]);
}

/// Every depth-aware path weighs depth through `sh_corner_depth_visibility`,
/// and only its one Chebyshev core applies the floor.
#[test]
fn every_depth_aware_path_weighs_depth_through_the_shared_decode() {
    use crate::render::gpu_test_harness::wgsl_function;
    const SH_SAMPLE: &str = include_str!("../shaders/sh_sample.wgsl");
    for path in [
        "sample_sh_indirect_corners_pair",
        "sample_l1_whole_cell_atlas",
        "sh_whole_cell_weight_sum",
        "sample_sh_indirect_direct_corners",
    ] {
        assert!(
            wgsl_function(SH_SAMPLE, path).contains("sh_probe_weight("),
            "{path} must weigh corners through sh_probe_weight"
        );
    }
    assert!(wgsl_function(SH_SAMPLE, "sh_probe_weight").contains("sh_corner_depth_visibility("));
    // Defined once, called once: no path reaches the moments another way.
    assert_eq!(SH_SAMPLE.matches("sh_corner_depth_visibility(").count(), 2);
    assert_eq!(SH_SAMPLE.matches("sh_chebyshev_visibility(").count(), 2);
    assert_eq!(SH_SAMPLE.matches("sh_depth_tile_moments(").count(), 2);
    assert_eq!(SH_SAMPLE.matches("sh_depth_moment_pair(").count(), 2);
    assert_eq!(
        SH_SAMPLE.matches("SH_DEPTH_MIN_VISIBILITY").count(),
        2,
        "the floor is declared once and applied once, in the shared core"
    );
    let visibility = wgsl_function(SH_SAMPLE, "sh_corner_depth_visibility");
    assert_eq!(
        visibility.matches("textureLoad(sh_depth_moments").count(),
        1
    );
    assert!(visibility.contains("packed.g & SH_DEPTH_TILE_REFERENCE_FLAG"));
    let lookup = wgsl_function(SH_SAMPLE, "sh_depth_tile_moments");
    for forbidden in ["textureDimensions", "normalize", " / ", " % "] {
        assert!(
            !lookup.contains(forbidden),
            "the tile lookup must not use `{forbidden}`"
        );
    }
    assert!(!wgsl_function(SH_SAMPLE, "oct_encode_l1").contains("normalize"));
}

/// Untiled probes, one of them with `E[d²]` = +0, compute today's visibility
/// bit for bit beside a tiled neighbour; the tiled neighbour reads its tile in
/// the direction of the shaded point.
#[test]
fn an_untiled_probe_computes_todays_visibility_bit_for_bit_beside_a_tiled_one() {
    let Some(ctx) =
        gpu_or_skip("an_untiled_probe_computes_todays_visibility_bit_for_bit_beside_a_tiled_one")
    else {
        return;
    };
    let probes = vec![probe(1.0, 0x3d00), probe(1.0, 0x3d00), probe(0.5, 0x0000)];
    let grid = [3, 1, 1];
    let tile = directional_tile();
    let tiles = section(vec![1], vec![tile.clone()]);
    let moments = packed_rg(&probes, grid, Some(&tiles));
    assert_eq!(
        decode_moment_second_channel(moments[2][1]),
        MomentSecondChannel::Averaged(0),
        "+0 E[d²] stays untiled"
    );

    let mut queries = Vec::new();
    for untiled in [0, 2] {
        for step in 0..48 {
            let distance = step as f32 * 0.1;
            let dir = Vec3::new(0.3, -0.8, 0.52).normalize() * distance;
            for valid in [true, false] {
                queries.push(Query {
                    probe: [untiled, 0, 0],
                    valid,
                    sample_world: (Vec3::new(untiled as f32, 0.0, 0.0) + dir).to_array(),
                });
            }
        }
    }
    let untiled_queries = queries.len();
    // The tiled neighbour, toward each interior texel centre.
    let edge = PROBE_DEPTH_TILE_EDGE;
    let mut directions = Vec::new();
    for y in 0..edge {
        for x in 0..edge {
            let dir = irradiance_interior_texel_direction(
                x,
                y,
                PROBE_DEPTH_TILE_BORDERED_EDGE,
                PROBE_DEPTH_TILE_BORDER,
            );
            let dir = Vec3::from(dir) * 1.5;
            directions.push((x, y, dir));
            queries.push(Query {
                probe: [1, 0, 0],
                valid: true,
                sample_world: (Vec3::X + dir).to_array(),
            });
        }
    }
    let results = run(
        &ctx,
        &Lattice {
            grid,
            moments: &moments,
            tiles: Some(&tiles),
        },
        &queries,
    );

    for (query, result) in queries.iter().zip(&results).take(untiled_queries) {
        assert_eq!(
            result.visibility_bits,
            result.today_bits,
            "probe {:?} at {:?}: {} vs today {}",
            query.probe,
            query.sample_world,
            result.visibility(),
            f32::from_bits(result.today_bits)
        );
        assert_eq!(result.tile_moments, [-1.0, -1.0], "untiled reads no tile");
    }
    for ((x, y, dir), result) in directions.iter().zip(&results[untiled_queries..]) {
        let [mean, mean_sq] = tile[((y + 1) * PROBE_DEPTH_TILE_BORDERED_EDGE + x + 1) as usize];
        let expected = [f16_bits_to_f32(mean), f16_bits_to_f32(mean_sq)];
        let mirror = sample_tile_moments(&tile, dir.to_array());
        for channel in 0..2 {
            assert!(
                (result.tile_moments[channel] - expected[channel]).abs() < 1.0e-3
                    && (mirror[channel] - expected[channel]).abs() < 1.0e-3,
                "texel ({x}, {y}) channel {channel}: GPU {:?}, CPU mirror {mirror:?}, texel {expected:?}",
                result.tile_moments
            );
        }
        assert!((0.03..=1.0).contains(&result.visibility()));
    }
}

/// A tile texel with mean 0 and a shaded point past it weighs exactly the
/// 0.03 floor; no distance takes a valid tiled probe below it.
#[test]
fn a_tiled_probe_never_weighs_below_the_floor() {
    let Some(ctx) = gpu_or_skip("a_tiled_probe_never_weighs_below_the_floor") else {
        return;
    };
    let probes = vec![probe(5.0, 0x4e40)];
    let tiles = section(vec![0], vec![uniform_tile(0.0, 0.0)]);
    let moments = packed_rg(&probes, [1, 1, 1], Some(&tiles));
    let mut queries = vec![Query {
        probe: [0, 0, 0],
        valid: true,
        sample_world: [0.0, 2.0, 0.0],
    }];
    // The shaded point at the probe itself: no direction, no delta.
    queries.push(Query {
        probe: [0, 0, 0],
        valid: true,
        sample_world: [0.0, 0.0, 0.0],
    });
    for step in 1..64 {
        let distance = step as f32 * 0.07;
        let dir = Vec3::new(
            (step as f32).sin(),
            (step as f32 * 1.7).cos(),
            0.4 - step as f32 * 0.01,
        );
        queries.push(Query {
            probe: [0, 0, 0],
            valid: true,
            sample_world: (dir.normalize() * distance).to_array(),
        });
    }
    let results = run(
        &ctx,
        &Lattice {
            grid: [1, 1, 1],
            moments: &moments,
            tiles: Some(&tiles),
        },
        &queries,
    );
    assert_eq!(results[0].tile_moments, [0.0, 0.0]);
    assert_eq!(
        results[0].visibility_bits,
        0.03f32.to_bits(),
        "mean 0 with the point past it weighs exactly the floor, got {}",
        results[0].visibility()
    );
    assert_eq!(
        results[1].visibility(),
        1.0,
        "a point at the probe is fully visible"
    );
    for (query, result) in queries.iter().zip(&results) {
        assert!(
            (0.03..=1.0).contains(&result.visibility()),
            "{:?}: {}",
            query.sample_world,
            result.visibility()
        );
    }
}

/// The flag is the raw sign bit: tile 0 (f16 −0) and tile 32,767 (f16 NaN)
/// each read their own tile in the full-cap 2560×1280 layout.
#[test]
fn tile_references_decode_from_raw_bits_at_both_ends_of_the_range() {
    let Some(ctx) = gpu_or_skip("tile_references_decode_from_raw_bits_at_both_ends_of_the_range")
    else {
        return;
    };
    let grid = [256, 128, 1];
    let count = MAX_PROBE_DEPTH_TILES;
    let tile_mean = |tile: u32| 1.0 + (tile % 1000) as f32 * 0.5;
    let tiles = section(
        (0..count).collect(),
        (0..count)
            .map(|tile| uniform_tile(tile_mean(tile), 1.0))
            .collect(),
    );
    assert_eq!(tiles.texture_layout().unwrap().extent(), [2560, 1280]);
    let probes = vec![probe(1.0, 0x3d00); count as usize];
    let moments = packed_rg(&probes, grid, Some(&tiles));
    assert_eq!(moments[0][1], 0x8000, "tile 0 is f16 -0");
    assert_eq!(
        moments[count as usize - 1][1],
        0xffff,
        "tile 32,767 is an f16 NaN"
    );

    let checked = [0u32, 1, 255, 256, 300, 999, 32_512, 32_766, 32_767];
    let queries: Vec<Query> = checked
        .iter()
        .map(|&tile| {
            let at = [(tile % 256) as i32, (tile / 256) as i32, 0];
            Query {
                probe: at,
                valid: true,
                sample_world: [at[0] as f32 + 0.3, at[1] as f32 - 0.2, 0.7],
            }
        })
        .collect();
    let results = run(
        &ctx,
        &Lattice {
            grid,
            moments: &moments,
            tiles: Some(&tiles),
        },
        &queries,
    );
    for (tile, result) in checked.iter().zip(&results) {
        assert_eq!(
            result.tile_moments,
            [tile_mean(*tile), 1.0],
            "tile {tile} must read its own texels"
        );
    }
}

/// Tile bytes appear in the SH ledger while the level is installed, equal to
/// the layout's price, and leave with the level. A zero-tile id 59 prices 0.
#[test]
fn tile_texture_bytes_are_ledgered_while_installed_and_gone_after_unload() {
    use crate::render::fluid_baked_frame_test_fixtures::sh_section;
    use crate::render::{LevelGeometry, LevelGeometryShStorage};
    let Some(mut renderer) = offscreen_renderer_or_skip(
        "tile_texture_bytes_are_ledgered_while_installed_and_gone_after_unload",
        16,
        16,
    ) else {
        return;
    };
    let sh = sh_section([0.0; 3], [1.0; 3], [4, 4, 2], |_| Vec3::splat(0.5));
    let tiled = section(vec![3, 17, 30], vec![uniform_tile(1.0, 2.0); 3]);
    let zero = section(Vec::new(), Vec::new());
    let bvh = postretro_render_data::geometry::BvhTree {
        nodes: Vec::new(),
        leaves: Vec::new(),
        root_node_index: 0,
    };
    let tile_bytes = |renderer: &crate::render::Renderer| {
        renderer.sh_residency_report().map_or(0, |report| {
            report
                .allocations
                .iter()
                .filter(|allocation| allocation.name == "depth_tiles")
                .map(|allocation| allocation.bytes)
                .sum::<u64>()
        })
    };
    for (tiles, expected) in [(&tiled, probe_depth_tile_texture_bytes(3)), (&zero, 0)] {
        renderer
            .install_level_geometry(
                &LevelGeometry {
                    vertices: &[],
                    vertex_spec_owners: &[],
                    indices: &[],
                    bvh: &bvh,
                    lights: &[],
                    light_influences: &[],
                    sh_volume: Some(&sh),
                    sh_storage: LevelGeometryShStorage::Legacy,
                    probe_depth_tiles: Some(tiles),
                    lightmap: None,
                    lightmap_streaming: None,
                    chunk_light_list: None,
                    animated_light_chunks: None,
                    animated_light_weight_maps: None,
                    delta_sh_volumes: None,
                    direct_sh_volume: None,
                    direct_sh_delta_volumes: None,
                    animated_direct_sh_delta_volumes: None,
                    billboard_direct_scatter_volume: None,
                    animated_billboard_direct_scatter_delta_volumes: None,
                    entity_shadow_lights: &[],
                    shadowmask_atlas: None,
                    sdf_atlas: None,
                    lightmap_mode: postretro_level_loader::LightmapMode::Shadowed,
                    cell_draw_index: None,
                    kinematic_geometry: None,
                    cells: &[],
                    texture_materials: &[],
                },
                Default::default(),
            )
            .expect("level resources install");
        assert!(renderer.sh_residency_report().is_some());
        assert_eq!(tile_bytes(&renderer), expected);
        renderer.release_level_resources();
        assert_eq!(tile_bytes(&renderer), 0, "unload frees the tile texture");
    }
}
