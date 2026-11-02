//! Reach cull through a warm stage cache on the two-rooms fixture: warm
//! equals cold, with verify mode re-walking every culled pair past the cache
//! and, off, the stage served from it (research P3, P5).
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use super::reach_bake::{ReachRun, reach_bake_with};
use super::*;
use crate::lightmap_reach::test_fixtures::{ReachFixture, Wall, point_light, room_a_centre};

// A cold bake with verify on fills the cache. Warm with verify on, the stage
// reads no memo and re-walks every culled pair; warm with it off, the stage
// is served from the cache. Both warm bakes equal the cold one, at each
// floor setting.
#[test]
fn reach_cull_two_rooms_warm_equals_cold_with_verify_on_and_off() {
    let fixture = ReachFixture::two_rooms(Wall::Solid);
    let lights = [point_light(room_a_centre(), 6.0)];
    for penumbra_floor in [true, false] {
        let dir = fresh_cache_dir("reach_warm");
        let cache = StageCache::new(&dir).expect("create reach cache");
        let bake = |verify: bool| {
            let mut run = ReachRun::new(penumbra_floor, lightmap_layer::ExactSkips::ON, 4);
            run.cache = Some(&cache);
            run.verify = Some(verify);
            let bake = reach_bake_with(&fixture, &lights, &run).expect("bake");
            assert_eq!(bake.counts.verify, verify);
            bake
        };
        let cold = bake(true);
        assert!(
            cold.counts.culled > 0 && cold.counts.baked > 0,
            "{:?}",
            cold.counts
        );
        let rewalked = bake(true);
        assert_eq!(
            (rewalked.counts.loaded, rewalked.counts.culled),
            (0, cold.counts.culled),
            "verify on: no layer is read, every culled pair is re-walked"
        );
        assert_eq!(rewalked.counts.rewalked, rewalked.counts.culled);
        let served = bake(false);
        assert_eq!(
            (served.counts.baked, served.counts.walked),
            (0, 0),
            "verify off: served from cache"
        );
        for (warm, label) in [(&rewalked, "verify on"), (&served, "verify off")] {
            assert!(
                warm.lightmap == cold.lightmap && warm.shadowmask == cold.shadowmask,
                "floor {penumbra_floor}, {label}: warm bytes differ from cold"
            );
        }
        drop(cache);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
