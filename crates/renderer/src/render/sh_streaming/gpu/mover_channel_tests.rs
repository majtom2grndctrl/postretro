//! GPU proof of the mover channel on the streamed direct compose: a drained
//! cluster's channel row composes the same gating the whole-load passes do,
//! and only when the mover's t′ moves.
//! See: context/lib/rendering_pipeline.md §4 (Mover state lighting, Cluster
//! SH residency)

use super::*;
use crate::render::MoverTravel;
use crate::render::animated_direct_sh_compose::AnimatedDirectShDebugOverride;
use crate::render::direct_sh_compose::DirectShDebugOverride;
use crate::render::fluid_surface_frame_test::f16_rgb;
use crate::render::gpu_test_harness::{GpuCtx, bc_gpu_or_skip};
use crate::render::{ShSampleRegion, ShSampleRegionSets};
use postretro_level_format::mover_probe_channel::{
    MoverChannelCell, MoverDirectRecord, MoverLightNamespace, MoverLightRampRecord,
    MoverPayloadPlacement, MoverProbeChannelSection,
};
use postretro_render_cpu::frame_uniforms::{LightTermMask, UNIFORM_SIZE};

/// f16 0.25 base radiance, and the gated light's own tile.
const FILL: PayloadFill = PayloadFill {
    atlas: 0x3400,
    sparse_tile: 0x3000,
};
const TERM: f32 = 0.125;
const TOLERANCE: f32 = 1.0e-3;

/// One bake-only light at row 0, probe 0, closed by mover 7 at terminus 0.
fn channel() -> MoverProbeChannelSection {
    MoverProbeChannelSection {
        affinity_factor: 4,
        affinity_dims: [2, 1, 1],
        direct_placement: MoverPayloadPlacement::Resident,
        bounce_placement: MoverPayloadPlacement::Resident,
        cells: vec![MoverChannelCell {
            cell: 0,
            valid_probe_mask: u64::MAX,
        }],
        direct_offsets: vec![0, 1],
        direct_records: vec![MoverDirectRecord {
            namespace: MoverLightNamespace::BakeOnlyStaticLight,
            carries_own_tile: true,
            light_index: 0,
            probe_mask: 1,
        }],
        mover_offsets: vec![0, 1],
        direct_movers: vec![7],
        visibility: vec![0, 255],
        own_tiles: vec![0x3000; 108],
        bounce_offsets: vec![0, 0],
        bounce_entries: Vec::new(),
        bounce_tiles: Vec::new(),
        depth_records: Vec::new(),
        depth_tiles: Vec::new(),
        ramps: vec![MoverLightRampRecord {
            mover_id: 7,
            start: 0.0,
            end: 1.0,
        }],
    }
}

/// One planned and recorded streamed frame at mover 7's `t`. Returns the
/// promotion-free pool's RGB and the rows Pass A composed.
fn compose_at(
    fixture: &mut UploadOrderSh,
    device: &wgpu::Device,
    queue: &UploadQueue,
    states: &[MoverTravel],
) -> (Vec<f32>, usize) {
    use wgpu::util::DeviceExt;
    let mask = LightTermMask::ALL;
    let promotion = DirectShDebugOverride::default();
    let animated = AnimatedDirectShDebugOverride::default();
    let regions = [ShSampleRegion::new(
        glam::Vec3::ZERO,
        glam::Vec3::splat(8.0),
    )];
    fixture.sh.mover_channel.set_states(states);
    let changes = fixture.sh.mover_channel.observe_t_changes().to_vec();
    fixture.sh.mover_channel.upload_t(queue);
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
            false,
            false,
            false,
            mask,
            promotion,
            animated,
            &[],
            &[],
            &changes,
        )
        .expect("streamed compose plan");
    fixture.sh.mover_channel.commit_t_changes();
    let mut uniform_bytes = [0u8; UNIFORM_SIZE];
    uniform_bytes[88..92].copy_from_slice(&mask.bits().to_ne_bytes());
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("streamed mover channel uniforms"),
        contents: &uniform_bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("streamed mover channel uniforms"),
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
            false,
            mask,
            promotion,
            animated,
            &[],
            None,
            None,
        )
        .expect("streamed direct compose");
    queue.submit([encoder.finish()]);
    // The counter reports the last recorded frame's rows.
    let composed = fixture.state.snapshot().static_direct_compose.rows_composed;
    let gpu = fixture.state.gpu.as_ref().unwrap();
    let pool = super::direct_split_tests::read_pool(
        device,
        queue,
        &gpu.direct_atlas.as_ref().unwrap().texture,
    );
    let rgb = pool
        .as_chunks::<8>()
        .0
        .iter()
        .flat_map(|texel| {
            let half = |at: usize| u16::from_le_bytes([texel[at], texel[at + 1]]);
            f16_rgb([half(0), half(2), half(4)]).to_array()
        })
        .collect();
    (rgb, composed as usize)
}

fn state(t: f32) -> MoverTravel {
    MoverTravel { mover_id: 7, t }
}

/// Streamed half (the whole-load half is `compose_tests`): a drained
/// cluster composes the channel's gating, S₀ at t′ = 0 against the ungated
/// pool at t′ = 1 on exactly one probe tile, and their mix at 0.5. A
/// frame whose t′ holds composes no row; one whose t′ moves composes only
/// the mover's row.
#[test]
fn a_drained_cluster_composes_the_channel_gate_and_only_the_movers_row() {
    let Some(GpuCtx { device, queue: raw }) =
        bc_gpu_or_skip("a_drained_cluster_composes_the_channel_gate_and_only_the_movers_row")
    else {
        return;
    };
    let section = channel();
    let mut fixture =
        UploadOrderSh::with_static_direct_compose_and_channel(&device, &raw, &section);
    fixture.payload_fill = FILL;
    let queue = UploadQueue::new(&device, raw, true);
    fixture.drain(&device, &queue, 0);

    let (open, settling) = compose_at(&mut fixture, &device, &queue, &[state(1.0)]);
    assert!(settling > 0, "the install composes its rows");
    let (held, composed) = compose_at(&mut fixture, &device, &queue, &[state(1.0)]);
    assert_eq!(held, open);
    assert_eq!(composed, 0, "t′ held: no row recomposes");

    for (t, loss) in [(0.0, TERM), (0.5, TERM * 0.5)] {
        let (gated, composed) = compose_at(&mut fixture, &device, &queue, &[state(t)]);
        assert_eq!(composed, 1, "t′ {t}: only row 0, the mover's, recomposes");
        let mut changed = 0;
        for (open, gated) in open.iter().zip(&gated) {
            if (open - gated).abs() > TOLERANCE {
                assert!(
                    (open - gated - loss).abs() <= TOLERANCE,
                    "t′ {t}: a channel lost {}, expected {loss}",
                    open - gated
                );
                changed += 1;
            }
        }
        assert_eq!(changed, 36 * 3, "t′ {t}: one probe's 6×6 RGB tile moves");
    }
}

/// Every cluster covering a pooled channel cell carries its id-57 row. A
/// cluster whose dense rows cover one without it would leave the cell's
/// payload unplaced, composing it ungated with nothing said, so the cluster
/// is refused as malformed and installs nothing, as a chunk omitting a
/// required dense member is.
#[test]
fn a_dense_row_without_its_pooled_cells_channel_row_refuses_the_cluster() {
    let Some(GpuCtx { device, queue: raw }) =
        bc_gpu_or_skip("a_dense_row_without_its_pooled_cells_channel_row_refuses_the_cluster")
    else {
        return;
    };
    // The direct payload streams with id 50's cells; this fixture's clusters
    // carry no id-57 rows.
    let mut section = channel();
    section.direct_placement = MoverPayloadPlacement::Streamed;
    section.visibility.clear();
    section.own_tiles.clear();
    let mut fixture =
        UploadOrderSh::with_static_direct_compose_and_channel(&device, &raw, &section);
    assert!(fixture.sh.mover_channel.pools_cell(0));
    fixture.payload_fill = FILL;
    let queue = UploadQueue::new(&device, raw, true);
    let error = fixture
        .drain_many(&device, &queue, &[0])
        .expect_err("a dense row without its channel row is refused");
    assert!(
        matches!(
            error,
            ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "MoverProbeChannel (id 57) cell is malformed or incomplete",
            }
        ),
        "{error:?}"
    );
    assert!(fixture.state.installed.is_empty(), "nothing installs");
    assert_eq!(fixture.sh.mover_channel.pool_bytes(), (0, 0));
}
