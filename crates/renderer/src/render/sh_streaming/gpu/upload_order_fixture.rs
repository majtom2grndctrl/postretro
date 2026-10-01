//! Real SH drains with undersized initial backing force production growth
//! and upload submissions. The test fixture's reserved CSR entry makes copy
//! ordering observable without corrupting any resident row's light index.

use std::collections::BTreeMap;

use super::*;
use crate::render::sh_streaming::install_tests::SyntheticMap;
use crate::render::sh_streaming::{ShDrainBatch, ShResidencyState};
use crate::render::sh_volume::ShVolumeSections;
use crate::render::uploads::UploadQueue;

pub(crate) struct UploadOrderSh {
    map: SyntheticMap,
    state: ShResidencyState,
    sh: ShVolumeResources,
    uniforms: wgpu::BindGroupLayout,
    weights: wgpu::Buffer,
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

    fn with_map(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dense_slots: u32,
        map: SyntheticMap,
    ) -> Self {
        let state = map.state();
        let mut ledger = crate::render::sh_residency::ShAllocationLedger::new();
        let sh = ShVolumeResources::new(
            device,
            queue,
            ShVolumeSections {
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
            uniforms,
            weights,
        };
        // Only the initial capacities are reduced. Allocation, installation,
        // growth, retirement and submission all run through production code.
        fixture.state.gpu = Some(
            StreamingGpuPools::new(
                device,
                queue,
                &fixture.map.base(),
                &fixture.map.sources(),
                crate::render::sh_streaming::floor::InitialPoolFloor {
                    dense_slots,
                    sparse_capacities: BTreeMap::new(),
                    dense_group_minimum_bytes: 0,
                    sparse_group_minimum_bytes: BTreeMap::new(),
                    effective_floor_bytes: 0,
                },
                false,
                &mut fixture.sh,
                &fixture.uniforms,
                &fixture.weights,
            )
            .expect("minimal SH pools"),
        );
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

    pub(crate) fn drain(&mut self, device: &wgpu::Device, queue: &UploadQueue, cluster: u32) {
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
                bytes.resize(bytes.len() + slots as usize * 8 * 8 * 8, 0);
            } else if block.kind == crate::render::sh_streaming::SPARSE_ROWS_BLOCK {
                let body = &ready.chunk.bytes[previous];
                let word =
                    |offset| u32::from_le_bytes(body[offset..offset + 4].try_into().unwrap());
                let rows = word(0);
                // Each source row is L2 with one stored tile. Provide the full
                // stride, not the CPU-only fixture's abbreviated f16 payload.
                let stride = delta_probe_f16_stride(4) as u32;
                let mut words = vec![rows, rows, rows * stride, 0];
                for row in 0..rows {
                    words.extend_from_slice(&[word(16 + row as usize * 16), row, 1, 1]);
                }
                for row in 0..rows {
                    words.extend_from_slice(&[0, row * stride, stride, 0]);
                }
                words.resize(words.len() + (rows * stride / 2) as usize, 0);
                for word in words {
                    bytes.extend_from_slice(&word.to_le_bytes());
                }
            } else {
                bytes.extend_from_slice(&ready.chunk.bytes[previous]);
            }
            block.body = start..bytes.len();
        }
        ready.chunk.bytes = bytes;
        let outcome = self
            .state
            .drain(
                device,
                queue,
                &mut self.sh,
                &self.uniforms,
                &self.weights,
                ShDrainBatch {
                    generation: self.state.generation,
                    content_tag: self.state.content_tag,
                    ready: vec![ready],
                    ..Default::default()
                },
            )
            .expect("production SH drain");
        assert_eq!(outcome.accepted, [cluster]);
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
            )
            .expect("real streamed compose plan");
        let mut uniform_bytes = [0u8; 96];
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
