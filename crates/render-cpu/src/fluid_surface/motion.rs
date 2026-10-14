//! Fluid surface motion: the CPU reference for `fluid_surface.wgsl`'s warp.
//!
//! See: context/lib/rendering_pipeline.md §7.3a (Fluid Surface Pass).
//!
//! Motion moves the albedo sample point by whole texels on the face texture's
//! own texel grid, so a fluid keeps the grain a wall at the same scale has.
//! Three layers add:
//! - **Shimmer.** Each texel cell steps at [`FLUID_MOTION_STEP_HZ`] on its own
//!   hashed phase, so a view-filling pool never changes on one frame. Its
//!   offset cycles `0, +d, 0, −d` along the cell's own hashed direction `d`,
//!   so over any window it averages to zero within one step.
//! - **Rings.** Every block of the grid takes one drop per ring lifetime, at
//!   a hashed spot and phase, and spreads one ring per step, never past its
//!   own block, so a cell reads one block. A cell
//!   reads the ring at its own step, so its displayed offset changes only on
//!   its own step boundaries.
//! - **Drift.** A current scrolls the whole face by a whole-texel offset
//!   beneath the fixed cell grid ([`drift_scroll_periods`]).
//!
//! The offset fades to flat cell by cell as texels shrink below a pixel.
//! Every function here is GPU-free and pure; the shader mirrors each constant,
//! pinned by `render/fluid_surface_tests.rs`.

use glam::{IVec2, Vec2};

/// Cell step rate. Under the 3 Hz general flash threshold, with margin, so
/// a cell's offset changes at most every 0.4 s.
pub const FLUID_MOTION_STEP_HZ: f32 = 2.5;
/// Shimmer offset bound per texel axis, in texels.
pub const FLUID_SHIMMER_AMPLITUDE: i32 = 1;
/// Ring offset bound per texel axis, in texels.
pub const FLUID_RING_AMPLITUDE: i32 = 1;
/// Ring drop blocks are `1 << FLUID_RING_BLOCK_SHIFT` texels square.
pub const FLUID_RING_BLOCK_SHIFT: i32 = 5;
/// Steps a ring lives, its birth step included (which draws nothing).
pub const FLUID_RING_LIFETIME_STEPS: i32 = 5;
/// A drop lands at least this many texels inside its block's edges, so its
/// widest ring never leaves the block and a cell evaluates one block, not a
/// neighbourhood. Evaluating the 3x3 neighbourhood cost about 0.5 ms on a
/// view-filling 1080p pool (1660 Super).
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
/// `lowbias32` multipliers (Chris Wellons' integer hash).
pub const FLUID_HASH_MUL_A: u32 = 0x7FEB_352D;
pub const FLUID_HASH_MUL_B: u32 = 0x846C_A68B;

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

/// The ring reaching `cell` at `step`, if its block's drop has one there. A
/// ring pushes its cells one texel outward from its drop, at most
/// [`FLUID_RING_AMPLITUDE`] per axis.
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
/// pixel spans, from the unwarped UV derivatives).
pub fn faded_flat(cell: IVec2, texels_per_pixel: f32) -> bool {
    let threshold = FLUID_FADE_START_TEXELS_PER_PIXEL
        + (FLUID_FADE_END_TEXELS_PER_PIXEL - FLUID_FADE_START_TEXELS_PER_PIXEL)
            * hash_unit(hash_u32(cell_seed(cell) ^ FLUID_SALT_FADE));
    texels_per_pixel > threshold
}

/// The whole-texel offset `cell` displays at `time`: shimmer plus rings, both
/// keyed on the cell's own step, or zero once faded flat.
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

/// The texel-space sample point for a fragment at unwarped texel UV
/// `texel_uv`: drift beneath, then the cell offset keyed on the fixed grid.
pub fn warped_texel_uv(
    texel_uv: Vec2,
    drift_rate: Vec2,
    dims: Vec2,
    time: f32,
    texels_per_pixel: f32,
) -> Vec2 {
    let cell = texel_cell(texel_uv);
    texel_uv
        + drift_texels(drift_rate, time, dims)
        + displayed_offset(cell, time, texels_per_pixel).as_vec2()
}

#[cfg(test)]
#[path = "motion_tests.rs"]
mod tests;
