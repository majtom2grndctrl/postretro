// Id 34's last recorded bake time, under its volume key: what the Build
// Summary projects a mover's `--release` bounce bake from when id 34 comes
// from the cache.
// See: context/lib/build_pipeline.md §Build Cache (Bake fidelity)

use std::time::Duration;

use crate::cache::{CacheKey, StageCache};

/// The record's cache stage. Its own entry, so recording the time changes
/// no byte of the `sh_base_volume` entry or its version.
const STAGE_ID: &str = "sh_base_bake_time";
/// 1: id 34's bake time in seconds, one little-endian `f64`.
const STAGE_VERSION: u32 = 1;

/// The record under one id-34 volume key: a build that bakes id 34 writes
/// it, cold or warm, and a build that reads id 34 from the cache reads it.
pub(crate) struct BakeTimeRecord<'c> {
    cache: &'c StageCache,
    key: CacheKey,
}

impl<'c> BakeTimeRecord<'c> {
    pub(crate) fn new(cache: &'c StageCache, volume_key: &CacheKey) -> Self {
        Self {
            cache,
            key: Self::key(volume_key),
        }
    }

    /// The record's key under id 34's `volume_key`.
    pub(crate) fn key(volume_key: &CacheKey) -> CacheKey {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"sh base bake time\0");
        hasher.update(volume_key.as_filename().as_bytes());
        CacheKey::new(STAGE_ID, STAGE_VERSION, hasher.finalize().as_bytes())
    }

    /// The recorded time, or `None` when none was recorded under this key
    /// or the payload is not one finite, non-negative `f64`.
    pub(crate) fn get(&self) -> Option<Duration> {
        let bytes: [u8; 8] = self.cache.get(&self.key)?.try_into().ok()?;
        let seconds = f64::from_le_bytes(bytes);
        (seconds.is_finite() && seconds >= 0.0).then(|| Duration::from_secs_f64(seconds))
    }

    pub(crate) fn put(&self, time: Duration) {
        self.cache.put(&self.key, &time.as_secs_f64().to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recorded_time_reads_back_under_its_volume_key_alone() {
        let dir = tempfile::tempdir().expect("cache dir");
        let cache = StageCache::new(dir.path()).expect("open cache");
        let volume = CacheKey::new("sh_base_volume", 1, b"volume a");
        let other = CacheKey::new("sh_base_volume", 1, b"volume b");
        let record = BakeTimeRecord::new(&cache, &volume);
        assert_eq!(record.get(), None, "nothing recorded reads as none");

        record.put(Duration::from_millis(12_500));
        assert_eq!(record.get(), Some(Duration::from_millis(12_500)));
        assert_eq!(BakeTimeRecord::new(&cache, &other).get(), None);
    }
}
