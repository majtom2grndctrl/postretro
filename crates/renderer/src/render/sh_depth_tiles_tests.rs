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
                    mover_probe_channel: None,
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

/// Probe `index`'s id-34 moments in the rewrite fixtures: distinct per probe,
/// non-negative f16 bits.
fn fixture_moments(index: usize) -> [u16; 2] {
    let low = index as u16 & 0x3ff;
    [0x3c00 | low, 0x4400 | low]
}

/// A 4×4×2 whole-load lattice with distinct moments and one invalid probe
/// (7), and an id 59 tiling probes 3 and 17.
fn rewrite_fixture() -> (
    postretro_level_format::sh_volume::OctahedralShVolumeSection,
    ProbeDepthTilesSection,
) {
    use crate::render::fluid_baked_frame_test_fixtures::sh_section;
    let mut sh = sh_section([0.0; 3], [1.0; 3], [4, 4, 2], |_| Vec3::splat(0.5));
    for (index, probe) in sh.probes.iter_mut().enumerate() {
        [probe.mean_distance, probe.mean_sq_distance] = fixture_moments(index);
    }
    sh.probes[7].validity = 0;
    let tiles = section(
        vec![3, 17],
        vec![uniform_tile(0.5, 0.5), directional_tile()],
    );
    (sh, tiles)
}

fn install_whole(
    renderer: &mut crate::render::Renderer,
    sh: &postretro_level_format::sh_volume::OctahedralShVolumeSection,
    tiles: &ProbeDepthTilesSection,
) {
    use crate::render::renderer_textures::tests::with_one_triangle;
    use postretro_render_data::material::Material;
    with_one_triangle(&[Material::Default], Some(sh), None, &[], |geometry| {
        renderer
            .install_level_geometry(
                &crate::render::LevelGeometry {
                    probe_depth_tiles: Some(tiles),
                    sh_storage: crate::render::LevelGeometryShStorage::Legacy,
                    ..*geometry
                },
                Default::default(),
            )
            .expect("level resources install")
    });
}

/// Every texel of `level`'s whole-load moments texture, x fastest. The
/// readback's submit carries any staged frame write first.
fn whole_texels(
    renderer: &crate::render::Renderer,
    level: &crate::render::level_resource_set::LevelResourceSet,
) -> Vec<[u16; 4]> {
    let device = &renderer.device;
    let texture = level.sh_volume_resources.depth_moment_texture();
    let extent = texture.size();
    let row = extent.width * 8;
    let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let rows = extent.height * extent.depth_or_array_layers;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("whole-load depth moments readback"),
        size: u64::from(padded * rows),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(extent.height),
            },
        },
        extent,
    );
    renderer.queue.submit([encoder.finish()]);
    let (send, receive) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            send.send(result).unwrap()
        });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    receive.recv().unwrap().unwrap();
    let bytes = buffer.slice(..).get_mapped_range().unwrap().to_vec();
    buffer.unmap();
    bytes
        .chunks(padded as usize)
        .flat_map(|line| line[..row as usize].as_chunks::<8>().0.to_vec())
        .map(|texel| {
            let half = |at: usize| u16::from_le_bytes([texel[at], texel[at + 1]]);
            [half(0), half(2), half(4), half(6)]
        })
        .collect()
}

fn second(texel: [u16; 4]) -> MomentSecondChannel {
    decode_moment_second_channel(texel[1])
}

/// O5, O7 on a whole-loaded level: a rewrite switches the probe's texel by
/// the next submit, on a probe with or without a tile, through the same
/// texture and bindings; the switched texel weighs with the new tile, and
/// clearing restores the never-tiled texel, so today's visibility, bit for
/// bit. The seam refuses what the level does not have.
#[test]
fn a_whole_load_rewrite_switches_the_texel_next_frame_and_clearing_restores_it() {
    use crate::render::ProbeDepthTileRewriteError as Error;
    use postretro_render_cpu::sh_depth_tiles::DepthTileReferenceError;
    let test = "a_whole_load_rewrite_switches_the_texel_next_frame_and_clearing_restores_it";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let Some(ctx) = gpu_or_skip(test) else {
        return;
    };
    assert_eq!(
        renderer.set_probe_depth_tile_reference(5, Some(0)),
        Err(Error::NoLevel)
    );
    let (sh, tiles) = rewrite_fixture();
    install_whole(&mut renderer, &sh, &tiles);
    let before = whole_texels(&renderer, renderer.level());
    assert_eq!(second(before[3]), MomentSecondChannel::Tile(0));
    assert_eq!([before[5][0], before[5][1]], fixture_moments(5));

    let layout_before = renderer.level().sh_bind_group() as *const wgpu::BindGroup;
    renderer.set_probe_depth_tile_reference(5, Some(1)).unwrap();
    let redirected = whole_texels(&renderer, renderer.level());
    assert_eq!(
        redirected[5][0],
        fixture_moments(5)[0],
        "R stays id-34 E[d]"
    );
    assert_eq!(second(redirected[5]), MomentSecondChannel::Tile(1));
    assert_eq!(redirected[5][2..], before[5][2..], "B/A keep the word");
    assert_eq!(
        renderer.level().sh_bind_group() as *const wgpu::BindGroup,
        layout_before,
        "no new binding"
    );

    // The texels the GPU now holds weigh probe 5 with tile 1 toward the
    // shaded point; cleared, they weigh it as the never-tiled probe did.
    let rg = |texels: &[[u16; 4]]| -> Vec<[u16; 2]> {
        texels.iter().map(|&[r, g, _, _]| [r, g]).collect()
    };
    let queries = [Query {
        probe: [1, 1, 0],
        valid: true,
        sample_world: [1.8, 1.4, 0.3],
    }];
    let visibility = |texels: &[[u16; 4]]| {
        run(
            &ctx,
            &Lattice {
                grid: [4, 4, 2],
                moments: &rg(texels),
                tiles: Some(&tiles),
            },
            &queries,
        )[0]
    };
    let today = visibility(&before);
    assert_eq!(today.tile_moments, [-1.0, -1.0]);
    let switched = visibility(&redirected);
    assert_ne!(switched.tile_moments, [-1.0, -1.0], "reads tile 1");

    renderer.set_probe_depth_tile_reference(5, None).unwrap();
    let cleared = whole_texels(&renderer, renderer.level());
    assert_eq!(cleared[5], before[5], "the never-tiled texel, bit for bit");
    assert_eq!(visibility(&cleared).visibility_bits, today.visibility_bits);

    renderer.set_probe_depth_tile_reference(3, None).unwrap();
    let untiled = whole_texels(&renderer, renderer.level());
    assert_eq!([untiled[3][0], untiled[3][1]], fixture_moments(3));
    assert_eq!(untiled[3][2..], before[3][2..]);

    let refused = [
        (
            renderer.set_probe_depth_tile_reference(32, Some(0)),
            Err(Error::ProbeOutsideLattice {
                probe: 32,
                probe_count: 32,
            }),
        ),
        (
            renderer.set_probe_depth_tile_reference(7, Some(0)),
            Err(Error::InvalidProbe { probe: 7 }),
        ),
        (
            renderer.set_probe_depth_tile_reference(5, Some(2)),
            Err(Error::Reference(DepthTileReferenceError::TileOutOfRange {
                tile: 2,
                tile_count: 2,
            })),
        ),
    ];
    for (result, expected) in refused {
        assert_eq!(result, expected);
    }
    assert_eq!(whole_texels(&renderer, renderer.level()), untiled);
    eprintln!("[DepthTileProof] whole-load rewrite, clear and refusals held; ran on GPU");
}

/// O10: a reference rewritten on the published load is absent from a set
/// built off-main while it is published, from that set once published, and
/// from a reload of the same map: each load's texels decode to its own id 59.
#[test]
fn a_rewritten_reference_stays_with_the_load_it_was_made_on() {
    use crate::render::level_build::{GeometryExtras, GeometryRest};
    use crate::render::level_resource_set::PreviousLevelSet;
    use crate::render::renderer_textures::tests::with_one_triangle;
    use postretro_render_data::material::Material;
    let test = "a_rewritten_reference_stays_with_the_load_it_was_made_on";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let (sh, tiles) = rewrite_fixture();
    install_whole(&mut renderer, &sh, &tiles);
    renderer.set_probe_depth_tile_reference(3, Some(1)).unwrap();
    assert_eq!(
        second(whole_texels(&renderer, renderer.level())[3]),
        MomentSecondChannel::Tile(1)
    );

    // The same map, built off-main beside the published, rewritten set.
    let (builder, mut pending) = renderer.begin_level_set_build(renderer.level_build_tiers());
    let (source_sh, source_tiles) = (sh.clone(), tiles.clone());
    builder.geometry_from(
        Box::new(move |visit| {
            with_one_triangle(
                &[Material::Default],
                Some(&source_sh),
                None,
                &[],
                |geometry| {
                    visit(
                        &crate::render::LevelGeometry {
                            probe_depth_tiles: Some(&source_tiles),
                            sh_storage: crate::render::LevelGeometryShStorage::Legacy,
                            ..*geometry
                        },
                        GeometryExtras {
                            volumes: &[],
                            reflection_probes: None,
                            fog_pixel_scale: 0,
                            fog_cell_masks: None,
                        },
                    )
                },
            )
        }),
        GeometryRest {
            gpu_lighting_payloads: Default::default(),
            sh_plan: None,
            lightmap_plan: None,
            fluid: None,
            planar_candidate: None,
        },
    );
    builder.finish();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let id = loop {
        if let Some(outcome) = pending.try_take(&mut renderer) {
            break outcome.expect("the off-main build succeeds");
        }
        assert!(std::time::Instant::now() < deadline, "the build reports");
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    let built = renderer
        .full()
        .resident_levels
        .iter()
        .find(|level| level.id == id)
        .expect("the built set is resident");
    let built_texels = whole_texels(&renderer, built);
    assert_eq!(second(built_texels[3]), MomentSecondChannel::Tile(0));
    assert_eq!(second(built_texels[17]), MomentSecondChannel::Tile(1));
    assert_eq!(
        second(whole_texels(&renderer, renderer.level())[3]),
        MomentSecondChannel::Tile(1),
        "the published set keeps its own rewrite"
    );

    renderer.publish_level_set(id, PreviousLevelSet::Retire);
    assert_eq!(renderer.level().id, id);
    assert_eq!(
        second(whole_texels(&renderer, renderer.level())[3]),
        MomentSecondChannel::Tile(0)
    );

    // Rewrite this load too, then reload the map from scratch.
    renderer.set_probe_depth_tile_reference(3, None).unwrap();
    // The readback's submit is the frame boundary unload expects.
    assert_eq!(
        second(whole_texels(&renderer, renderer.level())[3]),
        MomentSecondChannel::Averaged(fixture_moments(3)[1])
    );
    renderer.release_level_resources();
    install_whole(&mut renderer, &sh, &tiles);
    let reloaded = whole_texels(&renderer, renderer.level());
    assert_eq!(second(reloaded[3]), MomentSecondChannel::Tile(0));
    assert_eq!(second(reloaded[17]), MomentSecondChannel::Tile(1));
    eprintln!("[DepthTileProof] rewrites stayed with their load across build, publish, reload");
}
