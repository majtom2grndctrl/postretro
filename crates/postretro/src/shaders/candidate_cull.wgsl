// Candidate cull compute shader — one invocation per candidate BVH leaf.
//
// The CPU expands the visible cells' owned BVH-leaf spans (from the baked
// `CellDrawIndex` CSR) into a flat `candidate_leaves` buffer of global leaf
// indices, then dispatches `ceil(candidate_count / 64)` workgroups of 64.
// Each invocation frustum-tests one candidate leaf and writes that leaf's
// existing global indirect/status slot. Non-candidate leaves stay cleared to
// zero by the CPU-side `clear_buffer` over the camera world ranges. This path
// replaces the legacy whole-BVH tree walk (`bvh_cull.wgsl::cull_main`) on
// portal-visible camera frames; it reads leaves, frustum planes, and the
// candidate buffer only — never BVH nodes or skip_index.
//
// The struct/helper definitions below are copied BYTE-FOR-BYTE from
// `bvh_cull.wgsl` (WGSL has no include). A shader-validation test (Task 6)
// asserts that equivalence — keep this text identical.

struct FrustumPlane {
    // .xyz = normal, .w = dist
    data: vec4<f32>,
};

struct CullUniforms {
    planes: array<FrustumPlane, 6>,
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
    chunk_range_start: u32,    // offset 40
    chunk_range_count: u32,    // offset 44
};

struct DrawIndexedIndirect {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
};

// 16-byte params: candidate_count plus padding to a vec4-aligned uniform.
struct CandidateCullParams {
    candidate_count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0) var<uniform> uniforms: CullUniforms;
@group(0) @binding(1) var<storage, read> leaves: array<BvhLeaf>;
@group(0) @binding(2) var<storage, read_write> indirect_draws: array<DrawIndexedIndirect>;
// Per-leaf cull status for the debug wireframe overlay.
// 0 = portal-culled (non-candidate, left cleared),
// 1 = frustum-culled,
// 2 = visible/rendered.
@group(0) @binding(3) var<storage, read_write> cull_status: array<u32>;
@group(0) @binding(4) var<storage, read> candidate_leaves: array<u32>;
@group(0) @binding(5) var<uniform> params: CandidateCullParams;

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

@compute @workgroup_size(64, 1, 1)
fn candidate_cull_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.candidate_count {
        return;
    }

    let leaf_idx = candidate_leaves[gid.x];
    let leaf = leaves[leaf_idx];
    let leaf_min = vec3<f32>(leaf.min_x, leaf.min_y, leaf.min_z);
    let leaf_max = vec3<f32>(leaf.max_x, leaf.max_y, leaf.max_z);

    if is_aabb_outside_frustum(leaf_min, leaf_max) {
        // Frustum-rejected: leave the (already-cleared) indirect slot at zero.
        cull_status[leaf_idx] = 1u;
    } else {
        indirect_draws[leaf_idx].index_count = leaf.index_count;
        indirect_draws[leaf_idx].instance_count = 1u;
        indirect_draws[leaf_idx].first_index = leaf.index_offset;
        indirect_draws[leaf_idx].base_vertex = 0;
        indirect_draws[leaf_idx].first_instance = 0u;
        cull_status[leaf_idx] = 2u;
    }
}
