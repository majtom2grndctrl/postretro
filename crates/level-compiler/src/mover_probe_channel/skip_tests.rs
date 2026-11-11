// Redundant mid-pose skipping on synthetic per-pose curves over one probe of
// a 3×3×3 grid: what skips, what stores, and that compose at every skipped
// pose's t′, after the ramp and every other skip, stays within the probe's
// bars (brief ACs 10, 11, 18; research pins MP2, MP8, MP9).
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)

use postretro_level_format::mover_probe_channel::{
    DepthLeg, Knot, MOVER_IRRADIANCE_TILE_F16_COUNT, PoseMark, depth_leg, effective_knots,
    pose_interval,
};
use postretro_level_format::probe_depth_tiles::{PROBE_DEPTH_TILE_TEXELS, ProbeDepthTilesSection};
use postretro_level_format::sh_volume::{OctahedralShProbe, OctahedralShVolumeSection};

use super::*;
use crate::mover_probe_channel::domain::{BOUNCE_FLOOR_LEVELS, display_levels};
use crate::mover_probe_channel::states::{BounceProbe, DepthProbe, MoverBounceStates};
use crate::sh_bake::ProbeDepthTiles;

/// The grid's centre probe, with a valid neighbour in every direction.
const PROBE: u32 = 13;
/// Id 34's bounce at every probe, one value in every texel and channel.
const BASE_BOUNCE: f32 = 0.05;
/// Every probe's averaged id-34 depth moments: open air past every
/// neighbour.
const BASE_DEPTH: [f32; 2] = [3.0, 9.5];
const TILE_DIM: u32 = 6;

/// Id 34 over a 3×3×3 grid at 1 m spacing (every probe valid, every tile
/// [`BASE_BOUNCE`], every probe's moments [`BASE_DEPTH`]) and an id 59 that
/// tiles no probe. Id 34 stores f16, so each probe's round-trip bar is 0
/// and the perceptual floor alone decides.
fn fixture() -> (OctahedralShVolumeSection, ProbeDepthTiles) {
    let probes = 27u32;
    let texels = (probes * TILE_DIM * TILE_DIM) as usize;
    let texel: Vec<u8> = [BASE_BOUNCE, BASE_BOUNCE, BASE_BOUNCE, 1.0]
        .iter()
        .flat_map(|&value| f32_to_f16_bits(value).to_le_bytes())
        .collect();
    let section = OctahedralShVolumeSection {
        cell_size: [1.0; 3],
        grid_dimensions: [3, 3, 3],
        tile_dimension: TILE_DIM,
        atlas_dimensions: [probes * TILE_DIM, TILE_DIM],
        layer_count: 1,
        tiles_per_layer: probes,
        atlas_tiles_per_row: probes,
        probes: vec![
            OctahedralShProbe {
                validity: 1,
                mean_distance: f32_to_f16_bits(BASE_DEPTH[0]),
                mean_sq_distance: f32_to_f16_bits(BASE_DEPTH[1]),
                density_level: 0,
                node_scale: 0,
            };
            probes as usize
        ],
        compact_atlas: texel.repeat(texels),
        ..OctahedralShVolumeSection::placeholder()
    };
    let depth = ProbeDepthTiles {
        section: ProbeDepthTilesSection {
            probe_indices: Vec::new(),
            texels: Vec::new(),
        },
        dropped: 0,
    };
    (section, depth)
}

/// What one pose holds for the probe in a synthetic curve.
#[derive(Clone, Copy)]
enum At {
    /// A stored state: one value in every texel (and, for depth, the
    /// moments `[d, d² + 0.25]`).
    Stored(f32),
    Base,
    Unmarked,
}

fn marks_of(curve: &[At]) -> Vec<PoseMark> {
    curve
        .iter()
        .map(|at| match at {
            At::Stored(_) => PoseMark::Stored,
            At::Base => PoseMark::Base,
            At::Unmarked => PoseMark::Unmarked,
        })
        .collect()
}

fn bounce_probe(curve: &[At]) -> BounceProbe {
    let texel = |value: f32| [f32_to_f16_bits(value); 3];
    BounceProbe {
        probe: PROBE,
        marks: marks_of(curve),
        tiles: curve
            .iter()
            .filter_map(|at| match at {
                At::Stored(value) => {
                    Some(texel(*value).repeat(MOVER_IRRADIANCE_TILE_F16_COUNT / 3))
                }
                _ => None,
            })
            .flatten()
            .collect(),
    }
}

fn moments(d: f32) -> [f32; 2] {
    [d, d * d + 0.25]
}

fn depth_tile(value: [f32; 2]) -> Vec<[u16; 2]> {
    vec![value.map(f32_to_f16_bits); PROBE_DEPTH_TILE_TEXELS]
}

/// A depth probe whose stored poses hold `tiles`, in pose order.
fn depth_probe(marks: Vec<PoseMark>, tiles: Vec<Vec<[u16; 2]>>) -> DepthProbe {
    DepthProbe {
        probe: PROBE,
        marks,
        tiles: tiles.concat(),
    }
}

/// Even path knots for `count` poses.
fn even(count: usize) -> Vec<f32> {
    (0..count).map(|k| k as f32 / (count - 1) as f32).collect()
}

/// Skip one mover holding `bounce` and `depth` over `knots` and `ramp`.
fn skip(
    bounce: Vec<BounceProbe>,
    depth: Vec<DepthProbe>,
    knots: &[f32],
    ramp: (f32, f32),
) -> (MoverBounceStates, SkipCounts) {
    let (section, tiles) = fixture();
    let bars = SkipBars::new(&section, MoverAbsentDepth::new(&section, &tiles), true);
    let mut states = MoverBounceStates::new(1, knots.len(), bounce, depth);
    let counts =
        skip_redundant_mid_poses(&mut states, &effective_knots(knots, ramp.0, ramp.1), &bars);
    (states, counts)
}

fn base_tile() -> Vec<OctahedralAtlasTexel> {
    texels(&[f32_to_f16_bits(BASE_BOUNCE); MOVER_IRRADIANCE_TILE_F16_COUNT])
}

fn value_tile(value: f32) -> Vec<OctahedralAtlasTexel> {
    texels(&[f32_to_f16_bits(value); MOVER_IRRADIANCE_TILE_F16_COUNT])
}

/// The renderer's bounce compose of the curve `raw` (one state per pose,
/// the base where none stores) at `t` (a stated t′) under `marks`, the
/// marks left after skipping.
fn compose_bounce(
    raw: &[At],
    marks: &[PoseMark],
    knots: &[Knot],
    t: f32,
) -> Vec<OctahedralAtlasTexel> {
    let span = pose_interval(knots, t, |pose| marks[pose as usize] != PoseMark::Unmarked);
    let state = |pose: u32| match raw[pose as usize] {
        At::Stored(value) if marks[pose as usize] == PoseMark::Stored => value_tile(value),
        _ => base_tile(),
    };
    mix_tiles(&state(span.lo), &state(span.hi), span.weight)
}

/// Every pose `raw` stored, now skipped and read by compose: the composed
/// tile at its t′ stays within its bars (`--uncompressed-irradiance`: the
/// perceptual floor). Returns the worst display levels seen.
fn assert_skipped_compose_within_bars(raw: &[At], marks: &[PoseMark], knots: &[Knot]) -> f32 {
    let mut worst = 0.0f32;
    for knot in knots {
        let pose = knot.pose as usize;
        let At::Stored(value) = raw[pose] else {
            continue;
        };
        if marks[pose] != PoseMark::Unmarked {
            continue;
        }
        let composed = compose_bounce(raw, marks, knots, knot.t);
        let own = value_tile(value);
        assert!(
            !clears_bounce_bars(&composed, &own, 0.0),
            "skipped pose {pose} at t′ {}: compose reads {:.2} levels off its state",
            knot.t,
            display_levels(&composed, &own)
        );
        worst = worst.max(display_levels(&composed, &own));
    }
    worst
}

/// The f16 value one step above `value`.
fn next_f16(value: f32) -> f32 {
    f16_bits_to_f32(f32_to_f16_bits(value) + 1)
}

// AC 10: a probe whose states run linear in t between its termini stores
// none of its mid poses; compose reads the termini blend, which lands on
// every mid state.
#[test]
fn a_gentle_curve_skips_every_mid_pose_and_composes_within_its_bars() {
    let knots = even(6);
    let raw: Vec<At> = knots.iter().map(|t| At::Stored(0.2 + 0.4 * t)).collect();
    let (states, counts) = skip(vec![bounce_probe(&raw)], Vec::new(), &knots, (0.0, 1.0));
    let [probe] = states.bounce.as_slice() else {
        panic!("the probe keeps its termini");
    };
    use PoseMark::{Stored as S, Unmarked as U};
    assert_eq!(probe.marks, [S, U, U, U, U, S]);
    assert_eq!(probe.tiles.len(), 2 * MOVER_IRRADIANCE_TILE_F16_COUNT);
    assert_eq!((counts.bounce_stored, counts.bounce_skipped), (0, 4));
    assert_skipped_compose_within_bars(&raw, &probe.marks, &effective_knots(&knots, 0.0, 1.0));
}

// AC 10: a mid pose whose state sits one f16 step past its neighbours'
// blend by the perceptual floor stores it; one step nearer, it skips.
#[test]
fn a_mid_pose_missing_its_bars_by_a_step_stores_and_within_them_skips() {
    let knots = even(3);
    let blend = mix_tiles(&value_tile(0.2), &value_tile(0.6), 0.5);
    let blended = f16_bits_to_f32(blend[0].rgba[0]);
    let mut outside = blended;
    while display_levels(&blend, &value_tile(outside)) <= BOUNCE_FLOOR_LEVELS {
        outside = next_f16(outside);
    }
    let inside = f16_bits_to_f32(f32_to_f16_bits(outside) - 1);
    use PoseMark::{Stored as S, Unmarked as U};
    for (mid, marks) in [(outside, [S, S, S]), (inside, [S, U, S])] {
        let raw = [At::Stored(0.2), At::Stored(mid), At::Stored(0.6)];
        let (states, _) = skip(vec![bounce_probe(&raw)], Vec::new(), &knots, (0.0, 1.0));
        assert_eq!(
            states.bounce[0].marks, marks,
            "mid state {mid} against the blend {blended}"
        );
    }
}

// AC 10, MP2: states curving gently (v = a + c·k²) sit within the bars of
// their immediate neighbours' blend at every mid pose, yet the termini
// alone miss. Skips are judged against the interval compose reads after
// every other skip, so enough states stay, and compose at every skipped
// pose's t′ is within the bars.
#[test]
fn adjacent_skips_stay_within_the_bars_at_every_skipped_pose() {
    let knots = even(6);
    let c = 0.004;
    let value = |k: usize| 0.2 + c * (k * k) as f32;
    let raw: Vec<At> = (0..6).map(|k| At::Stored(value(k))).collect();
    for k in 1..5 {
        let neighbours = mix_tiles(&value_tile(value(k - 1)), &value_tile(value(k + 1)), 0.5);
        assert!(
            !clears_bounce_bars(&neighbours, &value_tile(value(k)), 0.0),
            "precondition: pose {k} is within its immediate neighbours' blend"
        );
    }
    let termini_only = [
        At::Stored(value(0)),
        At::Unmarked,
        At::Unmarked,
        At::Unmarked,
        At::Unmarked,
        At::Stored(value(5)),
    ];
    let read = effective_knots(&knots, 0.0, 1.0);
    let worst_termini = (1..5)
        .map(|k| {
            display_levels(
                &compose_bounce(&termini_only, &marks_of(&termini_only), &read, read[k].t),
                &value_tile(value(k)),
            )
        })
        .fold(0.0, f32::max);
    assert!(
        worst_termini > BOUNCE_FLOOR_LEVELS,
        "precondition: the termini alone miss ({worst_termini:.2} levels)"
    );

    let (states, counts) = skip(vec![bounce_probe(&raw)], Vec::new(), &knots, (0.0, 1.0));
    let marks = &states.bounce[0].marks;
    assert!(
        counts.bounce_skipped > 0,
        "some adjacent poses skip: {marks:?}"
    );
    assert!(
        counts.bounce_stored > 0,
        "the curve keeps some mid states: {marks:?}"
    );
    let worst = assert_skipped_compose_within_bars(&raw, marks, &read);
    assert!(worst <= BOUNCE_FLOOR_LEVELS, "{worst}");
}

// AC 11, MP9: on poses at t 0, 0.2, 0.4, 0.6, 0.8 and 1 under ramp (0.6,
// 0.9), compose reads knots t′ 0 (pose 0), 0.667 (pose 4) and 1 (pose 5).
// Poses 1 to 3 are squeezed out and never read, so they skip whatever they
// store; pose 4 is judged against pose 0 → pose 5 at t′ 0.667, and skips.
// With no ramp, pose 4's neighbours are poses 3 and 5, so it stores: the
// ramp re-decides skips.
#[test]
fn skips_are_judged_after_the_ramp_and_a_ramp_edit_re_decides_them() {
    let knots = even(6);
    let pose_4 =
        f16_bits_to_f32(mix_tiles(&value_tile(0.2), &value_tile(0.6), 2.0 / 3.0)[0].rgba[0]);
    let raw = [
        At::Stored(0.2),
        At::Stored(0.9),
        At::Stored(0.9),
        At::Stored(0.9),
        At::Stored(pose_4),
        At::Stored(0.6),
    ];
    use PoseMark::{Stored as S, Unmarked as U};

    let ramp = (0.6, 0.9);
    let (ramped, counts) = skip(vec![bounce_probe(&raw)], Vec::new(), &knots, ramp);
    assert_eq!(ramped.bounce[0].marks, [S, U, U, U, U, S]);
    assert_eq!((counts.bounce_skipped, counts.squeezed), (4, 3));
    assert_skipped_compose_within_bars(
        &raw,
        &ramped.bounce[0].marks,
        &effective_knots(&knots, ramp.0, ramp.1),
    );

    let (plain, _) = skip(vec![bounce_probe(&raw)], Vec::new(), &knots, (0.0, 1.0));
    let marks = &plain.bounce[0].marks;
    assert_eq!(
        marks[4], S,
        "unramped, pose 4 misses its neighbours' blend: {marks:?}"
    );
    assert_eq!(
        marks[1], S,
        "unramped, the plateau's first pose is read: {marks:?}"
    );
    assert_skipped_compose_within_bars(&raw, marks, &effective_knots(&knots, 0.0, 1.0));
}

// MP11, AC 11: a probe whose only stored mid state sits at a pose the ramp
// squeezes out reads id 34 wherever compose reads, so it leaves the bounce
// states; one carrying depth only there leaves the depth states and no
// longer carries depth.
#[test]
fn a_probe_storing_only_at_squeezed_out_poses_leaves_the_states() {
    let knots = even(6);
    let raw = [
        At::Unmarked,
        At::Stored(0.9),
        At::Base,
        At::Base,
        At::Base,
        At::Unmarked,
    ];
    let depth = depth_probe(marks_of(&raw), vec![depth_tile(moments(0.1))]);
    let (states, counts) = skip(vec![bounce_probe(&raw)], vec![depth], &knots, (0.6, 0.9));
    assert!(states.bounce.is_empty() && states.depth.is_empty());
    assert_eq!(states.depth_carried, 0);
    assert_eq!(
        (counts.bounce_skipped, counts.depth_skipped, counts.squeezed),
        (1, 1, 2)
    );
}

/// The renderer's depth blend over `poses` (each pose's moments, the base
/// where it stores none) at `t` under `marks`: D_lo → base → D_hi, packed
/// as the live tile packs it.
fn compose_depth(poses: &[[f32; 2]], marks: &[PoseMark], knots: &[Knot], t: f32) -> Vec<[u16; 2]> {
    let span = pose_interval(knots, t, |pose| marks[pose as usize] != PoseMark::Unmarked);
    let (a, b, s) = match depth_leg(span.weight) {
        DepthLeg::Exact => (poses[span.lo as usize], poses[span.lo as usize], 0.0),
        DepthLeg::ToBase(s) => (poses[span.lo as usize], BASE_DEPTH, s),
        DepthLeg::FromBase(s) => (BASE_DEPTH, poses[span.hi as usize], s),
    };
    depth_tile([0, 1].map(|m| lerp(a[m], b[m], s)))
}

/// One texel's moments of a uniform depth tile, as stored.
fn tile_moments(tile: &[[u16; 2]]) -> [f32; 2] {
    tile[0].map(f16_bits_to_f32)
}

// AC 18, MP8: a depth probe whose mid tiles lie on its termini's
// through-base blend skips every mid pose, so its interval runs terminus to
// terminus through the base, today's blend at every t′. A probe outside the
// near-field domain (no mid state) keeps its marks and composes the same
// blend. A mid tile far off the blend (dark at pose 2) stores, and so does
// pose 1, which the shorter interval to it would otherwise read as the base.
// In every case each skipped tile is within the significance floor of what
// compose reads at its t′.
#[test]
fn every_mid_pose_skipping_its_depth_composes_the_termini_through_base_blend() {
    let knots = even(6);
    let read = effective_knots(&knots, 0.0, 1.0);
    let termini_marks = {
        let mut marks = vec![PoseMark::Unmarked; 6];
        marks[0] = PoseMark::Stored;
        marks[5] = PoseMark::Stored;
        marks
    };
    let mut ends = [BASE_DEPTH; 6];
    ends[0] = [0.0, 0.0];
    ends[5] = moments(1.0);
    let on_blend: Vec<Vec<[u16; 2]>> = read
        .iter()
        .map(|knot| compose_depth(&ends, &termini_marks, &read, knot.t))
        .collect();
    let (section, tiles) = fixture();
    let absent = MoverAbsentDepth::new(&section, &tiles);
    assert!(
        absent.score(PROBE, &on_blend[0]) >= DEPTH_SIGNIFICANCE_FLOOR,
        "precondition: the dark terminus carries depth"
    );

    let mut dark_2 = on_blend.clone();
    dark_2[2] = depth_tile([0.0, 0.0]);
    let all_stored = vec![PoseMark::Stored; 6];
    for (label, marks, raw, stores) in [
        ("on the blend", &all_stored, &on_blend, vec![0, 5]),
        ("outside the domain", &termini_marks, &on_blend, vec![0, 5]),
        ("dark at pose 2", &all_stored, &dark_2, vec![0, 1, 2, 5]),
    ] {
        let stored_tiles = (0..6)
            .filter(|&k| marks[k] == PoseMark::Stored)
            .map(|k| raw[k].clone())
            .collect();
        let probe = depth_probe(marks.clone(), stored_tiles);
        let (states, _) = skip(Vec::new(), vec![probe], &knots, (0.0, 1.0));
        let [probe] = states.depth.as_slice() else {
            panic!("{label}: the probe keeps its depth");
        };
        let stored: Vec<usize> = (0..6)
            .filter(|&k| probe.marks[k] == PoseMark::Stored)
            .collect();
        assert_eq!(stored, stores, "{label}: {:?}", probe.marks);
        let poses: Vec<[f32; 2]> = raw.iter().map(|tile| tile_moments(tile)).collect();
        for knot in &read[1..5] {
            let composed = compose_depth(&poses, &probe.marks, &read, knot.t);
            if stores == [0, 5] {
                assert_eq!(
                    composed, on_blend[knot.pose as usize],
                    "{label}: today's through-base blend at t′ {}",
                    knot.t
                );
            }
            let pose = knot.pose as usize;
            if marks[pose] == PoseMark::Stored && probe.marks[pose] == PoseMark::Unmarked {
                let score = absent.score_against(PROBE, &composed, &raw[pose]);
                assert!(
                    score < DEPTH_SIGNIFICANCE_FLOOR,
                    "{label}: skipped pose {pose} scores {score} against compose"
                );
            }
        }
    }
}

// A mid pose reading the base stays defined: no interval reaches past it,
// it is never skipped, and a stored pose beside it is judged against the
// blend toward the base, which it misses here.
#[test]
fn a_base_reading_mid_pose_bounds_every_interval() {
    let knots = even(5);
    let raw = [
        At::Stored(0.2),
        At::Stored(0.2),
        At::Base,
        At::Stored(0.2),
        At::Stored(0.2),
    ];
    let (states, counts) = skip(vec![bounce_probe(&raw)], Vec::new(), &knots, (0.0, 1.0));
    use PoseMark::{Base as B, Stored as S};
    assert_eq!(states.bounce[0].marks, [S, S, B, S, S]);
    assert_eq!(counts.bounce_skipped, 0);
}
