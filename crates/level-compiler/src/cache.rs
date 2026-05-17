// Disk-backed content-hash cache for expensive compile stages.
// See: context/lib/build_pipeline.md

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

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
