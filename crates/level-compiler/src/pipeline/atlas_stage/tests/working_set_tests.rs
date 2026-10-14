// Pre-cut lower-bound proof against the packed stage's actual prediction.
use super::*;
use crate::lightmap_bake::{BlockOrdering, minimum_working_set_bytes};

#[test]
fn pre_cut_budget_exact_minimum_passes_and_one_below_refuses_before_cutting() {
    let (geometry, _) = three_leaf_fixture();
    let lights = lit();
    let static_lights = StaticBakedLights::from_lights(&lights);
    let charts = lightmap_bake::plan_cut_charts(
        &mut geometry.clone(),
        &static_lights,
        0.1,
        &[],
        2048,
        u64::MAX,
        false,
    )
    .unwrap()
    .charts;
    for uncompressed in [false, true] {
        let minimum = minimum_working_set_bytes(&geometry, &charts, 64, uncompressed);
        let mut refused = geometry.clone();
        let error = lightmap_bake::plan_cut_charts(
            &mut refused,
            &static_lights,
            0.1,
            &[],
            64,
            minimum - 1,
            uncompressed,
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("working-set lower bound"));
        assert!(error.to_string().contains("--working-set-max-size"));
        assert_eq!(
            refused.face_index_ranges.len(),
            geometry.face_index_ranges.len()
        );
        let mut admitted = geometry.clone();
        let cut = lightmap_bake::plan_cut_charts(
            &mut admitted,
            &static_lights,
            0.1,
            &[],
            64,
            minimum,
            uncompressed,
        )
        .unwrap();
        assert!(cut.face_remap.is_some(), "equality must reach the cut");
    }
}

#[test]
fn pre_cut_sparse_polygon_is_admitted_when_empty_grid_windows_exceed_the_budget() {
    let (mut geometry, _) = three_leaf_fixture();
    geometry.geometry.vertices.clear();
    geometry.geometry.indices.clear();
    geometry.geometry.faces.clear();
    geometry.face_index_ranges.clear();
    // Short first edge fixes the chart axes; a long, thin diagonal triangle
    // then misses almost every window in its large axis-aligned cut grid.
    let points = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [3000.0, 0.0, 3000.0]];
    geometry.geometry.vertices.extend(points.map(|p| {
        Vertex::new(
            p,
            [0.0; 2],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0; 2],
            0,
        )
    }));
    geometry.geometry.indices = vec![0, 1, 2];
    geometry.geometry.faces = vec![FaceMeta {
        leaf_index: 0,
        texture_index: 0,
    }];
    geometry.face_index_ranges = vec![FaceIndexRange {
        index_offset: 0,
        index_count: 3,
    }];
    let lights = lit();
    let static_lights = StaticBakedLights::from_lights(&lights);
    let charts = lightmap_bake::plan_cut_charts(
        &mut geometry.clone(),
        &static_lights,
        1.0,
        &[],
        8192,
        u64::MAX,
        false,
    )
    .unwrap()
    .charts;
    let windows =
        lightmap_bake::planned_chart_area(&charts, &lightmap_bake::plan_face_cuts(&charts, 64));
    for uncompressed in [false, true] {
        let mut packed_geo = geometry.clone();
        let cut = lightmap_bake::plan_cut_charts(
            &mut packed_geo,
            &static_lights,
            1.0,
            &[],
            64,
            u64::MAX,
            uncompressed,
        )
        .unwrap();
        let packed = lightmap_bake::pack_cut_charts(
            &mut packed_geo,
            &static_lights,
            cut.charts,
            cut.cut_parents,
            BlockOrdering::by_cell_id(2),
            64,
            &BakeControl::unrestricted(),
        )
        .unwrap();
        let prediction = crate::pipeline::lightmap_stage::predicted_peak(
            &packed,
            crate::pipeline::lightmap_stage::MetadataCounts::new(
                &static_lights,
                &crate::light_namespaces::AlphaLightsNs::from_lights(&lights),
                packed.charts.len(),
            ),
            uncompressed,
            8,
        )
        .total();
        let minimum = minimum_working_set_bytes(&geometry, &charts, 64, uncompressed);
        assert!(
            minimum <= prediction,
            "minimum{minimum} prediction{prediction}"
        );
        assert!(
            windows * if uncompressed { 8 } else { 1 } > prediction,
            "windows{windows} prediction{prediction}"
        );
        lightmap_bake::plan_cut_charts(
            &mut geometry.clone(),
            &static_lights,
            1.0,
            &[],
            64,
            prediction,
            uncompressed,
        )
        .unwrap();
    }
}
