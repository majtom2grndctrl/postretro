//! Per-map use records the start-of-build prune spares.
//! See: context/lib/build_pipeline.md §Build Cache (Eviction).
//!
//! Each map has a directory under `records/` in the cache directory, named by
//! a digest of its input path. A build appends every entry it reads or writes
//! to its own journal as it goes, so a killed build's touches persist. A
//! successful build replaces the map's `last-success` record with its own set
//! and deletes the map's journals. The prune spares, for every map, the last
//! success plus every journal written since.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use super::HASH_BYTES;

/// Spared entries: digest → entry size on disk, as recorded when touched.
pub(super) type SparedSet = HashMap<[u8; HASH_BYTES], u64>;

/// Subdirectory of the cache directory. The prune considers only files at the
/// top level, so records are never evicted as entries.
const RECORDS_DIR: &str = "records";
const SUCCESS_FILE: &str = "last-success";
const JOURNAL_PREFIX: &str = "journal-";
/// One record: entry digest, then its size as `u64` little endian. A trailing
/// partial record (a build killed mid-append) is ignored.
const RECORD_BYTES: usize = HASH_BYTES + 8;

/// The directory name for a map: a digest of its input path, made absolute
/// so the same map built from another working directory shares its record.
pub(super) fn map_id(input: &Path) -> String {
    let path = fs::canonicalize(input)
        .or_else(|_| std::path::absolute(input))
        .unwrap_or_else(|_| input.to_path_buf());
    let digest = blake3::hash(path.to_string_lossy().as_bytes());
    super::hex_encode(&digest.as_bytes()[..16])
}

/// Every map's spared entries: its last success plus every journal since.
/// Unreadable records are skipped with a warning; the prune then spares less.
pub(super) fn read_spared(cache_dir: &Path) -> SparedSet {
    let mut spared = SparedSet::new();
    let Ok(maps) = fs::read_dir(cache_dir.join(RECORDS_DIR)) else {
        return spared;
    };
    for map in maps.flatten() {
        let Ok(files) = fs::read_dir(map.path()) else {
            continue;
        };
        for file in files.flatten() {
            let name = file.file_name();
            let name = name.to_string_lossy();
            if name == SUCCESS_FILE || name.starts_with(JOURNAL_PREFIX) {
                if let Err(err) = read_records(&file.path(), &mut spared) {
                    log::warn!(
                        "[cache] cannot read use record {}: {err}",
                        file.path().display()
                    );
                }
            }
        }
    }
    spared
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

/// One build's append-only journal for one map.
pub(super) struct Journal {
    map_dir: PathBuf,
    path: PathBuf,
    /// `None` once a write fails: the build continues unrecorded.
    file: Mutex<Option<fs::File>>,
}

impl Journal {
    /// Start this build's journal in the map's record directory.
    pub(super) fn begin(cache_dir: &Path, map_id: &str) -> io::Result<Self> {
        let map_dir = cache_dir.join(RECORDS_DIR).join(map_id);
        fs::create_dir_all(&map_dir)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let path = map_dir.join(format!("{JOURNAL_PREFIX}{stamp}-{}", std::process::id()));
        let file = fs::OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)?;
        Ok(Self {
            map_dir,
            path,
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
        let staged = self.map_dir.join(format!("{SUCCESS_FILE}.tmp"));
        fs::write(&staged, &bytes)?;
        fs::rename(&staged, self.map_dir.join(SUCCESS_FILE))?;
        for file in fs::read_dir(&self.map_dir)?.flatten() {
            if file
                .file_name()
                .to_string_lossy()
                .starts_with(JOURNAL_PREFIX)
            {
                let _ = fs::remove_file(file.path());
            }
        }
        Ok(())
    }
}
