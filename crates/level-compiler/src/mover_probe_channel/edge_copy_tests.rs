// Enclosure-run edge copy: which pose each pose's value comes from (entry,
// exit, interior, one-pose runs and their tie, runs at a terminus, runs
// under an author ramp), and the bounce states it rewrites. Skipping over
// copied states is in `skip_tests.rs`; the direct half's copy, through the
// bake, in `direct_tests.rs`.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use postretro_level_format::mover_probe_channel::{
    Knot, MOVER_IRRADIANCE_TILE_F16_COUNT, PoseMark, effective_knots,
};
use postretro_level_format::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;

use super::{copy_bounce_edges, value_sources};
use crate::mover_probe_channel::states::{BounceProbe, DepthProbe, MoverBounceStates};

fn even_knots(count: usize) -> Vec<f32> {
    (0..count).map(|k| k as f32 / (count - 1) as f32).collect()
}

/// The knots compose reads for path fractions `knots` under no ramp.
fn plain(knots: &[f32]) -> Vec<Knot> {
    effective_knots(knots, 0.0, 1.0)
}

/// [`plain`] over `count` even knots.
fn even(count: usize) -> Vec<Knot> {
    plain(&even_knots(count))
}

fn runs(pattern: &str) -> Vec<bool> {
    pattern.chars().map(|c| c == '#').collect()
}

// A run between clear poses: its first pose (the entry) takes the clear pose
// before it, its last (the exit) the clear pose after it, and the pose
// inside keeps its own dark state. Clear poses keep their own.
#[test]
fn a_runs_entry_and_exit_take_their_clear_neighbours_and_its_inside_stays_dark() {
    assert_eq!(
        value_sources(&runs("..###.."), &even(7)),
        [0, 1, 1, 3, 5, 5, 6]
    );
    assert_eq!(value_sources(&runs(".##."), &even(4)), [0, 0, 3, 3]);
    assert_eq!(value_sources(&runs("......"), &even(6)), [0, 1, 2, 3, 4, 5]);
}

// A one-pose run takes the nearer clear pose in travel, by the knots, and
// the later one on a tie. Gaps a few f32 ulps apart still tie: 0.1 to 0.2
// and 0.2 to 0.3 differ in their last bits.
#[test]
fn a_one_pose_run_takes_the_nearer_clear_pose_and_the_later_on_a_tie() {
    let knots = plain(&[0.0, 0.2, 0.3, 0.6, 1.0]);
    assert_eq!(value_sources(&runs("..#.."), &knots), [0, 1, 1, 3, 4]);
    let knots = plain(&[0.0, 0.2, 0.5, 0.6, 1.0]);
    assert_eq!(value_sources(&runs("..#.."), &knots), [0, 1, 3, 3, 4]);
    assert_eq!(value_sources(&runs("..#.."), &even(5)), [0, 1, 3, 3, 4]);
    // Two one-pose runs either side of one clear pose: each its own tie.
    assert_eq!(value_sources(&runs(".#.#."), &even(5)), [0, 2, 2, 4, 4]);
    let (lo, mid, hi) = (0.1f32, 0.2f32, 0.3f32);
    assert_ne!(
        f64::from(mid) - f64::from(lo),
        f64::from(hi) - f64::from(mid),
        "precondition: the gaps differ in their last bits"
    );
    let knots = plain(&[0.0, lo, mid, hi, 1.0]);
    assert_eq!(value_sources(&runs("..#.."), &knots), [0, 1, 3, 3, 4]);
}

// A run end at a terminus keeps its own: the terminus reads the base. The
// run's other end still copies, and a run over the whole path, or one
// terminus alone, copies nothing.
#[test]
fn a_run_end_at_a_terminus_keeps_its_own() {
    assert_eq!(value_sources(&runs("###.."), &even(5)), [0, 1, 3, 3, 4]);
    assert_eq!(value_sources(&runs("..###"), &even(5)), [0, 1, 1, 3, 4]);
    assert_eq!(value_sources(&runs("#...."), &even(5)), [0, 1, 2, 3, 4]);
    assert_eq!(value_sources(&runs("##..."), &even(5)), [0, 2, 2, 3, 4]);
    assert_eq!(value_sources(&runs("#####"), &even(5)), [0, 1, 2, 3, 4]);
}

/// A tile of one value in every half.
fn tile(value: u16) -> Vec<u16> {
    vec![value; MOVER_IRRADIANCE_TILE_F16_COUNT]
}

/// A bounce probe from per-pose (mark, value): a stored pose holds a tile
/// of its value; any other pose's value is unused.
fn probe(probe: u32, poses: &[(PoseMark, u16)]) -> BounceProbe {
    BounceProbe {
        probe,
        marks: poses.iter().map(|&(mark, _)| mark).collect(),
        tiles: poses
            .iter()
            .filter(|(mark, _)| *mark == PoseMark::Stored)
            .flat_map(|&(_, value)| tile(value))
            .collect(),
    }
}

// Bounce: a copied pose takes what compose reads at its source: a stored
// tile, the base marked at a mid pose, or the base a terminus storing
// nothing reads (marked base at the mid pose). A probe left storing nothing
// reads the base everywhere and leaves the states; one with no enclosure is
// untouched. Depth is never touched.
#[test]
fn bounce_edges_take_what_compose_reads_at_their_source() {
    use PoseMark::{Base as B, Stored as S, Unmarked as U};
    const LIT: u16 = 0x3c00;
    const DARK: u16 = 0;
    let exit_and_entry = probe(
        1,
        &[
            (U, 0),
            (S, LIT),
            (S, DARK),
            (S, DARK),
            (S, DARK),
            (B, 0),
            (U, 0),
        ],
    );
    // Enclosed at pose 1 alone, nearer the first terminus, which stores none.
    let by_terminus = probe(
        2,
        &[
            (U, 0),
            (S, DARK),
            (S, LIT),
            (S, LIT),
            (S, LIT),
            (S, LIT),
            (U, 0),
        ],
    );
    // Enclosed at pose 3 alone; both neighbours read the base.
    let all_base = probe(
        3,
        &[(U, 0), (B, 0), (B, 0), (S, DARK), (B, 0), (B, 0), (U, 0)],
    );
    let clear = probe(
        4,
        &[
            (U, 0),
            (S, LIT),
            (S, DARK),
            (S, DARK),
            (S, DARK),
            (S, LIT),
            (U, 0),
        ],
    );
    let knots = plain(&[0.0, 0.05, 0.3, 0.5, 0.7, 0.9, 1.0]);
    let mut states = MoverBounceStates::new(
        9,
        7,
        vec![exit_and_entry, by_terminus, all_base, clear.clone()],
        Vec::new(),
    );
    let copied = copy_bounce_edges(&mut states, &knots, |probe| match probe {
        1 => runs("..###.."),
        2 => runs(".#....."),
        3 => runs("...#..."),
        _ => runs("......."),
    });
    assert_eq!(copied, 4);
    assert_eq!(
        states.bounce,
        [
            probe(
                1,
                &[
                    (U, 0),
                    (S, LIT),
                    (S, LIT),
                    (S, DARK),
                    (B, 0),
                    (B, 0),
                    (U, 0)
                ]
            ),
            probe(
                2,
                &[
                    (U, 0),
                    (B, 0),
                    (S, LIT),
                    (S, LIT),
                    (S, LIT),
                    (S, LIT),
                    (U, 0)
                ]
            ),
            clear,
        ]
    );
}

// Runs are found over the knots compose reads. On 11 even poses under ramp
// (0.6, 0.9), compose reads poses 0, 7 (t′ ≈ 0.33), 8 (t′ ≈ 0.67) and 10:
// poses 1 to 6 squeeze onto t′ 0 and pose 9 onto 1. Poses 5 to 9 enclose
// the probe, so the read run is poses 7 and 8: its first read pose takes
// the read clear pose before it, terminus 0, and its last the terminus
// after it. The squeezed poses keep their own, enclosed or clear, and
// squeezed clear pose 4 is never a source.
#[test]
fn runs_and_their_sources_are_the_poses_compose_reads_under_the_ramp() {
    let ramped = effective_knots(&even_knots(11), 0.6, 0.9);
    assert_eq!(
        ramped.iter().map(|knot| knot.pose).collect::<Vec<_>>(),
        [0, 7, 8, 10],
        "precondition: the poses compose reads"
    );
    assert_eq!(
        value_sources(&runs(".....#####."), &ramped),
        [0, 1, 2, 3, 4, 5, 6, 0, 10, 9, 10]
    );
    // Unramped, the same enclosure copies its raw edges.
    assert_eq!(
        value_sources(&runs(".....#####."), &even(11)),
        [0, 1, 2, 3, 4, 4, 6, 7, 8, 10, 10]
    );
    // A one-pose read run measures nearer in t′, past squeezed poses: pose
    // 2 sits at t′ 0.17 between terminus 0 and pose 3 at t′ 0.67, so it
    // takes terminus 0, never squeezed clear pose 1 (the raw knots tie).
    let ramped = effective_knots(&[0.0, 0.5, 0.65, 0.8, 1.0], 0.6, 0.9);
    assert_eq!(
        ramped.iter().map(|knot| knot.pose).collect::<Vec<_>>(),
        [0, 2, 3, 4],
        "precondition: pose 1 is squeezed out"
    );
    assert_eq!(value_sources(&runs("..#.."), &ramped), [0, 1, 0, 3, 4]);
}

// Bounce under the ramp: the run's first read pose takes what compose reads
// at terminus 0 (the base, since it stores nothing) and its last read pose
// terminus 10's stored lit tile. The squeezed poses keep their states, and
// depth keeps the dark tile at every enclosed pose.
#[test]
fn ramped_bounce_edges_carry_the_read_clear_neighbours_and_depth_stays_dark() {
    use PoseMark::{Base as B, Stored as S, Unmarked as U};
    const LIT: u16 = 0x3c00;
    const DARK: u16 = 0;
    let enclosed = runs(".....#####.");
    let raw: Vec<(PoseMark, u16)> = (0..11)
        .map(|pose| match pose {
            0 => (U, 0),
            _ if enclosed[pose] => (S, DARK),
            _ => (S, LIT),
        })
        .collect();
    let open = vec![[0x4200u16, 0x4880u16]; PROBE_DEPTH_TILE_TEXELS];
    let shut = vec![[0u16, 0u16]; PROBE_DEPTH_TILE_TEXELS];
    let depth = DepthProbe {
        probe: 1,
        marks: vec![S; 11],
        tiles: enclosed
            .iter()
            .flat_map(|&enclosed| if enclosed { shut.clone() } else { open.clone() })
            .collect(),
    };
    let mut states = MoverBounceStates::new(9, 11, vec![probe(1, &raw)], vec![depth.clone()]);
    let ramped = effective_knots(&even_knots(11), 0.6, 0.9);
    let copied = copy_bounce_edges(&mut states, &ramped, |_| enclosed.clone());
    assert_eq!(copied, 2);
    let mut expected = raw.clone();
    expected[7] = (B, 0);
    expected[8] = (S, LIT);
    assert_eq!(states.bounce, [probe(1, &expected)]);
    assert_eq!(states.depth, [depth], "depth is never touched");
}

// A mover with no mid pose copies nothing.
#[test]
fn termini_alone_copy_nothing() {
    use PoseMark::{Stored as S, Unmarked as U};
    let only = probe(1, &[(S, 1), (U, 0)]);
    let mut states = MoverBounceStates::new(9, 2, vec![only.clone()], Vec::new());
    assert_eq!(
        copy_bounce_edges(&mut states, &plain(&[0.0, 1.0]), |_| runs("#.")),
        0
    );
    assert_eq!(states.bounce, [only]);
}
