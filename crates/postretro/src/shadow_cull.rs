// Per-slot GPU cone cull for the spot-shadow depth passes.
// See: context/lib/rendering_pipeline.md §7.1 (step 6) · §4 (spot shadows)

use glam::Mat4;
use wgpu::util::DeviceExt;

use crate::compute_cull::{
    CULL_SHADER_SOURCE, CULL_UNIFORMS_SIZE, CullUniforms, DRAW_INDIRECT_SIZE, SetTextureFn,
    VISIBLE_CELLS_WORDS, draw_indirect_buckets, serialize_cull_uniforms,
};
use crate::geometry::BucketRange;
use crate::lighting::cone_frustum::extract_frustum_planes_for_gpu;
use crate::lighting::spot_shadow::SHADOW_POOL_SIZE;

/// Persistent, renderer-owned per-slot cone cull for the spot-shadow pool —
/// sibling to the camera `ComputeCullPipeline`. Built at init/level-load and
/// shares the read-only BVH node/leaf storage buffers uploaded once by the
/// camera cull (no per-frame BVH re-serialization). Each occupied shadow slot
/// dispatches the same `bvh_cull.wgsl` traversal into its own indirect
/// sub-region, gated by the slot's cone frustum planes only (the shared
/// visible-cells buffer is all-ones, neutralizing the `cell_is_visible` AND).
///
/// WHY cone-only (no camera-PVS AND): an occluder outside the camera PVS can
/// still cast a shadow onto a receiver the camera sees. ANDing the cone test
/// with the camera's visible cells would drop such valid shadows. Cone-only is
/// a strict superset of the geometry needed, and a strict subset of today's
/// "draw all world geometry per slot", so it can never drop a shadow.
pub struct ShadowCullPipeline {
    pipeline: wgpu::ComputePipeline,

    /// Per-slot frustum-planes uniform. One buffer per slot, no dynamic offset,
    /// no alignment padding — the per-slot bind group binds its own uniform.
    slot_uniform_buffers: Vec<wgpu::Buffer>,
    /// Per-slot bind group, mirroring the camera cull's uniform bind group
    /// layout. Shares the BVH node/leaf + all-ones visible-cells buffers; only
    /// the uniform (binding 0) and the indirect sub-region (binding 4) differ.
    slot_bind_groups: Vec<wgpu::BindGroup>,

    /// ONE indirect buffer carved into `SHADOW_POOL_SIZE` sub-regions by offset:
    /// each sub-region holds the full per-leaf layout (`total_leaves` slots ×
    /// 20 bytes), matching the camera path's per-leaf layout and BVH leaf
    /// ordering exactly. Sub-regions are spaced by `region_stride_bytes`, not
    /// the raw per-leaf size, so each region base is a valid storage-binding
    /// offset.
    indirect_buffer: wgpu::Buffer,
    /// 256-byte-aligned stride between adjacent slot sub-regions in
    /// `indirect_buffer`. Used as the per-slot offset for both the binding-4
    /// region base (where the slot's cull dispatch writes) and the slot's
    /// indirect draw region (where the depth pass reads) — they must agree.
    region_stride_bytes: u64,
    total_leaves: u32,
    bucket_ranges: Vec<BucketRange>,
    has_multi_draw_indirect: bool,
}

impl ShadowCullPipeline {
    /// Build the shadow cull owner, sharing the camera cull's read-only BVH
    /// node/leaf storage buffers. Call this wherever the camera
    /// `ComputeCullPipeline` is (re)built so the bind groups reference the
    /// freshly-uploaded BVH buffers — if the level reloads and rebuilds the
    /// camera cull, this must be rebuilt too.
    pub fn new(
        device: &wgpu::Device,
        node_buffer: &wgpu::Buffer,
        leaf_buffer: &wgpu::Buffer,
        total_leaves: u32,
        bucket_ranges: Vec<BucketRange>,
        has_multi_draw_indirect: bool,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shadow Cull Compute Shader"),
            source: wgpu::ShaderSource::Wgsl(CULL_SHADER_SOURCE.into()),
        });

        // Mirror the camera cull's bind group layout exactly: the shader is the
        // same module, so the binding types must match `compute_cull.rs`.
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shadow Cull Bind Group Layout"),
            entries: &[
                storage_or_uniform_entry(0, wgpu::BufferBindingType::Uniform),
                storage_or_uniform_entry(1, wgpu::BufferBindingType::Storage { read_only: true }),
                storage_or_uniform_entry(2, wgpu::BufferBindingType::Storage { read_only: true }),
                storage_or_uniform_entry(3, wgpu::BufferBindingType::Storage { read_only: true }),
                storage_or_uniform_entry(4, wgpu::BufferBindingType::Storage { read_only: false }),
                storage_or_uniform_entry(5, wgpu::BufferBindingType::Storage { read_only: false }),
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Shadow Cull Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Shadow Cull Compute Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("cull_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        // One shared all-ones visible-cells buffer (cone-only gate). Initialized
        // via the same `0xFFFFFFFF`-per-word scheme `write_bitmask_draw_all`
        // uses, so the `cell_is_visible` AND in the WGSL always passes.
        let all_ones: Vec<u8> = (0..VISIBLE_CELLS_WORDS)
            .flat_map(|_| 0xFFFFFFFFu32.to_le_bytes())
            .collect();
        let visible_cells_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Shadow Cull Visible Cells (all-ones)"),
            contents: &all_ones,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // ONE indirect buffer, `SHADOW_POOL_SIZE` sub-regions of `total_leaves`
        // slots each. Each sub-region holds the full per-leaf layout.
        //
        // Each sub-region is bound as a STORAGE buffer (binding 4) at its base
        // offset, so that offset must satisfy `min_storage_buffer_offset_alignment`
        // (256 by default; this build does not override device limits). The raw
        // per-region size `total_leaves * 20` is only a multiple of 256 when
        // `total_leaves` is a multiple of 64, so we pad the stride up to 256.
        // (The per-slot frustum *uniform* buffers are separate allocations and
        // need no such padding — only this shared buffer is sub-divided by offset.)
        let region_slots = total_leaves.max(1) as u64;
        let region_bytes = region_slots * DRAW_INDIRECT_SIZE;
        let region_stride_bytes = region_bytes.next_multiple_of(256);
        // Pool sizing: SHADOW_POOL_SIZE (64) slots × the aligned per-slot region.
        // A future reader sizing large community maps should expect this 64×
        // multiplier on top of the padded per-region footprint.
        let indirect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shadow Cull Indirect Buffer"),
            size: SHADOW_POOL_SIZE as u64 * region_stride_bytes,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Shared cull-status scratch (binding 5). The shadow path has no debug
        // wireframe overlay, but the layout requires the binding; every slot's
        // dispatch overwrites it, so one shared buffer suffices.
        let cull_status_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shadow Cull Status Scratch"),
            size: region_slots * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut slot_uniform_buffers = Vec::with_capacity(SHADOW_POOL_SIZE);
        let mut slot_bind_groups = Vec::with_capacity(SHADOW_POOL_SIZE);
        for slot in 0..SHADOW_POOL_SIZE {
            let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Shadow Cull Uniforms"),
                size: CULL_UNIFORMS_SIZE as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            // Region base for this slot. The cull dispatch writes leaf indirect
            // slots relative to this binding-4 offset, and the depth draw reads
            // from the same base via `region_stride_bytes` — they must match.
            let region_offset = slot as u64 * region_stride_bytes;
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Shadow Cull Bind Group"),
                layout: &bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: node_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: leaf_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: visible_cells_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &indirect_buffer,
                            offset: region_offset,
                            size: std::num::NonZeroU64::new(region_bytes),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: cull_status_buffer.as_entire_binding(),
                    },
                ],
            });

            slot_uniform_buffers.push(uniform_buffer);
            slot_bind_groups.push(bind_group);
        }

        log::info!(
            "[Renderer] Shadow cull pipeline ready: {} leaves, {} slots, multi_draw={}",
            total_leaves,
            SHADOW_POOL_SIZE,
            has_multi_draw_indirect,
        );

        Self {
            pipeline,
            slot_uniform_buffers,
            slot_bind_groups,
            indirect_buffer,
            region_stride_bytes,
            total_leaves,
            bucket_ranges,
            has_multi_draw_indirect,
        }
    }

    /// Run one compute pass that loops the occupied slots, dispatching the BVH
    /// traversal into each slot's indirect sub-region with that slot's cone
    /// frustum planes. `slot_matrices[slot]` is the slot's light-space matrix
    /// (the single source of truth from `update_dynamic_light_slots`); `None`
    /// slots are skipped. Runs after the camera BVH cull and before the
    /// spot-shadow depth render passes.
    pub fn dispatch_occupied_slots(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        slot_matrices: &[Option<Mat4>; SHADOW_POOL_SIZE],
    ) {
        if self.total_leaves == 0 {
            return;
        }

        // Write each occupied slot's cone planes into its own uniform first,
        // outside the compute-pass scope (queue writes are ordered before the
        // submitted commands).
        let mut occupied: Vec<usize> = Vec::new();
        for (slot, matrix) in slot_matrices.iter().enumerate() {
            let Some(matrix) = matrix else { continue };
            let planes = extract_frustum_planes_for_gpu(matrix);
            let uniforms = CullUniforms { planes };
            queue.write_buffer(
                &self.slot_uniform_buffers[slot],
                0,
                &serialize_cull_uniforms(&uniforms),
            );
            occupied.push(slot);
        }

        if occupied.is_empty() {
            return;
        }

        let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Shadow Cull Pass"),
            timestamp_writes: None,
        });
        compute_pass.set_pipeline(&self.pipeline);
        for slot in occupied {
            compute_pass.set_bind_group(0, &self.slot_bind_groups[slot], &[]);
            compute_pass.dispatch_workgroups(1, 1, 1);
        }
    }

    /// Issue the per-slot indirect depth draw from `slot`'s indirect
    /// sub-region. `set_texture_fn = None` for the depth-only shadow pipeline
    /// (no group-1 material slot), matching the depth pre-pass.
    pub fn draw_slot_indirect<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        slot: u32,
        set_texture_fn: Option<&SetTextureFn<'a>>,
    ) {
        // Read from the SAME region base the slot's cull dispatch wrote to:
        // both derive their offset from `region_stride_bytes` (256-aligned), so
        // the binding offset, dispatch output region, and draw region agree.
        //
        // Stale/unwritten indirect slots are safe: leaves outside this slot's
        // cone leave their indirect slots unwritten, but those same leaves clip
        // out against the slot's light-space projection and contribute no depth.
        // First-frame/reload safety comes from wgpu zero-initializing the buffer
        // — the same property that makes the camera draw path correct.
        let region_byte_offset = slot as u64 * self.region_stride_bytes;
        draw_indirect_buckets(
            render_pass,
            &self.indirect_buffer,
            region_byte_offset,
            &self.bucket_ranges,
            self.has_multi_draw_indirect,
            set_texture_fn,
        );
    }
}

fn storage_or_uniform_entry(
    binding: u32,
    ty: wgpu::BufferBindingType,
) -> wgpu::BindGroupLayoutEntry {
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

#[cfg(test)]
mod tests {
    use crate::geometry::{BVH_NODE_FLAG_LEAF, BvhLeaf, BvhNode, BvhTree};
    use crate::lighting::cone_frustum::{Aabb, aabb_intersects_frustum, cone_frustum_planes};
    use crate::lighting::spot_shadow::light_space_matrix;
    use crate::prl::{FalloffModel, LightType, MapLight};
    use glam::Vec3;

    fn leaf_at(min: [f32; 3], max: [f32; 3], index_count: u32) -> BvhLeaf {
        BvhLeaf {
            aabb_min: min,
            material_bucket_id: 0,
            aabb_max: max,
            index_offset: 0,
            index_count,
            cell_id: 0,
            chunk_range_start: 0,
            chunk_range_count: 0,
        }
    }

    fn leaf_node(leaf_index: u32, aabb_min: [f32; 3], aabb_max: [f32; 3]) -> BvhNode {
        BvhNode {
            aabb_min,
            skip_index: leaf_index + 1,
            aabb_max,
            left_child_or_leaf_index: leaf_index,
            flags: BVH_NODE_FLAG_LEAF,
        }
    }

    /// Spotlight at the origin aimed down -Z, 20m range. Cone covers a small
    /// region near the -Z axis.
    fn spot_down_neg_z() -> MapLight {
        MapLight {
            origin: [0.0, 0.0, 0.0],
            light_type: LightType::Spot,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 20.0,
            cone_angle_inner: 0.2,
            cone_angle_outer: 0.3,
            cone_direction: [0.0, 0.0, -1.0],
            is_dynamic: true,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![],
            cell_index: 0,
            shadow_type: crate::prl::ShadowType::StaticLightMap,
        }
    }

    /// CPU replay of the GPU per-slot cull predicate: sum the `index_count` of
    /// the BVH leaves whose AABB passes the slot's cone frustum planes. This
    /// mirrors the WGSL convention (`is_aabb_outside_frustum`) via the shared
    /// `aabb_intersects_frustum`, so it predicts what the GPU dispatch submits.
    fn submitted_index_count_for_cone(tree: &BvhTree, light: &MapLight) -> u32 {
        let planes = cone_frustum_planes(&light_space_matrix(light));
        let mut sum = 0u32;
        for leaf in &tree.leaves {
            let aabb = Aabb {
                min: Vec3::from(leaf.aabb_min),
                max: Vec3::from(leaf.aabb_max),
            };
            if aabb_intersects_frustum(&aabb, &planes) {
                sum += leaf.index_count;
            }
        }
        sum
    }

    /// AC#1: on a scene where the spotlight's cone covers a small fraction of
    /// the level, the summed submitted index count for that slot is strictly
    /// LESS than the full scene index count. Deterministic CPU replay of the
    /// same leaf-AABB-vs-cone-frustum predicate the GPU dispatch uses — no GPU
    /// readback.
    #[test]
    fn cone_cull_submits_fewer_indices_than_full_scene() {
        // Three leaves: one inside the cone (down -Z), two outside (behind the
        // light, and far off to the side).
        let leaves = vec![
            // Inside: on-axis, ~10m down -Z.
            leaf_at([-0.5, -0.5, -10.5], [0.5, 0.5, -9.5], 30),
            // Behind the light (positive Z) — cannot be in a cone aimed at -Z.
            leaf_at([-0.5, -0.5, 9.5], [0.5, 0.5, 10.5], 60),
            // Far off to the side, beyond the cone's angular spread.
            leaf_at([49.5, -0.5, -10.5], [50.5, 0.5, -9.5], 90),
        ];
        let nodes = vec![
            leaf_node(0, leaves[0].aabb_min, leaves[0].aabb_max),
            leaf_node(1, leaves[1].aabb_min, leaves[1].aabb_max),
            leaf_node(2, leaves[2].aabb_min, leaves[2].aabb_max),
        ];
        let tree = BvhTree {
            nodes,
            leaves,
            root_node_index: 0,
        };

        let full_scene: u32 = tree.leaves.iter().map(|l| l.index_count).sum();
        let submitted = submitted_index_count_for_cone(&tree, &spot_down_neg_z());

        assert!(
            submitted < full_scene,
            "cone cull should submit fewer indices ({submitted}) than the full scene ({full_scene})"
        );
        // Only the on-axis leaf is inside the cone.
        assert_eq!(submitted, 30, "only the in-cone leaf should be submitted");
    }
}
