// Mover depth's tile range: live tiles reserved ahead of id 59,
// id 59 re-trimmed by its own rank, and a named error past the range.
// See: context/lib/build_pipeline.md §PRL section IDs (ProbeDepthTiles, MoverProbeChannel)

use postretro_level_format::probe_depth_tiles::MAX_PROBE_DEPTH_TILES;

use postretro_level_format::mover_probe_channel::PoseMark;
use postretro_level_format::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;

use super::{CarriedDepth, MoverTileRangeExceeded, refuse_cut_short, reserve_mover_tiles};
use crate::mover_probe_channel::{DepthProbe, MoverBounceStates};
use crate::sh_bake::depth_tiles_test_fixtures::DepthTileFixture;

/// The static-door closet: id 59 tiles its door probes.
const STATIC_DOOR: &str = "mover-closet-static-door";

// With `live` mover tiles reserved in a range of
// `cap + live`, id 59 keeps exactly the tiles a bake capped at `cap` keeps,
// whose candidates fill it, and counts the extra drops; the bytes of every
// kept tile are the uncapped bake's.
#[test]
fn reserving_mover_tiles_keeps_what_a_selection_at_the_shrunk_cap_keeps() {
    let fixture = DepthTileFixture::load(STATIC_DOOR);
    let full = fixture.bake(MAX_PROBE_DEPTH_TILES, None);
    let kept = full.depth_tiles.section.tile_count() as u32;
    assert!(kept >= 4, "the closet tiles its door probes: {kept}");
    for live in [1u32, kept / 2] {
        let shrunk = kept - live;
        let capped = fixture.bake(shrunk, None);
        let mut tiles = full.depth_tiles.clone();
        let reservation = reserve_mover_tiles(
            &mut tiles,
            &full.section,
            CarriedDepth::exact(live as usize),
            kept,
        )
        .expect("the live tiles fit the range");
        assert_eq!(reservation.live, live as usize);
        assert_eq!(reservation.shrunk_cap, shrunk);
        assert_eq!(reservation.trimmed, u64::from(live));
        assert_eq!(
            tiles.section, capped.depth_tiles.section,
            "{live} live tile(s): id 59 keeps the shrunk cap's selection"
        );
        assert_eq!(
            tiles.dropped,
            full.depth_tiles.dropped + u64::from(live),
            "the trimmed tiles count as dropped"
        );
    }
}

// Id 59 and the live tiles exactly at the range keep every
// tile; one live tile past it drops one more id-59 tile; the live tiles
// alone past the range fail by name and drop nothing.
#[test]
fn the_tile_range_holds_live_tiles_first_and_refuses_them_past_it_by_name() {
    let fixture = DepthTileFixture::load(STATIC_DOOR);
    let full = fixture.bake(MAX_PROBE_DEPTH_TILES, None);
    let kept = full.depth_tiles.section.tile_count();
    let live = 3usize;
    let cap = (kept + live) as u32;

    let mut at_cap = full.depth_tiles.clone();
    let reservation =
        reserve_mover_tiles(&mut at_cap, &full.section, CarriedDepth::exact(live), cap)
            .expect("fits");
    assert_eq!(reservation.trimmed, 0);
    assert_eq!(
        at_cap, full.depth_tiles,
        "exactly at the range, nothing drops"
    );

    let mut over = full.depth_tiles.clone();
    let reservation =
        reserve_mover_tiles(&mut over, &full.section, CarriedDepth::exact(live + 1), cap)
            .expect("fits");
    assert_eq!(reservation.trimmed, 1);
    assert_eq!(over.section.tile_count(), kept - 1);

    let mut refused = full.depth_tiles.clone();
    let carried = CarriedDepth::exact(cap as usize + 1);
    let error = reserve_mover_tiles(&mut refused, &full.section, carried, cap)
        .expect_err("live tiles alone past the range");
    assert_eq!(error, MoverTileRangeExceeded { carried, cap });
    let message = error.to_string();
    assert!(
        message.contains("MoverProbeChannel (id 57)")
            && message.contains(&format!(": {} probes carry", cap + 1))
            && message.contains("no mover tile is dropped"),
        "{message}"
    );
    let partial = CarriedDepth {
        exact: false,
        ..carried
    };
    let message = reserve_mover_tiles(&mut refused, &full.section, partial, cap)
        .expect_err("a partial count past the range")
        .to_string();
    assert!(
        message.contains(&format!("at least {} probes carry", cap + 1)),
        "a partial count reads as a lower bound: {message}"
    );
    assert_eq!(
        refused, full.depth_tiles,
        "a refused reservation changes nothing"
    );
}

/// A three-pose mover whose depth records store at its mid pose alone, one
/// per probe in `probes`, held whole.
fn mid_pose_depth(mover_id: u32, probes: std::ops::Range<u32>) -> MoverBounceStates {
    let depth = probes
        .map(|probe| DepthProbe {
            probe,
            marks: vec![PoseMark::Unmarked, PoseMark::Stored, PoseMark::Unmarked],
            tiles: vec![[0u16; 2]; PROBE_DEPTH_TILE_TEXELS],
        })
        .collect();
    MoverBounceStates::new(mover_id, 3, Vec::new(), depth)
}

// Regression: a mover carrying exactly one depth record past the range, the
// last job, was neither stopped nor cut, so its skips could bring the count
// back within the range and the build passed, where any later job would
// have stopped the bake and refused it.
#[test]
fn depth_past_the_tile_range_is_refused_before_skips_whatever_the_job_order() {
    let cap = 3u32;
    let one_past = [mid_pose_depth(0, 0..cap + 1)];
    assert!(
        !one_past[0].depth_cut(),
        "one record past the range is no cut"
    );
    let error = refuse_cut_short(&one_past, false, cap).expect_err("one record past the range");
    let carried = CarriedDepth::exact(cap as usize + 1);
    assert_eq!(error, MoverTileRangeExceeded { carried, cap });
    let message = error.to_string();
    assert!(
        message.contains(&format!(": {} probes carry", cap + 1)) && !message.contains("at least"),
        "the whole count, exact: {message}"
    );

    // Two movers each within the range whose union passes it: refused with
    // the union, exact, in either job order.
    let first = mid_pose_depth(0, 0..2);
    let second = mid_pose_depth(1, 2..4);
    for movers in [[first.clone(), second.clone()], [second, first]] {
        let error = refuse_cut_short(&movers, false, cap).expect_err("the union passes the range");
        assert_eq!(
            error,
            MoverTileRangeExceeded {
                carried: CarriedDepth::exact(4),
                cap
            }
        );
    }

    // Overlapping movers whose union fills the range exactly pass.
    let at_range = [mid_pose_depth(0, 0..cap), mid_pose_depth(1, 0..cap)];
    refuse_cut_short(&at_range, false, cap).expect("the union fills the range exactly");
}
