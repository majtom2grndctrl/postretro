// Sound asset loading and the per-level sound registry. Resolves paths under
// `content/<mod>/_sounds/`, decodes SFX in memory (static) and probes music for
// streaming, and holds the result keyed by content-relative name. Consumed by
// the play API (Task 4). Load and decode failures degrade gracefully: a missing
// or undecodable file logs an `[Audio]` warning, is skipped, and never panics.
// See: context/lib/audio.md · context/lib/resource_management.md §7.2

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use kira::sound::static_sound::StaticSoundData;
use kira::sound::streaming::StreamingSoundData;

/// Top-level collection whose sounds stream from disk instead of decoding into
/// memory. Everything outside `music/` is treated as an in-memory SFX clip.
/// Classification is purely by the first path segment of the registry key.
const STREAMING_COLLECTION: &str = "music";

/// Subdirectory under a mod's content root holding all sound assets.
const SOUNDS_DIR: &str = "_sounds";

/// A loaded sound entry held by the registry. The two kira sound-data types are
/// modeled directly because they behave differently:
///
/// - `Static` owns a fully decoded [`StaticSoundData`]. Its samples are
///   `Arc<[Frame]>`, so cloning the entry for each playback is a cheap refcount
///   bump and the same decoded buffer is replayed any number of times.
/// - `Streaming` cannot hold a replayable `StreamingSoundData`: that type wraps
///   a `Box<dyn Decoder>`, is not `Clone`, and is consumed when handed to the
///   manager. Re-fetching for playback means re-opening the file, so the entry
///   stores the resolved path and re-creates a fresh `StreamingSoundData` via
///   [`StreamingSoundData::from_file`] each time it is played. The file is still
///   decode-probed at load so an undecodable music track degrades like any other
///   missing asset rather than failing the first time it is triggered.
pub(crate) enum LoadedSound {
    /// Decode-in-memory clip; clone per playback (cheap Arc bump). The inner
    /// data is read by the play API in Task 4 (and by tests via `matches!`).
    /// Boxed because `StaticSoundData` (with its settings) is far larger than
    /// the `Streaming` variant — keeps the enum small (clippy::large_enum_variant).
    #[allow(dead_code)]
    Static(Box<StaticSoundData>),
    /// Stream-from-disk track; re-opened from this path for each playback.
    Streaming { path: PathBuf },
}

impl LoadedSound {
    /// Re-open a streaming entry for playback. Returns `None` (with an `[Audio]`
    /// warning) if the file became unreadable since load. Static entries return
    /// `None` — callers play those by cloning the `StaticSoundData` directly.
    /// Consumed by the play API in Task 4.
    #[allow(dead_code)]
    pub(crate) fn open_streaming(&self) -> Option<StreamingSoundData<kira::sound::FromFileError>> {
        match self {
            LoadedSound::Static(_) => None,
            LoadedSound::Streaming { path } => match StreamingSoundData::from_file(path) {
                Ok(data) => Some(data),
                Err(err) => {
                    log::warn!(
                        "[Audio] streaming sound '{}' became unreadable: {err} — not played",
                        path.display(),
                    );
                    None
                }
            },
        }
    }
}

/// Sound assets for the currently installed level, keyed by content-relative
/// name (the path under `_sounds/` with the extension stripped, e.g.
/// `sfx/test_tone` or `music/test_loop`). Populated at level install and cleared
/// at level unload so the registry follows level lifetime, mirroring textures
/// (`resource_management.md` §7.2).
#[derive(Default)]
pub(crate) struct SoundRegistry {
    sounds: HashMap<String, LoadedSound>,
}

impl SoundRegistry {
    /// An empty registry — the state before any level is installed and after
    /// unload.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Number of registered sounds. Used by tests and diagnostics.
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.sounds.len()
    }

    /// Whether a sound is registered under `key`. Used by tests.
    #[allow(dead_code)]
    pub(crate) fn contains(&self, key: &str) -> bool {
        self.sounds.contains_key(key)
    }

    /// Look up a loaded sound by its content-relative key. Returns `None` if no
    /// sound registered under that key (e.g. it failed to decode and was
    /// skipped). Consumed by the play API in Task 4.
    #[allow(dead_code)]
    pub(crate) fn get(&self, key: &str) -> Option<&LoadedSound> {
        self.sounds.get(key)
    }

    /// Drop every registered sound. Called at level unload; the registry returns
    /// to its empty state and can be repopulated by a subsequent load.
    pub(crate) fn clear(&mut self) {
        self.sounds.clear();
    }

    /// Scan `<content_root>/_sounds/` and register every decodable `.ogg`/`.wav`
    /// file found, replacing any previously registered sounds. Files under the
    /// top-level `music/` collection load as streaming; all others decode into
    /// memory as static. A missing `_sounds/` directory is not an error (the mod
    /// simply ships no sounds). Individual unreadable or undecodable files log an
    /// `[Audio]` warning, are skipped, and never panic.
    pub(crate) fn load_from_content_root(&mut self, content_root: &Path) {
        self.clear();

        let sounds_root = content_root.join(SOUNDS_DIR);
        if !sounds_root.is_dir() {
            // No sound assets shipped with this mod — not an error.
            log::info!(
                "[Audio] no sound directory at {} — level has no sounds",
                sounds_root.display(),
            );
            return;
        }

        let mut files: Vec<PathBuf> = Vec::new();
        collect_sound_files(&sounds_root, &mut files);
        // Deterministic registration order so logs and any future tie-breaking
        // are reproducible across runs (directory iteration order is not).
        files.sort();

        for path in files {
            self.register_file(&sounds_root, &path);
        }

        log::info!(
            "[Audio] loaded {} sound(s) from {}",
            self.sounds.len(),
            sounds_root.display(),
        );
    }

    /// Decode and register a single sound file. `sounds_root` is the `_sounds/`
    /// directory the key is made relative to. Failures warn and skip.
    fn register_file(&mut self, sounds_root: &Path, path: &Path) {
        let key = match registry_key(sounds_root, path) {
            Some(k) => k,
            None => {
                log::warn!(
                    "[Audio] sound '{}' is outside the sounds root — skipped",
                    path.display(),
                );
                return;
            }
        };

        if is_streaming(&key) {
            // Probe-decode so an undecodable music file degrades at load time;
            // the playable entry re-opens the path per playback.
            match StreamingSoundData::from_file(path) {
                Ok(_probe) => {
                    self.sounds.insert(
                        key,
                        LoadedSound::Streaming {
                            path: path.to_path_buf(),
                        },
                    );
                }
                Err(err) => {
                    log::warn!(
                        "[Audio] cannot decode streaming sound '{}': {err} — skipped",
                        path.display(),
                    );
                }
            }
        } else {
            match StaticSoundData::from_file(path) {
                Ok(data) => {
                    self.sounds.insert(key, LoadedSound::Static(Box::new(data)));
                }
                Err(err) => {
                    log::warn!(
                        "[Audio] cannot decode sound '{}': {err} — skipped",
                        path.display(),
                    );
                }
            }
        }
    }
}

/// Whether a registry key belongs to the streaming collection. The rule is a
/// simple first-segment match against `music/`; subdirectories under it (e.g.
/// `music/ambient/track`) stream too.
fn is_streaming(key: &str) -> bool {
    key.split('/').next() == Some(STREAMING_COLLECTION)
}

/// Map an absolute sound path to its content-relative registry key: the path
/// relative to `_sounds/`, with the extension stripped and separators
/// normalized to `/`. Returns `None` if `path` is not under `sounds_root`.
fn registry_key(sounds_root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(sounds_root).ok()?;
    let with_ext_stripped = relative.with_extension("");
    // Normalize to forward slashes so keys are platform-independent and match
    // the `<collection>/<name>` convention regardless of host separator.
    let key = with_ext_stripped
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/");
    if key.is_empty() { None } else { Some(key) }
}

/// Whether a file extension names a supported sound container. Case-insensitive.
fn is_supported_extension(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("ogg") | Some("wav")
    )
}

/// Recursively collect `.ogg`/`.wav` files under `dir` into `out`. Unreadable
/// subdirectories warn and are skipped rather than aborting the scan.
fn collect_sound_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            log::warn!(
                "[Audio] cannot read sound directory {}: {err} — skipped",
                dir.display(),
            );
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sound_files(&path, out);
        } else if is_supported_extension(&path) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Registry keys are content-relative and platform-independent; the loader
    // classifies by the `music/` first segment and degrades on missing files.

    #[test]
    fn registry_key_strips_root_and_extension() {
        let root = Path::new("/content/dev/_sounds");
        let path = Path::new("/content/dev/_sounds/sfx/test_tone.wav");
        assert_eq!(registry_key(root, path).as_deref(), Some("sfx/test_tone"));
    }

    #[test]
    fn registry_key_rejects_path_outside_root() {
        let root = Path::new("/content/dev/_sounds");
        let path = Path::new("/content/dev/textures/foo.wav");
        assert_eq!(registry_key(root, path), None);
    }

    #[test]
    fn is_streaming_matches_only_music_collection() {
        assert!(is_streaming("music/test_loop"));
        assert!(is_streaming("music/ambient/drone"));
        assert!(!is_streaming("sfx/test_tone"));
        assert!(!is_streaming("ui/click"));
    }

    #[test]
    fn supported_extension_is_case_insensitive() {
        assert!(is_supported_extension(Path::new("a/b.ogg")));
        assert!(is_supported_extension(Path::new("a/b.WAV")));
        assert!(!is_supported_extension(Path::new("a/b.mp3")));
        assert!(!is_supported_extension(Path::new("a/b")));
    }

    /// Absolute path to the repo's `content/dev` so tests find the committed
    /// fixtures regardless of the working directory `cargo test` runs from.
    fn dev_content_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/dev")
            .canonicalize()
            .expect("content/dev exists relative to the crate manifest")
    }

    #[test]
    fn load_registers_fixtures_under_expected_keys() {
        let mut registry = SoundRegistry::new();
        registry.load_from_content_root(&dev_content_root());

        // The committed fixtures: a static SFX tone and a streaming music loop.
        assert!(
            registry.contains("sfx/test_tone"),
            "static SFX fixture registers under its content-relative key",
        );
        assert!(
            registry.contains("music/test_loop"),
            "music fixture registers under its content-relative key",
        );
        assert!(
            matches!(registry.get("sfx/test_tone"), Some(LoadedSound::Static(_))),
            "non-music collection loads as static",
        );
        assert!(
            matches!(
                registry.get("music/test_loop"),
                Some(LoadedSound::Streaming { .. })
            ),
            "music collection loads as streaming",
        );
    }

    #[test]
    fn missing_sounds_dir_degrades_to_empty_registry() {
        let mut registry = SoundRegistry::new();
        // A content root with no `_sounds/` subdirectory: warn-free, no panic.
        let no_sounds = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        registry.load_from_content_root(&no_sounds);
        assert_eq!(registry.len(), 0, "absent _sounds/ leaves registry empty");
    }

    #[test]
    fn missing_file_is_not_registered() {
        // A sound key that does not exist on disk must never appear, and looking
        // it up returns None rather than panicking.
        let mut registry = SoundRegistry::new();
        registry.load_from_content_root(&dev_content_root());
        assert!(
            !registry.contains("sfx/does_not_exist"),
            "absent files are never registered",
        );
        assert!(registry.get("sfx/does_not_exist").is_none());
    }

    #[test]
    fn release_clears_then_load_repopulates() {
        let mut registry = SoundRegistry::new();
        registry.load_from_content_root(&dev_content_root());
        let loaded = registry.len();
        assert!(loaded > 0, "fixtures populate the registry");

        registry.clear();
        assert_eq!(registry.len(), 0, "release empties the registry");
        assert!(!registry.contains("sfx/test_tone"));

        registry.load_from_content_root(&dev_content_root());
        assert_eq!(
            registry.len(),
            loaded,
            "a subsequent load repopulates the same entries",
        );
        assert!(registry.contains("sfx/test_tone"));
    }
}
