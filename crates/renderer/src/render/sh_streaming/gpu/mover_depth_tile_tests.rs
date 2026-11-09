//! GPU proofs for mover depth live tiles on a streamed level: the frame's
//! reference moves reach the resident texels that frame, beside the tiles
//! they point at, and live references survive eviction, reinstall and pool
//! growth, since the load's one table, not the session, owns them.
//! See: context/lib/rendering_pipeline.md §4 (Mover state lighting, Cluster SH residency)

use postretro_level_format::mover_probe_channel::MoverProbeChannelSection;
use postretro_level_format::probe_depth_tiles::PROBE_DEPTH_TILE_BORDERED_EDGE;

use super::depth_tile_texel_tests::{
    TILED, TILED_IN_ONE, UNTILED, assert_tiled, assert_untiled, install_and_promote, map_read,
    texels, tiles,
};
use super::*;
use crate::render::MoverTravel;
use crate::render::gpu_test_harness::{GpuCtx, bc_gpu_or_skip};
use crate::render::mover_channel::depth_test_support::{depth_section, ramp_tile};
use crate::render::mover_depth_tiles::write_mover_depth_tiles;

/// Mover 4 changes probes 9 and 10 (cluster 0) and 22 (cluster 1). Live
/// tiles follow id 59's two: 9 → 2, 10 → 3, 22 → 4.
const LIVE: [(u32, u16); 3] = [(TILED, 2), (UNTILED, 3), (TILED_IN_ONE, 4)];

fn d(probe: u32, terminus: u32) -> Vec<[u16; 2]> {
    ramp_tile(0.5 + probe as f32 / 8.0 + terminus as f32 * 3.0)
}

fn channel() -> MoverProbeChannelSection {
    depth_section(
        LIVE.iter()
            .map(|&(probe, _)| (4, probe, d(probe, 0), d(probe, 1)))
            .collect(),
    )
}

fn fixture(device: &wgpu::Device, raw: &wgpu::Queue) -> UploadOrderSh {
    UploadOrderSh::with_map_tiles_and_channel(
        device,
        raw,
        128,
        SyntheticMap::new([8, 4, 4], [1, 1, 1]).direct_base_only(),
        Some(&tiles()),
        Some(&channel()),
    )
}

/// One frame's live-tile step at the drain boundary, with `t` for mover 4
/// (`None`: no state).
fn hand_over(f: &mut UploadOrderSh, device: &wgpu::Device, queue: &UploadQueue, t: Option<f32>) {
    let states: Vec<MoverTravel> = t
        .map(|t| MoverTravel { mover_id: 4, t })
        .into_iter()
        .collect();
    write_mover_depth_tiles(&mut f.sh, Some(&mut f.state), device, queue, &states)
        .expect("the live-tile step");
}

/// Live tile `tile`'s bordered texels, read back through `queue` (its submit
/// carries the frame's staged writes first).
fn live_tile(
    f: &UploadOrderSh,
    device: &wgpu::Device,
    queue: &UploadQueue,
    tile: u16,
) -> Vec<[u16; 2]> {
    let tiles = &f.sh.depth_tiles;
    let texture = tiles.texture();
    let extent = texture.size();
    let row = extent.width * 4;
    let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("streamed live tile readback"),
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
    queue.submit([encoder.finish()]);
    let bytes = map_read(device, &buffer);
    let layout = tiles.layout().expect("a real tile texture");
    let [ox, oy] = layout.tile_origin(u32::from(tile));
    let edge = PROBE_DEPTH_TILE_BORDERED_EDGE as usize;
    (0..edge)
        .flat_map(|line| {
            let start = (oy as usize + line) * padded as usize + ox as usize * 4;
            bytes[start..start + edge * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|texel| {
                    [
                        u16::from_le_bytes([texel[0], texel[1]]),
                        u16::from_le_bytes([texel[2], texel[3]]),
                    ]
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Streamed half: the first hand-over at a
/// held terminus moves every resident changed probe's texel onto its live
/// tile on that frame, with no further drain, while the tile already holds
/// that terminus's D; a step from 1 to 0 rewrites the tiles that frame and
/// the texels keep pointing at them. Before the hand-over every texel is
/// id 59's or the averaged moments; losing the state restores them bit for
/// bit.
#[test]
fn a_streamed_first_hand_over_points_resident_texels_at_written_tiles_that_frame() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("streamed mover depth first frame")
    else {
        return;
    };
    let mut f = fixture(&device, &raw);
    let queue = UploadQueue::new(&device, raw, true);
    install_and_promote(&mut f, &device, &queue, 0);
    let before = texels(&f, &device, &queue);
    assert_tiled(&f, &before, TILED, 0, true);
    assert_untiled(&f, &before, UNTILED);

    hand_over(&mut f, &device, &queue, None);
    assert_eq!(
        texels(&f, &device, &queue),
        before,
        "no state: today's texels"
    );

    // Probe 22's cluster is not resident: its texel waits for promotion,
    // which packs G from the same table, while its tile is written now.
    hand_over(&mut f, &device, &queue, Some(1.0));
    let held = texels(&f, &device, &queue);
    for (probe, live) in LIVE {
        if probe != TILED_IN_ONE {
            assert_tiled(&f, &held, probe, live, true);
        }
        assert_eq!(
            live_tile(&f, &device, &queue, live),
            d(probe, 1),
            "probe {probe}: D₁"
        );
    }
    assert_eq!(held[TILED_IN_ONE as usize], before[TILED_IN_ONE as usize]);

    hand_over(&mut f, &device, &queue, Some(0.0));
    let stepped = texels(&f, &device, &queue);
    for (probe, live) in LIVE {
        if probe != TILED_IN_ONE {
            assert_tiled(&f, &stepped, probe, live, true);
        }
        assert_eq!(
            live_tile(&f, &device, &queue, live),
            d(probe, 0),
            "probe {probe}: D₀"
        );
    }

    hand_over(&mut f, &device, &queue, None);
    let restored = texels(&f, &device, &queue);
    assert_eq!(restored[TILED as usize], before[TILED as usize]);
    assert_eq!(restored[UNTILED as usize], before[UNTILED as usize]);
    eprintln!("[MoverDepthProof] streamed first frame and 1→0 step held; ran on GPU");
}

/// Streamed half: live references survive eviction (the
/// texel keeps G under the invalid word), reinstall, and dense-pool
/// growth; a probe whose cluster arrives after the hand-over promotes onto
/// its live tile. Live tiles never enter the pool, so their texels stay.
#[test]
fn live_references_survive_eviction_reinstall_and_pool_growth() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("streamed mover depth residency")
    else {
        return;
    };
    let mut f = fixture(&device, &raw);
    let queue = UploadQueue::new(&device, raw, true);
    install_and_promote(&mut f, &device, &queue, 0);
    hand_over(&mut f, &device, &queue, Some(0.5));
    let tile_before = live_tile(&f, &device, &queue, 2);

    let outcome = f
        .drain_batch(
            &device,
            &queue,
            ShDrainBatch {
                target_remove: vec![0],
                evictions: vec![0],
                ..ShDrainBatch::default()
            },
            &[],
        )
        .unwrap();
    assert_eq!(outcome.evicted, [0]);
    let evicted = texels(&f, &device, &queue);
    assert_tiled(&f, &evicted, TILED, 2, false);
    assert_tiled(&f, &evicted, UNTILED, 3, false);

    f.drain_batch(
        &device,
        &queue,
        ShDrainBatch {
            target_add: vec![0],
            mandatory: vec![0],
            ..ShDrainBatch::default()
        },
        &[0],
    )
    .unwrap();
    f.compose(&device, &queue);
    f.drain_many(&device, &queue, &[]).unwrap();
    assert_tiled(&f, &texels(&f, &device, &queue), TILED, 2, true);

    f.drain(&device, &queue, 1);
    assert_eq!(f.growth_events(), 1, "cluster 1 grows the dense pool");
    f.compose(&device, &queue);
    f.drain_many(&device, &queue, &[]).unwrap();
    let grown = texels(&f, &device, &queue);
    assert_tiled(&f, &grown, TILED, 2, true);
    assert_tiled(&f, &grown, UNTILED, 3, true);
    assert_tiled(&f, &grown, TILED_IN_ONE, 4, true);
    assert_eq!(
        live_tile(&f, &device, &queue, 2),
        tile_before,
        "the tile itself stays"
    );
    eprintln!("[MoverDepthProof] live references survived evict, reinstall, growth; ran on GPU");
}

/// Streamed half: with no id 59 the texture holds the
/// live tiles alone, and the resident changed probes reference them,
/// holding D₀, the frame of the first hand-over.
#[test]
fn a_streamed_level_without_id_59_points_at_live_tiles_alone() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("streamed live tiles without id 59")
    else {
        return;
    };
    let mut f = UploadOrderSh::with_map_tiles_and_channel(
        &device,
        &raw,
        128,
        SyntheticMap::new([8, 4, 4], [1, 1, 1]).direct_base_only(),
        None,
        Some(&channel()),
    );
    let queue = UploadQueue::new(&device, raw, true);
    assert_eq!(f.sh.depth_tiles.first_live_tile(), 0);
    assert_eq!(f.sh.depth_tiles.references().tile_count(), 3);
    install_and_promote(&mut f, &device, &queue, 0);
    hand_over(&mut f, &device, &queue, Some(0.0));
    let texels = texels(&f, &device, &queue);
    for (live, probe) in [(0, TILED), (1, UNTILED)] {
        assert_tiled(&f, &texels, probe, live, true);
        assert_eq!(live_tile(&f, &device, &queue, live), d(probe, 0));
    }
}
