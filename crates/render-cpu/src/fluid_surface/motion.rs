// Fluid surface motion: the pure, GPU-free CPU reference for `fluid_surface.wgsl`'s warp.
// See: context/lib/rendering_pipeline.md §7.3a

use glam::{IVec2, Vec2};

// The shader mirrors each constant below; `render/fluid_surface_tests.rs` pins
// them against it.

/// Cell step rate. Under the 3 Hz general flash threshold, with margin, so
/// a cell's offset changes at most every 0.4 s.
pub const FLUID_MOTION_STEP_HZ: f32 = 2.5;
/// Pinned per-axis bound, in texels, on a shimmer offset. The shimmer
/// directions' `{−1, 0, 1}` components guarantee it; nothing reads this
/// constant to size them. Tests and the shader-constant pin hold the bound.
pub const FLUID_SHIMMER_AMPLITUDE: i32 = 1;
/// Pinned per-axis bound, in texels, on a ring offset. The rounded unit-vector
/// push guarantees it; nothing reads this constant to size the push. Tests and
/// the shader-constant pin hold the bound.
pub const FLUID_RING_AMPLITUDE: i32 = 1;
/// Ring drop blocks are `1 << FLUID_RING_BLOCK_SHIFT` texels square.
pub const FLUID_RING_BLOCK_SHIFT: i32 = 5;
/// Steps a ring lives, its birth step included (which draws nothing).
pub const FLUID_RING_LIFETIME_STEPS: i32 = 5;
/// A drop lands at least this many texels inside its block's edges, so its
/// widest ring never leaves the block, so a cell reads one block instead of
/// the nine around it.
pub const FLUID_RING_MARGIN: i32 = 9;
/// Ring radius growth per step, texels.
pub const FLUID_RING_STEP_TEXELS: f32 = 2.0;
/// Texels per screen pixel where cells start fading flat, and where every
/// cell is flat. Each cell flattens at its own hashed point in the band, so
/// a camera moving away never flattens a pool on one frame.
pub const FLUID_FADE_START_TEXELS_PER_PIXEL: f32 = 0.75;
pub const FLUID_FADE_END_TEXELS_PER_PIXEL: f32 = 1.5;

/// Hash salts. A cell and a block each hash their coordinates once into a
/// seed; every per-cell or per-block draw rehashes the seed with its own salt.
pub const FLUID_SALT_CELL: u32 = 0x9E37_79B9;
pub const FLUID_SALT_SHIMMER: u32 = 0x85EB_CA6B;
pub const FLUID_SALT_FADE: u32 = 0xC2B2_AE35;
pub const FLUID_SALT_BLOCK: u32 = 0x27D4_EB2F;
pub const FLUID_SALT_DROP: u32 = 0x1656_67B1;
/// Wave crest-pair salts, one per axis, so the `u` and `v` patterns differ.
pub const FLUID_SALT_WAVE_U: u32 = 0x4C95_7F2D;
pub const FLUID_SALT_WAVE_V: u32 = 0x5851_F42D;
/// Crest pairs per hash-repeat span: a power of two, so a pair hashes on
/// `pair & (FLUID_WAVE_PAIRS − 1)` and the phase count wraps by masking. The
/// layout repeats every 4096 pairs: 65 536 texels at a 16-texel pair.
pub const FLUID_WAVE_PAIRS: i32 = 4096;
/// Half-arc `h(x) = g · (LINEAR + QUADRATIC · g)`, `g = x(1 − x)`: zero at
/// both ends, 1 at the middle, peak slope ≈ 3.043 per unit arc (a parabola's
/// is 4), so a crest of 8 × amplitude peaks at ≈ 0.761 texel per cell.
pub const FLUID_WAVE_ARC_LINEAR: f32 = 3.0;
pub const FLUID_WAVE_ARC_QUADRATIC: f32 = 4.0;
/// `lowbias32` multipliers (Chris Wellons' integer hash).
pub const FLUID_HASH_MUL_A: u32 = 0x7FEB_352D;
pub const FLUID_HASH_MUL_B: u32 = 0x846C_A68B;

const _: () = assert!(
    FLUID_WAVE_PAIRS > 0 && FLUID_WAVE_PAIRS & (FLUID_WAVE_PAIRS - 1) == 0,
    "the wave's pair span must be a power of two to wrap by masking"
);

const _: () = assert!(
    (FLUID_RING_LIFETIME_STEPS - 1) as f32 * FLUID_RING_STEP_TEXELS + 0.5
        <= FLUID_RING_MARGIN as f32
        && 2 * FLUID_RING_MARGIN < (1 << FLUID_RING_BLOCK_SHIFT),
    "a drop's widest ring must stay inside its block"
);

/// Integer avalanche hash, as `fluid_hash_u32` in the shader.
pub fn hash_u32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(FLUID_HASH_MUL_A);
    x ^= x >> 15;
    x = x.wrapping_mul(FLUID_HASH_MUL_B);
    x ^= x >> 16;
    x
}

/// Three words into one hash, as `fluid_hash3`.
pub fn hash3(a: u32, b: u32, c: u32) -> u32 {
    hash_u32(a ^ hash_u32(b ^ hash_u32(c)))
}

/// A hash as a uniform `[0, 1)` value from its top 24 bits.
pub fn hash_unit(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// The cell's seed: its phase, and the root of its other draws.
fn cell_seed(cell: IVec2) -> u32 {
    hash3(cell.x as u32, cell.y as u32, FLUID_SALT_CELL)
}

/// The texel cell a texel-space UV lies in. Floored, so cells −1 and 0 are
/// distinct across the origin.
pub fn texel_cell(texel_uv: Vec2) -> IVec2 {
    texel_uv.floor().as_ivec2()
}

/// The cell's step index at `time` seconds: its own phase shifts every
/// boundary, so neighbouring cells step on different frames.
pub fn cell_step(cell: IVec2, time: f32) -> i32 {
    let phase = hash_unit(cell_seed(cell));
    (time * FLUID_MOTION_STEP_HZ + phase).floor() as i32
}

/// Shimmer offset at `step`: a four-step cycle `0, +d, 0, −d`, with `d` one of
/// the nine directions with components in `{−1, 0, 1}`, fixed per cell. A
/// direction redrawn per cycle would let a window catch an unmatched `−d` at
/// one end and an unrelated `+d` at the other, doubling its worst mean.
pub fn shimmer_offset(cell: IVec2, step: i32) -> IVec2 {
    let position = step & 3;
    if position & 1 == 0 {
        return IVec2::ZERO;
    }
    let h = hash_u32(cell_seed(cell) ^ FLUID_SALT_SHIMMER);
    let direction = (h >> 8) % 9;
    let d = IVec2::new((direction % 3) as i32 - 1, (direction / 3) as i32 - 1);
    if position == 1 { d } else { -d }
}

/// The ring reaching `cell` at `step`, if its block's drop has one there. Every
/// block takes one drop per ring lifetime, at a hashed spot and phase, and
/// spreads one ring per step, never past its own block, so a cell reads one
/// block. A ring pushes its cells one texel outward from its drop (a rounded
/// unit vector, so within [`FLUID_RING_AMPLITUDE`] per axis). A cell reads the
/// ring at its own step, so the offset changes only on its own step boundaries.
pub fn ring_offset(cell: IVec2, step: i32) -> IVec2 {
    let block = cell >> FLUID_RING_BLOCK_SHIFT;
    let seed = hash3(block.x as u32, block.y as u32, FLUID_SALT_BLOCK);
    let shifted = step + (seed % FLUID_RING_LIFETIME_STEPS as u32) as i32;
    let epoch = shifted.div_euclid(FLUID_RING_LIFETIME_STEPS);
    let age = shifted.rem_euclid(FLUID_RING_LIFETIME_STEPS);
    if age == 0 {
        return IVec2::ZERO;
    }
    let position = hash_u32(seed ^ hash_u32(epoch as u32 ^ FLUID_SALT_DROP));
    let span = ((1 << FLUID_RING_BLOCK_SHIFT) - 2 * FLUID_RING_MARGIN) as u32;
    let center = (block << FLUID_RING_BLOCK_SHIFT)
        + IVec2::splat(FLUID_RING_MARGIN)
        + IVec2::new((position % span) as i32, ((position >> 16) % span) as i32);
    let diff = (cell - center).as_vec2();
    let distance_sq = diff.length_squared();
    let radius = age as f32 * FLUID_RING_STEP_TEXELS;
    // |distance - radius| < 0.5, without a square root off the ring.
    if distance_sq <= (radius - 0.5) * (radius - 0.5)
        || distance_sq >= (radius + 0.5) * (radius + 0.5)
    {
        return IVec2::ZERO;
    }
    (diff / distance_sq.sqrt() + 0.5).floor().as_ivec2()
}

/// Whether `cell` has faded flat at `texels_per_pixel` (texels one screen
/// pixel spans, from the unwarped UV derivatives). Each cell flattens at its
/// own hashed point in the fade band, so a pool never flattens on one frame.
///
/// Accepted limit: the decision is per fragment with no hysteresis, and a
/// stateless shader cannot add any. The 3 Hz bound governs the animation clock
/// at a fixed view. A camera move (dolly, bob) can cross a cell's fade
/// threshold off its step boundaries, and a cell straddling two pixel quads can
/// split. Those are view changes, like mip or LOD transitions, and happen only
/// where texels are about a pixel.
pub fn faded_flat(cell: IVec2, texels_per_pixel: f32) -> bool {
    let threshold = FLUID_FADE_START_TEXELS_PER_PIXEL
        + (FLUID_FADE_END_TEXELS_PER_PIXEL - FLUID_FADE_START_TEXELS_PER_PIXEL)
            * hash_unit(hash_u32(cell_seed(cell) ^ FLUID_SALT_FADE));
    texels_per_pixel > threshold
}

/// The whole-texel offset `cell` displays at `time`: shimmer plus rings, both
/// keyed on the cell's own step, or zero once faded flat. Offsets are whole
/// texels on the face texture's own grid, so a fluid keeps the grain a wall at
/// the same scale has.
pub fn displayed_offset(cell: IVec2, time: f32, texels_per_pixel: f32) -> IVec2 {
    if faded_flat(cell, texels_per_pixel) {
        return IVec2::ZERO;
    }
    let step = cell_step(cell, time);
    shimmer_offset(cell, step) + ring_offset(cell, step)
}

/// Keep the high 12 significant bits of `x`, so two halves multiply exactly.
fn high_half(x: f32) -> f32 {
    f32::from_bits(x.to_bits() & 0xFFFF_F000)
}

/// `fract(a * b)` without the product's rounding: each factor splits into two
/// 12-bit halves whose four partial products are exact in f32.
pub fn exact_fract_product(a: f32, b: f32) -> f32 {
    let (a_hi, b_hi) = (high_half(a), high_half(b));
    let (a_lo, b_lo) = (a - a_hi, b - b_hi);
    let fract = |x: f32| x - x.floor();
    fract(fract(a_hi * b_hi) + fract(a_hi * b_lo) + fract(a_lo * b_hi) + fract(a_lo * b_lo))
}

/// The drift scroll at `time`, in texture periods, wrapped to `[−0.5, 0.5)`
/// so the warped UV stays within half a period of the unwarped one. `rate`
/// is the face's scroll in periods per second. Wrapping by whole periods
/// keeps the sample on the same texel, since textures repeat.
pub fn drift_scroll_periods(rate: Vec2, time: f32) -> Vec2 {
    let wrap = |rate: f32| {
        let periods = exact_fract_product(time, rate) + 0.5;
        periods - periods.floor() - 0.5
    };
    Vec2::new(wrap(rate.x), wrap(rate.y))
}

/// The drift scroll at `time` as a whole-texel offset for a texture of
/// `dims` texels.
pub fn drift_texels(rate: Vec2, time: f32, dims: Vec2) -> Vec2 {
    (drift_scroll_periods(rate, time) * dims).floor()
}

/// `a * b` as a whole count and a fraction in `[0, 1)`, both from one split:
/// the four exact partial products of [`exact_fract_product`] give their
/// floors to the count and their fractions to the fraction, and the
/// fractions' carry moves into the count, so the two never disagree.
pub fn exact_split_product(a: f32, b: f32) -> (i32, f32) {
    let (a_hi, b_hi) = (high_half(a), high_half(b));
    let (a_lo, b_lo) = (a - a_hi, b - b_hi);
    let (p0, p1, p2, p3) = (a_hi * b_hi, a_hi * b_lo, a_lo * b_hi, a_lo * b_lo);
    let (f0, f1, f2, f3) = (p0.floor(), p1.floor(), p2.floor(), p3.floor());
    let fraction = (p0 - f0) + (p1 - f1) + (p2 - f2) + (p3 - f3);
    let carry = fraction.floor();
    let count = f0 as i32 + f1 as i32 + f2 as i32 + f3 as i32 + carry as i32;
    (count, fraction - carry)
}

/// A fluid's wave, as its params row packs it. All zero is no wave.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FluidWaveParams {
    /// `1 / wavePeriod`; 0 is no wave.
    pub inv_period: f32,
    /// Peak sideways shift, texels; 0 is no wave.
    pub amplitude: f32,
    /// Crest length range, texels.
    pub length_min: f32,
    pub length_max: f32,
}

impl FluidWaveParams {
    pub const NONE: Self = Self {
        inv_period: 0.0,
        amplitude: 0.0,
        length_min: 0.0,
        length_max: 0.0,
    };
}

/// The crest at `position` (texels along the wave's axis) at `phase`: `x` is
/// the distance into the crest, `y` its length. Crests come in pairs spanning
/// exactly two mean lengths, the first `mean + d` and the second `mean − d`,
/// with `d` hashed per pair, so every crest lies in the length range and a
/// pair is found in O(1). The layout translates by one mean length per period.
///
/// `phase` is the time in mean lengths, `time · inv_period`, split exactly
/// into a whole count and a fraction ([`exact_split_product`]). The count
/// wraps over whole hash-repeat spans and splits into pairs and a parity, so
/// pair indices stay integers and every f32 here stays small at any script
/// time. Wrapping over whole spans, never one period, keeps every crest's
/// hashed length across the wrap.
pub fn wave_crest(position: f32, phase: (i32, f32), wave: FluidWaveParams, salt: u32) -> Vec2 {
    let (count, fraction) = phase;
    let count = count & (2 * FLUID_WAVE_PAIRS - 1);
    let pairs = count >> 1;
    let parity = (count & 1) as f32;
    let pair_length = wave.length_min + wave.length_max;
    let pair_position = position / pair_length - (parity + fraction) * 0.5;
    let whole = pair_position.floor();
    let pair = (whole as i32 - pairs) & (FLUID_WAVE_PAIRS - 1);
    let into_pair = (pair_position - whole) * pair_length;
    let d = (2.0 * hash_unit(hash_u32(pair as u32 ^ salt)) - 1.0)
        * (wave.length_max - wave.length_min)
        * 0.5;
    let first = 0.5 * pair_length + d;
    if into_pair < first {
        Vec2::new(into_pair, first)
    } else {
        Vec2::new(into_pair - first, pair_length - first)
    }
}

/// A crest's signed shape at `crest` ([`wave_crest`]): a positive half-arc
/// then a negative one, each half the crest long, each zero at both ends, so
/// crests of different lengths join continuously.
pub fn wave_arc(crest: Vec2) -> f32 {
    let half = 0.5 * crest.y;
    let second = crest.x >= half;
    let x = (crest.x - if second { half } else { 0.0 }) / half;
    let g = x * (1.0 - x);
    let h = g * (FLUID_WAVE_ARC_LINEAR + FLUID_WAVE_ARC_QUADRATIC * g);
    if second { -h } else { h }
}

/// The wave's continuous shift, in texels, at `position` along its axis and
/// `phase` ([`wave_crest`]), before rounding. Exactly zero with no amplitude
/// or no period: a real guard, as the crest divides by the pair length.
pub fn wave_value(position: f32, phase: (i32, f32), wave: FluidWaveParams, salt: u32) -> f32 {
    if wave.amplitude == 0.0 || wave.inv_period == 0.0 {
        return 0.0;
    }
    wave.amplitude * wave_arc(wave_crest(position, phase, wave, salt))
}

/// The wave's whole-texel offset for `cell` at `time`: the `u` shift keyed on
/// the cell's `v`, the `v` shift on its `u`, each sampled at the cell centre
/// on the fixed grid from one phase split. Texels count, not texture periods,
/// so the offset is the same whatever the texture's size.
pub fn wave_offset(cell: IVec2, time: f32, wave: FluidWaveParams) -> IVec2 {
    let phase = exact_split_product(time, wave.inv_period);
    let shift = Vec2::new(
        wave_value(cell.y as f32 + 0.5, phase, wave, FLUID_SALT_WAVE_U),
        wave_value(cell.x as f32 + 0.5, phase, wave, FLUID_SALT_WAVE_V),
    );
    (shift + 0.5).floor().as_ivec2()
}

/// The texel-space sample point for a fragment at unwarped texel UV
/// `texel_uv`: drift beneath, then the cell offsets keyed on the fixed grid.
/// Four layers add: shimmer and rings per cell, fading to flat as texels
/// shrink below a pixel; the declared wave per cell, which never fades; and
/// drift scrolling the whole face beneath the fixed cell grid
/// ([`drift_scroll_periods`]). The wave sits beside drift, outside
/// [`displayed_offset`], so the flash guards measure shimmer and rings alone.
pub fn warped_texel_uv(
    texel_uv: Vec2,
    drift_rate: Vec2,
    dims: Vec2,
    wave: FluidWaveParams,
    time: f32,
    texels_per_pixel: f32,
) -> Vec2 {
    let cell = texel_cell(texel_uv);
    texel_uv
        + drift_texels(drift_rate, time, dims)
        + wave_offset(cell, time, wave).as_vec2()
        + displayed_offset(cell, time, texels_per_pixel).as_vec2()
}

#[cfg(test)]
#[path = "motion_tests.rs"]
mod tests;
