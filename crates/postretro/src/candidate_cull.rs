// GPU candidate cull: gather only visible cells' owned BVH leaves (from the
// baked `CellDrawIndex` CSR) and dispatch one invocation per candidate leaf,
// instead of the legacy whole-BVH tree walk. Writes the SAME global per-leaf
// indirect/status slots as `ComputeCullPipeline`, so the draw path
// (`bucket_ranges` / `draw_indirect_buckets`) is byte-for-byte unchanged.
// See: context/lib/rendering_pipeline.md §7.1
//
// This module is split per development_guide.md §4.1:
//   * `gather_candidate_leaves` — pure, GPU-free data-logic (dedupe visible
//     cell ids, CSR expansion). Unit-tested without a GPU.
//   * `CandidateCullPipeline` — the wgpu dispatch layer.

use glam::Mat4;

use crate::compute_cull::{CullUniforms, extract_frustum_planes_for_gpu, serialize_cull_uniforms};
use crate::prl::CellDrawIndex;

pub(crate) const CANDIDATE_CULL_SHADER_SOURCE: &str = include_str!("shaders/candidate_cull.wgsl");

/// Workgroup size of `candidate_cull.wgsl::candidate_cull_main`. The dispatch
/// rounds the candidate count up to this.
pub(crate) const CANDIDATE_CULL_WORKGROUP_SIZE: u32 = 64;

/// 16-byte params uniform (`CandidateCullParams` in the shader):
/// `candidate_count` plus three pad words to a vec4-aligned uniform.
pub(crate) const CANDIDATE_PARAMS_SIZE: u64 = 16;

/// Serialize the 16-byte `CandidateCullParams` uniform: `candidate_count`
/// little-endian followed by three zero pad words, matching the WGSL struct
/// `{ candidate_count: u32, _pad0: u32, _pad1: u32, _pad2: u32 }`. Extracted
/// from `dispatch` so the CPU/WGSL ABI is covered by a unit test rather than
/// only exercised on a GPU frame.
pub(crate) fn serialize_candidate_params(candidate_count: u32) -> Vec<u8> {
    let mut params = Vec::with_capacity(CANDIDATE_PARAMS_SIZE as usize);
    params.extend_from_slice(&candidate_count.to_le_bytes());
    params.extend_from_slice(&0u32.to_le_bytes());
    params.extend_from_slice(&0u32.to_le_bytes());
    params.extend_from_slice(&0u32.to_le_bytes());
    params
}

/// Outcome of the pure candidate gather.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CandidateGather {
    /// Flat list of global BVH-leaf indices to test this frame. May be empty
    /// (every visible cell owns no drawable leaves) — the caller still clears
    /// the camera buffers, then skips the dispatch.
    Candidates(Vec<u32>),
    /// At least one visible cell id was outside the loaded index
    /// (`>= cell_count`). The caller logs once and falls back to the legacy
    /// tree walk for this frame rather than gathering a partial/corrupt set.
    OutOfRange { cell_id: u32 },
}

/// Pure, GPU-free candidate gather. Expands the visible cells' owned BVH-leaf
/// spans into a flat list of global leaf indices by indexing the `CellDrawIndex`
/// CSR with each id in the visible-cell set.
///
/// Steps:
///   1. Dedupe `visible_cells` preserving first-seen order, so a duplicate cell
///      id cannot produce duplicate writes to the same indirect/status slot.
///   2. For each unique cell `c`, append the leaves of every span in
///      `spans[offset[c]..offset[c+1]]`, expanded to individual leaf indices.
///
/// A visible cell id `>= cell_count` returns [`CandidateGather::OutOfRange`]
/// immediately — the caller must not gather a partial set from a corrupt id.
/// The CSR was cross-validated at load (spans in-bounds, drawable-only,
/// exact-once coverage), so no per-span re-checking happens here.
pub(crate) fn gather_candidate_leaves(
    index: &CellDrawIndex,
    visible_cells: &[u32],
) -> CandidateGather {
    let mut seen = std::collections::HashSet::with_capacity(visible_cells.len());
    let mut out: Vec<u32> = Vec::new();

    for &cell in visible_cells {
        if !seen.insert(cell) {
            continue; // duplicate cell id — first-seen order already handled it.
        }
        if cell >= index.cell_count {
            return CandidateGather::OutOfRange { cell_id: cell };
        }
        let c = cell as usize;
        let start = index.cell_span_offset[c] as usize;
        let end = index.cell_span_offset[c + 1] as usize;
        for span in &index.spans[start..end] {
            let leaf_start = span.leaf_start;
            for k in 0..span.leaf_count {
                out.push(leaf_start + k);
            }
        }
    }

    CandidateGather::Candidates(out)
}

/// GPU-side candidate cull dispatch layer. Owns the candidate buffer, the
/// params uniform, and the pipeline; binds the camera cull's existing leaf,
/// indirect, and status buffers (passed per dispatch) so it writes the SAME
/// global per-leaf slots as the tree walk.
pub struct CandidateCullPipeline {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,

    uniform_buffer: wgpu::Buffer,
    candidate_buffer: wgpu::Buffer,
    params_buffer: wgpu::Buffer,

    total_leaves: u32,
    /// Reused per frame to avoid reallocating the upload staging vector.
    candidate_scratch: Vec<u8>,
}

impl CandidateCullPipeline {
    pub fn new(device: &wgpu::Device, total_leaves: u32) -> Self {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Candidate Cull Uniforms"),
            size: crate::compute_cull::CULL_UNIFORMS_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Worst case: every leaf is a candidate. `.max(1)` keeps a valid
        // (non-zero) storage buffer for empty-geometry edge cases.
        let candidate_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Candidate Leaves"),
            size: (total_leaves.max(1) as u64) * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Candidate Cull Params"),
            size: CANDIDATE_PARAMS_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Candidate Cull Compute Shader"),
            source: wgpu::ShaderSource::Wgsl(CANDIDATE_CULL_SHADER_SOURCE.into()),
        });

        let storage_entry = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let uniform_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Candidate Cull Bind Group Layout"),
            entries: &[
                uniform_entry(0),        // CullUniforms
                storage_entry(1, true),  // leaves (read)
                storage_entry(2, false), // indirect_draws (read_write)
                storage_entry(3, false), // cull_status (read_write)
                storage_entry(4, true),  // candidate_leaves (read)
                uniform_entry(5),        // CandidateCullParams
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Candidate Cull Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Candidate Cull Compute Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("candidate_cull_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        log::info!(
            "[Renderer] Candidate cull pipeline ready: total_leaves={}",
            total_leaves,
        );

        Self {
            pipeline,
            bind_group_layout,
            uniform_buffer,
            candidate_buffer,
            params_buffer,
            total_leaves,
            candidate_scratch: Vec::new(),
        }
    }

    /// Clear the camera `indirect_draws` and `cull_status` buffers over ONLY
    /// the camera world ranges (the first `total_leaves` slots), then — if any
    /// candidates remain — upload them and dispatch one invocation per
    /// candidate. Non-candidate leaves stay cleared to zero.
    ///
    /// `indirect_buffer`, `cull_status_buffer`, and `leaf_buffer` are the
    /// camera cull's existing global buffers, threaded in so the candidate path
    /// writes the same per-leaf slots. Clearing only `total_leaves * stride`
    /// bytes leaves any future shadow/entity/packed non-camera regions of a
    /// shared buffer untouched.
    #[allow(clippy::too_many_arguments)]
    pub fn dispatch(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        leaf_buffer: &wgpu::Buffer,
        indirect_buffer: &wgpu::Buffer,
        cull_status_buffer: &wgpu::Buffer,
        candidate_leaves: &[u32],
        view_proj: &Mat4,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        // Clear ONLY the camera world ranges. The candidate path writes only
        // submitted candidate slots, so the indirect buffer must start zeroed
        // (the tree walk instead explicitly writes index_count=0 for rejects).
        let indirect_clear_bytes =
            self.total_leaves as u64 * crate::compute_cull::DRAW_INDIRECT_SIZE;
        let status_clear_bytes = self.total_leaves as u64 * 4;
        if self.total_leaves > 0 {
            encoder.clear_buffer(indirect_buffer, 0, Some(indirect_clear_bytes));
            encoder.clear_buffer(cull_status_buffer, 0, Some(status_clear_bytes));
        }

        let planes = extract_frustum_planes_for_gpu(view_proj);
        let uniforms = CullUniforms { planes };
        queue.write_buffer(&self.uniform_buffer, 0, &serialize_cull_uniforms(&uniforms));

        let candidate_count = candidate_leaves.len() as u32;
        let params = serialize_candidate_params(candidate_count);
        queue.write_buffer(&self.params_buffer, 0, &params);

        // candidate_count == 0: buffers are already cleared; skip the dispatch.
        if candidate_count == 0 || self.total_leaves == 0 {
            return;
        }

        self.candidate_scratch.clear();
        self.candidate_scratch.reserve(candidate_leaves.len() * 4);
        for &leaf in candidate_leaves {
            self.candidate_scratch
                .extend_from_slice(&leaf.to_le_bytes());
        }
        queue.write_buffer(&self.candidate_buffer, 0, &self.candidate_scratch);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Candidate Cull Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: leaf_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: indirect_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: cull_status_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.candidate_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: self.params_buffer.as_entire_binding(),
                },
            ],
        });

        let workgroups = candidate_count.div_ceil(CANDIDATE_CULL_WORKGROUP_SIZE);

        let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Candidate Cull Pass"),
            timestamp_writes,
        });
        compute_pass.set_pipeline(&self.pipeline);
        compute_pass.set_bind_group(0, &bind_group, &[]);
        compute_pass.dispatch_workgroups(workgroups, 1, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::cell_draw_index::{CellDrawIndexSection, Span};

    fn index_from(cell_span_offset: Vec<u32>, spans: Vec<Span>) -> CellDrawIndex {
        CellDrawIndexSection {
            cell_count: (cell_span_offset.len() - 1) as u32,
            span_count: spans.len() as u32,
            cell_span_offset,
            spans,
        }
    }

    /// Smoke test: dedupe of visible cell ids (first-seen order) plus CSR span
    /// expansion to a flat global-leaf list. Task 6 extends equivalence and
    /// diagnostics coverage.
    #[test]
    fn gather_dedupes_and_expands_csr() {
        // 3 cells. cell 0 -> spans[0..2], cell 1 -> [] (empty row),
        // cell 2 -> spans[2..3].
        // span 0: leaves 0,1,2  span 1: leaf 10  span 2: leaves 20,21
        let index = index_from(
            vec![0, 2, 2, 3],
            vec![
                Span {
                    leaf_start: 0,
                    leaf_count: 3,
                },
                Span {
                    leaf_start: 10,
                    leaf_count: 1,
                },
                Span {
                    leaf_start: 20,
                    leaf_count: 2,
                },
            ],
        );

        // Visible cells with a duplicate (2 appears twice) — must not double-write.
        let gather = gather_candidate_leaves(&index, &[2, 0, 2]);
        match gather {
            CandidateGather::Candidates(leaves) => {
                // First-seen order: cell 2 first (20,21), then cell 0 (0,1,2,10).
                assert_eq!(leaves, vec![20, 21, 0, 1, 2, 10]);
            }
            other => panic!("expected Candidates, got {other:?}"),
        }
    }

    #[test]
    fn gather_empty_cell_yields_no_leaves() {
        let index = index_from(
            vec![0, 1, 1],
            vec![Span {
                leaf_start: 5,
                leaf_count: 2,
            }],
        );
        // Cell 1 owns no leaves.
        let gather = gather_candidate_leaves(&index, &[1]);
        assert_eq!(gather, CandidateGather::Candidates(vec![]));
    }

    /// The candidate shader must be valid WGSL (a malformed copy would fail
    /// only at GPU pipeline creation, which the GPU-free test suite never hits).
    #[test]
    fn candidate_shader_parses_as_wgsl() {
        naga::front::wgsl::parse_str(CANDIDATE_CULL_SHADER_SOURCE)
            .expect("candidate cull shader should parse as WGSL");
    }

    #[test]
    fn gather_out_of_range_cell_signals_fallback() {
        let index = index_from(
            vec![0, 1],
            vec![Span {
                leaf_start: 0,
                leaf_count: 1,
            }],
        );
        // cell_count == 1, so id 1 is out of range.
        let gather = gather_candidate_leaves(&index, &[0, 1]);
        assert_eq!(gather, CandidateGather::OutOfRange { cell_id: 1 });
    }

    /// The gather visits ONLY the visible cells' spans: its output equals the
    /// union of those cells' CSR spans and nothing else (work ∝ candidate
    /// count, not total leaves/nodes). Cell 1's leaves and the non-visible
    /// cell 3's leaves must never appear when only cells 0 and 2 are visible.
    #[test]
    fn gather_visits_only_visible_cells_spans() {
        // 4 cells. cell 0 -> [span0], cell 1 -> [span1], cell 2 -> [span2],
        // cell 3 -> [span3]. Each span owns a disjoint global-leaf range.
        let index = index_from(
            vec![0, 1, 2, 3, 4],
            vec![
                Span {
                    leaf_start: 0,
                    leaf_count: 2,
                }, // cell 0: leaves 0,1
                Span {
                    leaf_start: 2,
                    leaf_count: 3,
                }, // cell 1: leaves 2,3,4
                Span {
                    leaf_start: 5,
                    leaf_count: 1,
                }, // cell 2: leaf 5
                Span {
                    leaf_start: 6,
                    leaf_count: 2,
                }, // cell 3: leaves 6,7
            ],
        );

        let gather = gather_candidate_leaves(&index, &[2, 0]);
        let leaves = match gather {
            CandidateGather::Candidates(leaves) => leaves,
            other => panic!("expected Candidates, got {other:?}"),
        };

        // Exactly the union of cells 2 and 0 spans (first-seen order: 2 then 0).
        assert_eq!(leaves, vec![5, 0, 1]);

        // Nothing from the non-visible cells leaked in.
        for hidden in [2u32, 3, 4, 6, 7] {
            assert!(
                !leaves.contains(&hidden),
                "leaf {hidden} from a non-visible cell must not be gathered"
            );
        }
        // Candidate count equals the visible cells' leaf total, not the global
        // leaf count (8): the gather did no work proportional to total leaves.
        assert_eq!(leaves.len(), 3);
    }
}
