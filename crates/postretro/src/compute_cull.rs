// GPU-driven BVH traversal compute pipeline and indirect draw dispatch.
// See: context/lib/rendering_pipeline.md §7.1

use glam::Mat4;
use wgpu::util::DeviceExt;

use crate::geometry::{BVH_NODE_FLAG_LEAF, BucketRange, BvhTree};

/// The `+ 'a` bound is required because type aliases default the trait
/// object lifetime to `'static`, unlike an inline `&dyn Fn(...)` which
/// picks up the outer reference's lifetime via elision.
pub type SetTextureFn<'a> = dyn Fn(&mut wgpu::RenderPass<'a>, u32) + 'a;

// All GPU uploads below use little-endian byte order because the WGSL storage
// buffers, PRL on-disk format, and every wgpu backend target (Vulkan, Metal,
// DX12 on x86_64 / aarch64 / wasm32) are little-endian. Enforce at compile
// time so a hypothetical big-endian build fails loudly instead of silently
// scrambling BVH data.
const _: () = assert!(
    cfg!(target_endian = "little"),
    "postretro GPU upload path assumes little-endian; add a byte-swap layer before porting"
);

const DRAW_INDIRECT_SIZE: u64 = 20;

/// Fixed 128-word (512-byte) bitmask covering up to 4096 cell IDs. Fixed
/// size removes any resize path from the hot frame loop.
pub(crate) const VISIBLE_CELLS_WORDS: usize = 128;
const VISIBLE_CELLS_BYTES: u64 = (VISIBLE_CELLS_WORDS * 4) as u64;
pub(crate) const MAX_VISIBLE_CELLS: u32 = (VISIBLE_CELLS_WORDS as u32) * 32;

// Rust serializers write matching strides: 40 bytes for `BvhNode`, 48 for
// `BvhLeaf`. `wgsl_bvh_struct_strides_match_spec` pins the contract against naga.
const CULL_SHADER_SOURCE: &str = include_str!("shaders/bvh_cull.wgsl");

#[derive(Debug, Clone, Copy)]
struct CullUniforms {
    planes: [[f32; 4]; 6],
}

pub struct ComputeCullPipeline {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,

    node_buffer: wgpu::Buffer,
    leaf_buffer: wgpu::Buffer,
    visible_cells_buffer: wgpu::Buffer,
    uniform_buffer: wgpu::Buffer,

    /// One `DrawIndexedIndirect` slot per leaf, indexed by leaf array position.
    /// Leaves sorted by `material_bucket_id` so each bucket owns a contiguous range.
    indirect_buffer: wgpu::Buffer,
    total_leaves: u32,
    bucket_ranges: Vec<BucketRange>,

    has_multi_draw_indirect: bool,

    /// Per-leaf: 0 = portal-culled, 1 = frustum-culled, 2 = visible/rendered.
    cull_status_buffer: wgpu::Buffer,

    visible_bitmask_scratch: Vec<u32>,
}

impl ComputeCullPipeline {
    pub fn new(device: &wgpu::Device, bvh: &BvhTree, has_multi_draw_indirect: bool) -> Self {
        let total_leaves = bvh.leaves.len() as u32;
        let bucket_ranges = bvh.derive_bucket_ranges();

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

        let leaf_bytes = serialize_bvh_leaves(&bvh.leaves);
        let leaf_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("BVH Leaf Storage"),
            contents: if leaf_bytes.is_empty() {
                &[0u8; 48]
            } else {
                &leaf_bytes
            },
            usage: wgpu::BufferUsages::STORAGE,
        });

        let visible_cells_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Visible Cells Bitmask"),
            size: VISIBLE_CELLS_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Cull Uniforms"),
            size: CULL_UNIFORMS_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let indirect_buffer_size = (total_leaves.max(1) as u64) * DRAW_INDIRECT_SIZE;
        let indirect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Indirect Draw Buffer"),
            size: indirect_buffer_size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

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

    pub fn dispatch(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        visible: &crate::visibility::VisibleCells,
        view_proj: &Mat4,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
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

        let planes = extract_frustum_planes_for_gpu(view_proj);
        let uniforms = CullUniforms { planes };
        let uniforms_bytes = serialize_cull_uniforms(&uniforms);
        queue.write_buffer(&self.uniform_buffer, 0, &uniforms_bytes);

        // clear_buffer zeros on the GPU; compute shader then writes 1/2 for touched leaves.
        if self.total_leaves > 0 {
            encoder.clear_buffer(&self.cull_status_buffer, 0, None);
        }

        if self.total_leaves == 0 {
            return;
        }

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
            timestamp_writes,
        });

        compute_pass.set_pipeline(&self.pipeline);
        compute_pass.set_bind_group(0, &bind_group, &[]);
        compute_pass.dispatch_workgroups(1, 1, 1);
    }

    /// Pass `set_texture_fn = None` for depth-only passes (e.g. depth pre-pass)
    /// whose pipeline layout has no group 1 slot — binding one would fail wgpu validation.
    pub fn draw_indirect<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        set_texture_fn: Option<&SetTextureFn<'a>>,
    ) {
        for range in &self.bucket_ranges {
            if range.leaf_count == 0 {
                continue;
            }

            if let Some(f) = set_texture_fn {
                f(render_pass, range.material_bucket_id);
            }
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

    pub fn cull_status_buffer(&self) -> &wgpu::Buffer {
        &self.cull_status_buffer
    }

    pub fn debug_bitmask_fingerprint(&self) -> (u32, u32) {
        let mut pop = 0u32;
        let mut hash = 0u32;
        for (i, &w) in self.visible_bitmask_scratch.iter().enumerate() {
            pop += w.count_ones();
            hash ^= w.wrapping_mul((i as u32).wrapping_mul(2654435761).wrapping_add(1));
        }
        (pop, hash)
    }
}

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
        let flags = node.flags & BVH_NODE_FLAG_LEAF;
        buf.extend_from_slice(&flags.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes()); // _pad
    }
    buf
}

fn serialize_bvh_leaves(leaves: &[crate::geometry::BvhLeaf]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(leaves.len() * 48);
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
        buf.extend_from_slice(&leaf.chunk_range_start.to_le_bytes());
        buf.extend_from_slice(&leaf.chunk_range_count.to_le_bytes());
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
            chunk_range_start: 0,
            chunk_range_count: 0,
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
    fn bvh_leaf_serialization_is_48_bytes() {
        let bytes = serialize_bvh_leaves(&[leaf(0, 0)]);
        assert_eq!(bytes.len(), 48);
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

    /// Guards against `vec3<f32>` creeping back into the WGSL structs: alignment 16
    /// would silently shift every node/leaf after index 0 in the GPU storage buffers.
    #[test]
    fn wgsl_bvh_struct_strides_match_spec() {
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
            leaf_span, 48,
            "BvhLeaf WGSL stride is {leaf_span}, expected 48; \
             a vec3<f32> field likely crept back in (align 16 → stride 64), \
             or the chunk_range_* fields were dropped"
        );
    }

    #[test]
    fn unbalanced_bvh_skip_index_layout() {
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
        assert_eq!(leaf_bytes.len(), tree.leaves.len() * 48);
    }
}
