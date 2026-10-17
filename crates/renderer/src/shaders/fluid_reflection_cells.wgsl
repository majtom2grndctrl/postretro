// SPIKE (cell-rate spike, never merged): the Fluid Reflection Cell Pass.
// Appended to the fluid shader's composed source, so each cell runs the
// fragment's own pixel-style reflection functions, once per frame.
// See: context/lib/rendering_pipeline.md §7.3a (Reflection → Style)

// Mirrors render-cpu `reflection_cells::FluidCellFaceGpu`.
struct FluidCellFace {
    origin: vec3<f32>,
    probe: u32,
    axis_u: vec3<f32>,
    cell_min_x: i32,
    axis_v: vec3<f32>,
    cell_min_y: i32,
    outward: vec3<f32>,
    atlas_origin: u32,
    extent: vec2<u32>,
    _pad: vec2<u32>,
};

// Mirrors `FluidCellTileGpu`: the tile's face, its position in tiles
// (x | y << 16), and the face's volume reflectivity this frame.
struct FluidCellTile {
    face: u32,
    tile: u32,
    reflectivity: f32,
    _pad: u32,
};

// Mirrors `FluidCellPassUniform`.
struct FluidCellPass {
    planes: array<vec4<f32>, 6>,
    extent: vec2<f32>,
    tile_count: u32,
    groups_x: u32,
    stats: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

struct FluidCellStats {
    evaluated: atomic<u32>,
};

// Compute-only group 4 entries, past every binding the fluid module declares.
@group(4) @binding(8) var fluid_cell_atlas_store: texture_storage_2d<rgba16float, write>;
@group(4) @binding(9) var<storage, read> fluid_cell_faces: array<FluidCellFace>;
@group(4) @binding(10) var<storage, read> fluid_cell_tiles: array<FluidCellTile>;
@group(4) @binding(11) var<uniform> fluid_cell_pass: FluidCellPass;
@group(4) @binding(12) var<storage, read_write> fluid_cell_stats: FluidCellStats;

// Debug lever: skip cells whose bounding sphere lies outside the frustum.
// Any visible fragment of a cell lies in its square, inside that sphere.
override FLUID_CELLS_FRUSTUM_CULL: bool = false;

const FLUID_CELL_TILE: u32 = 8u;
const FLUID_CELL_FLAT_TEXELS_PER_PIXEL: f32 = 1.0e30;
const FLUID_CELL_JACOBIAN_EPSILON: f32 = 1.0e-8;
const FLUID_CELL_W_EPSILON: f32 = 1.0e-6;

// Mirrors render-cpu `cell_texels_per_pixel`: the fragment's
// max(length(dpdx(uv) * dims), length(dpdy(uv) * dims)), analytically at
// the cell centre from the texel-to-pixel Jacobian's inverse.
fn fluid_cell_texels_per_pixel(centre: vec3<f32>, u: vec3<f32>, v: vec3<f32>) -> f32 {
    let c = uniforms.view_proj * vec4<f32>(centre, 1.0);
    if !(c.w > FLUID_CELL_W_EPSILON) {
        return FLUID_CELL_FLAT_TEXELS_PER_PIXEL;
    }
    let cu = uniforms.view_proj * vec4<f32>(u, 0.0);
    let cv = uniforms.view_proj * vec4<f32>(v, 0.0);
    let scale = fluid_cell_pass.extent * 0.5 / (c.w * c.w);
    let ju = (cu.xy * c.w - c.xy * cu.w) * scale;
    let jv = (cv.xy * c.w - c.xy * cv.w) * scale;
    let det = ju.x * jv.y - jv.x * ju.y;
    if det * det <= FLUID_CELL_JACOBIAN_EPSILON * dot(ju, ju) * dot(jv, jv) {
        return FLUID_CELL_FLAT_TEXELS_PER_PIXEL;
    }
    let per_pixel_x = vec2<f32>(jv.y, -ju.y) / det;
    let per_pixel_y = vec2<f32>(-jv.x, ju.x) / det;
    return max(length(per_pixel_x), length(per_pixel_y));
}

fn fluid_cell_in_frustum(centre: vec3<f32>, radius: f32) -> bool {
    for (var i: u32 = 0u; i < 6u; i = i + 1u) {
        let plane = fluid_cell_pass.planes[i];
        if dot(plane.xyz, centre) + plane.w < -radius {
            return false;
        }
    }
    return true;
}

// One thread per cell of one 8 x 8 tile; threads past the face's rect, and
// workgroups past the tile list, return. Mirrors fs_main's pixel path.
@compute @workgroup_size(8, 8, 1)
fn cs_reflection_cells(@builtin(workgroup_id) group: vec3<u32>,
                       @builtin(local_invocation_id) lane: vec3<u32>) {
    let tile_index = group.y * fluid_cell_pass.groups_x + group.x;
    if tile_index >= fluid_cell_pass.tile_count {
        return;
    }
    let tile = fluid_cell_tiles[tile_index];
    let face = fluid_cell_faces[tile.face];
    let local = vec2<u32>(tile.tile & 0xFFFFu, tile.tile >> 16u) * FLUID_CELL_TILE + lane.xy;
    if any(local >= face.extent) {
        return;
    }
    let centre = face.origin + face.axis_u * (f32(local.x) + 0.5)
        + face.axis_v * (f32(local.y) + 0.5);
    let axes = FluidTexelAxes(face.axis_u, face.axis_v);
    if FLUID_CELLS_FRUSTUM_CULL {
        let radius = 0.5 * max(length(face.axis_u + face.axis_v), length(face.axis_u - face.axis_v));
        if !fluid_cell_in_frustum(centre, radius) {
            return;
        }
    }
    if fluid_cell_pass.stats != 0u {
        atomicAdd(&fluid_cell_stats.evaluated, 1u);
    }
    let texel = vec2<u32>(face.atlas_origin & 0xFFFFu, face.atlas_origin >> 16u) + local;
    let outward = face.outward;
    if dot(uniforms.camera_position - centre, outward) <= 0.0 {
        textureStore(fluid_cell_atlas_store, texel, vec4<f32>(0.0));
        return;
    }
    let cell = vec2<i32>(face.cell_min_x, face.cell_min_y) + vec2<i32>(local);
    let probe = face.probe;
    let texels_per_pixel = fluid_cell_texels_per_pixel(centre, face.axis_u, face.axis_v);
    let offset = fluid_displayed_offset(cell, uniforms.time, texels_per_pixel);
    let point = fluid_reflection_cell_within_box(centre, axes, fluid_probes.probes[probe].box_min,
        fluid_probes.probes[probe].box_max, fluid_probes.probes[probe].capture_point);
    let normal = fluid_reflection_cell_normal(outward, axes, offset);
    let view = normalize(uniforms.camera_position - point);
    let reflection = fluid_reflection_sample(probe, point, view, normal);
    let fresnel = fluid_reflection_fresnel(dot(view, outward), tile.reflectivity)
        * reflection.weight;
    textureStore(fluid_cell_atlas_store, texel, vec4<f32>(reflection.capture, fresnel));
}
