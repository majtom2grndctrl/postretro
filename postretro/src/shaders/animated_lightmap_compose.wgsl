// Animated lightmap compose compute pass.
//
// Combines per-animated-light baked weight maps with runtime descriptor
// curves into a screen-resolution-independent `Rgba16Float` atlas that the
// forward pass samples alongside the static directional lightmap.
//
// Compose-only: the atlas is zero-initialized by wgpu at creation and the
// compose pass writes every texel the forward pass samples, so no per-frame
// clear is needed.
//
// Dispatch shape: one workgroup per 8×8 atlas tile (a `DispatchTile`
// record), flattened in `workgroup_id.x`. CPU-side `animated_lightmap.rs`
// refuses to load a map whose tile count exceeds 65535 — the 2D-dispatch
// fallback in the spec is not wired up. Bundled maps stay well below the
// cap; if a future authored map trips it, revisit here.
//
// Curve helpers come from `curve_eval.wgsl`, concatenated after this
// source at pipeline-build time. Both helpers read `anim_samples` by
// lexical name, so the binding declaration here doubles as the declaration
// for the helper.

struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    ambient_floor: f32,
    light_count: u32,
    time: f32,
    lighting_isolation: u32,
    _pad: u32,
};

// Same 48-byte layout as `forward.wgsl` — the buffer is shared via
// `AnimatedLightBuffers` so the field order must match exactly.
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

struct ChunkAtlasRect {
    atlas_x: u32,
    atlas_y: u32,
    width: u32,
    height: u32,
    texel_offset: u32,
};

struct TexelOffsetCount {
    offset: u32,
    count: u32,
};

struct TexelLight {
    light_index: u32,
    weight: f32,
};

struct DispatchTile {
    chunk_idx: u32,
    tile_origin_x: u32,
    tile_origin_y: u32,
    _pad: u32,
};

// Debug visualization uniform. Written once at init from the
// `POSTRETRO_ANIMATED_LM_DEBUG` env var (see `animated_lightmap.rs`).
//   mode = 0: normal path (accumulate shaded irradiance).
//   mode = 1: per-texel animated-light count as a red heatmap, scaled by
//             `MAX_ANIMATED_LIGHTS_PER_CHUNK_F`.
//   mode = 2: isolate a single descriptor slot; only contributions whose
//             `light_index == isolate_slot` accumulate.
struct DebugConfig {
    mode: u32,
    isolate_slot: u32,
    max_lights_per_chunk: u32,
    _pad: u32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

@group(1) @binding(0) var<storage, read> chunk_rects: array<ChunkAtlasRect>;
@group(1) @binding(1) var<storage, read> offset_counts: array<TexelOffsetCount>;
@group(1) @binding(2) var<storage, read> texel_lights: array<TexelLight>;
@group(1) @binding(3) var<storage, read> dispatch_tiles: array<DispatchTile>;
@group(1) @binding(4) var<storage, read> descriptors: array<AnimationDescriptor>;
@group(1) @binding(5) var<storage, read> anim_samples: array<f32>;
@group(1) @binding(6) var animated_lm_atlas: texture_storage_2d<rgba16float, write>;
@group(1) @binding(7) var<uniform> debug_config: DebugConfig;

@compute @workgroup_size(8, 8, 1)
fn compose_main(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    let tile = dispatch_tiles[wg.x];
    let rect = chunk_rects[tile.chunk_idx];
    let rect_x = tile.tile_origin_x + lid.x;
    let rect_y = tile.tile_origin_y + lid.y;
    if (rect_x >= rect.width || rect_y >= rect.height) {
        return;
    }
    // u32 arithmetic: WGSL wraps silently on overflow, but this is safe
    // within current atlas and budget constraints — the compiler uses checked
    // arithmetic when emitting texel_offset, width, and height values, so
    // any PRL that passes validation cannot produce an overflowing index here.
    let texel_idx = rect.texel_offset + rect_y * rect.width + rect_x;
    let oc = offset_counts[texel_idx];

    // Debug mode 1: per-texel light-count heatmap (red channel). Emit
    // before the accumulation loop and return — nothing else matters.
    if (debug_config.mode == 1u) {
        let denom = max(f32(debug_config.max_lights_per_chunk), 1.0);
        let heat = f32(oc.count) / denom;
        textureStore(
            animated_lm_atlas,
            vec2<i32>(i32(rect.atlas_x + rect_x), i32(rect.atlas_y + rect_y)),
            vec4<f32>(heat, 0.0, 0.0, 1.0),
        );
        return;
    }

    var accum = vec3<f32>(0.0);
    for (var i: u32 = 0u; i < oc.count; i = i + 1u) {
        let entry = texel_lights[oc.offset + i];
        // Debug mode 2: isolate a single descriptor slot.
        if (debug_config.mode == 2u && entry.light_index != debug_config.isolate_slot) {
            continue;
        }
        let desc = descriptors[entry.light_index];
        if (desc.is_active == 0u) {
            continue;
        }
        let t = fract(uniforms.time / max(desc.period, 1.0e-6) + desc.phase);
        let b = sample_curve_catmull_rom(desc.brightness_offset, desc.brightness_count, t);
        let c = sample_color_catmull_rom(desc.color_offset, desc.color_count, t, desc.base_color);
        accum = accum + c * b * entry.weight;
    }
    textureStore(
        animated_lm_atlas,
        vec2<i32>(i32(rect.atlas_x + rect_x), i32(rect.atlas_y + rect_y)),
        vec4<f32>(accum, 1.0),
    );
}
