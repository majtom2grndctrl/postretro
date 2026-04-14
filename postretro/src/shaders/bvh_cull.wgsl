// BVH traversal compute shader — flat DFS with skip-index.
//
// One invocation walks the entire tree per frame (`@workgroup_size(1,1,1)`);
// parallelism over subtrees is a deferred optimization that the current
// scene scale does not need. See the Rust-side header in `compute_cull.rs`
// for the full design rationale (traversal strategy, portal integration,
// rejected multi-frustum alternative).

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
