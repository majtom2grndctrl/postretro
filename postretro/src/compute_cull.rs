// GPU-driven BVH traversal compute pipeline and indirect draw dispatch.
// See: context/lib/rendering_pipeline.md §7.1
// See: context/plans/in-progress/bvh-foundation/2-runtime-bvh.md
//
// Fixed-slot design: each BVH leaf owns a permanent slot in the indirect
// draw buffer, indexed by its position in the flat leaf array. Leaves are
// sorted by `material_bucket_id` so each bucket's commands are contiguous.
// At load time we derive per-bucket `(first_leaf, leaf_count)` ranges once
// and issue one `multi_draw_indexed_indirect` per bucket.
//
// Each frame, the compute shader walks the BVH in DFS order using the
// `skip_index` pointer to jump over rejected subtrees — no stack, no depth
// cap, no abort path. For each leaf it hits:
//   - The leaf's AABB is frustum-tested (parent AABB may be larger).
//   - The leaf's `cell_id` is tested against the per-frame visible-cell
//     bitmask produced by the portal DFS on the CPU side.
//   - If both checks pass, a full `DrawIndexedIndirect` is written to the
//     leaf's fixed slot; otherwise `index_count` is zeroed so the slot
//     becomes a no-op GPU draw.
//
// This replaces the Milestone 3.5 per-cell chunk compute cull. Portal DFS
// still runs on the CPU; it just feeds a bitmask instead of a flat cell id
// list. See `determine_visible_cells` in `visibility.rs`.

use glam::Mat4;
use wgpu::util::DeviceExt;

use crate::geometry::{BVH_NODE_FLAG_LEAF, BucketRange, BvhTree};

// All GPU uploads below use little-endian byte order because the WGSL storage
// buffers, PRL on-disk format, and every wgpu backend target (Vulkan, Metal,
// DX12 on x86_64 / aarch64 / wasm32) are little-endian. Enforce at compile
// time so a hypothetical big-endian build fails loudly instead of silently
// scrambling BVH data.
const _: () = assert!(
    cfg!(target_endian = "little"),
    "postretro GPU upload path assumes little-endian; add a byte-swap layer before porting"
);

/// Size of a single DrawIndexedIndirect command in bytes.
/// Layout: index_count(4) + instance_count(4) + first_index(4) +
///         base_vertex(4) + first_instance(4) = 20 bytes.
const DRAW_INDIRECT_SIZE: u64 = 20;

/// Visible-cell bitmask: fixed 128-word (512-byte) storage buffer covering
/// up to 4096 cell IDs (bit test `bitmask[cell_id >> 5] & (1 << (cell_id & 31))`).
/// The fixed size matches the contract documented in the BVH foundation plan
/// and removes any resize path from the hot frame loop.
const VISIBLE_CELLS_WORDS: usize = 128;
const VISIBLE_CELLS_BYTES: u64 = (VISIBLE_CELLS_WORDS * 4) as u64;
const MAX_VISIBLE_CELLS: u32 = (VISIBLE_CELLS_WORDS as u32) * 32;

// --- Compute Shader (WGSL) ---
//
// Traversal strategy: skip-index (flat DFS), not stack-based.
//
// Sub-plan 1 writes nodes in depth-first order with a `skip_index` per node
// pointing to the next sibling subtree. On AABB reject we jump to
// `skip_index`; on AABB accept we advance to `i + 1` (left child). This
// eliminates the explicit stack entirely, has no depth cap, and is the
// standard approach for software GPU BVH traversal.
//
// Portal integration: per-leaf visible-cell bitmask check. BVH leaves carry
// a `cell_id: u32`. The portal DFS runs first each frame on the CPU and
// builds a 128-word bitmask uploaded to `visible_cells`. Each surviving leaf
// is tested against this bitmask; mismatches zero their indirect slot.
//
// Alternative rejected: multi-frustum traversal — one BVH pass per
// portal-narrowed frustum. Tighter cull isn't worth N traversals per frame
// and the added shader complexity. One traversal, simple shader code,
// O(1) per-leaf cost. See the BVH foundation sub-plan 2 notes.
//
// WGSL struct layouts must match the on-disk stride byte-for-byte (see
// `1-compile-bvh.md` for the exact layouts). The AABB corners are declared as
// six scalar `f32` fields instead of `vec3<f32>` on purpose: in WGSL a
// `vec3<f32>` has *size* 12 but *alignment* 16, which forces any containing
// struct up to 16-byte alignment and rounds the stride from 40 to 48. Splitting
// the vectors into scalars gives the struct 4-byte alignment and a natural
// 40-byte stride that matches the on-disk layout exactly. We reconstruct the
// corners as `vec3<f32>` once per node at traversal time — the constructor is
// free on the GPU. Verified against naga 29.0.1 in `wgsl_struct_strides_are_40_bytes`.
const CULL_SHADER_SOURCE: &str = r#"
// BVH traversal compute shader — flat DFS with skip-index.
//
// One invocation walks the entire tree per frame (`@workgroup_size(1,1,1)`);
// parallelism over subtrees is a deferred optimization that the current
// scene scale does not need.

struct FrustumPlane {
    // .xyz = normal, .w = dist
    data: vec4<f32>,
};

struct CullUniforms {
    planes: array<FrustumPlane, 6>,
};

// AABB corners are stored as six scalar f32s, not vec3<f32>: WGSL's
// AlignOf(vec3<f32>) = 16 would force struct stride to 48. Scalar-only structs
// have 4-byte alignment, giving a natural 40-byte stride that matches the
// on-disk layout. `wgsl_struct_strides_are_40_bytes` enforces this with naga.
struct BvhNode {
    min_x: f32,                     // offset  0
    min_y: f32,                     // offset  4
    min_z: f32,                     // offset  8
    skip_index: u32,                // offset 12
    max_x: f32,                     // offset 16
    max_y: f32,                     // offset 20
    max_z: f32,                     // offset 24
    left_child_or_leaf_index: u32,  // offset 28
    flags: u32,                     // offset 32 (bit 0 = is_leaf)
    _pad: u32,                      // offset 36
};

struct BvhLeaf {
    min_x: f32,                // offset  0
    min_y: f32,                // offset  4
    min_z: f32,                // offset  8
    material_bucket_id: u32,   // offset 12
    max_x: f32,                // offset 16
    max_y: f32,                // offset 20
    max_z: f32,                // offset 24
    index_offset: u32,         // offset 28
    index_count: u32,          // offset 32
    cell_id: u32,              // offset 36
};

struct DrawIndexedIndirect {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
};

@group(0) @binding(0) var<uniform> uniforms: CullUniforms;
@group(0) @binding(1) var<storage, read> nodes: array<BvhNode>;
@group(0) @binding(2) var<storage, read> leaves: array<BvhLeaf>;
@group(0) @binding(3) var<storage, read> visible_cells: array<u32>;
@group(0) @binding(4) var<storage, read_write> indirect_draws: array<DrawIndexedIndirect>;
// Per-leaf cull status for the debug wireframe overlay.
// 0 = portal-culled (cell not in visible set),
// 1 = frustum-culled,
// 2 = visible/rendered.
@group(0) @binding(5) var<storage, read_write> cull_status: array<u32>;

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

fn cell_is_visible(cell_id: u32) -> bool {
    // Fixed 128-word / 4096-cell bitmask; sub-plan 1 enforces cell_id < 4096.
    let word_idx = cell_id >> 5u;
    if word_idx >= 128u {
        return false;
    }
    let bit = 1u << (cell_id & 31u);
    return (visible_cells[word_idx] & bit) != 0u;
}

@compute @workgroup_size(1, 1, 1)
fn cull_main() {
    let node_count = arrayLength(&nodes);
    if node_count == 0u {
        return;
    }

    var i = 0u;
    loop {
        if i >= node_count {
            break;
        }

        let node = nodes[i];
        let node_min = vec3<f32>(node.min_x, node.min_y, node.min_z);
        let node_max = vec3<f32>(node.max_x, node.max_y, node.max_z);

        // Reject: jump over the entire subtree via skip_index.
        if is_aabb_outside_frustum(node_min, node_max) {
            if (node.flags & 1u) != 0u {
                // Leaf that didn't pass the frustum test.
                let leaf_idx = node.left_child_or_leaf_index;
                indirect_draws[leaf_idx].index_count = 0u;
                cull_status[leaf_idx] = 1u;
            }
            i = node.skip_index;
            continue;
        }

        if (node.flags & 1u) != 0u {
            // Leaf: do the per-leaf tests and write its indirect slot.
            let leaf_idx = node.left_child_or_leaf_index;
            let leaf = leaves[leaf_idx];
            let leaf_min = vec3<f32>(leaf.min_x, leaf.min_y, leaf.min_z);
            let leaf_max = vec3<f32>(leaf.max_x, leaf.max_y, leaf.max_z);

            if is_aabb_outside_frustum(leaf_min, leaf_max) {
                indirect_draws[leaf_idx].index_count = 0u;
                cull_status[leaf_idx] = 1u;
            } else if !cell_is_visible(leaf.cell_id) {
                indirect_draws[leaf_idx].index_count = 0u;
                cull_status[leaf_idx] = 0u;
            } else {
                indirect_draws[leaf_idx].index_count = leaf.index_count;
                indirect_draws[leaf_idx].instance_count = 1u;
                indirect_draws[leaf_idx].first_index = leaf.index_offset;
                indirect_draws[leaf_idx].base_vertex = 0;
                indirect_draws[leaf_idx].first_instance = 0u;
                cull_status[leaf_idx] = 2u;
            }
            i = node.skip_index;
            continue;
        }

        // Internal node survived — descend to left child.
        i = i + 1u;
    }
}
"#;

/// Cull uniforms: 6 frustum planes.
/// 6 * 16 = 96 bytes.
#[derive(Debug, Clone, Copy)]
struct CullUniforms {
    planes: [[f32; 4]; 6],
}

/// GPU-driven BVH traversal compute pipeline. Created at level load,
/// dispatched each frame before the render pass.
pub struct ComputeCullPipeline {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,

    node_buffer: wgpu::Buffer,
    leaf_buffer: wgpu::Buffer,
    visible_cells_buffer: wgpu::Buffer,
    uniform_buffer: wgpu::Buffer,

    /// Indirect draw buffer: one `DrawIndexedIndirect` per BVH leaf, indexed
    /// by leaf array position. Leaves are sorted by `material_bucket_id` so
    /// each bucket owns a contiguous slot range.
    indirect_buffer: wgpu::Buffer,
    /// Total number of leaves (= total indirect draw slots).
    total_leaves: u32,
    /// Per-bucket (first_leaf, leaf_count) ranges derived at load time.
    bucket_ranges: Vec<BucketRange>,

    has_multi_draw_indirect: bool,

    /// Per-leaf cull status buffer for debug wireframe overlay. One u32 per
    /// leaf: 0 = portal-culled, 1 = frustum-culled, 2 = visible/rendered.
    cull_status_buffer: wgpu::Buffer,

    /// Scratch buffer used to construct the 128-word visible-cell bitmask
    /// each frame. Reused to avoid a per-frame allocation.
    visible_bitmask_scratch: Vec<u32>,
}

impl ComputeCullPipeline {
    /// Create the compute culling pipeline and upload the BVH to GPU.
    pub fn new(device: &wgpu::Device, bvh: &BvhTree, has_multi_draw_indirect: bool) -> Self {
        let total_leaves = bvh.leaves.len() as u32;
        let bucket_ranges = bvh.derive_bucket_ranges();

        // Node storage buffer.
        let node_bytes = serialize_bvh_nodes(&bvh.nodes);
        let node_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("BVH Node Storage"),
            contents: if node_bytes.is_empty() {
                &[0u8; 40]
            } else {
                &node_bytes
            },
            usage: wgpu::BufferUsages::STORAGE,
        });

        // Leaf storage buffer.
        let leaf_bytes = serialize_bvh_leaves(&bvh.leaves);
        let leaf_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("BVH Leaf Storage"),
            contents: if leaf_bytes.is_empty() {
                &[0u8; 40]
            } else {
                &leaf_bytes
            },
            usage: wgpu::BufferUsages::STORAGE,
        });

        // Visible-cells bitmask buffer (fixed 512 bytes). Uploaded each frame.
        let visible_cells_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Visible Cells Bitmask"),
            size: VISIBLE_CELLS_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Cull uniforms buffer (6 planes = 96 bytes).
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Cull Uniforms"),
            size: CULL_UNIFORMS_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Indirect draw buffer: one DrawIndexedIndirect slot per leaf. The
        // compute shader writes each leaf's slot every frame (or zeroes
        // index_count for culled slots), so no template or per-frame reset
        // is required.
        let indirect_buffer_size = (total_leaves.max(1) as u64) * DRAW_INDIRECT_SIZE;
        let indirect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Indirect Draw Buffer"),
            size: indirect_buffer_size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Per-leaf cull status buffer for debug wireframe overlay.
        let cull_status_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Cull Status Buffer"),
            size: (total_leaves.max(1) as u64) * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("BVH Cull Compute Shader"),
            source: wgpu::ShaderSource::Wgsl(CULL_SHADER_SOURCE.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("BVH Cull Bind Group Layout"),
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
                // binding 1: nodes
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
                // binding 2: leaves
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
                // binding 3: visible_cells bitmask
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
                // binding 5: cull_status (read-write)
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
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
            label: Some("BVH Cull Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("BVH Cull Compute Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("cull_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        log::info!(
            "[Renderer] BVH cull pipeline ready: {} nodes, {} leaves, {} buckets, multi_draw={}",
            bvh.nodes.len(),
            total_leaves,
            bucket_ranges.len(),
            has_multi_draw_indirect,
        );

        Self {
            pipeline,
            bind_group_layout,
            node_buffer,
            leaf_buffer,
            visible_cells_buffer,
            uniform_buffer,
            indirect_buffer,
            total_leaves,
            bucket_ranges,
            has_multi_draw_indirect,
            cull_status_buffer,
            visible_bitmask_scratch: vec![0u32; VISIBLE_CELLS_WORDS],
        }
    }

    /// Build the visible-cell bitmask from a flat cell-id list. Cell IDs
    /// outside the bitmask's range (0..4096) are clamped out with a
    /// one-time warning log; sub-plan 1 already constrains cell IDs so this
    /// should never fire in practice.
    fn write_bitmask_from_cells(&mut self, cells: &[u32]) {
        for w in &mut self.visible_bitmask_scratch {
            *w = 0;
        }
        for &cell in cells {
            if cell >= MAX_VISIBLE_CELLS {
                log::warn!(
                    "[Renderer] cell_id {} exceeds visible-cell bitmask capacity {}",
                    cell,
                    MAX_VISIBLE_CELLS
                );
                continue;
            }
            let word = (cell >> 5) as usize;
            let bit = 1u32 << (cell & 31);
            self.visible_bitmask_scratch[word] |= bit;
        }
    }

    fn write_bitmask_draw_all(&mut self) {
        for w in &mut self.visible_bitmask_scratch {
            *w = 0xFFFFFFFFu32;
        }
    }

    /// Upload visible cell bitmask and frustum planes, then dispatch the
    /// BVH traversal compute shader. After this call the indirect buffer
    /// is ready for `draw_indexed_indirect` / `multi_draw_indexed_indirect`.
    pub fn dispatch(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        visible: &crate::visibility::VisibleCells,
        view_proj: &Mat4,
    ) {
        // Build the visible-cell bitmask on CPU and upload to the fixed
        // 512-byte storage buffer. This is the per-frame portal DFS
        // handoff to the compute shader.
        match visible {
            crate::visibility::VisibleCells::Culled(cells) => {
                self.write_bitmask_from_cells(cells);
            }
            crate::visibility::VisibleCells::DrawAll => {
                self.write_bitmask_draw_all();
            }
        }
        let bitmask_bytes = serialize_u32_slice(&self.visible_bitmask_scratch);
        queue.write_buffer(&self.visible_cells_buffer, 0, &bitmask_bytes);

        // Upload frustum planes.
        let planes = extract_frustum_planes_for_gpu(view_proj);
        let uniforms = CullUniforms { planes };
        let uniforms_bytes = serialize_cull_uniforms(&uniforms);
        queue.write_buffer(&self.uniform_buffer, 0, &uniforms_bytes);

        // Reset cull_status to 0 (portal-culled) before dispatch via
        // `clear_buffer`, which zeros directly on the GPU and avoids a
        // per-frame host-side allocation. The compute shader writes 1
        // (frustum) or 2 (visible) for leaves it touches; untouched leaves
        // (all of them are touched by the single DFS walk) retain 0.
        if self.total_leaves > 0 {
            encoder.clear_buffer(&self.cull_status_buffer, 0, None);
        }

        // Early out: nothing to cull.
        if self.total_leaves == 0 {
            return;
        }

        // Build the bind group each frame. Caching on buffer resize is a
        // deferred perf follow-up — buffers here are sized once at level load.
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("BVH Cull Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.node_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.leaf_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.visible_cells_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.indirect_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: self.cull_status_buffer.as_entire_binding(),
                },
            ],
        });

        let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("BVH Cull Pass"),
            timestamp_writes: None,
        });

        compute_pass.set_pipeline(&self.pipeline);
        compute_pass.set_bind_group(0, &bind_group, &[]);
        compute_pass.dispatch_workgroups(1, 1, 1);
    }

    /// Issue indirect draw calls for the render pass. One call per material
    /// bucket via `multi_draw_indexed_indirect` (or the singular fallback).
    /// `set_texture_fn` binds the correct texture before each bucket's draws.
    pub fn draw_indirect<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        set_texture_fn: &dyn Fn(&mut wgpu::RenderPass<'a>, u32),
    ) {
        for range in &self.bucket_ranges {
            if range.leaf_count == 0 {
                continue;
            }

            set_texture_fn(render_pass, range.material_bucket_id);
            let byte_offset = (range.first_leaf as u64) * DRAW_INDIRECT_SIZE;

            if self.has_multi_draw_indirect {
                render_pass.multi_draw_indexed_indirect(
                    &self.indirect_buffer,
                    byte_offset,
                    range.leaf_count,
                );
            } else {
                for i in 0..range.leaf_count {
                    let offset = byte_offset + (i as u64) * DRAW_INDIRECT_SIZE;
                    render_pass.draw_indexed_indirect(&self.indirect_buffer, offset);
                }
            }
        }
    }

    /// Reference to the per-leaf cull status buffer for the wireframe overlay.
    pub fn cull_status_buffer(&self) -> &wgpu::Buffer {
        &self.cull_status_buffer
    }
}

// --- GPU data serialization ---

const CULL_UNIFORMS_SIZE: usize = 96;

fn serialize_cull_uniforms(uniforms: &CullUniforms) -> Vec<u8> {
    let mut buf = Vec::with_capacity(CULL_UNIFORMS_SIZE);
    for plane in &uniforms.planes {
        for &v in plane {
            buf.extend_from_slice(&v.to_le_bytes());
        }
    }
    buf
}

fn serialize_u32_slice(slice: &[u32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(slice.len() * 4);
    for &val in slice {
        buf.extend_from_slice(&val.to_le_bytes());
    }
    buf
}

/// Serialize BVH nodes to the 40-byte WGSL storage layout.
///
/// Written as `min.x, min.y, min.z, skip_index, max.x, max.y, max.z,
/// left_child_or_leaf_index, flags, _pad` — six scalar f32s + four u32s.
/// This matches the scalar-field WGSL struct shape on purpose: declaring
/// the AABB corners as `vec3<f32>` on the shader side would push the
/// struct stride from 40 to 48 (see the comment at the WGSL struct
/// definition above and the `wgsl_struct_strides_are_40_bytes` regression
/// test), silently garbling every node after index 0.
fn serialize_bvh_nodes(nodes: &[crate::geometry::BvhNode]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(nodes.len() * 40);
    for node in nodes {
        for &c in &node.aabb_min {
            buf.extend_from_slice(&c.to_le_bytes());
        }
        buf.extend_from_slice(&node.skip_index.to_le_bytes());
        for &c in &node.aabb_max {
            buf.extend_from_slice(&c.to_le_bytes());
        }
        buf.extend_from_slice(&node.left_child_or_leaf_index.to_le_bytes());
        // Mask to the only flag bit we currently use; everything else is
        // reserved and must be zero to match the WGSL struct expectation.
        let flags = node.flags & BVH_NODE_FLAG_LEAF;
        buf.extend_from_slice(&flags.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes()); // _pad
    }
    buf
}

/// Serialize BVH leaves to the 40-byte WGSL storage layout.
fn serialize_bvh_leaves(leaves: &[crate::geometry::BvhLeaf]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(leaves.len() * 40);
    for leaf in leaves {
        for &c in &leaf.aabb_min {
            buf.extend_from_slice(&c.to_le_bytes());
        }
        buf.extend_from_slice(&leaf.material_bucket_id.to_le_bytes());
        for &c in &leaf.aabb_max {
            buf.extend_from_slice(&c.to_le_bytes());
        }
        buf.extend_from_slice(&leaf.index_offset.to_le_bytes());
        buf.extend_from_slice(&leaf.index_count.to_le_bytes());
        buf.extend_from_slice(&leaf.cell_id.to_le_bytes());
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
    use crate::geometry::{BvhLeaf, BvhNode, BvhTree};

    fn leaf_node(leaf_index: u32, skip_index: u32) -> BvhNode {
        BvhNode {
            aabb_min: [0.0; 3],
            skip_index,
            aabb_max: [1.0; 3],
            left_child_or_leaf_index: leaf_index,
            flags: BVH_NODE_FLAG_LEAF,
        }
    }

    fn leaf(material_bucket_id: u32, cell_id: u32) -> BvhLeaf {
        BvhLeaf {
            aabb_min: [0.0; 3],
            material_bucket_id,
            aabb_max: [1.0; 3],
            index_offset: 0,
            index_count: 3,
            cell_id,
        }
    }

    #[test]
    fn cull_uniforms_size() {
        let uniforms = CullUniforms {
            planes: [[0.0; 4]; 6],
        };
        assert_eq!(serialize_cull_uniforms(&uniforms).len(), CULL_UNIFORMS_SIZE);
    }

    #[test]
    fn bvh_node_serialization_is_40_bytes() {
        let node = leaf_node(0, 1);
        let bytes = serialize_bvh_nodes(&[node]);
        assert_eq!(bytes.len(), 40);
    }

    #[test]
    fn bvh_leaf_serialization_is_40_bytes() {
        let bytes = serialize_bvh_leaves(&[leaf(0, 0)]);
        assert_eq!(bytes.len(), 40);
    }

    #[test]
    fn single_leaf_bvh_bucket_ranges() {
        let tree = BvhTree {
            nodes: vec![leaf_node(0, 1)],
            leaves: vec![leaf(0, 0)],
            root_node_index: 0,
        };
        let ranges = tree.derive_bucket_ranges();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].leaf_count, 1);
    }

    #[test]
    fn bitmask_round_trip_bit_math() {
        // Independent re-derivation of the bitmask bit test used in the
        // shader.
        fn is_visible(bitmask: &[u32], cell_id: u32) -> bool {
            let word = (cell_id >> 5) as usize;
            let bit = 1u32 << (cell_id & 31);
            (bitmask[word] & bit) != 0
        }

        let mut bitmask = vec![0u32; VISIBLE_CELLS_WORDS];
        for &cell in &[0u32, 1, 31, 32, 63, 100, 4095] {
            let word = (cell >> 5) as usize;
            let bit = 1u32 << (cell & 31);
            bitmask[word] |= bit;
        }

        for &cell in &[0u32, 1, 31, 32, 63, 100, 4095] {
            assert!(is_visible(&bitmask, cell));
        }
        assert!(!is_visible(&bitmask, 2));
        assert!(!is_visible(&bitmask, 4094));
    }

    #[test]
    fn draw_indirect_size_is_20_bytes() {
        assert_eq!(DRAW_INDIRECT_SIZE, 20);
    }

    #[test]
    fn visible_cells_bitmask_buffer_size() {
        assert_eq!(VISIBLE_CELLS_BYTES, 512);
        assert_eq!(MAX_VISIBLE_CELLS, 4096);
    }

    /// Regression: a WGSL `vec3<f32>` has alignment 16, so any struct that
    /// contains one gets rounded up to 48-byte stride — silently shifting
    /// every node/leaf after index 0 in the GPU storage buffers and reading
    /// garbage. The fix is to store the AABB corners as six scalar `f32`
    /// fields. This test parses the live shader source with naga and asserts
    /// both `BvhNode` and `BvhLeaf` end up at 40 bytes. If someone re-vec3s
    /// either struct, this test fails loudly before the breakage reaches
    /// a GPU round-trip.
    #[test]
    fn wgsl_struct_strides_are_40_bytes() {
        let module = naga::front::wgsl::parse_str(CULL_SHADER_SOURCE)
            .expect("cull shader should parse as WGSL");

        let mut seen = std::collections::HashMap::new();
        for (_handle, ty) in module.types.iter() {
            if let naga::TypeInner::Struct { span, .. } = &ty.inner {
                if let Some(name) = &ty.name {
                    seen.insert(name.clone(), *span);
                }
            }
        }

        let node_span = seen
            .get("BvhNode")
            .copied()
            .expect("shader should declare struct BvhNode");
        let leaf_span = seen
            .get("BvhLeaf")
            .copied()
            .expect("shader should declare struct BvhLeaf");

        assert_eq!(
            node_span, 40,
            "BvhNode WGSL stride is {node_span}, expected 40; \
             a vec3<f32> field likely crept back in (align 16 → stride 48)"
        );
        assert_eq!(
            leaf_span, 40,
            "BvhLeaf WGSL stride is {leaf_span}, expected 40; \
             a vec3<f32> field likely crept back in (align 16 → stride 48)"
        );
    }

    #[test]
    fn unbalanced_bvh_skip_index_layout() {
        // Simulate a deep right-leaning chain: every internal node's left
        // child is a leaf, and `skip_index` must walk past the right subtree.
        // This just exercises the Rust-side data model — the shader walks
        // nodes in DFS order via `skip_index` and never indexes past the
        // flat node array.
        let nodes = vec![
            BvhNode {
                aabb_min: [0.0; 3],
                skip_index: 5,
                aabb_max: [1.0; 3],
                left_child_or_leaf_index: 0,
                flags: 0,
            },
            leaf_node(0, 2),
            BvhNode {
                aabb_min: [0.0; 3],
                skip_index: 5,
                aabb_max: [1.0; 3],
                left_child_or_leaf_index: 0,
                flags: 0,
            },
            leaf_node(1, 4),
            leaf_node(2, 5),
        ];
        let tree = BvhTree {
            nodes,
            leaves: vec![leaf(0, 0), leaf(0, 1), leaf(0, 2)],
            root_node_index: 0,
        };
        let bytes = serialize_bvh_nodes(&tree.nodes);
        assert_eq!(bytes.len(), tree.nodes.len() * 40);
        let leaf_bytes = serialize_bvh_leaves(&tree.leaves);
        assert_eq!(leaf_bytes.len(), tree.leaves.len() * 40);
    }
}
