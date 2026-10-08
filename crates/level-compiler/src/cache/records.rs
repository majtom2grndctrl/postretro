//! Per-map use records the start-of-build prune spares.
//! See: context/lib/build_pipeline.md §Build Cache (Eviction).
//!
//! Each map has a directory under `records/` in the cache directory, named by
//! a digest of its input path and holding that path. A build appends every
//! entry it reads or writes to its own journal as it goes, so a killed build's
//! touches persist, and holds a lock on the journal's sidecar lock file while
//! it runs. A successful build replaces the map's `last-success` record with
//! its own set and deletes the journals it supersedes that are no longer live.
//! The prune spares, for every live map, the last success plus every journal
//! written since. A map whose file stays missing for a day retires.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{HASH_BYTES, STAGE_COUNTER, STALE_STAGE_AGE};

/// Spared entries: digest → entry size on disk, as recorded when touched.
pub(super) type SparedSet = HashMap<[u8; HASH_BYTES], u64>;

/// Subdirectory of the cache directory. The prune considers only files at the
/// top level, so records are never evicted as entries.
const RECORDS_DIR: &str = "records";
const SUCCESS_FILE: &str = "last-success";
const JOURNAL_PREFIX: &str = "journal-";
/// Suffix of a journal's sidecar lock file, which its build holds locked while
/// it runs.
const LOCK_SUFFIX: &str = ".lock";
/// The map's resolved input path, UTF-8. A record directory without one
/// predates stored paths and is always spared.
const MAP_PATH_FILE: &str = "map-path";
/// Empty marker whose mtime is when a prune first found the map path missing.
const MISSING_SINCE_FILE: &str = "missing-since";
/// How long a map path must stay missing before its record retires. A map
/// can vanish and come back: an editor's delete-then-rename save, a branch
/// switch, an unplugged drive or network share (which reads as missing, not
/// as an error). Retiring on first sight would evict a set that can take
/// hours to re-bake; a day of grace costs only disk.
const MISSING_MAP_GRACE: Duration = Duration::from_secs(24 * 60 * 60);
/// One record: entry digest, then its size as `u64` little endian. A trailing
/// partial record (a build killed mid-append) is ignored.
const RECORD_BYTES: usize = HASH_BYTES + 8;

/// Which read of the records is running.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// The start-of-build prune: marks, retires, and sweeps as it reads.
    Prune,
    /// The end-of-build budget report: reads only.
    Report,
}

/// The records could not be read, so no map's set is known and the prune
/// must not evict on a guess.
pub(super) struct RecordsUnreadable {
    path: PathBuf,
    error: io::Error,
}

impl fmt::Display for RecordsUnreadable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot read use records {}: {}",
            self.path.display(),
            self.error
        )
    }
}

fn unreadable(path: &Path, error: io::Error) -> RecordsUnreadable {
    RecordsUnreadable {
        path: path.to_path_buf(),
        error,
    }
}

/// The path a map's record is named for, made absolute so the same map built
/// from another working directory shares its record. Canonical when the map
/// file exists, which the returned flag reports.
fn resolve_map_path(input: &Path) -> (PathBuf, bool) {
    match fs::canonicalize(input) {
        Ok(path) => (path, true),
        Err(_) => (
            std::path::absolute(input).unwrap_or_else(|_| input.to_path_buf()),
            false,
        ),
    }
}

/// The directory name for a map: a digest of its resolved input path.
fn map_id(map_path: &Path) -> String {
    let digest = blake3::hash(map_path.to_string_lossy().as_bytes());
    super::hex_encode(&digest.as_bytes()[..16])
}

fn is_journal(name: &str) -> bool {
    name.starts_with(JOURNAL_PREFIX) && !name.ends_with(LOCK_SUFFIX)
}

fn lock_path(journal: &Path) -> PathBuf {
    let mut path = OsString::from(journal.as_os_str());
    path.push(LOCK_SUFFIX);
    PathBuf::from(path)
}

/// Whether `path` was last modified more than `age` ago. A file whose time
/// cannot be read counts as young, so it is left alone.
fn older_than(path: &Path, age: Duration) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| {
            SystemTime::now()
                .duration_since(modified)
                .is_ok_and(|elapsed| elapsed > age)
        })
}

/// The start-of-build prune's read: every live map's spared entries.
///
/// It also maintains the records. A map first found missing is marked and
/// stays spared; one missing for [`MISSING_MAP_GRACE`] is retired (not
/// spared, and deleted on a best-effort basis); one that is back loses its
/// mark. Stage files older than a day, and lock files whose journal is gone,
/// are debris from killed builds and are deleted.
pub(super) fn read_spared_for_prune(cache_dir: &Path) -> Result<SparedSet, RecordsUnreadable> {
    read_spared(cache_dir, Pass::Prune)
}

/// The entries the next prune would spare, read without changing anything on
/// disk. A record the next prune would retire is left out. Telling whether a
/// journal's build still runs takes its lock for an instant, nothing more.
pub(super) fn read_spared_for_report(cache_dir: &Path) -> Result<SparedSet, RecordsUnreadable> {
    read_spared(cache_dir, Pass::Report)
}

/// Every live map's spared entries: its last success plus every journal since.
///
/// A missing records directory means no build has recorded anything yet.
/// Any other failure to list the records, or to read one, is an error: the
/// caller cannot know that map's set, so it must not evict this build.
fn read_spared(cache_dir: &Path, pass: Pass) -> Result<SparedSet, RecordsUnreadable> {
    let records_dir = cache_dir.join(RECORDS_DIR);
    let mut spared = SparedSet::new();
    let maps = match fs::read_dir(&records_dir) {
        Ok(maps) => maps,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(spared),
        Err(err) => return Err(unreadable(&records_dir, err)),
    };
    for map in maps {
        let map = map.map_err(|err| unreadable(&records_dir, err))?;
        match map.file_type() {
            Ok(file_type) if !file_type.is_dir() => continue,
            Ok(_) => {}
            Err(err) => return Err(unreadable(&map.path(), err)),
        }
        let map_dir = map.path();
        if record_kept(&map_dir, pass) {
            read_map_records(&map_dir, pass, &mut spared)?;
        }
    }
    Ok(spared)
}

/// One map's last success plus its journals. The prune pass also deletes
/// stale debris it meets.
fn read_map_records(
    map_dir: &Path,
    pass: Pass,
    into: &mut SparedSet,
) -> Result<(), RecordsUnreadable> {
    let files = match fs::read_dir(map_dir) {
        Ok(files) => files,
        // Retired by a concurrent prune since the records were listed.
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(unreadable(map_dir, err)),
    };
    let mut journal_vanished = false;
    for file in files {
        let file = file.map_err(|err| unreadable(map_dir, err))?;
        let path = file.path();
        let name = file.file_name();
        let name = name.to_string_lossy();
        if pass == Pass::Prune && is_debris(&path, &name) {
            // Best effort: a failed delete is retried by a later prune.
            let _ = fs::remove_file(&path);
            continue;
        }
        let journal = is_journal(&name);
        if name != SUCCESS_FILE && !journal {
            continue;
        }
        match read_records(&path, into) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => journal_vanished |= journal,
            Err(err) => return Err(unreadable(&path, err)),
        }
    }
    // A successful build of this map deleted a journal since the listing. It
    // renamed its record into place first, so the record holds that build's
    // set; read it again in case the listing reached the old one first.
    if journal_vanished {
        let success = map_dir.join(SUCCESS_FILE);
        match read_records(&success, into) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(unreadable(&success, err)),
        }
    }
    Ok(())
}

/// A record-file stage (`last-success.<pid>-<n>.tmp`, `map-path.<pid>-<n>.tmp`)
/// or a journal's lock file whose journal is gone, older than a day: left by a
/// build killed between creating it and renaming or deleting it. A younger
/// one may belong to a running build.
fn is_debris(path: &Path, name: &str) -> bool {
    let orphan = if name.ends_with(".tmp") {
        true
    } else if let Some(journal) = name.strip_suffix(LOCK_SUFFIX) {
        // Only a journal that definitely no longer exists orphans its lock: a
        // failed check must not delete a long-running build's lock file.
        name.starts_with(JOURNAL_PREFIX)
            && matches!(path.with_file_name(journal).try_exists(), Ok(false))
    } else {
        false
    };
    orphan && older_than(path, STALE_STAGE_AGE)
}

/// Whether a record still spares its entries. Only a stored path that
/// definitely does not exist counts as missing. A directory without one,
/// written before paths were stored or by a build whose input did not exist,
/// is always kept, as is one whose path cannot be checked. The marker clears
/// only when a prune sees the map present, so an absence that a prune never
/// observed ending counts from the first prune that saw it.
fn record_kept(map_dir: &Path, pass: Pass) -> bool {
    let Ok(stored) = fs::read_to_string(map_dir.join(MAP_PATH_FILE)) else {
        return true;
    };
    if stored.is_empty() {
        return true;
    }
    let marker = map_dir.join(MISSING_SINCE_FILE);
    match Path::new(&stored).try_exists() {
        Ok(true) => {
            if pass == Pass::Prune && fs::remove_file(&marker).is_ok() {
                log::info!(
                    "[cache] map {stored} is back; its use record no longer counts down to retirement"
                );
            }
            true
        }
        Ok(false) => missing_map_record_kept(map_dir, &stored, &marker, pass),
        Err(_) => true,
    }
}

/// Whether the record of a map whose file is missing is still spared: until
/// the map has been missing for [`MISSING_MAP_GRACE`], dated by the marker the
/// prune writes on first sight. The prune retires one past it.
fn missing_map_record_kept(map_dir: &Path, stored: &str, marker: &Path, pass: Pass) -> bool {
    let since = match fs::metadata(marker).and_then(|metadata| metadata.modified()) {
        Ok(since) => since,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            // A failed marker write leaves the next prune to try again; until
            // one succeeds, the record stays spared.
            if pass == Pass::Prune
                && fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(marker)
                    .is_ok()
            {
                log::info!(
                    "[cache] map {stored} of a use record is missing; its entries stay spared until it has been missing for 24 hours"
                );
            }
            return true;
        }
        // A marker that cannot be dated cannot start the countdown.
        Err(_) => return true,
    };
    let missing_long_enough = SystemTime::now()
        .duration_since(since)
        .is_ok_and(|elapsed| elapsed >= MISSING_MAP_GRACE);
    // A build of the map still running, however long the file has been gone,
    // keeps its record.
    if !missing_long_enough || has_live_journal(map_dir) {
        return true;
    }
    if pass == Pass::Prune {
        log::warn!(
            "[cache] retiring use record for missing map {stored}; its entries are no longer spared"
        );
        retire(map_dir);
    }
    false
}

/// Delete a retired record, best effort. The stored path and marker go last:
/// a partial failure leaves a record that still retires, never one that has
/// lost its path and would be spared forever.
fn retire(map_dir: &Path) {
    let Ok(files) = fs::read_dir(map_dir) else {
        return;
    };
    let mut cleared = true;
    for file in files.flatten() {
        let name = file.file_name();
        if name == MAP_PATH_FILE || name == MISSING_SINCE_FILE {
            continue;
        }
        cleared &= fs::remove_file(file.path()).is_ok();
    }
    if cleared {
        let _ = fs::remove_dir_all(map_dir);
    }
}

fn has_live_journal(map_dir: &Path) -> bool {
    // Unlike superseding, an unknown here must keep the record: a directory
    // that cannot be listed may hold a running build's journal.
    let Ok(files) = fs::read_dir(map_dir) else {
        return true;
    };
    files.into_iter().any(|file| match file {
        Ok(file) => {
            is_journal(&file.file_name().to_string_lossy())
                && journal_stopped(&file.path()) != Some(true)
        }
        Err(_) => true,
    })
}

/// Whether the build that owns `journal` has stopped; `None` when that cannot
/// be told.
///
/// A build holds an exclusive lock on its journal's sidecar lock file until
/// the handle closes, which the OS does when the process exits or is killed.
/// The lock is on a sidecar, not the journal, because Windows locks are
/// mandatory: a lock on the journal would make every other build's read of
/// it fail, and the prune would skip eviction whenever a build was running.
fn journal_stopped(journal: &Path) -> Option<bool> {
    // Read-write: file systems that emulate `flock` with a whole-file `fcntl`
    // lock (NFS) grant an exclusive lock only through a writable handle, so a
    // read-only probe would see a live build's lock as unsupported.
    match fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path(journal))
    {
        Ok(lock) => match lock.try_lock() {
            // Released when `lock` closes; nothing re-locks a stopped build's file.
            Ok(()) => Some(true),
            Err(fs::TryLockError::WouldBlock) => Some(false),
            // The file system cannot lock, so its builds hold no locks either;
            // keeping their journals would spare stale entries forever.
            Err(fs::TryLockError::Error(_)) => Some(true),
        },
        // No lock file: a journal written before locks, or whose build could
        // not lock.
        Err(err) if err.kind() == io::ErrorKind::NotFound => Some(true),
        Err(_) => None,
    }
}

fn read_records(path: &Path, into: &mut SparedSet) -> io::Result<()> {
    let mut bytes = Vec::new();
    fs::File::open(path)?.read_to_end(&mut bytes)?;
    for record in bytes.as_chunks::<RECORD_BYTES>().0 {
        let digest: [u8; HASH_BYTES] = record[..HASH_BYTES].try_into().expect("digest width");
        let size = u64::from_le_bytes(record[HASH_BYTES..].try_into().expect("size width"));
        into.insert(digest, size);
    }
    Ok(())
}

fn encode(digest: &[u8; HASH_BYTES], size: u64) -> [u8; RECORD_BYTES] {
    let mut record = [0u8; RECORD_BYTES];
    record[..HASH_BYTES].copy_from_slice(digest);
    record[HASH_BYTES..].copy_from_slice(&size.to_le_bytes());
    record
}

/// A per-writer stage name for a record file, unique like an entry's stage,
/// so concurrent builds of one map never share a temp file.
fn stage_path(map_dir: &Path, file: &str) -> PathBuf {
    let stage_id = STAGE_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
    map_dir.join(format!("{file}.{}-{stage_id}.tmp", std::process::id()))
}

/// Stage `bytes`, sync them, and rename them into place as `file`. Unlike an
/// entry, a torn record fails silently (it spares less, or retires its map),
/// so it is synced; record writes are rare, at most one per build.
fn publish_synced(map_dir: &Path, file: &str, bytes: &[u8]) -> io::Result<()> {
    let staged = stage_path(map_dir, file);
    let result = (|| -> io::Result<()> {
        let mut staged_file = fs::File::create(&staged)?;
        staged_file.write_all(bytes)?;
        staged_file.sync_all()?;
        drop(staged_file);
        fs::rename(&staged, map_dir.join(file))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result
}

/// Every journal in the map's directory. A listing failure yields none: the
/// journals are then left for a later successful build to supersede.
fn list_journals(map_dir: &Path) -> Vec<PathBuf> {
    let Ok(files) = fs::read_dir(map_dir) else {
        return Vec::new();
    };
    files
        .flatten()
        .filter(|file| is_journal(&file.file_name().to_string_lossy()))
        .map(|file| file.path())
        .collect()
}

/// Create and lock `journal`'s sidecar lock file. `None` when either fails:
/// the build then runs with its journal unmarked, which a concurrent
/// successful build of the map may delete.
fn lock_journal(journal: &Path) -> Option<fs::File> {
    let path = lock_path(journal);
    let locked = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|lock| match lock.try_lock() {
            Ok(()) => Ok(lock),
            Err(fs::TryLockError::WouldBlock) => Err(io::Error::from(io::ErrorKind::WouldBlock)),
            Err(fs::TryLockError::Error(err)) => Err(err),
        });
    match locked {
        Ok(lock) => Some(lock),
        Err(err) => {
            log::info!(
                "[cache] cannot lock use journal {} ({err}); a concurrent successful build of this map may delete it",
                journal.display()
            );
            None
        }
    }
}

/// One build's append-only journal for one map.
pub(super) struct Journal {
    map_dir: PathBuf,
    path: PathBuf,
    /// Journals already in the map's directory when this build began: the
    /// stopped or earlier builds its success supersedes. A journal begun later
    /// belongs to a concurrent build of the map and is left alone.
    superseded: Vec<PathBuf>,
    /// `None` once a write fails: the build continues unrecorded.
    file: Mutex<Option<fs::File>>,
    /// The locked sidecar that marks this journal live; `None` once released.
    lock: Mutex<Option<fs::File>>,
}

impl Journal {
    /// Start this build's journal in the record directory of the map at
    /// `input`, storing the map's path there if it is not yet stored.
    pub(super) fn begin(cache_dir: &Path, input: &Path) -> io::Result<Self> {
        let (map_path, map_exists) = resolve_map_path(input);
        let map_dir = cache_dir.join(RECORDS_DIR).join(map_id(&map_path));
        fs::create_dir_all(&map_dir)?;
        // The path is stored only when the map file exists, so it is the
        // canonical one. A build whose input does not exist fails at parse;
        // its record, like one written before paths were stored, has no path
        // and is never retired. A path that is not UTF-8 is not stored either.
        if map_exists
            && let Some(utf8) = map_path.to_str()
            && !map_dir.join(MAP_PATH_FILE).exists()
            && let Err(err) = publish_synced(&map_dir, MAP_PATH_FILE, utf8.as_bytes())
        {
            log::warn!(
                "[cache] cannot store the map path in {} ({err}); its record will not retire with the map",
                map_dir.display()
            );
        }
        let superseded = list_journals(&map_dir);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let path = map_dir.join(format!(
            "{JOURNAL_PREFIX}{stamp}-{}-{}",
            std::process::id(),
            STAGE_COUNTER.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        // Locked before the journal exists, so no build ever sees it unlocked
        // while this one runs.
        let lock = lock_journal(&path);
        let file = match fs::OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(err) => {
                if lock.is_some() {
                    drop(lock);
                    let _ = fs::remove_file(lock_path(&path));
                }
                return Err(err);
            }
        };
        Ok(Self {
            map_dir,
            path,
            superseded,
            file: Mutex::new(Some(file)),
            lock: Mutex::new(lock),
        })
    }

    /// Record one touched entry. Unbuffered, so the record outlives a kill.
    pub(super) fn append(&self, digest: &[u8; HASH_BYTES], size: u64) {
        let mut file = self
            .file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(open) = file.as_mut() else {
            return;
        };
        if let Err(err) = open.write_all(&encode(digest, size)) {
            log::warn!(
                "[cache] use journal {} failed ({err}); this build's entries are not recorded",
                self.path.display()
            );
            *file = None;
        }
    }

    /// A successful build's set becomes the map's record. Its own journal and
    /// every superseded journal whose build has stopped are deleted; a journal
    /// of an earlier build still running is kept, since that build still
    /// appends to it.
    pub(super) fn promote(&self, entries: &SparedSet) -> io::Result<()> {
        *self
            .file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let mut bytes = Vec::with_capacity(entries.len() * RECORD_BYTES);
        let mut sorted: Vec<_> = entries.iter().collect();
        sorted.sort_unstable();
        for (digest, &size) in sorted {
            bytes.extend_from_slice(&encode(digest, size));
        }
        publish_synced(&self.map_dir, SUCCESS_FILE, &bytes)?;
        for journal in &self.superseded {
            if journal_stopped(journal) == Some(true) {
                let _ = fs::remove_file(journal);
                let _ = fs::remove_file(lock_path(journal));
            }
        }
        // Released before the delete, so no handle keeps the lock file alive.
        *self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(lock_path(&self.path));
        Ok(())
    }
}

/// The map at `input`'s `last-success` record alone, without its journals.
#[cfg(test)]
pub(super) fn read_last_success(cache_dir: &Path, input: &Path) -> io::Result<SparedSet> {
    let (map_path, _) = resolve_map_path(input);
    let mut entries = SparedSet::new();
    read_records(
        &cache_dir
            .join(RECORDS_DIR)
            .join(map_id(&map_path))
            .join(SUCCESS_FILE),
        &mut entries,
    )?;
    Ok(entries)
}
