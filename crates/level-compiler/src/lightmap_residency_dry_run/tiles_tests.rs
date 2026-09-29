use super::dry_run_test_fixtures::{bc6h_formats, chart, input};
use super::run_dry_run;
use super::tiles::{
    OversizeCharts, TILE_SIZES, TileUnit, UnitStamp, UnitTiles, pack_unit_tiles, tile_layout,
};
use super::tiles_render::cluster_units_agree_across_granularity;
use super::visible_set_tests::u_turn_input;

#[test]
fn tile_packing_puts_charts_that_exactly_fill_a_tile_in_one_tile() {
    let quarters: Vec<_> = [(0, 0), (64, 0), (0, 64), (64, 64)]
        .map(|(x, y)| chart(0, 0, x, y, 64, 64))
        .to_vec();
    assert_eq!(pack_unit_tiles(&quarters, 128).tiles, 1);
    assert_eq!(
        pack_unit_tiles(&[chart(0, 0, 0, 0, 128, 128)], 128).tiles,
        1
    );
    // One texel more than a tile holds opens a second one.
    let mut spill = quarters.clone();
    spill.push(chart(0, 0, 0, 0, 1, 1));
    assert_eq!(pack_unit_tiles(&spill, 128).tiles, 2);
}

#[test]
fn tile_packing_gives_an_oversize_chart_ceil_tiles_of_its_padded_extent() {
    let charts = [chart(0, 0, 0, 0, 300, 130), chart(0, 0, 0, 0, 16, 16)];
    let packed = pack_unit_tiles(&charts, 128);
    assert_eq!(
        packed,
        UnitTiles {
            // 3 × 2 dedicated tiles, plus one shared tile for the small chart.
            tiles: 7,
            oversize: OversizeCharts {
                charts: 1,
                chart_texels: 300 * 130,
                tiles: 6,
                interior_fits: 0,
            },
        }
    );
    // At P = 512 the same chart is ordinary and shares a tile.
    assert_eq!(pack_unit_tiles(&charts, 512).tiles, 1);
    // A 128² interior padded to 132² takes a 2 × 2 block at P = 128.
    let padded = pack_unit_tiles(&[chart(0, 0, 0, 0, 132, 132)], 128);
    assert_eq!((padded.tiles, padded.oversize.interior_fits), (4, 1));
}

#[test]
fn tile_layout_costs_a_chartless_unit_zero() {
    assert_eq!(pack_unit_tiles(&[], 128), UnitTiles::default());
    let fixture = input(
        bc6h_formats(64, 1, true),
        &[0, 1],
        vec![chart(0, 0, 0, 0, 20, 20)],
        Vec::new(),
    );
    let mut stamp = UnitStamp::default();
    for unit in TileUnit::ALL {
        let layout = tile_layout(&fixture, unit, 128);
        assert_eq!(layout.unit_tiles, vec![1, 0], "{unit:?}");
        assert_eq!(layout.mandatory_bytes(&[1], &mut stamp), 0, "{unit:?}");
        assert_eq!(layout.mandatory_bytes(&[], &mut stamp), 0, "{unit:?}");
    }
}

#[test]
fn tile_bytes_charge_irradiance_direction_and_the_doubled_shadowmask() {
    let fixture = input(bc6h_formats(64, 1, true), &[0], Vec::new(), Vec::new());
    for p in TILE_SIZES {
        let layout = tile_layout(&fixture, TileUnit::Cell, p);
        let texels = u64::from(p) * u64::from(p);
        // BC6H 1 B/texel + half-res Rg8 direction 0.5 B/texel + two BC5 mask
        // groups side by side at 1 B/texel each.
        assert_eq!(layout.tile_bytes, texels + texels / 2 + 2 * texels, "P={p}");
    }
    let no_mask = input(bc6h_formats(64, 1, false), &[0], Vec::new(), Vec::new());
    assert_eq!(
        tile_layout(&no_mask, TileUnit::Cell, 128).tile_bytes,
        128 * 128 * 3 / 2
    );
}

#[test]
fn neither_tile_unit_dominates_for_the_same_mandatory_set() {
    // Cluster 0 holds a small-chart cell 0 and a tile-filling cell 1.
    let fixture = input(
        bc6h_formats(256, 1, true),
        &[0, 0],
        vec![chart(0, 0, 0, 0, 16, 16), chart(1, 0, 0, 0, 128, 120)],
        Vec::new(),
    );
    let cell = tile_layout(&fixture, TileUnit::Cell, 128);
    let cluster = tile_layout(&fixture, TileUnit::Cluster, 128);
    assert_eq!(cell.unit_tiles, vec![1, 1]);
    assert_eq!(
        cluster.unit_tiles,
        vec![2],
        "16² cannot join a 128x120 tile"
    );
    let tile = cell.tile_bytes;
    let mut stamp = UnitStamp::default();
    // Only cell 0 mandatory: the cluster drags in cell 1's tile.
    assert_eq!(cell.mandatory_bytes(&[0], &mut stamp), tile);
    assert_eq!(cluster.mandatory_bytes(&[0], &mut stamp), 2 * tile);

    // Two small-chart cells of one cluster: per-cell tiles cost double.
    let small = input(
        bc6h_formats(256, 1, true),
        &[0, 0],
        vec![chart(0, 0, 0, 0, 16, 16), chart(1, 0, 16, 0, 16, 16)],
        Vec::new(),
    );
    let cell = tile_layout(&small, TileUnit::Cell, 128);
    let cluster = tile_layout(&small, TileUnit::Cluster, 128);
    // Duplicate cells and shared units count once.
    assert_eq!(cell.mandatory_bytes(&[0, 1, 1], &mut stamp), 2 * tile);
    assert_eq!(cluster.mandatory_bytes(&[0, 1, 1], &mut stamp), tile);
}

#[test]
fn tiled_visible_set_bytes_never_undercut_texel_exact_and_clusters_agree() {
    let report = run_dry_run(&u_turn_input());
    let visible = report
        .visible_set
        .as_ref()
        .expect("world and graph present");
    assert_eq!(
        report.tile_layouts.len(),
        TileUnit::ALL.len() * TILE_SIZES.len()
    );
    for result in visible.dense.iter().chain(&visible.sparse_lead_zero) {
        for lead in &result.leads {
            assert_eq!(lead.tile_bytes.len(), lead.cells.len());
            for ((cell, bytes), tiles) in lead.cells.iter().zip(&lead.tile_bytes) {
                for (layout, &tile_bytes) in report.tile_layouts.iter().zip(tiles) {
                    assert!(
                        tile_bytes as f64 >= bytes.texel_exact,
                        "{:?} P={} cell {cell}",
                        layout.unit,
                        layout.tile_size
                    );
                }
            }
        }
    }
    assert!(cluster_units_agree_across_granularity(
        visible,
        &report.tile_layouts
    ));
    // Every U-turn chart fits one 128² tile, and cluster 2's second cell is
    // solid and chartless: three tiles under either unit.
    for layout in &report.tile_layouts {
        assert_eq!(layout.virtual_tiles(), 3);
        assert_eq!(layout.oversize, OversizeCharts::default());
    }
    let rendered = report.render();
    assert!(rendered.contains("tiled residency"));
    assert!(rendered.contains("cluster-unit bytes identical under both M(c): yes"));
    assert!(rendered.contains("bytes cover id 22 + id 42 only"));
}
