// GPU-driven cell culling compute pipeline and indirect draw dispatch.
// See: context/lib/rendering_pipeline.md §7.1
//
// Fixed-slot design: each chunk in the chunk table has a permanent slot in
// the indirect draw buffer, sorted by material bucket so each bucket's
// commands are contiguous. Before each frame the buffer is reset from a
// stored template (all instance_count = 0). The compute shader sets
// instance_count = 1 for chunks that pass frustum culling, writing to
// the chunk's bucket-sorted slot via a remap table. The render pass
// issues multi_draw_indexed_indirect per bucket. Invisible chunks
// produce zero-instance draws that the GPU handles as no-ops.

use glam::Mat4;
use wgpu::util::DeviceExt;

use crate::geometry::CellChunkTable;

/// Size of a single DrawIndexedIndirect command in bytes.
/// Layout: index_count(4) + instance_count(4) + first_index(4) +
///         base_vertex(4) + first_instance(4) = 20 bytes.
const DRAW_INDIRECT_SIZE: u64 = 20;

// --- Compute Shader (WGSL) ---

const CULL_SHADER_SOURCE: &str = r#"
// Fixed-slot cell culling compute shader.
//
// Each visible cell ID is processed by one invocation. The shader looks up
// the cell's chunk range, frustum-tests each chunk's AABB, and chunks that
// pass get instance_count = 1 written into their bucket-sorted slot in the
// indirect buffer via the remap table.

struct FrustumPlane {
    // .xyz = normal, .w = dist
    data: vec4<f32>,
};

struct CullUniforms {
    planes: array<FrustumPlane, 6>,
};

struct CellRange {
    // x = cell_id, y = chunk_start, z = chunk_count, w = pad
    data: vec4<u32>,
};

struct DrawChunk {
    // x = cell_id (float bits), y = aabb_min.x, z = aabb_min.y, w = aabb_min.z
    header: vec4<f32>,
    // x = aabb_max.x, y = aabb_max.y, z = aabb_max.z, w = index_offset (float bits)
    bounds_and_offset: vec4<f32>,
    // x = index_count, y = material_bucket_id, z = pad, w = pad
    indices: vec4<u32>,
};

// Matches wgpu DrawIndexedIndirect layout (20 bytes, tightly packed).
// We only write instance_count; the rest are pre-filled at level load.
struct DrawIndexedIndirect {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
};

@group(0) @binding(0) var<uniform> uniforms: CullUniforms;
@group(0) @binding(1) var<storage, read> visible_cells: array<u32>;
@group(0) @binding(2) var<storage, read> cell_ranges: array<CellRange>;
@group(0) @binding(3) var<storage, read> chunks: array<DrawChunk>;
@group(0) @binding(4) var<storage, read_write> indirect_draws: array<DrawIndexedIndirect>;
@group(0) @binding(5) var<storage, read> chunk_remap: array<u32>;
// Per-chunk cull status for debug wireframe overlay.
// 0 = not processed (portal-culled cell), 1 = frustum-culled,
// 2 = visible/rendered.
@group(0) @binding(6) var<storage, read_write> cull_status: array<u32>;

fn is_aabb_outside_frustum(aabb_min: vec3<f32>, aabb_max: vec3<f32>) -> bool {
    for (var i = 0u; i < 6u; i = i + 1u) {
        let plane_data = uniforms.planes[i].data;
        let normal = plane_data.xyz;
        let dist = plane_data.w;
        let p = vec3<f32>(
            select(aabb_min.x, aabb_max.x, normal.x >= 0.0),
            select(aabb_min.y, aabb_max.y, normal.y >= 0.0),
            select(aabb_min.z, aabb_max.z, normal.z >= 0.0),
        );
        if dot(normal, p) + dist < 0.0 {
            return true;
        }
    }
    return false;
}

fn find_cell_range(cell_id: u32, num_ranges: u32) -> u32 {
    // Binary search: cell_ranges are sorted by cell_id (BTreeMap iteration order).
    var lo = 0u;
    var hi = num_ranges;
    while lo < hi {
        let mid = (lo + hi) / 2u;
        let mid_id = cell_ranges[mid].data.x;
        if mid_id < cell_id {
            lo = mid + 1u;
        } else if mid_id > cell_id {
            hi = mid;
        } else {
            return mid;
        }
    }
    return 0xFFFFFFFFu;
}

@compute @workgroup_size(64)
fn cull_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let cell_idx = gid.x;
    let num_visible = arrayLength(&visible_cells);
    if cell_idx >= num_visible {
        return;
    }

    let cell_id = visible_cells[cell_idx];
    let num_ranges = arrayLength(&cell_ranges);
    let range_idx = find_cell_range(cell_id, num_ranges);
    if range_idx == 0xFFFFFFFFu {
        return;
    }

    let range_data = cell_ranges[range_idx].data;
    let chunk_start = range_data.y;
    let chunk_count = range_data.z;

    for (var i = 0u; i < chunk_count; i = i + 1u) {
        let chunk_idx = chunk_start + i;
        let chunk = chunks[chunk_idx];

        let aabb_min = vec3<f32>(chunk.header.y, chunk.header.z, chunk.header.w);
        let aabb_max = chunk.bounds_and_offset.xyz;

        if is_aabb_outside_frustum(aabb_min, aabb_max) {
            cull_status[chunk_idx] = 1u; // frustum-culled
            continue;
        }

        // Chunk passes frustum test — enable its bucket-sorted slot.
        let sorted_slot = chunk_remap[chunk_idx];
        indirect_draws[sorted_slot].instance_count = 1u;
        cull_status[chunk_idx] = 2u; // visible/rendered
    }
}
"#;

/// GPU-side chunk data formatted for the storage buffer.
/// Three vec4 fields = 48 bytes per chunk.
#[derive(Debug, Clone, Copy)]
struct GpuDrawChunk {
    header: [f32; 4],
    bounds_and_offset: [f32; 4],
    indices: [u32; 4],
}

/// GPU-side cell range data. One vec4<u32> = 16 bytes.
#[derive(Debug, Clone, Copy)]
struct GpuCellRange {
    data: [u32; 4],
}

/// Cull uniforms: 6 frustum planes.
/// 6 * 16 = 96 bytes.
#[derive(Debug, Clone, Copy)]
struct CullUniforms {
    planes: [[f32; 4]; 6],
}

/// Per-bucket metadata computed at level load time. Tracks the offset and
/// count of each material bucket's contiguous region in the bucket-sorted
/// indirect draw buffer.
#[derive(Debug, Clone)]
struct BucketInfo {
    /// Byte offset of this bucket's first DrawIndexedIndirect command in the
    /// indirect buffer.
    indirect_byte_offset: u64,
    /// Number of DrawIndexedIndirect commands in this bucket.
    draw_count: u32,
}

/// GPU-driven compute culling pipeline with frustum culling.
/// Created at level load, dispatched each frame before the render pass.
pub struct ComputeCullPipeline {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,

    chunk_buffer: wgpu::Buffer,
    cell_range_buffer: wgpu::Buffer,
    visible_cells_buffer: wgpu::Buffer,
    visible_cells_capacity: u32,
    uniform_buffer: wgpu::Buffer,
    /// Remap table: chunk_index -> bucket-sorted slot index.
    remap_buffer: wgpu::Buffer,

    /// Indirect draw buffer: one DrawIndexedIndirect per chunk, sorted by
    /// material bucket so each bucket's commands are contiguous.
    indirect_buffer: wgpu::Buffer,
    /// Total number of chunks (= total DrawIndexedIndirect commands).
    total_chunks: u32,
    /// Pre-built indirect buffer template (all instance_count = 0) for
    /// per-frame reset. Stored at init to avoid rebuilding each frame.
    indirect_template: Vec<u8>,
    /// Per-bucket offset and count for multi_draw_indexed_indirect calls.
    bucket_info: Vec<BucketInfo>,

    #[allow(dead_code)]
    num_buckets: u32,
    has_multi_draw_indirect: bool,

    /// Per-chunk cull status buffer for debug wireframe overlay. One u32 per
    /// chunk: 0 = portal-culled, 1 = frustum-culled, 2 = visible/rendered.
    /// Initialized to 0 each frame (portal-culled default).
    cull_status_buffer: wgpu::Buffer,
}

impl ComputeCullPipeline {
    /// Create the compute culling pipeline and upload the chunk table to GPU.
    pub fn new(
        device: &wgpu::Device,
        table: &CellChunkTable,
        num_buckets: u32,
        has_multi_draw_indirect: bool,
    ) -> Self {
        let num_buckets = num_buckets.max(1);
        let total_chunks = table.chunks.len() as u32;

        // Build bucket-sorted order: sort chunks by material_bucket_id,
        // preserving original order within each bucket (stable sort).
        let mut sorted_indices: Vec<u32> = (0..total_chunks).collect();
        sorted_indices.sort_by_key(|&i| table.chunks[i as usize].material_bucket_id);

        // Build remap table: chunk_index -> sorted_slot.
        let mut remap = vec![0u32; total_chunks as usize];
        for (sorted_slot, &chunk_idx) in sorted_indices.iter().enumerate() {
            remap[chunk_idx as usize] = sorted_slot as u32;
        }

        // Build the bucket-sorted indirect buffer template.
        let indirect_template = build_indirect_buffer_sorted(table, &sorted_indices);

        // Compute per-bucket info from the sorted order.
        let bucket_info = compute_bucket_info_sorted(table, &sorted_indices, num_buckets);

        // Build GPU chunk data (in original chunk-table order for compute shader).
        let gpu_chunks: Vec<GpuDrawChunk> = table
            .chunks
            .iter()
            .map(|c| GpuDrawChunk {
                header: [
                    f32::from_bits(c.cell_id),
                    c.aabb_min[0],
                    c.aabb_min[1],
                    c.aabb_min[2],
                ],
                bounds_and_offset: [
                    c.aabb_max[0],
                    c.aabb_max[1],
                    c.aabb_max[2],
                    f32::from_bits(c.index_offset),
                ],
                indices: [c.index_count, c.material_bucket_id, 0, 0],
            })
            .collect();

        let gpu_cell_ranges: Vec<GpuCellRange> = table
            .cell_ranges
            .iter()
            .map(|cr| GpuCellRange {
                data: [cr.cell_id, cr.chunk_start, cr.chunk_count, 0],
            })
            .collect();

        let num_cell_ranges = gpu_cell_ranges.len() as u32;

        // Chunk storage buffer.
        let chunk_bytes = serialize_gpu_chunks(&gpu_chunks);
        let chunk_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Chunk Table Storage"),
            contents: if chunk_bytes.is_empty() {
                &[0u8; 48]
            } else {
                &chunk_bytes
            },
            usage: wgpu::BufferUsages::STORAGE,
        });

        // Cell range storage buffer.
        let cell_range_bytes = serialize_gpu_cell_ranges(&gpu_cell_ranges);
        let cell_range_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Cell Range Index Storage"),
            contents: if cell_range_bytes.is_empty() {
                &[0u8; 16]
            } else {
                &cell_range_bytes
            },
            usage: wgpu::BufferUsages::STORAGE,
        });

        // Remap storage buffer (chunk_index -> sorted_slot).
        let remap_bytes = serialize_u32_slice(&remap);
        let remap_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Chunk Remap Table"),
            contents: if remap_bytes.is_empty() {
                &[0u8; 4]
            } else {
                &remap_bytes
            },
            usage: wgpu::BufferUsages::STORAGE,
        });

        // Visible cells buffer.
        let initial_capacity = 256u32.max(num_cell_ranges);
        let visible_cells_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Visible Cells Buffer"),
            size: (initial_capacity as u64) * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Cull uniforms buffer: 6 planes (96) = 96 bytes.
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Cull Uniforms"),
            size: CULL_UNIFORMS_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Create the indirect draw buffer from the template.
        let indirect_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Indirect Draw Buffer"),
            contents: if indirect_template.is_empty() {
                &[0u8; 20] // One dummy command
            } else {
                &indirect_template
            },
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST,
        });

        // Per-chunk cull status buffer for debug wireframe overlay.
        let cull_status_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Cull Status Buffer"),
            size: (total_chunks.max(1) as u64) * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Create compute shader and pipeline.
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Cell Cull Compute Shader"),
            source: wgpu::ShaderSource::Wgsl(CULL_SHADER_SOURCE.into()),
        });

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Cell Cull Bind Group Layout"),
                entries: &[
                    // binding 0: uniforms
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // binding 1: visible_cells
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // binding 2: cell_ranges
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // binding 3: chunks
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // binding 4: indirect_draws (read-write)
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // binding 5: chunk_remap
                    wgpu::BindGroupLayoutEntry {
                        binding: 5,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // binding 6: cull_status (per-chunk debug output)
                    wgpu::BindGroupLayoutEntry {
                        binding: 6,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Cell Cull Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Cell Cull Compute Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("cull_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        log::info!(
            "[Renderer] Compute cull pipeline ready: {} cells, {} chunks, {} buckets, multi_draw={}",
            num_cell_ranges,
            total_chunks,
            num_buckets,
            has_multi_draw_indirect,
        );

        Self {
            pipeline,
            bind_group_layout,
            chunk_buffer,
            cell_range_buffer,
            visible_cells_buffer,
            visible_cells_capacity: initial_capacity,
            uniform_buffer,
            remap_buffer,
            indirect_buffer,
            total_chunks,
            cull_status_buffer,
            indirect_template,
            bucket_info,
            num_buckets,
            has_multi_draw_indirect,
        }
    }

    /// Upload visible cell IDs and frustum planes, clear the indirect buffer's
    /// instance_count fields, then dispatch the compute cull shader.
    ///
    /// After this call the indirect buffer is ready for `draw_indirect`.
    pub fn dispatch(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        visible_cell_ids: &[u32],
        view_proj: &Mat4,
    ) {
        let num_visible = visible_cell_ids.len() as u32;

        // Resize visible cells buffer if needed.
        if num_visible > self.visible_cells_capacity {
            let new_capacity = num_visible.next_power_of_two();
            self.visible_cells_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Visible Cells Buffer"),
                size: (new_capacity as u64) * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.visible_cells_capacity = new_capacity;
        }

        // Upload visible cell IDs. When num_visible == 0 the buffer retains
        // stale data from the previous frame, but the dispatch below uses 0
        // workgroups so no shader invocations run and the stale data is never read.
        if num_visible > 0 {
            let cell_bytes = serialize_u32_slice(visible_cell_ids);
            queue.write_buffer(&self.visible_cells_buffer, 0, &cell_bytes);
        }

        // Upload frustum planes.
        let planes = extract_frustum_planes_for_gpu(view_proj);
        let uniforms = CullUniforms {
            planes,
        };
        let uniforms_bytes = serialize_cull_uniforms(&uniforms);
        queue.write_buffer(&self.uniform_buffer, 0, &uniforms_bytes);

        // Reset all instance_count fields to 0 by re-uploading the template.
        // The template has all fields pre-filled except instance_count = 0.
        // The compute shader sets instance_count = 1 for visible chunks.
        if self.total_chunks > 0 && !self.indirect_template.is_empty() {
            queue.write_buffer(&self.indirect_buffer, 0, &self.indirect_template);
        }

        // Reset cull status to 0 (portal-culled) for all chunks. The compute
        // shader writes 1 (frustum-culled) or 2 (visible) for chunks in
        // visible cells; chunks in portal-culled cells retain the default 0.
        if self.total_chunks > 0 {
            let zeros = vec![0u8; self.total_chunks as usize * 4];
            queue.write_buffer(&self.cull_status_buffer, 0, &zeros);
        }

        // Create bind group. Recreated unconditionally every frame; caching
        // and rebuilding only on buffer resize is a deferred perf follow-up.
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Cell Cull Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.visible_cells_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.cell_range_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.chunk_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.indirect_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: self.remap_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: self.cull_status_buffer.as_entire_binding(),
                },
            ],
        });

        // Dispatch compute shader.
        let workgroup_count = num_visible.div_ceil(64);
        {
            let mut compute_pass =
                encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Cell Cull Pass"),
                    timestamp_writes: None,
                });

            compute_pass.set_pipeline(&self.pipeline);
            compute_pass.set_bind_group(0, &bind_group, &[]);

            if num_visible > 0 {
                compute_pass.dispatch_workgroups(workgroup_count, 1, 1);
            }
        }
    }

    /// Issue indirect draw calls for the render pass. One call per material
    /// bucket via multi_draw_indexed_indirect (or singular fallback).
    ///
    /// `set_texture_fn` binds the correct texture before each bucket's draws.
    pub fn draw_indirect<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        set_texture_fn: &dyn Fn(&mut wgpu::RenderPass<'a>, u32),
    ) {
        for (bucket_idx, info) in self.bucket_info.iter().enumerate() {
            if info.draw_count == 0 {
                continue;
            }

            set_texture_fn(render_pass, bucket_idx as u32);

            if self.has_multi_draw_indirect {
                render_pass.multi_draw_indexed_indirect(
                    &self.indirect_buffer,
                    info.indirect_byte_offset,
                    info.draw_count,
                );
            } else {
                // Fallback: issue individual draw_indexed_indirect calls.
                for i in 0..info.draw_count {
                    let offset =
                        info.indirect_byte_offset + (i as u64) * DRAW_INDIRECT_SIZE;
                    render_pass.draw_indexed_indirect(&self.indirect_buffer, offset);
                }
            }
        }
    }


    /// Reference to the indirect draw buffer.
    #[allow(dead_code)]
    pub fn indirect_buffer(&self) -> &wgpu::Buffer {
        &self.indirect_buffer
    }

    /// Reference to the per-chunk cull status buffer for the wireframe overlay.
    pub fn cull_status_buffer(&self) -> &wgpu::Buffer {
        &self.cull_status_buffer
    }

}

// --- Indirect buffer initialization ---

/// Build the bucket-sorted indirect draw buffer contents. Chunks are laid
/// out in the order given by `sorted_indices`, with instance_count = 0.
fn build_indirect_buffer_sorted(table: &CellChunkTable, sorted_indices: &[u32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(sorted_indices.len() * DRAW_INDIRECT_SIZE as usize);
    for &chunk_idx in sorted_indices {
        let chunk = &table.chunks[chunk_idx as usize];
        // index_count
        buf.extend_from_slice(&chunk.index_count.to_ne_bytes());
        // instance_count = 0 (cleared, compute shader sets to 1)
        buf.extend_from_slice(&0u32.to_ne_bytes());
        // first_index = index_offset
        buf.extend_from_slice(&chunk.index_offset.to_ne_bytes());
        // base_vertex = 0
        buf.extend_from_slice(&0i32.to_ne_bytes());
        // first_instance = 0
        buf.extend_from_slice(&0u32.to_ne_bytes());
    }
    buf
}

/// Compute per-bucket metadata from the bucket-sorted chunk order.
/// Each bucket's commands are contiguous in the sorted buffer.
fn compute_bucket_info_sorted(
    table: &CellChunkTable,
    sorted_indices: &[u32],
    num_buckets: u32,
) -> Vec<BucketInfo> {
    let mut info = vec![
        BucketInfo {
            indirect_byte_offset: 0,
            draw_count: 0,
        };
        num_buckets as usize
    ];

    // Count chunks per bucket.
    for chunk in &table.chunks {
        let b = chunk.material_bucket_id as usize;
        if b < info.len() {
            info[b].draw_count += 1;
        }
    }

    // Compute byte offsets. Buckets are laid out contiguously in sorted order.
    // Walk sorted_indices to find where each bucket starts.
    let mut current_bucket = u32::MAX;
    for (slot, &chunk_idx) in sorted_indices.iter().enumerate() {
        let bucket = table.chunks[chunk_idx as usize].material_bucket_id;
        if bucket != current_bucket {
            let b = bucket as usize;
            if b < info.len() {
                info[b].indirect_byte_offset = (slot as u64) * DRAW_INDIRECT_SIZE;
            }
            current_bucket = bucket;
        }
    }

    info
}

// --- GPU data serialization ---

/// Size in bytes of the CullUniforms struct.
/// 6 planes * 16 = 96 bytes.
const CULL_UNIFORMS_SIZE: usize = 96;

fn serialize_cull_uniforms(uniforms: &CullUniforms) -> Vec<u8> {
    let mut buf = Vec::with_capacity(CULL_UNIFORMS_SIZE);
    for plane in &uniforms.planes {
        for &v in plane {
            buf.extend_from_slice(&v.to_ne_bytes());
        }
    }
    buf
}

fn serialize_gpu_chunks(chunks: &[GpuDrawChunk]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(chunks.len() * 48);
    for chunk in chunks {
        for &v in &chunk.header {
            buf.extend_from_slice(&v.to_ne_bytes());
        }
        for &v in &chunk.bounds_and_offset {
            buf.extend_from_slice(&v.to_ne_bytes());
        }
        for &v in &chunk.indices {
            buf.extend_from_slice(&v.to_ne_bytes());
        }
    }
    buf
}

fn serialize_gpu_cell_ranges(ranges: &[GpuCellRange]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(ranges.len() * 16);
    for r in ranges {
        for &v in &r.data {
            buf.extend_from_slice(&v.to_ne_bytes());
        }
    }
    buf
}

fn serialize_u32_slice(slice: &[u32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(slice.len() * 4);
    for &val in slice {
        buf.extend_from_slice(&val.to_ne_bytes());
    }
    buf
}

fn extract_frustum_planes_for_gpu(view_proj: &Mat4) -> [[f32; 4]; 6] {
    let row = |n: usize| -> glam::Vec4 {
        glam::Vec4::new(
            view_proj.col(0)[n],
            view_proj.col(1)[n],
            view_proj.col(2)[n],
            view_proj.col(3)[n],
        )
    };

    let r0 = row(0);
    let r1 = row(1);
    let r2 = row(2);
    let r3 = row(3);

    let raw_planes = [
        r3 + r0, // Left
        r3 - r0, // Right
        r3 + r1, // Bottom
        r3 - r1, // Top
        r3 + r2, // Near
        r3 - r2, // Far
    ];

    let mut gpu_planes = [[0.0f32; 4]; 6];
    for (i, raw) in raw_planes.iter().enumerate() {
        let normal = glam::Vec3::new(raw.x, raw.y, raw.z);
        let length = normal.length();
        if length > 0.0 {
            let inv_len = 1.0 / length;
            let n = normal * inv_len;
            gpu_planes[i] = [n.x, n.y, n.z, raw.w * inv_len];
        }
    }
    gpu_planes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cull_uniforms_serialized_size_is_96_bytes() {
        let uniforms = CullUniforms {
            planes: [[0.0; 4]; 6],
        };
        let bytes = serialize_cull_uniforms(&uniforms);
        assert_eq!(bytes.len(), CULL_UNIFORMS_SIZE);
    }

    #[test]
    fn gpu_draw_chunk_serialized_size_is_48_bytes() {
        let chunk = GpuDrawChunk {
            header: [0.0; 4],
            bounds_and_offset: [0.0; 4],
            indices: [0; 4],
        };
        let bytes = serialize_gpu_chunks(&[chunk]);
        assert_eq!(bytes.len(), 48);
    }

    #[test]
    fn gpu_cell_range_serialized_size_is_16_bytes() {
        let range = GpuCellRange { data: [0; 4] };
        let bytes = serialize_gpu_cell_ranges(&[range]);
        assert_eq!(bytes.len(), 16);
    }

    #[test]
    fn frustum_plane_extraction_produces_normalized_planes() {
        let view = Mat4::look_at_rh(
            glam::Vec3::ZERO,
            glam::Vec3::NEG_Z,
            glam::Vec3::Y,
        );
        let proj = Mat4::perspective_rh(
            std::f32::consts::FRAC_PI_2,
            16.0 / 9.0,
            0.1,
            4096.0,
        );
        let vp = proj * view;
        let planes = extract_frustum_planes_for_gpu(&vp);

        for (i, plane) in planes.iter().enumerate() {
            let len = (plane[0] * plane[0]
                + plane[1] * plane[1]
                + plane[2] * plane[2])
                .sqrt();
            assert!(
                (len - 1.0).abs() < 1e-5,
                "GPU frustum plane {i} not normalized: length = {len}"
            );
        }
    }

    #[test]
    fn draw_indirect_size_is_20_bytes() {
        assert_eq!(DRAW_INDIRECT_SIZE, 20);
    }

    #[test]
    fn indirect_buffer_sorted_produces_correct_layout() {
        use crate::geometry::{CellChunkTable, CellRange, DrawChunk};

        let table = CellChunkTable {
            cell_ranges: vec![CellRange {
                cell_id: 0,
                chunk_start: 0,
                chunk_count: 1,
            }],
            chunks: vec![DrawChunk {
                cell_id: 0,
                aabb_min: [0.0; 3],
                aabb_max: [1.0; 3],
                index_offset: 42,
                index_count: 6,
                material_bucket_id: 0,
            }],
        };
        let sorted_indices: Vec<u32> = vec![0];
        let data = build_indirect_buffer_sorted(&table, &sorted_indices);

        // 20 bytes per chunk
        assert_eq!(data.len(), 20);

        // index_count = 6
        let index_count = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]);
        assert_eq!(index_count, 6);

        // instance_count = 0 (cleared)
        let instance_count = u32::from_ne_bytes([data[4], data[5], data[6], data[7]]);
        assert_eq!(instance_count, 0);

        // first_index = 42
        let first_index = u32::from_ne_bytes([data[8], data[9], data[10], data[11]]);
        assert_eq!(first_index, 42);
    }

    #[test]
    fn chunk_cell_id_roundtrips_through_float_bits() {
        for cell_id in [0u32, 1, 42, 255, 1000, u32::MAX] {
            let bits = f32::from_bits(cell_id);
            let recovered = bits.to_bits();
            assert_eq!(cell_id, recovered);
        }
    }

    #[test]
    fn bucket_sorting_groups_chunks_contiguously() {
        use crate::geometry::{CellChunkTable, CellRange, DrawChunk};

        // Two cells, each with chunks for buckets 0 and 1 (interleaved).
        let table = CellChunkTable {
            cell_ranges: vec![
                CellRange {
                    cell_id: 0,
                    chunk_start: 0,
                    chunk_count: 2,
                },
                CellRange {
                    cell_id: 1,
                    chunk_start: 2,
                    chunk_count: 2,
                },
            ],
            chunks: vec![
                DrawChunk {
                    cell_id: 0,
                    aabb_min: [0.0; 3],
                    aabb_max: [1.0; 3],
                    index_offset: 0,
                    index_count: 3,
                    material_bucket_id: 0,
                },
                DrawChunk {
                    cell_id: 0,
                    aabb_min: [0.0; 3],
                    aabb_max: [1.0; 3],
                    index_offset: 3,
                    index_count: 6,
                    material_bucket_id: 1,
                },
                DrawChunk {
                    cell_id: 1,
                    aabb_min: [2.0; 3],
                    aabb_max: [3.0; 3],
                    index_offset: 9,
                    index_count: 3,
                    material_bucket_id: 0,
                },
                DrawChunk {
                    cell_id: 1,
                    aabb_min: [2.0; 3],
                    aabb_max: [3.0; 3],
                    index_offset: 12,
                    index_count: 6,
                    material_bucket_id: 1,
                },
            ],
        };

        let total_chunks = table.chunks.len() as u32;
        let mut sorted_indices: Vec<u32> = (0..total_chunks).collect();
        sorted_indices.sort_by_key(|&i| table.chunks[i as usize].material_bucket_id);

        // Bucket 0 chunks should come first, then bucket 1.
        assert_eq!(
            table.chunks[sorted_indices[0] as usize].material_bucket_id,
            0
        );
        assert_eq!(
            table.chunks[sorted_indices[1] as usize].material_bucket_id,
            0
        );
        assert_eq!(
            table.chunks[sorted_indices[2] as usize].material_bucket_id,
            1
        );
        assert_eq!(
            table.chunks[sorted_indices[3] as usize].material_bucket_id,
            1
        );

        // Verify remap table.
        let mut remap = vec![0u32; total_chunks as usize];
        for (sorted_slot, &chunk_idx) in sorted_indices.iter().enumerate() {
            remap[chunk_idx as usize] = sorted_slot as u32;
        }

        // Chunk 0 (cell 0, bucket 0) should map to slot 0 or 1.
        assert!(remap[0] < 2, "bucket-0 chunk should be in first 2 slots");
        // Chunk 1 (cell 0, bucket 1) should map to slot 2 or 3.
        assert!(
            remap[1] >= 2,
            "bucket-1 chunk should be in last 2 slots"
        );

        // Verify bucket info.
        let bucket_info = compute_bucket_info_sorted(&table, &sorted_indices, 2);
        assert_eq!(bucket_info.len(), 2);
        assert_eq!(bucket_info[0].draw_count, 2);
        assert_eq!(bucket_info[1].draw_count, 2);
        assert_eq!(bucket_info[0].indirect_byte_offset, 0);
        assert_eq!(
            bucket_info[1].indirect_byte_offset,
            2 * DRAW_INDIRECT_SIZE
        );
    }

    #[test]
    fn empty_chunk_table_produces_empty_indirect_buffer() {
        use crate::geometry::{CellChunkTable, CellRange, DrawChunk};
        let _ = (CellRange { cell_id: 0, chunk_start: 0, chunk_count: 0 }, DrawChunk {
            cell_id: 0,
            aabb_min: [0.0; 3],
            aabb_max: [0.0; 3],
            index_offset: 0,
            index_count: 0,
            material_bucket_id: 0,
        });

        let table = CellChunkTable {
            cell_ranges: vec![],
            chunks: vec![],
        };
        let sorted_indices: Vec<u32> = vec![];
        let data = build_indirect_buffer_sorted(&table, &sorted_indices);
        assert!(data.is_empty(), "empty chunk table should produce empty indirect buffer");

        let bucket_info = compute_bucket_info_sorted(&table, &sorted_indices, 1);
        assert_eq!(bucket_info.len(), 1);
        assert_eq!(bucket_info[0].draw_count, 0);
    }

    #[test]
    fn indirect_buffer_all_instance_counts_start_at_zero() {
        use crate::geometry::{CellChunkTable, CellRange, DrawChunk};

        // Build a table with multiple chunks and verify every instance_count
        // in the template buffer starts at zero.
        let table = CellChunkTable {
            cell_ranges: vec![
                CellRange { cell_id: 0, chunk_start: 0, chunk_count: 3 },
            ],
            chunks: vec![
                DrawChunk {
                    cell_id: 0,
                    aabb_min: [0.0; 3],
                    aabb_max: [1.0; 3],
                    index_offset: 0,
                    index_count: 3,
                    material_bucket_id: 0,
                },
                DrawChunk {
                    cell_id: 0,
                    aabb_min: [0.0; 3],
                    aabb_max: [1.0; 3],
                    index_offset: 3,
                    index_count: 6,
                    material_bucket_id: 1,
                },
                DrawChunk {
                    cell_id: 0,
                    aabb_min: [0.0; 3],
                    aabb_max: [1.0; 3],
                    index_offset: 9,
                    index_count: 3,
                    material_bucket_id: 2,
                },
            ],
        };
        let mut sorted_indices: Vec<u32> = (0..3).collect();
        sorted_indices.sort_by_key(|&i| table.chunks[i as usize].material_bucket_id);

        let data = build_indirect_buffer_sorted(&table, &sorted_indices);

        // 3 chunks * 20 bytes each = 60 bytes
        assert_eq!(data.len(), 60);

        // Verify every instance_count field is zero.
        for i in 0..3 {
            let offset = i * DRAW_INDIRECT_SIZE as usize;
            let instance_count = u32::from_ne_bytes([
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]);
            assert_eq!(
                instance_count, 0,
                "chunk {i} instance_count should be 0 in template"
            );
        }
    }

    #[test]
    fn remap_table_is_inverse_of_sort() {
        use crate::geometry::{CellChunkTable, CellRange, DrawChunk};

        // Chunks with mixed bucket order to exercise the remap.
        let table = CellChunkTable {
            cell_ranges: vec![
                CellRange { cell_id: 0, chunk_start: 0, chunk_count: 1 },
                CellRange { cell_id: 1, chunk_start: 1, chunk_count: 1 },
                CellRange { cell_id: 2, chunk_start: 2, chunk_count: 1 },
            ],
            chunks: vec![
                DrawChunk { cell_id: 0, aabb_min: [0.0; 3], aabb_max: [1.0; 3], index_offset: 0, index_count: 3, material_bucket_id: 2 },
                DrawChunk { cell_id: 1, aabb_min: [0.0; 3], aabb_max: [1.0; 3], index_offset: 3, index_count: 3, material_bucket_id: 0 },
                DrawChunk { cell_id: 2, aabb_min: [0.0; 3], aabb_max: [1.0; 3], index_offset: 6, index_count: 3, material_bucket_id: 1 },
            ],
        };

        let total = table.chunks.len() as u32;
        let mut sorted_indices: Vec<u32> = (0..total).collect();
        sorted_indices.sort_by_key(|&i| table.chunks[i as usize].material_bucket_id);

        // Build remap.
        let mut remap = vec![0u32; total as usize];
        for (sorted_slot, &chunk_idx) in sorted_indices.iter().enumerate() {
            remap[chunk_idx as usize] = sorted_slot as u32;
        }

        // Remap is the inverse: remap[chunk_idx] = sorted_slot,
        // sorted_indices[sorted_slot] = chunk_idx.
        for (chunk_idx, &sorted_slot) in remap.iter().enumerate() {
            assert_eq!(
                sorted_indices[sorted_slot as usize] as usize, chunk_idx,
                "remap should be inverse of sorted_indices"
            );
        }
    }

    #[test]
    fn bucket_info_covers_all_chunks() {
        use crate::geometry::{CellChunkTable, CellRange, DrawChunk};

        // 5 chunks across 3 buckets.
        let table = CellChunkTable {
            cell_ranges: vec![
                CellRange { cell_id: 0, chunk_start: 0, chunk_count: 3 },
                CellRange { cell_id: 1, chunk_start: 3, chunk_count: 2 },
            ],
            chunks: vec![
                DrawChunk { cell_id: 0, aabb_min: [0.0; 3], aabb_max: [1.0; 3], index_offset: 0, index_count: 3, material_bucket_id: 0 },
                DrawChunk { cell_id: 0, aabb_min: [0.0; 3], aabb_max: [1.0; 3], index_offset: 3, index_count: 3, material_bucket_id: 1 },
                DrawChunk { cell_id: 0, aabb_min: [0.0; 3], aabb_max: [1.0; 3], index_offset: 6, index_count: 3, material_bucket_id: 2 },
                DrawChunk { cell_id: 1, aabb_min: [0.0; 3], aabb_max: [1.0; 3], index_offset: 9, index_count: 3, material_bucket_id: 0 },
                DrawChunk { cell_id: 1, aabb_min: [0.0; 3], aabb_max: [1.0; 3], index_offset: 12, index_count: 3, material_bucket_id: 2 },
            ],
        };

        let total = table.chunks.len() as u32;
        let mut sorted_indices: Vec<u32> = (0..total).collect();
        sorted_indices.sort_by_key(|&i| table.chunks[i as usize].material_bucket_id);

        let bucket_info = compute_bucket_info_sorted(&table, &sorted_indices, 3);

        // Total draw_count across all buckets should equal total chunks.
        let total_draw_count: u32 = bucket_info.iter().map(|b| b.draw_count).sum();
        assert_eq!(total_draw_count, 5, "all chunks accounted for in bucket info");

        // Bucket 0: 2 chunks (cell 0 and cell 1)
        assert_eq!(bucket_info[0].draw_count, 2);
        // Bucket 1: 1 chunk (cell 0)
        assert_eq!(bucket_info[1].draw_count, 1);
        // Bucket 2: 2 chunks (cell 0 and cell 1)
        assert_eq!(bucket_info[2].draw_count, 2);

        // Verify offsets are contiguous.
        let mut expected_offset = 0u64;
        for info in &bucket_info {
            assert_eq!(info.indirect_byte_offset, expected_offset);
            expected_offset += info.draw_count as u64 * DRAW_INDIRECT_SIZE;
        }
    }

    #[test]
    fn frustum_planes_match_between_cpu_and_gpu_extraction() {
        // Verify that the GPU frustum plane extraction produces the same
        // planes as the CPU visibility module's extraction.
        use crate::visibility::extract_frustum_planes;

        let view = Mat4::look_at_rh(
            glam::Vec3::new(5.0, 3.0, -10.0),
            glam::Vec3::new(0.0, 0.0, -50.0),
            glam::Vec3::Y,
        );
        let proj = Mat4::perspective_rh(
            std::f32::consts::FRAC_PI_2,
            16.0 / 9.0,
            0.1,
            4096.0,
        );
        let vp = proj * view;

        let cpu_frustum = extract_frustum_planes(vp);
        let gpu_planes = extract_frustum_planes_for_gpu(&vp);

        for (i, (cpu_plane, gpu_plane)) in
            cpu_frustum.planes.iter().zip(gpu_planes.iter()).enumerate()
        {
            let nx_diff = (cpu_plane.normal.x - gpu_plane[0]).abs();
            let ny_diff = (cpu_plane.normal.y - gpu_plane[1]).abs();
            let nz_diff = (cpu_plane.normal.z - gpu_plane[2]).abs();
            let d_diff = (cpu_plane.dist - gpu_plane[3]).abs();
            assert!(
                nx_diff < 1e-5 && ny_diff < 1e-5 && nz_diff < 1e-5 && d_diff < 1e-5,
                "plane {i} differs: CPU ({:?}, {}) vs GPU ({:?})",
                cpu_plane.normal,
                cpu_plane.dist,
                gpu_plane,
            );
        }
    }
}
