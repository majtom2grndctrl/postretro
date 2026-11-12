// Mover depth live-tile texels: which two moment sets a mover's t′ blends
// for one probe (the interval rule over its stored and base-reading poses,
// then the format's depth rule), the blend itself, several movers'
// differences from the base, and the f16 conversions. `depth.rs` owns the
// per-frame plan that calls it.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
//
// IP design rules (ip-372 §Design levers): mover state only blends moment
// sets, and several movers combine by adding differences, never by selecting
// one. It never flags a probe occluded, zeroes a weight or excludes a probe.

use postretro_level_format::mover_probe_channel::{
    DepthMix, DepthSource, depth_segment, mix_depth_tile,
};
use postretro_level_format::probe_depth_tiles::{
    PROBE_DEPTH_TILE_TEXEL_BYTES, PROBE_DEPTH_TILE_TEXELS, f16_bits_to_f32,
};

use super::knots::{Knot, interval};

const TEXEL_BYTES: usize = PROBE_DEPTH_TILE_TEXEL_BYTES as usize;
const TILE_BYTES: usize = PROBE_DEPTH_TILE_TEXELS * TEXEL_BYTES;
/// Values one CPU-held tile spans: its texels' means, then their second
/// moments, so each moment runs contiguous for the blend's vector lanes.
pub(super) const TILE_MOMENTS: usize = 2 * PROBE_DEPTH_TILE_TEXELS;
/// A record pose that stores no tile but reads the base (a terminus, or a
/// mid pose marked base), before [`super::depth::MoverDepthTiles::bind`]
/// points it at its probe's base tile. CPU-side only; never on the wire.
pub(super) const UNBOUND_BASE: u32 = u32::MAX;
/// A mid pose the record leaves unmarked: blended across.
pub(super) const ACROSS: u32 = u32::MAX - 1;
/// The largest finite binary16, the most a live-tile moment may hold: the
/// bake saturates a far texel's `E[d²]` to it, and a sum past it would
/// encode as infinity, which the sampler turns into NaN.
const F16_MAX: f64 = 65504.0;

/// One stated mover's depth blend at a probe: `from` toward `to` by
/// `weight`, each texel mixed by `mix` ([`mix_depth_tile`]).
#[derive(Clone, Copy, Debug)]
pub(super) struct DepthBlend<'a> {
    /// A tile of [`TILE_MOMENTS`] values: means, then second moments.
    pub(super) from: &'a [f32],
    pub(super) to: &'a [f32],
    pub(super) weight: f32,
    pub(super) mix: DepthMix,
}

/// The blend one record reads at `t` (a stated t′): `poses` holds each
/// pose's tile in `tiles` (a stored tile, the probe's bound `base`, or
/// [`ACROSS`]). The interval rule finds the two defined poses around `t`,
/// and the format's [`depth_segment`] says how depth crosses them: between
/// two termini, raw moments through `base` (D_lo → base → D_hi, reaching
/// the base at the interval's midpoint); otherwise the two poses' mean and
/// variance. Exact at every read knot. The compiler's skipping judges by the
/// same rule.
pub(super) fn segment<'a>(
    tiles: &'a [f32],
    poses: &[u32],
    knots: &[Knot],
    base: &'a [f32],
    t: f32,
) -> DepthBlend<'a> {
    let span = interval(knots, t, |pose| poses[pose as usize] != ACROSS);
    let blend = depth_segment(span, poses.len());
    let source = |source: DepthSource| match source {
        DepthSource::Pose(pose) => {
            let at = poses[pose as usize];
            debug_assert!(at < ACROSS, "a bound level points every read pose");
            &tiles[at as usize..at as usize + TILE_MOMENTS]
        }
        DepthSource::Base => base,
    };
    DepthBlend {
        from: source(blend.from),
        to: source(blend.to),
        weight: blend.weight,
        mix: blend.mix,
    }
}

/// Append one probe's live tile to `out` as texel bytes, from each stated
/// mover's blend ([`segment`]). One stated mover writes its blend itself:
/// exact at weight 0 and 1, and exactly the tile over any segment whose ends
/// are the same tile. Several write, per texel and per moment, the base plus
/// each mover's blended difference from the base, then clamp to a valid
/// moment set: `E[d] ≥ 0`, `E[d²] ≥ E[d]²` (owner, 2026-11-09), each moment
/// then capped at the largest finite f16, so a saturated far texel (`E[d²]`
/// stored as 65504 below `E[d]²`) returns exactly and nothing encodes as
/// infinity. Differences add, never select, so the result is independent of
/// hand-over order. The sum runs in f64, where every difference of two f32
/// moments is exact: a mover whose blend equals the base adds exactly zero,
/// and a single non-zero difference returns that mover's blend bit for bit
/// wherever the blend is itself a valid moment set.
pub(super) fn blend_probe<'a>(
    base: &[f32],
    mut stated: impl Iterator<Item = DepthBlend<'a>>,
    out: &mut Vec<u8>,
) {
    let first = stated.next().expect("a written probe has a stated mover");
    let at = out.len();
    out.resize(at + TILE_BYTES, 0);
    let (tile, _) = out[at..].as_chunks_mut::<TEXEL_BYTES>();
    let mut blended = [0.0f32; TILE_MOMENTS];
    let Some(second) = stated.next() else {
        let blend = first;
        let source = if blend.weight <= 0.0 || std::ptr::eq(blend.from, blend.to) {
            blend.from
        } else if blend.weight >= 1.0 {
            blend.to
        } else {
            mix_depth_tile(blend.from, blend.to, blend.weight, blend.mix, &mut blended);
            &blended
        };
        let (means, second) = source.split_at(PROBE_DEPTH_TILE_TEXELS);
        for ((texel, &mean), &mean_sq) in tile.iter_mut().zip(means).zip(second) {
            *texel = texel_bytes([mean, mean_sq]);
        }
        return;
    };
    let mut sum = [0.0f64; TILE_MOMENTS];
    for blend in [first, second].into_iter().chain(stated) {
        mix_depth_tile(blend.from, blend.to, blend.weight, blend.mix, &mut blended);
        for ((sum, &blended), &base) in sum.iter_mut().zip(&blended).zip(base) {
            *sum += f64::from(blended) - f64::from(base);
        }
    }
    let (mean_sums, second_sums) = sum.split_at(PROBE_DEPTH_TILE_TEXELS);
    let (base_means, base_second) = base.split_at(PROBE_DEPTH_TILE_TEXELS);
    for (texel, i) in tile.iter_mut().zip(0..) {
        let mean = (f64::from(base_means[i]) + mean_sums[i]).clamp(0.0, F16_MAX);
        let mean_sq = (f64::from(base_second[i]) + second_sums[i])
            .max(mean * mean)
            .min(F16_MAX);
        *texel = texel_bytes([mean as f32, mean_sq as f32]);
    }
}

/// One `Rg16Float` texel's little-endian bytes.
fn texel_bytes([mean, mean_sq]: [f32; 2]) -> [u8; TEXEL_BYTES] {
    (u32::from(f32_to_f16(mean)) | (u32::from(f32_to_f16(mean_sq)) << 16)).to_le_bytes()
}

/// Binary16 bits → f32, exact; normal values by bit shuffle.
pub(super) fn f16_to_f32(bits: u16) -> f32 {
    let exponent = u32::from((bits >> 10) & 0x1f);
    if exponent == 0 || exponent == 0x1f {
        return f16_bits_to_f32(bits);
    }
    f32::from_bits(
        (u32::from(bits & 0x8000) << 16)
            | ((exponent + 112) << 23)
            | (u32::from(bits & 0x3ff) << 13),
    )
}

/// f32 → binary16 bits, round to nearest even, branch-free so the live
/// tile's texel loops vectorise: each range is computed and the right one
/// selected. The normal range by integer rounding of the f32 bits; below
/// 2^-14, adding 0.5 (whose f32 step is 2^-24, the subnormal step) leaves
/// the subnormal mantissa in the low bits; past 65520 infinity, NaN quiet.
pub(super) fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = (bits >> 16) & 0x8000;
    let magnitude = bits & 0x7fff_ffff;
    let normal =
        (magnitude.wrapping_add(0x0fff + ((magnitude >> 13) & 1)) >> 13).wrapping_sub(112 << 10);
    let subnormal = (f32::from_bits(magnitude) + 0.5)
        .to_bits()
        .wrapping_sub(0.5f32.to_bits());
    let half = if magnitude < 0x3880_0000 {
        subnormal
    } else if magnitude < 0x477f_f000 {
        normal
    } else if magnitude <= 0x7f80_0000 {
        0x7c00
    } else {
        0x7e00
    };
    (sign | half) as u16
}

#[cfg(test)]
#[path = "depth_blend_tests.rs"]
mod tests;
