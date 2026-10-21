// Working-set gate boundaries across atlas preparation, memo admission and bake.
// See: context/lib/build_pipeline.md §Compiler pipeline
use super::*;

fn prepare(geometry: &mut GeometryResult, lights: &[MapLight], density: f32) -> PreparedAtlas {
    lightmap_bake::prepare_atlas_within(
        geometry,
        &StaticBakedLights::from_lights(lights),
        density,
        &[],
        lightmap_bake::BlockOrdering::by_cell_id(2),
        64,
        &BakeControl::unrestricted(),
    )
    .unwrap()
}

#[test]
fn budget_gate_compiles_over_256_layers_and_pre_cut_area_above_the_retired_cap() {
    let mut geometry = quads_in_one_cell(257);
    let lights = vec![point_light(DVec3::new(6.0, 3.0, 6.0), [1.0; 3])];
    let prepared = prepare(&mut geometry, &lights, 0.2);
    assert!(prepared.layer_count > 256, "{}", prepared.layer_count);
    let minimum = lightmap_bake::minimum_working_set_bytes(&geometry, &prepared.charts, 64, false);
    assert!(minimum > 256 * 64 * 64, "{minimum}");
    for uncompressed in [false, true] {
        let minimum =
            lightmap_bake::minimum_working_set_bytes(&geometry, &prepared.charts, 64, uncompressed);
        assert!(
            minimum
                <= predicted_peak(
                    &prepared,
                    metadata_counts(&prepared, &lights),
                    uncompressed,
                    LIGHTMAP_PARTITION_WINDOW
                )
                .total()
        );
    }
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let mut args = test_args();
    args.working_set_max_bytes = predicted_peak(
        &prepared,
        metadata_counts(&prepared, &lights),
        false,
        LIGHTMAP_PARTITION_WINDOW,
    )
    .total();
    let out = bake_fused_prepared(
        &args,
        None,
        &BakeControl::unrestricted(),
        &BakeControl::unrestricted(),
        &mut geometry,
        &StaticBakedLights::from_lights(&lights),
        &AlphaLightsNs::from_lights(&lights),
        None,
        &[],
        &bvh,
        &primitives,
        &config(false),
        prepared,
    )
    .unwrap();
    assert_eq!(out.lightmap.layer_count, 257);
    assert_eq!(out.lightmap.section.blocks.len(), 257);
}

#[test]
fn budget_gate_exact_boundary_compiles_but_one_below_reads_no_lighting_memo() {
    let lights = vec![point_light(DVec3::new(6.0, 3.0, 6.0), [1.0; 3])];
    let dir = tempfile::tempdir().unwrap();
    let cache = StageCache::new(dir.path()).unwrap();
    let run = |budget, cache: Option<&StageCache>| {
        let mut geometry = quads_in_one_cell(3);
        let prepared = prepare(&mut geometry, &lights, 0.25);
        let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
        let mut args = test_args();
        args.working_set_max_bytes = budget;
        bake_fused_prepared(
            &args,
            cache,
            &BakeControl::unrestricted(),
            &BakeControl::unrestricted(),
            &mut geometry,
            &StaticBakedLights::from_lights(&lights),
            &AlphaLightsNs::from_lights(&lights),
            None,
            &[],
            &bvh,
            &primitives,
            &config(false),
            prepared,
        )
    };
    let prepared = prepare(&mut quads_in_one_cell(3), &lights, 0.25);
    let prediction = predicted_peak(
        &prepared,
        metadata_counts(&prepared, &lights),
        false,
        LIGHTMAP_PARTITION_WINDOW,
    )
    .total();
    let cold = run(prediction, None).unwrap();
    let populated = run(prediction, Some(&cache)).unwrap();
    let warm = run(prediction, Some(&cache)).unwrap();
    assert_eq!(cold.lightmap.section, populated.lightmap.section);
    assert_eq!(cold.lightmap.section, warm.lightmap.section);
    assert_eq!(cold.shadowmask, warm.shadowmask);
    let before_lm = cache.test_access("lightmap_section");
    let before_sm = cache.test_access("shadowmask_atlas");
    let before_partitions = cache.test_access("lightmap_layer");
    let cold_error = run(prediction - 1, None).err().unwrap().to_string();
    let warm_error = run(prediction - 1, Some(&cache)).err().unwrap().to_string();
    assert_eq!(cold_error, warm_error);
    for required in [
        prediction.to_string(),
        (prediction - 1).to_string(),
        prepared.layer_count.to_string(),
        "--working-set-max-size".into(),
    ] {
        assert!(warm_error.contains(&required), "{warm_error}");
    }
    assert_eq!(cache.test_access("lightmap_section"), before_lm);
    assert_eq!(cache.test_access("shadowmask_atlas"), before_sm);
    assert_eq!(cache.test_access("lightmap_layer"), before_partitions);
}

#[test]
fn budget_gate_no_static_or_no_placed_chart_costs_zero() {
    let mut geometry = quads_in_one_cell(257);
    let no_static = prepare(&mut geometry, &[], 0.001);
    assert!(no_static.placements.is_empty());
    assert!(no_static.layout.blocks.is_empty());
    assert_eq!(
        predicted_peak(&no_static, metadata_counts(&no_static, &[]), false, 8).total(),
        0
    );
    assert_eq!(
        predicted_peak(&no_static, metadata_counts(&no_static, &[]), true, 8).total(),
        0
    );
    peak::gate_working_set(&no_static, metadata_counts(&no_static, &[]), true, 8, 0).unwrap();
    let lights = vec![point_light(DVec3::new(0.0, 3.0, 0.0), [1.0; 3])];
    let mut empty = geometry.clone();
    empty.geometry.vertices.clear();
    empty.geometry.indices.clear();
    empty.geometry.faces.clear();
    empty.face_index_ranges.clear();
    let prepared = prepare(&mut empty, &lights, 0.25);
    assert_eq!(
        predicted_peak(&prepared, metadata_counts(&prepared, &lights), false, 8).total(),
        0
    );
    peak::gate_working_set(&prepared, metadata_counts(&prepared, &lights), false, 8, 0).unwrap();
}

#[test]
fn budget_gate_shadowmask_term_counts_one_layer_plus_cut_carry_and_encode_scratch() {
    let lights = broad_and_corner();
    let mut prepared = prepare(&mut quads_in_one_cell(3), &lights, 0.25);
    let before = predicted_peak(&prepared, metadata_counts(&prepared, &lights), false, 8);
    prepared.layer_count += 100;
    assert_eq!(
        before.shadowmask_fill,
        predicted_peak(&prepared, metadata_counts(&prepared, &lights), false, 8).shadowmask_fill
    );
    assert_eq!(
        before.shadowmask_encode_scratch,
        10 * u64::from(prepared.atlas_width) * u64::from(prepared.atlas_height)
    );
    let cut = prepare(&mut horizontal_quads(&[FLOOR]), &lights, 0.25);
    assert!(!cut.cut_parents.is_empty());
    for uncompressed in [false, true] {
        // The uncut parent is the pre-cut input to the lower bound.
        let parent = horizontal_quads(&[FLOOR]);
        let charts = lightmap_bake::plan_cut_charts(
            &mut parent.clone(),
            &StaticBakedLights::from_lights(&lights),
            0.25,
            &[],
            2048,
            u64::MAX,
            uncompressed,
        )
        .unwrap()
        .charts;
        let minimum = lightmap_bake::minimum_working_set_bytes(&parent, &charts, 64, uncompressed);
        let peak = predicted_peak(&cut, metadata_counts(&cut, &lights), uncompressed, 8);
        assert!(minimum <= peak.total());
        assert_eq!(
            peak.shadowmask_fill,
            4 * u64::from(cut.atlas_width) * u64::from(cut.atlas_height)
                + shadowmask_bake::maximum_cut_carry_bytes(&cut)
        );
    }
}
