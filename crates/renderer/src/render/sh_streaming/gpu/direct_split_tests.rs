//! GPU proofs of the streamed direct-SH promotion split: real drains and
//! composes write a promotion-free direct pool that no promotion weight moves
//! and a handoff pool that subtracts promoted lights, and the shared group 3
//! and mesh superset bind them respectively.
//! See: context/lib/rendering_pipeline.md §4 (Promoted static lights,
//! Cluster SH residency)

use super::*;
use crate::render::animated_direct_sh_compose::AnimatedDirectShDebugOverride;
use crate::render::direct_sh_compose::DirectShDebugOverride;
use crate::render::fluid_surface_frame_test::f16_rgb;
use crate::render::gpu_test_harness::{GpuCtx, bc_gpu_or_skip};
use crate::render::renderer_types::{PromotedBakedLightState, PromotedShadowPoolKind};
use crate::render::{ShSampleRegion, ShSampleRegionSets};
use postretro_render_cpu::frame_uniforms::{LightTermMask, UNIFORM_SIZE};
use postretro_render_cpu::sh_volume::BIND_SH_DIRECT_ATLAS;

/// f16 0.25 base radiance and 0.125 per-light delta.
const FILL: PayloadFill = PayloadFill {
    atlas: 0x3400,
    sparse_tile: 0x3000,
};
/// [`FILL`]'s per-light delta, decoded.
const LIGHT_DELTA: f32 = 0.125;
/// Slack on radiance f16 holds exactly (0.25 minus multiples of 0.0625).
const TOLERANCE: f32 = 1.0e-3;

/// The view a binding-15 entry names.
fn binding_15_view<'a>(entries: &'a [wgpu::BindGroupEntry<'a>]) -> &'a wgpu::TextureView {
    let mut matches = entries
        .iter()
        .filter(|entry| entry.binding == BIND_SH_DIRECT_ATLAS);
    let entry = matches.next().expect("binding 15 is bound");
    assert!(matches.next().is_none(), "binding 15 is bound once");
    match &entry.resource {
        wgpu::BindingResource::TextureView(view) => view,
        other => panic!("binding 15 must be a texture view, got {other:?}"),
    }
}

/// Binding 15 of the shared and mesh entries the production builder makes
/// over a fresh dense generation of `fixture`'s map.
fn assert_binding_15(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    fixture: &mut UploadOrderSh,
    expect: impl Fn(&super::super::DenseTextures) -> (&wgpu::TextureView, &wgpu::TextureView),
) {
    let shape = fixture.state.gpu.as_ref().unwrap().shape;
    let dense = super::super::DenseTextures::new(
        device,
        queue,
        &fixture.map.base(),
        &fixture.map.sources(),
        shape,
        &mut fixture.sh,
    )
    .expect("dense generation");
    let grid_info = super::super::create_grid_info(
        device,
        &fixture.map.base(),
        shape,
        false,
        fixture.sh.depth_tiles.grid_words(),
    );
    let depth_view = fixture.state.gpu.as_ref().unwrap().depth_moment_view();
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
    let (shared, mesh) = super::super::sample_bind_group_entries(
        &fixture.sh,
        &dense,
        &grid_info,
        &depth_view,
        &sampler,
    );
    let (expected_shared, expected_mesh) = expect(&dense);
    assert_eq!(binding_15_view(&shared), expected_shared);
    assert_eq!(binding_15_view(&mesh), expected_mesh);
}

#[test]
fn streamed_group_3_binds_the_promotion_free_pool_and_the_mesh_superset_the_handoff() {
    let Some(GpuCtx { device, queue }) = bc_gpu_or_skip(
        "streamed_group_3_binds_the_promotion_free_pool_and_the_mesh_superset_the_handoff",
    ) else {
        return;
    };
    let mut composed = UploadOrderSh::with_direct_compose(&device, &queue);
    assert_binding_15(&device, &queue, &mut composed, |dense| {
        let shared = &dense.direct_atlas.as_ref().unwrap().sampled_view;
        let mesh = &dense.direct_handoff.as_ref().unwrap().sampled_view;
        assert_ne!(shared.texture(), mesh.texture(), "two distinct pools");
        (shared, mesh)
    });
}

#[test]
fn streamed_base_only_level_binds_the_direct_base_to_both_groups() {
    let Some(GpuCtx { device, queue }) =
        bc_gpu_or_skip("streamed_base_only_level_binds_the_direct_base_to_both_groups")
    else {
        return;
    };
    // Id 35 without id 41/45 composes nothing: both groups sample the base.
    let mut base_only = UploadOrderSh::new(&device, &queue, 128);
    assert_binding_15(&device, &queue, &mut base_only, |dense| {
        assert!(dense.direct_atlas.is_none() && dense.direct_handoff.is_none());
        let base = dense.direct_base_view.as_ref().unwrap();
        (base, base)
    });
}

/// Every layer of an Rgba16Float pool, de-padded.
fn read_pool(device: &wgpu::Device, queue: &UploadQueue, texture: &wgpu::Texture) -> Vec<u8> {
    let size = texture.size();
    let row = size.width * 8;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("streamed direct split readback"),
        size: u64::from(padded * size.height * size.depth_or_array_layers),
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
                rows_per_image: Some(size.height),
            },
        },
        size,
    );
    queue.submit([encoder.finish()]);
    let (send, receive) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            send.send(result).unwrap();
        });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    receive.recv().unwrap().unwrap();
    let bytes = buffer
        .slice(..)
        .get_mapped_range()
        .unwrap()
        .chunks(padded as usize)
        .flat_map(|line| line[..row as usize].to_vec())
        .collect();
    buffer.unmap();
    bytes
}

/// One full-resident streamed compose at promotion weight `weight` for the
/// static light (through the selection override) and the animated light.
/// Returns `(promotion-free, handoff)` pool bytes.
fn compose_at(
    fixture: &mut UploadOrderSh,
    device: &wgpu::Device,
    queue: &UploadQueue,
    weight: f32,
) -> (Vec<u8>, Vec<u8>) {
    use wgpu::util::DeviceExt;
    let mask = LightTermMask::ALL;
    let promotion = DirectShDebugOverride {
        enabled: true,
        selection_index: 0,
        weight,
    };
    let animated = AnimatedDirectShDebugOverride::default();
    let animated_states = [PromotedBakedLightState {
        weight,
        pool_kind: Some(PromotedShadowPoolKind::Spot),
        ..PromotedBakedLightState::default()
    }];
    let regions = [ShSampleRegion::new(
        glam::Vec3::ZERO,
        glam::Vec3::splat(8.0),
    )];
    fixture
        .state
        .prepare_compose_frame(
            ShSampleRegionSets {
                visible_cells: &postretro_visibility::VisibleCells::DrawAll,
                fog_cells: &[],
                movers: &regions,
            },
            None,
            false,
            false,
            true,
            true,
            false,
            false,
            mask,
            promotion,
            animated,
            &[],
            &animated_states,
        )
        .expect("streamed compose plan");
    // Bytes 88..92 are `FrameUniforms::light_term_mask` (`build_uniform_data`).
    let mut uniform_bytes = [0u8; UNIFORM_SIZE];
    uniform_bytes[88..92].copy_from_slice(&mask.bits().to_ne_bytes());
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("streamed direct split uniforms"),
        contents: &uniform_bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("streamed direct split uniforms"),
        layout: &fixture.uniforms,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform.as_entire_binding(),
        }],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    fixture
        .state
        .dispatch_indirect_compose(queue, &mut encoder, &binding, false, mask, None)
        .expect("streamed indirect compose");
    fixture
        .state
        .dispatch_direct_compose(
            queue,
            &mut encoder,
            &binding,
            true,
            mask,
            promotion,
            animated,
            &animated_states,
            None,
            None,
        )
        .expect("streamed direct compose");
    queue.submit([encoder.finish()]);
    let gpu = fixture.state.gpu.as_ref().unwrap();
    (
        read_pool(device, queue, &gpu.direct_atlas.as_ref().unwrap().texture),
        read_pool(device, queue, &gpu.direct_handoff.as_ref().unwrap().texture),
    )
}

/// Every texel's decoded RGB; alpha is slot validity, not light.
fn texel_rgb(pool: &[u8]) -> Vec<f32> {
    pool.as_chunks::<8>()
        .0
        .iter()
        .flat_map(|texel| {
            let half = |at: usize| u16::from_le_bytes([texel[at], texel[at + 1]]);
            f16_rgb([half(0), half(2), half(4)]).to_array()
        })
        .collect()
}

/// Drains one cluster of `fixture`, composes it at promotion weights 0, 0.5
/// and 1, and proves the split: the promotion-free pool is written and never
/// moves, equals the handoff pool at weight zero, and the handoff pool loses
/// exactly `weight` times the promoted light's delta wherever it reaches.
/// The fixture's id-45 light has no animation descriptor, so its full scale
/// is zero and id 41's delta is the only one in play.
fn assert_streamed_split(device: &wgpu::Device, raw: wgpu::Queue, mut fixture: UploadOrderSh) {
    fixture.payload_fill = FILL;
    let queue = UploadQueue::new(device, raw, true);
    fixture.drain(device, &queue, 0);

    let (free_at_zero, handoff_at_zero) = compose_at(&mut fixture, device, &queue, 0.0);
    assert!(
        free_at_zero.iter().any(|&byte| byte != 0),
        "the drained cluster composes nonzero direct light",
    );
    assert_eq!(
        free_at_zero, handoff_at_zero,
        "with every promotion weight zero the two pools agree byte for byte",
    );
    let free_rgb = texel_rgb(&free_at_zero);
    for weight in [0.5, 1.0] {
        let (free, handoff) = compose_at(&mut fixture, device, &queue, weight);
        assert_eq!(
            free, free_at_zero,
            "the promotion-free pool must not move at weight {weight}",
        );
        let expected = weight * LIGHT_DELTA;
        let removed: Vec<f32> = free_rgb
            .iter()
            .zip(texel_rgb(&handoff))
            .map(|(free, handoff)| free - handoff)
            .collect();
        if let Some(stray) = removed
            .iter()
            .find(|&&lost| lost.abs() > TOLERANCE && (lost - expected).abs() > TOLERANCE)
        {
            panic!(
                "at weight {weight} a handoff channel lost {stray}; each must keep its \
                 promotion-free radiance or lose exactly {expected}",
            );
        }
        assert!(
            removed
                .iter()
                .any(|&lost| (lost - expected).abs() <= TOLERANCE),
            "the handoff pool must subtract {expected} of the promoted light at weight {weight}",
        );
    }
}

#[test]
fn streamed_promotion_free_pool_ignores_promotion_weights() {
    let Some(GpuCtx { device, queue: raw }) =
        bc_gpu_or_skip("streamed_promotion_free_pool_ignores_promotion_weights")
    else {
        return;
    };
    let fixture = UploadOrderSh::with_direct_compose(&device, &raw);
    assert_streamed_split(&device, raw, fixture);
}

/// Id 41 without id 45: the promotion pass alone stores both pools.
#[test]
fn streamed_static_only_promotion_pass_stores_both_pools() {
    let Some(GpuCtx { device, queue: raw }) =
        bc_gpu_or_skip("streamed_static_only_promotion_pass_stores_both_pools")
    else {
        return;
    };
    let fixture = UploadOrderSh::with_static_direct_compose(&device, &raw);
    let gpu = fixture.state.gpu.as_ref().unwrap();
    assert!(
        gpu.direct_intermediate.is_none(),
        "a level without id 45 allocates no Pass B intermediate",
    );
    assert_streamed_split(&device, raw, fixture);
}
