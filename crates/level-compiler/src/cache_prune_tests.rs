//! Build-start prune under the per-map use record, through prl-build's own
//! cache construction (`construct_stage_cache`). Each budget is smaller than
//! the map's set unless a test says otherwise.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::cache::{CacheKey, StageCache};

const PAYLOAD: usize = 1000;

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "prl-build-prune-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn args(map: &Path, cache_dir: &Path, budget: u64) -> crate::Args {
    crate::parse_args_from(
        [
            map.to_string_lossy().into_owned(),
            "--cache-dir".to_owned(),
            cache_dir.to_string_lossy().into_owned(),
            "--cache-max-size".to_owned(),
            budget.to_string(),
        ]
        .into_iter(),
    )
    .expect("prune test arguments parse")
}

/// Start one build of `map` the way prl-build does: open, prune, journal.
fn open(map: &Path, cache_dir: &Path, budget: u64) -> StageCache {
    crate::construct_stage_cache(&args(map, cache_dir, budget)).expect("cache is enabled")
}

fn key(name: &str) -> CacheKey {
    CacheKey::new("prune_test", 1, name.as_bytes())
}

fn exists(cache_dir: &Path, name: &str) -> bool {
    cache_dir.join(key(name).as_filename()).is_file()
}

fn set_age(cache_dir: &Path, name: &str, seconds: u64) {
    fs::OpenOptions::new()
        .write(true)
        .open(cache_dir.join(key(name).as_filename()))
        .expect("open entry to age it")
        .set_modified(SystemTime::now() - Duration::from_secs(seconds))
        .expect("age entry");
}

/// Read each of `reads`, writing any that miss, then write each of `writes`.
/// Returns the read misses.
fn touch(cache: &StageCache, reads: &[&str], writes: &[&str]) -> usize {
    let mut misses = 0;
    for name in reads {
        if cache.get(&key(name)).is_none() {
            misses += 1;
            cache.put(&key(name), &[1; PAYLOAD]);
        }
    }
    for name in writes {
        cache.put(&key(name), &[2; PAYLOAD]);
    }
    misses
}

/// Write an entry no build records, as an older generation would leave.
fn orphan(cache_dir: &Path, name: &str) {
    StageCache::new(cache_dir)
        .expect("open raw cache")
        .put(&key(name), &[3; PAYLOAD]);
}

/// Top-level entry files, by name; records and stage files excluded.
fn entries(cache_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(cache_dir)
        .expect("list cache dir")
        .flatten()
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.ends_with(".tmp"))
        .collect();
    names.sort();
    names
}

fn entries_bytes(cache_dir: &Path) -> u64 {
    entries(cache_dir)
        .iter()
        .map(|name| fs::metadata(cache_dir.join(name)).unwrap().len())
        .sum()
}

// P6: build N spares every entry the previous successful build wrote and every
// entry it only read, including one written before that build began and older
// than everything else; an entry neither touched is evicted.
#[test]
fn prune_spares_previous_success_reads_and_writes() {
    let root = temp_dir("p6");
    let (map, cache) = (root.join("a.map"), root.join("cache"));

    let first = open(&map, &cache, 1);
    touch(&first, &[], &["old", "orphan"]);
    first.finish_successful_build(1);
    set_age(&cache, "old", 3000);
    set_age(&cache, "orphan", 2000);

    let previous = open(&map, &cache, 1);
    assert_eq!(
        touch(&previous, &["old"], &["new"]),
        0,
        "only-read entry hits"
    );
    previous.finish_successful_build(1);

    let _current = open(&map, &cache, 1);
    assert!(exists(&cache, "old"), "the only-read entry must survive");
    assert!(exists(&cache, "new"), "the written entry must survive");
    assert!(
        !exists(&cache, "orphan"),
        "an entry neither touched must go"
    );
    let _ = fs::remove_dir_all(&root);
}

// P7: a build stopped before its end-of-build step still spares everything it
// read or wrote, alongside the last successful build's record.
#[test]
fn prune_spares_stopped_build_touches_and_last_success() {
    let root = temp_dir("p7");
    let (map, cache) = (root.join("a.map"), root.join("cache"));

    let success = open(&map, &cache, 1);
    touch(&success, &[], &["a", "b"]);
    success.finish_successful_build(1);
    orphan(&cache, "d");

    let stopped = open(&map, &cache, 1);
    assert_eq!(touch(&stopped, &["a"], &["c"]), 0);
    drop(stopped);

    let _next = open(&map, &cache, 1);
    for name in ["a", "b", "c"] {
        assert!(exists(&cache, name), "{name} must survive");
    }
    assert!(!exists(&cache, "d"), "the unrecorded entry must go");
    let _ = fs::remove_dir_all(&root);
}

// P9: building another map in between exposes neither map's set.
#[test]
fn interleaved_maps_keep_each_record() {
    let root = temp_dir("p9");
    let (map_a, map_b, cache) = (root.join("a.map"), root.join("b.map"), root.join("cache"));
    let set_a = ["a1", "a2", "a3", "a4"];
    let set_b = ["b1", "b2", "b3", "b4"];

    let build = open(&map_a, &cache, 1);
    assert_eq!(touch(&build, &set_a, &[]), set_a.len());
    build.finish_successful_build(1);
    let build = open(&map_b, &cache, 1);
    assert_eq!(touch(&build, &set_b, &[]), set_b.len());
    build.finish_successful_build(1);

    let build = open(&map_a, &cache, 1);
    assert_eq!(
        touch(&build, &set_a, &[]),
        0,
        "the third build must not miss"
    );
    build.finish_successful_build(1);
    assert!(
        set_b.iter().all(|name| exists(&cache, name)),
        "B's record survives"
    );
    let _ = fs::remove_dir_all(&root);
}

// P10: a build that fails at parse prunes and touches nothing, then fails; the
// next build ends with the same entries as without it, and misses nothing.
#[test]
fn parse_failed_build_leaves_record_unchanged() {
    let set = ["a1", "a2", "a3"];
    let mut survivors = Vec::new();
    for with_failure in [false, true] {
        let root = temp_dir(if with_failure {
            "p10-failed"
        } else {
            "p10-clean"
        });
        let (map, cache) = (root.join("a.map"), root.join("cache"));
        let build = open(&map, &cache, 1);
        touch(&build, &set, &[]);
        build.finish_successful_build(1);
        orphan(&cache, "o1");
        orphan(&cache, "o2");
        set_age(&cache, "o1", 200);
        set_age(&cache, "o2", 100);

        if with_failure {
            let result: anyhow::Result<()> = crate::run_with_failure_cache_report(
                crate::construct_stage_cache(&args(&map, &cache, 1)),
                1,
                |_| anyhow::bail!("parse error"),
            );
            assert!(result.is_err());
        }

        let third = open(&map, &cache, 1);
        assert_eq!(
            touch(&third, &set, &[]),
            0,
            "failure {with_failure}: no misses"
        );
        third.finish_successful_build(1);
        survivors.push(entries(&cache));
        let _ = fs::remove_dir_all(&root);
    }
    assert_eq!(
        survivors[0], survivors[1],
        "a failed build must change nothing"
    );
}

// A project whose live set fits the budget stays at or below it after every
// build-start prune, even as each build orphans the last one's entries.
#[test]
fn orphaning_builds_stay_within_budget() {
    let root = temp_dir("fits");
    let (map, cache) = (root.join("a.map"), root.join("cache"));
    let seed = open(&map, &cache, u64::MAX);
    touch(&seed, &[], &["probe"]);
    let entry_bytes = fs::metadata(cache.join(key("probe").as_filename()))
        .unwrap()
        .len();
    drop(seed);
    let budget = 10 * entry_bytes;

    for generation in 0..6 {
        let build = open(&map, &cache, budget);
        assert!(
            entries_bytes(&cache) <= budget,
            "generation {generation}: {} bytes over budget {budget}",
            entries_bytes(&cache)
        );
        let names: Vec<String> = (0..4).map(|i| format!("gen{generation}-{i}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        touch(&build, &[], &names);
        build.finish_successful_build(budget);
    }
    let _ = fs::remove_dir_all(&root);
}
