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

// Candidates with equal changes straddle the cap. The cap keeps the highest
// change first and the lower probe index on a tie, writes them in ascending
// index order, and counts the rest as dropped, whatever order the probes
// offered them in.
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
// ledger prices them from, and names the cap the bake kept.
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
        assert_eq!(tiles.texture_bytes(0), bytes);
        let line = tiles.summary_line(MAX_PROBE_DEPTH_TILES, 0);
        assert!(
            line.starts_with(&format!(
                "Probe depth tiles: {count} tiles, 7 dropped past the 32768-tile cap"
            )),
            "{line}"
        );
        assert!(
            line.contains(&format!("tile texture {bytes} bytes")),
            "{line}"
        );
        // Mover live tiles share the texture: the summary prices both.
        let with_live = probe_depth_tile_texture_bytes(count + 3);
        assert_eq!(tiles.texture_bytes(3), with_live);
        let line = tiles.summary_line(MAX_PROBE_DEPTH_TILES, 3);
        assert!(
            line.contains(&format!(
                "tile texture {with_live} bytes ({:.2} MiB) for {} tile(s) with 3 live",
                with_live as f64 / (1024.0 * 1024.0),
                count + 3
            )),
            "{line}"
        );
    }
}

// A lowered cap reaches the summary as the bake kept it; one past the
// reference's range clamps there for the selector, the summary and the cache
// key alike.
#[test]
fn the_summary_and_cache_key_carry_the_clamped_cap() {
    let tiles = ProbeDepthTiles::default();
    assert!(
        tiles.summary_line(3, 0).contains("past the 3-tile cap"),
        "{}",
        tiles.summary_line(3, 0)
    );
    assert!(
        tiles
            .summary_line(u32::MAX, 0)
            .contains("past the 32768-tile cap"),
        "{}",
        tiles.summary_line(u32::MAX, 0)
    );
    let key = |cap| {
        let mut hasher = blake3::Hasher::new();
        fold_depth_tile_constants(&mut hasher, cap);
        hasher.finalize()
    };
    assert_eq!(key(u32::MAX), key(MAX_PROBE_DEPTH_TILES));
    assert_ne!(key(3), key(MAX_PROBE_DEPTH_TILES));

    let layout = row_layout(4);
    let offers: Vec<(u32, f32)> = (0..4).map(|probe| (probe, 0.5)).collect();
    assert_eq!(
        select(&layout, u32::MAX, &offers),
        select(&layout, MAX_PROBE_DEPTH_TILES, &offers)
    );
}

// Gather hits are unbounded: only a sky miss reads the far sentinel. A texel
// facing far geometry saturates its moments at the largest finite f16 rather
// than encoding +inf, so a bilinear sample mixing it with a near texel stays
// finite and the visibility floor holds.
#[test]
fn far_hits_saturate_tile_moments_and_a_mixed_sample_stays_floored() {
    let directions = probe_gather_directions();
    let table = TileRayTable::new(&directions);
    let rays = directions.len();
    let saturated = f32_to_f16_bits(F16_MAX);
    assert_eq!(f16_bits_to_f32(saturated), 65504.0);

    // At 300 m, E[d] fits an f16 but E[d²] = 90,000 m² does not.
    let far = table.interior(&vec![300.0; rays]);
    assert!(
        far.iter()
            .all(|&texel| texel == [f32_to_f16_bits(300.0), saturated])
    );
    // Past f16's range, E[d] saturates too.
    let farther = table.interior(&vec![70_000.0; rays]);
    assert!(farther.iter().all(|&texel| texel == [saturated, saturated]));
    let near = table.interior(&vec![1.0; rays]);

    let edge = PROBE_DEPTH_TILE_EDGE as usize;
    let centre = |x: u32, y: u32| {
        Vec3::from(irradiance_interior_texel_direction(
            x,
            y,
            PROBE_DEPTH_TILE_BORDERED_EDGE,
            PROBE_DEPTH_TILE_BORDER,
        ))
    };
    let bias = probe_depth_bias([1.0; 3]);
    for far in [far, farther] {
        // Far texels in the left four columns, near ones in the right four.
        let interior: Vec<[u16; 2]> = (0..PROBE_DEPTH_TILE_INTERIOR_TEXELS)
            .map(|i| if i % edge < edge / 2 { far[i] } else { near[i] })
            .collect();
        let tile = bordered_tile(&interior);

        // Texel centres, where a tap weighs its neighbour by zero, and the
        // seam between columns 3 and 4, where far and near taps mix.
        let mut dirs = Vec::new();
        for y in 0..PROBE_DEPTH_TILE_EDGE {
            for x in 0..PROBE_DEPTH_TILE_EDGE {
                dirs.push(centre(x, y));
            }
            dirs.push(centre(3, y) + centre(4, y));
        }
        for dir in dirs {
            let moments = sample_tile_moments(&tile, dir.to_array());
            assert!(moments.iter().all(|m| m.is_finite()), "{dir}: {moments:?}");
            for distance in [0.5, 2.0, 400.0, 100_000.0] {
                let visibility = chebyshev_visibility(moments, distance, bias);
                assert!(
                    (SH_DEPTH_MIN_VISIBILITY..=1.0).contains(&visibility),
                    "{dir} at {distance} m: {visibility}"
                );
            }
        }

        let [mixed_mean, _] = sample_tile_moments(&tile, (centre(3, 3) + centre(4, 3)).to_array());
        let far_mean = f16_bits_to_f32(far[0][0]);
        assert!(
            mixed_mean > 1.0 && mixed_mean < far_mean,
            "the seam mixes near and far: {mixed_mean}"
        );
    }
}

const SH_SAMPLE_WGSL: &str = include_str!("../../../renderer/src/shaders/sh_sample.wgsl");

/// The value of `sh_sample.wgsl`'s `const {name}: f32 = …;`.
fn wgsl_f32_constant(name: &str) -> f32 {
    let prefix = format!("const {name}: f32 =");
    let line = SH_SAMPLE_WGSL
        .lines()
        .find(|line| line.trim_start().starts_with(&prefix))
        .unwrap_or_else(|| panic!("sh_sample.wgsl declares {name}"));
    line.split('=')
        .nth(1)
        .and_then(|value| value.trim().trim_end_matches(';').parse().ok())
        .unwrap_or_else(|| panic!("{name} is an f32 literal: {line}"))
}

// The bake ranks and the reflection capture weighs with the runtime's
// Chebyshev weight; its constants come from the shader.
#[test]
fn chebyshev_constants_match_sh_sample_wgsl() {
    assert_eq!(
        SH_DEPTH_MIN_VARIANCE_M2,
        wgsl_f32_constant("SH_DEPTH_MIN_VARIANCE_M2")
    );
    assert_eq!(
        SH_DEPTH_BIAS_CELL_FRACTION,
        wgsl_f32_constant("SH_DEPTH_BIAS_CELL_FRACTION")
    );
    assert_eq!(
        SH_DEPTH_MIN_VISIBILITY,
        wgsl_f32_constant("SH_DEPTH_MIN_VISIBILITY")
    );
}
