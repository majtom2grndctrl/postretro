//! Mover depth under a lowered probe depth tile range (bake rows B5 and B6, through the binary):
//! the closet's live tiles are reserved ahead of id 59, which drops its lowest-ranked tiles to
//! make room and reports them against the shrunk cap; live tiles alone past the range fail the
//! build by name; and a warm mover-only edit that changes the live-tile count traces no id-34
//! ray and packs the cold PRL.
//! See: context/lib/build_pipeline.md §PRL section IDs, §Build Cache

mod mover_bounce_support;
mod prl_build_support;

use mover_bounce_support::{Built, Cache, build, channel, depth_tiles, dev_map, try_build};

/// The closet's door as authored: its open pose in the lintel.
const LINTEL: &str = "320 496 168";

/// An open pose standing inside the closet, where it shadows probes too.
const INSIDE: &str = "320 600 56";

/// The closet with its door's open waypoint at `open` (authored [`LINTEL`]).
fn closet(open: &str) -> String {
    dev_map("mover-closet.map").replace(
        &format!("\"origin\" \"{LINTEL}\""),
        &format!("\"origin\" \"{open}\""),
    )
}

/// The live-tile count and id 59's tile count a build packed.
fn counts(built: &Built) -> (usize, usize) {
    let live = channel(&built.prl).map_or(0, |channel| channel.live_tile_count());
    (live, depth_tiles(&built.prl).tile_count())
}

fn assert_reports(built: &Built, cap: u32) {
    let (live, id59) = counts(built);
    let shrunk = cap as usize - live;
    assert_eq!(id59, shrunk, "id 59 fills what the live tiles leave");
    for needle in [
        format!("Probe depth tiles: {id59} kept, "),
        format!("Probe depth tiles: {id59} tiles, "),
    ] {
        let line = built
            .logs
            .lines()
            .find(|line| line.contains(&needle))
            .unwrap_or_else(|| panic!("a line with `{needle}`\n{}", built.logs));
        assert!(
            line.contains(&format!("past the {shrunk}-tile cap")),
            "the line reports the shrunk cap {shrunk}: {line}"
        );
    }
}

// Bake rows B5 and B6 (P31): with the range lowered below id 59 plus the
// door's live tiles, id 59 keeps exactly the range less the live tiles, and
// the log and summary report that count against the shrunk cap. With the door's
// open pose inside the closet, moving it out into the hall changes the
// live-tile count; warm over the base
// cache, the edit hits id 34's volume, re-bakes the door's bounce, re-trims
// id 59 and packs the edited map's cold bytes.
#[test]
fn a_lowered_range_trims_id_59_for_mover_tiles_and_a_mover_edit_retrims_warm() {
    let cap = 70;
    let flags = ["--probe-depth-tile-cap", "70"];
    let cache = tempfile::tempdir().expect("create cache");
    let base = build(
        &closet(INSIDE),
        "mover-closet",
        Cache::Warm(cache.path()),
        &flags,
    );
    assert_reports(&base, cap);
    let (base_live, _) = counts(&base);
    assert!(base_live > 0, "the closed door carries depth");

    let edited = closet("320 400 56");
    let warm = build(&edited, "mover-closet", Cache::Warm(cache.path()), &flags);
    let cold = build(&edited, "mover-closet", Cache::Cold, &flags);
    assert!(
        warm.logs.contains("[cache] sh_base_volume hit"),
        "the edit traces no id-34 ray\n{}",
        warm.logs
    );
    assert!(
        warm.logs
            .contains("[cache] mover_probe_bounce miss (1 of 1 mover(s) re-bake)"),
        "{}",
        warm.logs
    );
    let (edited_live, _) = counts(&warm);
    assert_ne!(
        edited_live, base_live,
        "the edit changes the live-tile count"
    );
    assert_reports(&warm, cap);
    assert!(
        std::fs::read(&warm.prl).expect("warm PRL") == std::fs::read(&cold.prl).expect("cold PRL"),
        "the warm PRL equals the edited map's cold PRL"
    );
}

// Bake row B5: a range the door's live tiles alone exceed fails the build by
// name before packing; one exactly at id 59 plus the live tiles packs, drops
// nothing, and loads.
#[test]
fn live_tiles_past_the_range_fail_by_name_and_a_range_exactly_full_packs_and_loads() {
    let full = build(&closet(LINTEL), "mover-closet", Cache::Cold, &[]);
    let (live, id59) = counts(&full);
    let exact = (live + id59).to_string();
    let at_cap = build(
        &closet(LINTEL),
        "mover-closet",
        Cache::Cold,
        &["--probe-depth-tile-cap", &exact],
    );
    assert_eq!(
        counts(&at_cap),
        (live, id59),
        "exactly at the range, nothing drops"
    );
    postretro_level_loader::load_prl(at_cap.prl.to_str().expect("UTF-8 path"))
        .expect("a PRL exactly at the range loads");

    let short = (live - 1).to_string();
    let logs = try_build(
        &closet(LINTEL),
        "mover-closet",
        Cache::Cold,
        &["--probe-depth-tile-cap", &short],
    )
    .err()
    .expect("live tiles past the range fail the build");
    assert!(
        logs.contains("MoverProbeChannel (id 57)")
            && logs.contains(&format!("{live} probes carry closed-pose depth"))
            && logs.contains("no mover tile is dropped"),
        "{logs}"
    );
}
