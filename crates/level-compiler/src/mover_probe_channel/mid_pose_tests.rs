// The mid-pose bounce rule (research pin MP3): a mid pose stores its own
// posed bounce one plus the termini's later shares blended to its t, a share
// being the terminus's state (id 34 where it stores none) less its posed
// bounce one; an enclosing terminus lends none and the other's holds across;
// with neither lending, the mid pose stores nothing and blends across.

use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::sh_volume::OctahedralAtlasTexel;

use super::{ProbeShares, rgb_tile};
use crate::mover_probe_channel::gather::DomainProbe;
use crate::mover_probe_channel::states::MidState;
use crate::sh_bake::pack_rgb_tile;

const PROBE: u32 = 7;

/// L0-only coefficients: a tile constant over every direction.
fn dc(rgb: [f32; 3]) -> [f32; 27] {
    let mut coefficients = [0.0; 27];
    coefficients[..3].copy_from_slice(&rgb);
    coefficients
}

fn flat(value: f32) -> Vec<OctahedralAtlasTexel> {
    pack_rgb_tile(&vec![[value; 3]; 36])
}

/// Terminus k's bounce one gathered at the probe.
fn bounce_one(rgb: [f32; 3], enclosed: bool) -> DomainProbe {
    DomainProbe {
        probe: PROBE,
        coefficients: dc(rgb),
        depth: None,
        enclosed,
    }
}

/// The rule written out: f16 of (mid bounce one + lerp of each lending
/// terminus's state less its bounce one), texel by texel.
fn expected(
    mid: [f32; 27],
    first: Option<(&[OctahedralAtlasTexel], [f32; 27])>,
    last: Option<(&[OctahedralAtlasTexel], [f32; 27])>,
    t: f32,
) -> Vec<OctahedralAtlasTexel> {
    let share = |(state, one): (&[OctahedralAtlasTexel], [f32; 27])| -> Vec<[f32; 3]> {
        let one = rgb_tile(&one);
        state
            .iter()
            .zip(&one)
            .map(|(texel, one)| {
                [0, 1, 2].map(|c| crate::bc6h::f16_bits_to_f32(texel.rgba[c]) - one[c])
            })
            .collect()
    };
    let (first, last) = (first.map(share), last.map(share));
    let blended: Vec<[f32; 3]> = match (first, last) {
        (Some(a), Some(b)) => a
            .iter()
            .zip(&b)
            .map(|(a, b)| [0, 1, 2].map(|c| (1.0 - t) * a[c] + t * b[c]))
            .collect(),
        (Some(only), None) | (None, Some(only)) => only,
        (None, None) => unreachable!("a stored state has a share"),
    };
    let state: Vec<[f32; 3]> = rgb_tile(&mid)
        .iter()
        .zip(&blended)
        .map(|(one, later)| [0, 1, 2].map(|c| one[c] + later[c]))
        .collect();
    pack_rgb_tile(&state)
}

fn stored(state: Option<MidState<Vec<OctahedralAtlasTexel>>>) -> Vec<OctahedralAtlasTexel> {
    match state {
        Some(MidState::Stored(tile)) => tile,
        other => panic!("expected a stored state, got {other:?}"),
    }
}

// Terminus 0 stores a state; terminus 1 is unchanged and lends id 34 less its
// bounce one. A mid pose stores its own bounce one plus the two shares
// lerped to its t, so at the knot it equals what each terminus's state
// minus its bounce one leaves, blended by mover position.
#[test]
fn a_mid_pose_stores_its_bounce_one_plus_the_termini_later_shares_blended_to_its_t() {
    let base = flat(0.5);
    let first_state = flat(3.0);
    let (first_one, last_one, mid_one) = (dc([1.0; 3]), dc([0.25; 3]), dc([2.0, 1.5, 1.0]));
    let shares = ProbeShares::new(
        PROBE,
        base.clone(),
        0.0,
        [Some(&first_state), None],
        [&bounce_one([1.0; 3], false), &bounce_one([0.25; 3], false)],
    );
    for t in [0.2, 0.5, 0.8] {
        let tile = stored(shares.classify(&mid_one, false, t));
        assert_eq!(
            tile,
            expected(
                mid_one,
                Some((&first_state, first_one)),
                Some((&base, last_one)),
                t
            ),
            "t {t}"
        );
    }
    // The rule at t 0.5, by hand on texel 0's red: the mid bounce one, plus
    // half of (3 − first bounce one) and half of (0.5 − last bounce one).
    let one = |coefficients: [f32; 27]| rgb_tile(&coefficients)[0][0];
    let by_hand = one(mid_one) + (0.5 * (3.0 - one(first_one)) + 0.5 * (0.5 - one(last_one)));
    let tile = stored(shares.classify(&mid_one, false, 0.5));
    assert_eq!(tile[0].rgba[0], f32_to_f16_bits(by_hand));
}

// MP3: the first terminus encloses the probe, so it lends no share, and the
// last terminus's share holds across every mid pose, whatever its t. With
// both termini enclosing it, no share exists and the mid pose lists nothing:
// the probe blends across it.
#[test]
fn an_enclosing_terminus_lends_no_share_and_the_others_holds_across() {
    let base = flat(0.5);
    let last_state = flat(4.0);
    let (last_one, mid_one) = (dc([0.5; 3]), dc([2.0; 3]));
    let shares = ProbeShares::new(
        PROBE,
        base.clone(),
        0.0,
        [None, Some(&last_state)],
        [&bounce_one([9.0; 3], true), &bounce_one([0.5; 3], false)],
    );
    assert!(
        shares.shares[0].is_none(),
        "the enclosing terminus lends none"
    );
    let held = expected(mid_one, None, Some((&last_state, last_one)), 0.0);
    for t in [0.1, 0.5, 0.9] {
        assert_eq!(stored(shares.classify(&mid_one, false, t)), held, "t {t}");
    }

    let neither = ProbeShares::new(
        PROBE,
        base,
        0.0,
        [None, None],
        [&bounce_one([1.0; 3], true), &bounce_one([1.0; 3], true)],
    );
    assert_eq!(
        neither.classify(&mid_one, false, 0.5),
        None,
        "neither terminus lends a share: the mid pose blends across"
    );
}

// AC 12: a mid pose that encloses the probe stores no value state there and
// blends across, share or no share (owner, 2026-11-10); one whose state
// stays within the probe's bars against id 34 reads the base.
#[test]
fn an_enclosing_mid_pose_blends_across_and_an_unchanged_one_reads_the_base() {
    let base = flat(0.5);
    let shares = ProbeShares::new(
        PROBE,
        base.clone(),
        0.0,
        [None, None],
        [&bounce_one([0.0; 3], false), &bounce_one([0.0; 3], false)],
    );
    assert_eq!(
        shares.classify(&dc([7.0; 3]), true, 0.5),
        None,
        "enclosed: blends across"
    );
    // Both termini unchanged with no bounce one: each share is id 34
    // itself, so a mid pose with no bounce one of its own is id 34.
    assert_eq!(
        shares.classify(&dc([0.0; 3]), false, 0.5),
        Some(MidState::Base),
        "within the bars"
    );
}
