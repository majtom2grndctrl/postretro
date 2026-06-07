// Disk-backed content-hash cache for expensive compile stages.
// See: context/lib/build_pipeline.md

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Default LRU size budget for the on-disk stage cache, in bytes (2 GiB).
/// Pruned down to this at build start unless `--cache-max-size` overrides it.
/// Content addressing never reclaims orphaned generations on its own, so this
/// bound is what stops the cache from growing without limit.
pub const DEFAULT_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Length-of-payload prefix (u32 little endian) preceding the integrity hash.
const LENGTH_PREFIX_BYTES: usize = 4;
/// blake3 digest length.
const HASH_BYTES: usize = 32;
/// Combined header size in front of the payload on disk.
const HEADER_BYTES: usize = LENGTH_PREFIX_BYTES + HASH_BYTES;

/// Identifier for a single cache entry. Hashes `(stage_id, stage_version,
/// input_hash)` so unrelated stages and incompatible bakers never collide on
/// the same filename.
pub struct CacheKey {
    digest: [u8; HASH_BYTES],
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
        }
    }

    /// Hex-encoded blake3 digest, used as the on-disk filename.
    pub fn as_filename(&self) -> String {
        hex_encode(&self.digest)
    }
}

/// Directory-backed cache. `put` writes atomically; `get` validates the
/// length prefix and blake3 digest before returning the payload.
pub struct StageCache {
    dir: PathBuf,
}

impl StageCache {
    /// Open (or create) a cache directory. Any I/O error from `create_dir_all`
    /// surfaces — the caller decides whether to disable caching for the run.
    pub fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        let dir = path.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Load and validate an entry. Missing entries return `None` silently.
    /// Corrupted entries (short read, length mismatch, hash mismatch) log a
    /// warning and return `None` so the stage falls through to a rebuild.
    ///
    /// A successful read bumps the entry's mtime to now so the LRU prune
    /// (`prune_to_budget`) treats it as recently used. This is what keeps a
    /// long-stable entry (one whose inputs never change, so it is hit every
    /// build but never rewritten) from being evicted purely for being old.
    pub fn get(&self, key: &CacheKey) -> Option<Vec<u8>> {
        let path = self.entry_path(key);
        let mut file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
            Err(err) => {
                log::warn!("[cache] failed to open {}: {err}", path.display());
                return None;
            }
        };

        // Mark the entry as freshly used for LRU. Best-effort: a failure here
        // only makes the prune slightly less accurate, never breaks the read.
        let _ = file.set_modified(SystemTime::now());

        let mut header = [0u8; HEADER_BYTES];
        if let Err(err) = file.read_exact(&mut header) {
            log::warn!("[cache] entry {} header read failed: {err}", path.display());
            return None;
        }

        let declared_len =
            u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let stored_hash: [u8; HASH_BYTES] = header[LENGTH_PREFIX_BYTES..]
            .try_into()
            .expect("header slice is exactly HASH_BYTES wide");

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

        Some(payload)
    }

    /// Write an entry atomically. Best-effort: any error is logged and
    /// swallowed so a flaky cache directory cannot break a build.
    pub fn put(&self, key: &CacheKey, bytes: &[u8]) {
        let final_path = self.entry_path(key);
        // Distinct keys produce distinct hex filenames (no extension), so `<digest>.tmp` is unique per key — parallel group bakes never collide here.
        let tmp_path = final_path.with_extension("tmp");

        if let Err(err) = self.write_entry(&tmp_path, bytes) {
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
        }
    }

    /// Evict least-recently-used entries until the cache directory's total size
    /// is at or below `max_bytes`. Run once at build start, before any bake
    /// writes a fresh generation, so the directory stays bounded across builds.
    ///
    /// Recency is the entry's mtime, which `get` bumps on every hit and `put`
    /// sets on write — so "least recently used" means "longest since a build
    /// last read or wrote it", which is exactly the orphaned-generation tail
    /// that content addressing leaves behind. Within the same mtime, eviction
    /// order is unspecified.
    ///
    /// Best-effort: any I/O error while scanning or deleting is logged and the
    /// prune moves on. A failure to reclaim enough never fails the build — the
    /// cache is always safe to leave larger than the budget. Entries are deleted
    /// oldest-first only as far as needed; if the total already fits, nothing is
    /// touched. `*.tmp` files (in-flight `put` stages) are skipped so a
    /// concurrent write is never corrupted.
    pub fn prune_to_budget(&self, max_bytes: u64) {
        let read_dir = match fs::read_dir(&self.dir) {
            Ok(rd) => rd,
            Err(err) => {
                log::warn!(
                    "[cache] prune skipped: cannot read {}: {err}",
                    self.dir.display()
                );
                return;
            }
        };

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
                "[cache] prune: evicted {removed} LRU entries ({} reclaimed), now ~{} (budget {})",
                human_bytes(reclaimed),
                human_bytes(total),
                human_bytes(max_bytes),
            );
        }
    }

    fn write_entry(&self, tmp_path: &Path, bytes: &[u8]) -> io::Result<()> {
        let hash = blake3::hash(bytes);
        let len = u32::try_from(bytes.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "cache payload exceeds u32 length prefix",
            )
        })?;

        let mut file = fs::File::create(tmp_path)?;
        file.write_all(&len.to_le_bytes())?;
        file.write_all(hash.as_bytes())?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }

    fn entry_path(&self, key: &CacheKey) -> PathBuf {
        self.dir.join(key.as_filename())
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

    /// Overwrite an entry's mtime so prune-ordering tests are deterministic
    /// instead of depending on wall-clock write order.
    fn set_mtime(path: &Path, t: SystemTime) {
        fs::File::open(path)
            .expect("open entry to set mtime")
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

    #[test]
    fn get_refreshes_mtime_so_hot_entries_survive_prune() {
        let dir = fresh_temp_dir("prune_touch");
        let cache = StageCache::new(&dir).expect("create cache dir");

        let payload = vec![0u8; 100];
        let entry_len = (HEADER_BYTES + payload.len()) as u64;
        let now = SystemTime::now();

        // `old` was written long ago; `new` more recently. Without a touch,
        // a budget-for-one prune would evict `old`.
        let old = CacheKey::new("lightmap_layer", 1, b"old");
        let new = CacheKey::new("lightmap_layer", 1, b"new");
        cache.put(&old, &payload);
        set_mtime(
            &dir.join(old.as_filename()),
            now - std::time::Duration::from_secs(300),
        );
        cache.put(&new, &payload);
        set_mtime(
            &dir.join(new.as_filename()),
            now - std::time::Duration::from_secs(100),
        );

        // A hit on `old` bumps its mtime to ~now, making `new` the LRU victim.
        assert!(cache.get(&old).is_some(), "warm-up read must hit");

        cache.prune_to_budget(entry_len + 10); // room for exactly one entry

        assert!(
            cache.get(&old).is_some(),
            "the recently-read entry must survive even though it was written first"
        );
        assert!(
            cache.get(&new).is_none(),
            "the now-least-recently-used entry must be evicted"
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
