// GPU-driven BVH traversal compute pipeline and indirect draw dispatch.
// See: context/lib/rendering_pipeline.md §7.1

use glam::Mat4;
use wgpu::util::DeviceExt;

use crate::geometry::{BVH_NODE_FLAG_LEAF, BucketRange, BvhLeaf, BvhNode, BvhTree};

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

pub(crate) const DRAW_INDIRECT_SIZE: u64 = 20;

/// Fixed 128-word (512-byte) bitmask covering up to 4096 cell IDs. Fixed
/// size removes any resize path from the hot frame loop.
pub(crate) const VISIBLE_CELLS_WORDS: usize = 128;
const VISIBLE_CELLS_BYTES: u64 = (VISIBLE_CELLS_WORDS * 4) as u64;
pub(crate) const MAX_VISIBLE_CELLS: u32 = (VISIBLE_CELLS_WORDS as u32) * 32;
const FRONTIER_TARGET_SUBTREES: usize = 64;

// Rust serializers write matching strides: 40 bytes for `BvhNode`, 48 for
// `BvhLeaf`. `wgsl_bvh_struct_strides_match_spec` pins the contract against naga.
pub(crate) const CULL_SHADER_SOURCE: &str = include_str!("shaders/bvh_cull.wgsl");

#[derive(Debug, Clone, Copy)]
pub(crate) struct CullUniforms {
    pub(crate) planes: [[f32; 4]; 6],
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub struct BvhFrontierDiagnostics {
    pub frontier_subtrees: u32,
    pub total_estimated_work: u32,
    pub max_subtree_work: u32,
    pub imbalance_ratio: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub struct BvhCullDiagnostics {
    pub estimated_node_visits: u32,
    pub leaf_tests: u32,
    pub frustum_rejects: u32,
    pub visible_cell_rejects: u32,
    pub submitted_leaves: u32,
    pub submitted_index_count: u32,
    pub submitted_bucket_spans: u32,
    pub frontier: BvhFrontierDiagnostics,
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
    // CPU mirrors of the read-only BVH, kept for the dev-tools-only cull-cost
    // estimate (`estimate_diagnostics`). Dead in shipping builds.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    bvh_nodes: Vec<BvhNode>,
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    bvh_leaves: Vec<BvhLeaf>,
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
            bvh_nodes: bvh.nodes.clone(),
            bvh_leaves: bvh.leaves.clone(),
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
        draw_indirect_buckets(
            render_pass,
            &self.indirect_buffer,
            0,
            &self.bucket_ranges,
            self.has_multi_draw_indirect,
            set_texture_fn,
        );
    }

    pub fn cull_status_buffer(&self) -> &wgpu::Buffer {
        &self.cull_status_buffer
    }

    /// The global per-leaf indirect draw buffer. The candidate-cull path
    /// (`CandidateCullPipeline`) writes the SAME slots in this buffer that the
    /// tree walk does, so the draw path (`bucket_ranges` / `draw_indirect_buckets`)
    /// is byte-for-byte identical regardless of which cull ran.
    pub(crate) fn indirect_buffer(&self) -> &wgpu::Buffer {
        &self.indirect_buffer
    }

    /// Read-only BVH node storage buffer, uploaded once at level load. The
    /// shadow cull owner (`ShadowCullPipeline`) binds the SAME buffer rather
    /// than re-serializing the BVH per slot.
    pub(crate) fn node_buffer(&self) -> &wgpu::Buffer {
        &self.node_buffer
    }

    /// Read-only BVH leaf storage buffer, shared with the shadow cull owner.
    pub(crate) fn leaf_buffer(&self) -> &wgpu::Buffer {
        &self.leaf_buffer
    }

    pub(crate) fn total_leaves(&self) -> u32 {
        self.total_leaves
    }

    pub(crate) fn bucket_ranges(&self) -> &[BucketRange] {
        &self.bucket_ranges
    }

    pub(crate) fn has_multi_draw_indirect(&self) -> bool {
        self.has_multi_draw_indirect
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

    /// Estimate the would-be tree-walk cull cost for this frame's frustum and
    /// visible set. Pure read-only analysis over the CPU BVH mirrors — runs
    /// regardless of which GPU cull strategy actually dispatched, so the
    /// baseline panel is never starved when the candidate path takes over. The
    /// bucket scratch is a one-per-frame local alloc; this is dev-tools-only.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn estimate_diagnostics(
        &self,
        visible: &crate::visibility::VisibleCells,
        view_proj: &Mat4,
    ) -> BvhCullDiagnostics {
        let planes = extract_frustum_planes_for_gpu(view_proj);
        let mut bucket_scratch = vec![false; self.bucket_ranges.len()];
        estimate_bvh_cull_with_planes(
            &self.bvh_nodes,
            &self.bvh_leaves,
            &self.bucket_ranges,
            visible,
            &planes,
            &mut bucket_scratch,
        )
    }
}

fn build_visible_cell_bitmask(
    visible: &crate::visibility::VisibleCells,
) -> [u32; VISIBLE_CELLS_WORDS] {
    let mut bitmask = [0u32; VISIBLE_CELLS_WORDS];
    match visible {
        crate::visibility::VisibleCells::Culled(cells) => {
            for &cell in cells {
                if cell >= MAX_VISIBLE_CELLS {
                    continue;
                }
                let word = (cell >> 5) as usize;
                let bit = 1u32 << (cell & 31);
                bitmask[word] |= bit;
            }
        }
        crate::visibility::VisibleCells::DrawAll => bitmask.fill(u32::MAX),
    }
    bitmask
}

fn bitmask_cell_is_visible(bitmask: &[u32; VISIBLE_CELLS_WORDS], cell_id: u32) -> bool {
    let word = (cell_id >> 5) as usize;
    if word >= bitmask.len() {
        return false;
    }
    let bit = 1u32 << (cell_id & 31);
    (bitmask[word] & bit) != 0
}

fn is_aabb_outside_gpu_planes(
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
    planes: &[[f32; 4]; 6],
) -> bool {
    for plane in planes {
        let px = if plane[0] >= 0.0 {
            aabb_max[0]
        } else {
            aabb_min[0]
        };
        let py = if plane[1] >= 0.0 {
            aabb_max[1]
        } else {
            aabb_min[1]
        };
        let pz = if plane[2] >= 0.0 {
            aabb_max[2]
        } else {
            aabb_min[2]
        };
        if plane[0] * px + plane[1] * py + plane[2] * pz + plane[3] < 0.0 {
            return true;
        }
    }
    false
}

fn next_skip_index(current: usize, skip_index: u32, limit: usize) -> usize {
    let next = skip_index as usize;
    debug_assert!(
        next > current || next >= limit,
        "flat BVH skip_index must advance traversal"
    );
    if next > current {
        next.min(limit)
    } else {
        limit
    }
}

fn estimate_subtree_work(
    nodes: &[BvhNode],
    leaves: &[BvhLeaf],
    planes: &[[f32; 4]; 6],
    visible_bitmask: &[u32; VISIBLE_CELLS_WORDS],
    root: usize,
    end: usize,
) -> BvhCullDiagnostics {
    let mut diagnostics = BvhCullDiagnostics::default();
    let mut i = root;
    let end = end.min(nodes.len());

    while i < end {
        let node = nodes[i];
        diagnostics.estimated_node_visits += 1;
        if is_aabb_outside_gpu_planes(node.aabb_min, node.aabb_max, planes) {
            diagnostics.frustum_rejects += 1;
            i = next_skip_index(i, node.skip_index, end);
            continue;
        }

        if (node.flags & BVH_NODE_FLAG_LEAF) != 0 {
            let leaf_idx = node.left_child_or_leaf_index as usize;
            if let Some(leaf) = leaves.get(leaf_idx) {
                diagnostics.leaf_tests += 1;
                if is_aabb_outside_gpu_planes(leaf.aabb_min, leaf.aabb_max, planes) {
                    diagnostics.frustum_rejects += 1;
                } else if !bitmask_cell_is_visible(visible_bitmask, leaf.cell_id) {
                    diagnostics.visible_cell_rejects += 1;
                } else {
                    diagnostics.submitted_leaves += 1;
                    diagnostics.submitted_index_count = diagnostics
                        .submitted_index_count
                        .saturating_add(leaf.index_count);
                }
            }
            i = next_skip_index(i, node.skip_index, end);
        } else {
            i += 1;
        }
    }

    diagnostics
}

fn child_roots(nodes: &[BvhNode], root: usize) -> Option<(usize, usize)> {
    let node = nodes.get(root)?;
    if (node.flags & BVH_NODE_FLAG_LEAF) != 0 {
        return None;
    }
    let left = root + 1;
    let left_node = nodes.get(left)?;
    let right = left_node.skip_index as usize;
    if right <= left || right >= node.skip_index as usize || right >= nodes.len() {
        return None;
    }
    Some((left, right))
}

fn fixed_frontier_roots(nodes: &[BvhNode]) -> Vec<usize> {
    if nodes.is_empty() {
        return Vec::new();
    }

    let mut frontier = vec![0usize];
    while frontier.len() < FRONTIER_TARGET_SUBTREES {
        let Some(expand_at) = frontier
            .iter()
            .position(|&root| child_roots(nodes, root).is_some())
        else {
            break;
        };
        let root = frontier.remove(expand_at);
        let Some((left, right)) = child_roots(nodes, root) else {
            frontier.insert(expand_at, root);
            break;
        };
        frontier.insert(expand_at, right);
        frontier.insert(expand_at, left);
    }

    frontier
}

fn estimate_fixed_frontier(
    nodes: &[BvhNode],
    leaves: &[BvhLeaf],
    planes: &[[f32; 4]; 6],
    visible_bitmask: &[u32; VISIBLE_CELLS_WORDS],
) -> BvhFrontierDiagnostics {
    let roots = fixed_frontier_roots(nodes);
    if roots.is_empty() {
        return BvhFrontierDiagnostics::default();
    }

    let mut total_work = 0u32;
    let mut max_work = 0u32;
    for &root in &roots {
        let end = nodes
            .get(root)
            .map(|node| node.skip_index as usize)
            .unwrap_or(nodes.len());
        let subtree = estimate_subtree_work(nodes, leaves, planes, visible_bitmask, root, end);
        let work = subtree
            .estimated_node_visits
            .saturating_add(subtree.leaf_tests);
        total_work = total_work.saturating_add(work);
        max_work = max_work.max(work);
    }

    let avg_work = total_work as f32 / roots.len() as f32;
    BvhFrontierDiagnostics {
        frontier_subtrees: roots.len() as u32,
        total_estimated_work: total_work,
        max_subtree_work: max_work,
        imbalance_ratio: if avg_work > 0.0 {
            max_work as f32 / avg_work
        } else {
            0.0
        },
    }
}

fn estimate_bvh_cull_with_planes(
    nodes: &[BvhNode],
    leaves: &[BvhLeaf],
    bucket_ranges: &[BucketRange],
    visible: &crate::visibility::VisibleCells,
    planes: &[[f32; 4]; 6],
    submitted_bucket_scratch: &mut Vec<bool>,
) -> BvhCullDiagnostics {
    let visible_bitmask = build_visible_cell_bitmask(visible);
    let mut diagnostics =
        estimate_subtree_work(nodes, leaves, planes, &visible_bitmask, 0, nodes.len());

    if submitted_bucket_scratch.len() != bucket_ranges.len() {
        submitted_bucket_scratch.resize(bucket_ranges.len(), false);
    }
    submitted_bucket_scratch.fill(false);

    if diagnostics.submitted_leaves > 0 {
        let mut i = 0usize;
        while i < nodes.len() {
            let node = nodes[i];
            if is_aabb_outside_gpu_planes(node.aabb_min, node.aabb_max, planes) {
                i = next_skip_index(i, node.skip_index, nodes.len());
                continue;
            }
            if (node.flags & BVH_NODE_FLAG_LEAF) != 0 {
                let leaf_idx = node.left_child_or_leaf_index as usize;
                if let Some(leaf) = leaves.get(leaf_idx) {
                    let submitted =
                        !is_aabb_outside_gpu_planes(leaf.aabb_min, leaf.aabb_max, planes)
                            && bitmask_cell_is_visible(&visible_bitmask, leaf.cell_id);
                    if submitted {
                        if let Some(bucket_index) = bucket_ranges.iter().position(|range| {
                            let start = range.first_leaf as usize;
                            let end = start + range.leaf_count as usize;
                            (start..end).contains(&leaf_idx)
                        }) {
                            submitted_bucket_scratch[bucket_index] = true;
                        }
                    }
                }
                i = next_skip_index(i, node.skip_index, nodes.len());
            } else {
                i += 1;
            }
        }
    }

    diagnostics.submitted_bucket_spans = submitted_bucket_scratch
        .iter()
        .filter(|&&submitted| submitted)
        .count() as u32;
    diagnostics.frontier = estimate_fixed_frontier(nodes, leaves, planes, &visible_bitmask);
    diagnostics
}

/// Issue one `multi_draw_indexed_indirect` (or a fallback loop of
/// `draw_indexed_indirect`) per material bucket over a slice of an indirect
/// buffer. `region_byte_offset` is the byte offset of the slot's per-leaf
/// region within the indirect buffer (0 for the camera path's single region;
/// `slot * region_stride_bytes` for the shadow owner's per-slot sub-regions,
/// where `region_stride_bytes = (total_leaves * DRAW_INDIRECT_SIZE).next_multiple_of(256)`
/// — padded to 256 bytes to satisfy `min_storage_buffer_offset_alignment`).
/// The per-bucket `first_leaf`/`leaf_count` layout is identical across regions,
/// so the bucket-offset table is shared.
///
/// `set_texture_fn = None` skips the group-1 material bind (depth-only passes,
/// including the spot-shadow depth pass, have no group-1 slot).
pub(crate) fn draw_indirect_buckets<'a>(
    render_pass: &mut wgpu::RenderPass<'a>,
    indirect_buffer: &'a wgpu::Buffer,
    region_byte_offset: u64,
    bucket_ranges: &[BucketRange],
    has_multi_draw_indirect: bool,
    set_texture_fn: Option<&SetTextureFn<'a>>,
) {
    for range in bucket_ranges {
        if range.leaf_count == 0 {
            continue;
        }

        if let Some(f) = set_texture_fn {
            f(render_pass, range.material_bucket_id);
        }
        let byte_offset = region_byte_offset + (range.first_leaf as u64) * DRAW_INDIRECT_SIZE;

        if has_multi_draw_indirect {
            render_pass.multi_draw_indexed_indirect(indirect_buffer, byte_offset, range.leaf_count);
        } else {
            for i in 0..range.leaf_count {
                let offset = byte_offset + (i as u64) * DRAW_INDIRECT_SIZE;
                render_pass.draw_indexed_indirect(indirect_buffer, offset);
            }
        }
    }
}

pub(crate) const CULL_UNIFORMS_SIZE: usize = 96;

pub(crate) fn serialize_cull_uniforms(uniforms: &CullUniforms) -> Vec<u8> {
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

/// Extract the 6 frustum planes from a combined view-projection matrix in the
/// layout the cull WGSL (`bvh_cull.wgsl::is_aabb_outside_frustum`) consumes.
///
/// Convention (mirrored verbatim by the CPU cone-frustum code in
/// `lighting::cone_frustum`, so both tests agree): 6 planes from the combined
/// matrix rows — L,R,B,T,N,F = `r3+r0, r3-r0, r3+r1, r3-r1, r3+r2, r3-r2` —
/// normalized, emitted as `[nx,ny,nz,d]`. Inside-sign matches the WGSL: a point
/// `p` is *outside* a plane when `dot(normal, p) + d < 0`.
///
/// `pub(crate)` so the lighting module can build a spotlight's cone frustum from
/// `light_space_matrix()` through this same single implementation rather than
/// duplicating the row math.
pub(crate) fn extract_frustum_planes_for_gpu(view_proj: &Mat4) -> [[f32; 4]; 6] {
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

    fn leaf_node_aabb(leaf_index: u32, skip_index: u32, min: [f32; 3], max: [f32; 3]) -> BvhNode {
        BvhNode {
            aabb_min: min,
            skip_index,
            aabb_max: max,
            left_child_or_leaf_index: leaf_index,
            flags: BVH_NODE_FLAG_LEAF,
        }
    }

    fn internal_node(skip_index: u32, min: [f32; 3], max: [f32; 3]) -> BvhNode {
        BvhNode {
            aabb_min: min,
            skip_index,
            aabb_max: max,
            left_child_or_leaf_index: 0,
            flags: 0,
        }
    }

    fn leaf(material_bucket_id: u32, cell_id: u32) -> BvhLeaf {
        leaf_aabb(material_bucket_id, cell_id, [0.0; 3], [1.0; 3])
    }

    fn leaf_aabb(material_bucket_id: u32, cell_id: u32, min: [f32; 3], max: [f32; 3]) -> BvhLeaf {
        BvhLeaf {
            aabb_min: min,
            material_bucket_id,
            aabb_max: max,
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

    #[test]
    fn cpu_bvh_diagnostics_count_frustum_skipped_subtree() {
        let tree = BvhTree {
            nodes: vec![
                internal_node(3, [-0.5, -0.5, -0.5], [3.0, 0.5, 0.5]),
                leaf_node_aabb(0, 2, [-0.5, -0.5, -0.5], [0.5, 0.5, 0.5]),
                leaf_node_aabb(1, 3, [2.0, -0.5, -0.5], [3.0, 0.5, 0.5]),
            ],
            leaves: vec![
                leaf_aabb(0, 7, [-0.5, -0.5, -0.5], [0.5, 0.5, 0.5]),
                leaf_aabb(1, 8, [2.0, -0.5, -0.5], [3.0, 0.5, 0.5]),
            ],
            root_node_index: 0,
        };
        let planes = extract_frustum_planes_for_gpu(&Mat4::IDENTITY);
        let mut bucket_scratch = Vec::new();
        let diagnostics = estimate_bvh_cull_with_planes(
            &tree.nodes,
            &tree.leaves,
            &tree.derive_bucket_ranges(),
            &crate::visibility::VisibleCells::Culled(vec![7, 8]),
            &planes,
            &mut bucket_scratch,
        );

        assert_eq!(diagnostics.estimated_node_visits, 3);
        assert_eq!(diagnostics.leaf_tests, 1);
        assert_eq!(diagnostics.frustum_rejects, 1);
        assert_eq!(diagnostics.visible_cell_rejects, 0);
        assert_eq!(diagnostics.submitted_leaves, 1);
        assert_eq!(diagnostics.submitted_index_count, 3);
        assert_eq!(diagnostics.submitted_bucket_spans, 1);
        assert_eq!(diagnostics.frontier.frontier_subtrees, 2);
        assert_eq!(diagnostics.frontier.total_estimated_work, 3);
        assert_eq!(diagnostics.frontier.max_subtree_work, 2);
    }

    #[test]
    fn cpu_bvh_diagnostics_count_visible_cell_rejects() {
        let tree = BvhTree {
            nodes: vec![
                internal_node(3, [-0.5, -0.5, -0.5], [0.5, 0.5, 0.5]),
                leaf_node_aabb(0, 2, [-0.5, -0.5, -0.5], [0.0, 0.5, 0.5]),
                leaf_node_aabb(1, 3, [0.0, -0.5, -0.5], [0.5, 0.5, 0.5]),
            ],
            leaves: vec![
                leaf_aabb(0, 7, [-0.5, -0.5, -0.5], [0.0, 0.5, 0.5]),
                leaf_aabb(1, 8, [0.0, -0.5, -0.5], [0.5, 0.5, 0.5]),
            ],
            root_node_index: 0,
        };
        let planes = extract_frustum_planes_for_gpu(&Mat4::IDENTITY);
        let mut bucket_scratch = Vec::new();
        let diagnostics = estimate_bvh_cull_with_planes(
            &tree.nodes,
            &tree.leaves,
            &tree.derive_bucket_ranges(),
            &crate::visibility::VisibleCells::Culled(vec![7]),
            &planes,
            &mut bucket_scratch,
        );

        assert_eq!(diagnostics.estimated_node_visits, 3);
        assert_eq!(diagnostics.leaf_tests, 2);
        assert_eq!(diagnostics.frustum_rejects, 0);
        assert_eq!(diagnostics.visible_cell_rejects, 1);
        assert_eq!(diagnostics.submitted_leaves, 1);
        assert_eq!(diagnostics.submitted_index_count, 3);
        assert_eq!(diagnostics.submitted_bucket_spans, 1);
        assert_eq!(diagnostics.frontier.frontier_subtrees, 2);
        assert_eq!(diagnostics.frontier.total_estimated_work, 4);
        assert_eq!(diagnostics.frontier.max_subtree_work, 2);
        assert!((diagnostics.frontier.imbalance_ratio - 1.0).abs() < f32::EPSILON);
    }

    /// Guards against `vec3<f32>` creeping back into the WGSL structs: alignment 16
    /// would silently shift every node/leaf after index 0 in the GPU storage buffers.
    /// Parse a WGSL source and map every declared struct name to its naga
    /// `span` (the byte stride). Drift guard input is the actual shader text,
    /// never a hand-copied field list.
    fn struct_strides(source: &str) -> std::collections::HashMap<String, u32> {
        let module = naga::front::wgsl::parse_str(source).expect("shader should parse as WGSL");
        let mut seen = std::collections::HashMap::new();
        for (_handle, ty) in module.types.iter() {
            if let naga::TypeInner::Struct { span, .. } = &ty.inner {
                if let Some(name) = &ty.name {
                    seen.insert(name.clone(), *span);
                }
            }
        }
        seen
    }

    /// Extract the full text of a top-level `fn <name>(...) { ... }` from WGSL
    /// source, signature through the matching closing brace. Used to compare a
    /// helper byte-for-byte between two shaders that copy it (WGSL has no
    /// include). Returns the exact source slice including the braces.
    fn extract_wgsl_fn<'a>(source: &'a str, name: &str) -> &'a str {
        let needle = format!("fn {name}");
        let start = source
            .find(&needle)
            .unwrap_or_else(|| panic!("shader should declare fn {name}"));
        let brace_start = source[start..]
            .find('{')
            .map(|o| start + o)
            .expect("function should have a body");
        let mut depth = 0usize;
        for (i, c) in source[brace_start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &source[start..brace_start + i + 1];
                    }
                }
                _ => {}
            }
        }
        panic!("unbalanced braces extracting fn {name}");
    }

    /// `BvhNode`/`BvhLeaf` strides in `bvh_cull.wgsl` must match the on-disk
    /// layout. Guards against `vec3<f32>` creeping back in (align 16 would
    /// silently shift every node/leaf after index 0 in the GPU storage buffers).
    #[test]
    fn wgsl_bvh_struct_strides_match_spec() {
        let strides = struct_strides(CULL_SHADER_SOURCE);

        let node_span = strides
            .get("BvhNode")
            .copied()
            .expect("shader should declare struct BvhNode");
        let leaf_span = strides
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

    /// The candidate cull shader reuses `BvhLeaf`, `DrawIndexedIndirect`,
    /// `FrustumPlane`, and `CullUniforms` from `bvh_cull.wgsl`. Their naga
    /// strides must match between the two shaders, so the candidate path binds
    /// the SAME leaf/indirect storage buffers without a layout mismatch. Strides
    /// are derived from each shader's actual text, not a hand-copied list.
    #[test]
    fn candidate_shader_reuses_bvh_cull_struct_layouts() {
        let bvh = struct_strides(CULL_SHADER_SOURCE);
        let candidate = struct_strides(crate::candidate_cull::CANDIDATE_CULL_SHADER_SOURCE);

        for name in [
            "BvhLeaf",
            "DrawIndexedIndirect",
            "FrustumPlane",
            "CullUniforms",
        ] {
            let a = bvh
                .get(name)
                .unwrap_or_else(|| panic!("bvh_cull.wgsl should declare struct {name}"));
            let b = candidate
                .get(name)
                .unwrap_or_else(|| panic!("candidate_cull.wgsl should declare struct {name}"));
            assert_eq!(
                a, b,
                "struct {name} stride differs between bvh_cull.wgsl ({a}) and \
                 candidate_cull.wgsl ({b}); the candidate path must reuse the \
                 byte-for-byte layout to share storage buffers"
            );
        }

        // The shared BvhLeaf must still be 48 bytes in the candidate shader.
        assert_eq!(candidate.get("BvhLeaf").copied(), Some(48));
    }

    /// `is_aabb_outside_frustum` is copied byte-for-byte between the two shaders
    /// (no WGSL include). A diverging copy would frustum-test candidates with
    /// different math than the tree walk, breaking the equivalence proof.
    #[test]
    fn is_aabb_outside_frustum_is_identical_across_shaders() {
        let bvh_fn = extract_wgsl_fn(CULL_SHADER_SOURCE, "is_aabb_outside_frustum");
        let candidate_fn = extract_wgsl_fn(
            crate::candidate_cull::CANDIDATE_CULL_SHADER_SOURCE,
            "is_aabb_outside_frustum",
        );
        assert_eq!(
            bvh_fn, candidate_fn,
            "is_aabb_outside_frustum must be byte-for-byte identical between \
             bvh_cull.wgsl and candidate_cull.wgsl"
        );
    }

    /// The 16-byte `CandidateCullParams` ABI: the WGSL struct is exactly 16
    /// bytes (`candidate_count` + three pad words), the Rust constant agrees,
    /// and the CPU serializer writes 16 bytes with `candidate_count` in the
    /// first little-endian word.
    #[test]
    fn candidate_params_abi_is_sixteen_bytes() {
        let strides = struct_strides(crate::candidate_cull::CANDIDATE_CULL_SHADER_SOURCE);
        let params_span = strides
            .get("CandidateCullParams")
            .copied()
            .expect("candidate_cull.wgsl should declare struct CandidateCullParams");
        assert_eq!(
            params_span as u64,
            crate::candidate_cull::CANDIDATE_PARAMS_SIZE,
            "CandidateCullParams WGSL stride must equal CANDIDATE_PARAMS_SIZE"
        );

        let bytes = crate::candidate_cull::serialize_candidate_params(0x1234_5678);
        assert_eq!(
            bytes.len() as u64,
            crate::candidate_cull::CANDIDATE_PARAMS_SIZE,
            "serialized params must be exactly CANDIDATE_PARAMS_SIZE bytes"
        );
        assert_eq!(
            &bytes[0..4],
            &0x1234_5678u32.to_le_bytes(),
            "candidate_count must occupy the first little-endian word"
        );
        assert_eq!(&bytes[4..], &[0u8; 12], "pad words must be zero");
    }

    /// The baseline estimate is path-independent: it measures the full tree
    /// walk over a `Culled` visible set — exactly the set a candidate-eligible
    /// frame carries — and returns non-default counts. This is the contract the
    /// renderer-driven analysis pass relies on: the baseline panel populates on
    /// candidate frames (where the tree-walk dispatch never runs), not just on
    /// tree-walk frames.
    ///
    /// Regression: baseline showed all zeros on candidate-eligible frames
    /// because the estimate was a side effect of the tree-walk dispatch.
    #[test]
    fn cpu_bvh_estimate_populates_for_candidate_style_visible_set() {
        let tree = BvhTree {
            nodes: vec![
                internal_node(3, [-0.5, -0.5, -0.5], [3.0, 0.5, 0.5]),
                leaf_node_aabb(0, 2, [-0.5, -0.5, -0.5], [0.5, 0.5, 0.5]),
                leaf_node_aabb(1, 3, [2.0, -0.5, -0.5], [3.0, 0.5, 0.5]),
            ],
            leaves: vec![
                leaf_aabb(0, 7, [-0.5, -0.5, -0.5], [0.5, 0.5, 0.5]),
                leaf_aabb(1, 8, [2.0, -0.5, -0.5], [3.0, 0.5, 0.5]),
            ],
            root_node_index: 0,
        };
        let planes = extract_frustum_planes_for_gpu(&Mat4::IDENTITY);
        let mut bucket_scratch = Vec::new();
        // `Culled` with a concrete visible-cell set — the shape a candidate
        // frame carries. The full tree walk still runs in the estimate.
        let diagnostics = estimate_bvh_cull_with_planes(
            &tree.nodes,
            &tree.leaves,
            &tree.derive_bucket_ranges(),
            &crate::visibility::VisibleCells::Culled(vec![7]),
            &planes,
            &mut bucket_scratch,
        );

        assert_ne!(
            diagnostics,
            BvhCullDiagnostics::default(),
            "baseline estimate must be non-default for a candidate-style visible set"
        );
        // Whole tree visited; only the frustum-passing visible leaf submits.
        assert_eq!(diagnostics.estimated_node_visits, 3);
        assert_eq!(diagnostics.submitted_leaves, 1);
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
