// Bounded asynchronous irradiance readback for streamed probe markers.
// See: context/lib/rendering_pipeline.md §Cluster SH residency

use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use postretro_level_format::sh_reconstruct::{
    node_corner_coord, node_l1_corner_weight, node_local_coord,
};
use postretro_level_loader::ShStreamBaseMetadata;
use postretro_render_cpu::sh_volume::STREAMED_SH_PHYSICAL_TILE_STRIDE;

use super::ShResidencyState;
use crate::render::debug_lines::MAX_DEBUG_SEGMENTS;
use crate::render::sh_indirection::decode_probe_indirection_word;
use crate::render::uploads::UploadQueue;

// A marker spends three segments; reduction never exceeds the drawing budget.
const MAX_PROBES: usize = MAX_DEBUG_SEGMENTS / 3;
const READBACK_INTERVAL: Duration = Duration::from_millis(100);
const SHADER: &str = include_str!("../../shaders/sh_probe_readback.wgsl");

#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
struct Request {
    slots: [u32; 8],
    weights: [f32; 8],
}

#[derive(Clone, Copy)]
struct Identity {
    dense: usize,
    word: u32,
}

struct ProbeColor {
    identity: Identity,
    rgb: [f32; 3],
}

struct GpuReadback {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    requests: wgpu::Buffer,
    colors: wgpu::Buffer,
    params: wgpu::Buffer,
    readback: wgpu::Buffer,
}

impl GpuReadback {
    fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Streamed SH marker reduction layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                buffer_entry(1, wgpu::BufferBindingType::Storage { read_only: true }),
                buffer_entry(2, wgpu::BufferBindingType::Storage { read_only: false }),
                buffer_entry(3, wgpu::BufferBindingType::Uniform),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Streamed SH marker reduction pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Streamed SH marker reduction shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Streamed SH marker reduction"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            pipeline,
            layout,
            requests: buffer(
                "Streamed SH marker requests",
                (MAX_PROBES * size_of::<Request>()) as u64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            colors: buffer(
                "Streamed SH marker colors",
                (MAX_PROBES * 16) as u64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            ),
            params: buffer(
                "Streamed SH marker params",
                32,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            readback: buffer(
                "Streamed SH marker readback",
                (MAX_PROBES * 16) as u64,
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            ),
        }
    }
}

fn buffer_entry(binding: u32, ty: wgpu::BufferBindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

#[derive(Default)]
pub(super) struct StreamedProbeReadback {
    gpu: Option<GpuReadback>,
    wanted: bool,
    camera: Vec3,
    radius: f32,
    epoch: u64,
    copied_epoch: u64,
    copied: bool,
    mapping: bool,
    // 0 = pending, 1 = mapped, 2 = failed. Callbacks retain no GPU handles.
    map_status: Arc<AtomicU8>,
    last_copy: Option<Instant>,
    requests: Vec<Request>,
    identities: Vec<Identity>,
    colors: Vec<ProbeColor>,
}

impl StreamedProbeReadback {
    pub(super) fn set_overlay(&mut self, wanted: bool, camera: Vec3, radius: f32) {
        if self.wanted != wanted {
            self.invalidate();
        }
        self.wanted = wanted;
        self.camera = camera;
        self.radius = radius;
    }

    pub(super) fn wanted(&self) -> bool {
        self.wanted
    }

    // Slot reuse and even an ABA reassignment invalidate both cached and pending colors.
    pub(super) fn invalidate(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.colors.clear();
    }

    pub(super) fn color(&self, dense: usize, sampled_word: u32) -> Option<[f32; 3]> {
        let index = self
            .colors
            .binary_search_by_key(&dense, |color| color.identity.dense)
            .ok()?;
        let color = &self.colors[index];
        (sampled_word != 0 && color.identity.word == sampled_word).then_some(color.rgb)
    }

    fn wants_copy(&self, now: Instant) -> bool {
        self.wanted
            && !self.copied
            && !self.mapping
            && self
                .last_copy
                .is_none_or(|last| now.duration_since(last) >= READBACK_INTERVAL)
    }

    fn post_submit(&mut self, device: &wgpu::Device) {
        if !self.mapping && !self.copied {
            return;
        }
        let _ = device.poll(wgpu::PollType::Poll);
        match self.map_status.swap(0, Ordering::AcqRel) {
            1 => {
                let gpu = self
                    .gpu
                    .as_ref()
                    .expect("mapping follows GPU readback creation");
                let view = gpu
                    .readback
                    .slice(..(self.identities.len() * 16) as u64)
                    .get_mapped_range()
                    .expect("successful marker readback mapping");
                self.colors.clear();
                if self.wanted && self.copied_epoch == self.epoch {
                    self.colors.extend(
                        self.identities
                            .iter()
                            .copied()
                            .zip(view.as_chunks::<16>().0)
                            .map(|(identity, bytes)| ProbeColor {
                                identity,
                                rgb: std::array::from_fn(|channel| {
                                    f32::from_le_bytes(
                                        bytes[channel * 4..channel * 4 + 4]
                                            .try_into()
                                            .expect("four-byte color channel"),
                                    )
                                }),
                            }),
                    );
                }
                drop(view);
                gpu.readback.unmap();
                self.mapping = false;
            }
            2 => {
                self.mapping = false;
            }
            _ => {}
        }
        if self.copied && !self.mapping {
            self.copied = false;
            self.mapping = true;
            let status = Arc::clone(&self.map_status);
            self.gpu
                .as_ref()
                .expect("copy follows GPU readback creation")
                .readback
                .slice(..(self.identities.len() * 16) as u64)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let value = match result {
                        Ok(()) => 1,
                        Err(error) => {
                            log::warn!("[Renderer] streamed SH marker map failed: {error:?}");
                            2
                        }
                    };
                    status.store(value, Ordering::Release);
                });
        }
    }
}

impl ShResidencyState {
    pub(in crate::render) fn set_probe_overlay(&mut self, wanted: bool, camera: Vec3, radius: f32) {
        self.probe_readback.set_overlay(wanted, camera, radius);
    }

    pub(in crate::render) fn probe_overlay_wanted(&self) -> bool {
        self.probe_readback.wanted()
    }

    pub(in crate::render) fn probe_irradiance(&self, dense: usize) -> Option<[f32; 3]> {
        self.probe_readback
            .color(dense, *self.sampled_words.get(dense)?)
    }

    pub(in crate::render) fn encode_probe_readback(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let now = Instant::now();
        let readback = &mut self.probe_readback;
        if !readback.wants_copy(now) {
            return;
        }
        let Some(pool) = self.gpu.as_ref() else {
            return;
        };
        collect_requests(
            &self.base_metadata,
            &self.sampled_words,
            readback.camera,
            readback.radius,
            &mut readback.requests,
            &mut readback.identities,
        );
        if readback.requests.is_empty() {
            return;
        }
        let gpu = readback.gpu.get_or_insert_with(|| GpuReadback::new(device));
        let params = [
            readback.requests.len() as u32,
            pool.shape.tiles_per_row,
            pool.shape.tiles_per_layer,
            STREAMED_SH_PHYSICAL_TILE_STRIDE,
            self.base_metadata.tile_dimension,
            self.base_metadata.tile_border,
            0,
            0,
        ];
        queue.write_buffer(&gpu.requests, 0, bytemuck::cast_slice(&readback.requests));
        queue.write_buffer(&gpu.params, 0, bytemuck::cast_slice(&params));
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Streamed SH marker reduction bindings"),
            layout: &gpu.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&pool.total_sampled_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: gpu.requests.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: gpu.colors.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: gpu.params.as_entire_binding(),
                },
            ],
        });
        {
            // A sampled read after compose in this encoder supplies the same
            // storage→sampled dependency as the forward lighting consumer.
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Streamed SH marker reduction"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups((readback.requests.len() as u32).div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(
            &gpu.colors,
            0,
            &gpu.readback,
            0,
            (readback.requests.len() * 16) as u64,
        );
        readback.copied_epoch = readback.epoch;
        readback.copied = true;
        readback.last_copy = Some(now);
    }

    pub(in crate::render) fn post_submit_probe_readback(&mut self, device: &wgpu::Device) {
        self.probe_readback.post_submit(device);
    }
}

fn collect_requests(
    base: &ShStreamBaseMetadata,
    words: &[u32],
    camera: Vec3,
    radius: f32,
    requests: &mut Vec<Request>,
    identities: &mut Vec<Identity>,
) {
    requests.clear();
    identities.clear();
    let [nx, ny, _] = base.grid_dimensions;
    let origin = Vec3::from(base.grid_origin);
    let cell = Vec3::from(base.cell_size);
    let mut markers = 0;
    for (dense, &word) in words.iter().enumerate() {
        let coord = [
            dense as u32 % nx,
            (dense as u32 / nx) % ny,
            dense as u32 / (nx * ny),
        ];
        let position = origin + Vec3::from(coord.map(|v| v as f32)) * cell;
        if position.distance_squared(camera) > radius * radius {
            continue;
        }
        if markers == MAX_PROBES {
            break;
        }
        markers += 1;
        if let Some(request) = probe_request(dense, base.grid_dimensions, words) {
            requests.push(request);
            identities.push(Identity { dense, word });
        }
    }
}

fn probe_request(dense: usize, dims: [u32; 3], words: &[u32]) -> Option<Request> {
    let word = decode_probe_indirection_word(*words.get(dense)?);
    if !word.valid {
        return None;
    }
    let mut request = Request::default();
    match word.level {
        0 | 2 => {
            request.slots[0] = word.slot;
            request.weights[0] = 1.0;
        }
        1 => {
            let [nx, ny, _] = dims;
            let coord = [
                dense as u32 % nx,
                (dense as u32 / nx) % ny,
                dense as u32 / (nx * ny),
            ];
            let local = node_local_coord(coord, word.scale as u8)?;
            let origin = std::array::from_fn(|axis| coord[axis] - local[axis]);
            let mut sum = 0.0;
            for corner in 0..8u8 {
                let global = node_corner_coord(origin, word.scale as u8, corner)?;
                if global.iter().zip(dims).any(|(&v, dim)| v >= dim) {
                    continue;
                }
                let index = (global[0] + global[1] * nx + global[2] * nx * ny) as usize;
                let other = decode_probe_indirection_word(*words.get(index)?);
                if !other.valid
                    || other.level != 1
                    || other.scale != word.scale
                    || other.slot != word.slot
                {
                    continue;
                }
                let weight = node_l1_corner_weight(local, corner, word.scale as u8)?;
                request.slots[corner as usize] = word.slot + u32::from(corner);
                request.weights[corner as usize] = weight;
                sum += weight;
            }
            if sum <= 0.0 {
                return None;
            }
            for weight in &mut request.weights {
                *weight /= sum;
            }
        }
        _ => return None,
    }
    Some(request)
}

#[cfg(test)]
mod tests;
