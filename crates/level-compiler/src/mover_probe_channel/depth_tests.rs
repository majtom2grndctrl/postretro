// Mover depth's tile range: live tiles reserved ahead of id 59,
// id 59 re-trimmed by its own rank, and a named error past the range.
// See: context/lib/build_pipeline.md §PRL section IDs (ProbeDepthTiles, MoverProbeChannel)

use postretro_level_format::probe_depth_tiles::MAX_PROBE_DEPTH_TILES;

use super::{MoverTileRangeExceeded, reserve_mover_tiles};
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
        let reservation = reserve_mover_tiles(&mut tiles, &full.section, live as usize, kept)
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
    let reservation = reserve_mover_tiles(&mut at_cap, &full.section, live, cap).expect("fits");
    assert_eq!(reservation.trimmed, 0);
    assert_eq!(
        at_cap, full.depth_tiles,
        "exactly at the range, nothing drops"
    );

    let mut over = full.depth_tiles.clone();
    let reservation = reserve_mover_tiles(&mut over, &full.section, live + 1, cap).expect("fits");
    assert_eq!(reservation.trimmed, 1);
    assert_eq!(over.section.tile_count(), kept - 1);

    let mut refused = full.depth_tiles.clone();
    let error = reserve_mover_tiles(&mut refused, &full.section, cap as usize + 1, cap)
        .expect_err("live tiles alone past the range");
    assert_eq!(
        error,
        MoverTileRangeExceeded {
            live: cap as usize + 1,
            cap,
        }
    );
    let message = error.to_string();
    assert!(
        message.contains("MoverProbeChannel (id 57)")
            && message.contains("no mover tile is dropped"),
        "{message}"
    );
    assert_eq!(
        refused, full.depth_tiles,
        "a refused reservation changes nothing"
    );
}
