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

use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::mover_probe_channel::{
    DepthMix, DepthSource, depth_segment, mix_depth_moments,
};
use postretro_level_format::probe_depth_tiles::{
    PROBE_DEPTH_TILE_TEXEL_BYTES, PROBE_DEPTH_TILE_TEXELS, f16_bits_to_f32,
};

use super::knots::{Knot, interval};

const TEXEL_BYTES: usize = PROBE_DEPTH_TILE_TEXEL_BYTES as usize;
const TILE_BYTES: usize = PROBE_DEPTH_TILE_TEXELS * TEXEL_BYTES;
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
/// `weight`, each texel mixed by `mix` ([`mix_depth_moments`]).
#[derive(Clone, Copy, Debug)]
pub(super) struct DepthBlend<'a> {
    pub(super) from: &'a [[f32; 2]],
    pub(super) to: &'a [[f32; 2]],
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
    tiles: &'a [[f32; 2]],
    poses: &[u32],
    knots: &[Knot],
    base: &'a [[f32; 2]],
    t: f32,
) -> DepthBlend<'a> {
    let span = interval(knots, t, |pose| poses[pose as usize] != ACROSS);
    let blend = depth_segment(span, poses.len());
    let source = |source: DepthSource| match source {
        DepthSource::Pose(pose) => {
            let at = poses[pose as usize];
            debug_assert!(at < ACROSS, "a bound level points every read pose");
            &tiles[at as usize..at as usize + PROBE_DEPTH_TILE_TEXELS]
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
    base: &[[f32; 2]],
    mut stated: impl Iterator<Item = DepthBlend<'a>>,
    out: &mut Vec<u8>,
) {
    let first = stated.next().expect("a written probe has a stated mover");
    let at = out.len();
    out.resize(at + TILE_BYTES, 0);
    let (tile, _) = out[at..].as_chunks_mut::<TEXEL_BYTES>();
    let Some(second) = stated.next() else {
        let blend = first;
        if blend.weight <= 0.0 || blend.weight >= 1.0 || std::ptr::eq(blend.from, blend.to) {
            let source = if blend.weight >= 1.0 {
                blend.to
            } else {
                blend.from
            };
            for (texel, &moments) in tile.iter_mut().zip(source) {
                *texel = texel_bytes(moments);
            }
        } else {
            // One loop per mix, so each inlines its own arithmetic.
            let s = blend.weight;
            let texels = tile.iter_mut().zip(blend.from).zip(blend.to);
            match blend.mix {
                DepthMix::Raw => {
                    for ((texel, &a), &b) in texels {
                        *texel = texel_bytes(mix_depth_moments(a, b, s, DepthMix::Raw));
                    }
                }
                DepthMix::MeanVariance => {
                    for ((texel, &a), &b) in texels {
                        *texel = texel_bytes(mix_depth_moments(a, b, s, DepthMix::MeanVariance));
                    }
                }
            }
        }
        return;
    };
    let mut sum = [[0.0f64; 2]; PROBE_DEPTH_TILE_TEXELS];
    for blend in [first, second].into_iter().chain(stated) {
        let (s, mix) = (blend.weight, blend.mix);
        for (((texel, base), &a), &b) in sum.iter_mut().zip(base).zip(blend.from).zip(blend.to) {
            let blended = mix_depth_moments(a, b, s, mix);
            for moment in 0..2 {
                texel[moment] += f64::from(blended[moment]) - f64::from(base[moment]);
            }
        }
    }
    for ((texel, difference), base) in tile.iter_mut().zip(&sum).zip(base) {
        let mean = (f64::from(base[0]) + difference[0]).clamp(0.0, F16_MAX);
        let mean_sq = (f64::from(base[1]) + difference[1])
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

/// f32 → binary16 bits, round to nearest even; the normal range by integer
/// rounding, everything else through the shared converter.
pub(super) fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = (bits >> 16) & 0x8000;
    let magnitude = bits & 0x7fff_ffff;
    // [2^-14, 65520): normal binary16 results, no overflow to infinity.
    if (0x3880_0000..0x477f_f000).contains(&magnitude) {
        let rounded = magnitude.wrapping_add(0x0fff + ((magnitude >> 13) & 1)) >> 13;
        return (sign | rounded.wrapping_sub(112 << 10)) as u16;
    }
    f32_to_f16_bits(value)
}

#[cfg(test)]
#[path = "depth_blend_tests.rs"]
mod tests;
