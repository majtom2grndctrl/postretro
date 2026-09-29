// Compiler pipeline tests: delta working-set gate, SH coarsening dispatch, stage contract, and stage logs.
// See: context/lib/build_pipeline.md §Compiler pipeline

use super::*;

use glam::DVec3;
use log::Level;
use postretro_test_log_capture::LogCapture;

#[test]
fn direct_delta_plan_bake_csr_divergence_is_caught() {
    let plan = DeltaCsrPlan::from_csr(vec![0, 1, 2], vec![3, 7]);
    ensure_bake_csr_matches_plan("DirectShDeltaVolumes (id 41)", &plan, &[0, 1, 2], &[3, 7])
        .expect("identical CSRs must pass");

    let error =
        ensure_bake_csr_matches_plan("DirectShDeltaVolumes (id 41)", &plan, &[0, 1, 2], &[3, 8])
            .expect_err("a changed flat light list must fail the plan/bake gate");
    assert!(error.to_string().contains("plan/bake CSR divergence"));
}

#[test]
fn delta_working_set_gate_refuses_three_cumulative_40_percent_bakes() {
    let indirect = vec![0, 1];
    let direct = vec![0, 1];
    let animated_direct = vec![0, 1];

    let error = gate_delta_working_set(
        [
            DeltaCsrProjectionInput {
                label: "DeltaShVolumes (id 27)",
                affinity_lights: &indirect,
                static_indices: None,
            },
            DeltaCsrProjectionInput {
                label: "DirectShDeltaVolumes (id 41)",
                affinity_lights: &direct,
                static_indices: None,
            },
            DeltaCsrProjectionInput {
                label: "AnimatedDirectShDeltaVolumes (id 45)",
                affinity_lights: &animated_direct,
                static_indices: None,
            },
        ],
        5,
        150,
        DELTA_WORKING_SET_COPY_FACTOR,
    )
    .expect_err("three 40%-sized dense bakes must be gated cumulatively");

    let DeltaWorkingSetGateError::BudgetExceeded(projection) = error else {
        panic!("only the budget should reject this bounded projection");
    };
    assert_eq!(projection.cumulative_dense_bytes, 60);
    assert_eq!(projection.estimated_peak_bytes, 180);
    assert_eq!(projection.bakes.len(), 3);
    assert!(
        projection.bakes.iter().all(|bake| bake.dense_bytes == 20),
        "each individual bake fits the budget; only the co-resident sum refuses"
    );
}

#[test]
fn delta_working_set_gate_reserves_base_headroom_at_all_valid_compaction_boundary() {
    let all_valid_dense = vec![0; 10];
    let empty = [];
    let inputs = || {
        [
            DeltaCsrProjectionInput {
                label: "DeltaShVolumes (id 27)",
                affinity_lights: &all_valid_dense,
                static_indices: None,
            },
            DeltaCsrProjectionInput {
                label: "DirectShDeltaVolumes (id 41)",
                affinity_lights: &empty,
                static_indices: None,
            },
            DeltaCsrProjectionInput {
                label: "AnimatedDirectShDeltaVolumes (id 45)",
                affinity_lights: &empty,
                static_indices: None,
            },
        ]
    };

    // Regression: all-valid L0 compaction needs a second 100-byte delta
    // buffer. The former 2x estimate admitted at 200 bytes and left no room
    // for the id 34/id 35 originals and clones that remain live here.
    let error =
        gate_delta_working_set(inputs(), 5, 200, delta_working_set_copy_chain_factor(false))
            .expect_err("the exact delta-only compaction boundary must retain base headroom");
    let DeltaWorkingSetGateError::BudgetExceeded(projection) = error else {
        panic!("only the budget should reject this bounded projection");
    };
    assert_eq!(projection.cumulative_dense_bytes, 100);
    assert_eq!(projection.copy_chain_factor, 3);
    assert_eq!(projection.estimated_peak_bytes, 300);

    let admitted =
        gate_delta_working_set(inputs(), 5, 300, delta_working_set_copy_chain_factor(false))
            .expect("the conservative projection remains inclusive at its own boundary");
    assert_eq!(admitted.estimated_peak_bytes, 300);
}

#[test]
fn delta_working_set_gate_attributes_a_dominant_direct_bake_before_execution() {
    let indirect = vec![0];
    let direct = vec![1; 50];
    let animated_direct = vec![0];
    let static_indices = vec![7, 42];

    let error = gate_delta_working_set(
        [
            DeltaCsrProjectionInput {
                label: "DeltaShVolumes (id 27)",
                affinity_lights: &indirect,
                static_indices: None,
            },
            DeltaCsrProjectionInput {
                label: "DirectShDeltaVolumes (id 41)",
                affinity_lights: &direct,
                static_indices: Some(&static_indices),
            },
            DeltaCsrProjectionInput {
                label: "AnimatedDirectShDeltaVolumes (id 45)",
                affinity_lights: &animated_direct,
                static_indices: None,
            },
        ],
        4,
        800,
        DELTA_WORKING_SET_COPY_FACTOR,
    )
    .expect_err("the direct CSR alone dominates the pre-bake projection");

    let diagnostic = error.to_string();
    assert!(diagnostic.contains("DeltaShVolumes (id 27) dense 8 bytes"));
    assert!(diagnostic.contains("DirectShDeltaVolumes (id 41) dense 400 bytes"));
    assert!(diagnostic.contains("AnimatedDirectShDeltaVolumes (id 45) dense 8 bytes"));
    assert!(diagnostic.contains("light 1 (static_index 42): 50 CSR entries, 400 dense bytes"));
}

#[test]
fn delta_working_set_gate_admits_empty_csrs_without_histogram_rows() {
    let empty = [];
    let empty_static_indices: [u64; 0] = [];
    let projection = gate_delta_working_set(
        [
            DeltaCsrProjectionInput {
                label: "DeltaShVolumes (id 27)",
                affinity_lights: &empty,
                static_indices: None,
            },
            DeltaCsrProjectionInput {
                label: "DirectShDeltaVolumes (id 41)",
                affinity_lights: &empty,
                static_indices: Some(&empty_static_indices),
            },
            DeltaCsrProjectionInput {
                label: "AnimatedDirectShDeltaVolumes (id 45)",
                affinity_lights: &empty,
                static_indices: None,
            },
        ],
        13_824 / 2,
        0,
        DELTA_WORKING_SET_COPY_FACTOR,
    )
    .expect("zero entries must not divide by zero or consume the budget");

    assert_eq!(projection.estimated_peak_bytes, 0);
    assert!(
        projection
            .bakes
            .iter()
            .all(|bake| bake.histogram.is_empty())
    );
}

#[test]
fn delta_working_set_gate_applies_analysis_factor_only_when_coarsening_retains_clone() {
    let one = [0];
    let inputs = || {
        [
            DeltaCsrProjectionInput {
                label: "DeltaShVolumes (id 27)",
                affinity_lights: &one,
                static_indices: None,
            },
            DeltaCsrProjectionInput {
                label: "DirectShDeltaVolumes (id 41)",
                affinity_lights: &one,
                static_indices: None,
            },
            DeltaCsrProjectionInput {
                label: "AnimatedDirectShDeltaVolumes (id 45)",
                affinity_lights: &one,
                static_indices: None,
            },
        ]
    };

    let normal_factor =
        delta_working_set_copy_chain_factor(retains_sh_analyze_dense_deltas(false, true));
    let normal = gate_delta_working_set(inputs(), 1, 21, normal_factor)
        .expect("two delta copies plus base-copy headroom fit the selected budget");
    assert_eq!(normal.estimated_peak_bytes, 18);
    let uniform_l0_analysis_factor =
        delta_working_set_copy_chain_factor(retains_sh_analyze_dense_deltas(true, false));
    let uniform_l0_analysis = gate_delta_working_set(inputs(), 1, 21, uniform_l0_analysis_factor)
        .expect("--sh-analyze without coarsening retains no dense clone");
    assert_eq!(
        uniform_l0_analysis.copy_chain_factor,
        DELTA_WORKING_SET_COPY_FACTOR
    );
    assert_eq!(uniform_l0_analysis.estimated_peak_bytes, 18);

    let coarsened_analysis_factor =
        delta_working_set_copy_chain_factor(retains_sh_analyze_dense_deltas(true, true));
    let error = gate_delta_working_set(inputs(), 1, 21, coarsened_analysis_factor)
        .expect_err("the retained --sh-analyze clone makes four shares exceed the budget");
    let DeltaWorkingSetGateError::BudgetExceeded(analyze) = error else {
        panic!("the analysis factor must be the only rejection cause");
    };
    assert_eq!(
        analyze.copy_chain_factor,
        DELTA_WORKING_SET_ANALYZE_COPY_FACTOR
    );
    assert_eq!(analyze.estimated_peak_bytes, 24);
}

#[test]
fn delta_working_set_admission_logs_one_summary_without_histograms() {
    let indirect = [3, 3, 1];
    let direct = [0, 0];
    let animated_direct = [7];
    let static_indices = [31];
    let projection = gate_delta_working_set(
        [
            DeltaCsrProjectionInput {
                label: "DeltaShVolumes (id 27)",
                affinity_lights: &indirect,
                static_indices: None,
            },
            DeltaCsrProjectionInput {
                label: "DirectShDeltaVolumes (id 41)",
                affinity_lights: &direct,
                static_indices: Some(&static_indices),
            },
            DeltaCsrProjectionInput {
                label: "AnimatedDirectShDeltaVolumes (id 45)",
                affinity_lights: &animated_direct,
                static_indices: None,
            },
        ],
        1,
        100,
        DELTA_WORKING_SET_COPY_FACTOR,
    )
    .expect("small diagnostic fixture is admitted");
    let capture = LogCapture::start();

    log_delta_working_set_projection(&projection, false);

    capture.assert_logged_once(Level::Info, "SH delta working-set gate:");
    capture.assert_not_logged(Level::Info, "SH delta working-set histogram:");
}

#[test]
fn delta_working_set_verbose_logging_labels_each_bake_histogram() {
    let indirect = [3, 3];
    let direct = [0];
    let animated_direct = [7];
    let static_indices = [31];
    let projection = gate_delta_working_set(
        [
            DeltaCsrProjectionInput {
                label: "DeltaShVolumes (id 27)",
                affinity_lights: &indirect,
                static_indices: None,
            },
            DeltaCsrProjectionInput {
                label: "DirectShDeltaVolumes (id 41)",
                affinity_lights: &direct,
                static_indices: Some(&static_indices),
            },
            DeltaCsrProjectionInput {
                label: "AnimatedDirectShDeltaVolumes (id 45)",
                affinity_lights: &animated_direct,
                static_indices: None,
            },
        ],
        1,
        100,
        DELTA_WORKING_SET_COPY_FACTOR,
    )
    .expect("small diagnostic fixture is admitted");
    let capture = LogCapture::start();

    log_delta_working_set_projection(&projection, true);

    capture.assert_logged_once(Level::Info, "DeltaShVolumes (id 27) light 3");
    capture.assert_logged_once(
        Level::Info,
        "DirectShDeltaVolumes (id 41) selection slot 0 (static_index 31)",
    );
    capture.assert_logged_once(Level::Info, "AnimatedDirectShDeltaVolumes (id 45) light 7");
}

#[test]
fn combined_protect_aabbs_concatenates_cli_then_map_sources() {
    let cli = [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
    let map = [
        [2.0, 2.0, 2.0, 3.0, 3.0, 3.0],
        [4.0, 4.0, 4.0, 5.0, 5.0, 5.0],
    ];
    let combined = combined_protect_aabbs(&cli, &map);
    // Both sources reach the classifier: neither is dropped, and CLI boxes
    // lead so the order is stable for the coarsening sweep.
    assert_eq!(combined.len(), 3);
    assert_eq!(combined[0], cli[0]);
    assert_eq!(combined[1], map[0]);
    assert_eq!(combined[2], map[1]);
}

#[test]
fn combined_protect_aabbs_handles_empty_sources() {
    let empty: [[f32; 6]; 0] = [];
    let one = [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
    assert!(combined_protect_aabbs(&empty, &empty).is_empty());
    assert_eq!(combined_protect_aabbs(&one, &empty), one);
    assert_eq!(combined_protect_aabbs(&empty, &one), one);
}

#[test]
fn watertight_open_edge_provenance_names_groups_and_leaves_ungrouped_edges_unnamed() {
    let edge = partition::OpenEdge {
        midpoint: DVec3::ZERO,
        brush_index: 1,
    };
    let assemblies = vec![map_data::MapAssembly {
        provenance: "service tunnel".to_string(),
        group_id: "17".to_string(),
        linked_group_id: None,
    }];

    assert_eq!(
        watertight_open_edge_provenance(&edge, &[None, Some(0)], &assemblies),
        Some("service tunnel".to_string())
    );
    assert_eq!(
        watertight_open_edge_provenance(&edge, &[None, None], &assemblies),
        None,
        "worldspawn brushes retain the existing ungrouped diagnostic"
    );
}

#[test]
fn watertight_open_edge_provenance_disambiguates_same_named_groups() {
    let edge = partition::OpenEdge {
        midpoint: DVec3::ZERO,
        brush_index: 0,
    };
    let assemblies = vec![
        map_data::MapAssembly {
            provenance: "pink ambient lights".to_string(),
            group_id: "3".to_string(),
            linked_group_id: None,
        },
        map_data::MapAssembly {
            provenance: "pink ambient lights".to_string(),
            group_id: "4".to_string(),
            linked_group_id: Some("other-template".to_string()),
        },
    ];

    assert_eq!(
        watertight_open_edge_provenance(&edge, &[Some(1)], &assemblies),
        Some("pink ambient lights (4)".to_string())
    );
}

#[test]
fn uniform_grid_optout_short_circuits_before_classification() {
    let calls = std::cell::Cell::new(0);
    run_sh_coarsening_if_enabled(false, || {
        calls.set(calls.get() + 1);
        Ok(())
    })
    .expect("disabled coarsening remains a valid uniform bake");
    assert_eq!(calls.get(), 0, "classification must not run under opt-out");
}

#[test]
fn default_path_invokes_classification_once() {
    let calls = std::cell::Cell::new(0);
    run_sh_coarsening_if_enabled(true, || {
        calls.set(calls.get() + 1);
        Ok(())
    })
    .expect("default-on classification succeeds");
    assert_eq!(calls.get(), 1);
}

fn direct_delta_stats_fixture() -> direct_sh_bake::DirectDeltaBakeStats {
    direct_sh_bake::DirectDeltaBakeStats {
        rows: vec![
            direct_sh_bake::DirectDeltaBakeStatsRow {
                selection_slot: 3,
                static_index: 17,
                csr_entry_count: 4,
                byte_total: 55_296,
            },
            direct_sh_bake::DirectDeltaBakeStatsRow {
                selection_slot: 1,
                static_index: 5,
                csr_entry_count: 1,
                byte_total: 13_824,
            },
        ],
        total_bytes: 69_120,
    }
}

#[test]
fn planned_stage_contract_pins_order_labels_and_sdf_prediction() {
    let without_sdf = planned_stages_for_sdf(false);
    let with_sdf = planned_stages_for_sdf(true);

    assert_eq!(without_sdf.len(), 26);
    assert_eq!(with_sdf.len(), 26);
    assert_eq!(
        without_sdf
            .iter()
            .map(|stage| (stage.id, stage.label))
            .collect::<Vec<_>>(),
        vec![
            (StageId::Parsing, "Parsing"),
            (StageId::DataScript, "DataScript"),
            (StageId::TextureValidation, "TexValidation"),
            (StageId::Partitioning, "Partitioning"),
            (StageId::Visibility, "Visibility"),
            (StageId::Geometry, "Geometry"),
            (StageId::BvhBuild, "BVH Build"),
            (StageId::CellVisibility, "Cell Visibility"),
            (StageId::NavMesh, "NavMesh"),
            (StageId::ShBake, "SH Bake"),
            (StageId::DeltaShBake, "Delta SH Bake"),
            (StageId::DirectShBake, "Direct SH Bake"),
            (StageId::AnimatedDirectShBake, "Animated Direct SH Bake"),
            (StageId::EntityShadowLights, "EntityShadowLights"),
            (StageId::DirectShDeltaBake, "Direct SH Delta Bake"),
            (
                StageId::BillboardDirectScatterBake,
                "Billboard Direct Scatter Bake",
            ),
            (StageId::ChunkLightList, "ChunkLightList"),
            (StageId::AtlasPreparation, "Atlas Preparation"),
            (StageId::LightmapBake, "Lightmap Bake"),
            (StageId::ShadowmaskAtlas, "ShadowmaskAtlas"),
            (StageId::AnimatedLightChunks, "AnimLightChunks"),
            (StageId::AnimatedWeightMaps, "AnimWeightMaps"),
            (StageId::SdfAtlasBake, "SDF Atlas Bake"),
            (StageId::TextureMips, "TextureMips"),
            (StageId::ClusterDirectory, "ClusterDirectory"),
            (StageId::Packing, "Packing"),
        ]
    );
    assert!(
        without_sdf
            .iter()
            .filter(|stage| stage.id != StageId::SdfAtlasBake)
            .all(|stage| stage.predicted_present)
    );
    let sdf_index = without_sdf
        .iter()
        .position(|stage| stage.id == StageId::SdfAtlasBake)
        .expect("planned stages include SDF atlas bake");
    assert!(!without_sdf[sdf_index].predicted_present);
    assert!(with_sdf[sdf_index].predicted_present);

    let stage_index = |id| {
        without_sdf
            .iter()
            .position(|stage| stage.id == id)
            .expect("stage is planned")
    };
    let atlas_index = stage_index(StageId::AtlasPreparation);
    for sh_stage in [
        StageId::ShBake,
        StageId::DeltaShBake,
        StageId::DirectShBake,
        StageId::AnimatedDirectShBake,
        StageId::EntityShadowLights,
        StageId::DirectShDeltaBake,
        StageId::BillboardDirectScatterBake,
        StageId::ChunkLightList,
    ] {
        assert!(
            stage_index(sh_stage) < atlas_index,
            "{sh_stage:?} must complete before atlas preparation"
        );
    }
    assert!(atlas_index < stage_index(StageId::LightmapBake));
    assert!(stage_index(StageId::LightmapBake) < stage_index(StageId::ShadowmaskAtlas));
}

#[test]
fn direct_delta_info_logging_emits_one_summary_without_histogram() {
    let capture = LogCapture::start();

    log_direct_sh_delta_stats(Some(&direct_delta_stats_fixture()), false);

    let records = capture.records();
    assert_eq!(
        records.len(),
        1,
        "info mode adds exactly one delta log line"
    );
    assert_eq!(records[0].level, Level::Info);
    assert_eq!(
        records[0].message,
        "DirectShDeltaVolumes: 69120 delta bytes; top static_index 17 (selection slot 3, 4 CSR entries, 55296 bytes)"
    );
    capture.assert_not_logged(Level::Info, "DirectShDeltaVolumes histogram:");
}

#[test]
fn direct_delta_verbose_logging_emits_sorted_per_slot_histogram() {
    let capture = LogCapture::start();

    log_direct_sh_delta_stats(Some(&direct_delta_stats_fixture()), true);

    let messages: Vec<_> = capture
        .records()
        .into_iter()
        .map(|record| record.message)
        .collect();
    assert_eq!(
        messages,
        vec![
            "DirectShDeltaVolumes: 69120 delta bytes; top static_index 17 (selection slot 3, 4 CSR entries, 55296 bytes)",
            "DirectShDeltaVolumes histogram: selection slot 3, static_index 17, 4 CSR entries, 55296 bytes",
            "DirectShDeltaVolumes histogram: selection slot 1, static_index 5, 1 CSR entries, 13824 bytes",
        ]
    );
}

#[test]
fn direct_delta_info_logging_is_silent_after_usability_rejection() {
    let capture = LogCapture::start();

    log_direct_sh_delta_stats(None, false);

    assert!(
        capture.records().is_empty(),
        "a discarded delta section must not publish a footprint summary"
    );
}
