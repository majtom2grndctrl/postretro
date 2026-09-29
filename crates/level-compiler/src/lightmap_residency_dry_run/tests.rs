use std::collections::BTreeSet;

use postretro_level_format::bvh::{BvhLeaf, BvhSection};
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};

use super::attribution::attribute;
use super::dry_run_test_fixtures::{
    METER, bc6h_formats, chart, corridor_input, input, pair, raw_formats,
};
use super::inputs::reconstruct_charts;
use super::layouts::{cluster_ordered_layout, stored_repack_matches};
use super::mandatory::{DistanceBound, Granularity, MandatoryContext, Neighbors};
use super::portal_distance::{
    HubCell, HubPortal, PortalGraphInput, recompute_pairs, validate_against_stored,
};
use super::{ChartRect, FaceSlot, ShadowmaskState, run_dry_run};
use crate::chart_raster::CHART_PADDING_TEXELS;
use crate::lightmap_bake::pack_cell_block;

#[test]
fn attribution_accounts_for_every_payload_byte_across_formats() {
    let bc = corridor_input();
    let result = attribute(&bc.formats, &bc.charts, bc.cell_count());
    let shadowmask = result.shadowmask.as_ref().expect("fixture carries id 42");
    for section in [&result.irradiance, &result.direction, shadowmask] {
        assert_eq!(section.attributed() + section.unattributed, section.payload);
        assert!(section.unattributed > 0, "fixture leaves empty atlas space");
    }
    assert_eq!(result.overlap_texels, 0);
    // Every charted cell owns bytes in every section.
    for cell in 0..bc.cell_count() {
        assert!(result.irradiance.per_cell[cell] > 0, "cell {cell}");
        assert!(result.direction.per_cell[cell] > 0, "cell {cell}");
        assert!(shadowmask.per_cell[cell] > 0, "cell {cell}");
    }

    let raw = input(
        raw_formats(64, 1),
        &[0, 0],
        vec![chart(0, 0, 1, 1, 10, 7), chart(1, 0, 11, 1, 5, 5)],
        Vec::new(),
    );
    let result = attribute(&raw.formats, &raw.charts, raw.cell_count());
    assert!(result.shadowmask.is_none());
    for section in [&result.irradiance, &result.direction] {
        assert_eq!(section.attributed() + section.unattributed, section.payload);
    }
    // Raw formats attribute per texel: exactly the chart area.
    assert_eq!(result.irradiance.per_cell[0], 10 * 7 * 8);
    assert_eq!(result.direction.per_cell[1], 5 * 5 * 4);
}

#[test]
fn attribution_gives_a_straddled_block_to_its_majority_cell() {
    // Block (0,0) holds 12 texels of cell 1 (x 0..3) and 4 of cell 0 (x 3..4).
    let straddle = input(
        bc6h_formats(64, 1, true),
        &[0, 0],
        vec![chart(1, 0, 0, 0, 3, 4), chart(0, 0, 3, 0, 5, 4)],
        Vec::new(),
    );
    let result = attribute(&straddle.formats, &straddle.charts, straddle.cell_count());
    // Cell 1 wins block 0; cell 0 owns block 1 (x 4..8) alone.
    assert_eq!(result.irradiance.per_cell, vec![16, 16]);
    assert_eq!(result.shadowmask.unwrap().per_cell, vec![32, 32]);
}

#[test]
fn mandatory_set_includes_own_cluster_without_coupled_pairs() {
    let mut fixture = input(
        bc6h_formats(64, 1, false),
        &[0, 0, 1],
        vec![chart(0, 0, 0, 0, 8, 8), chart(2, 0, 8, 0, 8, 8)],
        Vec::new(),
    );
    fixture.component_ids = vec![0, 1, 2];
    let neighbors = Neighbors::from_pairs(fixture.cell_count(), &fixture.coupled_pairs);
    let mut context = MandatoryContext::new(&fixture);
    for granularity in Granularity::ALL {
        for bound in DistanceBound::ALL {
            let mut set = |cell| context.cell_set(cell, bound, &neighbors, granularity);
            assert_eq!(set(0), vec![0, 1], "{bound:?} {granularity:?}");
            assert_eq!(set(1), vec![0, 1], "{bound:?} {granularity:?}");
            assert_eq!(set(2), vec![2], "{bound:?} {granularity:?}");
        }
    }
}

#[test]
fn zero_chart_input_reports_zero_bytes_without_panic() {
    let empty = input(
        bc6h_formats(64, 1, true),
        &[0, 1, 1],
        Vec::new(),
        Vec::new(),
    );
    let report = run_dry_run(&empty);
    // Every non-solid, non-exterior cell is a camera cell, charted or not.
    assert_eq!(report.camera_cells, vec![0, 1, 2]);
    for source in &report.sources {
        for bound in &source.bounds {
            for (_, bytes) in &bound.cells {
                assert_eq!(bytes.texel_exact, 0.0);
                assert!(bytes.layer.iter().all(|&layer| layer == 0));
            }
        }
    }
    assert_eq!(report.attribution.irradiance.attributed(), 0);
    assert_eq!(
        report.attribution.irradiance.unattributed,
        report.attribution.irradiance.payload
    );
    assert!(
        report
            .layouts
            .iter()
            .skip(1)
            .all(|l| l.regular_layer_count == 0)
    );
    assert!(report.render().contains("3 camera cells"));

    let no_cells = input(bc6h_formats(64, 1, false), &[], Vec::new(), Vec::new());
    let report = run_dry_run(&no_cells);
    assert!(report.camera_cells.is_empty());
    let _ = report.render();

    // A chartless camera cell carries its cluster-mate's bytes.
    let one = input(
        bc6h_formats(64, 1, false),
        &[0, 0],
        vec![chart(0, 0, 0, 0, 4, 4)],
        Vec::new(),
    );
    let report = run_dry_run(&one);
    let (cell, bytes) = &report.sources[0].bounds[0].cells[1];
    assert_eq!(*cell, 1);
    assert!(bytes.texel_exact > 0.0, "cell 1 shares cell 0's cluster");
}

#[test]
fn distance_bound_includes_exact_distance_and_excludes_one_unit_beyond() {
    let fixture = input(
        bc6h_formats(64, 1, false),
        &[0, 1, 2],
        vec![
            chart(0, 0, 0, 0, 8, 8),
            chart(1, 0, 8, 0, 8, 8),
            chart(2, 0, 16, 0, 8, 8),
        ],
        vec![pair(0, 1, 16 * METER), pair(0, 2, 16 * METER + 1)],
    );
    let neighbors = Neighbors::from_pairs(fixture.cell_count(), &fixture.coupled_pairs);
    let mut context = MandatoryContext::new(&fixture);
    let mut set = |cell, meters| {
        context.cell_set(
            cell,
            DistanceBound::Meters(meters),
            &neighbors,
            Granularity::Cell,
        )
    };
    assert_eq!(set(0, 16), vec![0, 1]);
    assert_eq!(set(0, 32), vec![0, 1, 2]);
    // Coupling is symmetric: the far cell sees the camera cell back.
    assert_eq!(set(2, 16), vec![2]);
}

#[test]
fn cluster_closure_admits_whole_clusters_of_cells_within_the_bound_only() {
    // Cell 1 at exactly 16 m shares cluster 1 with cell 3, which no pair
    // reaches; cell 2 at 16 m + 1 unit owns cluster 2 with cell 4.
    let fixture = input(
        bc6h_formats(64, 1, false),
        &[0, 1, 2, 1, 2],
        vec![
            chart(0, 0, 0, 0, 8, 8),
            chart(1, 0, 8, 0, 8, 8),
            chart(2, 0, 16, 0, 8, 8),
            chart(3, 0, 24, 0, 8, 8),
            chart(4, 0, 32, 0, 8, 8),
        ],
        vec![pair(0, 1, 16 * METER), pair(0, 2, 16 * METER + 1)],
    );
    let neighbors = Neighbors::from_pairs(fixture.cell_count(), &fixture.coupled_pairs);
    let mut context = MandatoryContext::new(&fixture);
    let mut set = |meters, granularity| {
        context.cell_set(0, DistanceBound::Meters(meters), &neighbors, granularity)
    };
    assert_eq!(set(16, Granularity::Cell), vec![0, 1]);
    assert_eq!(set(16, Granularity::ClusterClosure), vec![0, 1, 3]);
    assert_eq!(set(32, Granularity::ClusterClosure), vec![0, 1, 2, 3, 4]);
}

#[test]
fn larger_distance_bound_never_yields_fewer_bytes() {
    let report = run_dry_run(&corridor_input());
    for source in &report.sources {
        for window in source.bounds.windows(2) {
            for ((cell, near), (_, far)) in window[0].cells.iter().zip(&window[1].cells) {
                assert!(far.texel_exact >= near.texel_exact, "cell {cell}");
                assert!(far.half_res >= near.half_res, "cell {cell}");
                for (far_layer, near_layer) in far.layer.iter().zip(&near.layer) {
                    assert!(far_layer >= near_layer, "cell {cell}");
                }
            }
        }
    }
    // The chain really grows the set between bounds.
    let sets: Vec<usize> = report.sources[0]
        .bounds
        .iter()
        .map(|b| b.cells[0].1.cell_count)
        .collect();
    assert!(sets.first() < sets.last(), "{sets:?}");
}

#[test]
fn pinned_clusters_join_every_set_and_unpinned_unreachable_clusters_do_not() {
    let mut fixture = corridor_input();
    fixture.pinned_clusters = vec![2];
    // Cluster 3 (cells 6, 7) sits in its own reachability component.
    fixture.coupled_pairs.retain(|p| p.cell_b != 6);
    fixture.component_ids = vec![0, 0, 0, 0, 0, 0, 1, 1];
    let neighbors = Neighbors::from_pairs(fixture.cell_count(), &fixture.coupled_pairs);
    let mut context = MandatoryContext::new(&fixture);
    for granularity in Granularity::ALL {
        for camera in 0..fixture.cell_count() as u32 {
            for bound in DistanceBound::ALL {
                let set = context.cell_set(camera, bound, &neighbors, granularity);
                let case = format!("{camera} {bound:?} {granularity:?}");
                assert!(set.contains(&4) && set.contains(&5), "{case}");
                if camera < 6 {
                    assert!(!set.contains(&6) && !set.contains(&7), "{case}");
                }
            }
        }
    }
}

#[test]
fn cluster_ordered_packing_keeps_each_cell_on_one_capped_layer() {
    let mut charts: Vec<ChartRect> = (0..24u32)
        .flat_map(|cell| {
            let side = 10 + (cell * 7) % 21;
            [
                chart(cell, 0, 0, 0, side, side),
                chart(cell, 0, 0, 0, side / 2 + 3, side),
            ]
        })
        .collect();
    // Cell 24: area over 64² in 40² charts. Cell 25: one chart wider than 64.
    charts.extend([
        chart(24, 0, 0, 0, 40, 40),
        chart(24, 0, 0, 0, 40, 40),
        chart(24, 0, 0, 0, 40, 40),
        chart(25, 0, 0, 0, 70, 12),
    ]);
    let clusters: Vec<u32> = (0..26).map(|cell| (cell * 5) % 7).collect();
    let fixture = input(bc6h_formats(64, 1, true), &clusters, charts, Vec::new());
    let layout = cluster_ordered_layout(&fixture, 64);

    let oversize: Vec<u32> = layout.oversize_cells.iter().map(|o| o.cell).collect();
    assert_eq!(oversize, vec![24, 25]);
    assert!(layout.regular_layer_count > 1);
    for (cell, layers) in layout.cell_layers.iter().enumerate() {
        assert_eq!(layers.len(), 1, "cell {cell} spans {layers:?}");
    }
    for oversize in &layout.oversize_cells {
        let layer = layout.cell_layers[oversize.cell as usize][0];
        assert!(
            layer >= layout.regular_layer_count,
            "oversize cell stays out of the array"
        );
        assert!(oversize.layer_dim > 64);
    }
    // Every placement lies inside its layer and no two charts overlap.
    let mut occupied = BTreeSet::new();
    for (rect, placement) in fixture.charts.iter().zip(&layout.placements) {
        let (width, height) = layout.layer_dims[placement.layer as usize];
        if placement.layer < layout.regular_layer_count {
            assert!(width <= 64 && height <= 64);
        }
        assert!(placement.x + rect.width <= width && placement.y + rect.height <= height);
        for y in placement.y..placement.y + rect.height {
            for x in placement.x..placement.x + rect.width {
                assert!(
                    occupied.insert((placement.layer, x, y)),
                    "overlap at {x},{y}"
                );
            }
        }
    }
}

#[test]
fn layer_granularity_bytes_never_undercut_texel_exact_bytes() {
    let report = run_dry_run(&corridor_input());
    let mut checked = 0;
    for source in &report.sources {
        for bound in &source.bounds {
            for (cell, bytes) in &bound.cells {
                for &layer in &bytes.layer {
                    assert!(layer as f64 >= bytes.texel_exact, "cell {cell}");
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(
        checked,
        8 * DistanceBound::ALL.len() * report.layouts.len() * Granularity::ALL.len()
    );
}

#[test]
fn identical_input_renders_identical_report() {
    let first = run_dry_run(&corridor_input());
    let second = run_dry_run(&corridor_input());
    assert_eq!(first, second);
    assert_eq!(first.render(), second.render());
    assert_eq!(first.csv(), second.csv());
}

/// Encode one face exactly as the bake's UV assignment does: extreme vertices
/// on the padded block-local placement's interior edges, naming block slot
/// `block_slot` (block id + 1; 0 is none) over a square `extent`-texel block.
fn face_vertices(x: u32, y: u32, w: u32, h: u32, block_slot: u16, extent: f32) -> Vec<Vertex> {
    let pad = CHART_PADDING_TEXELS as f32;
    let (x0, y0) = (x as f32 + pad, y as f32 + pad);
    let (x1, y1) = ((x + w) as f32 - pad, (y + h) as f32 - pad);
    [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
        .into_iter()
        .map(|(u, v)| {
            Vertex::new(
                [0.0; 3],
                [0.0; 2],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [u / extent, v / extent],
                block_slot,
            )
        })
        .collect()
}

fn face_leaf(index_offset: u32, index_count: u32, cell_id: u32) -> BvhLeaf {
    BvhLeaf {
        aabb_min: [0.0; 3],
        material_bucket_id: 0,
        aabb_max: [0.0; 3],
        index_offset,
        index_count,
        cell_id,
        chunk_range_start: 0,
        chunk_range_count: 0,
    }
}

#[test]
fn chart_reconstruction_recovers_padded_placements_from_vertex_uvs() {
    let mut formats = bc6h_formats(2048, 3, true);
    // Block `b` holds the chart whose `layer` is `b`.
    for (block, cell) in formats.blocks.iter_mut().zip([3, 1, 0]) {
        block.cell = cell;
    }
    let expected = [
        chart(0, 2, 17, 905, 133, 9),
        chart(3, 0, 2040 - 45, 7, 45, 61),
        chart(1, 1, 0, 0, 5, 5),
    ];
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut leaves = Vec::new();
    // BVH leaves arrive bucket-sorted, not in face order: push them with
    // descending index offsets so reconstruction must re-sort.
    for rect in expected.iter().rev() {
        let base = vertices.len() as u32;
        vertices.extend(face_vertices(
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            rect.layer as u16 + 1,
            2048.0,
        ));
        leaves.push(face_leaf(indices.len() as u32, 6, rect.cell));
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    leaves.reverse();
    // An uncharted face (all-zero UVs) and a degenerate two-index face.
    let base = vertices.len() as u32;
    vertices.extend(face_vertices(0, 0, 5, 5, 0, f32::INFINITY));
    leaves.push(face_leaf(indices.len() as u32, 3, 2));
    indices.extend([base, base + 1, base + 2]);
    leaves.push(face_leaf(indices.len() as u32, 2, 2));
    indices.extend([base, base + 1]);

    // Face records in bake order: the three charts (cells 1, 3, 0), the
    // uncharted and short faces of cell 2, then a zero-index face of cell 3
    // that has no BVH leaf at all.
    let face = |leaf_index| FaceMeta {
        leaf_index,
        texture_index: 0,
    };
    let geometry = GeometrySection {
        vertices,
        indices,
        faces: vec![face(1), face(3), face(0), face(2), face(2), face(3)],
    };
    let bvh = BvhSection {
        nodes: Vec::new(),
        leaves,
        root_node_index: 0,
    };
    let (charts, faces, stats) = reconstruct_charts(&geometry, &bvh, &formats, 4).unwrap();
    let mut in_face_order = expected.to_vec();
    in_face_order.reverse();
    assert_eq!(charts, in_face_order);
    assert_eq!(
        faces,
        vec![
            FaceSlot::Chart(0),
            FaceSlot::Chart(1),
            FaceSlot::Chart(2),
            FaceSlot::Placeholder { cell: 2 },
            FaceSlot::Placeholder { cell: 2 },
            FaceSlot::Placeholder { cell: 3 },
        ]
    );
    assert_eq!(stats.uncharted_faces, 1);
    assert_eq!(stats.short_index_faces, 1);
    assert_eq!(stats.zero_index_faces, 1);
    assert_eq!(stats.unmatched_bvh_faces, 0);
    assert_eq!(stats.charts, 3);
}

#[test]
fn hub_metric_recompute_matches_hand_computed_chain_distance() {
    // Three 4 m cubes along X joined by unit portals at x = 4 and x = 8.
    let cube = |x: f32| HubCell {
        bounds_min: [x, 0.0, 0.0],
        bounds_max: [x + 4.0, 4.0, 4.0],
        solid: false,
    };
    let portal = |x: f32, front, back| HubPortal {
        front,
        back,
        vertices: vec![[x, 1.5, 1.5], [x, 2.5, 1.5], [x, 2.5, 2.5], [x, 1.5, 2.5]],
    };
    let graph = PortalGraphInput {
        cells: vec![cube(0.0), cube(4.0), cube(8.0)],
        portals: vec![portal(4.0, 0, 1), portal(8.0, 1, 2)],
    };
    // Cell centers at x = 2, 6, 10; portal centroids at (4|8, 2, 2).
    let pairs = recompute_pairs(&graph, 128 * METER);
    let distances: Vec<(u32, u32, u32)> = pairs
        .iter()
        .map(|p| (p.cell_a, p.cell_b, p.distance))
        .collect();
    assert_eq!(
        distances,
        vec![(0, 1, 4 * METER), (0, 2, 8 * METER), (1, 2, 4 * METER)]
    );
    let bounded = recompute_pairs(&graph, 4 * METER);
    assert_eq!(bounded.len(), 2, "the 8 m pair lies beyond a 4 m bound");

    let stored = vec![pair(0, 1, 4 * METER), pair(0, 2, 8 * METER + 1)];
    let validation = validate_against_stored(&stored, &pairs, 128 * METER);
    assert_eq!(
        (validation.checked, validation.matched, validation.missing),
        (2, 2, 0)
    );
    assert_eq!(validation.max_abs_diff, 1);
}

#[test]
fn stored_repack_places_degenerate_face_placeholders_like_the_bake() {
    // Cell 0: an aligned 64² chart plus a degenerate face, packed into one
    // block with the bake's cell-block packer, so the placeholder grows the
    // block past the chart; cell 1: a 2² chart alone.
    let cell0 = pack_cell_block(&[(64, 64), (1, 1)], 4).unwrap();
    assert!(cell0.area() > 64 * 64, "the placeholder must take room");
    let cell1 = pack_cell_block(&[(2, 2)], 4).unwrap();
    let mut formats = bc6h_formats(64, 2, false);
    for (block, packed) in formats.blocks.iter_mut().zip([&cell0, &cell1]) {
        block.width = packed.width;
        block.height = packed.height;
    }
    let (x0, y0) = cell0.placements[0];
    let (x1, y1) = cell1.placements[0];
    let mut fixture = input(
        formats,
        &[0, 1],
        vec![chart(0, 0, x0, y0, 64, 64), chart(1, 1, x1, y1, 2, 2)],
        Vec::new(),
    );
    fixture.faces = vec![
        FaceSlot::Chart(0),
        FaceSlot::Placeholder { cell: 0 },
        FaceSlot::Chart(1),
    ];
    let check = stored_repack_matches(&fixture);
    assert!(check.reproduces_stored(), "{check:?}");
    assert_eq!((check.matched, check.total), (2, 2));

    // Dropping the placeholder shrinks cell 0's block: the stored extent no
    // longer matches, which is exactly the mismatch the check exists for.
    fixture.faces = vec![FaceSlot::Chart(0), FaceSlot::Chart(1)];
    let check = stored_repack_matches(&fixture);
    assert!(check.error.is_none());
    assert!(!check.reproduces_stored(), "{check:?}");
}

#[test]
fn missing_cell_visibility_makes_every_stored_bound_the_whole_component() {
    let mut fixture = corridor_input();
    fixture.cell_visibility_present = false;
    fixture.coupled_pairs.clear();
    fixture.component_ids = vec![0; fixture.cell_count()];
    let report = run_dry_run(&fixture);
    for source in report
        .sources
        .iter()
        .filter(|s| s.name.starts_with("id-46"))
    {
        assert!(source.note.is_some());
        for bound in &source.bounds {
            for (cell, bytes) in &bound.cells {
                assert_eq!(bytes.cell_count, 8, "cell {cell} {:?}", bound.bound);
            }
        }
    }
}

#[test]
fn shadowmask_charges_follow_the_bake_width_rule() {
    let mut formats = bc6h_formats(8192, 1, true);
    let lightmap_only = |dim: u32| u64::from(dim) * u64::from(dim) * 3 / 2;
    let with_mask = |dim: u32| lightmap_only(dim) + u64::from(dim) * u64::from(dim) * 2;
    // Stored id 42: charged where the doubled width fits, never past it.
    assert_eq!(formats.layer_bytes_at(1024, 1024), with_mask(1024));
    assert_eq!(formats.layer_bytes_at(4096, 4096), with_mask(4096));
    assert_eq!(formats.layer_bytes_at(8192, 8192), lightmap_only(8192));

    // Omitted for width: the floor and the stored layers carry none, but a
    // simulated layer narrow enough to double is charged.
    formats.shadowmask = ShadowmaskState::OmittedForWidth;
    assert_eq!(formats.shadowmask_bytes_per_texel(), 0.0);
    assert_eq!(formats.stored_block_bytes(0), lightmap_only(8192));
    assert_eq!(formats.layer_bytes_at(1024, 1024), with_mask(1024));
    assert_eq!(formats.layer_bytes_at(8192, 8192), lightmap_only(8192));

    formats.shadowmask = ShadowmaskState::Absent;
    assert_eq!(formats.layer_bytes_at(1024, 1024), lightmap_only(1024));
}

#[test]
fn omitted_for_width_shadowmask_adds_a_restored_floor_column() {
    let mut fixture = corridor_input();
    let report = run_dry_run(&fixture);
    let (_, stored) = &report.sources[0].bounds[0].cells[0];
    assert!(stored.with_omitted_mask.is_none());
    assert!(!report.render().contains("omitted id42"));

    fixture.formats.shadowmask = ShadowmaskState::OmittedForWidth;
    let report = run_dry_run(&fixture);
    for source in &report.sources {
        for bound in &source.bounds {
            for (cell, bytes) in &bound.cells {
                let (texel_exact, half_res) = bytes.with_omitted_mask.expect("restored column");
                // Id 22 alone is 1.5 B/texel; the restored mask adds 2.
                let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
                assert!(
                    close(texel_exact * 1.5, bytes.texel_exact * 3.5),
                    "cell {cell}"
                );
                assert!(close(half_res * 1.5, bytes.half_res * 3.5), "cell {cell}");
            }
        }
    }
    let rendered = report.render();
    assert!(rendered.contains("texel-exact + omitted id42"));
    assert!(rendered.contains("half-res + omitted id42"));
    assert!(report.csv().contains("texel_exact_with_omitted_id42_bytes"));
    assert!(report.csv().contains("half_res_with_omitted_id42_bytes"));
}

/// Two portal components: cells 0–3 are 20 m cubes chained along X, cells 4
/// and 5 a separate pair far away. Clusters {0, 1}, {2}, {3}, {4, 5}.
fn two_component_portal_input() -> super::DryRunInput {
    let cube = |x: f32| HubCell {
        bounds_min: [x, 0.0, 0.0],
        bounds_max: [x + 20.0, 4.0, 4.0],
        solid: false,
    };
    let portal = |x: f32, front, back| HubPortal {
        front,
        back,
        vertices: vec![[x, 1.5, 1.5], [x, 2.5, 1.5], [x, 2.5, 2.5], [x, 1.5, 2.5]],
    };
    let graph = PortalGraphInput {
        cells: vec![
            cube(0.0),
            cube(20.0),
            cube(40.0),
            cube(60.0),
            cube(1000.0),
            cube(1020.0),
        ],
        portals: vec![
            portal(20.0, 0, 1),
            portal(40.0, 1, 2),
            portal(60.0, 2, 3),
            portal(1020.0, 4, 5),
        ],
    };
    let stored = recompute_pairs(&graph, 128 * METER);
    let charts = (0..6)
        .map(|cell| chart(cell, 0, cell * 20, 0, 12 + cell, 9))
        .collect();
    let mut fixture = input(
        bc6h_formats(128, 1, true),
        &[0, 0, 1, 2, 3, 3],
        charts,
        stored,
    );
    fixture.component_ids = vec![0, 0, 0, 0, 1, 1];
    fixture.portal_graph = Some(graph);
    fixture
}

#[test]
fn recompute_and_cluster_closure_grow_monotonically_and_deterministically() {
    let fixture = two_component_portal_input();
    let report = run_dry_run(&fixture);
    assert_eq!(report.sources.len(), 4, "two sources × two granularities");
    assert!(report.validation.unwrap().all_matched());
    for source in &report.sources {
        for window in source.bounds.windows(2) {
            for ((cell, near), (_, far)) in window[0].cells.iter().zip(&window[1].cells) {
                let case = format!("{} {:?} cell {cell}", source.name, source.granularity);
                assert!(far.cell_count >= near.cell_count, "{case}");
                assert!(far.texel_exact >= near.texel_exact, "{case}");
                assert!(far.half_res >= near.half_res, "{case}");
                for (f, n) in far.layer.iter().zip(&near.layer) {
                    assert!(f >= n, "{case}");
                }
            }
        }
    }
    // Cluster closure never admits less than the cell-granular set.
    for pair in report.sources.chunks(2) {
        let (cell, closure) = (&pair[0], &pair[1]);
        assert_eq!(
            (cell.granularity, closure.granularity),
            (Granularity::Cell, Granularity::ClusterClosure)
        );
        for (c, k) in cell.bounds.iter().zip(&closure.bounds) {
            for ((_, c), (_, k)) in c.cells.iter().zip(&k.cells) {
                assert!(k.texel_exact >= c.texel_exact);
                assert!(k.cell_count >= c.cell_count);
            }
        }
    }
    // Cell 0's reach grows 16 m → 32 m → 64 m and stops at its component.
    let recompute_cell = &report.sources[2];
    let sets: Vec<usize> = recompute_cell
        .bounds
        .iter()
        .map(|b| b.cells[0].1.cell_count)
        .collect();
    assert_eq!(sets, vec![2, 2, 4, 4, 4]);

    let again = run_dry_run(&two_component_portal_input());
    assert_eq!(report, again);
    assert_eq!(report.render(), again.render());
    assert_eq!(report.csv(), again.csv());
}

/// The yardstick reads new-format PRLs: charts recovered from a real
/// compiler layout's block ids and block-local UVs attribute every block
/// byte, and the moved `pack_cell_block` repack reproduces the stored blocks.
#[test]
fn dry_run_recovers_charts_and_repacks_a_compiler_cell_block_layout() {
    use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, LightmapHeader, LightmapMode};

    let mut fixture = crate::fixture_pipeline::load_fixture("soft_shadow_test");
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&fixture.lights);
    let prepared =
        crate::lightmap_bake::prepare_atlas(&mut fixture.geometry, &static_lights, 0.25, &[])
            .unwrap();
    let (_, _, bvh) = crate::bvh_build::build_bvh(&fixture.geometry).unwrap();
    let layout = &prepared.layout;
    assert!(layout.blocks.len() > 1);

    let header = LightmapHeader {
        block_count: layout.blocks.len() as u32,
        direction_texel_scale: layout.direction_texel_scale,
        irradiance_format: IRRADIANCE_FORMAT_BC6H,
        mode: LightmapMode::Shadowed,
    };
    let mut formats = bc6h_formats(64, 0, false);
    formats.direction_texel_scale = layout.direction_texel_scale;
    formats.blocks = layout
        .blocks
        .iter()
        .map(|block| super::StoredBlock {
            cell: block.cell_id,
            width: block.width,
            height: block.height,
            irradiance_bytes: header.irradiance_len(block.width, block.height).unwrap(),
            direction_bytes: header.direction_len(block.width, block.height).unwrap(),
        })
        .collect();
    let cell_count = fixture.tree.leaves.len() as u32;
    let (charts, faces, stats) =
        reconstruct_charts(&fixture.geometry.geometry, &bvh, &formats, cell_count).unwrap();
    assert_eq!(
        (
            stats.mixed_layer_faces,
            stats.out_of_bounds_faces,
            stats.unmatched_bvh_faces
        ),
        (0, 0, 0),
        "{stats:?}"
    );
    assert!(!charts.is_empty());

    let mut dry_run = input(formats, &vec![0; cell_count as usize], charts, Vec::new());
    dry_run.faces = faces;
    let check = stored_repack_matches(&dry_run);
    assert!(check.reproduces_stored(), "{check:?}");

    let attribution = attribute(&dry_run.formats, &dry_run.charts, dry_run.cell_count());
    assert_eq!(attribution.overlap_texels, 0);
    for section in [&attribution.irradiance, &attribution.direction] {
        assert_eq!(section.attributed() + section.unattributed, section.payload);
    }
}
