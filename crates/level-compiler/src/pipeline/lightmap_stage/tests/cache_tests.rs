use super::*;

#[test]
fn cold_lightmap_cache_streams_once_warm_and_no_cache_write_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let cache = StageCache::new(directory.path()).unwrap();
    let args = test_args();
    let lights = [point_light(DVec3::new(0.5, 2.0, 0.5), [1.0; 3])];
    let run = |args: &Args, stage_cache| {
        run_fused_with_geometry(
            args,
            stage_cache,
            &lights,
            None,
            &config(true),
            &BakeControl::unrestricted(),
            &BakeControl::unrestricted(),
            two_layer_geometry(),
        )
    };
    let cold = run(&args, Some(&cache));
    let section = &cold.lightmap.section;
    assert!(section.blocks.len() > 1);
    let access = cache.test_access("lightmap_section");
    assert_eq!(access.writes, 1);
    assert_eq!(access.streamed_writes, 1);
    let bound = section.index_bytes().len().max(
        section
            .blocks
            .iter()
            .map(|block| block.irradiance.len() + block.direction.len())
            .max()
            .unwrap(),
    );
    assert!(access.max_streamed_write_bytes <= bound);
    assert!(
        bound < section.byte_len(),
        "fixture distinguishes a whole-section write"
    );

    cache.clear_test_accesses();
    let warm = run(&args, Some(&cache));
    assert_eq!(warm.lightmap.section.to_bytes(), section.to_bytes());
    assert_eq!(cache.test_access("lightmap_section").read_hits, 1);
    assert_eq!(cache.test_access("lightmap_section").writes, 0);
    assert_eq!(cache.test_access("lightmap_section").streamed_writes, 0);

    cache.clear_test_accesses();
    let mut uncached_args = crate::parse_args_from(
        ["fixture.map", "--no-cache", "--soft-shadow-samples", "4"]
            .into_iter()
            .map(str::to_owned),
    )
    .unwrap();
    let uncached_directory = directory.path().join("must-not-exist");
    uncached_args.cache_dir = Some(uncached_directory.clone());
    let uncached_cache = crate::construct_stage_cache(&uncached_args);
    assert!(uncached_cache.is_none());
    let uncached = run(&uncached_args, uncached_cache.as_ref());
    assert!(!uncached_directory.exists());
    assert_eq!(uncached.lightmap.section.to_bytes(), section.to_bytes());
    assert_eq!(cache.test_access("lightmap_section").writes, 0);
    assert_eq!(cache.test_access("lightmap_section").streamed_writes, 0);
}
