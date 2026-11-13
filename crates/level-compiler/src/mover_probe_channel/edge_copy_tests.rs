// Enclosure-run edge copy: which pose each pose's value comes from (entry,
// exit, interior, one-pose runs and their tie, runs at a terminus), and the
// bounce states it rewrites. Skipping over copied states is in
// `skip_tests.rs`; the direct half's copy, through the bake, in
// `direct_tests.rs`.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use postretro_level_format::mover_probe_channel::{MOVER_IRRADIANCE_TILE_F16_COUNT, PoseMark};

use super::{copy_bounce_edges, value_sources};
use crate::mover_probe_channel::states::{BounceProbe, MoverBounceStates};

fn even(count: usize) -> Vec<f32> {
    (0..count).map(|k| k as f32 / (count - 1) as f32).collect()
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
// the later one on a tie.
#[test]
fn a_one_pose_run_takes_the_nearer_clear_pose_and_the_later_on_a_tie() {
    let knots = [0.0, 0.2, 0.3, 0.6, 1.0];
    assert_eq!(value_sources(&runs("..#.."), &knots), [0, 1, 1, 3, 4]);
    let knots = [0.0, 0.2, 0.5, 0.6, 1.0];
    assert_eq!(value_sources(&runs("..#.."), &knots), [0, 1, 3, 3, 4]);
    assert_eq!(value_sources(&runs("..#.."), &even(5)), [0, 1, 3, 3, 4]);
    // Two one-pose runs either side of one clear pose: each its own tie.
    assert_eq!(value_sources(&runs(".#.#."), &even(5)), [0, 2, 2, 4, 4]);
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
    let knots = [0.0, 0.05, 0.3, 0.5, 0.7, 0.9, 1.0];
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

// A mover with no mid pose copies nothing.
#[test]
fn termini_alone_copy_nothing() {
    use PoseMark::{Stored as S, Unmarked as U};
    let only = probe(1, &[(S, 1), (U, 0)]);
    let mut states = MoverBounceStates::new(9, 2, vec![only.clone()], Vec::new());
    assert_eq!(
        copy_bounce_edges(&mut states, &[0.0, 1.0], |_| runs("#.")),
        0
    );
    assert_eq!(states.bounce, [only]);
}
