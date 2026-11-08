// Probe depth tile selection on dev maps: which probes the bake tiles, its determinism, cache and cap.
// See: context/lib/build_pipeline.md §PRL section IDs · §Build Cache

use std::collections::BTreeSet;

use log::Level;
use postretro_level_format::probe_depth_tiles::{MAX_PROBE_DEPTH_TILES, f16_bits_to_f32};
use postretro_test_log_capture::LogCapture;

use super::depth_tiles::DepthTileSelector;
use super::depth_tiles_test_fixtures::DepthTileFixture;
use super::grid::probe_index_to_xyz;
use super::probe::probe_gather_directions;
use super::{ProbeGridLayout, segment_clear, vec3_from};
use crate::cache::StageCache;

const STATIC_DOOR: &str = "mover-closet-static-door";

/// Valid probes with a valid in-grid lattice neighbour that the segment
/// between their bake positions cannot reach: geometry blocks it.
fn probes_with_a_blocked_valid_neighbour(
    fixture: &DepthTileFixture,
    layout: &ProbeGridLayout,
) -> BTreeSet<u32> {
    let dims = layout.dims;
    fixture.with_ctx(|ctx| {
        let ray = ctx.ray_ctx();
        (0..layout.total_probes())
            .filter(|&probe| layout.validity[probe] != 0)
            .filter(|&probe| {
                let (x, y, z) = probe_index_to_xyz(probe, dims);
                let from = vec3_from(layout.probe_positions[probe]);
                (-1i64..=1).any(|oz| {
                    (-1i64..=1).any(|oy| {
                        (-1i64..=1).any(|ox| {
                            let n = [i64::from(x) + ox, i64::from(y) + oy, i64::from(z) + oz];
                            if (ox, oy, oz) == (0, 0, 0)
                                || n.iter()
                                    .zip(dims)
                                    .any(|(&c, d)| !(0..i64::from(d)).contains(&c))
                            {
                                return false;
                            }
                            let neighbour = n[0] as usize
                                + n[1] as usize * dims[0] as usize
                                + n[2] as usize * dims[0] as usize * dims[1] as usize;
                            layout.validity[neighbour] != 0
                                && !segment_clear(
                                    &ray,
                                    from,
                                    vec3_from(layout.probe_positions[neighbour]),
                                )
                        })
                    })
                })
            })
            .map(|probe| probe as u32)
            .collect()
    })
}

// Every probe beside the static door, the only thin thing between the
// closet and the bright room, has a valid lattice neighbour across it; each
// of those probes gets a tile.
#[test]
fn the_static_door_closet_tiles_every_probe_with_a_blocked_valid_neighbour() {
    let fixture = DepthTileFixture::load(STATIC_DOOR);
    let layout = fixture.layout();
    let blocked = probes_with_a_blocked_valid_neighbour(&fixture, &layout);
    let base = fixture.bake(MAX_PROBE_DEPTH_TILES, None);
    let tiled: BTreeSet<u32> = base
        .depth_tiles
        .section
        .probe_indices
        .iter()
        .copied()
        .collect();
    let valid = layout.validity.iter().filter(|&&v| v != 0).count();
    eprintln!(
        "{STATIC_DOOR}: {valid} valid probes, {} with a blocked valid neighbour, {} tiled, {} dropped",
        blocked.len(),
        tiled.len(),
        base.depth_tiles.dropped,
    );
    assert!(!blocked.is_empty(), "the door blocks some lattice segment");
    let missed: Vec<String> = blocked
        .difference(&tiled)
        .map(|&probe| {
            let (x, y, z) = probe_index_to_xyz(probe as usize, layout.dims);
            format!(
                "{probe} at ({x}, {y}, {z}) {:?}",
                layout.probe_positions[probe as usize]
            )
        })
        .collect();
    assert!(
        missed.is_empty(),
        "blocked probes without a tile: {missed:#?}"
    );
    assert_eq!(base.depth_tiles.dropped, 0);
}

// Two cold bakes, on one worker and on eight, give the same tiles.
#[test]
fn depth_tiles_are_deterministic_across_builds_and_thread_counts() {
    let fixture = DepthTileFixture::load(STATIC_DOOR);
    let on = |threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("pool")
            .install(|| fixture.bake(MAX_PROBE_DEPTH_TILES, None))
    };
    let one = on(1);
    assert!(one.depth_tiles.section.tile_count() > 0);
    assert_eq!(on(8), one);
    assert_eq!(on(8), one);
}

// O1: a cold build, then two warm builds of the same inputs. The first warm
// build writes the volume entry, the second hits it; each returns the cold
// tiles and logs the kept and dropped counts once.
#[test]
fn a_warm_hit_returns_the_cold_tiles_and_logs_the_dropped_count_again() {
    let fixture = DepthTileFixture::load(STATIC_DOOR);
    let capture = LogCapture::start();
    let cold = fixture.bake(MAX_PROBE_DEPTH_TILES, None);
    let kept = cold.depth_tiles.section.tile_count();
    assert!(kept > 0);
    let line = format!(
        "Probe depth tiles: {kept} kept, {} dropped past the {MAX_PROBE_DEPTH_TILES}-tile cap",
        cold.depth_tiles.dropped
    );
    capture.assert_logged_once(Level::Info, &line);

    let dir = tempfile::tempdir().expect("cache dir");
    for build in ["warm miss", "warm hit"] {
        capture.clear();
        let cache = StageCache::new(dir.path()).expect("cache");
        let warm = fixture.bake(MAX_PROBE_DEPTH_TILES, Some(&cache));
        let access = cache.test_access(crate::surface_radiance::SH_BASE_VOLUME_STAGE_ID);
        let hit = build == "warm hit";
        assert_eq!(
            (access.read_hits, access.writes),
            if hit { (1, 0) } else { (0, 1) },
            "{build}: {access:?}"
        );
        assert_eq!(
            warm.depth_tiles.section.to_bytes(),
            cold.depth_tiles.section.to_bytes(),
            "{build}: id 59 bytes"
        );
        assert_eq!(warm, cold, "{build}");
        capture.assert_logged_once(Level::Info, &line);
    }
}

// O9 on a real map: with the cap at half the closet's tiles, the bake keeps
// the half whose visibility the tile changes most, the lower index on a tie,
// in ascending order, each tile as the uncapped bake made it, and logs the
// dropped count.
#[test]
fn a_lowered_cap_keeps_the_closets_largest_visibility_changes() {
    let fixture = DepthTileFixture::load(STATIC_DOOR);
    let layout = fixture.layout();
    let full = fixture.bake(MAX_PROBE_DEPTH_TILES, None);
    let tiles = &full.depth_tiles.section;
    assert!(tiles.tile_count() >= 2);
    let cap = (tiles.tile_count() / 2) as u32;

    // Each kept probe's change, measured as the bake measures it.
    let selector =
        DepthTileSelector::new(&layout, &probe_gather_directions(), MAX_PROBE_DEPTH_TILES);
    let mut ranked: Vec<(f32, u32)> = tiles
        .probe_indices
        .iter()
        .enumerate()
        .map(|(tile, &probe)| {
            let record = &full.section.probes[probe as usize];
            let averaged = [
                f16_bits_to_f32(record.mean_distance),
                f16_bits_to_f32(record.mean_sq_distance),
            ];
            let change = selector.visibility_change(probe as usize, averaged, tiles.tile(tile));
            (change, probe)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut expected: Vec<u32> = ranked[..cap as usize]
        .iter()
        .map(|&(_, probe)| probe)
        .collect();
    expected.sort_unstable();

    let capture = LogCapture::start();
    let capped = fixture.bake(cap, None);
    assert_eq!(capped.depth_tiles.section.probe_indices, expected);
    for (tile, &probe) in expected.iter().enumerate() {
        let source = tiles
            .tile_of_probe(probe)
            .expect("the uncapped bake tiled it");
        assert_eq!(
            capped.depth_tiles.section.tile(tile),
            tiles.tile(usize::from(source)),
            "probe {probe}"
        );
    }
    let dropped = tiles.tile_count() as u64 - u64::from(cap);
    assert_eq!(capped.depth_tiles.dropped, dropped);
    capture.assert_logged_once(
        Level::Info,
        &format!("Probe depth tiles: {cap} kept, {dropped} dropped past the {cap}-tile cap"),
    );
    assert_eq!(capped.section, full.section, "the cap moves no id-34 byte");
}
