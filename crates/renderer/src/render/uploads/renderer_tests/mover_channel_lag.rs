//! GPU half on compiled maps, on the streamed path: a channel row that
//! lags composes before any frame samples it. A row lags when its mover's t′
//! changes while the row is outside the compose gate, or while the row is
//! not resident. The frame that admits it (back into the gate, or installed)
//! composes it at the current t′ before that frame's consumers sample it, so
//! its tiles read back after that frame's submit, which is what the frame
//! sampled, equal a whole compose at that t′, bit for bit, though t′ did not
//! change on that frame.
//! See: context/lib/rendering_pipeline.md §4 (Sampled-row compose, Mover
//! state lighting)
use super::real_level::{Gate, RealLevel};
use crate::render::composed_tiles::{ATLAS_NAMES, ComposedReadback, TILE_HALVES};
use postretro_level_loader::ShStreamingMode;
use std::collections::{BTreeMap, BTreeSet};

type Tiles = BTreeMap<u32, [Option<[u16; TILE_HALVES]>; 3]>;

/// Every channel probe's tiles in the three composed atlases; a probe not
/// resident reads `None` in all three.
fn channel_tiles(readback: &ComposedReadback, probes: &BTreeSet<u32>) -> Tiles {
    probes
        .iter()
        .map(|&probe| (probe, [0, 1, 2].map(|atlas| readback.l0_tile(atlas, probe))))
        .collect()
}

/// Halves where `found` differs from `expected`, over probes resident in
/// `found`, per atlas; and how many probes were resident.
fn mismatches(found: &Tiles, expected: &Tiles) -> ([usize; 3], usize) {
    let mut differ = [0; 3];
    let mut resident = 0;
    for (probe, tiles) in found {
        if tiles.iter().all(Option::is_none) {
            continue;
        }
        resident += 1;
        for atlas in 0..3 {
            if let (Some(found), Some(expected)) = (&tiles[atlas], &expected[probe][atlas]) {
                differ[atlas] += found.iter().zip(expected).filter(|(a, b)| a != b).count();
            } else {
                assert_eq!(tiles[atlas].is_some(), expected[probe][atlas].is_some());
            }
        }
    }
    (differ, resident)
}

/// Halves that differ between two complete tile sets, per atlas.
fn moving_halves(a: &Tiles, b: &Tiles) -> [usize; 3] {
    mismatches(a, b).0
}

/// GPU half on each streamed map named by `MOVER_C7_PRLS`
/// (`:`-separated compiled PRLs with id 50 and id 57), with every cluster
/// resident and every mover handed the same t′:
/// - Gate: t′ moves 0 → 1 on a frame whose gate admits no row. Every
///   channel tile still holds the t′ = 0 compose. The next frame admits
///   every row with t′ held at 1, and every channel tile holds the t′ = 1
///   compose that frame.
/// - Residency: every cluster is evicted, t′ moves 1 → 0 while none is
///   resident, and the clusters return with t′ held at 0. On each install
///   frame, every resident channel tile holds the t′ = 0 compose.
#[test]
#[ignore = "requires MOVER_C7_PRLS: `:`-separated compiled PRLs with id 50 and id 57 (mover-closet, \
            mover-setpiece built with --mover-payload-resident-bar 0)"]
fn c7_a_lagging_channel_row_composes_the_frame_it_is_first_sampled() {
    let paths = std::env::var("MOVER_C7_PRLS").expect("set MOVER_C7_PRLS");
    for path in paths.split(':').filter(|path| !path.is_empty()) {
        let Some(mut level) = RealLevel::load(path, ShStreamingMode::SyncProof) else {
            return;
        };
        let name = std::path::Path::new(path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(path)
            .to_owned();
        let probes: BTreeSet<u32> = level
            .direct_probes()
            .into_iter()
            .chain(level.bounce_probes().into_keys())
            .collect();
        let (open, closed) = (level.states(Some(1.0)), level.states(Some(0.0)));

        level.install_all(&closed, Gate::All);
        let at_0 = channel_tiles(&level.read(), &probes);
        level.frame(&open, Gate::All);
        let at_1 = channel_tiles(&level.read(), &probes);
        let moving = moving_halves(&at_1, &at_0);
        assert!(
            moving.iter().sum::<usize>() > 0,
            "{name}: the channel moves its tiles between t′ 0 and 1"
        );

        // Gate lag: t′ changes on a frame that composes no row.
        level.frame(&closed, Gate::All);
        let (held, _) = mismatches(&channel_tiles(&level.read(), &probes), &at_0);
        assert_eq!(held, [0; 3], "{name}: back at t′ 0");
        level.frame(&open, Gate::None);
        let (lagging, _) = mismatches(&channel_tiles(&level.read(), &probes), &at_0);
        assert_eq!(
            lagging, [0; 3],
            "{name}: outside the gate, no channel row composes the new t′"
        );
        level.frame(&open, Gate::All);
        let (admitted, resident) = mismatches(&channel_tiles(&level.read(), &probes), &at_1);
        eprintln!(
            "[C7] {name} gate: {} channel probes, {resident} resident; halves moving between t′ 0 and 1 {:?}; \
             off the t′ = 1 compose on the frame the rows re-enter the gate {admitted:?}",
            probes.len(),
            ATLAS_NAMES.iter().zip(moving).collect::<Vec<_>>()
        );
        assert_eq!(
            resident,
            probes.len(),
            "{name}: every channel probe resident"
        );
        assert_eq!(
            admitted, [0; 3],
            "{name}: rows re-entering the gate compose t′ 1 before that frame samples them"
        );

        // Residency lag: t′ changes while no channel row is resident.
        level.evict_all(&open, Gate::All);
        level.frame(&closed, Gate::All);
        let gone = channel_tiles(&level.read(), &probes);
        assert!(
            gone.values().all(|tiles| tiles.iter().all(Option::is_none)),
            "{name}: no channel probe is resident after eviction"
        );
        let mut install_frames = Vec::new();
        level.install_all_checking(&closed, Gate::All, |level| {
            install_frames.push(mismatches(&channel_tiles(&level.read(), &probes), &at_0));
        });
        eprintln!(
            "[C7] {name} residency: per install frame (halves off the t′ = 0 compose, resident channel probes): {install_frames:?}"
        );
        assert_eq!(
            install_frames.last().map(|(_, resident)| *resident),
            Some(probes.len()),
            "{name}: every channel probe returns"
        );
        for (frame, (differ, _)) in install_frames.iter().enumerate() {
            assert_eq!(
                *differ, [0; 3],
                "{name}: install frame {frame} samples a channel row not composed at the t′ that changed while it was evicted"
            );
        }
    }
}
