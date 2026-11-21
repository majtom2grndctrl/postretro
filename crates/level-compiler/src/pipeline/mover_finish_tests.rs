// Each mover finishes before the next bakes: its states reduce, join the
// tile-range union, skip, encode and drop. The job order never reaches the
// sections or the verdict, and no two movers' states are live at once.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Memory)

use postretro_level_format::mover_probe_channel::MoverProbeChannelSection;
use postretro_level_format::probe_depth_tiles::MAX_PROBE_DEPTH_TILES;

use super::{DEFAULT_BOUNCE_FILL, bake_base, closet_edited, open, plan_half, two_doors, with_ctx};
use crate::cache::StageCache;
use crate::fixture_pipeline::{FixturePipeline, fixture_path, load_map};
use crate::live_bytes::{measured, watch_group};
use crate::mover_probe_channel::{
    MoverDirectBake, MoverPayloadBars, MoverRecords, ProbeCells, channel_section,
};
use crate::pipeline::mover_bounce_stage::finish_bounce_half;

/// The ledger kinds holding a mover's states: its poses' raw states, read
/// or traced, and its reduced states.
const STATES: &[&str] = &["mover posed probes", "mover bounce states"];

/// A second door, `closet_door_b`, on the first's path and half its width:
/// two mover jobs whose poses meet different probes.
fn narrow_second_door(text: String) -> String {
    let start = text
        .find("{\n\"classname\" \"kinematic_mover\"")
        .expect("the door");
    let end = start
        + text[start..]
            .find("\n{\n\"classname\" \"kinematic_waypoint\"")
            .expect("its waypoints")
        + 1;
    let door = text[start..end]
        .replace("\"name\" \"closet_door\"", "\"name\" \"closet_door_b\"")
        .replace("( 368 ", "( 320 ");
    format!("{}{door}{}", &text[..end], &text[end..])
}

/// What the bounce half emits for one job order: id 57 assembled from its
/// blocks alone (the direct half never reads the job order), id 59 after
/// the mover tiles reserve their range, and the tile-range verdict.
#[derive(Debug, PartialEq)]
struct Emitted {
    channel: Option<Vec<u8>>,
    depth_tiles: Vec<u8>,
    refused: bool,
}

fn emitted(fixture: &FixturePipeline, reversed: bool, depth_cap: Option<u32>) -> Emitted {
    with_ctx(fixture, |ctx, poses| {
        let mut plan = plan_half(fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
        if reversed {
            plan.request.jobs.reverse();
        }
        if let Some(cap) = depth_cap {
            plan.request.depth_cap = cap;
        }
        let (_, mut base, outcome) =
            bake_base(ctx, &plan, &plan.request, DEFAULT_BOUNCE_FILL, None);
        if outcome.refused.is_some() {
            return Emitted {
                channel: None,
                depth_tiles: Vec::new(),
                refused: true,
            };
        }
        finish_bounce_half(
            &outcome.blocks,
            true,
            &base.section,
            &mut base.depth_tiles,
            MAX_PROBE_DEPTH_TILES,
        )
        .expect("within the range");
        let cells = ProbeCells::new(base.section.grid_dimensions, |probe| {
            base.section.probes[probe].validity != 0
        });
        let channel = channel_section(
            &MoverDirectBake::default(),
            &outcome.blocks,
            &cells,
            &MoverRecords::new(&fixture.kinematic_movers, poses),
            &MoverPayloadBars::default(),
            |_, _, _| false,
            |_| true,
        )
        .expect("assembles")
        .as_ref()
        .map(MoverProbeChannelSection::to_bytes);
        Emitted {
            channel,
            depth_tiles: base.depth_tiles.section.to_bytes(),
            refused: false,
        }
    })
}

// Two doors of different reach, in request order and reversed: id 57's
// bounce and depth blocks and id 59's trim are byte-identical, and so is the
// tile-range verdict, with a range the larger door alone passes and one
// both pass; one order stops the bake at its first door, the other does
// not (I2).
#[test]
fn reversed_job_order_keeps_sections_and_the_tile_range_verdict() {
    let maps = tempfile::tempdir().expect("maps dir");
    let fixture = load_map(&closet_edited(
        maps.path(),
        Some(2),
        "narrow-second-door",
        narrow_second_door,
    ));
    let carried = with_ctx(&fixture, |ctx, poses| {
        let plan = plan_half(&fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
        assert_eq!(plan.request.jobs.len(), 2, "two mover jobs");
        let (_, _, outcome) = bake_base(ctx, &plan, &plan.request, DEFAULT_BOUNCE_FILL, None);
        let each: Vec<usize> = outcome
            .reduced
            .iter()
            .map(|states| states.depth.len())
            .collect();
        let union = outcome
            .reduced
            .iter()
            .flat_map(|states| states.depth.iter().map(|probe| probe.probe))
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        (each, union)
    });
    let (each, union) = carried;
    let (small, large) = (each[0].min(each[1]), each[0].max(each[1]));
    assert!(
        small > 0 && small < large,
        "doors of different depth reach: {each:?}"
    );

    let forward = emitted(&fixture, false, None);
    assert!(
        !forward.refused && forward.channel.is_some(),
        "the full range admits"
    );
    assert_eq!(emitted(&fixture, true, None), forward, "full range");
    for (case, cap) in [
        ("the larger door alone past the range", large as u32 - 1),
        ("both doors past the range", small as u32 - 1),
    ] {
        let forward = emitted(&fixture, false, Some(cap));
        let reversed = emitted(&fixture, true, Some(cap));
        assert!(forward.refused, "{case}: refused");
        assert_eq!(reversed, forward, "{case}: the same verdict either way");
    }
    let at_union = emitted(&fixture, false, Some(union as u32));
    assert!(!at_union.refused, "a range the union fills admits");
    assert_eq!(emitted(&fixture, true, Some(union as u32)), at_union);
}

/// The most the states kinds held at once over one bake of `fixture` with
/// only the jobs `keep` picks, on four workers, over `cache`.
fn states_high_water(
    fixture: &FixturePipeline,
    keep: &(dyn Fn(u32) -> bool + Sync),
    cache: Option<&StageCache>,
) -> u64 {
    with_ctx(fixture, |ctx, poses| {
        let ((), report) = measured(4, || {
            watch_group("mover states", STATES);
            let mut plan = plan_half(fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
            plan.request.keep_states = false;
            plan.request.jobs.retain(|job| keep(job.mover_id));
            let (_, base, outcome) =
                bake_base(ctx, &plan, &plan.request, DEFAULT_BOUNCE_FILL, cache);
            assert!(outcome.refused.is_none());
            drop((base, outcome, plan));
        });
        report.groups["mover states"]
    })
}

/// Over `fixture`'s movers, cold, warm over an empty cache and on a full
/// hit: the states kinds never hold more than the largest one mover's do
/// baked alone in the same mode (M1).
fn one_movers_states_at_a_time(fixture: &FixturePipeline) {
    let ids: Vec<u32> = fixture
        .kinematic_movers
        .iter()
        .map(|mover| mover.mover_id)
        .collect();
    // `cache` makes each run's cache directory: none is cold.
    let check = |mode: &str, cache: &dyn Fn() -> Option<tempfile::TempDir>| {
        let run = |keep: &(dyn Fn(u32) -> bool + Sync)| {
            let dir = cache();
            let cache = dir.as_ref().map(|dir| open(dir.path()));
            states_high_water(fixture, keep, cache.as_ref())
        };
        let all = run(&|_| true);
        let alone = ids
            .iter()
            .map(|&id| run(&move |job| job == id))
            .max()
            .unwrap_or(0);
        assert!(alone > 0, "{mode}: a mover holds states");
        assert!(
            all <= alone,
            "{mode}: every mover's bake held {all} B of states, past the largest one mover's \
             {alone} B"
        );
        eprintln!("{mode}: states high-water {all} B, largest one mover alone {alone} B");
    };
    check("cold", &|| None);
    check("warm empty", &|| Some(tempfile::tempdir().expect("cache")));
    // Every run reads a copy of one primed cache, so each is a full hit.
    let primed = tempfile::tempdir().expect("cache");
    states_high_water(fixture, &|_| true, Some(&open(primed.path())));
    check("full hit", &|| {
        let copy = tempfile::tempdir().expect("cache");
        for entry in std::fs::read_dir(primed.path()).expect("primed cache") {
            let entry = entry.expect("entry");
            if entry.file_type().expect("type").is_file() {
                std::fs::copy(entry.path(), copy.path().join(entry.file_name())).expect("copy");
            }
        }
        Some(copy)
    });
}

#[test]
fn one_movers_states_are_live_at_a_time_on_the_two_door_closet() {
    let maps = tempfile::tempdir().expect("maps dir");
    let fixture = load_map(&closet_edited(maps.path(), Some(2), "two-doors", two_doors));
    one_movers_states_at_a_time(&fixture);
}

#[test]
#[ignore = "bakes mover-setpiece cold, warm and on a hit, with and without each mover; run on demand"]
fn one_movers_states_are_live_at_a_time_on_the_setpiece() {
    one_movers_states_at_a_time(&load_map(&fixture_path("mover-setpiece")));
}
