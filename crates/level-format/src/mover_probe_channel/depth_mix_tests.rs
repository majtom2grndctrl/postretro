// The depth rule shared by the renderer's live tiles and the compiler's
// redundant-pose skipping: which two moment sets an interval blends, and
// how each texel mixes them.

use super::*;
use crate::lightmap::f32_to_f16_bits;
use crate::probe_depth_tiles::f16_bits_to_f32;

fn span(lo: u32, hi: u32, weight: f32) -> PoseInterval {
    PoseInterval { lo, hi, weight }
}

/// The variance a texel holds against its own mean.
fn variance([mean, mean_sq]: [f32; 2]) -> f32 {
    mean_sq - mean * mean
}

const WEIGHTS: [f32; 9] = [0.01, 0.1, 0.25, 0.3, 0.5, 0.6, 0.75, 0.9, 0.99];

// An interval reads each end exactly: weight 0 reads its low pose, and the
// texel mix returns `from` at s 0 and `to` at s 1 under either mix, bit for
// bit, whatever the moments hold (saturated far texels included).
#[test]
fn an_interval_reads_each_end_exactly() {
    for pose_count in [2, 5] {
        for (lo, hi) in [(0, 1), (1, 3), (0, pose_count - 1)] {
            let segment = depth_segment(span(lo, hi, 0.0), pose_count as usize);
            assert_eq!(segment.from, DepthSource::Pose(lo));
            assert_eq!(segment.to, DepthSource::Pose(lo));
            assert_eq!(segment.weight, 0.0);
        }
    }
    let ends = [
        ([1.5, 2.5], [4.0, 17.0]),
        ([300.0, 65504.0], [2.0, 4.0]),
        ([10.0, 99.0], [0.0, 0.0]),
        ([0.125, 0.0156], [65504.0, 65504.0]),
    ];
    for mix in [DepthMix::Raw, DepthMix::MeanVariance] {
        for (a, b) in ends {
            assert_eq!(mix_depth_moments(a, b, 0.0, mix), a, "{mix:?} s 0");
            assert_eq!(mix_depth_moments(a, b, 1.0, mix), b, "{mix:?} s 1");
            assert_eq!(mix_depth_moments(a, a, 0.4, mix), a, "{mix:?} same ends");
        }
    }
}

// Any interval with a mid pose at either end blends mean and variance
// between its own two poses; it never runs through the base.
#[test]
fn an_interval_touching_a_mid_pose_blends_mean_and_variance_between_its_poses() {
    for (lo, hi) in [(0, 1), (1, 2), (2, 4), (1, 4), (0, 3)] {
        for weight in WEIGHTS {
            assert_eq!(
                depth_segment(span(lo, hi, weight), 5),
                DepthSegment {
                    from: DepthSource::Pose(lo),
                    to: DepthSource::Pose(hi),
                    weight,
                    mix: DepthMix::MeanVariance,
                },
                "poses {lo}–{hi} at {weight}"
            );
        }
    }
}

// One occluder translating along a texel's ray: the hit distance moves
// linearly with the mover and its spread does not change. The blend's mean
// follows the occluder exactly and its variance stays the ends' variance,
// where a raw-moment lerp adds s(1 − s)·Δμ².
#[test]
fn a_translating_occluder_composes_its_moved_mean_with_no_added_variance() {
    for spread in [0.0, 0.25] {
        let at = |mean: f32| [mean, mean * mean + spread];
        let (near, far) = (at(2.0), at(6.0));
        for s in [0.25, 0.5, 0.75] {
            let blended = mix_depth_moments(near, far, s, DepthMix::MeanVariance);
            let mean = 2.0 + 4.0 * s;
            assert_eq!(blended[0], mean, "mean at s {s}");
            assert_eq!(variance(blended), spread, "no added variance at s {s}");
            let raw = mix_depth_moments(near, far, s, DepthMix::Raw);
            let added = s * (1.0 - s) * 16.0;
            assert!(
                (variance(raw) - spread - added).abs() < 1e-4,
                "raw moments add s(1 − s)·Δμ² at s {s}: {}",
                variance(raw)
            );
        }
    }
}

// The mean is stored rounded to f16, and E[d²] is rebuilt from that rounded
// mean, so the stored pair's variance is the blended variance. A third of
// the way from 1 to the next f16 up rounds the mean back to 1.
#[test]
fn the_rebuilt_second_moment_follows_the_f16_rounded_mean() {
    let next = 1.0 + 2f32.powi(-10);
    let (a, b) = ([1.0, 1.5], [next, 1.6]);
    let blended = mix_depth_moments(a, b, 1.0 / 3.0, DepthMix::MeanVariance);
    assert_eq!(blended[0], 1.0, "the mean rounds to its f16");
    let (variance_a, variance_b) = (0.5f32, 1.6 - next * next);
    let blended_variance = variance_a + (variance_b - variance_a) / 3.0;
    assert!(
        (blended[1] - (blended_variance + 1.0)).abs() < 1e-6,
        "E[d²] {} rebuilt from the rounded mean, against {}",
        blended[1],
        blended_variance + 1.0
    );
}

// σ² never goes negative: an end whose stored E[d²] sits below E[d]² (f16
// rounding, or a saturated far texel) reads variance 0, and between two
// different texels the result's E[d²] never falls below its own mean
// squared (a texel blended with itself returns itself, as the raw lerp
// does). A saturated far texel
// (E[d²] stored as 65504, below E[d]²) blends to 65504 against another, and
// nothing passes the largest finite f16.
#[test]
fn variance_never_goes_negative_and_saturated_texels_return_exactly() {
    let samples = [
        [0.0, 0.0],
        [0.5, 0.2],
        [1.0, 0.999],
        [3.0, 9.5],
        [10.0, 99.0],
        [40.0, 1700.0],
        [255.9, 65504.0],
        [300.0, 65504.0],
        [310.0, 65504.0],
    ];
    for a in samples {
        for b in samples.into_iter().filter(|&b| b != a) {
            for s in WEIGHTS {
                let blended = mix_depth_moments(a, b, s, DepthMix::MeanVariance);
                assert!(blended[1] <= 65504.0, "{a:?} {b:?} {s}: {blended:?}");
                if blended[1] < 65504.0 {
                    assert!(
                        variance(blended) >= 0.0,
                        "{a:?} {b:?} at {s}: variance {}",
                        variance(blended)
                    );
                }
            }
        }
    }
    let (far, farther) = ([300.0, 65504.0], [310.0, 65504.0]);
    for s in WEIGHTS {
        assert_eq!(
            mix_depth_moments(far, farther, s, DepthMix::MeanVariance)[1],
            65504.0,
            "two saturated texels at {s}"
        );
        assert_eq!(
            mix_depth_moments(far, far, s, DepthMix::MeanVariance),
            far,
            "a saturated texel against itself at {s}"
        );
    }
}

/// The epoch-5 depth leg, restated: D_lo → base → D_hi with exact weights.
fn through_base(lo: [f32; 2], base: [f32; 2], hi: [f32; 2], w: f32) -> [f32; 2] {
    let lerp = |a: [f32; 2], b: [f32; 2], s: f32| {
        if s >= 1.0 {
            b
        } else {
            [0, 1].map(|m| a[m] + s * (b[m] - a[m]))
        }
    };
    if w <= 0.0 {
        lo
    } else if w <= 0.5 {
        lerp(lo, base, 2.0 * w)
    } else {
        lerp(base, hi, 2.0 * w - 1.0)
    }
}

// A termini ↔ termini interval (a two-pose mover, or a probe whose every
// mid pose blends across) keeps the raw through-base blend bit for bit.
#[test]
fn a_termini_interval_equals_the_through_base_blend_bit_for_bit() {
    let (lo, base, hi) = ([1.25, 2.0], [8.0, 65.0], [3.0, 10.0]);
    for pose_count in [2usize, 3, 7] {
        let last = pose_count as u32 - 1;
        for step in 0..=64 {
            let w = step as f32 / 64.0 * 0.999;
            let segment = depth_segment(span(0, last, w), pose_count);
            let moments = |source: DepthSource| match source {
                DepthSource::Pose(0) => lo,
                DepthSource::Pose(_) => hi,
                DepthSource::Base => base,
            };
            if w > 0.0 {
                assert_eq!(segment.mix, DepthMix::Raw);
            }
            let got = segment.texel(moments(segment.from), moments(segment.to));
            let expected = through_base(lo, base, hi, w);
            assert_eq!(
                got.map(f32::to_bits),
                expected.map(f32::to_bits),
                "{pose_count} poses at w {w}"
            );
        }
    }
}

// The fast rounding agrees with the shared f16 converters bit for bit.
#[test]
fn rounding_to_f16_matches_the_shared_converters() {
    let mut value = 1.0e-9f32;
    while value < 1.0e6 {
        for candidate in [value, -value, f32::from_bits(value.to_bits() + 0x0fff)] {
            assert_eq!(
                round_to_f16(candidate).to_bits(),
                f16_bits_to_f32(f32_to_f16_bits(candidate)).to_bits(),
                "{candidate}"
            );
        }
        value *= 1.0007;
    }
    for special in [0.0, -0.0, 65504.0, 65519.0, 65520.0] {
        assert_eq!(
            round_to_f16(special).to_bits(),
            f16_bits_to_f32(f32_to_f16_bits(special)).to_bits(),
            "{special}"
        );
    }
}
