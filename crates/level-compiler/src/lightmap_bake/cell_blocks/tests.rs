use super::*;
use crate::lightmap_bake::pack_heuristics::PACK_HEURISTICS;

const EDGE: u32 = 2048;

/// Deterministic pseudo-random padded chart extents, odd and even, wide and
/// tall, in `[low, high]`.
fn scattered(count: u32, low: u32, high: u32, seed: u32) -> Vec<(u32, u32)> {
    let span = high - low + 1;
    (0..count)
        .map(|i| {
            let a = (i.wrapping_mul(2_654_435_761) ^ seed).rotate_left(7);
            let b = (i.wrapping_mul(40_503) ^ seed.rotate_left(13)).wrapping_mul(2_246_822_519);
            (low + a % span, low + (b >> 7) % span)
        })
        .collect()
}

/// Strips: long thin charts, a cell that packs better turned.
fn strips() -> Vec<(u32, u32)> {
    let mut charts = vec![(61, 7); 6];
    charts.extend(vec![(9, 45); 5]);
    charts.extend([(33, 21), (5, 5), (17, 3)]);
    charts
}

fn placed(chart: (u32, u32), slot: &BlockSlot) -> (u32, u32) {
    if slot.transposed {
        (chart.1, chart.0)
    } else {
        chart
    }
}

/// Every chart inside its block, no two overlapping, every origin on the
/// chart grid, every block extent aligned.
fn assert_sound(charts: &[(u32, u32)], blocks: &[CellSubBlock], rules: PackRules) {
    let mut seen = vec![false; charts.len()];
    for sub in blocks {
        let block = &sub.block;
        assert_eq!(block.width % rules.align, 0);
        assert_eq!(block.height % rules.align, 0);
        let mut grid = vec![false; (block.width * block.height) as usize];
        for (&member, slot) in sub.members.iter().zip(&block.placements) {
            assert!(
                !std::mem::replace(&mut seen[member], true),
                "chart {member} twice"
            );
            assert_eq!(slot.x % rules.chart_align, 0, "chart {member} x origin");
            assert_eq!(slot.y % rules.chart_align, 0, "chart {member} y origin");
            let (w, h) = placed(charts[member], slot);
            assert!(slot.x + w <= block.width && slot.y + h <= block.height);
            for y in slot.y..slot.y + h {
                for x in slot.x..slot.x + w {
                    let texel = &mut grid[(y * block.width + x) as usize];
                    assert!(!std::mem::replace(texel, true), "overlap at {x},{y}");
                }
            }
        }
    }
    assert!(seen.iter().all(|&s| s), "every chart placed");
}

fn total_area(blocks: &[CellSubBlock]) -> u64 {
    blocks.iter().map(|sub| sub.block.area()).sum()
}

#[test]
fn lightmap_charts_turn_and_every_origin_sits_on_the_direction_grid() {
    let rules = PackRules::lightmap(4, 2);
    for charts in [strips(), scattered(40, 3, 70, 11), scattered(25, 1, 9, 3)] {
        let pack = pack_cell_sub_blocks(&charts, rules, EDGE, &PACK_HEURISTICS);
        assert_sound(&charts, &pack.blocks, rules);
    }
    let pack = pack_cell_sub_blocks(&strips(), rules, EDGE, &PACK_HEURISTICS);
    assert!(
        pack.blocks
            .iter()
            .flat_map(|sub| &sub.block.placements)
            .any(|slot| slot.transposed),
        "a strip cell turns some charts"
    );
}

#[test]
fn a_square_footprint_never_turns() {
    let rules = PackRules::lightmap(4, 2);
    // 5×6 rounds to 6×6.
    let charts = [(5, 6), (6, 5), (8, 8), (3, 4)];
    let pack = pack_cell_sub_blocks(&charts, rules, EDGE, &PACK_HEURISTICS);
    assert!(
        pack.blocks
            .iter()
            .flat_map(|sub| &sub.block.placements)
            .all(|slot| !slot.transposed)
    );
}

#[test]
fn the_kept_packing_is_no_larger_than_any_single_heuristic() {
    let rules = PackRules::lightmap(4, 2);
    for (index, charts) in [
        strips(),
        scattered(60, 3, 90, 7),
        scattered(12, 20, 200, 23),
        scattered(200, 2, 30, 5),
    ]
    .into_iter()
    .enumerate()
    {
        let kept = pack_cell_sub_blocks(&charts, rules, EDGE, &PACK_HEURISTICS);
        let kept_area = total_area(&kept.blocks);
        let mut areas = Vec::new();
        for heuristic in PACK_HEURISTICS {
            let single = pack_cell_sub_blocks(&charts, rules, EDGE, &[heuristic]);
            assert_sound(&charts, &single.blocks, rules);
            areas.push(total_area(&single.blocks));
        }
        assert!(
            areas.iter().all(|&area| kept_area <= area),
            "cell {index}: kept {kept_area} vs {areas:?}"
        );
        assert_eq!(areas[kept.heuristic], kept_area, "cell {index}");
        assert_eq!(
            kept.blocks,
            pack_cell_sub_blocks(&charts, rules, EDGE, &[PACK_HEURISTICS[kept.heuristic]]).blocks,
            "cell {index}: the kept packing is its heuristic's"
        );
    }
}

#[test]
fn multi_block_cells_keep_the_smallest_heuristic_too() {
    let rules = PackRules::lightmap(4, 2);
    let charts = scattered(90, 9, 120, 17);
    let edge = 256;
    let kept = pack_cell_sub_blocks(&charts, rules, edge, &PACK_HEURISTICS);
    assert!(kept.blocks.len() >= 3, "the cell splits");
    assert_sound(&charts, &kept.blocks, rules);
    for heuristic in PACK_HEURISTICS {
        let single = pack_cell_sub_blocks(&charts, rules, edge, &[heuristic]);
        assert!(total_area(&kept.blocks) <= total_area(&single.blocks));
        assert!(single.blocks.iter().all(|sub| sub.block.fits(edge)));
    }
}

/// The dry run recovers placed rects from a PRL and repacks them; a turned
/// chart handed in turned must land on the same rect.
#[test]
fn a_chart_handed_in_turned_packs_to_the_same_rects() {
    let rules = PackRules::lightmap(4, 2);
    let charts = strips();
    let pack = pack_cell_sub_blocks(&charts, rules, EDGE, &PACK_HEURISTICS);
    let as_placed: Vec<(u32, u32)> = pack
        .blocks
        .iter()
        .flat_map(|sub| sub.members.iter().zip(&sub.block.placements))
        .map(|(&member, slot)| (member, placed(charts[member], slot)))
        .fold(vec![(0, 0); charts.len()], |mut out, (member, extent)| {
            out[member] = extent;
            out
        });
    let repacked = pack_cell_sub_blocks(&as_placed, rules, EDGE, &PACK_HEURISTICS);
    assert_eq!(repacked.blocks.len(), pack.blocks.len());
    for (a, b) in pack.blocks.iter().zip(&repacked.blocks) {
        assert_eq!(
            (a.block.width, a.block.height),
            (b.block.width, b.block.height)
        );
        assert_eq!(a.members, b.members);
        for (sa, sb) in a.block.placements.iter().zip(&b.block.placements) {
            assert_eq!((sa.x, sa.y), (sb.x, sb.y));
            assert!(!sb.transposed, "the placed rect is already upright");
        }
    }
}

#[test]
fn mask_rules_pack_upright_unrounded_and_untrimmed_as_before() {
    let rules = PackRules::mask(4);
    let charts = strips();
    let block = pack_cell_block_within(&charts, rules, EDGE, PackHeuristic::DEFAULT).unwrap();
    assert!(block.placements.iter().all(|slot| !slot.transposed));
    // Odd extents pack edge to edge: some origin is odd.
    assert!(
        block
            .placements
            .iter()
            .any(|slot| slot.x % 2 == 1 || slot.y % 2 == 1)
    );
}

#[test]
fn heuristic_names_round_trip() {
    for heuristic in PACK_HEURISTICS {
        assert_eq!(PackHeuristic::parse(heuristic.name()), Some(heuristic));
    }
    assert_eq!(PackHeuristic::names().len(), PACK_HEURISTICS.len());
    assert_eq!(PackHeuristic::parse("best"), None);
    assert_eq!(PACK_HEURISTICS[0], PackHeuristic::DEFAULT);
}
