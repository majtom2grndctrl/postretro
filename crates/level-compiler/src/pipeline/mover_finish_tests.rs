// Each mover finishes before the next bakes: its states reduce, join the
// tile-range union, skip, encode and drop. The job order never reaches the
// sections or the verdict, and no two movers' states are live at once.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Memory)

use std::path::Path;

use postretro_level_format::mover_probe_channel::MoverProbeChannelSection;
use postretro_level_format::probe_depth_tiles::MAX_PROBE_DEPTH_TILES;

use super::{
    DEFAULT_BOUNCE_FILL, bake_base, closet_edited, open, plan_half, plan_half_at, two_doors,
    with_ctx,
};
use crate::bake_fidelity::{BakeFidelity, BakeMode};
use crate::cache::StageCache;
use crate::fixture_pipeline::{FixturePipeline, fixture_path, load_map};
use crate::live_bytes::{measured, watch_group};
use crate::mover_probe_channel::{
    BounceBlocks, MoverBounceRefusal, MoverBounceRequest, MoverDirectBake, MoverPayloadBars,
    MoverRecords, ProbeCells, channel_section,
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
    emitted_with(fixture, HIGH, None, |request| {
        if reversed {
            request.jobs.reverse();
        }
        if let Some(cap) = depth_cap {
            request.depth_cap = cap;
        }
    })
}

const HIGH: BakeMode = BakeMode::Interactive(BakeFidelity::High);
const LOW: BakeMode = BakeMode::Interactive(BakeFidelity::Low);

/// [`emitted`] at `mode`, over `cache`, with `edit` applied to the request.
fn emitted_with(
    fixture: &FixturePipeline,
    mode: BakeMode,
    cache: Option<&StageCache>,
    edit: impl FnOnce(&mut MoverBounceRequest),
) -> Emitted {
    with_ctx(fixture, |ctx, poses| {
        let mut plan = plan_half_at(fixture, ctx, poses, DEFAULT_BOUNCE_FILL, mode);
        edit(&mut plan.request);
        let (_, mut base, outcome) =
            bake_base(ctx, &plan, &plan.request, DEFAULT_BOUNCE_FILL, cache);
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
    states_high_water_ordered(fixture, keep, cache, false)
}

/// [`states_high_water`], the jobs `reversed` when asked.
fn states_high_water_ordered(
    fixture: &FixturePipeline,
    keep: &(dyn Fn(u32) -> bool + Sync),
    cache: Option<&StageCache>,
    reversed: bool,
) -> u64 {
    with_ctx(fixture, |ctx, poses| {
        let ((), report) = measured(4, || {
            watch_group("mover states", STATES);
            let mut plan = plan_half(fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
            plan.request.keep_states = false;
            plan.request.jobs.retain(|job| keep(job.mover_id));
            if reversed {
                plan.request.jobs.reverse();
            }
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

/// The two-door closet with the second door's `light_bounces` set to
/// `bounces` (both start at 2).
fn second_door_bounces(maps: &Path, bounces: u32) -> FixturePipeline {
    load_map(&closet_edited(
        maps,
        Some(2),
        &format!("two-doors-b{bounces}"),
        |text| {
            let text = two_doors(text);
            let b = "\"name\" \"closet_door_b\"\n\"light_bounces\" \"2\"";
            assert!(text.contains(b), "the second door's key");
            text.replace(
                b,
                &format!("\"name\" \"closet_door_b\"\n\"light_bounces\" \"{bounces}\""),
            )
        },
    ))
}

/// Copy `from`'s cache entries into a fresh directory.
fn cache_copy(from: &Path) -> tempfile::TempDir {
    let copy = tempfile::tempdir().expect("cache");
    for entry in std::fs::read_dir(from).expect("cache dir") {
        let entry = entry.expect("entry");
        if entry.file_type().expect("type").is_file() {
            std::fs::copy(entry.path(), copy.path().join(entry.file_name())).expect("copy");
        }
    }
    copy
}

// The two-door closet, warm over a cache primed with the second door's
// `light_bounces` changed: the first door hits every pose, the second
// bakes. In either job order the states kinds never hold more than the
// larger of the held door read back alone and the baking door baked alone,
// so the held door's states are never live during the other's bake (M2).
#[test]
fn a_held_door_is_not_live_while_the_other_bakes() {
    let maps = tempfile::tempdir().expect("maps dir");
    let primer = second_door_bounces(maps.path(), 1);
    let fixture = second_door_bounces(maps.path(), 2);
    let primed = tempfile::tempdir().expect("cache");
    states_high_water(&primer, &|_| true, Some(&open(primed.path())));
    let ids: Vec<u32> = fixture
        .kinematic_movers
        .iter()
        .map(|mover| mover.mover_id)
        .collect();
    let run = |keep: &(dyn Fn(u32) -> bool + Sync), reversed: bool| {
        let copy = cache_copy(primed.path());
        states_high_water_ordered(&fixture, keep, Some(&open(copy.path())), reversed)
    };
    let alone = ids
        .iter()
        .map(|&id| run(&move |job| job == id, false))
        .max()
        .expect("two doors");
    for reversed in [false, true] {
        let all = run(&|_| true, reversed);
        assert!(
            all <= alone,
            "reversed {reversed}: both doors held {all} B of states, past one door's {alone} B"
        );
        eprintln!("reversed {reversed}: states high-water {all} B, one door alone {alone} B");
    }
}

// Warm over a cache whose lookup found every pose, a door's read-back
// fails at its slot: that pose bakes there, bounce one and id 34's later
// bounces built then, and every emitted section equals a cold build's, at
// `low` and `high` with no higher entries cached; with both doors held that
// way, and with one held and the other pending (M3).
#[test]
fn a_failed_slot_read_bakes_the_pose_and_matches_cold() {
    let maps = tempfile::tempdir().expect("maps dir");
    let primer = second_door_bounces(maps.path(), 1);
    let fixture = second_door_bounces(maps.path(), 2);
    let ids: Vec<u32> = fixture
        .kinematic_movers
        .iter()
        .map(|mover| mover.mover_id)
        .collect();
    for mode in [LOW, HIGH] {
        let cold = emitted_with(&fixture, mode, None, |_| {});
        assert!(cold.channel.is_some(), "{mode:?}: the doors emit id 57");
        // Every pose held: one pose of each door fails at its slot.
        let full = tempfile::tempdir().expect("cache");
        emitted_with(&fixture, mode, Some(&open(full.path())), |_| {});
        let copy = cache_copy(full.path());
        let failed = emitted_with(&fixture, mode, Some(&open(copy.path())), |request| {
            request.fail_read_back = vec![(ids[0], 0), (ids[1], 1)];
        });
        assert_eq!(
            failed, cold,
            "{mode:?}: every door held, a read-back failing"
        );
        // The second door pending: the first's read-back fails.
        let primed = tempfile::tempdir().expect("cache");
        emitted_with(&primer, mode, Some(&open(primed.path())), |_| {});
        let failed = emitted_with(&fixture, mode, Some(&open(primed.path())), |request| {
            request.fail_read_back = vec![(ids[0], 1)];
        });
        assert_eq!(failed, cold, "{mode:?}: one door held, the other pending");
    }
}

// At `low`, warm over a cache primed at `high`: the lookup serves the
// doors' poses from their `high` entries. Where one of those reads fails at
// its slot, the pose bakes at the `high` count, and every emitted section
// equals the same build with the read succeeding (M5).
#[test]
fn a_high_read_failing_at_the_slot_bakes_at_the_high_count() {
    let maps = tempfile::tempdir().expect("maps dir");
    let fixture = second_door_bounces(maps.path(), 2);
    let id = fixture.kinematic_movers[0].mover_id;
    let primed = tempfile::tempdir().expect("cache");
    emitted_with(&fixture, HIGH, Some(&open(primed.path())), |_| {});
    let served = emitted_with(
        &fixture,
        LOW,
        Some(&open(cache_copy(primed.path()).path())),
        |_| {},
    );
    let low_cold = emitted_with(&fixture, LOW, None, |_| {});
    assert_ne!(served, low_cold, "the `high` entries serve the `low` build");
    let failed = emitted_with(
        &fixture,
        LOW,
        Some(&open(cache_copy(primed.path()).path())),
        |request| {
            request.fail_read_back = vec![(id, 0)];
        },
    );
    assert_eq!(failed, served, "the failed pose bakes at the `high` count");
}

/// One bake of `fixture` at `high` with `edit` applied to the request: its
/// refusal, the blocks' bytes and the blocks.
fn refusal_of(
    fixture: &FixturePipeline,
    edit: impl FnOnce(&mut MoverBounceRequest),
) -> (Option<MoverBounceRefusal>, u64, BounceBlocks) {
    with_ctx(fixture, |ctx, poses| {
        let mut plan = plan_half_at(fixture, ctx, poses, DEFAULT_BOUNCE_FILL, HIGH);
        edit(&mut plan.request);
        let (_, _, outcome) = bake_base(ctx, &plan, &plan.request, DEFAULT_BOUNCE_FILL, None);
        let bytes = outcome.blocks.heap_bytes();
        (outcome.refused, bytes, outcome.blocks)
    })
}

// The payload cap counts every byte the bounce and depth blocks hold: a cap
// equal to the two doors' blocks admits them unchanged, and one byte under
// fails as the second door's blocks would join, naming the flag, the
// blocks' bytes and the door joined before it; nothing is dropped (C1).
#[test]
fn the_payload_cap_fails_by_name_and_admits_at_its_bytes() {
    let maps = tempfile::tempdir().expect("maps dir");
    let fixture = load_map(&closet_edited(maps.path(), Some(2), "two-doors", two_doors));
    let (refused, bytes, blocks) = refusal_of(&fixture, |request| {
        request.payload_cap = u64::MAX;
    });
    assert!(refused.is_none() && bytes > 0, "the doors emit blocks");
    let (refused, at_cap, capped) = refusal_of(&fixture, |request| {
        request.payload_cap = bytes;
    });
    assert!(
        refused.is_none(),
        "a cap equal to the blocks' bytes admits them"
    );
    assert_eq!(
        (at_cap, &capped),
        (bytes, &blocks),
        "nothing dropped or coarsened"
    );
    let (refused, _, _) = refusal_of(&fixture, |request| {
        request.payload_cap = bytes - 1;
    });
    let Some(MoverBounceRefusal::PayloadCap(error)) = refused else {
        panic!("one byte under the blocks' bytes fails by the cap: {refused:?}");
    };
    assert_eq!((error.bytes, error.cap), (bytes, bytes - 1));
    assert_eq!(error.mover, "kinematic_mover 'closet_door_b'");
    assert_eq!(error.joined, "kinematic_mover 'closet_door'");
    let message = error.to_string();
    assert!(
        message.contains("--mover-payload-max-size")
            && message.contains(&bytes.to_string())
            && message.contains("closet_door_b")
            && message.contains("joined so far: kinematic_mover 'closet_door'"),
        "{message}"
    );
}

// One door whose depth passes the tile range and whose blocks would pass
// the cap: the tile range refuses it, before it skips or encodes (I4).
#[test]
fn a_mover_past_both_fails_by_the_tile_range_before_skipping() {
    let maps = tempfile::tempdir().expect("maps dir");
    let fixture = load_map(&super::closet(maps.path(), Some(2)));
    let (refused, bytes, blocks) = refusal_of(&fixture, |request| {
        request.depth_cap = 0;
        request.payload_cap = 0;
    });
    assert!(
        matches!(refused, Some(MoverBounceRefusal::TileRange(_))),
        "{refused:?}"
    );
    assert_eq!(
        (bytes, blocks),
        (0, BounceBlocks::default()),
        "nothing encoded or joined"
    );
}

// The narrow door's blocks alone pass the payload cap; the wide door's
// depth alone passes the tile range. Each job order fails by name: the
// narrow door first fails by the cap, the wide door first by the range
// (I3).
#[test]
fn cap_and_range_on_different_movers_fail_by_name_in_both_orders() {
    let maps = tempfile::tempdir().expect("maps dir");
    let fixture = load_map(&closet_edited(
        maps.path(),
        Some(2),
        "narrow-second-door",
        narrow_second_door,
    ));
    let narrow = fixture
        .kinematic_movers
        .iter()
        .find(|mover| mover.name == "closet_door_b")
        .expect("the narrow door")
        .mover_id;
    let (reduced, narrow_bytes) = with_ctx(&fixture, |ctx, poses| {
        let mut plan = plan_half_at(&fixture, ctx, poses, DEFAULT_BOUNCE_FILL, HIGH);
        plan.request.jobs.retain(|job| job.mover_id == narrow);
        let (_, _, outcome) = bake_base(ctx, &plan, &plan.request, DEFAULT_BOUNCE_FILL, None);
        (outcome.reduced, outcome.blocks.heap_bytes())
    });
    let narrow_depth = reduced[0].depth.len() as u32;
    let edit = |narrow_first: bool| {
        move |request: &mut MoverBounceRequest| {
            request.depth_cap = narrow_depth;
            request.payload_cap = narrow_bytes - 1;
            request
                .jobs
                .sort_by_key(|job| (job.mover_id == narrow) != narrow_first);
        }
    };
    let (refused, ..) = refusal_of(&fixture, edit(true));
    assert!(
        matches!(refused, Some(MoverBounceRefusal::PayloadCap(_))),
        "narrow door first: {refused:?}"
    );
    let (refused, ..) = refusal_of(&fixture, edit(false));
    assert!(
        matches!(refused, Some(MoverBounceRefusal::TileRange(_))),
        "wide door first: {refused:?}"
    );
}
