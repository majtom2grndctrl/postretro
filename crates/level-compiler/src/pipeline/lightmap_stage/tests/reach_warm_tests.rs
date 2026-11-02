//! Reach cull through a warm stage cache on the two-rooms fixture: warm
//! equals cold, with verify mode re-walking every culled pair past the cache
//! and, off, the stage served from it, with the same bytes at any worker count.
//! The stage line names a cache only when one served the stage.
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use postretro_test_log_capture::CapturedRecord;

use super::super::reach::stage_line;
use super::reach_bake::{ReachRun, reach_bake, reach_bake_with};
use super::*;
use crate::lightmap_reach::test_fixtures::{ReachFixture, Wall, point_light, room_a_centre};

/// The info lines `records` holds that start with `prefix`.
fn lines_with<'r>(records: &'r [CapturedRecord], prefix: &str) -> Vec<&'r str> {
    records
        .iter()
        .filter(|record| record.level == Level::Info && record.message.starts_with(prefix))
        .map(|record| record.message.as_str())
        .collect()
}

/// The stage's reach line, which it logs once.
fn reach_line(records: &[CapturedRecord]) -> &str {
    match lines_with(records, "[Compiler] Lightmap reach cull:").as_slice() {
        [line] => line,
        lines => panic!("one reach line expected: {lines:?}"),
    }
}

// A cold bake with verify on fills the cache. Warm with verify on, the stage
// reads no memo and re-walks every culled pair; warm with it off, the stage
// is served from the `lightmap_section` memo. Both warm bakes equal the
// cold one, at each floor setting.
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
        assert_eq!(
            lines_with(&rewalked.records, "[cache] lightmap_section"),
            ["[cache] lightmap_section bypassed (reach verify)"],
            "verify on: the memo read is bypassed, not missed"
        );
        let served = bake(false);
        assert_eq!(
            (served.counts.baked, served.counts.walked),
            (0, 0),
            "verify off: served from the `lightmap_section` memo"
        );
        assert_eq!(
            reach_line(&served.records),
            "[Compiler] Lightmap reach cull: stage served from the lightmap_section memo, \
             no pair walked",
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

// Every static light resolves through the SDF, so no light bakes a lightmap
// layer and the stage has no partition, cold and with no cache at all: its
// line says no pair was walked and names no cache.
#[test]
fn reach_line_names_no_cache_when_no_light_bakes_a_lightmap_layer() {
    let fixture = ReachFixture::two_rooms(Wall::Solid);
    let lights = [MapLight {
        shadow_type: crate::map_data::ShadowType::Sdf,
        ..point_light(room_a_centre(), 6.0)
    }];
    let bake =
        reach_bake(&fixture, &lights, true, lightmap_layer::ExactSkips::ON, 1).expect("bake");
    assert_eq!((bake.counts.baked, bake.counts.loaded), (0, 0));
    assert_eq!(
        reach_line(&bake.records),
        "[Compiler] Lightmap reach cull: no pair walked, no partition baked or loaded",
    );
}

// Each source a stage that walked no partition can have, one with the cull
// switched off, and a walked stage with partitions from cache.
#[test]
fn reach_line_says_where_a_stage_that_walked_nothing_came_from() {
    let none = ReachCounts {
        walked: 0,
        culled: 0,
        skipped: 0,
        traced_in_culled: 0,
        rewalked: 0,
        baked: 0,
        loaded: 0,
        switched_off: false,
        verify: false,
    };
    let loaded = ReachCounts { loaded: 3, ..none };
    let prefix = "[Compiler] Lightmap reach cull: ";
    for (counts, memo_hit, expected) in [
        (none, false, "no pair walked, no partition baked or loaded"),
        (
            none,
            true,
            "stage served from the lightmap_section memo, no pair walked",
        ),
        (
            loaded,
            true,
            "stage served from the lightmap_section memo and 3 cached partition(s), no pair \
             walked",
        ),
        (
            loaded,
            false,
            "stage served from cache (3 partition(s) loaded), no pair walked",
        ),
        (
            ReachCounts {
                switched_off: true,
                ..none
            },
            false,
            "no pair walked, no partition baked or loaded; switched off for the whole map by \
             coverage-proof holes",
        ),
        (
            ReachCounts {
                walked: 10,
                culled: 4,
                skipped: 7,
                traced_in_culled: 2,
                rewalked: 4,
                baked: 2,
                loaded: 1,
                switched_off: false,
                verify: true,
            },
            false,
            "10 pair(s) walked, 4 culled, 4 re-walked by verify mode; 7 texel(s) skipped, 2 \
             traced inside culled pairs; 2 partition(s) walked this build, 1 from cache",
        ),
    ] {
        assert_eq!(
            stage_line(&counts, memo_hit),
            format!("{prefix}{expected}"),
            "{counts:?}, memo hit {memo_hit}"
        );
    }
}
