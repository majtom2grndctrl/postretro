// GPU proofs for mover depth live tiles on a whole-loaded level: what the
// tile texture and the moment texels hold after each frame boundary, the
// write triggers, install and unload, and the ledger. The streamed path's
// proofs sit with its fixture (`sh_streaming/gpu/mover_depth_tile_tests.rs`).
// See: context/lib/rendering_pipeline.md §4 (Direction-aware probe
// visibility, Mover state lighting) · context/lib/testing_guide.md §3

use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::mover_probe_channel::{
    MoverProbeChannelSection, depth_tile_texture_count,
};
use postretro_level_format::probe_depth_tiles::{
    MomentSecondChannel, PROBE_DEPTH_TILE_BORDERED_EDGE, PROBE_DEPTH_TILE_TEXELS,
    ProbeDepthTileTextureLayout, ProbeDepthTilesSection, f16_bits_to_f32,
    probe_depth_tile_texture_bytes,
};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use crate::render::gpu_test_harness::offscreen_renderer_or_skip;
use crate::render::mover_channel::depth_test_support::{depth_section, ramp_tile, uniform_tile};
use crate::render::sh_depth_tiles::tests::{fixture_moments, second, whole_texels};
use crate::render::{MoverTravel, Renderer};
use postretro_level_loader::ShDrainBatch;

/// Id 59 tiles probes 3 and 17. Mover 4 changes probes 3, 5 and 9; mover 6
/// changes 9 and 20. Live tiles follow id 59's two: probe 3 → 2, 5 → 3,
/// 9 → 4, 20 → 5.
const ID59_PROBES: [u32; 2] = [3, 17];
const LIVE: [(u32, u32); 4] = [(3, 2), (5, 3), (9, 4), (20, 5)];

fn d(mover: u32, probe: u32, terminus: u32) -> Vec<[u16; 2]> {
    ramp_tile(0.25 + mover as f32 + probe as f32 / 8.0 + terminus as f32 * 2.0)
}

fn channel() -> MoverProbeChannelSection {
    depth_section(
        [(4, 3), (4, 5), (4, 9), (6, 9), (6, 20)]
            .into_iter()
            .map(|(mover, probe)| (mover, probe, d(mover, probe, 0), d(mover, probe, 1)))
            .collect(),
    )
}

fn id59() -> ProbeDepthTilesSection {
    ProbeDepthTilesSection {
        probe_indices: ID59_PROBES.to_vec(),
        texels: [uniform_tile(0.5, 0.5), uniform_tile(1.5, 3.0)].concat(),
    }
}

/// The 4×4×2 whole-load lattice of the rewrite fixtures: distinct moments,
/// probe 7 invalid.
fn lattice() -> OctahedralShVolumeSection {
    use crate::render::fluid_baked_frame_test_fixtures::sh_section;
    let mut sh = sh_section([0.0; 3], [1.0; 3], [4, 4, 2], |_| glam::Vec3::splat(0.5));
    for (index, probe) in sh.probes.iter_mut().enumerate() {
        [probe.mean_distance, probe.mean_sq_distance] = fixture_moments(index);
    }
    sh.probes[7].validity = 0;
    sh
}

fn install(
    renderer: &mut Renderer,
    sh: &OctahedralShVolumeSection,
    tiles: Option<&ProbeDepthTilesSection>,
    channel: Option<&MoverProbeChannelSection>,
) {
    use crate::render::renderer_textures::tests::with_one_triangle;
    use postretro_render_data::material::Material;
    with_one_triangle(&[Material::Default], Some(sh), None, &[], |geometry| {
        renderer
            .install_level_geometry(
                &crate::render::LevelGeometry {
                    probe_depth_tiles: tiles,
                    mover_probe_channel: channel,
                    sh_storage: crate::render::LevelGeometryShStorage::Legacy,
                    ..*geometry
                },
                Default::default(),
            )
            .expect("level resources install")
    });
}

fn state(mover_id: u32, t: f32) -> MoverTravel {
    MoverTravel { mover_id, t }
}

/// One frame boundary: the game hands over `states`, the frame drains, and
/// the frame's upload batch is submitted, as its end submits it.
fn frame(renderer: &mut Renderer, states: &[MoverTravel]) {
    renderer.set_mover_travel(states);
    renderer
        .drain_sh_residency(ShDrainBatch::default())
        .expect("the frame's drain");
    renderer.queue.flush_skipped_frame();
}

fn tiles_written(renderer: &Renderer) -> u32 {
    renderer
        .level()
        .sh_volume_resources
        .mover_channel
        .depth_tiles_written()
}

/// Every texel of the level's tile texture, with its layout. The readback's
/// submit carries the frame's staged writes first, as the frame's would.
fn tile_texture(renderer: &Renderer) -> (Vec<[u16; 2]>, Option<ProbeDepthTileTextureLayout>) {
    let tiles = &renderer.level().sh_volume_resources.depth_tiles;
    let texture = tiles.texture();
    let device = &renderer.device;
    let extent = texture.size();
    let row = extent.width * 4;
    let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("depth tile texture readback"),
        size: u64::from(padded * extent.height),
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
    let texels = bytes
        .chunks(padded as usize)
        .flat_map(|line| line[..row as usize].as_chunks::<4>().0.to_vec())
        .map(|texel| {
            [
                u16::from_le_bytes([texel[0], texel[1]]),
                u16::from_le_bytes([texel[2], texel[3]]),
            ]
        })
        .collect();
    (texels, tiles.layout())
}

/// Tile `tile`'s bordered texels out of a [`tile_texture`] readback.
fn tile_of(
    texture: &(Vec<[u16; 2]>, Option<ProbeDepthTileTextureLayout>),
    tile: u32,
) -> Vec<[u16; 2]> {
    let layout = texture.1.expect("a real tile texture");
    let width = layout.extent()[0] as usize;
    let edge = PROBE_DEPTH_TILE_BORDERED_EDGE as usize;
    let [ox, oy] = layout.tile_origin(tile);
    (0..edge)
        .flat_map(|row| {
            let at = (oy as usize + row) * width + ox as usize;
            texture.0[at..at + edge].to_vec()
        })
        .collect()
}

/// Within one f16 ulp of `(1 − t)·D₀ + t·D₁`, per channel.
fn assert_blend(found: &[[u16; 2]], d0: &[[u16; 2]], d1: &[[u16; 2]], t: f32) {
    for (at, texel) in found.iter().enumerate() {
        for channel in 0..2 {
            let exact =
                (1.0 - t) * f16_bits_to_f32(d0[at][channel]) + t * f16_bits_to_f32(d1[at][channel]);
            assert!(
                texel[channel].abs_diff(f32_to_f16_bits(exact)) <= 1,
                "texel {at} channel {channel} at t′ {t}"
            );
        }
    }
}

/// Before any hand-over every
/// probe keeps its id-59 tile or averaged moments bit for bit; the first
/// hand-over at the seeded terminus points each changed probe at its live
/// tile, which already holds that terminus's tile; a live tile then reads
/// D₀, D₁ and their mix within one f16 ulp, rising and after a reversal.
/// Only G moves: R keeps id-34 E[d] and B/A the word, and the probe stays
/// valid; a probe no mover changes keeps its texel and tile bit for bit.
#[test]
fn a_live_tile_holds_the_blend_and_only_g_moves_to_it() {
    let test = "a_live_tile_holds_the_blend_and_only_g_moves_to_it";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let (sh, tiles, channel) = (lattice(), id59(), channel());
    install(&mut renderer, &sh, Some(&tiles), Some(&channel));
    let before = whole_texels(&renderer, renderer.level());
    let texture_before = tile_texture(&renderer);
    frame(&mut renderer, &[]);
    assert_eq!(tiles_written(&renderer), 0);
    assert_eq!(
        whole_texels(&renderer, renderer.level()),
        before,
        "no state: every texel as id 59 left it"
    );
    assert_eq!(second(before[3]), MomentSecondChannel::Tile(0));
    assert_eq!(
        second(before[5]),
        MomentSecondChannel::Averaged(fixture_moments(5)[1])
    );

    // Mover 4 held at its seeded terminus 1 from load.
    frame(&mut renderer, &[state(4, 1.0)]);
    assert_eq!(tiles_written(&renderer), 3);
    let texels = whole_texels(&renderer, renderer.level());
    let texture = tile_texture(&renderer);
    for (probe, live) in LIVE {
        if probe == 20 {
            assert_eq!(texels[20], before[20], "mover 6 has no state");
            continue;
        }
        let p = probe as usize;
        assert_eq!(second(texels[p]), MomentSecondChannel::Tile(live as u16));
        assert_eq!(texels[p][0], fixture_moments(p)[0], "R keeps id-34 E[d]");
        assert_eq!(texels[p][2..], before[p][2..], "B/A keep the valid word");
        assert_eq!(tile_of(&texture, live), d(4, probe, 1), "probe {probe}: D₁");
    }
    for probe in (0..32).filter(|probe| ![3, 5, 9].contains(probe)) {
        assert_eq!(texels[probe], before[probe], "probe {probe} untouched");
    }
    for tile in [0, 1] {
        assert_eq!(
            tile_of(&texture, tile),
            tile_of(&texture_before, tile),
            "id-59 tile {tile}"
        );
    }

    for t in [0.5, 0.0, 0.5, 1.0, 0.25] {
        frame(&mut renderer, &[state(4, t)]);
        let texture = tile_texture(&renderer);
        for probe in [3, 5] {
            let live = LIVE.iter().find(|(p, _)| *p == probe).unwrap().1;
            assert_blend(
                &tile_of(&texture, live),
                &d(4, probe, 0),
                &d(4, probe, 1),
                t,
            );
            if t == 0.0 {
                assert_eq!(tile_of(&texture, live), d(4, probe, 0));
            }
        }
        let texels = whole_texels(&renderer, renderer.level());
        for probe in [3, 5, 9] {
            assert_eq!(texels[probe][0], fixture_moments(probe)[0]);
            assert!(matches!(
                second(texels[probe]),
                MomentSecondChannel::Tile(_)
            ));
        }
    }
    eprintln!("[MoverDepthProof] live tiles blended, only G moved; ran on GPU");
}

/// Stripping id 57 installs exactly
/// today's tile texture and texels; with id 57 and no state, every texel
/// matches the stripped install bit for bit. A frame with no t′ change, or
/// one that hands over the same t′ (a ramp holding it at 0 or 1), writes no
/// tile; a travelling mover writes only its tiles, and a tile shared with a
/// held mover is rewritten at the held t′.
#[test]
fn tiles_are_written_only_on_change_and_no_state_keeps_todays_texels() {
    let test = "tiles_are_written_only_on_change_and_no_state_keeps_todays_texels";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let (sh, tiles, channel) = (lattice(), id59(), channel());
    install(&mut renderer, &sh, Some(&tiles), None);
    let stripped = whole_texels(&renderer, renderer.level());
    let stripped_texture = tile_texture(&renderer);
    assert_eq!(
        stripped_texture.1,
        ProbeDepthTileTextureLayout::for_tile_count(2),
        "no section: id 59's texture alone"
    );
    frame(&mut renderer, &[state(4, 0.5)]);
    assert_eq!(whole_texels(&renderer, renderer.level()), stripped);
    renderer.release_level_resources();

    install(&mut renderer, &sh, Some(&tiles), Some(&channel));
    frame(&mut renderer, &[]);
    assert_eq!(whole_texels(&renderer, renderer.level()), stripped);
    let texture = tile_texture(&renderer);
    for tile in [0, 1] {
        assert_eq!(tile_of(&texture, tile), tile_of(&stripped_texture, tile));
    }

    frame(&mut renderer, &[state(4, 0.0), state(6, 1.0)]);
    assert_eq!(
        tiles_written(&renderer),
        4,
        "first hand-over: every live tile"
    );
    let held = tile_texture(&renderer);
    for _ in 0..3 {
        frame(&mut renderer, &[state(4, 0.0), state(6, 1.0)]);
        assert_eq!(
            tiles_written(&renderer),
            0,
            "an unchanged t′ writes nothing"
        );
    }
    // Mover 6 holds at 1; mover 4 travels. Probe 20 (mover 6 alone) stays.
    frame(&mut renderer, &[state(4, 0.5), state(6, 1.0)]);
    assert_eq!(tiles_written(&renderer), 3);
    let texture = tile_texture(&renderer);
    assert_eq!(
        tile_of(&texture, 5),
        tile_of(&held, 5),
        "probe 20 untouched"
    );
    // Probe 9 (shared, untiled): per texel and moment its averaged moments
    // plus mover 4's blend at 0.5 and mover 6's held D₁, each less the base,
    // clamped to a valid moment set.
    let shared = tile_of(&texture, 4);
    let d4 = (d(4, 9, 0), d(4, 9, 1));
    let d6 = d(6, 9, 1);
    let base = fixture_moments(9).map(f16_bits_to_f32);
    for (at, texel) in shared.iter().enumerate() {
        let [mean, mean_sq] = [0, 1].map(|channel| {
            let mover_4 =
                0.5 * f16_bits_to_f32(d4.0[at][channel]) + 0.5 * f16_bits_to_f32(d4.1[at][channel]);
            let mover_6 = f16_bits_to_f32(d6[at][channel]);
            base[channel] + (mover_4 - base[channel]) + (mover_6 - base[channel])
        });
        let mean = mean.max(0.0);
        let expected = [mean, mean_sq.max(mean * mean)];
        for channel in 0..2 {
            assert!(
                texel[channel].abs_diff(f32_to_f16_bits(expected[channel])) <= 1,
                "texel {at} channel {channel}: the summed differences"
            );
        }
    }
    // Mover 6 now travels with mover 4 held: the shared tile reads 4's held 0.5.
    frame(&mut renderer, &[state(4, 0.5), state(6, 0.0)]);
    assert_eq!(tiles_written(&renderer), 2, "probes 9 and 20");
    eprintln!("[MoverDepthProof] write-on-change and no-state texels held; ran on GPU");
}

/// Whole-load half: a t′ change on a frame that runs no
/// drain is written by the next frame that does, though t′ no longer
/// changes; a change staged on a frame that skips present lands with the
/// skipped frame's flush.
#[test]
fn a_change_no_frame_wrote_lands_on_the_next_recording_frame() {
    let test = "a_change_no_frame_wrote_lands_on_the_next_recording_frame";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    install(&mut renderer, &lattice(), Some(&id59()), Some(&channel()));
    frame(&mut renderer, &[state(4, 0.0)]);
    // A held frame: the game hands over a new t′, nothing records.
    renderer.set_mover_travel(&[state(4, 1.0)]);
    frame(&mut renderer, &[state(4, 1.0)]);
    assert_eq!(tiles_written(&renderer), 3);
    assert_eq!(tile_of(&tile_texture(&renderer), 3), d(4, 5, 1));
    // A skipped frame: the drain stages, the frame flushes without drawing.
    renderer.set_mover_travel(&[state(4, 0.0)]);
    renderer
        .drain_sh_residency(ShDrainBatch::default())
        .unwrap();
    renderer.queue.flush_skipped_frame();
    frame(&mut renderer, &[state(4, 0.0)]);
    assert_eq!(tiles_written(&renderer), 0, "already written");
    assert_eq!(tile_of(&tile_texture(&renderer), 3), d(4, 5, 0));
}

/// Whole-load half: a hand-over with no
/// level writes nothing and never becomes the next level's state; a
/// relevel reusing a mover id at the old level's last t′ writes the new
/// level's own tiles on its first frame; a t′ handed over on the frame a
/// level unloads or a relevel installs leaves no pending upload and no
/// write in either level.
#[test]
fn a_relevel_writes_its_own_tiles_and_an_unload_frame_writes_nothing() {
    let test = "a_relevel_writes_its_own_tiles_and_an_unload_frame_writes_nothing";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    renderer.set_mover_travel(&[state(4, 1.0)]);
    let (sh, tiles) = (lattice(), id59());
    install(&mut renderer, &sh, Some(&tiles), Some(&channel()));
    frame(&mut renderer, &[]);
    assert_eq!(
        tiles_written(&renderer),
        0,
        "the pre-install hand-over is gone"
    );
    assert_eq!(
        second(whole_texels(&renderer, renderer.level())[5]),
        MomentSecondChannel::Averaged(fixture_moments(5)[1])
    );
    frame(&mut renderer, &[state(4, 1.0)]);
    assert_eq!(tiles_written(&renderer), 3);

    // The unload frame: the game hands over a new t′, then the level goes.
    // Install and release assert no upload is pending at the boundary.
    renderer.set_mover_travel(&[state(4, 0.5)]);
    renderer.release_level_resources();
    renderer.queue.assert_empty("after unload");

    // Level B reuses mover 4 with other tiles, at A's last t′.
    let other = depth_section(vec![(4, 5, d(9, 5, 0), d(9, 5, 1))]);
    renderer.set_mover_travel(&[state(4, 1.0)]);
    install(&mut renderer, &sh, Some(&tiles), Some(&other));
    renderer.queue.assert_empty("after install");
    frame(&mut renderer, &[state(4, 1.0)]);
    assert_eq!(tiles_written(&renderer), 1);
    assert_eq!(
        tile_of(&tile_texture(&renderer), 2),
        d(9, 5, 1),
        "B's own D₁"
    );
    assert_eq!(
        second(whole_texels(&renderer, renderer.level())[3]),
        MomentSecondChannel::Tile(0),
        "B's probe 3 never inherits A's live tile"
    );
}

/// With id 59 absent or zero-tile and id 57 kept, the
/// texture is sized from the live tiles alone and the changed probes
/// reference live tiles holding D₀; with id 57 stripped as well the 1×1
/// dummy binds and no tile bytes are allocated.
#[test]
fn live_tiles_without_id_59_size_the_texture_alone() {
    let test = "live_tiles_without_id_59_size_the_texture_alone";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let tile_bytes = |renderer: &Renderer| {
        renderer.sh_residency_report().map_or(0, |report| {
            report
                .allocations
                .iter()
                .filter(|row| row.name == "depth_tiles")
                .map(|row| row.bytes)
                .sum::<u64>()
        })
    };
    let zero = ProbeDepthTilesSection::default();
    for tiles in [None, Some(&zero)] {
        install(&mut renderer, &lattice(), tiles, Some(&channel()));
        assert_eq!(tile_bytes(&renderer), probe_depth_tile_texture_bytes(4));
        frame(&mut renderer, &[state(4, 0.0)]);
        let texels = whole_texels(&renderer, renderer.level());
        let texture = tile_texture(&renderer);
        for (probe, live) in [(3u32, 0u32), (5, 1), (9, 2)] {
            assert_eq!(
                second(texels[probe as usize]),
                MomentSecondChannel::Tile(live as u16)
            );
            assert_eq!(tile_of(&texture, live), d(4, probe, 0));
        }
        renderer.release_level_resources();

        install(&mut renderer, &lattice(), tiles, None);
        assert_eq!(tile_bytes(&renderer), 0);
        assert_eq!(
            renderer
                .level()
                .sh_volume_resources
                .depth_tiles
                .texture()
                .size(),
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1
            },
            "the dummy binds"
        );
        renderer.release_level_resources();
    }
}

/// The tile texture's ledger row is
/// the layout's bytes for id 59's tiles plus the live tiles, sourced to ids
/// 59 and 57; the CPU-held closed-pose tiles are a host row outside the
/// physical total; both leave with the level.
#[test]
fn tile_bytes_and_held_closed_pose_tiles_are_ledgered_and_leave_with_the_level() {
    use crate::render::ResidencySource;
    let test = "tile_bytes_and_held_closed_pose_tiles_are_ledgered_and_leave_with_the_level";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let (tiles, channel) = (id59(), channel());
    install(&mut renderer, &lattice(), Some(&tiles), Some(&channel));
    let report = renderer.sh_residency_report().expect("an SH report");
    let row = report
        .allocations
        .iter()
        .find(|row| row.name == "depth_tiles")
        .expect("the tile texture row");
    let count = depth_tile_texture_count(tiles.tile_count(), Some(&channel)) as u32;
    assert_eq!(count, 6);
    assert_eq!(row.bytes, probe_depth_tile_texture_bytes(count));
    assert_eq!(
        row.sources,
        [ResidencySource::Section(59), ResidencySource::Section(57)]
    );
    let gpu_total: u64 = report.allocations.iter().map(|row| row.bytes).sum();
    assert_eq!(report.total_bytes, gpu_total, "the host row stays outside");
    assert_eq!(report.host_rows.len(), 1);
    let host = &report.host_rows[0];
    assert_eq!(host.name, "mover_closed_pose_depth_tiles");
    assert_eq!(host.sources, [ResidencySource::Section(57)]);
    assert_eq!(
        host.bytes,
        (5 * 2 + 1) * PROBE_DEPTH_TILE_TEXELS as u64 * 8,
        "decoded f32 pairs: ten closed-pose tiles, and the base tile of probe 9, which two movers change"
    );
    renderer.release_level_resources();
    assert!(
        renderer.sh_residency_report().is_none(),
        "nothing after unload"
    );
}

/// Whole-load half: after warm-up, a frame writing two
/// travelling movers' live tiles allocates nothing on the CPU, staging
/// included (the batch's submit is outside the window, as the frame's is).
#[test]
fn a_frame_writing_two_travelling_movers_tiles_allocates_nothing() {
    use postretro_sim::alloc_probe::AllocSnapshot;
    let test = "a_frame_writing_two_travelling_movers_tiles_allocates_nothing";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    install(&mut renderer, &lattice(), Some(&id59()), Some(&channel()));
    let states = |t: f32| [state(4, t), state(6, 1.0 - t)];
    for warm in 0..3 {
        frame(&mut renderer, &states(warm as f32 * 0.1));
        renderer.queue.flush_skipped_frame();
    }
    for step in 0..8 {
        let frame_states = states(0.3 + step as f32 * 0.05);
        renderer.set_mover_travel(&frame_states);
        let probe = AllocSnapshot::arm();
        renderer.write_mover_depth_tiles().unwrap();
        assert_eq!(probe.allocs_since(), 0, "frame {step}");
        assert_eq!(tiles_written(&renderer), 4);
        renderer.queue.flush_skipped_frame();
    }
}

/// Shader clause: live tiles need no sampler change. The
/// depth path of `sh_sample.wgsl` (its constants, the moment decode, the
/// tile lookup and the Chebyshev weight with its floor) hashes to the
/// pinned byte-identical to the pre-mover-channel sampler; the mover
/// backface weight is outside the hashed extract.
#[test]
fn the_sampler_depth_path_is_byte_identical_to_the_parents() {
    use crate::render::gpu_test_harness::{wgsl_const_line, wgsl_function};
    const SH_SAMPLE_WGSL: &str = include_str!("../shaders/sh_sample.wgsl");
    /// FNV-1a 64 of the extract below, taken from the parent's file.
    const PARENT_DEPTH_PATH_FNV1A: u64 = 0xd057_a202_d8c8_8c9f;
    let consts = [
        "SH_DEPTH_MIN_VARIANCE_M2",
        "SH_DEPTH_BIAS_CELL_FRACTION",
        "SH_DEPTH_MIN_VISIBILITY",
        "SH_DEPTH_TILE_REFERENCE_FLAG",
        "SH_DEPTH_TILE_INDEX_MASK",
        "SH_DEPTH_TILE_EDGE",
        "SH_DEPTH_TILE_BORDER",
        "SH_DEPTH_TILE_BORDERED_EDGE",
    ]
    .map(|name| wgsl_const_line(SH_SAMPLE_WGSL, name));
    let functions = [
        "sh_probe_depth_bias",
        "sh_depth_moment_pair",
        "sh_chebyshev_visibility",
        "oct_encode_l1",
        "sh_depth_tile_moments",
        "sh_corner_depth_visibility",
    ]
    .map(|name| wgsl_function(SH_SAMPLE_WGSL, name));
    let extract = consts
        .iter()
        .chain(&functions)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    let hash = extract
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    assert_eq!(
        hash, PARENT_DEPTH_PATH_FNV1A,
        "the depth path changed:\n{extract}"
    );
}
