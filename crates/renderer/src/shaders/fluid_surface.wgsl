// Fluid surface pass: translucent, two-sided fluid faces moving in whole texels,
// lit per fragment by baked SH indirect, baked direct SH, and dynamic lights,
// and mirroring their room through distance-corrected box-projected
// reflection probes.
// See: context/lib/rendering_pipeline.md §7.3a

// --- Group 0: camera uniforms (shared with forward and billboard) ---
// Field offsets mirror the Rust `Uniforms` writer; see billboard.wgsl.
struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    ambient_floor: f32,
    // Dynamic-tier record count: the prefix of `lights`.
    light_count: u32,
    // Level-relative animation clock (`script_time`), seconds.
    time: f32,
    light_term_mask: u32,
    indirect_scale: f32,
    sdf_shadow_flags: u32,
    sdf_shadow_mode: u32,
    sdf_force_visibility_one: u32,
    direct_scale: f32,
    has_scatter: u32,
    // Bit 0: a baked DIRECT SH section is present. Bits 1..31: the raw
    // section-45 AnimatedBakedLights tail count.
    has_direct: u32,
    // Dynamic tier, then the mover-only animated-baked tail, then promoted static.
    total_light_count: u32,
    _spec_shadowmask_flags_inert: u32,
};
@group(0) @binding(0) var<uniform> uniforms: Uniforms;

// --- Group 1: the face's world material (shared with forward and movers) ---
// Only albedo, emissive, the per-material uniform and the sampler are read.
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var emissive_texture: texture_2d<f32>;
struct MaterialUniform {
    shininess: f32,
    emissive_strength: f32,
    // Surface Depth fields; a fluid face never marches.
    surface_depth_peak_raise: f32,
    surface_depth_trough: f32,
    surface_depth_meters: f32,
    surface_depth_fade_distance: f32,
    surface_depth_quantize_levels: f32,
    surface_depth_march: u32,
};
@group(1) @binding(3) var<uniform> material: MaterialUniform;
@group(1) @binding(5) var aniso_sampler: sampler;

// --- Group 2: dynamic lights (shared with forward and billboard) ---
struct GpuLight {
    position_and_type: vec4<f32>,
    color_and_falloff_model: vec4<f32>,
    direction_and_range: vec4<f32>,
    cone_angles_and_pad: vec4<f32>,
};
@group(2) @binding(0) var<storage, read> lights: array<GpuLight>;
@group(2) @binding(1) var<storage, read> light_influence: array<vec4<f32>>;

// --- Group 3: SH irradiance volume (shared with forward and billboard) ---
struct ShGridInfo {
    grid_origin: vec3<f32>,
    has_sh_volume: u32,
    cell_size: vec3<f32>,
    _pad0: u32,
    grid_dimensions: vec3<u32>,
    _pad1: u32,
    atlas_dimensions: vec2<u32>,
    tile_dimension: u32,
    tile_border: u32,
    atlas_tiles_per_row: u32,
    atlas_tile_rows: u32,
    tile_interior: u32,
    physical_tile_stride: u32,
    probe_occlusion: u32,
    tiles_per_layer: u32,
    atlas_layer_count: u32,
    _pad3: u32,
};
@group(3) @binding(1) var sh_total_atlas: texture_2d_array<f32>;
@group(3) @binding(2) var sh_atlas_sampler: sampler;
@group(3) @binding(10) var<uniform> sh_grid: ShGridInfo;

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
// Per animated-baked light compose descriptors, roster-indexed (the loader
// refuses section 54 unless the roster-to-descriptor table is identity): the
// reflection's live lighting reads them as SH compose does.
@group(3) @binding(11) var<storage, read> anim_descriptors: array<AnimationDescriptor>;
// Curve samples and per-dynamic-light descriptors, as the forward pass reads
// them, so a scripted light pulses on the water as it does on the walls.
@group(3) @binding(12) var<storage, read> anim_samples: array<f32>;
@group(3) @binding(13) var<storage, read> scripted_light_descriptors: array<AnimationDescriptor>;
@group(3) @binding(14) var sh_depth_moments: texture_3d<u32>;
// Baked direct SH. Fluid takes baked direct from here per fragment, never from
// the VERTEX-only billboard scatter volume (binding 17).
@group(3) @binding(15) var sh_direct_atlas: texture_2d_array<f32>;

// --- Group 4: per-volume parameters, one uniform window of draw slots ---
// Mirrors render-cpu `FluidParamsRow`, packed CPU-side. The window rides a
// dynamic offset; the draw's instance index picks the slot inside it.
struct FluidVolumeRow {
    // x = surface opacity, y = fog extinction (3 / fogDistance, 0 = no fog),
    // z = reflectivity (0 for a volume with no probe-bound face), w spare.
    surface: vec4<f32>,
    // rgb = fog albedo (fogColor scaled), w spare.
    fog_albedo: vec4<f32>,
    // x = 1 / wavePeriod, y = amplitude, z = length min, w = length max; all 0 = no wave.
    wave: vec4<f32>,
};
struct FluidVolumeParams {
    slots: array<FluidVolumeRow, 1360>,
};
@group(4) @binding(0) var<uniform> fluid_volumes: FluidVolumeParams;

// The level's reflection probes: one capture tile per array layer, sampled at
// level 0 through the SH atlas's linear-clamp sampler, and the share table,
// written once at install. Mirrors render-cpu `fluid_surface::reflection`'s
// `FluidProbeLightGpu`, `FluidProbeGpu` and table header.
struct FluidProbeLight {
    reference: vec3<f32>,
    roster_index: u32,
    // One unorm10x3 RGB share per lobe, red lowest; the tail pads.
    shares: array<vec4<u32>, 2>,
};
struct FluidProbe {
    box_min: vec3<f32>,
    _pad0: u32,
    box_max: vec3<f32>,
    _pad1: u32,
    capture_point: vec3<f32>,
    layer: u32,
    lights: array<FluidProbeLight, 4>,
};
struct FluidProbeTable {
    // x = tile dimension, y = tile border, z = probe count, w spare.
    tile: vec4<u32>,
    probes: array<FluidProbe, 256>,
};
@group(4) @binding(1) var fluid_probe_array: texture_2d_array<f32>;
@group(4) @binding(2) var<uniform> fluid_probes: FluidProbeTable;
// Beside each capture tile, in the same layer, mapping and border: the mean
// inverse L1 distance from the capture point to what each texel saw, 0 where
// it saw nothing. Sampled at level 0 through the same sampler.
@group(4) @binding(3) var fluid_probe_distance: texture_2d_array<f32>;

// --- Group 5: fluid scene inputs, rebuilt only with the scene targets ---
// Scene depth, loaded under the pass's read-only depth attachment.
@group(5) @binding(0) var scene_depth: texture_depth_2d;
struct FluidSceneInputs {
    // The frame's eye fog weight.
    fog_weight: f32,
    // The mod's reflection style: FLUID_REFLECTION_STYLE_SMOOTH, else pixel.
    reflection_style: u32,
    spare: vec2<f32>,
};
@group(5) @binding(1) var<uniform> fluid_scene: FluidSceneInputs;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    // Second vertex buffer: the face's drift in texture periods per second,
    // rewritten in place whenever drift re-resolves.
    @location(3) drift_rate: vec2<f32>,
    // Third vertex buffer: the face's reflection probe, written at install;
    // FLUID_REFLECTION_NO_PROBE for a face no binding names.
    @location(4) probe: u32,
    // Fourth vertex buffer (cell-rate spike): the face's cell-atlas offset
    // (atlas origin minus first cell) and its inclusive cell rect; zero for a
    // face with no rect.
    @location(5) cell_offset: vec2<i32>,
    @location(6) cell_bounds: vec4<i32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) world_position: vec3<f32>,
    @location(3) @interpolate(flat) slot: u32,
    // The face's drift scroll this frame, texture periods in [-0.5, 0.5).
    @location(4) @interpolate(flat) drift_scroll: vec2<f32>,
    @location(5) @interpolate(flat) probe: u32,
    @location(6) @interpolate(flat) cell_offset: vec2<i32>,
    @location(7) @interpolate(flat) cell_bounds: vec4<i32>,
};

@vertex
fn vs_main(in: VertexInput, @builtin(instance_index) slot: u32) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.world_normal = in.normal;
    out.world_position = in.position;
    out.slot = slot;
    out.drift_scroll = fluid_drift_scroll(in.drift_rate, uniforms.time);
    out.probe = in.probe;
    out.cell_offset = in.cell_offset;
    out.cell_bounds = in.cell_bounds;
    return out;
}

// Whole-texel fluid motion on the face texture's own grid. Mirrors
// `postretro_render_cpu::fluid_surface::motion`; every constant is pinned to it.
const FLUID_MOTION_STEP_HZ: f32 = 2.5;
const FLUID_SHIMMER_AMPLITUDE: i32 = 1;
const FLUID_RING_AMPLITUDE: i32 = 1;
const FLUID_RING_BLOCK_SHIFT: i32 = 5;
const FLUID_RING_LIFETIME_STEPS: i32 = 5;
const FLUID_RING_MARGIN: i32 = 9;
const FLUID_RING_STEP_TEXELS: f32 = 2.0;
const FLUID_FADE_START_TEXELS_PER_PIXEL: f32 = 0.75;
const FLUID_FADE_END_TEXELS_PER_PIXEL: f32 = 1.5;
const FLUID_SALT_CELL: u32 = 0x9E3779B9u;
const FLUID_SALT_SHIMMER: u32 = 0x85EBCA6Bu;
const FLUID_SALT_FADE: u32 = 0xC2B2AE35u;
const FLUID_SALT_BLOCK: u32 = 0x27D4EB2Fu;
const FLUID_SALT_DROP: u32 = 0x165667B1u;
const FLUID_SALT_WAVE_U: u32 = 0x4C957F2Du;
const FLUID_SALT_WAVE_V: u32 = 0x5851F42Du;
const FLUID_WAVE_PAIRS: i32 = 4096;
const FLUID_WAVE_ARC_LINEAR: f32 = 3.0;
const FLUID_WAVE_ARC_QUADRATIC: f32 = 4.0;
const FLUID_HASH_MUL_A: u32 = 0x7FEB352Du;
const FLUID_HASH_MUL_B: u32 = 0x846CA68Bu;

fn fluid_hash_u32(value: u32) -> u32 {
    var x = value;
    x = x ^ (x >> 16u);
    x = x * FLUID_HASH_MUL_A;
    x = x ^ (x >> 15u);
    x = x * FLUID_HASH_MUL_B;
    x = x ^ (x >> 16u);
    return x;
}

fn fluid_hash3(a: u32, b: u32, c: u32) -> u32 {
    return fluid_hash_u32(a ^ fluid_hash_u32(b ^ fluid_hash_u32(c)));
}

fn fluid_hash_unit(h: u32) -> f32 {
    return f32(h >> 8u) * (1.0 / 16777216.0);
}

// The cell's seed: its phase, and the root of its other draws.
fn fluid_cell_seed(cell: vec2<i32>) -> u32 {
    return fluid_hash3(bitcast<u32>(cell.x), bitcast<u32>(cell.y), FLUID_SALT_CELL);
}

// Floor division for a positive divisor.
fn fluid_floor_div(a: i32, b: i32) -> i32 {
    let q = a / b;
    return select(q, q - 1, (a % b) != 0 && a < 0);
}

fn fluid_cell_step(seed: u32, time: f32) -> i32 {
    return i32(floor(time * FLUID_MOTION_STEP_HZ + fluid_hash_unit(seed)));
}

// Four-step cycle 0, +d, 0, -d along the cell's fixed direction: zero mean.
fn fluid_shimmer_offset(seed: u32, step: i32) -> vec2<i32> {
    let position = step & 3;
    if (position & 1) == 0 {
        return vec2<i32>(0);
    }
    let h = fluid_hash_u32(seed ^ FLUID_SALT_SHIMMER);
    let direction = (h >> 8u) % 9u;
    let d = vec2<i32>(i32(direction % 3u) - 1, i32(direction / 3u) - 1);
    return select(-d, d, position == 1);
}

// The ring of the cell's block drop, if it reaches the cell. Drops land at
// least FLUID_RING_MARGIN texels inside their block, so no ring leaves it.
fn fluid_ring_offset(cell: vec2<i32>, step: i32) -> vec2<i32> {
    let block_shift = u32(FLUID_RING_BLOCK_SHIFT);
    let block = cell >> vec2<u32>(block_shift);
    let seed = fluid_hash3(bitcast<u32>(block.x), bitcast<u32>(block.y), FLUID_SALT_BLOCK);
    let shifted = step + i32(seed % u32(FLUID_RING_LIFETIME_STEPS));
    let epoch = fluid_floor_div(shifted, FLUID_RING_LIFETIME_STEPS);
    let age = shifted - epoch * FLUID_RING_LIFETIME_STEPS;
    if age == 0 {
        return vec2<i32>(0);
    }
    let position = fluid_hash_u32(seed ^ fluid_hash_u32(bitcast<u32>(epoch) ^ FLUID_SALT_DROP));
    let span = u32((1 << block_shift) - 2 * FLUID_RING_MARGIN);
    let center = (block << vec2<u32>(block_shift))
        + vec2<i32>(FLUID_RING_MARGIN)
        + vec2<i32>(i32(position % span), i32((position >> 16u) % span));
    let diff = vec2<f32>(cell - center);
    let distance_sq = dot(diff, diff);
    let radius = f32(age) * FLUID_RING_STEP_TEXELS;
    // |distance - radius| < 0.5, without a square root off the ring.
    if distance_sq <= (radius - 0.5) * (radius - 0.5) || distance_sq >= (radius + 0.5) * (radius + 0.5) {
        return vec2<i32>(0);
    }
    return vec2<i32>(floor(diff / sqrt(distance_sq) + 0.5));
}

fn fluid_faded_flat(seed: u32, texels_per_pixel: f32) -> bool {
    let threshold = FLUID_FADE_START_TEXELS_PER_PIXEL
        + (FLUID_FADE_END_TEXELS_PER_PIXEL - FLUID_FADE_START_TEXELS_PER_PIXEL)
            * fluid_hash_unit(fluid_hash_u32(seed ^ FLUID_SALT_FADE));
    return texels_per_pixel > threshold;
}

fn fluid_displayed_offset(cell: vec2<i32>, time: f32, texels_per_pixel: f32) -> vec2<i32> {
    let seed = fluid_cell_seed(cell);
    if fluid_faded_flat(seed, texels_per_pixel) {
        return vec2<i32>(0);
    }
    let step = fluid_cell_step(seed, time);
    return fluid_shimmer_offset(seed, step) + fluid_ring_offset(cell, step);
}

// fract(a * b) without the product's rounding: 12-bit halves multiply exactly,
// so a day of script time still scrolls on speed.
fn fluid_high_half(x: f32) -> f32 {
    return bitcast<f32>(bitcast<u32>(x) & 0xFFFFF000u);
}

fn fluid_exact_fract_product(a: f32, b: f32) -> f32 {
    let a_hi = fluid_high_half(a);
    let b_hi = fluid_high_half(b);
    let a_lo = a - a_hi;
    let b_lo = b - b_hi;
    return fract(fract(a_hi * b_hi) + fract(a_hi * b_lo) + fract(a_lo * b_hi) + fract(a_lo * b_lo));
}

fn fluid_drift_scroll(rate: vec2<f32>, time: f32) -> vec2<f32> {
    let periods = vec2<f32>(
        fluid_exact_fract_product(time, rate.x),
        fluid_exact_fract_product(time, rate.y),
    ) + vec2<f32>(0.5);
    return periods - floor(periods) - vec2<f32>(0.5);
}

// a * b as a whole count and a fraction in [0, 1) from one split, so the two
// never disagree: the four exact partial products give their floors to the
// count and their fractions, with the carry, to the fraction.
struct FluidSplit {
    count: i32,
    fraction: f32,
};

fn fluid_exact_split_product(a: f32, b: f32) -> FluidSplit {
    let a_hi = fluid_high_half(a);
    let b_hi = fluid_high_half(b);
    let a_lo = a - a_hi;
    let b_lo = b - b_hi;
    let p0 = a_hi * b_hi;
    let p1 = a_hi * b_lo;
    let p2 = a_lo * b_hi;
    let p3 = a_lo * b_lo;
    let f0 = floor(p0);
    let f1 = floor(p1);
    let f2 = floor(p2);
    let f3 = floor(p3);
    let fraction = (p0 - f0) + (p1 - f1) + (p2 - f2) + (p3 - f3);
    let carry = floor(fraction);
    let count = i32(f0) + i32(f1) + i32(f2) + i32(f3) + i32(carry);
    return FluidSplit(count, fraction - carry);
}

// The crest at `position` texels along the wave's axis: x = distance into it,
// y = its length. Crests come in hashed pairs spanning two mean lengths; the
// phase count wraps over whole hash-repeat spans, so every crest keeps its
// length across the wrap. `phase` is time * (1/period), split once per cell;
// `wave` is the row's (1/period, amplitude, min, max).
fn fluid_wave_crest(position: f32, phase: FluidSplit, wave: vec4<f32>, salt: u32) -> vec2<f32> {
    let count = phase.count & (2 * FLUID_WAVE_PAIRS - 1);
    let pairs = count >> 1u;
    let parity = f32(count & 1);
    let pair_length = wave.z + wave.w;
    let pair_position = position / pair_length - (parity + phase.fraction) * 0.5;
    let whole = floor(pair_position);
    let pair = (i32(whole) - pairs) & (FLUID_WAVE_PAIRS - 1);
    let into_pair = (pair_position - whole) * pair_length;
    let d = (2.0 * fluid_hash_unit(fluid_hash_u32(bitcast<u32>(pair) ^ salt)) - 1.0)
        * (wave.w - wave.z)
        * 0.5;
    let first = 0.5 * pair_length + d;
    return select(vec2<f32>(into_pair - first, pair_length - first), vec2<f32>(into_pair, first),
        into_pair < first);
}

// A positive half-arc then a negative one, each zero at both ends.
fn fluid_wave_arc(crest: vec2<f32>) -> f32 {
    let half = 0.5 * crest.y;
    let second = crest.x >= half;
    let x = (crest.x - select(0.0, half, second)) / half;
    let g = x * (1.0 - x);
    let h = g * (FLUID_WAVE_ARC_LINEAR + FLUID_WAVE_ARC_QUADRATIC * g);
    return select(h, -h, second);
}

// A real guard, not a select: a no-wave or unwritten row has zero lengths,
// and the crest divides by their sum.
fn fluid_wave_value(position: f32, phase: FluidSplit, wave: vec4<f32>, salt: u32) -> f32 {
    if wave.y == 0.0 || wave.x == 0.0 {
        return 0.0;
    }
    return wave.y * fluid_wave_arc(fluid_wave_crest(position, phase, wave, salt));
}

// Whole-texel wave offset for a cell: u keyed on the cell's v, v on its u,
// both from one phase split.
fn fluid_wave_offset(cell: vec2<i32>, time: f32, wave: vec4<f32>) -> vec2<i32> {
    let phase = fluid_exact_split_product(time, wave.x);
    let shift = vec2<f32>(
        fluid_wave_value(f32(cell.y) + 0.5, phase, wave, FLUID_SALT_WAVE_U),
        fluid_wave_value(f32(cell.x) + 0.5, phase, wave, FLUID_SALT_WAVE_V),
    );
    return vec2<i32>(floor(shift + vec2<f32>(0.5)));
}

// The warped sample UV. Cells are floored from the UNWARPED texel UV, so
// shimmer, rings and the wave stay on the fixed grid while the drift scrolls
// beneath. The wave adds beside drift, outside the displayed offset.
fn fluid_warp_uv(uv: vec2<f32>, drift_scroll: vec2<f32>, dims: vec2<f32>, wave: vec4<f32>,
                 time: f32, texels_per_pixel: f32) -> vec2<f32> {
    let texel_uv = uv * dims;
    let cell = vec2<i32>(floor(texel_uv));
    let drift_texels = floor(drift_scroll * dims);
    let wave_texels = vec2<f32>(fluid_wave_offset(cell, time, wave));
    let cell_texels = vec2<f32>(fluid_displayed_offset(cell, time, texels_per_pixel));
    return (texel_uv + drift_texels + wave_texels + cell_texels) / dims;
}

// Depth fog. Mirrors `postretro_render_cpu::fluid_surface::fog`: each fog
// function and constant here is pinned to it, and `fs_main`'s tail inlines its
// `fog_fragment`. The extinction constants are CPU-only, used in packing.
const FLUID_FOG_BAND: f32 = 0.08;
const FLUID_FOG_PATH_MAX: f32 = 1.0e4;
const FLUID_FOG_DENOM_EPSILON: f32 = 1.0e-8;
const FLUID_FOG_ALPHA_EPSILON: f32 = 1.0e-4;

// Metres the view ray runs past the fragment to the scene depth behind it.
// The ray is linear in clip space, so solving its NDC depth needs no inverse.
fn fluid_fog_path(view_proj: mat4x4<f32>, camera: vec3<f32>, frag: vec3<f32>, depth: f32) -> f32 {
    let c0 = view_proj * vec4<f32>(camera, 1.0);
    let c1 = view_proj * vec4<f32>(frag, 1.0);
    let denom = (c1.z - c0.z) - depth * (c1.w - c0.w);
    let s = select(FLUID_FOG_PATH_MAX, (depth * c0.w - c0.z) / denom,
        abs(denom) > FLUID_FOG_DENOM_EPSILON);
    return clamp(s - 1.0, 0.0, FLUID_FOG_PATH_MAX) * length(frag - camera);
}

fn fluid_fog_transmittance(extinction: f32, path: f32) -> f32 {
    return exp(-extinction * path);
}

// 0 at and below the face's own plane, 1 one band above it along the
// outward normal, so an eye bobbing at a plane never flips the murk.
fn fluid_fog_proximity(camera: vec3<f32>, frag: vec3<f32>, outward: vec3<f32>) -> f32 {
    return clamp(dot(camera - frag, outward) / FLUID_FOG_BAND, 0.0, 1.0);
}

fn fluid_fog_seen_from_outside(outward: vec3<f32>, view: vec3<f32>) -> bool {
    return dot(outward, view) > 0.0;
}

fn fluid_fog_layer_opacity(weight: f32, proximity: f32, transmittance: f32) -> f32 {
    return weight * proximity * (1.0 - transmittance);
}

// Surface over fog over scene, as one straight-alpha output.
fn fluid_fog_composite(rgb: vec3<f32>, alpha: f32, fog_rgb: vec3<f32>, fog_opacity: f32) -> vec4<f32> {
    let total = 1.0 - (1.0 - alpha) * (1.0 - fog_opacity);
    let out = (rgb * alpha + fog_rgb * ((1.0 - alpha) * fog_opacity))
        / max(total, FLUID_FOG_ALPHA_EPSILON);
    return vec4<f32>(out, total);
}

// Set per pipeline (render-cpu `FluidShaderVariant`). The pass builds this
// shader twice over one layout: false for draws that cannot reflect, so their
// fragments compile with no reflection code at all (its mere presence slows
// the always-run lighting on some drivers), true for the rest.
override FLUID_REFLECTIONS: bool = true;

// Reflection probes. Mirrors `postretro_render_cpu::fluid_surface::reflection`:
// each function and constant here is pinned to it, and `fs_main`'s tail
// inlines its `reflection_fragment` after the fog composite.
const FLUID_REFLECTION_BASE_F0: f32 = 0.02;
const FLUID_REFLECTION_NO_PROBE: u32 = 0xFFFFFFFFu;
const FLUID_REFLECTION_ALL_TERMS: u32 = 0x17Fu;
const FLUID_REFLECTION_DIRECTION_EPSILON: f32 = 1.0e-6;
const FLUID_REFLECTION_COVERAGE_EPSILON: f32 = 1.0e-4;
const FLUID_REFLECTION_REFERENCE_FLOOR: f32 = 1.0e-30;
const FLUID_REFLECTION_UNORM10_MAX: u32 = 1023u;
const FLUID_REFLECTION_NO_LIGHT: u32 = 0xFFFFFFFFu;
const FLUID_REFLECTION_LIGHTS_PER_PROBE: u32 = 4u;
const FLUID_REFLECTION_RAY_MIN_Y: f32 = 1.0e-3;

// The capture bakes every lighting term together, so any isolating mask
// omits the probe term.
fn fluid_reflection_mask_on(mask: u32) -> bool {
    return (mask & FLUID_REFLECTION_ALL_TERMS) == FLUID_REFLECTION_ALL_TERMS;
}

// From the face's up side, by the interpolated outward normal, never the
// viewer-flipped shading normal.
fn fluid_reflection_seen_from_above(outward: vec3<f32>, view: vec3<f32>) -> bool {
    return dot(outward, view) > 0.0;
}

fn fluid_reflection_gate(probe: u32, outward: vec3<f32>, view: vec3<f32>, mask: u32,
                         reflectivity: f32) -> bool {
    return probe != FLUID_REFLECTION_NO_PROBE
        && reflectivity > 0.0
        && fluid_reflection_seen_from_above(outward, view)
        && fluid_reflection_mask_on(mask);
}

fn fluid_reflection_mirror(view: vec3<f32>, outward: vec3<f32>) -> vec3<f32> {
    return outward * (2.0 * dot(view, outward)) - view;
}

// The reflected ray kept in the upper hemisphere: unchanged at or above the
// floor, else lifted to it and renormalized, azimuth kept. A pixel cell's
// tilt, or a flat mirror's grazing view, can reach the floor.
fn fluid_reflection_above_horizon(ray: vec3<f32>) -> vec3<f32> {
    if ray.y >= FLUID_REFLECTION_RAY_MIN_Y {
        return ray;
    }
    return normalize(vec3<f32>(ray.x, FLUID_REFLECTION_RAY_MIN_Y, ray.z));
}

// The runtime slab test: where the reflected ray leaves the projection box.
// A component near zero is nudged off it rather than divided by. The origin
// may lie past the box (water outside an authored box, below a derived
// box's floor); a slab whose exit lies at or behind it (past a face the ray
// leaves through) does not stop the ray, which ends at another slab's exit.
fn fluid_reflection_box_hit(origin: vec3<f32>, ray: vec3<f32>, box_min: vec3<f32>,
                            box_max: vec3<f32>) -> vec3<f32> {
    let nudged = select(vec3<f32>(FLUID_REFLECTION_DIRECTION_EPSILON),
        vec3<f32>(-FLUID_REFLECTION_DIRECTION_EPSILON), ray < vec3<f32>(0.0));
    let safe = select(ray, nudged, abs(ray) < vec3<f32>(FLUID_REFLECTION_DIRECTION_EPSILON));
    let t0 = (box_min - origin) / safe;
    let t1 = (box_max - origin) / safe;
    let far = max(t0, t1);
    let lead = max(far.x, max(far.y, far.z));
    let ahead = select(vec3<f32>(lead), far, far > vec3<f32>(0.0));
    let t = max(min(ahead.x, min(ahead.y, ahead.z)), 0.0);
    return origin + ray * t;
}

// From the capture point to the hit, clamped to the horizon below it.
fn fluid_reflection_lookup(hit: vec3<f32>, capture_point: vec3<f32>, ray: vec3<f32>) -> vec3<f32> {
    let toward = hit - capture_point;
    let level = vec3<f32>(toward.x, max(toward.y, 0.0), toward.z);
    return select(level, vec3<f32>(ray.x, max(ray.y, 0.0), ray.z),
        dot(level, level) < FLUID_REFLECTION_DIRECTION_EPSILON);
}

// The SH atlas's octahedral mapping and border convention over one tile.
fn fluid_reflection_tile_uv(direction: vec3<f32>, tile_dimension: u32, tile_border: u32) -> vec2<f32> {
    let dimension = f32(tile_dimension);
    let border = f32(tile_border);
    let interior = dimension - 2.0 * border;
    return (vec2<f32>(border) + oct_encode_unquantized(direction) * interior) / dimension;
}

// Schlick with base reflectance 0.02 x reflectivity, topping at 1.
fn fluid_reflection_fresnel(cos_theta: f32, reflectivity: f32) -> f32 {
    let f0 = min(FLUID_REFLECTION_BASE_F0 * reflectivity, 1.0);
    let m = 1.0 - clamp(cos_theta, 0.0, 1.0);
    let m2 = m * m;
    return f0 + (1.0 - f0) * (m2 * m2 * m);
}

// The reflection over the straight-alpha surface (over fog) at union
// coverage, still straight alpha: opacity never dims the reflection.
fn fluid_reflection_composite(surface: vec4<f32>, reflection: vec3<f32>, fresnel: f32) -> vec4<f32> {
    let coverage = fresnel + (1.0 - fresnel) * surface.a;
    let rgb = (reflection * fresnel + surface.rgb * ((1.0 - fresnel) * surface.a))
        / max(coverage, FLUID_REFLECTION_COVERAGE_EPSILON);
    return vec4<f32>(rgb, coverage);
}

// Live lighting by light share. The capture holds every animated light at its
// reference; each stored light scales it by its share toward the lookup
// direction times its live-over-reference change. Reads no SH and no
// promotion weight, so a switch reaches every reflection on the next frame at
// any distance, and promotion never moves it.

// The share lobes' blend weights for a lookup direction: up, +X, -X, +Z, -Z.
// Mirrors level-format `reflection_probe_lobe_weights`, which the compiler
// split the capture's light by.
fn fluid_reflection_lobe_weights(direction: vec3<f32>) -> array<f32, 5> {
    let positive = max(direction, vec3<f32>(0.0));
    let negative = max(-direction, vec3<f32>(0.0));
    let down = negative.y * negative.y * 0.25;
    let weights = array<f32, 5>(
        positive.y * positive.y,
        positive.x * positive.x + down,
        negative.x * negative.x + down,
        positive.z * positive.z + down,
        negative.z * negative.z + down,
    );
    let sum = weights[0] + weights[1] + weights[2] + weights[3] + weights[4];
    if !(sum > 0.0) {
        return array<f32, 5>(1.0, 0.0, 0.0, 0.0, 0.0);
    }
    return array<f32, 5>(weights[0] / sum, weights[1] / sum, weights[2] / sum, weights[3] / sum,
        weights[4] / sum);
}

// The reserved roster value, and any index past the descriptors, indexes
// nothing: the entry adds no term.
fn fluid_reflection_light_present(roster_index: u32) -> bool {
    return roster_index != FLUID_REFLECTION_NO_LIGHT
        && roster_index < arrayLength(&anim_descriptors);
}

// A stored light's live value: sh_compose.wgsl's `animated_light_scale`, read
// straight from descriptor `roster_index` (identity table). Clamped colour x
// clamped brightness, 0 while inactive; no promotion weight, lighting mask or
// debug override.
fn fluid_reflection_light_value(roster_index: u32) -> vec3<f32> {
    let desc = anim_descriptors[roster_index];
    if (desc.is_active == 0u) {
        return vec3<f32>(0.0);
    }

    let t = animation_curve_t(desc.period, desc.phase, uniforms.time);
    let brightness = max(
        sample_curve_catmull_rom(desc.brightness_offset, desc.brightness_count, t),
        0.0,
    );
    var color = desc.base_color;
    if (desc.color_count > 0u) {
        color = max(
            sample_color_catmull_rom(desc.color_offset, desc.color_count, t, vec3<f32>(1.0)),
            vec3<f32>(0.0),
        ) * desc.base_color;
    }
    return color * brightness;
}

fn fluid_reflection_unpack_unorm10x3(word: u32) -> vec3<f32> {
    let field = FLUID_REFLECTION_UNORM10_MAX;
    return vec3<f32>(f32(word & field), f32((word >> 10u) & field), f32((word >> 20u) & field))
        / f32(field);
}

// A stored light's RGB share toward the lookup: its lobe shares, blended.
fn fluid_reflection_light_share(shares: array<vec4<u32>, 2>, lobes: array<f32, 5>) -> vec3<f32> {
    return fluid_reflection_unpack_unorm10x3(shares[0].x) * lobes[0]
        + fluid_reflection_unpack_unorm10x3(shares[0].y) * lobes[1]
        + fluid_reflection_unpack_unorm10x3(shares[0].z) * lobes[2]
        + fluid_reflection_unpack_unorm10x3(shares[0].w) * lobes[3]
        + fluid_reflection_unpack_unorm10x3(shares[1].x) * lobes[4];
}

// current / reference - 1 per channel; a channel the reference holds no light
// in adds no term.
fn fluid_reflection_light_change(current: vec3<f32>, reference: vec3<f32>) -> vec3<f32> {
    return select(vec3<f32>(0.0),
        current / max(reference, vec3<f32>(FLUID_REFLECTION_REFERENCE_FLOOR)) - vec3<f32>(1.0),
        reference > vec3<f32>(0.0));
}

// The capture's live-lighting scale toward `direction`, clamped at 0. Lights
// past the stored K have no entry and hold their reference.
fn fluid_reflection_ratio(probe: u32, direction: vec3<f32>) -> vec3<f32> {
    let lobes = fluid_reflection_lobe_weights(direction);
    var ratio = vec3<f32>(1.0);
    for (var i: u32 = 0u; i < FLUID_REFLECTION_LIGHTS_PER_PROBE; i = i + 1u) {
        let light = fluid_probes.probes[probe].lights[i];
        if !fluid_reflection_light_present(light.roster_index) {
            continue;
        }
        let current = fluid_reflection_light_value(light.roster_index);
        ratio = ratio + fluid_reflection_light_share(light.shares, lobes)
            * fluid_reflection_light_change(current, light.reference);
    }
    return max(ratio, vec3<f32>(0.0));
}

// Distance search. Mirrors `postretro_render_cpu::fluid_surface::reflection_search`:
// box projection corrected against the probe's stored per-texel distance.
// K sphere steps then M false-position estimates, both constant bounds; at
// most 2 + K + M - 1 distance taps, beside the capture-shadow fade's two to
// four, which depend on the origin alone. Its state lives only inside
// `fluid_reflection_search`, which runs to its end before the colour tap and
// the light-share loop, so only the lookup and the weight outlive it.
const FLUID_REFLECTION_SEARCH_STEPS: u32 = 3u;
const FLUID_REFLECTION_SEARCH_REFINEMENTS: u32 = 2u;
// The capture-shadow fade: N capture texels of angle (pi / interior each)
// beside the shadow, gated on the side tap's stored surface standing in
// front of the opposite one's, and the depth margin kappa behind it.
const FLUID_REFLECTION_SHADOW_FADE_TEXELS: f32 = 2.0;
const FLUID_REFLECTION_SHADOW_GATE_START: f32 = 6.0;
const FLUID_REFLECTION_SHADOW_GATE_FULL: f32 = 10.0;
const FLUID_REFLECTION_SHADOW_MARGIN: f32 = 0.05;
const FLUID_REFLECTION_HALF_TURN: f32 = 3.1415927;
const FLUID_REFLECTION_SEARCH_EPSILON: f32 = 1.0e-6;

// The search's result: the tile lookup direction, and the probe term's
// weight (1, except faded out in the capture shadow).
struct FluidReflectionSearch {
    lookup: vec3<f32>,
    weight: f32,
};

// h(Q) = |v|1 * w(v) - 1 for v the horizon-clamped offset from the capture
// point to q: negative in front of the stored surface, at or above 0 behind
// it; a stored 0 (no surface) gives -1, never a root. `layer` is the face's
// probe layer, passed through unchanged.
fn fluid_reflection_search_h(q: vec3<f32>, capture_point: vec3<f32>, tile: vec2<u32>,
                             layer: u32) -> f32 {
    let toward = q - capture_point;
    let level = vec3<f32>(toward.x, max(toward.y, 0.0), toward.z);
    let l1 = abs(level.x) + abs(level.y) + abs(level.z);
    if l1 < FLUID_REFLECTION_SEARCH_EPSILON {
        return -1.0;
    }
    return l1 * textureSampleLevel(fluid_probe_distance, sh_atlas_sampler,
        fluid_reflection_tile_uv(level, tile.x, tile.y), layer, 0.0).r - 1.0;
}

// The fade's angle in radians: N interior texels of pi / interior each.
fn fluid_reflection_shadow_fade_angle(tile: vec2<u32>) -> f32 {
    return FLUID_REFLECTION_SHADOW_FADE_TEXELS * FLUID_REFLECTION_HALF_TURN
        / (f32(tile.x) - 2.0 * f32(tile.y));
}

// h at the origin turned about the capture point's vertical by `turn`
// radians, keeping its distance; `toward` is the origin minus the capture
// point. No trig: the turn's sine and cosine are short Taylor series, exact
// to f32 at the fade's few degrees.
fn fluid_reflection_shadow_tap(capture_point: vec3<f32>, toward: vec3<f32>, turn: f32,
                               tile: vec2<u32>, layer: u32) -> f32 {
    let turn2 = turn * turn;
    let sine = turn * (1.0 - turn2 / 6.0 * (1.0 - turn2 / 20.0));
    let cosine = 1.0 - turn2 / 2.0 * (1.0 - turn2 / 12.0);
    let turned = vec3<f32>(toward.x * cosine + toward.z * sine, toward.y,
        toward.z * cosine - toward.x * sine);
    return fluid_reflection_search_h(capture_point + turned, capture_point, tile, layer);
}

// One side's shadow edge, in radians: at most `angle`, 0 for an origin
// behind the stored surface. A far tap behind its surface, gated on standing
// in front of the opposite tap's surface, takes a half-turn tap and places
// the edge where h, interpolated linearly, crosses 0.
fn fluid_reflection_shadow_edge(capture_point: vec3<f32>, toward: vec3<f32>, angle: f32,
                                side: f32, h_origin: f32, h_far: f32, h_opposite: f32,
                                tile: vec2<u32>, layer: u32) -> f32 {
    let step = (h_far - h_opposite) / max(1.0 + h_opposite, FLUID_REFLECTION_SEARCH_EPSILON);
    let h_lit = min(h_origin, 0.0);
    let gate = smoothstep(FLUID_REFLECTION_SHADOW_GATE_START * angle,
        FLUID_REFLECTION_SHADOW_GATE_FULL * angle, step);
    if h_far <= 0.0 || gate <= 0.0 {
        return angle;
    }
    let h_half = fluid_reflection_shadow_tap(capture_point, toward, 0.5 * side, tile, layer);
    var crossing: f32;
    if h_half > 0.0 {
        crossing = 0.5 * angle * h_lit / (h_lit - h_half);
    } else {
        crossing = 0.5 * angle + 0.5 * angle * h_half / (h_half - h_far);
    }
    return angle + (crossing - angle) * gate;
}

// The fade beside the capture shadow: 1 with no edge within the angle,
// falling smoothly to 0 at the nearer side's edge. Two taps, three with an
// edge near, four with one each side.
fn fluid_reflection_shadow_fade(origin: vec3<f32>, capture_point: vec3<f32>, tile: vec2<u32>,
                                h_origin: f32, layer: u32) -> f32 {
    let angle = fluid_reflection_shadow_fade_angle(tile);
    let toward = origin - capture_point;
    let h_plus = fluid_reflection_shadow_tap(capture_point, toward, angle, tile, layer);
    let h_minus = fluid_reflection_shadow_tap(capture_point, toward, -angle, tile, layer);
    let edge = min(
        fluid_reflection_shadow_edge(capture_point, toward, angle, angle, h_origin, h_plus,
            h_minus, tile, layer),
        fluid_reflection_shadow_edge(capture_point, toward, angle, -angle, h_origin, h_minus,
            h_plus, tile, layer));
    return smoothstep(0.0, angle, edge);
}

// The search along unit `ray` from `origin` (the fragment, or the pixel
// cell's point), inside the probe's box or past it: two independent taps
// (origin, box hit); an origin behind the stored surface is in the capture
// shadow, drawing nothing past the depth margin. Else the fade beside the
// shadow weighs the term, and up to K sphere steps close a bracket, or the
// box hit does, or nothing stored lies inside the box (the box hit, plain
// box projection); then M false-position estimates on the bracket.
fn fluid_reflection_search(origin: vec3<f32>, ray: vec3<f32>, box_min: vec3<f32>,
                           box_max: vec3<f32>, capture_point: vec3<f32>, tile: vec2<u32>,
                           layer: u32) -> FluidReflectionSearch {
    let box_hit = fluid_reflection_box_hit(origin, ray, box_min, box_max);
    let t_box = dot(box_hit - origin, ray);
    let h_origin = fluid_reflection_search_h(origin, capture_point, tile, layer);
    let h_box = fluid_reflection_search_h(box_hit, capture_point, tile, layer);
    if h_origin >= FLUID_REFLECTION_SHADOW_MARGIN {
        return FluidReflectionSearch(fluid_reflection_lookup(box_hit, capture_point, ray), 0.0);
    }
    let weight = fluid_reflection_shadow_fade(origin, capture_point, tile, h_origin, layer)
        * (1.0 - smoothstep(0.0, FLUID_REFLECTION_SHADOW_MARGIN, h_origin));
    if h_origin >= 0.0 {
        return FluidReflectionSearch(fluid_reflection_lookup(box_hit, capture_point, ray), weight);
    }

    // Front end of the bracket: the last sample in front of a surface. The
    // back end starts at the box hit and moves to the first sample behind one.
    var t_front = 0.0;
    var h_front = h_origin;
    var t_back = t_box;
    var h_back = h_box;
    let from_capture = origin - capture_point;
    let b = dot(from_capture, ray);
    for (var i: u32 = 0u; i < FLUID_REFLECTION_SEARCH_STEPS; i = i + 1u) {
        if h_front <= -1.0 {
            break;
        }
        let q = origin + ray * t_front;
        let toward = q - capture_point;
        let level = vec3<f32>(toward.x, max(toward.y, 0.0), toward.z);
        let radius = length(level) / (h_front + 1.0);
        let disc = b * b - (dot(from_capture, from_capture) - radius * radius);
        if disc < 0.0 {
            break;
        }
        let t_next = -b + sqrt(disc);
        if t_next <= t_front || t_next >= t_box {
            break;
        }
        let h_next = fluid_reflection_search_h(origin + ray * t_next, capture_point, tile, layer);
        if h_next >= 0.0 {
            t_back = t_next;
            h_back = h_next;
            break;
        }
        t_front = t_next;
        h_front = h_next;
    }
    if !(h_back >= 0.0) {
        return FluidReflectionSearch(fluid_reflection_lookup(box_hit, capture_point, ray), weight);
    }

    var t_hit = t_back;
    for (var refinement: u32 = 0u; refinement < FLUID_REFLECTION_SEARCH_REFINEMENTS;
         refinement = refinement + 1u) {
        if h_back != h_front {
            t_hit = t_front - h_front * (t_back - t_front) / (h_back - h_front);
        } else {
            t_hit = t_back;
        }
        if refinement + 1u == FLUID_REFLECTION_SEARCH_REFINEMENTS {
            break;
        }
        let h_hit = fluid_reflection_search_h(origin + ray * t_hit, capture_point, tile, layer);
        if h_hit >= 0.0 {
            t_back = t_hit;
            h_back = h_hit;
        } else {
            t_front = t_hit;
            h_front = h_hit;
        }
    }
    return FluidReflectionSearch(fluid_reflection_lookup(origin + ray * t_hit, capture_point, ray),
        weight);
}

// The probe term before Fresnel: the capture toward the search's lookup,
// scaled by the live-lighting ratio toward that same direction, and the
// search's weight.
struct FluidReflectionSample {
    capture: vec3<f32>,
    weight: f32,
};

// The search runs first and ends; only its lookup and weight reach the colour
// tap and the ratio loop. The layer comes from the face's probe index alone,
// and the distance taps read that same layer; the lookup picks only the texel
// and the share. A mirrored ray at the horizon is lifted above it first.
fn fluid_reflection_sample(probe: u32, world_position: vec3<f32>, view: vec3<f32>,
                           outward: vec3<f32>) -> FluidReflectionSample {
    let ray = fluid_reflection_above_horizon(fluid_reflection_mirror(view, outward));
    let layer = fluid_probes.probes[probe].layer;
    let search = fluid_reflection_search(world_position, ray, fluid_probes.probes[probe].box_min,
        fluid_probes.probes[probe].box_max, fluid_probes.probes[probe].capture_point,
        fluid_probes.tile.xy, layer);
    let uv = fluid_reflection_tile_uv(search.lookup, fluid_probes.tile.x, fluid_probes.tile.y);
    let capture = textureSampleLevel(fluid_probe_array, sh_atlas_sampler, uv, layer, 0.0).rgb;
    return FluidReflectionSample(capture * fluid_reflection_ratio(probe, search.lookup),
        search.weight);
}

// Reflection style. Mirrors `postretro_render_cpu::fluid_surface::reflection_style`:
// smooth reflects the flat face per fragment; pixel reflects once per water
// texel cell, from the cell's world-space centre (held to the probe's box at
// a wall rim) about a normal tilted by the cell's displayed offset, so every
// fragment of a cell reads one sample.
// Unlike the warp, this look is the author's (`render.reflections.style`).
const FLUID_REFLECTION_STYLE_PIXEL: u32 = 0u;
const FLUID_REFLECTION_STYLE_SMOOTH: u32 = 1u;
const FLUID_REFLECTION_PIXEL_TILT: f32 = 0.05;
const FLUID_REFLECTION_AXES_EPSILON: f32 = 1.0e-8;
// A pulled rim cell lands this fraction of its horizontal L1 distance from
// the capture point inside the box (at least the floor, in metres), off the
// face where the stored wall's h is 0 and a ray into it leaves at t = 0.
const FLUID_REFLECTION_RIM_INSET: f32 = 1.0e-3;
const FLUID_REFLECTION_RIM_INSET_MIN: f32 = 1.0e-4;

// The face's world step per texel along u and v.
struct FluidTexelAxes {
    u: vec3<f32>,
    v: vec3<f32>,
};

struct FluidReflectionPoint {
    position: vec3<f32>,
    normal: vec3<f32>,
};

// The texel-to-world Jacobian from one fragment's screen derivatives; zero on
// an edge-on or vanished footprint.
fn fluid_reflection_texel_axes(ddx_texel: vec2<f32>, ddy_texel: vec2<f32>, ddx_world: vec3<f32>,
                               ddy_world: vec3<f32>) -> FluidTexelAxes {
    let det = ddx_texel.x * ddy_texel.y - ddy_texel.x * ddx_texel.y;
    let scale = dot(ddx_texel, ddx_texel) * dot(ddy_texel, ddy_texel);
    if det * det <= FLUID_REFLECTION_AXES_EPSILON * scale {
        return FluidTexelAxes(vec3<f32>(0.0), vec3<f32>(0.0));
    }
    return FluidTexelAxes((ddx_world * ddy_texel.y - ddy_world * ddx_texel.y) / det,
        (ddy_world * ddx_texel.x - ddx_world * ddy_texel.x) / det);
}

// The world-space centre of the unwarped texel cell the fragment lies in.
fn fluid_reflection_cell_centre(texel_uv: vec2<f32>, world_position: vec3<f32>,
                                axes: FluidTexelAxes) -> vec3<f32> {
    let to_centre = floor(texel_uv) + vec2<f32>(0.5) - texel_uv;
    return world_position + axes.u * to_centre.x + axes.v * to_centre.y;
}

// The cell centre pulled toward the probe's box inset horizontally by the
// rim inset m: whole while its horizontal overshoot o past the face is at
// most h, half the cell's longer diagonal, then tapering to nothing at 2h,
// the pull scaled by clamp((2h - |o|) / h, 0, 1). A cell straddling a wall
// the box fits starts just in front of the wall's face, not behind the
// stored wall or on it; a cell a full cell or more outside the box stays put.
fn fluid_reflection_cell_within_box(centre: vec3<f32>, axes: FluidTexelAxes,
                                    box_min: vec3<f32>, box_max: vec3<f32>,
                                    capture_point: vec3<f32>) -> vec3<f32> {
    let past = centre - clamp(centre, box_min, box_max);
    let length_past = length(vec3<f32>(past.x, 0.0, past.z));
    let half_cell = 0.5 * max(length(axes.u + axes.v), length(axes.u - axes.v));
    let toward = centre - capture_point;
    let inset = max(FLUID_REFLECTION_RIM_INSET * (abs(toward.x) + abs(toward.z)),
        FLUID_REFLECTION_RIM_INSET_MIN);
    let margin = vec3<f32>(inset, 0.0, inset);
    let target_point = max(min(centre, box_max - margin), box_min + margin);
    let pull = vec3<f32>(centre.x - target_point.x, 0.0, centre.z - target_point.z);
    var keep = 0.0;
    if half_cell > 0.0 {
        keep = clamp((2.0 * half_cell - length_past) / half_cell, 0.0, 1.0);
    }
    return centre - pull * keep;
}

fn fluid_reflection_unit_or_zero(v: vec3<f32>) -> vec3<f32> {
    let length_sq = dot(v, v);
    return select(vec3<f32>(0.0), v * inverseSqrt(length_sq), length_sq > 0.0);
}

// The flat normal tilted along the texture's axes by the cell's offset.
fn fluid_reflection_cell_normal(outward: vec3<f32>, axes: FluidTexelAxes, offset: vec2<i32>) -> vec3<f32> {
    let tilt = fluid_reflection_unit_or_zero(axes.u) * f32(offset.x)
        + fluid_reflection_unit_or_zero(axes.v) * f32(offset.y);
    return normalize(outward + tilt * FLUID_REFLECTION_PIXEL_TILT);
}

// Where the fragment reflects from, and about which normal. The pixel
// offset is the albedo's own shimmer and rings, never drift or the wave; the
// pixel centre is held to the probe's box at a wall rim.
fn fluid_reflection_point(style: u32, texel_uv: vec2<f32>, world_position: vec3<f32>,
                          outward: vec3<f32>, axes: FluidTexelAxes, time: f32,
                          texels_per_pixel: f32, box_min: vec3<f32>,
                          box_max: vec3<f32>, capture_point: vec3<f32>) -> FluidReflectionPoint {
    if style == FLUID_REFLECTION_STYLE_SMOOTH {
        return FluidReflectionPoint(world_position, outward);
    }
    let offset = fluid_displayed_offset(vec2<i32>(floor(texel_uv)), time, texels_per_pixel);
    let centre = fluid_reflection_cell_centre(texel_uv, world_position, axes);
    return FluidReflectionPoint(
        fluid_reflection_cell_within_box(centre, axes, box_min, box_max, capture_point),
        fluid_reflection_cell_normal(outward, axes, offset));
}

// Post Retro sample, the same body as kinematic_brush.wgsl (pinned by a parity
// test). The seam width and mip footprint come from the UNWARPED derivatives:
// the warp shifts the sample point, it does not change the pixel footprint.
fn sample_post_retro(tex: texture_2d<f32>, samp: sampler, uv: vec2<f32>,
                     ddx: vec2<f32>, ddy: vec2<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(tex, 0));
    let uv_tex = uv * dims;
    let seam = floor(uv_tex + 0.5);
    let seam_width = max((abs(ddx) + abs(ddy)) * dims, vec2<f32>(1.0e-6));
    let aa = clamp((uv_tex - seam) / seam_width, vec2(-0.5), vec2(0.5));
    let uv_recon = (seam + aa) / dims;
    return textureSampleGrad(tex, samp, uv_recon, ddx, ddy);
}

fn has_direct_sh() -> bool {
    return (uniforms.has_direct & 1u) != 0u;
}

// Indirect (`.a`) and, when baked direct SH exists, direct (`.b`) through one
// shared corner blend. The lookup lifts off the surface along the viewer-side
// normal, and backface rejection is on, as for static world faces.
fn fluid_sample_sh(world_pos: vec3<f32>, n: vec3<f32>, want_direct: bool) -> ShDirPair {
    var result: ShDirPair;
    result.a = vec3<f32>(0.0);
    result.b = vec3<f32>(0.0);
    if sh_grid.has_sh_volume == 0u {
        return result;
    }
    const SH_NORMAL_OFFSET_M: f32 = 0.1;
    let offset_world = world_pos + n * SH_NORMAL_OFFSET_M * sh_grid.cell_size;
    let gdims_f = max(vec3<f32>(sh_grid.grid_dimensions) - vec3<f32>(1.0), vec3<f32>(0.0));
    let cell_coord = (offset_world - sh_grid.grid_origin) /
        max(sh_grid.cell_size, vec3<f32>(1.0e-6));
    let gf = clamp(cell_coord, vec3<f32>(0.0), gdims_f);
    let gi = vec3<u32>(floor(gf));
    let gfrac = fract(gf);
    let probe_occlusion = sh_grid.probe_occlusion != 0u;
    if want_direct {
        return sample_sh_indirect_direct_corners(
            sh_direct_atlas, gi, gfrac, offset_world, n, n, true, probe_occlusion,
        );
    }
    result.a = sample_sh_indirect_corners_depth_aware(
        gi, gfrac, offset_world, n, n, true, probe_occlusion,
    );
    return result;
}

// One runtime light's diffuse contribution. Scripted descriptors exist only
// for the dynamic prefix; promoted-static records pass `false`.
fn fluid_light(light_idx: u32, world_pos: vec3<f32>, n: vec3<f32>, scripted: bool) -> vec3<f32> {
    let influence = light_influence[light_idx];
    let inf_radius = influence.w;
    if inf_radius <= 1.0e30 {
        let d = world_pos - influence.xyz;
        if dot(d, d) > inf_radius * inf_radius {
            return vec3<f32>(0.0);
        }
    }

    let light = lights[light_idx];
    let light_type = bitcast<u32>(light.position_and_type.w);
    let falloff_model = bitcast<u32>(light.color_and_falloff_model.w);
    var effective_color = light.color_and_falloff_model.xyz;
    var effective_aim = light.direction_and_range.xyz;
    if scripted {
        let desc = scripted_light_descriptors[light_idx];
        if desc.is_active != 0u {
            let cycle_t = animation_curve_t(desc.period, desc.phase, uniforms.time);
            if desc.color_count > 0u {
                let unit_sample = max(
                    sample_color_catmull_rom(desc.color_offset, desc.color_count, cycle_t, desc.base_color),
                    vec3<f32>(0.0),
                );
                let intensity = light_eval_scripted_intensity_scalar(light.color_and_falloff_model.xyz, desc.base_color);
                let brightness = max(sample_curve_catmull_rom(desc.brightness_offset, desc.brightness_count, cycle_t), 0.0);
                effective_color = unit_sample * intensity * brightness;
            } else if desc.brightness_count > 0u {
                let brightness = max(sample_curve_catmull_rom(desc.brightness_offset, desc.brightness_count, cycle_t), 0.0);
                effective_color = light.color_and_falloff_model.xyz * brightness;
            }
            if light_type == 1u && desc.direction_count > 0u {
                effective_aim = light_eval_animated_direction(desc, cycle_t, effective_aim);
            }
        } else if light_eval_scripted_descriptor_present(desc) {
            effective_color = vec3<f32>(0.0);
        }
    }

    var L: vec3<f32>;
    var attenuation: f32;
    switch light_type {
        case 0u: {
            let to_light = light.position_and_type.xyz - world_pos;
            let dist = length(to_light);
            L = to_light / max(dist, 0.0001);
            attenuation = light_eval_falloff(dist, light.direction_and_range.w, falloff_model);
        }
        case 1u: {
            let to_light = light.position_and_type.xyz - world_pos;
            let dist = length(to_light);
            L = to_light / max(dist, 0.0001);
            attenuation = light_eval_falloff(dist, light.direction_and_range.w, falloff_model)
                * light_eval_cone_attenuation(L, effective_aim, light.cone_angles_and_pad.x, light.cone_angles_and_pad.y);
        }
        default: {
            L = -effective_aim;
            attenuation = 1.0;
        }
    }
    return effective_color * attenuation * max(dot(n, L), 0.0);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Derivatives in uniform control flow, from the unwarped UV, and the
    // world position's for the pixel reflection style's texel axes.
    let ddx = dpdx(in.uv);
    let ddy = dpdy(in.uv);
    let ddx_world = dpdx(in.world_position);
    let ddy_world = dpdy(in.world_position);
    let dims = vec2<f32>(textureDimensions(base_texture, 0));
    let texels_per_pixel = max(length(ddx * dims), length(ddy * dims));
    let row = fluid_volumes.slots[in.slot];
    let uv = fluid_warp_uv(in.uv, in.drift_scroll, dims, row.wave, uniforms.time, texels_per_pixel);
    let albedo = sample_post_retro(base_texture, aniso_sampler, uv, ddx, ddy);

    // Two-sided: shade the side the camera sees. For a planar face this is
    // exactly the back-face test, and it holds whatever the winding. Fog reads
    // the outward normal, never this viewer-flipped one.
    let V = normalize(uniforms.camera_position - in.world_position);
    let outward = normalize(in.world_normal);
    var n = outward;
    if dot(n, V) < 0.0 {
        n = -n;
    }

    const LIGHT_TERM_AMBIENT_FLOOR: u32 = 0x01u;
    const LIGHT_TERM_INDIRECT_STATIC: u32 = 0x02u;
    const LIGHT_TERM_INDIRECT_ANIMATED: u32 = 0x04u;
    const LIGHT_TERM_DYNAMIC_DIRECT: u32 = 0x20u;
    let light_terms = uniforms.light_term_mask;
    let use_ambient_floor = (light_terms & LIGHT_TERM_AMBIENT_FLOOR) != 0u;
    let use_indirect = (light_terms & (LIGHT_TERM_INDIRECT_STATIC | LIGHT_TERM_INDIRECT_ANIMATED)) != 0u;
    let use_dynamic = (light_terms & LIGHT_TERM_DYNAMIC_DIRECT) != 0u;

    // Baked terms. Static/animated isolation happens in the compose passes;
    // the composed direct atlas already has promoted static lights removed
    // while dynamic direct is on, so the loop below adds them back.
    let sh = fluid_sample_sh(in.world_position, n, has_direct_sh());
    var lighting = select(vec3<f32>(0.0), vec3<f32>(uniforms.ambient_floor), use_ambient_floor);
    if use_indirect {
        lighting = lighting + sh.a * uniforms.indirect_scale;
    }
    lighting = lighting + sh.b * uniforms.direct_scale;

    // Runtime lights: the dynamic prefix, then the promoted-static suffix. The
    // mover-only animated-baked tail between them is skipped; fluid takes those
    // lights from the composed direct atlas, as billboards do.
    if use_dynamic {
        for (var i: u32 = 0u; i < uniforms.light_count; i = i + 1u) {
            lighting = lighting + fluid_light(i, in.world_position, n, true);
        }
        let promoted_begin = min(
            uniforms.light_count + (uniforms.has_direct >> 1u),
            uniforms.total_light_count,
        );
        for (var i: u32 = promoted_begin; i < uniforms.total_light_count; i = i + 1u) {
            lighting = lighting + fluid_light(i, in.world_position, n, false);
        }
    }

    var rgb = albedo.rgb * lighting;
    if material.emissive_strength > 0.0 {
        rgb = rgb + sample_post_retro(emissive_texture, aniso_sampler, uv, ddx, ddy).rgb
            * material.emissive_strength;
    }
    let opacity = clamp(row.surface.x, 0.0, 1.0);
    let alpha = albedo.a * opacity;
    // Seen from outside a fogged fluid, the scene behind the face lies under
    // it: composite surface over fog over scene in one output. Clear fluids
    // and faces seen from inside keep exactly the surface.
    var surface = vec4<f32>(rgb, alpha);
    let extinction = row.surface.y;
    if extinction > 0.0 && fluid_fog_seen_from_outside(outward, V) {
        let depth = textureLoad(scene_depth, vec2<i32>(in.clip_position.xy), 0);
        let path = fluid_fog_path(uniforms.view_proj, uniforms.camera_position, in.world_position,
            depth);
        let transmittance = fluid_fog_transmittance(extinction, path);
        let proximity = fluid_fog_proximity(uniforms.camera_position, in.world_position, outward);
        let fog_opacity = fluid_fog_layer_opacity(fluid_scene.fog_weight, proximity, transmittance);
        surface = fluid_fog_composite(rgb, alpha, row.fog_albedo.rgb * lighting, fog_opacity);
    }
    // The reflection-free variant ends here; the block below folds away.
    if !FLUID_REFLECTIONS {
        return surface;
    }
    // A probe-bound face seen from above under the all-terms mask mirrors its
    // probe over that at union coverage; every other fragment is unchanged.
    // The reflection starts from the style's point and sees the eye from there.
    let reflectivity = row.surface.z;
    if !fluid_reflection_gate(in.probe, outward, V, uniforms.light_term_mask, reflectivity) {
        return surface;
    }
    // Cell-rate spike: the cell's reflection, evaluated once in compute, read
    // from the cell atlas at the key pixel style floors.
    if FLUID_REFLECTION_CELLS {
        let key = clamp(vec2<i32>(floor(in.uv * dims)), in.cell_bounds.xy, in.cell_bounds.zw);
        let cell = textureLoad(fluid_cell_atlas, key + in.cell_offset, 0);
        let cell_band = fluid_fog_proximity(uniforms.camera_position, in.world_position, outward);
        return fluid_reflection_composite(surface, cell.rgb, cell.a * cell_band);
    }
    let axes = fluid_reflection_texel_axes(ddx * dims, ddy * dims, ddx_world, ddy_world);
    let point = fluid_reflection_point(fluid_scene.reflection_style, in.uv * dims, in.world_position,
        outward, axes, uniforms.time, texels_per_pixel, fluid_probes.probes[in.probe].box_min,
        fluid_probes.probes[in.probe].box_max, fluid_probes.probes[in.probe].capture_point);
    let view = normalize(uniforms.camera_position - point.position);
    let reflection = fluid_reflection_sample_spike(in.probe, point.position, view, point.normal);
    let fresnel = fluid_reflection_fresnel(dot(view, outward), reflectivity) * reflection.weight;
    // From above, an eye within the fog's band of the plane scales the term
    // by its height through the band, the fog's own proximity, reaching 0 at
    // the plane: an eye bobbing there never flips the mirror.
    let band = fluid_fog_proximity(uniforms.camera_position, in.world_position, outward);
    return fluid_reflection_composite(surface, reflection.capture, fresnel * band);
}
