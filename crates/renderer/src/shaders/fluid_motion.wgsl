// Fluid surface motion: whole-texel shimmer, rings, fade, drift and wave on
// the face texture's own grid. Concatenated after fluid_surface.wgsl.
// See: context/lib/rendering_pipeline.md §7.3a

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
