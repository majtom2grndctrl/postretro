// Probe depth tile bake units: the ray table, the capped ranking and the summary's pricing.
// See: context/lib/build_pipeline.md §PRL section IDs

use glam::DVec3;
use postretro_level_format::probe_depth_tiles::{
    MAX_PROBE_DEPTH_TILES, PROBE_DEPTH_TILE_TEXELS, ProbeDepthTilesSection,
    interior_texel_of_direction, probe_depth_tile_texture_bytes,
};
use rayon::prelude::*;

use super::*;
use crate::sh_bake::probe::probe_gather_directions;

/// A one-row lattice of `count` valid probes at 1 m spacing.
fn row_layout(count: u32) -> ProbeGridLayout {
    ProbeGridLayout {
        world_min: DVec3::ZERO,
        dims: [count, 1, 1],
        cell_size: [1.0; 3],
        far_sentinel: 4.0 * 3f32.sqrt(),
        probe_positions: (0..count)
            .map(|x| DVec3::new(f64::from(x), 0.0, 0.0))
            .collect(),
        validity: vec![1; count as usize],
    }
}

/// A candidate whose every texel carries its probe index, so a tile can be
/// traced back to the probe that offered it.
fn candidate(probe: u32, score: f32) -> Candidate {
    Candidate {
        score,
        probe,
        tile: [[probe as u16, 0]; PROBE_DEPTH_TILE_TEXELS],
    }
}

/// Offer `(probe, score)` pairs to a selector over `layout` keeping `cap`,
/// in the given order, and finish it.
fn select(layout: &ProbeGridLayout, cap: u32, offers: &[(u32, f32)]) -> ProbeDepthTiles {
    let selector = DepthTileSelector::new(layout, &probe_gather_directions(), cap);
    for &(probe, score) in offers {
        selector.offer(candidate(probe, score));
    }
    selector.finish()
}

// O9: candidates with equal changes straddle the cap. The cap keeps the
// highest change first and the lower probe index on a tie, writes them in
// ascending index order, and counts the rest as dropped, whatever order the
// probes offered them in.
#[test]
fn a_lowered_cap_keeps_the_highest_change_first_and_the_lower_index_on_a_tie() {
    let layout = row_layout(16);
    let offers = [
        (9, 0.4),
        (2, 0.9),
        (7, 0.4),
        (12, 0.2),
        (4, 0.4),
        (0, 0.15),
        (11, 0.9),
    ];
    // Two 0.9s, then the 0.4 tie keeps 4 over 7 and 9.
    let expected_indices = vec![2, 4, 11];
    let forward = select(&layout, 3, &offers);
    assert_eq!(forward.section.probe_indices, expected_indices);
    assert_eq!(forward.dropped, 4);
    for (tile, &probe) in forward.section.probe_indices.iter().enumerate() {
        assert!(
            forward
                .section
                .tile(tile)
                .iter()
                .all(|t| t[0] == probe as u16),
            "tile {tile} is probe {probe}'s"
        );
    }

    let mut reversed = offers;
    reversed.reverse();
    assert_eq!(select(&layout, 3, &reversed), forward);

    // Offered from many threads at once, the kept set is the same.
    for _ in 0..8 {
        let selector = DepthTileSelector::new(&layout, &probe_gather_directions(), 3);
        offers
            .par_iter()
            .for_each(|&(probe, score)| selector.offer(candidate(probe, score)));
        assert_eq!(selector.finish(), forward);
    }
}

#[test]
fn a_zero_cap_keeps_nothing_and_drops_every_candidate() {
    let tiles = select(&row_layout(4), 0, &[(1, 0.5), (3, 0.2)]);
    assert_eq!(tiles.section, ProbeDepthTilesSection::default());
    assert_eq!(tiles.dropped, 2);
}

#[test]
fn the_shipped_cap_is_the_15_bit_reference_limit() {
    assert_eq!(MAX_PROBE_DEPTH_TILES, 32_768);
}

// Retained candidate memory follows the cap, not the probe count: the kept
// heap is reserved once at min(cap, valid probes) and never grows.
#[test]
fn the_kept_heap_is_reserved_at_the_cap_not_the_probe_count() {
    let layout = row_layout(64);
    let selector = DepthTileSelector::new(&layout, &probe_gather_directions(), 3);
    let reserved = selector.reserved_bytes();
    assert!(reserved >= 3 * size_of::<Candidate>() as u64);
    assert!(reserved < 64 * size_of::<Candidate>() as u64);
    for probe in 0..64 {
        selector.offer(candidate(probe, 0.5 + probe as f32 / 128.0));
    }
    assert_eq!(selector.reserved_bytes(), reserved, "the heap never grows");
    let tiles = selector.finish();
    assert_eq!(tiles.section.probe_indices, vec![61, 62, 63]);
    assert_eq!(tiles.dropped, 61);
}

// Every interior texel reads at least one gather ray; one no ray lands in
// borrows the ray nearest its centre.
#[test]
fn every_tile_texel_reads_a_gather_ray_and_an_empty_one_borrows_the_nearest() {
    let directions = probe_gather_directions();
    let table = TileRayTable::new(&directions);
    for (texel, rays) in table.members.iter().enumerate() {
        assert!(!rays.is_empty(), "texel {texel}");
        assert!(
            rays.windows(2).all(|w| w[0] < w[1]),
            "texel {texel} sums in ray order"
        );
        let natural: Vec<u16> = rays
            .iter()
            .copied()
            .filter(|&ray| {
                interior_texel_of_direction(directions[usize::from(ray)].to_array()) == texel
            })
            .collect();
        if natural.is_empty() {
            let edge = PROBE_DEPTH_TILE_EDGE as usize;
            let centre = Vec3::from(irradiance_interior_texel_direction(
                (texel % edge) as u32,
                (texel / edge) as u32,
                PROBE_DEPTH_TILE_BORDERED_EDGE,
                PROBE_DEPTH_TILE_BORDER,
            ));
            let best = directions
                .iter()
                .map(|dir| centre.dot(*dir))
                .fold(f32::MIN, f32::max);
            assert_eq!(rays.len(), 1, "texel {texel} borrows one ray");
            assert_eq!(centre.dot(directions[usize::from(rays[0])]), best);
        } else {
            assert_eq!(natural, *rays, "texel {texel} reads only its own rays");
        }
    }

    // A probe whose every ray hits at one distance reads it in every texel.
    let tile = table.interior(&vec![2.5; directions.len()]);
    assert!(
        tile.iter()
            .all(|&texel| texel == [f32_to_f16_bits(2.5), f32_to_f16_bits(6.25)])
    );
}

// The Build Summary prices tiles from the function the renderer's allocation
// ledger prices them from.
#[test]
fn the_summary_reports_the_tile_texture_bytes_the_runtime_allocates() {
    for count in [0u32, 1, 5, 13_451] {
        let tiles = ProbeDepthTiles {
            section: ProbeDepthTilesSection {
                probe_indices: (0..count).collect(),
                texels: vec![[0, 0]; count as usize * PROBE_DEPTH_TILE_TEXELS],
            },
            dropped: 7,
        };
        let bytes = probe_depth_tile_texture_bytes(count);
        assert_eq!(tiles.texture_bytes(), bytes);
        let line = tiles.summary_line();
        assert!(
            line.starts_with(&format!("Probe depth tiles: {count} tiles, 7 dropped")),
            "{line}"
        );
        assert!(
            line.contains(&format!("tile texture {bytes} bytes")),
            "{line}"
        );
    }
}
