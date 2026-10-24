//! Admission and byte-order regressions for lazy layer/light metadata.
use super::*;

fn prepared_at_density(
    geometry: &mut GeometryResult,
    lights: &[MapLight],
    density: f32,
) -> PreparedAtlas {
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
fn metadata_gate_refuses_high_pair_count_before_hashing_or_cache_access() {
    // Regression: 1,024 layers × 10,000 lights admitted 7 MiB, then eagerly
    // allocated hundreds of MiB of items/hashes before the bounded walk.
    let lights: Vec<_> = (0..10_000)
        .map(|i| point_light(DVec3::new(i as f64 * 0.01, 3.0, 6.0), [1.0; 3]))
        .collect();
    let mut geometry = quads_in_one_cell(1024);
    let prepared = prepared_at_density(&mut geometry, &lights, 1.0 / 3.0);
    assert_eq!(prepared.layer_count, 1024);
    assert_eq!(prepared.atlas_width, 64);
    assert!(
        prepared
            .layout
            .blocks
            .iter()
            .all(|b| b.width == 40 && b.height == 40)
    );
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let static_lights = StaticBakedLights::from_lights(&lights);
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let counts = MetadataCounts::new(&static_lights, &alpha_lights, primitives.len());
    let peak = predicted_peak(&prepared, counts, false, LIGHTMAP_PARTITION_WINDOW);
    let texel_only = peak.shadowmask_fill
        + peak.shadowmask_encode_scratch
        + peak.shadowmask_face_map
        + peak.layer_plane
        + peak.partitions
        + peak.sections;
    let budget = 7 * 1024 * 1024;
    assert!(
        texel_only < budget,
        "the former estimator admitted this fixture"
    );
    assert!(peak.metadata >= (lights.len() * std::mem::size_of::<blake3::Hasher>()) as u64);
    assert!(peak.cache_metadata >= 1024 * 10_000 * 40);
    assert!(peak.total() > budget);
    peak::gate_working_set(
        &prepared,
        counts,
        false,
        LIGHTMAP_PARTITION_WINDOW,
        peak.total(),
    )
    .unwrap();
    assert!(
        peak::gate_working_set(
            &prepared,
            counts,
            false,
            LIGHTMAP_PARTITION_WINDOW,
            peak.total() - 1
        )
        .is_err()
    );

    let directory = tempfile::tempdir().unwrap();
    let cache = StageCache::new(directory.path()).unwrap();
    let mut args = test_args();
    args.working_set_max_bytes = budget;
    let mut errors = Vec::new();
    for stage_cache in [None, Some(&cache)] {
        let trial = prepared_at_density(&mut geometry, &lights, 1.0 / 3.0);
        let error = bake_fused_prepared(
            &args,
            stage_cache,
            &BakeControl::unrestricted(),
            &BakeControl::unrestricted(),
            &mut geometry,
            &static_lights,
            &alpha_lights,
            None,
            &[],
            &bvh,
            &primitives,
            &config(false),
            trial,
        )
        .err()
        .expect("metadata must refuse before allocation")
        .to_string();
        assert!(error.contains(&peak.total().to_string()));
        errors.push(error);
    }
    assert_eq!(errors[0], errors[1]);
    for stage in ["lightmap_layer", "lightmap_section", "shadowmask_atlas"] {
        assert_eq!(cache.test_access(stage), Default::default());
    }
}

#[test]
fn lazy_pairs_and_streamed_memos_keep_legacy_order_and_keys() {
    let lights = vec![
        point_light(DVec3::new(6.0, 3.0, 6.0), [1.0; 3]),
        point_light(DVec3::new(20.0, 3.0, 6.0), [0.5; 3]),
        point_light(DVec3::new(34.0, 3.0, 6.0), [0.25; 3]),
    ];
    let mut geometry = quads_in_one_cell(3);
    let prepared = prepared_at_density(&mut geometry, &lights, 0.25);
    let (_, primitives, _) = build_bvh(&geometry).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let static_lights = StaticBakedLights::from_lights(&lights);
    let layer_lights: Vec<_> = static_lights.entries().iter().collect();
    let prefixes = layer_key_prefixes(
        &layer_lights,
        &shared,
        &primitives,
        &geometry,
        0.25,
        4,
        PENUMBRA_FLOOR_DEFAULT,
        2,
        &BakeControl::unrestricted(),
    );
    let old_pairs: Vec<_> = (0..prepared.layer_count)
        .flat_map(|layer| (0..lights.len()).map(move |light| (layer, light)))
        .collect();
    let items = LayerItems::all(prepared.layer_count, lights.len());
    assert_eq!(
        (0..items.len()).map(|i| items.get(i)).collect::<Vec<_>>(),
        old_pairs
    );
    let selected = LayerItems::selected(prepared.layer_count, &[0, 2]);
    assert_eq!(
        (0..selected.len())
            .map(|i| selected.get(i))
            .collect::<Vec<_>>(),
        old_pairs
            .iter()
            .copied()
            .filter(|(_, light)| *light != 1)
            .collect::<Vec<_>>()
    );
    let dense: Vec<_> = old_pairs
        .iter()
        .map(|&(layer, light)| {
            lightmap_layer::layer_input_hash(
                layer_lights[light].light,
                &shared,
                &primitives,
                &geometry,
                0.25,
                4,
                PENUMBRA_FLOOR_DEFAULT,
                layer,
            )
        })
        .collect();
    assert_eq!(
        layer_hashes(&prefixes, prepared.layer_count).collect::<Vec<_>>(),
        dense
    );

    // The old section helper folded the dense layer-major matrix verbatim;
    // the texel-step threshold follows the encode settings.
    let mut old_section = blake3::Hasher::new();
    old_section.update(&lightmap_layer::LAYER_FORMAT_VERSION.to_le_bytes());
    old_section.update(&(dense.len() as u32).to_le_bytes());
    for hash in &dense {
        old_section.update(hash);
    }
    old_section.update(&lightmap_layer::atlas_layout_fingerprint(&shared));
    old_section.update(&0.25f32.to_le_bytes());
    old_section.update(&[0]);
    old_section.update(&2u32.to_le_bytes());
    old_section.update(&TEXEL_STEP_THRESHOLD.to_bits().to_le_bytes());
    assert_eq!(
        *old_section.finalize().as_bytes(),
        lightmap_layer::section_input_hash(
            layer_hashes(&prefixes, prepared.layer_count),
            &shared,
            0.25,
            false,
            2,
            TEXEL_STEP_THRESHOLD,
        )
    );

    let alpha = AlphaLightsNs::from_lights(&lights);
    let eligible = shadowmask_bake::owner_eligible_lights(&static_lights, &alpha, None);
    let mut old_mask = blake3::Hasher::new();
    old_mask.update(&lightmap_layer::LAYER_FORMAT_VERSION.to_le_bytes());
    old_mask.update(&16u32.to_le_bytes()); // Existing memo's lit-texel floor.
    old_mask.update(&(shared.charts.len() as u64).to_le_bytes());
    old_mask.update(&(eligible.len() as u32).to_le_bytes());
    for light in &eligible {
        old_mask.update(&light.spec_index.to_le_bytes());
        old_mask.update(&[u8::from(light.promoted)]);
        for layer in 0..prepared.layer_count {
            old_mask.update(&dense[layer as usize * lights.len() + light.source_index]);
        }
    }
    for word in [
        prepared.atlas_width,
        prepared.atlas_height,
        prepared.layer_count,
        0,
    ] {
        old_mask.update(&word.to_le_bytes());
    }
    assert_eq!(
        *old_mask.finalize().as_bytes(),
        shadowmask_bake::shadowmask_atlas_input_hash(
            &eligible,
            &|source, layer| lightmap_layer::LayerKeyContext::layer_hash(&prefixes[source], layer),
            shared.charts.len(),
            prepared.atlas_width,
            prepared.atlas_height,
            prepared.layer_count,
            &[],
        )
    );
}
