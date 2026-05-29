// SH compose compute pass.
//
// Per-frame compose of the static base SH irradiance volume plus per-light
// animated deltas into a parallel set of "total" SH band textures. SH
// consumers (forward, billboard, fog) sample the total textures, so any
// animated-delta data this pass writes is automatically picked up without
// consumer-side branching.
//
// Algorithm (sparse CSR / affinity-cell form): one thread per base probe.
// The dispatch is `@workgroup_size(4,4,4)` over the base SH grid, so the
// workgroup grid equals `affinity_dims = ceil(base_dims/4)` and the
// `workgroup_id` IS this thread's affinity-cell coordinate. The affinity
// cell's CSR range in `affinity_offsets` names exactly the animated lights
// that touch this 4×4×4 block of probes; for each such light we read its
// pre-baked delta sub-block at the probe slot coincident with this thread
// (a direct 1:1 point read — no trilinear interpolation, no AABB test),
// evaluate the Catmull-Rom animation curve, and accumulate
// `delta × brightness × color`. The accumulated delta is added to the base
// bands at full weight (the `delta_scale` dev knob was retired alongside the
// indirect-only delta — the delta now carries bounce only, so there is no
// double-count to bisect away).
//
// Sparse-CSR invariants (validated at bake/load, Task 3) replace the old
// declared-vs-written probe-count mismatch bug class entirely:
//   • `affinity_offsets.len() == affinity_cell_count + 1`
//   • every `affinity_lights[i] < animated_light_count`
// so the in-shader loop needs no bounds/out-of-range path. An affinity cell
// with no animated lights has `start == end` and the loop runs zero times.
//
// Curve helpers (`sample_curve_catmull_rom`, `sample_color_catmull_rom`)
// come from `curve_eval.wgsl`, concatenated after this source at
// pipeline-build time. Both helpers read `anim_samples` by lexical name.

struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    ambient_floor: f32,
    light_count: u32,
    time: f32,
    lighting_isolation: u32,
    _pad: u32,
};

// Same 48-byte layout as `forward.wgsl` and `animated_lightmap_compose.wgsl`.
// The buffer is shared via `AnimatedLightBuffers` so the field order must
// match exactly.
struct AnimationDescriptor {
    period: f32,
    phase: f32,
    brightness_offset: u32,
    brightness_count: u32,
    base_color: vec3<f32>,
    color_offset: u32,
    color_count: u32,
    is_active: u32,
    direction_offset: u32,
    direction_count: u32,
};

struct GridDims {
    dims: vec3<u32>,
    // 4th field is padding. It once held `delta_light_count`, then the
    // `delta_scale` knob; both are retired. Kept so the uniform size and the
    // std140 vec4 row stay unchanged.
    _pad: f32,
};

struct GridFrame {
    grid_origin: vec3<f32>,
    _pad0: f32,
    cell_size: vec3<f32>,
    _pad1: f32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

@group(1) @binding(0) var sh_base_band0: texture_3d<f32>;
@group(1) @binding(1) var sh_base_band1: texture_3d<f32>;
@group(1) @binding(2) var sh_base_band2: texture_3d<f32>;
@group(1) @binding(3) var sh_base_band3: texture_3d<f32>;
@group(1) @binding(4) var sh_base_band4: texture_3d<f32>;
@group(1) @binding(5) var sh_base_band5: texture_3d<f32>;
@group(1) @binding(6) var sh_base_band6: texture_3d<f32>;
@group(1) @binding(7) var sh_base_band7: texture_3d<f32>;
@group(1) @binding(8) var sh_base_band8: texture_3d<f32>;

@group(1) @binding(9)  var sh_total_band0: texture_storage_3d<rgba16float, write>;
@group(1) @binding(10) var sh_total_band1: texture_storage_3d<rgba16float, write>;
@group(1) @binding(11) var sh_total_band2: texture_storage_3d<rgba16float, write>;
@group(1) @binding(12) var sh_total_band3: texture_storage_3d<rgba16float, write>;
@group(1) @binding(13) var sh_total_band4: texture_storage_3d<rgba16float, write>;
@group(1) @binding(14) var sh_total_band5: texture_storage_3d<rgba16float, write>;
@group(1) @binding(15) var sh_total_band6: texture_storage_3d<rgba16float, write>;
@group(1) @binding(16) var sh_total_band7: texture_storage_3d<rgba16float, write>;
@group(1) @binding(17) var sh_total_band8: texture_storage_3d<rgba16float, write>;

@group(1) @binding(18) var<uniform> grid: GridDims;
@group(1) @binding(19) var<uniform> grid_frame: GridFrame;
// Sparse delta payload: one stride-28-half sub-block (64 probes) per CSR
// entry, f16 coeffs packed two-per-`u32` as raw bits; `unpack2x16float`
// returns `(low, high)` matching the bake's even/odd coeff order.
@group(1) @binding(20) var<storage, read> delta_subblocks: array<u32>;
// CSR offsets into `affinity_lights`, indexed by affinity-cell linear index;
// length is `affinity_cell_count + 1` (trailing total).
@group(1) @binding(21) var<storage, read> affinity_offsets: array<u32>;
@group(1) @binding(22) var<storage, read> descriptors: array<AnimationDescriptor>;
@group(1) @binding(23) var<storage, read> anim_samples: array<f32>;
// Flat CSR light indices, index-parallel to the delta sub-blocks: CSR entry
// `i` (light `affinity_lights[i]`) owns sub-block `i`.
@group(1) @binding(24) var<storage, read> affinity_lights: array<u32>;

// f16 halves per probe in `delta_subblocks` (27 logical coeffs + 1 zero pad);
// the trailing half is discarded. Matches `PROBE_F16_STRIDE` in the bake.
const PROBE_F16_STRIDE: u32 = 28u;
// Probes per affinity cell (4×4×4). Matches `PROBES_PER_CELL` in the bake.
const PROBES_PER_CELL: u32 = 64u;

// Read the 27 SH coefficients of one probe slot from the f16 sub-block payload.
// `entry` is the CSR entry / sub-block index; `local` is the in-cell probe
// index (x-fastest `lx + ly*4 + lz*16`), coincident 1:1 with this thread's
// base probe. Coeffs are packed two-per-`u32`: coeff `2k` is the low half,
// coeff `2k+1` the high half. 14 `u32` reads cover all 28 halves; the final
// high half (the zero pad) is dropped.
fn read_delta_subblock(entry: u32, local: u32) -> array<vec3<f32>, 9> {
    // Half-offset of this probe slot; `delta_subblocks` is `u32`, so divide by 2.
    let half_base = (entry * PROBES_PER_CELL + local) * PROBE_F16_STRIDE;
    let word_base = half_base / 2u;

    // Unpack the 27 coeffs into a flat scratch array, then pack into RGB bands.
    var coeffs: array<f32, 27>;
    for (var w: u32 = 0u; w < 14u; w = w + 1u) {
        let pair = unpack2x16float(delta_subblocks[word_base + w]);
        let lo = w * 2u;
        if (lo < 27u) {
            coeffs[lo] = pair.x;
        }
        let hi = lo + 1u;
        if (hi < 27u) {
            coeffs[hi] = pair.y;
        }
    }

    var bands: array<vec3<f32>, 9>;
    for (var b: u32 = 0u; b < 9u; b = b + 1u) {
        let o = b * 3u;
        bands[b] = vec3<f32>(coeffs[o], coeffs[o + 1u], coeffs[o + 2u]);
    }
    return bands;
}

@compute @workgroup_size(4, 4, 4)
fn compose_main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_index) local: u32,
) {
    if (gid.x >= grid.dims.x || gid.y >= grid.dims.y || gid.z >= grid.dims.z) {
        return;
    }
    let p = vec3<i32>(i32(gid.x), i32(gid.y), i32(gid.z));

    // Load base SH bands at this probe.
    var bands: array<vec3<f32>, 9>;
    let t0 = textureLoad(sh_base_band0, p, 0);
    bands[0] = t0.rgb;
    // Baked per-probe validity rides in base band-0 alpha (0 = in-wall/off-grid,
    // 1 = valid). Carry it through to total band-0 alpha unchanged: delta
    // accumulation below is rgb-only, so validity never gets polluted. Consumers
    // sample the total textures, so this is the only path the bit can travel.
    let base_validity = t0.a;
    bands[1] = textureLoad(sh_base_band1, p, 0).rgb;
    bands[2] = textureLoad(sh_base_band2, p, 0).rgb;
    bands[3] = textureLoad(sh_base_band3, p, 0).rgb;
    bands[4] = textureLoad(sh_base_band4, p, 0).rgb;
    bands[5] = textureLoad(sh_base_band5, p, 0).rgb;
    bands[6] = textureLoad(sh_base_band6, p, 0).rgb;
    bands[7] = textureLoad(sh_base_band7, p, 0).rgb;
    bands[8] = textureLoad(sh_base_band8, p, 0).rgb;

    // This thread's affinity-cell coordinate IS the workgroup id: the dispatch
    // is `@workgroup_size(4,4,4)` over the base grid, so the workgroup grid
    // equals `affinity_dims = ceil(base_dims/4)`. Linearize x-fastest to index
    // the CSR offsets — do NOT recompute the cell from the base probe index.
    let affinity_dims = (grid.dims + vec3<u32>(3u)) / vec3<u32>(4u);
    let cell = wg.x + wg.y * affinity_dims.x + wg.z * affinity_dims.x * affinity_dims.y;

    // Accumulate this affinity cell's animated-delta contributions. The CSR
    // range names exactly the lights that touch this 4×4×4 block; `start == end`
    // (empty cell / no animated lights) runs zero passes. The delta is
    // indirect-only (bounce), so it is added to the base at full weight below.
    var delta_sum: array<vec3<f32>, 9>;
    for (var b: u32 = 0u; b < 9u; b = b + 1u) {
        delta_sum[b] = vec3<f32>(0.0);
    }

    let start = affinity_offsets[cell];
    let end = affinity_offsets[cell + 1u];
    for (var i: u32 = start; i < end; i = i + 1u) {
        let light = affinity_lights[i];
        let desc = descriptors[light];
        if (desc.is_active == 0u) {
            continue;
        }

        // Evaluate the animation curve at the current time. `period` may
        // be 0 for static-by-construction descriptors; clamp to avoid
        // divide-by-zero. The animated-lightmap pass uses the same guard.
        let t = fract(uniforms.time / max(desc.period, 1.0e-6) + desc.phase);
        // Clamp intensity/tint non-negative: Catmull-Rom overshoots between
        // keyframes and can dip below zero, which would flip the sign of a delta
        // light's contribution and produce flickering wrong-colored irradiance
        // where multiple differently-colored animated lights overlap.
        let brightness = max(
            sample_curve_catmull_rom(desc.brightness_offset, desc.brightness_count, t),
            0.0,
        );
        let color = max(
            sample_color_catmull_rom(
                desc.color_offset,
                desc.color_count,
                t,
                desc.base_color,
            ),
            vec3<f32>(0.0),
        );

        // Direct point read: CSR entry `i`'s sub-block, this thread's in-cell
        // probe slot (`local_invocation_index`), is coincident 1:1 with this
        // base probe — no interpolation, no AABB test.
        let delta = read_delta_subblock(i, local);
        for (var b: u32 = 0u; b < 9u; b = b + 1u) {
            // Per-channel modulate by `color` so a colored animation curve
            // tints the delta SH the same way the lightmap pass does.
            delta_sum[b] = delta_sum[b] + delta[b] * color * brightness;
        }
    }

    // Fold the indirect-only animated delta into the base at full weight.
    for (var b: u32 = 0u; b < 9u; b = b + 1u) {
        bands[b] = bands[b] + delta_sum[b];
    }

    textureStore(sh_total_band0, p, vec4<f32>(bands[0], base_validity));
    textureStore(sh_total_band1, p, vec4<f32>(bands[1], 0.0));
    textureStore(sh_total_band2, p, vec4<f32>(bands[2], 0.0));
    textureStore(sh_total_band3, p, vec4<f32>(bands[3], 0.0));
    textureStore(sh_total_band4, p, vec4<f32>(bands[4], 0.0));
    textureStore(sh_total_band5, p, vec4<f32>(bands[5], 0.0));
    textureStore(sh_total_band6, p, vec4<f32>(bands[6], 0.0));
    textureStore(sh_total_band7, p, vec4<f32>(bands[7], 0.0));
    textureStore(sh_total_band8, p, vec4<f32>(bands[8], 0.0));
}
