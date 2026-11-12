// CPU proofs of the knot expansion and the interval rule every mover state
// consumer blends by. GPU proofs are in `compose_tests.rs` and
// `bracket_tests.rs`.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use postretro_level_format::mover_probe_channel::MoverRecord;

use super::*;

fn record(knots: &[f32], ramp: (f32, f32)) -> MoverRecord {
    MoverRecord {
        mover_id: 1,
        ramp_start: ramp.0,
        ramp_end: ramp.1,
        knots: knots.to_vec(),
    }
}

fn poses(knots: &[Knot]) -> Vec<u32> {
    knots.iter().map(|knot| knot.pose).collect()
}

/// MP1: ramp (0.6, 0.9) on poses at t 0, 0.2, 0.4, 0.6, 0.8, 1 maps the
/// knots to t′ 0, 0, 0, 0, 0.667, 1; the poses squeezed onto 0 are never
/// read, so pose 0 carries t′ 0. MP10: with poses at 0.95 and 1 both on
/// t′ 1, only the last is read.
#[test]
fn knots_map_through_the_ramp_and_squeezed_poses_are_never_read() {
    let mp1 = record(&[0.0, 0.2, 0.4, 0.6, 0.8, 1.0], (0.6, 0.9)).effective_knots();
    assert_eq!(poses(&mp1), [0, 4, 5]);
    assert_eq!(mp1[0].t, 0.0);
    assert_eq!(mp1[1].t, ramped(0.8, 0.6, 0.9));
    assert!((mp1[1].t - 2.0 / 3.0).abs() < 1.0e-6);
    assert_eq!(mp1[2].t, 1.0);

    let knots: Vec<f32> = (0..21).map(|k| k as f32 / 20.0).collect();
    let mp10 = record(&knots, (0.6, 0.9)).effective_knots();
    // t 0.65 .. 0.85 land inside (0, 1); 0.9 .. 0.95 squeeze onto 1.
    assert_eq!(poses(&mp10), [0, 13, 14, 15, 16, 17, 20]);
    for knot in &mp10[1..mp10.len() - 1] {
        let t = knots[knot.pose as usize];
        assert_eq!(
            knot.t,
            ramped(t, 0.6, 0.9),
            "knot {} reads ramp(t_k)",
            knot.pose
        );
    }

    // No ramp: knots are their own t′, bit for bit.
    let plain = record(&[0.0, 0.25, 0.5, 1.0], (0.0, 1.0)).effective_knots();
    assert_eq!(
        plain,
        [
            Knot { t: 0.0, pose: 0 },
            Knot { t: 0.25, pose: 1 },
            Knot { t: 0.5, pose: 2 },
            Knot { t: 1.0, pose: 3 },
        ]
    );
}

/// The interval rule: exact at a defined knot, a blend between the defined
/// knots around t′ otherwise, an undefined mid pose blended across, the
/// termini always defined, and t′ ≤ 0 / ≥ 1 reading the first / last pose.
#[test]
fn the_interval_rule_reads_defined_knots_exactly_and_blends_across_the_rest() {
    let knots = record(&[0.0, 0.25, 0.5, 0.75, 1.0], (0.0, 1.0)).effective_knots();
    let all = |_: u32| true;
    assert_eq!(interval(&knots, 0.5, all), Interval::exact(2));
    assert_eq!(interval(&knots, 0.0, all), Interval::exact(0));
    assert_eq!(interval(&knots, 1.0, all), Interval::exact(4));
    assert_eq!(interval(&knots, 7.0, all), Interval::exact(4));
    let between = interval(&knots, 0.375, all);
    assert_eq!((between.lo, between.hi), (1, 2));
    assert_eq!(between.weight, 0.5);

    // Pose 2 undefined (an unmarked mid pose): 1 → 3 across it, and exactly
    // at its knot the blend reads halfway.
    let skip_2 = |pose: u32| pose != 2;
    let across = interval(&knots, 0.5, skip_2);
    assert_eq!((across.lo, across.hi, across.weight), (1, 3, 0.5));
    // Every mid pose undefined: termini to termini, weight = t′.
    let none = |_: u32| false;
    let ends = interval(&knots, 0.3, none);
    assert_eq!((ends.lo, ends.hi), (0, 4));
    assert!((ends.weight - 0.3).abs() < 1.0e-7);
    // Out and back reads alike: the lookup is a function of t′ alone.
    for t in [0.1, 0.33, 0.5, 0.9] {
        assert_eq!(interval(&knots, t, skip_2), interval(&knots, t, skip_2));
    }
}

/// The knot block both channel tables carry: a run word per slot, then
/// `[N, E, (t′ bits, pose) × E]` per slot.
#[test]
fn the_knot_block_lays_out_one_run_per_slot() {
    let section = MoverProbeChannelSection {
        mover_records: vec![
            record(&[0.0, 1.0], (0.0, 1.0)),
            MoverRecord {
                mover_id: 4,
                ..record(&[0.0, 0.5, 1.0], (0.0, 1.0))
            },
        ],
        ..super::super::tests::fixture_section([1, 1, 1], &[])
    };
    let knots = MoverKnots::new(Some(&section), &[1, 4]).unwrap();
    assert_eq!(knots.table_word_count(), 2 + (2 + 4) + (2 + 6));
    let mut words = vec![0u32; 3 + knots.table_word_count()];
    knots.write_table(&mut words, 3);
    assert_eq!(&words[3..5], &[5, 11]);
    assert_eq!(&words[5..11], &[2, 2, 0, 0, 1.0f32.to_bits(), 1]);
    assert_eq!(&words[11..13], &[3, 3]);
    assert_eq!(&words[15..17], &[0.5f32.to_bits(), 1]);
    assert!(
        MoverKnots::new(Some(&section), &[2]).is_err(),
        "a mover with no record"
    );
}

/// The knot bracket the t′ block carries: the last knot at or below t′
/// for t′ in [0, 1), squeezed knots never counted, and always below the
/// last knot so the shaders' knot above it exists.
#[test]
fn the_knot_bracket_is_the_last_knot_at_or_below_t() {
    let section = MoverProbeChannelSection {
        mover_records: vec![record(&[0.0, 0.2, 0.4, 0.6, 0.8, 1.0], (0.0, 1.0)), {
            // MP1: poses 1–3 squeeze onto t′ 0; knots 0, 2/3 (pose 4), 1.
            MoverRecord {
                mover_id: 2,
                ..record(&[0.0, 0.2, 0.4, 0.6, 0.8, 1.0], (0.6, 0.9))
            }
        }],
        ..super::super::tests::fixture_section([1, 1, 1], &[])
    };
    let knots = MoverKnots::new(Some(&section), &[1, 2]).unwrap();
    for (t, bracket) in [
        (-1.0, 0),
        (0.0, 0),
        (0.1, 0),
        (0.2, 1),
        (0.2f32.next_down(), 0),
        (0.5, 2),
        (0.8, 4),
        (0.99, 4),
        (1.0, 4),
    ] {
        assert_eq!(knots.bracket(0, t), bracket, "t′ {t}");
    }
    let squeezed = ramped(0.8, 0.6, 0.9);
    for (t, bracket) in [(0.0, 0), (squeezed.next_down(), 0), (squeezed, 1), (1.0, 1)] {
        assert_eq!(knots.bracket(1, t), bracket, "ramped t′ {t}");
    }
}
