//! Per-map use records the start-of-build prune spares.
//! See: context/lib/build_pipeline.md §Build Cache (Eviction).
//!
//! Each map has a directory under `records/` in the cache directory, named by
//! a digest of its input path and holding that path. A build appends every
//! entry it reads or writes to its own journal as it goes, so a killed build's
//! touches persist. A successful build replaces the map's `last-success`
//! record with its own set and deletes the journals it supersedes. The prune
//! spares, for every map whose file still exists, the last success plus every
//! journal written since.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{HASH_BYTES, STAGE_COUNTER};

/// Spared entries: digest → entry size on disk, as recorded when touched.
pub(super) type SparedSet = HashMap<[u8; HASH_BYTES], u64>;

/// Subdirectory of the cache directory. The prune considers only files at the
/// top level, so records are never evicted as entries.
const RECORDS_DIR: &str = "records";
const SUCCESS_FILE: &str = "last-success";
const JOURNAL_PREFIX: &str = "journal-";
/// The map's resolved input path, UTF-8. A record directory without one
/// predates stored paths and is always spared.
const MAP_PATH_FILE: &str = "map-path";
/// One record: entry digest, then its size as `u64` little endian. A trailing
/// partial record (a build killed mid-append) is ignored.
const RECORD_BYTES: usize = HASH_BYTES + 8;

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

/// Every live map's spared entries: its last success plus every journal since.
///
/// A missing records directory means no build has recorded anything yet.
/// Any other failure to list the records, or to read one, is an error: the
/// caller cannot know that map's set, so it must not evict this build. A
/// record whose stored map path no longer exists is retired: not spared, and
/// deleted on a best-effort basis.
pub(super) fn read_spared(cache_dir: &Path) -> Result<SparedSet, RecordsUnreadable> {
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
        if map_retired(&map_dir) {
            // A failed delete leaves the record for a later prune to retire.
            let _ = fs::remove_dir_all(&map_dir);
            continue;
        }
        read_map_records(&map_dir, &mut spared)?;
    }
    Ok(spared)
}

/// One map's last success plus its journals.
fn read_map_records(map_dir: &Path, into: &mut SparedSet) -> Result<(), RecordsUnreadable> {
    let files = match fs::read_dir(map_dir) {
        Ok(files) => files,
        // Retired by a concurrent prune since the records were listed.
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(unreadable(map_dir, err)),
    };
    let mut journal_vanished = false;
    for file in files {
        let file = file.map_err(|err| unreadable(map_dir, err))?;
        let name = file.file_name();
        let name = name.to_string_lossy();
        let is_journal = name.starts_with(JOURNAL_PREFIX);
        if name != SUCCESS_FILE && !is_journal {
            continue;
        }
        match read_records(&file.path(), into) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => journal_vanished |= is_journal,
            Err(err) => return Err(unreadable(&file.path(), err)),
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

/// Whether a record directory's map is gone. Only a stored path that
/// definitely no longer exists retires a record. A directory without one,
/// written before paths were stored or by a build whose input did not exist,
/// stays spared, as does one whose path cannot be checked.
fn map_retired(map_dir: &Path) -> bool {
    let Ok(stored) = fs::read_to_string(map_dir.join(MAP_PATH_FILE)) else {
        return false;
    };
    !stored.is_empty() && matches!(Path::new(&stored).try_exists(), Ok(false))
}

fn read_records(path: &Path, into: &mut SparedSet) -> io::Result<()> {
    let mut bytes = Vec::new();
    fs::File::open(path)?.read_to_end(&mut bytes)?;
    for record in bytes.chunks_exact(RECORD_BYTES) {
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
        .filter(|file| {
            file.file_name()
                .to_string_lossy()
                .starts_with(JOURNAL_PREFIX)
        })
        .map(|file| file.path())
        .collect()
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
        let file = fs::OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)?;
        Ok(Self {
            map_dir,
            path,
            superseded,
            file: Mutex::new(Some(file)),
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

    /// A successful build's set becomes the map's record; the journals it
    /// supersedes, this one included, are deleted.
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
        for journal in self.superseded.iter().chain(std::iter::once(&self.path)) {
            let _ = fs::remove_file(journal);
        }
        Ok(())
    }
}
