//! GPU proofs that every streamed moment-texel writer packs a tiled probe's R
//! from id 34 and G from the load's tile-reference table (promotion,
//! eviction, session clear, pool growth), and that a runtime reference
//! rewrite reaches the resident texel on the next drain. Only the texture
//! shows what the last write in queue order left.
//! See: context/lib/rendering_pipeline.md §4 (Probe depth moments, Cluster SH residency)

use postretro_level_format::probe_depth_tiles::{
    MomentSecondChannel, PROBE_DEPTH_TILE_TEXELS, ProbeDepthTilesSection,
    decode_moment_second_channel,
};
use postretro_render_cpu::sh_depth_tiles::DepthTileGridWords;

use super::*;
use crate::render::gpu_test_harness::{GpuCtx, bc_gpu_or_skip};
use crate::render::sh_depth_tile_rewrite::set_depth_tile_reference;
use crate::render::sh_streaming::PROBE_INDIRECTION_VALID_BIT;
use crate::render::sh_streaming::install_tests::synthetic_moments;

/// Cluster 0 holds probes with x < 4, cluster 1 the rest (8×4×4 lattice).
const TILED: u32 = 9; // (1, 1, 0), cluster 0, id-59 tile 0
const TILED_IN_ONE: u32 = 22; // (6, 2, 0), cluster 1, id-59 tile 1
const UNTILED: u32 = 10; // (2, 1, 0), cluster 0

fn tiles() -> ProbeDepthTilesSection {
    ProbeDepthTilesSection {
        probe_indices: vec![TILED, TILED_IN_ONE],
        texels: vec![[0x3800, 0x3400]; 2 * PROBE_DEPTH_TILE_TEXELS],
    }
}

fn fixture(device: &wgpu::Device, raw: &wgpu::Queue) -> UploadOrderSh {
    UploadOrderSh::with_map_and_tiles(
        device,
        raw,
        128,
        SyntheticMap::new([8, 4, 4], [1, 1, 1]).direct_base_only(),
        Some(&tiles()),
    )
}

/// Every texel of the streamed moments texture, x fastest.
fn texels(f: &UploadOrderSh, device: &wgpu::Device, queue: &UploadQueue) -> Vec<[u16; 4]> {
    let texture = &f.state.gpu.as_ref().unwrap().depth_moments;
    let extent = texture.size();
    let row = extent.width * 8;
    let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let rows = extent.height * extent.depth_or_array_layers;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("streamed depth moments readback"),
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
    queue.submit([encoder.finish()]);
    let bytes = map_read(device, &buffer);
    bytes
        .chunks(padded as usize)
        .flat_map(|line| line[..row as usize].as_chunks::<8>().0.to_vec())
        .map(|texel| {
            let half = |at: usize| u16::from_le_bytes([texel[at], texel[at + 1]]);
            [half(0), half(2), half(4), half(6)]
        })
        .collect()
}

fn map_read(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Vec<u8> {
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
}

/// The live streamed grid uniform's bytes, through a compute copy: the
/// uniform carries no `COPY_SRC`.
fn grid_info_bytes(f: &UploadOrderSh, device: &wgpu::Device, queue: &UploadQueue) -> Vec<u8> {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("streamed grid readback"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../../uploads/grid_readback.wgsl").into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("streamed grid readback"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("streamed grid readback output"),
        size: 96,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("streamed grid readback map"),
        size: 96,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("streamed grid readback"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: f.state.gpu.as_ref().unwrap().grid_info.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &read, 0, 96);
    queue.submit([encoder.finish()]);
    map_read(device, &read)
}

fn word(texel: [u16; 4]) -> u32 {
    u32::from(texel[2]) | (u32::from(texel[3]) << 16)
}

/// `dense`'s texel: R its id-34 `E[d]`, G a reference to `tile`, B/A the
/// sampled word (valid exactly when `sampleable`).
fn assert_tiled(f: &UploadOrderSh, texels: &[[u16; 4]], dense: u32, tile: u16, sampleable: bool) {
    let texel = texels[dense as usize];
    assert_eq!(
        texel[0],
        synthetic_moments(dense)[0],
        "probe {dense}: R is id-34 E[d]"
    );
    assert_eq!(
        decode_moment_second_channel(texel[1]),
        MomentSecondChannel::Tile(tile),
        "probe {dense}: G references tile {tile}"
    );
    assert_eq!(word(texel), f.state.sampled_words[dense as usize]);
    assert_eq!(word(texel) & PROBE_INDIRECTION_VALID_BIT != 0, sampleable);
}

/// `dense`'s texel packs as an untiled probe: id-34 `E[d]` and `E[d²]`.
fn assert_untiled(f: &UploadOrderSh, texels: &[[u16; 4]], dense: u32) {
    let texel = texels[dense as usize];
    assert_eq!(
        [texel[0], texel[1]],
        synthetic_moments(dense),
        "probe {dense}"
    );
    assert_eq!(word(texel), f.state.sampled_words[dense as usize]);
}

fn install_and_promote(
    f: &mut UploadOrderSh,
    device: &wgpu::Device,
    queue: &UploadQueue,
    cluster: u32,
) {
    f.drain(device, queue, cluster);
    f.compose(device, queue);
    f.drain_many(device, queue, &[]).unwrap();
    assert!(f.state.sampleable.contains(&cluster));
}

fn rewrite(f: &mut UploadOrderSh, queue: &UploadQueue, probe: u32, tile: Option<u16>) {
    set_depth_tile_reference(&mut f.sh, Some(&mut f.state), queue, probe, tile)
        .expect("a valid probe and an existing tile");
}

/// O3, O8: promotion, eviction, eviction then reinstall, and session clear
/// then reinstall each leave the tiled probe's R at id-34 `E[d]` and G on its
/// tile; untiled probes keep `E[d²]`.
#[test]
fn every_streamed_texel_writer_keeps_e_d_in_r_and_the_tile_in_g() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("streamed depth tile texel writers")
    else {
        return;
    };
    let mut f = fixture(&device, &raw);
    let queue = UploadQueue::new(&device, raw, true);

    install_and_promote(&mut f, &device, &queue, 0);
    let after_promote = texels(&f, &device, &queue);
    assert_tiled(&f, &after_promote, TILED, 0, true);
    assert_untiled(&f, &after_promote, UNTILED);

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
    let after_evict = texels(&f, &device, &queue);
    assert_tiled(&f, &after_evict, TILED, 0, false);
    assert_untiled(&f, &after_evict, UNTILED);

    let outcome = f
        .drain_batch(
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
    assert_eq!(outcome.accepted, [0]);
    f.compose(&device, &queue);
    f.drain_many(&device, &queue, &[]).unwrap();
    assert!(f.state.sampleable.contains(&0));
    assert_tiled(&f, &texels(&f, &device, &queue), TILED, 0, true);

    // A new generation clears the session: zero moments under the invalid
    // word. The table, not the session, owns the reference.
    let generation = f.state.generation + 1;
    let content_tag = f.state.content_tag;
    f.state
        .drain(
            &device,
            &queue,
            &mut f.sh,
            &f.weights,
            ShDrainBatch {
                generation,
                content_tag,
                target_reset: Some(vec![0b1]),
                ..ShDrainBatch::default()
            },
        )
        .unwrap();
    let cleared = texels(&f, &device, &queue)[TILED as usize];
    assert_eq!((cleared[0], word(cleared)), (0, 0), "cleared texel");
    install_and_promote(&mut f, &device, &queue, 0);
    let reinstalled = texels(&f, &device, &queue);
    assert_tiled(&f, &reinstalled, TILED, 0, true);
    assert_untiled(&f, &reinstalled, UNTILED);
    eprintln!("[DepthTileProof] streamed promote/evict/clear writers carried the tile; ran on GPU");
}

/// O4: dense-pool growth rebinds the sample groups; the tiled probe's texel
/// and the grid uniform's tile words reach the grown generation unchanged.
#[test]
fn a_tiled_probe_keeps_its_texel_and_tile_words_across_dense_pool_growth() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("streamed depth tiles across growth")
    else {
        return;
    };
    let mut f = fixture(&device, &raw);
    let queue = UploadQueue::new(&device, raw, true);
    let words = DepthTileGridWords::for_layout(f.sh.depth_tiles.references().texture_layout());
    assert_ne!(words, DepthTileGridWords::NONE);
    let tile_words = |bytes: &[u8]| {
        let at = |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        (at(28), f32::from_bits(at(44)), f32::from_bits(at(92)))
    };
    let expected = (words.row_shift, words.inv_width, words.inv_height);

    install_and_promote(&mut f, &device, &queue, 0);
    assert_eq!(tile_words(&grid_info_bytes(&f, &device, &queue)), expected);
    assert_eq!(f.growth_events(), 0);

    f.drain(&device, &queue, 1);
    assert_eq!(f.growth_events(), 1, "cluster 1 grows the dense pool");
    assert_tiled(&f, &texels(&f, &device, &queue), TILED, 0, true);
    assert_eq!(
        tile_words(&grid_info_bytes(&f, &device, &queue)),
        expected,
        "the grown generation's grid uniform locates the same tiles"
    );

    f.compose(&device, &queue);
    f.drain_many(&device, &queue, &[]).unwrap();
    assert!(f.state.sampleable.contains(&1));
    let grown = texels(&f, &device, &queue);
    assert_tiled(&f, &grown, TILED, 0, true);
    assert_tiled(&f, &grown, TILED_IN_ONE, 1, true);
    eprintln!("[DepthTileProof] tile texel and words survived dense growth; ran on GPU");
}

/// O5–O7 on a streamed level: a rewrite reaches a resident texel at the next
/// drain, on a probe with or without a tile; clearing restores the
/// never-tiled texel bit for bit; a redirect survives eviction and reinstall.
#[test]
fn a_streamed_rewrite_reaches_the_resident_texel_next_drain_and_clearing_restores_it() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("streamed depth tile rewrite") else {
        return;
    };
    let mut f = fixture(&device, &raw);
    let queue = UploadQueue::new(&device, raw, true);
    install_and_promote(&mut f, &device, &queue, 0);
    let before = texels(&f, &device, &queue);

    rewrite(&mut f, &queue, UNTILED, Some(1));
    assert_eq!(
        texels(&f, &device, &queue)[UNTILED as usize],
        before[UNTILED as usize],
        "nothing lands before the next drain"
    );
    f.drain_many(&device, &queue, &[]).unwrap();
    assert_tiled(&f, &texels(&f, &device, &queue), UNTILED, 1, true);

    rewrite(&mut f, &queue, UNTILED, None);
    f.drain_many(&device, &queue, &[]).unwrap();
    assert_eq!(
        texels(&f, &device, &queue)[UNTILED as usize],
        before[UNTILED as usize],
        "cleared: the never-tiled texel, bit for bit"
    );

    rewrite(&mut f, &queue, TILED, None);
    f.drain_many(&device, &queue, &[]).unwrap();
    assert_untiled(&f, &texels(&f, &device, &queue), TILED);

    // O6: redirect, evict, reinstall.
    rewrite(&mut f, &queue, TILED, Some(1));
    f.drain_many(&device, &queue, &[]).unwrap();
    assert_tiled(&f, &texels(&f, &device, &queue), TILED, 1, true);
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
    assert_tiled(&f, &texels(&f, &device, &queue), TILED, 1, false);
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
    assert_tiled(&f, &texels(&f, &device, &queue), TILED, 1, true);
    eprintln!("[DepthTileProof] streamed rewrite, clear and evict/reinstall held; ran on GPU");
}

/// O5: a rewrite and the promotion of its probe's cluster in one drain keep
/// the rewrite, whichever writer runs first in the batch, and whether the
/// rewrite came before the cluster's install or after it.
#[test]
fn a_rewrite_and_its_clusters_promotion_in_one_drain_keep_the_rewrite_in_either_order() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("streamed depth tile same drain")
    else {
        return;
    };
    let queue = UploadQueue::new(&device, raw.clone(), true);

    // The rewrite precedes the cluster's install.
    let mut f = fixture(&device, &raw);
    rewrite(&mut f, &queue, TILED_IN_ONE, Some(0));
    install_and_promote(&mut f, &device, &queue, 1);
    assert_tiled(&f, &texels(&f, &device, &queue), TILED_IN_ONE, 0, true);

    // The cluster is installed and composed; the rewrite lands before the
    // drain that promotes it, and that drain's batch carries both writers,
    // in each order.
    for flush_first in [false, true] {
        let mut f = fixture(&device, &raw);
        f.drain(&device, &queue, 0);
        f.compose(&device, &queue);
        rewrite(&mut f, &queue, TILED, Some(1));
        rewrite(&mut f, &queue, UNTILED, Some(0));
        let references = f.sh.depth_tiles.references();
        let mut uploads = f.state.begin_uploads(0);
        if flush_first {
            f.state
                .flush_depth_tile_texels(&mut uploads, references)
                .unwrap();
            f.state.promote_completed(&mut uploads, references).unwrap();
        } else {
            f.state.promote_completed(&mut uploads, references).unwrap();
            f.state
                .flush_depth_tile_texels(&mut uploads, references)
                .unwrap();
        }
        f.state.submit_uploads(uploads, &device, &queue);
        assert!(f.state.sampleable.contains(&0));
        let both = texels(&f, &device, &queue);
        assert_tiled(&f, &both, TILED, 1, true);
        assert_tiled(&f, &both, UNTILED, 0, true);
    }
    eprintln!("[DepthTileProof] same-drain rewrite won in both orders; ran on GPU");
}

/// A streamed level's seam names only valid probes of its own lattice.
#[test]
fn a_streamed_rewrite_refuses_a_probe_outside_the_lattice_or_invalid() {
    use crate::render::ProbeDepthTileRewriteError as Error;
    let mut state = SyntheticMap::new([8, 4, 4], [1, 1, 1]).state();
    assert_eq!(
        state.check_depth_tile_probe(128),
        Err(Error::ProbeOutsideLattice {
            probe: 128,
            probe_count: 128
        })
    );
    state.base_metadata.probes[5].validity = 0;
    assert_eq!(
        state.check_depth_tile_probe(5),
        Err(Error::InvalidProbe { probe: 5 })
    );
    assert_eq!(state.check_depth_tile_probe(TILED), Ok(()));
}
