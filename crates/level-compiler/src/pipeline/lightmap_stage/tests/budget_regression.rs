//! Cold section goldens captured before changing the shadowmask fill lifetime.
use super::*;

// Captured cold at 7e7614d36, before production edits: default map density,
// four soft-shadow samples, compressed irradiance, empty stage cache.
fn expected_sections(name: &str) -> (&'static str, &'static str) {
    match name {
        "gate-heavily-lit" => (
            "85ed0cad0b9c3f2fbbf106fc6e138d8811879f3efd7a8b70bbcf2fe0423e7988",
            "b6084630583ea0edb0e3f3d505aee4071ecd06886f911a217272a1998284540e",
        ),
        "soft_shadow_test" => (
            "aea7bcf1140d6b7197cf41237e670a6820144e2e38e2b0d88975a79ba7893818",
            "6402ba73f82a1e86271987e4152f9c1dd403c84e78080770b39361d3d68e4b90",
        ),
        "test_animated_weight_maps_cap" => (
            "fe87dcb8396cfeee8452cce791c76eb38bec190c3741746950c7fb629c8a114d",
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
        ),
        "test_animated_weight_maps_mixed" => (
            "40961fd54f94bfb0eb04a9c407b756b33604f2cc92478f1b4cac082587ff2a18",
            "862f163f768233721ee44d87ccea4676f69a37ad5159e5d70484b5122982bfcd",
        ),
        "test_animated_weight_maps_occluded" => (
            "fe87dcb8396cfeee8452cce791c76eb38bec190c3741746950c7fb629c8a114d",
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
        ),
        "test_animated_weight_maps_single" => (
            "fe87dcb8396cfeee8452cce791c76eb38bec190c3741746950c7fb629c8a114d",
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
        ),
        "four-layer-cut" => (
            "a6faca302b8c39846d56eddc3924eadbc5fc082b55fbddb976ddd25d9c510b12",
            "f1a1d6e5d9e186c654a130d50ef52ce66e3d391a99022ee3f1f3957a4535d0d3",
        ),
        _ => panic!("missing pre-change section baseline: {name}"),
    }
}

fn digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn entries(dir: &std::path::Path) -> Vec<Vec<u8>> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| std::fs::read(entry.unwrap().path()).unwrap())
        .collect()
}

fn assert_cache_contains(dir: &std::path::Path, section: &[u8]) {
    assert!(
        entries(dir).iter().any(|entry| entry.ends_with(section)),
        "id-22 cache entry must carry the exact section"
    );
}

/// One fixture loop checks both sections and the id-22 memo together.
#[test]
#[ignore = "cold gate-fixture baseline; run explicitly"]
fn layer_budget_sections_match_pre_change_cold_and_warm() {
    use crate::fixture_pipeline::{GATE_FIXTURES, load_fixture};
    for &name in GATE_FIXTURES {
        let fx = load_fixture(name);
        let dir = fresh_cache_dir(name);
        let cache = StageCache::new(&dir).unwrap();
        let cfg = LightmapConfig {
            lightmap_density: lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS,
            ..config(false)
        };
        let bake = || {
            run_fused_with_geometry(
                &test_args(),
                Some(&cache),
                &fx.lights,
                None,
                &cfg,
                &BakeControl::unrestricted(),
                &BakeControl::unrestricted(),
                fx.geometry.clone(),
            )
        };
        let cold = bake();
        let id22 = cold.lightmap.section.to_bytes();
        let id42 = cold
            .shadowmask
            .as_ref()
            .map_or_else(Vec::new, ShadowmaskAtlasSection::to_bytes);
        let expected = expected_sections(name);
        assert_eq!(digest(&id22), expected.0, "{name}: cold id 22");
        assert_eq!(digest(&id42), expected.1, "{name}: cold id 42");
        println!(
            "BASELINE {name} layers={} id22={} id42={}",
            cold.lightmap.layer_count,
            digest(&id22),
            digest(&id42)
        );
        let has_blocks = !cold.lightmap.section.blocks.is_empty();
        if has_blocks {
            assert_cache_contains(&dir, &id22);
            assert_eq!(cache.test_access("lightmap_section").writes, 1);
        } else {
            assert_eq!(cache.test_access("lightmap_section").writes, 0);
        }
        cache.clear_test_accesses();
        let warm = bake();
        assert_eq!(
            cache.test_access("lightmap_section").read_hits,
            usize::from(has_blocks)
        );
        assert_eq!(warm.lightmap.section.to_bytes(), id22, "{name}: warm id 22");
        assert_eq!(
            warm.shadowmask
                .as_ref()
                .map_or_else(Vec::new, ShadowmaskAtlasSection::to_bytes),
            id42,
            "{name}: warm id 42"
        );
        drop(cache);
        std::fs::remove_dir_all(dir).unwrap();
    }
    let lights = broad_and_corner();
    let dir = fresh_cache_dir("baseline_four_layer_cut");
    let cache = StageCache::new(&dir).unwrap();
    // Reuse the established 24 m floor and 64-texel pool edge, unchanged.
    let mut geo = horizontal_quads(&[FLOOR]);
    let static_lights = StaticBakedLights::from_lights(&lights);
    let prepared = lightmap_bake::prepare_atlas_within(
        &mut geo,
        &static_lights,
        0.25,
        &[],
        lightmap_bake::BlockOrdering::by_cell_id(2),
        CUT_POOL_EDGE,
        &BakeControl::unrestricted(),
    )
    .unwrap();
    assert_eq!(prepared.layer_count, 4);
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let out = bake_fused_prepared(
        &test_args(),
        Some(&cache),
        &BakeControl::unrestricted(),
        &BakeControl::unrestricted(),
        &mut geo,
        &static_lights,
        &AlphaLightsNs::from_lights(&lights),
        None,
        &[],
        &bvh,
        &prims,
        &config(false),
        prepared,
    )
    .unwrap();
    let id22 = out.lightmap.section.to_bytes();
    let section = out.shadowmask.unwrap();
    let id42 = section.to_bytes();
    // The cache's PRC2 header is 44 bytes, followed by the report prefix.
    let memo = entries(&dir)
        .into_iter()
        .find(|entry| entry.ends_with(&id42))
        .unwrap();
    let prefix_len = memo.len() - id42.len() - 1;
    let report = &memo[44..prefix_len];
    println!(
        "BASELINE four-layer-cut layers=4 id22={} id42={} owners={:?} report={:?} report_digest={}",
        digest(&id22),
        digest(&id42),
        section.owners,
        report,
        digest(report)
    );
    let expected = expected_sections("four-layer-cut");
    assert_eq!(digest(&id22), expected.0, "four-layer cut: cold id 22");
    assert_eq!(digest(&id42), expected.1, "four-layer cut: cold id 42");
    assert_eq!(section.owners, vec![[0, 1, 2, 3]; 4]);
    assert_eq!(
        report,
        [
            5, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0,
            200, 0, 0, 0
        ]
    );
    assert_cache_contains(&dir, &id22);
    drop(cache);
    std::fs::remove_dir_all(dir).unwrap();
}
