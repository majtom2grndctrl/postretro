// Bounded diagnostic reduction of live streamed irradiance tiles.
// See: context/lib/rendering_pipeline.md §Cluster SH residency

struct Request {
    slots: array<u32, 8>,
    weights: array<f32, 8>,
};

struct Params {
    count: u32,
    tiles_per_row: u32,
    tiles_per_layer: u32,
    physical_tile_stride: u32,
    tile_dimension: u32,
    tile_border: u32,
    pad0: u32,
    pad1: u32,
};

@group(0) @binding(0) var atlas: texture_2d_array<f32>;
@group(0) @binding(1) var<storage, read> requests: array<Request>;
@group(0) @binding(2) var<storage, read_write> colors: array<vec4<f32>>;
@group(0) @binding(3) var<uniform> params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.count) {
        return;
    }
    let request = requests[id.x];
    var color = vec3<f32>(0.0);
    let end = params.tile_dimension - params.tile_border;
    let interior = params.tile_dimension - 2u * params.tile_border;
    for (var corner = 0u; corner < 8u; corner++) {
        let weight = request.weights[corner];
        if (weight <= 0.0) {
            continue;
        }
        let slot = request.slots[corner];
        let layer = slot / params.tiles_per_layer;
        let tile = slot % params.tiles_per_layer;
        let origin = vec2<u32>(tile % params.tiles_per_row, tile / params.tiles_per_row)
            * params.physical_tile_stride;
        var sum = vec3<f32>(0.0);
        for (var y = params.tile_border; y < end; y++) {
            for (var x = params.tile_border; x < end; x++) {
                sum += textureLoad(atlas, vec2<i32>(origin + vec2<u32>(x, y)), i32(layer), 0).rgb;
            }
        }
        color += sum * (weight / f32(interior * interior));
    }
    colors[id.x] = vec4<f32>(color, 1.0);
}
