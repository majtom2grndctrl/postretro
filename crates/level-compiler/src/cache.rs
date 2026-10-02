// Disk-backed content-hash cache for expensive compile stages.
// See: context/lib/build_pipeline.md

use std::fs;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use std::{collections::HashMap, fmt};

mod records;

use records::{Journal, SparedSet};

/// Default size budget for the on-disk stage cache, in bytes (2 GiB). The
/// start-of-build prune spares every map's use record (the entries its last
/// successful build read or wrote, plus every entry later builds of it read or
/// wrote) and evicts everything else, oldest first, down to this budget unless
/// `--cache-max-size` overrides it. Content addressing never reclaims orphaned
/// generations on its own, so this bound is what stops the cache from growing
/// without limit; the cache can still exceed it by the spared set.
pub const DEFAULT_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Cache-entry format marker. Entries without this marker predate the 64-bit
/// payload length and are treated as cache misses.
const ENTRY_MAGIC: [u8; 4] = *b"PRC2";
/// Length-of-payload prefix (u64 little endian) preceding the integrity hash.
const LENGTH_PREFIX_BYTES: usize = 8;
/// blake3 digest length.
const HASH_BYTES: usize = 32;
/// Combined header size in front of the payload on disk.
const HEADER_BYTES: usize = ENTRY_MAGIC.len() + LENGTH_PREFIX_BYTES + HASH_BYTES;
/// Write buffer for streamed entries, so a payload streamed in small pieces
/// reaches the file in few writes.
const STREAM_BUFFER_BYTES: usize = 64 * 1024;

/// Identifier for a single cache entry. Hashes `(stage_id, stage_version,
/// input_hash)` so unrelated stages and incompatible bakers never collide on
/// the same filename.
pub struct CacheKey {
    digest: [u8; HASH_BYTES],
    #[cfg(test)]
    stage_id: String,
}

impl CacheKey {
    /// Build a key from a stage identifier, the baker version, and the
    /// caller-computed input hash. The input hash is whatever fingerprint the
    /// stage chose for its inputs; this builder just folds it into the final
    /// filename digest.
    pub fn new(stage_id: &str, stage_version: u32, input_hash: &[u8]) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(stage_id.as_bytes());
        hasher.update(&stage_version.to_le_bytes());
        hasher.update(input_hash);
        let digest = hasher.finalize();
        Self {
            digest: *digest.as_bytes(),
            #[cfg(test)]
            stage_id: stage_id.to_owned(),
        }
    }

    /// Hex-encoded blake3 digest, used as the on-disk filename.
    pub fn as_filename(&self) -> String {
        hex_encode(&self.digest)
    }
}

/// Directory-backed cache. `put` writes atomically; `get` validates the
/// format marker, length prefix, and blake3 digest before returning the payload.
#[derive(Clone)]
pub struct StageCache {
    dir: Arc<PathBuf>,
    live_entries: Arc<Mutex<HashMap<[u8; HASH_BYTES], u64>>>,
    live_set_reported: Arc<AtomicBool>,
    /// This build's use journal, when the cache was opened for a map.
    journal: Option<Arc<Journal>>,
    #[cfg(test)]
    test_accesses: Arc<Mutex<HashMap<String, CacheTestAccess>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheLiveSet {
    pub entry_count: usize,
    pub total_bytes: u64,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CacheTestAccess {
    pub read_attempts: usize,
    pub read_hits: usize,
    pub writes: usize,
}

impl StageCache {
    /// Open (or create) a cache directory. Any I/O error from `create_dir_all`
    /// surfaces — the caller decides whether to disable caching for the run.
    pub fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        let dir = path.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        Ok(Self {
            dir: Arc::new(dir),
            live_entries: Arc::new(Mutex::new(HashMap::new())),
            live_set_reported: Arc::new(AtomicBool::new(false)),
            journal: None,
            #[cfg(test)]
            test_accesses: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Open the cache for one build of the map at `input`: prune to `max_bytes`
    /// while sparing every map's use record, then start this build's journal.
    /// The prune reads the records before this build records anything, so it
    /// spares the map's last success and any stopped build since.
    pub fn open_for_build(
        path: impl AsRef<Path>,
        input: &Path,
        max_bytes: u64,
    ) -> io::Result<Self> {
        let mut cache = Self::new(path)?;
        cache.prune_to_budget(max_bytes);
        match Journal::begin(cache.dir.as_path(), &records::map_id(input)) {
            Ok(journal) => cache.journal = Some(Arc::new(journal)),
            Err(err) => log::warn!(
                "[cache] cannot start a use journal in {} ({err}); the next prune may evict this build's entries",
                cache.dir.display()
            ),
        }
        Ok(cache)
    }

    /// Load and validate an entry. Missing entries return `None` silently.
    /// Corrupted entries (short read, length mismatch, hash mismatch) log a
    /// warning and return `None` so the stage falls through to a rebuild.
    ///
    /// A hit is recorded in this build's use journal, which is what keeps a
    /// long-stable entry (hit every build, never rewritten) from eviction; the
    /// read takes one open and never touches the entry's mtime.
    pub fn get(&self, key: &CacheKey) -> Option<Vec<u8>> {
        #[cfg(test)]
        self.record_test_read_attempt(key);
        let path = self.entry_path(key);
        let mut file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
            Err(err) => {
                log::warn!("[cache] failed to open {}: {err}", path.display());
                return None;
            }
        };

        let mut header = [0u8; HEADER_BYTES];
        if let Err(err) = file.read_exact(&mut header) {
            log::warn!("[cache] entry {} header read failed: {err}", path.display());
            return None;
        }

        if header[..ENTRY_MAGIC.len()] != ENTRY_MAGIC {
            log::warn!(
                "[cache] entry {} uses an obsolete header, rebuilding",
                path.display()
            );
            return None;
        }

        let length_start = ENTRY_MAGIC.len();
        let length_end = length_start + LENGTH_PREFIX_BYTES;
        let declared_len = u64::from_le_bytes(
            header[length_start..length_end]
                .try_into()
                .expect("header length slice is exactly u64 wide"),
        );
        let stored_hash: [u8; HASH_BYTES] = header[length_end..]
            .try_into()
            .expect("header slice is exactly HASH_BYTES wide");

        let actual_len = match file.metadata() {
            Ok(metadata) => metadata.len().saturating_sub(HEADER_BYTES as u64),
            Err(err) => {
                log::warn!(
                    "[cache] entry {} metadata read failed: {err}",
                    path.display()
                );
                return None;
            }
        };
        if actual_len != declared_len {
            log::warn!(
                "[cache] entry {} length mismatch: header={declared_len} actual={actual_len}",
                path.display(),
            );
            return None;
        }
        let declared_len = match usize::try_from(declared_len) {
            Ok(length) => length,
            Err(_) => {
                log::warn!(
                    "[cache] entry {} length exceeds this platform's address space",
                    path.display()
                );
                return None;
            }
        };

        let mut payload = Vec::with_capacity(declared_len);
        if let Err(err) = file.read_to_end(&mut payload) {
            log::warn!(
                "[cache] entry {} payload read failed: {err}",
                path.display()
            );
            return None;
        }

        if payload.len() != declared_len {
            log::warn!(
                "[cache] entry {} length mismatch: header={declared_len} actual={}",
                path.display(),
                payload.len()
            );
            return None;
        }

        let computed_hash = blake3::hash(&payload);
        if computed_hash.as_bytes() != &stored_hash {
            log::warn!("[cache] entry {} hash mismatch, ignoring", path.display());
            return None;
        }

        self.record_live_entry(key, HEADER_BYTES as u64 + declared_len as u64);
        #[cfg(test)]
        self.record_test_read_hit(key);
        Some(payload)
    }

    /// Write an entry atomically. Best-effort: any error is logged and
    /// swallowed so a flaky cache directory cannot break a build.
    ///
    /// The entry is staged to `<digest>.tmp` and renamed into place, without
    /// a sync: a killed or torn write leaves a temp file or a hash mismatch,
    /// so the next build misses rather than hits wrong.
    pub fn put(&self, key: &CacheKey, bytes: &[u8]) {
        self.publish(key, bytes.len() as u64, |tmp_path| {
            let header = entry_header(bytes.len() as u64, blake3::hash(bytes));
            let mut file = fs::File::create(tmp_path)?;
            file.write_all(&header)?;
            file.write_all(bytes)
        });
    }

    /// Write an entry atomically without requiring one contiguous payload.
    ///
    /// `write_payload` streams exactly `payload_len` bytes into the staged
    /// entry. The cache computes the same payload hash as [`Self::put`], then
    /// patches it into the reserved header before publishing.
    /// Length mismatches and I/O failures follow `put`'s best-effort logging
    /// and cleanup behavior.
    pub fn put_streamed(
        &self,
        key: &CacheKey,
        payload_len: u64,
        write_payload: impl FnOnce(&mut dyn Write) -> io::Result<()>,
    ) {
        self.publish(key, payload_len, |tmp_path| {
            Self::write_streamed_entry(tmp_path, payload_len, write_payload)
        });
    }

    /// Stage an entry through `write_tmp`, then rename it into place.
    fn publish(
        &self,
        key: &CacheKey,
        payload_len: u64,
        write_tmp: impl FnOnce(&Path) -> io::Result<()>,
    ) {
        let final_path = self.entry_path(key);
        // Distinct keys produce distinct hex filenames (no extension), so `<digest>.tmp` is unique per key — parallel group bakes never collide here.
        let tmp_path = final_path.with_extension("tmp");

        if let Err(err) = write_tmp(&tmp_path) {
            log::warn!(
                "[cache] failed to stage entry {}: {err}",
                tmp_path.display()
            );
            // Best-effort cleanup; ignore errors removing the partial file.
            let _ = fs::remove_file(&tmp_path);
            return;
        }

        if let Err(err) = fs::rename(&tmp_path, &final_path) {
            log::warn!(
                "[cache] failed to publish entry {}: {err}",
                final_path.display()
            );
            let _ = fs::remove_file(&tmp_path);
        } else {
            self.record_live_entry(key, HEADER_BYTES as u64 + payload_len);
            #[cfg(test)]
            self.record_test_write(key);
        }
    }

    #[cfg(test)]
    #[allow(dead_code)] // Consumed by binary-only cross-bake tests, not the library test target.
    pub(crate) fn test_access(&self, stage_id: &str) -> CacheTestAccess {
        self.test_accesses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(stage_id)
            .copied()
            .unwrap_or_default()
    }

    #[cfg(test)]
    #[allow(dead_code)] // Consumed by binary-only cross-bake tests, not the library test target.
    pub(crate) fn clear_test_accesses(&self) {
        self.test_accesses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    /// Count an entry this build used without reading it: a section-memo hit
    /// stands in for the per-light partitions it summarizes. A missing entry
    /// is ignored.
    pub fn mark_used(&self, key: &CacheKey) {
        if let Ok(metadata) = fs::metadata(self.entry_path(key)) {
            self.record_live_entry(key, metadata.len());
        }
    }

    /// End a successful build: its read/write set replaces the map's use
    /// record, superseding any stopped build's journal, then the spared-set
    /// warning runs.
    pub fn finish_successful_build(&self, budget_bytes: u64) {
        if let Some(journal) = &self.journal {
            let entries = self
                .live_entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if let Err(err) = journal.promote(&entries) {
                log::warn!(
                    "[cache] cannot record this build's entries in {} ({err}); the next prune may evict them",
                    self.dir.display()
                );
            }
        }
        self.warn_if_live_set_exceeds(budget_bytes);
    }

    /// The entries the next prune spares: every map's record on disk plus
    /// this build's read/write set.
    fn spared_set(&self) -> SparedSet {
        let mut spared = records::read_spared(self.dir.as_path());
        spared.extend(
            self.live_entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .map(|(digest, &size)| (*digest, size)),
        );
        spared
    }

    /// Unique cache entries successfully read or written by this build.
    pub fn live_set(&self) -> CacheLiveSet {
        let entries = self
            .live_entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        CacheLiveSet {
            entry_count: entries.len(),
            total_bytes: entries.values().copied().fold(0u64, u64::saturating_add),
        }
    }

    /// Warn once when the spared set (every map's use record plus this
    /// build's read/write set) is larger than the configured cache budget.
    /// The prune keeps the spared set whole, so the cache then outgrows its
    /// budget. Reporting never prunes or changes the build.
    pub fn warn_if_live_set_exceeds(&self, budget_bytes: u64) {
        let spared = self.spared_set();
        let total = spared.values().copied().fold(0u64, u64::saturating_add);
        if total <= budget_bytes || self.live_set_reported.swap(true, AtomicOrdering::AcqRel) {
            return;
        }
        log::warn!(
            "[cache] spared set {} across {} entries (every map's last-build record) exceeds cache budget {}; the cache keeps the spared set and evicts only entries outside it",
            ByteCount(total),
            spared.len(),
            ByteCount(budget_bytes),
        );
    }

    /// Evict entries outside every map's use record, oldest write first, until
    /// the cache directory's total size is at or below `max_bytes` or nothing
    /// unspared remains. Run once at build start, before any bake writes a
    /// fresh generation, so the directory stays bounded across builds.
    ///
    /// A spared entry is one a map's last successful build read or wrote, or
    /// one a later build of that map read or wrote (see `records`). Everything
    /// else is the orphaned-generation tail content addressing leaves behind,
    /// aged by its write time. Within the same mtime, eviction order is
    /// unspecified.
    ///
    /// Best-effort: any I/O error while scanning or deleting is logged and the
    /// prune moves on. A failure to reclaim enough never fails the build — the
    /// cache is always safe to leave larger than the budget. Entries are deleted
    /// oldest-first only as far as needed; if the total already fits, nothing is
    /// touched. `*.tmp` files (in-flight `put` stages) are skipped so a
    /// concurrent write is never corrupted.
    pub fn prune_to_budget(&self, max_bytes: u64) {
        let read_dir = match fs::read_dir(self.dir.as_path()) {
            Ok(rd) => rd,
            Err(err) => {
                log::warn!(
                    "[cache] prune skipped: cannot read {}: {err}",
                    self.dir.display()
                );
                return;
            }
        };

        let spared: std::collections::HashSet<String> = records::read_spared(self.dir.as_path())
            .keys()
            .map(|digest| hex_encode(digest))
            .collect();

        // Gather (mtime, size, path) for every entry file. Skip non-files and
        // in-flight `.tmp` stages; a metadata failure drops just that entry.
        struct Entry {
            mtime: SystemTime,
            size: u64,
            path: PathBuf,
        }
        let mut entries: Vec<Entry> = Vec::new();
        let mut total: u64 = 0;
        for dir_entry in read_dir {
            let dir_entry = match dir_entry {
                Ok(e) => e,
                Err(err) => {
                    log::warn!("[cache] prune: directory entry error: {err}");
                    continue;
                }
            };
            let path = dir_entry.path();
            if path.extension().is_some_and(|ext| ext == "tmp") {
                continue;
            }
            let meta = match dir_entry.metadata() {
                Ok(m) => m,
                Err(err) => {
                    log::warn!("[cache] prune: cannot stat {}: {err}", path.display());
                    continue;
                }
            };
            if !meta.is_file() {
                continue;
            }
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let size = meta.len();
            total = total.saturating_add(size);
            let name = dir_entry.file_name();
            if spared.contains(name.to_string_lossy().as_ref()) {
                continue;
            }
            entries.push(Entry { mtime, size, path });
        }

        if total <= max_bytes {
            return;
        }

        // Oldest first, so we evict the least-recently-used generations.
        entries.sort_by_key(|e| e.mtime);

        let mut reclaimed: u64 = 0;
        let mut removed: usize = 0;
        for entry in &entries {
            if total <= max_bytes {
                break;
            }
            match fs::remove_file(&entry.path) {
                Ok(()) => {
                    total = total.saturating_sub(entry.size);
                    reclaimed = reclaimed.saturating_add(entry.size);
                    removed += 1;
                }
                Err(err) => {
                    log::warn!(
                        "[cache] prune: failed to remove {}: {err}",
                        entry.path.display()
                    );
                }
            }
        }

        if removed > 0 {
            log::info!(
                "[cache] prune: evicted {removed} unspared entries ({} reclaimed), now ~{} (budget {})",
                human_bytes(reclaimed),
                human_bytes(total),
                human_bytes(max_bytes),
            );
        }
    }

    fn write_streamed_entry(
        tmp_path: &Path,
        payload_len: u64,
        write_payload: impl FnOnce(&mut dyn Write) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut buffered =
            BufWriter::with_capacity(STREAM_BUFFER_BYTES, fs::File::create(tmp_path)?);
        // The hash is patched in once the payload has streamed through.
        buffered.write_all(&entry_header(
            payload_len,
            blake3::Hash::from_bytes([0; HASH_BYTES]),
        ))?;

        let (actual_len, hash) = {
            let mut payload_writer = HashingWriter::new(&mut buffered);
            write_payload(&mut payload_writer)?;
            payload_writer.flush()?;
            payload_writer.finish()
        };
        let mut file = buffered
            .into_inner()
            .map_err(io::IntoInnerError::into_error)?;
        if actual_len != payload_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "streamed cache payload length mismatch: declared={payload_len} actual={actual_len}"
                ),
            ));
        }

        file.seek(SeekFrom::Start(
            (ENTRY_MAGIC.len() + LENGTH_PREFIX_BYTES) as u64,
        ))?;
        file.write_all(hash.as_bytes())
    }

    #[cfg(test)]
    fn record_test_read_attempt(&self, key: &CacheKey) {
        self.test_accesses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(key.stage_id.clone())
            .or_default()
            .read_attempts += 1;
    }

    #[cfg(test)]
    fn record_test_read_hit(&self, key: &CacheKey) {
        self.test_accesses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(key.stage_id.clone())
            .or_default()
            .read_hits += 1;
    }

    #[cfg(test)]
    fn record_test_write(&self, key: &CacheKey) {
        self.test_accesses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(key.stage_id.clone())
            .or_default()
            .writes += 1;
    }

    fn entry_path(&self, key: &CacheKey) -> PathBuf {
        self.dir.join(key.as_filename())
    }

    fn record_live_entry(&self, key: &CacheKey, bytes: u64) {
        let first_use = self
            .live_entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key.digest, bytes)
            .is_none();
        if first_use && let Some(journal) = &self.journal {
            journal.append(&key.digest, bytes);
        }
    }
}

struct ByteCount(u64);

impl fmt::Display for ByteCount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({})", self.0, human_bytes(self.0))
    }
}

/// Entry header: format marker, payload length, payload hash.
fn entry_header(payload_len: u64, hash: blake3::Hash) -> [u8; HEADER_BYTES] {
    let mut header = [0u8; HEADER_BYTES];
    header[..ENTRY_MAGIC.len()].copy_from_slice(&ENTRY_MAGIC);
    let length_end = ENTRY_MAGIC.len() + LENGTH_PREFIX_BYTES;
    header[ENTRY_MAGIC.len()..length_end].copy_from_slice(&payload_len.to_le_bytes());
    header[length_end..].copy_from_slice(hash.as_bytes());
    header
}

struct HashingWriter<'a> {
    file: &'a mut BufWriter<fs::File>,
    hasher: blake3::Hasher,
    bytes_written: u64,
}

impl<'a> HashingWriter<'a> {
    fn new(file: &'a mut BufWriter<fs::File>) -> Self {
        Self {
            file,
            hasher: blake3::Hasher::new(),
            bytes_written: 0,
        }
    }

    fn finish(self) -> (u64, blake3::Hash) {
        (self.bytes_written, self.hasher.finalize())
    }
}

impl Write for HashingWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.file.write(bytes)?;
        self.hasher.update(&bytes[..written]);
        self.bytes_written = self
            .bytes_written
            .checked_add(written as u64)
            .ok_or_else(|| io::Error::other("streamed cache payload length overflow"))?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Walk parent directories from `start` looking for the first `Cargo.toml`
/// encountered. Returns the directory containing it, used as the default cache
/// root when the CLI flag is omitted. This finds the nearest crate or workspace
/// manifest — not necessarily a `[workspace]` root — so callers should treat
/// the result as "a reasonable ancestor with a manifest", not a guaranteed
/// workspace root.
pub fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut current: Option<&Path> = Some(start);
    while let Some(dir) = current {
        if dir.join("Cargo.toml").is_file() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

/// Compact human-readable byte count for prune log lines (e.g. `1.83 GiB`).
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;
    use postretro_test_log_capture::LogCapture;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Unique per-test temp directory under the OS temp dir. Avoids pulling in
    /// `tempfile` for a handful of unit tests.
    fn fresh_temp_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "postretro_cache_test_{label}_{stamp}_{nonce}_{}",
            std::process::id()
        ));
        // Start clean if a previous run left this path behind.
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn cache_roundtrip_stores_and_retrieves_bytes() {
        let dir = fresh_temp_dir("roundtrip");
        let cache = StageCache::new(&dir).expect("create cache dir");
        let key = CacheKey::new("lightmap", 1, b"input-fingerprint");
        let payload = b"hello cache".to_vec();

        cache.put(&key, &payload);
        let loaded = cache.get(&key).expect("entry should be present");
        assert_eq!(loaded, payload);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_set_counts_each_read_or_written_key_once() {
        let dir = fresh_temp_dir("live_set_dedup");
        let cache = StageCache::new(&dir).unwrap();
        let first = CacheKey::new("lightmap_layer", 6, b"first");
        let second = CacheKey::new("lightmap_section", 3, b"second");
        cache.put(&first, b"1234");
        cache.put(&first, b"1234");
        assert_eq!(cache.get(&first), Some(b"1234".to_vec()));
        cache.put(&second, b"12345678");
        assert_eq!(cache.get(&second), Some(b"12345678".to_vec()));

        assert_eq!(
            cache.live_set(),
            CacheLiveSet {
                entry_count: 2,
                total_bytes: (HEADER_BYTES * 2 + 12) as u64,
            }
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_set_warning_is_exactly_once_and_names_total_and_budget() {
        let dir = fresh_temp_dir("live_set_warning");
        let cache = StageCache::new(&dir).unwrap();
        let key = CacheKey::new("lightmap_layer", 6, b"large");
        cache.put(&key, &[0; 32]);
        let total = (HEADER_BYTES + 32) as u64;
        let budget = total - 1;
        let capture = LogCapture::start();

        cache.warn_if_live_set_exceeds(budget);
        cache.warn_if_live_set_exceeds(budget);

        capture.assert_logged_once(
            Level::Warn,
            &format!("[cache] spared set {total} ({})", human_bytes(total)),
        );
        capture.assert_logged_once(
            Level::Warn,
            &format!("exceeds cache budget {budget} ({})", human_bytes(budget)),
        );
        capture.assert_not_logged(Level::Warn, "may evict");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_set_under_budget_is_silent() {
        let dir = fresh_temp_dir("live_set_under_budget");
        let cache = StageCache::new(&dir).unwrap();
        let key = CacheKey::new("lightmap_layer", 6, b"small");
        cache.put(&key, b"payload");
        let capture = LogCapture::start();

        cache.warn_if_live_set_exceeds(u64::MAX);

        capture.assert_not_logged(Level::Warn, "[cache] spared set");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn streamed_cache_write_matches_contiguous_entry_bytes_and_roundtrips() {
        let dir = fresh_temp_dir("streamed_identity");
        let cache = StageCache::new(&dir).expect("create cache dir");
        let contiguous_key = CacheKey::new("shadowmask_atlas", 2, b"contiguous");
        let streamed_key = CacheKey::new("shadowmask_atlas", 2, b"streamed");
        let parts: [&[u8]; 4] = [b"header", b"channels", &[0, 0, 0], b"atlas-data"];
        let payload: Vec<u8> = parts.iter().flat_map(|part| part.iter().copied()).collect();

        cache.put(&contiguous_key, &payload);
        cache.put_streamed(&streamed_key, payload.len() as u64, |writer| {
            for part in parts {
                writer.write_all(part)?;
            }
            Ok(())
        });

        let contiguous_entry =
            fs::read(dir.join(contiguous_key.as_filename())).expect("read contiguous entry");
        let streamed_entry =
            fs::read(dir.join(streamed_key.as_filename())).expect("read streamed entry");
        assert_eq!(streamed_entry, contiguous_entry);
        assert_eq!(cache.get(&streamed_key), Some(payload));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn streamed_cache_write_rejects_length_mismatch_without_publishing() {
        let dir = fresh_temp_dir("streamed_length_mismatch");
        let cache = StageCache::new(&dir).expect("create cache dir");
        let key = CacheKey::new("shadowmask_atlas", 2, b"wrong-length");

        cache.put_streamed(&key, 6, |writer| writer.write_all(b"short"));

        assert!(cache.get(&key).is_none());
        assert!(!dir.join(key.as_filename()).with_extension("tmp").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_streamed_cache_write_preserves_published_entry_and_cleans_stage() {
        let dir = fresh_temp_dir("streamed_atomic_failure");
        let cache = StageCache::new(&dir).expect("create cache dir");
        let key = CacheKey::new("shadowmask_atlas", 2, b"preserve-published");
        cache.put(&key, b"published");

        cache.put_streamed(&key, 7, |writer| {
            writer.write_all(b"partial")?;
            Err(io::Error::other("injected cache write failure"))
        });

        assert_eq!(cache.get(&key), Some(b"published".to_vec()));
        assert!(!dir.join(key.as_filename()).with_extension("tmp").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_get_returns_none_for_missing_entry() {
        let dir = fresh_temp_dir("missing");
        let cache = StageCache::new(&dir).expect("create cache dir");
        let key = CacheKey::new("sh_volume", 2, b"never-written");

        assert!(cache.get(&key).is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_detects_corrupted_entry() {
        let dir = fresh_temp_dir("corrupt");
        let cache = StageCache::new(&dir).expect("create cache dir");
        let key = CacheKey::new("lightmap", 1, b"corrupt-case");

        // Write garbage straight into the entry path so the header and hash
        // checks both fail.
        let entry_path = dir.join(key.as_filename());
        fs::write(&entry_path, b"not a valid cache entry payload").expect("write garbage");

        assert!(cache.get(&key).is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_uses_a_u64_payload_length_and_rejects_legacy_entries() {
        let dir = fresh_temp_dir("u64_length");
        let cache = StageCache::new(&dir).expect("create cache dir");
        let key = CacheKey::new("shadowmask_atlas", 1, b"large-entry");
        let path = dir.join(key.as_filename());

        // A pre-v2 `u32 length + hash + payload` entry must be a safe miss,
        // rather than being decoded as a huge u64 payload length.
        fs::write(&path, [0u8; 4 + HASH_BYTES]).expect("write legacy header");
        assert!(cache.get(&key).is_none());

        cache.put(&key, b"payload");
        let encoded = fs::read(&path).expect("read v2 entry");
        assert_eq!(&encoded[..ENTRY_MAGIC.len()], ENTRY_MAGIC);
        let length_start = ENTRY_MAGIC.len();
        let length_end = length_start + LENGTH_PREFIX_BYTES;
        assert_eq!(
            u64::from_le_bytes(encoded[length_start..length_end].try_into().unwrap()),
            7
        );
        assert_eq!(cache.get(&key), Some(b"payload".to_vec()));

        let _ = fs::remove_dir_all(&dir);
    }

    /// Overwrite an entry's mtime so prune-ordering tests are deterministic
    /// instead of depending on wall-clock write order.
    ///
    /// Opened for writing, not with `File::open`: Windows refuses to set a file
    /// time through a read-only handle. The read-only version of this helper
    /// failed on Windows for the same reason `StageCache::touch_for_lru` did,
    /// which is what kept that production defect looking like a test artifact.
    fn set_mtime(path: &Path, t: SystemTime) {
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open entry for writing to set mtime")
            .set_modified(t)
            .expect("set mtime");
    }

    #[test]
    fn prune_evicts_least_recently_used_until_under_budget() {
        let dir = fresh_temp_dir("prune_lru");
        let cache = StageCache::new(&dir).expect("create cache dir");

        // Three 100-byte entries with distinct ages: a oldest, c newest.
        let payload = vec![0u8; 100];
        let entry_len = (HEADER_BYTES + payload.len()) as u64;
        let now = SystemTime::now();
        for (label, age_secs) in [("a", 300u64), ("b", 200), ("c", 100)] {
            let key = CacheKey::new("lightmap_layer", 1, label.as_bytes());
            cache.put(&key, &payload);
            set_mtime(
                &dir.join(key.as_filename()),
                now - std::time::Duration::from_secs(age_secs),
            );
        }

        // Budget fits two entries but not three: the oldest (a) must go.
        cache.prune_to_budget(entry_len * 2 + 10);

        let a = CacheKey::new("lightmap_layer", 1, b"a");
        let b = CacheKey::new("lightmap_layer", 1, b"b");
        let c = CacheKey::new("lightmap_layer", 1, b"c");
        assert!(cache.get(&a).is_none(), "oldest entry must be evicted");
        assert!(cache.get(&b).is_some(), "newer entry must survive");
        assert!(cache.get(&c).is_some(), "newest entry must survive");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_is_noop_when_under_budget() {
        let dir = fresh_temp_dir("prune_noop");
        let cache = StageCache::new(&dir).expect("create cache dir");
        let key = CacheKey::new("sh_group", 1, b"keep-me");
        cache.put(&key, b"payload");

        cache.prune_to_budget(DEFAULT_MAX_BYTES);

        assert!(
            cache.get(&key).is_some(),
            "entry must survive when total is under budget"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A hit is recorded in the build's use journal through its one read
    /// handle: the entry's mtime is left alone, and the next build's prune
    /// spares it even though it is the oldest entry in the directory.
    #[test]
    fn get_hit_is_recorded_without_touching_the_entry() {
        let dir = fresh_temp_dir("hit_recorded");
        let input = dir.join("map.map");
        let payload = vec![0u8; 100];
        let old = CacheKey::new("lightmap_layer", 1, b"old");
        let new = CacheKey::new("lightmap_layer", 1, b"new");
        {
            let seed = StageCache::new(&dir).expect("create cache dir");
            seed.put(&old, &payload);
            seed.put(&new, &payload);
        }
        let stale = SystemTime::now() - std::time::Duration::from_secs(3600);
        set_mtime(&dir.join(old.as_filename()), stale);

        let build = StageCache::open_for_build(&dir, &input, u64::MAX).expect("open build");
        assert!(build.get(&old).is_some(), "warm-up read must hit");
        let after = fs::metadata(dir.join(old.as_filename()))
            .and_then(|metadata| metadata.modified())
            .expect("entry mtime");
        assert_eq!(after, stale, "a hit must not touch the entry");
        build.finish_successful_build(u64::MAX);

        let next = StageCache::open_for_build(&dir, &input, 0).expect("open next build");
        assert!(
            next.get(&old).is_some(),
            "the recorded hit must survive the prune"
        );
        assert!(
            next.get(&new).is_none(),
            "the unrecorded entry must be evicted"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_skips_in_flight_tmp_files() {
        let dir = fresh_temp_dir("prune_tmp");
        let cache = StageCache::new(&dir).expect("create cache dir");

        // Simulate an in-flight `put` stage file that prune must not touch.
        let tmp = dir.join("deadbeef.tmp");
        fs::write(&tmp, vec![0u8; 4096]).expect("write tmp stage");

        // Budget 0 forces eviction of everything prune is willing to delete.
        cache.prune_to_budget(0);

        assert!(tmp.is_file(), ".tmp stage files must be left untouched");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Child half of the killed-write case: when the parent sets this
    /// variable, stage a partial entry, signal, and wait to be killed.
    const KILL_CHILD_ENV: &str = "POSTRETRO_CACHE_KILL_CHILD_DIR";

    #[test]
    fn cache_kill_child_writer() {
        let Some(dir) = std::env::var_os(KILL_CHILD_ENV) else {
            return;
        };
        let dir = PathBuf::from(dir);
        let cache = StageCache::new(&dir).expect("child cache dir");
        let key = CacheKey::new("lightmap_layer", 1, b"killed");
        cache.put_streamed(&key, 4096, |writer| {
            writer.write_all(&[7; 1024])?;
            writer.flush()?;
            fs::write(dir.join("child-is-mid-write"), b"")?;
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        });
        unreachable!("the parent kills the child mid-write");
    }

    // A13 regression guard: durability is by verification, not sync. A deleted
    // or truncated entry, or a write killed before it published, reads back as
    // a miss, never as a wrong hit.
    #[test]
    fn cache_deleted_truncated_or_killed_entry_is_a_miss() {
        let dir = fresh_temp_dir("a13_guard");
        let cache = StageCache::new(&dir).expect("create cache dir");
        let payload = vec![42u8; 4096];

        let deleted = CacheKey::new("lightmap_layer", 1, b"deleted");
        cache.put(&deleted, &payload);
        fs::remove_file(dir.join(deleted.as_filename())).expect("delete entry");
        assert!(cache.get(&deleted).is_none(), "a deleted entry must miss");

        let truncated = CacheKey::new("lightmap_layer", 1, b"truncated");
        cache.put(&truncated, &payload);
        let path = dir.join(truncated.as_filename());
        for keep in [HEADER_BYTES as u64 + 100, HEADER_BYTES as u64 - 1, 0] {
            fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .expect("open entry")
                .set_len(keep)
                .expect("truncate entry");
            assert!(
                cache.get(&truncated).is_none(),
                "truncated to {keep} must miss"
            );
        }

        let child_dir = dir.join("killed");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "cache::tests::cache_kill_child_writer",
                "--exact",
                "--nocapture",
            ])
            .env(KILL_CHILD_ENV, &child_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn child writer");
        let marker = child_dir.join("child-is-mid-write");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !marker.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "child never reached its mid-write point"
            );
            if let Some(status) = child.try_wait().expect("poll child") {
                panic!("child exited before the kill: {status}");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        child.kill().expect("kill child mid-write");
        child.wait().expect("reap child");
        let killed = CacheKey::new("lightmap_layer", 1, b"killed");
        let reopened = StageCache::new(&child_dir).expect("reopen cache after kill");
        assert!(reopened.get(&killed).is_none(), "a killed write must miss");
        assert!(
            !child_dir.join(killed.as_filename()).exists(),
            "a killed write must not publish its entry"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// Flat-directory cost: per-op `put` and `get` time at 1k to 150k entries
    /// in one directory. Prints; asserts nothing. Set
    /// `POSTRETRO_CACHE_MEASURE_DIR` to measure on the volume a bake uses;
    /// real-time antivirus scanning of new files dominates if it covers it.
    #[test]
    #[ignore = "writes ~150k cache entries; run on demand"]
    fn measure_flat_cache_dir_op_cost() {
        const SAMPLE: usize = 1000;
        let dir = std::env::var_os("POSTRETRO_CACHE_MEASURE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| fresh_temp_dir("flat_dir_cost"));
        let _ = fs::remove_dir_all(&dir);
        let cache = StageCache::new(&dir).expect("create cache dir");
        let payload = vec![3u8; 14 * 1024];
        let mut written = 0usize;
        for target in [SAMPLE, 10_000, 50_000, 150_000] {
            while written + SAMPLE < target {
                let key = CacheKey::new("measure", 1, &(written as u64).to_le_bytes());
                cache.put(&key, &payload);
                written += 1;
            }
            let started = std::time::Instant::now();
            for i in 0..SAMPLE {
                let key = CacheKey::new("measure", 1, &((written + i) as u64).to_le_bytes());
                cache.put(&key, &payload);
            }
            let put = started.elapsed() / SAMPLE as u32;
            let started = std::time::Instant::now();
            for i in 0..SAMPLE {
                let key = CacheKey::new("measure", 1, &((written + i) as u64).to_le_bytes());
                assert!(cache.get(&key).is_some());
            }
            let get = started.elapsed() / SAMPLE as u32;
            written += SAMPLE;
            eprintln!("[flat-dir] {written} entries: put {put:?}/op, get {get:?}/op");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_workspace_root_locates_cargo_toml() {
        // CARGO_MANIFEST_DIR points at the level-compiler crate; src/ is a
        // child of that. Searching from src/ should find the crate manifest
        // (the first Cargo.toml encountered while walking up).
        let start = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let root = find_workspace_root(&start).expect("workspace root should be found");
        assert!(root.join("Cargo.toml").is_file());
    }
}
