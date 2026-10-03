//! Build-start prune under the per-map use records, through prl-build's own cache construction.
//! See: context/lib/build_pipeline.md §Build Cache

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use log::Level;
use postretro_test_log_capture::LogCapture;

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
/// Each test's budget is smaller than the map's set unless it says otherwise.
fn open(map: &Path, cache_dir: &Path, budget: u64) -> StageCache {
    crate::construct_stage_cache(&args(map, cache_dir, budget)).expect("cache is enabled")
}

fn key(name: &str) -> CacheKey {
    CacheKey::new("prune_test", 1, name.as_bytes())
}

fn exists(cache_dir: &Path, name: &str) -> bool {
    cache_dir.join(key(name).as_filename()).is_file()
}

/// More than a day, the grace a missing map's record and record debris get.
const DAY_AND_AN_HOUR: u64 = 25 * 60 * 60;

/// Set a file's mtime `seconds` into the past. Opened for writing: Windows
/// refuses to set a file time through a read-only handle.
fn set_file_age(path: &Path, seconds: u64) {
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open file to age it")
        .set_modified(SystemTime::now() - Duration::from_secs(seconds))
        .expect("age file");
}

fn set_age(cache_dir: &Path, name: &str, seconds: u64) {
    set_file_age(&cache_dir.join(key(name).as_filename()), seconds);
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

// Build N spares every entry the previous successful build wrote and every
// entry it only read, including one written before that build began and older
// than everything else; an entry neither touched is evicted.
#[test]
fn prune_spares_previous_success_reads_and_writes() {
    let root = temp_dir("spares-previous-success");
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

// A build stopped before its end-of-build step still spares everything it
// read or wrote, alongside the last successful build's record.
#[test]
fn prune_spares_stopped_build_touches_and_last_success() {
    let root = temp_dir("spares-stopped-build");
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

// Building another map in between exposes neither map's set.
#[test]
fn interleaved_maps_keep_each_record() {
    let root = temp_dir("interleaved-maps");
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

// A build that fails at parse prunes only unspared entries and records nothing;
// the next build ends with the same entries as without it, and misses nothing.
#[test]
fn parse_failed_build_leaves_record_unchanged() {
    let set = ["a1", "a2", "a3"];
    let mut survivors = Vec::new();
    for with_failure in [false, true] {
        let root = temp_dir(if with_failure {
            "parse-failed"
        } else {
            "parse-clean"
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

/// Create a map file at `root/name`, so its record stores the map's path.
fn map_file(root: &Path, name: &str) -> PathBuf {
    fs::create_dir_all(root).expect("create test root");
    let map = root.join(name);
    fs::write(&map, b"// prune test map").expect("write map file");
    map
}

/// Every map's record directory under `records/`.
fn record_dirs(cache_dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(cache_dir.join("records"))
        .expect("list records")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}

/// The record directory whose stored path is `map`'s; `map` must exist.
fn record_dir_of(cache_dir: &Path, map: &Path) -> PathBuf {
    let canonical = fs::canonicalize(map).expect("canonicalize map path");
    record_dirs(cache_dir)
        .into_iter()
        .find(|dir| {
            fs::read_to_string(dir.join("map-path"))
                .is_ok_and(|stored| Path::new(&stored) == canonical)
        })
        .expect("the map's record stores its path")
}

/// Every file under the records whose name satisfies `matches`.
fn record_files(cache_dir: &Path, matches: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    record_dirs(cache_dir)
        .iter()
        .flat_map(|dir| fs::read_dir(dir).expect("list record").flatten())
        .map(|file| file.path())
        .filter(|path| matches(&path.file_name().unwrap().to_string_lossy()))
        .collect()
}

fn journals(cache_dir: &Path) -> Vec<PathBuf> {
    record_files(cache_dir, |name| {
        name.starts_with("journal-") && !name.ends_with(".lock")
    })
}

fn lock_files(cache_dir: &Path) -> Vec<PathBuf> {
    record_files(cache_dir, |name| name.ends_with(".lock"))
}

fn modified(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .expect("read mtime")
}

// A record whose map file goes missing stays spared through the first prune
// that finds it missing, which marks the record. Once the mark is more than a
// day old, the next prune retires the record and its entries become evictable.
#[test]
fn missing_map_record_retires_only_after_a_day_missing() {
    let root = temp_dir("missing-map-retires");
    let (gone, kept) = (map_file(&root, "gone.map"), map_file(&root, "kept.map"));
    let cache = root.join("cache");

    let build = open(&gone, &cache, 1);
    touch(&build, &[], &["g1", "g2"]);
    build.finish_successful_build(1);
    let gone_record = record_dir_of(&cache, &gone);
    let marker = gone_record.join("missing-since");

    fs::remove_file(&gone).expect("delete map file");
    let capture = LogCapture::start();
    let build = open(&kept, &cache, 1);
    capture.assert_logged_once(Level::Info, "of a use record is missing");
    assert!(
        exists(&cache, "g1") && exists(&cache, "g2"),
        "a map missing for under a day keeps its entries spared"
    );
    assert!(
        marker.is_file(),
        "the first prune to find the map missing marks its record"
    );
    build.finish_successful_build(1);
    capture.assert_not_logged(Level::Warn, "[cache] retiring use record");

    set_file_age(&marker, DAY_AND_AN_HOUR);
    let _next = open(&kept, &cache, 1);
    capture.assert_logged_once(Level::Warn, "[cache] retiring use record for missing map");
    assert!(
        !exists(&cache, "g1") && !exists(&cache, "g2"),
        "a map missing for a day must leave its entries evictable"
    );
    assert!(!gone_record.exists(), "the retired record is deleted");
    assert_eq!(
        record_dirs(&cache).len(),
        1,
        "only the live map's record remains"
    );
    let _ = fs::remove_dir_all(&root);
}

// A map that comes back before its record retires clears the mark, even one
// past the grace, so its entries stay spared and a later absence starts a
// fresh day.
#[test]
fn reappearing_map_clears_missing_mark() {
    let root = temp_dir("missing-map-returns");
    let (map, other) = (map_file(&root, "a.map"), map_file(&root, "other.map"));
    let cache = root.join("cache");

    let build = open(&map, &cache, 1);
    touch(&build, &[], &["a1", "a2"]);
    build.finish_successful_build(1);
    let marker = record_dir_of(&cache, &map).join("missing-since");

    fs::remove_file(&map).expect("delete map file");
    drop(open(&other, &cache, 1));
    assert!(marker.is_file(), "the prune marks the missing map's record");

    map_file(&root, "a.map");
    set_file_age(&marker, DAY_AND_AN_HOUR);
    let _next = open(&other, &cache, 1);
    assert!(!marker.exists(), "a map that is back clears its mark");
    assert!(
        exists(&cache, "a1") && exists(&cache, "a2"),
        "a map that is back keeps its entries spared"
    );
    let _ = fs::remove_dir_all(&root);
}

// The end-of-build budget report only reads the records. It neither retires a
// record whose map has been missing for over a day nor marks one whose map
// just went missing; the next prune does both.
#[test]
fn budget_report_leaves_missing_map_records_unchanged() {
    let root = temp_dir("report-reads-only");
    let (overdue, fresh, kept) = (
        map_file(&root, "overdue.map"),
        map_file(&root, "fresh.map"),
        map_file(&root, "kept.map"),
    );
    let cache = root.join("cache");
    for (map, name) in [(&overdue, "o1"), (&fresh, "f1")] {
        let build = open(map, &cache, u64::MAX);
        touch(&build, &[], &[name]);
        build.finish_successful_build(u64::MAX);
    }
    let (overdue_record, fresh_record) = (
        record_dir_of(&cache, &overdue),
        record_dir_of(&cache, &fresh),
    );
    let overdue_marker = overdue_record.join("missing-since");

    fs::remove_file(&overdue).expect("delete map file");
    let build = open(&kept, &cache, 1);
    set_file_age(&overdue_marker, DAY_AND_AN_HOUR);
    let marked_at = modified(&overdue_marker);
    fs::remove_file(&fresh).expect("delete map file");
    build.finish_successful_build(1);

    assert!(
        overdue_record.join("last-success").is_file(),
        "the report must not retire a record"
    );
    assert_eq!(
        modified(&overdue_marker),
        marked_at,
        "the report must not touch a mark"
    );
    assert!(
        !fresh_record.join("missing-since").exists(),
        "the report must not mark a record"
    );

    let _next = open(&kept, &cache, 1);
    assert!(
        !overdue_record.exists(),
        "the next prune retires the record"
    );
    assert!(
        fresh_record.join("missing-since").is_file(),
        "the next prune marks the record"
    );
    let _ = fs::remove_dir_all(&root);
}

// Record stage files and orphaned journal lock files older than a day are
// debris from killed builds, and the prune deletes them; younger ones may
// belong to a running build and stay.
#[test]
fn prune_deletes_day_old_record_debris_and_keeps_young() {
    let root = temp_dir("record-debris");
    let (map, cache) = (map_file(&root, "a.map"), root.join("cache"));
    open(&map, &cache, u64::MAX).finish_successful_build(u64::MAX);
    let record = record_dir_of(&cache, &map);
    let stale_stage = record.join("last-success.1-1.tmp");
    let stale_lock = record.join("journal-1-1-1.lock");
    let young_stage = record.join("map-path.1-2.tmp");
    for debris in [&stale_stage, &stale_lock, &young_stage] {
        fs::write(debris, b"").expect("write debris");
    }
    set_file_age(&stale_stage, DAY_AND_AN_HOUR);
    set_file_age(&stale_lock, DAY_AND_AN_HOUR);

    let _next = open(&map, &cache, u64::MAX);
    assert!(!stale_stage.exists(), "a day-old stage file is deleted");
    assert!(
        !stale_lock.exists(),
        "a day-old lock file without its journal is deleted"
    );
    assert!(young_stage.exists(), "a young stage file stays");
    let _ = fs::remove_dir_all(&root);
}

// A record directory written before map paths were stored has no path to
// check, so it stays spared even after its map file is gone.
#[test]
fn record_without_stored_map_path_stays_spared() {
    let root = temp_dir("legacy-record");
    let (gone, other) = (map_file(&root, "gone.map"), map_file(&root, "other.map"));
    let cache = root.join("cache");

    let build = open(&gone, &cache, 1);
    touch(&build, &[], &["l1", "l2"]);
    build.finish_successful_build(1);
    let mut removed = 0;
    for dir in record_dirs(&cache) {
        if fs::remove_file(dir.join("map-path")).is_ok() {
            removed += 1;
        }
    }
    assert_eq!(removed, 1, "the build stored its map's path");
    fs::remove_file(&gone).expect("delete map file");

    let _next = open(&other, &cache, 1);
    assert!(
        exists(&cache, "l1") && exists(&cache, "l2"),
        "a record without a stored path must stay spared"
    );
    let _ = fs::remove_dir_all(&root);
}

// When the records directory exists but cannot be listed, no map's set is
// known: the prune warns and evicts nothing, though the cache is over budget.
#[test]
fn unlistable_records_dir_skips_eviction_and_warns() {
    let root = temp_dir("unlistable");
    let (map, cache) = (root.join("a.map"), root.join("cache"));
    let build = open(&map, &cache, 1);
    touch(&build, &[], &["a1"]);
    build.finish_successful_build(1);
    orphan(&cache, "o1");

    // Listing a file as a directory fails with an error other than NotFound.
    fs::remove_dir_all(cache.join("records")).expect("remove records");
    fs::write(cache.join("records"), b"").expect("replace records with a file");
    let capture = LogCapture::start();
    let blocked = open(&map, &cache, 1);

    capture.assert_logged_once(Level::Warn, "[cache] prune skipped eviction");
    blocked.finish_successful_build(1);
    capture.assert_logged_once(
        Level::Warn,
        "[cache] budget report: cannot read use records",
    );
    assert!(exists(&cache, "a1"), "the recorded entry must survive");
    assert!(
        exists(&cache, "o1"),
        "an unrecorded entry must survive while no set is known"
    );
    let _ = fs::remove_dir_all(&root);
}

// A successful build supersedes only the journals that existed when it began.
// A journal a concurrent build of the same map began later keeps sparing that
// build's entries.
#[test]
fn promote_keeps_journal_begun_after_this_build() {
    let root = temp_dir("concurrent");
    let (map, cache) = (root.join("a.map"), root.join("cache"));

    let build = open(&map, &cache, 1);
    touch(&build, &[], &["mine"]);
    let concurrent = open(&map, &cache, u64::MAX);
    touch(&concurrent, &[], &["theirs"]);
    build.finish_successful_build(1);
    drop(concurrent);
    orphan(&cache, "o1");

    let _next = open(&map, &cache, 1);
    assert!(exists(&cache, "mine"), "the promoted set must survive");
    assert!(
        exists(&cache, "theirs"),
        "the concurrent build's journal must survive the promote"
    );
    assert!(!exists(&cache, "o1"), "the unrecorded entry must go");
    let _ = fs::remove_dir_all(&root);
}

// A successful build deletes the journal of an earlier build of the same map
// only once that build has stopped. While the earlier build runs, its journal
// stays readable to the prune, survives the promote, and keeps sparing what
// that build touches afterwards.
#[test]
fn promote_keeps_running_earlier_journal_and_deletes_stopped_one() {
    let root = temp_dir("earlier-journal");
    let (map, cache) = (map_file(&root, "a.map"), root.join("cache"));

    let earlier = open(&map, &cache, u64::MAX);
    touch(&earlier, &[], &["early"]);
    orphan(&cache, "o1");
    let later = open(&map, &cache, 1);
    assert!(
        exists(&cache, "early"),
        "a running build's journal spares its entries"
    );
    assert!(
        !exists(&cache, "o1"),
        "a running build's journal is readable, so the prune still evicts"
    );
    touch(&later, &[], &["late"]);
    later.finish_successful_build(1);
    assert_eq!(
        journals(&cache).len(),
        1,
        "the running earlier build's journal survives the promote"
    );
    touch(&earlier, &[], &["early-after"]);
    orphan(&cache, "o2");
    drop(earlier);

    let next = open(&map, &cache, 1);
    for name in ["early", "early-after", "late"] {
        assert!(exists(&cache, name), "{name} must survive");
    }
    assert!(!exists(&cache, "o2"), "the unrecorded entry must go");
    next.finish_successful_build(1);
    assert!(
        journals(&cache).is_empty(),
        "the stopped earlier build's journal is deleted"
    );
    assert!(
        lock_files(&cache).is_empty(),
        "every journal's lock file goes with it"
    );
    let _ = fs::remove_dir_all(&root);
}

/// A warm build of `map` through the full pipeline, its cache opened as
/// prl-build opens it. Returns the cache handle the build used.
fn bake(map: &Path, cache_dir: &Path, baked_root: &Path, output: &Path) -> StageCache {
    let args = crate::parse_args_from(
        [
            map.to_string_lossy().into_owned(),
            "--cache-dir".to_owned(),
            cache_dir.to_string_lossy().into_owned(),
            "--baked-root".to_owned(),
            baked_root.to_string_lossy().into_owned(),
            "-o".to_owned(),
            output.to_string_lossy().into_owned(),
        ]
        .into_iter(),
    )
    .expect("warm bake arguments parse");
    let cache = crate::construct_stage_cache(&args).expect("cache is enabled");
    let started = Instant::now();
    let reporter: Arc<dyn crate::reporter::Reporter> = Arc::new(
        crate::reporter::PlainReporter::new(started, crate::logger::LogSink::default()),
    );
    crate::pipeline::run(
        &args,
        Some(cache.clone()),
        started,
        reporter,
        Arc::new(crate::governor::Governor::new(1, false)),
    )
    .expect("fixture compiles");
    cache
}

// A warm all-hit bake emits the all-miss bake's bytes with zero misses, and
// the entries it reads, plus the per-light partitions its section-memo hits
// mark as used, cover every entry the all-miss bake wrote, so the record it
// promotes spares them all. Byte equality with the cold reference is the
// fixture digest gate's job.
#[test]
fn warm_all_hit_covers_all_miss_writes_with_zero_misses() {
    let root = temp_dir("all-hit");
    fs::create_dir_all(&root).expect("create test root");
    let map = crate::fixture_pipeline::fixture_path("test_animated_weight_maps_mixed");
    let (cache, baked) = (root.join("cache"), root.join("baked"));
    let (miss_prl, hit_prl) = (root.join("all-miss.prl"), root.join("all-hit.prl"));

    bake(&map, &cache, &baked, &miss_prl);
    // The all-miss bake started from an empty cache, so every entry on disk
    // is one it wrote.
    let written = entries(&cache);
    assert!(!written.is_empty(), "the all-miss bake must write entries");

    let all_hit = bake(&map, &cache, &baked, &hit_prl);
    assert!(
        fs::read(&miss_prl).expect("read all-miss output")
            == fs::read(&hit_prl).expect("read all-hit output"),
        "the all-hit bake must emit the all-miss bake's bytes"
    );
    let accesses = all_hit.test_accesses_by_stage();
    for (stage, access) in &accesses {
        assert_eq!(
            access.read_hits, access.read_attempts,
            "{stage}: every request must hit"
        );
        assert_eq!(access.writes, 0, "{stage}: nothing may re-bake");
    }
    assert!(
        accesses.values().any(|access| access.read_hits > 0),
        "the all-hit bake must read the cache"
    );
    let used = all_hit.test_live_entry_names();
    let uncovered: Vec<&String> = written
        .iter()
        .filter(|name| !used.contains(*name))
        .collect();
    assert!(
        uncovered.is_empty(),
        "{} of {} all-miss entries are outside the all-hit use set: {uncovered:?}",
        uncovered.len(),
        written.len()
    );
    // The in-memory set alone would pass even if the promote failed.
    let promoted = StageCache::test_last_success_entry_names(&cache, &map)
        .expect("read the all-hit build's promoted record");
    let unrecorded: Vec<&String> = written
        .iter()
        .filter(|name| !promoted.contains(*name))
        .collect();
    assert!(
        unrecorded.is_empty(),
        "{} of {} all-miss entries are missing from the promoted record: {unrecorded:?}",
        unrecorded.len(),
        written.len()
    );
    let _ = fs::remove_dir_all(&root);
}
