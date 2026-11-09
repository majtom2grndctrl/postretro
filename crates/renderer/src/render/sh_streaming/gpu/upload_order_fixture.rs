//! Real SH drains with undersized initial backing force production growth
//! and upload submissions. The test fixture's reserved CSR entry makes copy
//! ordering observable without corrupting any resident row's light index.

use std::collections::BTreeMap;

use postretro_level_format::cluster_sh_payloads::CLUSTER_SH_SPARSE_ROW_RECORD_SIZE;

use super::*;
use crate::render::sh_streaming::install_tests::SyntheticMap;
use crate::render::sh_streaming::{ShDrainBatch, ShResidencyState};
use crate::render::sh_volume::ShVolumeSections;
use crate::render::uploads::UploadQueue;
use wgpu::util::DeviceExt;

/// f16 bit patterns a prepared cluster's payloads carry: every atlas texel's
/// RGB, and every sparse delta tile half. Zero is the plain fixture.
#[derive(Clone, Copy, Default)]
pub(in crate::render::sh_streaming) struct PayloadFill {
    pub(in crate::render::sh_streaming) atlas: u16,
    pub(in crate::render::sh_streaming) sparse_tile: u16,
}

impl PayloadFill {
    /// A filled texel is a valid one; the plain fixture stays all zero.
    fn atlas_alpha(self) -> u16 {
        if self.atlas == 0 { 0 } else { 0x3C00 }
    }
}

pub(crate) struct UploadOrderSh {
    map: SyntheticMap,
    pub(in crate::render::sh_streaming) state: ShResidencyState,
    sh: ShVolumeResources,
    uniforms: wgpu::BindGroupLayout,
    pipelines: crate::render::sh_streaming::StreamingShPipelines,
    weights: wgpu::Buffer,
    /// What [`Self::prepared`] writes into atlas texels and sparse tiles.
    pub(in crate::render::sh_streaming) payload_fill: PayloadFill,
}

impl UploadOrderSh {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue, dense_slots: u32) -> Self {
        Self::with_map(
            device,
            queue,
            dense_slots,
            SyntheticMap::new([8, 4, 4], [1, 1, 1]).direct_base_only(),
        )
    }

    pub(crate) fn with_direct_compose(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self::with_map(device, queue, 128, SyntheticMap::new([8, 4, 4], [1, 1, 1]))
    }

    /// Id 41 without id 45: the promotion pass stores both direct outputs.
    pub(in crate::render::sh_streaming) fn with_static_direct_compose(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Self {
        Self::with_map(
            device,
            queue,
            128,
            SyntheticMap::new([8, 4, 4], [1, 1, 1]).without_animated_direct(),
        )
    }

    fn with_map(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dense_slots: u32,
        map: SyntheticMap,
    ) -> Self {
        Self::with_map_and_tiles(device, queue, dense_slots, map, None)
    }

    /// `map` with id 59 `tiles`: the SH resources hold the load's tile
    /// texture and reference table, as a streamed level install builds them.
    fn with_map_and_tiles(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dense_slots: u32,
        map: SyntheticMap,
        tiles: Option<&postretro_level_format::probe_depth_tiles::ProbeDepthTilesSection>,
    ) -> Self {
        Self::with_map_tiles_and_channel(device, queue, dense_slots, map, tiles, None)
    }

    /// Id 41 without id 45, with an id-57 `channel` laid out as a streamed
    /// level install lays it out.
    pub(in crate::render::sh_streaming) fn with_static_direct_compose_and_channel(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        channel: &postretro_level_format::mover_probe_channel::MoverProbeChannelSection,
    ) -> Self {
        Self::with_map_tiles_and_channel(
            device,
            queue,
            128,
            SyntheticMap::new([8, 4, 4], [1, 1, 1]).without_animated_direct(),
            None,
            Some(channel),
        )
    }

    fn with_map_tiles_and_channel(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dense_slots: u32,
        map: SyntheticMap,
        tiles: Option<&postretro_level_format::probe_depth_tiles::ProbeDepthTilesSection>,
        channel: Option<&postretro_level_format::mover_probe_channel::MoverProbeChannelSection>,
    ) -> Self {
        let state = map.state();
        let mut ledger = crate::render::sh_residency::ShAllocationLedger::new();
        let mover_channel = match channel {
            Some(section) => crate::render::mover_channel::MoverChannelResources::new(
                device,
                &crate::render::mover_channel::MoverChannelInputs {
                    section: Some(section),
                    entity_shadow_lights: &[],
                    static_terms: None,
                    animated_terms: None,
                    animation_descriptor_indices: &[],
                    indirect_descriptor_indices: &[],
                },
                true,
                |_, _| None,
                &mut ledger,
            )
            .expect("the fixture channel lays out"),
            None => crate::render::mover_channel::MoverChannelResources::empty(device),
        };
        let sh = ShVolumeResources::new(
            device,
            queue,
            ShVolumeSections {
                depth_tiles: tiles,
                sh: None,
                stream_base_present: true,
                stream_animation_descriptors: Some(&[]),
                indirect_delta_present: true,
                direct: None,
                direct_delta: None,
                animated_direct_delta: None,
                billboard_direct_scatter: None,
                animated_billboard_direct_scatter_delta: None,
            },
            1,
            false,
            mover_channel,
            &mut ledger,
        );
        let uniforms = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SH upload-order uniform layout"),
            entries: &crate::render::pipeline_layout::uniform_bind_group_layout_entries(),
        });
        let weights = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SH upload-order promotion weights"),
            contents: &[0; 4],
            usage: wgpu::BufferUsages::STORAGE,
        });
        let mut fixture = Self {
            map,
            state,
            sh,
            pipelines: crate::render::sh_streaming::StreamingShPipelines::new(device, &uniforms),
            uniforms,
            weights,
            payload_fill: PayloadFill::default(),
        };
        // Only the initial capacities are reduced. Allocation, installation,
        // growth, retirement and submission all run through production code.
        fixture.state.gpu = Some(
            StreamingGpuPools::new(
                device,
                &device.limits(),
                queue,
                &fixture.map.base(),
                &fixture.map.sources(),
                std::sync::Arc::clone(&fixture.state.sparse_compose),
                InitialPool {
                    dense_slots,
                    sparse_capacities: BTreeMap::new(),
                    whole_map: WholeMapCap {
                        dense_slots: 128,
                        ..WholeMapCap::uncapped()
                    },
                    pool_bytes: 0,
                },
                false,
                &mut fixture.sh,
                &fixture.pipelines,
                &fixture.weights,
            )
            .expect("minimal SH pools"),
        );
        // The 128 above fixes geometry. These pools undersize their map on
        // purpose, so growth here answers to the device alone.
        fixture.state.gpu.as_mut().unwrap().whole_map = WholeMapCap::uncapped();
        fixture.state.mover_channel_rows = fixture.sh.mover_channel.rows().clone();
        fixture
    }

    pub(crate) fn retained_source(&self) -> wgpu::Buffer {
        self.state
            .gpu
            .as_ref()
            .unwrap()
            .indirect_compose
            .affinity_lights_for_test()
            .clone()
    }

    pub(in crate::render::sh_streaming) fn prepared(
        &self,
        cluster: u32,
    ) -> postretro_level_loader::PreparedShCluster {
        let fill = self.payload_fill;
        let mut ready = self.map.prepared(&self.state, cluster);
        // CPU transaction fixtures omit atlas texels; supply real 8x8 slot
        // payloads here so the exact GPU validators and uploads also execute.
        let mut bytes = Vec::new();
        for block in &mut ready.chunk.blocks {
            let previous = block.body.clone();
            let start = bytes.len();
            if block.kind == crate::render::sh_streaming::ISOLATED_ATLAS_BLOCK {
                let slots = block.element_count;
                for word in [IRRADIANCE_FORMAT_RGBA16F, slots, slots * 8, 8, 1] {
                    bytes.extend_from_slice(&word.to_le_bytes());
                }
                for _ in 0..slots as usize * 8 * 8 {
                    for half in [fill.atlas, fill.atlas, fill.atlas, fill.atlas_alpha()] {
                        bytes.extend_from_slice(&half.to_le_bytes());
                    }
                }
            } else if block.kind == crate::render::sh_streaming::SPARSE_ROWS_BLOCK {
                let body = &ready.chunk.bytes[previous];
                let word =
                    |offset| u32::from_le_bytes(body[offset..offset + 4].try_into().unwrap());
                let rows = word(0);
                // Each source row is L2 with one stored tile. Provide the full
                // stride, not the CPU-only fixture's abbreviated f16 payload.
                let stride = delta_probe_f16_stride(4) as u32;
                let entries = word(4);
                let mut words = vec![rows, entries, entries * stride, 0];
                let mut entry = 0;
                for row in 0..rows {
                    let offset = 16 + row as usize * CLUSTER_SH_SPARSE_ROW_RECORD_SIZE;
                    let count = word(offset + 8);
                    words.extend_from_slice(&[word(offset), entry, count]);
                    entry += count;
                }
                for entry in 0..entries {
                    words.extend_from_slice(&[0, entry * stride, stride, 0]);
                }
                let tile_word = u32::from(fill.sparse_tile) | (u32::from(fill.sparse_tile) << 16);
                words.resize(words.len() + (entries * stride / 2) as usize, tile_word);
                for word in words {
                    bytes.extend_from_slice(&word.to_le_bytes());
                }
            } else {
                bytes.extend_from_slice(&ready.chunk.bytes[previous]);
            }
            block.body = start..bytes.len();
        }
        ready.chunk.bytes = bytes;
        ready
    }

    pub(crate) fn drain(&mut self, device: &wgpu::Device, queue: &UploadQueue, cluster: u32) {
        let outcome = self
            .drain_many(device, queue, &[cluster])
            .expect("production SH drain");
        assert_eq!(outcome.accepted, [cluster]);
    }

    /// Every ready cluster is mandatory here: the old contract's "may grow".
    pub(in crate::render::sh_streaming) fn drain_many(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        clusters: &[u32],
    ) -> Result<postretro_level_loader::ShDrainOutcome, ShResidencyDrainError> {
        let mut mandatory = clusters.to_vec();
        mandatory.sort_unstable();
        self.drain_batch(
            device,
            queue,
            ShDrainBatch {
                mandatory,
                ..ShDrainBatch::default()
            },
            clusters,
        )
    }

    /// `batch`'s target deltas and capacity policy, with `clusters` ready.
    pub(in crate::render::sh_streaming) fn drain_batch(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        batch: ShDrainBatch,
        clusters: &[u32],
    ) -> Result<postretro_level_loader::ShDrainOutcome, ShResidencyDrainError> {
        let ready = clusters
            .iter()
            .map(|&cluster| self.prepared(cluster))
            .collect();
        self.state.drain(
            device,
            queue,
            &mut self.sh,
            &self.weights,
            ShDrainBatch {
                generation: self.state.generation,
                content_tag: self.state.content_tag,
                ready,
                ..batch
            },
        )
    }

    #[cfg(feature = "dev-tools")]
    pub(in crate::render::sh_streaming) fn evict_and_reinstall(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        cluster: u32,
    ) {
        let ready = self.prepared(cluster);
        self.state.targets.remove(&cluster);
        let mut uploads = self.state.begin_uploads(0);
        self.state
            .evict(&mut uploads, self.sh.depth_tiles.references(), cluster)
            .unwrap();
        self.state
            .flush_mover_releases(&mut uploads, &mut self.sh.mover_channel)
            .unwrap();
        self.state.submit_uploads(uploads, device, queue);
        self.state.targets.insert(cluster);
        let mut gpu = super::super::InstallGpu {
            device,
            queue,
            sh: &mut self.sh,
            selection_weights: &self.weights,
        };
        self.state.install(Some(&mut gpu), &ready).unwrap();
        self.state
            .probe_readback
            .reconcile(self.state.grid_dimensions, &self.state.sampled_words);
    }

    pub(crate) fn compose(&mut self, device: &wgpu::Device, queue: &UploadQueue) {
        use crate::render::animated_direct_sh_compose::AnimatedDirectShDebugOverride;
        use crate::render::direct_sh_compose::DirectShDebugOverride;
        use crate::render::{ShSampleRegion, ShSampleRegionSets};
        use postretro_render_cpu::frame_uniforms::LightTermMask;
        let mut mask = LightTermMask::ALL;
        let mut promotion = DirectShDebugOverride::default();
        let mut animated = AnimatedDirectShDebugOverride::default();
        if self.state.direct_compose_required {
            mask.set_enabled(LightTermMask::SPECULAR, false);
            promotion = DirectShDebugOverride {
                enabled: true,
                selection_index: 0,
                weight: 0.25,
            };
            animated = AnimatedDirectShDebugOverride {
                enabled: true,
                light_index: 0,
            };
        }
        let regions = [ShSampleRegion::new(
            glam::Vec3::ZERO,
            glam::Vec3::splat(8.0),
        )];
        self.state
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
                animated.active(),
                mask,
                promotion,
                animated,
                &[],
                &[],
                &[],
            )
            .expect("real streamed compose plan");
        // Bytes 88..92 are `FrameUniforms::light_term_mask` (`build_uniform_data`).
        let mut uniform_bytes = [0u8; postretro_render_cpu::frame_uniforms::UNIFORM_SIZE];
        uniform_bytes[88..92].copy_from_slice(&mask.bits().to_ne_bytes());
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("streamed compose inventory uniforms"),
            contents: &uniform_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("streamed compose inventory binding"),
            layout: &self.uniforms,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        let before = queue.counts();
        self.state
            .dispatch_indirect_compose(queue, &mut encoder, &binding, false, mask, None)
            .expect("real streamed compose dispatch");
        if self.state.direct_compose_required {
            self.state
                .dispatch_direct_compose(
                    queue,
                    &mut encoder,
                    &binding,
                    true,
                    mask,
                    promotion,
                    animated,
                    &[],
                    None,
                    None,
                )
                .expect("real streamed promotion and animated compose dispatch");
            assert!(self.state.snapshot().static_direct_compose.rows_composed > 0);
            assert!(self.state.snapshot().animated_direct_compose.rows_composed > 0);
        }
        assert!(self.state.snapshot().indirect_compose.rows_composed > 0);
        assert!(
            queue.counts().writes > before.writes,
            "streamed compose grid stages its actual writer"
        );
        assert_eq!(queue.counts().direct_writes, before.direct_writes);
        queue.submit([encoder.finish()]);
        queue.assert_empty("streamed compose frame exit");
        assert_eq!(queue.counts().batches, before.batches + 1);
    }

    pub(crate) fn growth_events(&self) -> u64 {
        self.state.gpu.as_ref().unwrap().growth.events
    }
}

#[path = "depth_tile_texel_tests.rs"]
mod depth_tile_texel_tests;
#[path = "direct_split_tests.rs"]
mod direct_split_tests;
#[path = "limit_tests.rs"]
mod limit_tests;
#[path = "mover_depth_tile_tests.rs"]
mod mover_depth_tile_tests;
#[path = "pool_admission_tests.rs"]
mod pool_admission_tests;
#[path = "shared_storage_tests.rs"]
mod shared_storage_tests;

#[cfg(test)]
#[path = "mover_channel_tests.rs"]
mod mover_channel_tests;
