// Sound asset loading and the per-level sound registry. Resolves paths under
// `content/<mod>/sounds/`, decodes SFX in memory (static) and probes music for
// streaming, and holds the result keyed by content-relative name. Consumed by
// `play`. Loading splits into a pure, `Send` decode half a level load may run
// off the main thread, and a main-thread registration half that registers in
// sorted path order. Load and decode failures degrade gracefully: a missing or
// undecodable file logs an `[Audio]` warning, is skipped, and never panics.
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
const SOUNDS_DIR: &str = "sounds";

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
    /// data is read by `play` (and by tests via `matches!`). Boxed because
    /// `StaticSoundData` (with its settings) is far larger than the `Streaming`
    /// variant — keeps the enum small (clippy::large_enum_variant).
    Static(Box<StaticSoundData>),
    /// Stream-from-disk track; re-opened from this path for each playback.
    Streaming { path: PathBuf },
}

impl LoadedSound {
    /// Re-open a streaming entry for playback. Returns `None` (with an `[Audio]`
    /// warning) if the file became unreadable since load. Static entries return
    /// `None` — callers play those by cloning the `StaticSoundData` directly.
    /// Consumed by `play`.
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
/// name (the path under `sounds/` with the extension stripped, e.g.
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
    /// skipped). Consumed by `play`.
    pub(crate) fn get(&self, key: &str) -> Option<&LoadedSound> {
        self.sounds.get(key)
    }

    /// Register an in-memory clip under `key`, for tests that synthesize their
    /// own signal (a tone at a chosen pitch and level) instead of a fixture.
    #[cfg(test)]
    pub(crate) fn insert_static_for_test(&mut self, key: &str, data: StaticSoundData) {
        self.sounds
            .insert(key.to_string(), LoadedSound::Static(Box::new(data)));
    }

    /// Drop every registered sound. Called at level unload; the registry returns
    /// to its empty state and can be repopulated by a subsequent load.
    pub(crate) fn clear(&mut self) {
        self.sounds.clear();
    }

    /// Scan `<content_root>/sounds/` and register every decodable `.ogg`/`.wav`
    /// file found, replacing any previously registered sounds. Files under the
    /// top-level `music/` collection load as streaming; all others decode into
    /// memory as static. A missing `sounds/` directory is not an error (the mod
    /// simply ships no sounds). Individual unreadable or undecodable files log an
    /// `[Audio]` warning, are skipped, and never panic.
    ///
    /// The two halves in sequence: [`decode_level_sounds`], which a load may
    /// run off the main thread, then [`Self::register_decoded`].
    pub(crate) fn load_from_content_root(&mut self, content_root: &Path) {
        self.register_decoded(decode_level_sounds(content_root));
    }

    /// Replace the registry's sounds with a decoded set, registering its files
    /// in sorted path order whatever order they were decoded in: a later path
    /// wins a shared key, with a warning. Every warning the decode deferred
    /// (an unreadable directory, a file outside the root, an undecodable file)
    /// logs here, in the order the scan and the files would have raised it.
    pub(crate) fn register_decoded(&mut self, decoded: DecodedLevelSounds) {
        self.clear();

        let DecodedLevelSounds {
            sounds_root,
            files,
            scan_warnings,
        } = decoded;
        let Some(files) = files else {
            // No sound assets shipped with this mod — not an error.
            log::info!(
                "[Audio] no sound directory at {} — level has no sounds",
                sounds_root.display(),
            );
            return;
        };
        for warning in scan_warnings {
            log::warn!("{warning}");
        }

        for file in files {
            self.register_file(file);
        }

        log::info!(
            "[Audio] loaded {} sound(s) from {}",
            self.sounds.len(),
            sounds_root.display(),
        );
    }

    /// Register one decoded file. Failures warn and skip.
    fn register_file(&mut self, file: DecodedSoundFile) {
        let DecodedSoundFile { path, outcome, .. } = file;
        match outcome {
            DecodeOutcome::OutsideRoot => {
                log::warn!(
                    "[Audio] sound '{}' is outside the sounds root — skipped",
                    path.display(),
                );
            }
            DecodeOutcome::Failed {
                streaming: true,
                error,
            } => {
                log::warn!(
                    "[Audio] cannot decode streaming sound '{}': {error} — skipped",
                    path.display(),
                );
            }
            DecodeOutcome::Failed {
                streaming: false,
                error,
            } => {
                log::warn!(
                    "[Audio] cannot decode sound '{}': {error} — skipped",
                    path.display(),
                );
            }
            DecodeOutcome::Decoded { key, sound } => {
                if self.sounds.contains_key(&key) {
                    log::warn!(
                        "[Audio] registry key '{}' already occupied — overwriting with '{}'",
                        key,
                        path.display(),
                    );
                }
                self.sounds.insert(key, sound);
            }
        }
    }

    /// One line per registered sound, sorted by key: its key, its kind, and
    /// what identifies the file that won it (the path of a streaming entry,
    /// the frame count and rate of a static one).
    pub(crate) fn summary(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .sounds
            .iter()
            .map(|(key, sound)| format!("{key}: {}", sound_summary(sound)))
            .collect();
        lines.sort();
        lines
    }
}

/// A registered sound's kind and what identifies the file it came from.
fn sound_summary(sound: &LoadedSound) -> String {
    match sound {
        LoadedSound::Static(data) => format!(
            "static {} frames @ {} Hz",
            data.num_frames(),
            data.sample_rate
        ),
        LoadedSound::Streaming { path } => format!("streaming {}", path.display()),
    }
}

/// The sound files of one level's `sounds/` directory, found and sorted but
/// not yet decoded: the first step of the decode half. Pure and `Send`, so a
/// load may decode its files on any thread, in any order.
#[derive(Debug)]
pub struct LevelSoundScan {
    sounds_root: PathBuf,
    /// Sorted. `None` when the directory does not exist.
    files: Option<Vec<PathBuf>>,
    /// Each unreadable subdirectory's warning, logged at registration.
    scan_warnings: Vec<String>,
}

/// One scanned file, decoded (or found unusable). Opaque: no kira type
/// crosses the crate's surface.
pub struct DecodedSoundFile {
    /// The file's position in its scan's sorted list.
    index: usize,
    path: PathBuf,
    outcome: DecodeOutcome,
}

enum DecodeOutcome {
    OutsideRoot,
    Decoded { key: String, sound: LoadedSound },
    Failed { streaming: bool, error: String },
}

/// One level's sounds, decoded and in sorted path order, waiting for the
/// main thread's registration. Opaque: no kira type crosses the crate's
/// surface. A missing or empty directory and an undecodable file are all
/// ordinary values here, never errors.
pub struct DecodedLevelSounds {
    sounds_root: PathBuf,
    /// Sorted by path. `None` when the directory does not exist.
    files: Option<Vec<DecodedSoundFile>>,
    scan_warnings: Vec<String>,
}

impl std::fmt::Debug for DecodedLevelSounds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodedLevelSounds")
            .field("sounds_root", &self.sounds_root)
            .field("files", &self.files.as_ref().map(Vec::len))
            .finish_non_exhaustive()
    }
}

// Compile-time guard: the decode half runs on the engine's load pool and
// hands its products to the main thread, so kira's decoded sound data and
// every decode-half product must be `Send`.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<StaticSoundData>();
    assert_send::<LoadedSound>();
    assert_send::<LevelSoundScan>();
    assert_send::<DecodedSoundFile>();
    assert_send::<DecodedLevelSounds>();
};

/// Find and sort every `.ogg`/`.wav` file under `<content_root>/sounds/`.
/// Logs nothing: the warnings wait for registration.
pub fn scan_level_sounds(content_root: &Path) -> LevelSoundScan {
    let sounds_root = content_root.join(SOUNDS_DIR);
    if !sounds_root.is_dir() {
        return LevelSoundScan {
            sounds_root,
            files: None,
            scan_warnings: Vec::new(),
        };
    }
    let mut files: Vec<PathBuf> = Vec::new();
    let mut scan_warnings = Vec::new();
    collect_sound_files(&sounds_root, &mut files, &mut scan_warnings);
    // Deterministic registration order so logs and any future tie-breaking
    // are reproducible across runs (directory iteration order is not).
    files.sort();
    LevelSoundScan {
        sounds_root,
        files: Some(files),
        scan_warnings,
    }
}

/// The decode half in full: scan, then decode every file in sorted order.
/// `Audio::register_level_sounds` registers the result.
pub fn decode_level_sounds(content_root: &Path) -> DecodedLevelSounds {
    let scan = scan_level_sounds(content_root);
    let decoded = (0..scan.file_count())
        .map(|index| scan.decode_file(index))
        .collect();
    scan.finish(decoded)
}

impl LevelSoundScan {
    /// How many files the scan found.
    pub fn file_count(&self) -> usize {
        self.files.as_ref().map_or(0, Vec::len)
    }

    /// Decode the scan's `index`th file (in sorted order): a static clip
    /// decoded into memory, or a `music/` track probe-opened for streaming.
    /// Never fails; an unusable file comes back as the warning registration
    /// will log. Panics if `index` is not below [`Self::file_count`].
    pub fn decode_file(&self, index: usize) -> DecodedSoundFile {
        let path = &self.files.as_ref().expect("a scan with files")[index];
        let outcome = match registry_key(&self.sounds_root, path) {
            None => DecodeOutcome::OutsideRoot,
            Some(key) if is_streaming(&key) => {
                // Probe-decode so an undecodable music file degrades at load
                // time; the playable entry re-opens the path per playback.
                // Re-opening does blocking disk I/O on the calling thread;
                // acceptable at level-load time but revisit if streaming
                // assets are loaded mid-frame.
                match StreamingSoundData::from_file(path) {
                    Ok(_probe) => DecodeOutcome::Decoded {
                        key,
                        sound: LoadedSound::Streaming { path: path.clone() },
                    },
                    Err(err) => DecodeOutcome::Failed {
                        streaming: true,
                        error: err.to_string(),
                    },
                }
            }
            Some(key) => match StaticSoundData::from_file(path) {
                Ok(data) => DecodeOutcome::Decoded {
                    key,
                    sound: LoadedSound::Static(Box::new(data)),
                },
                Err(err) => DecodeOutcome::Failed {
                    streaming: false,
                    error: err.to_string(),
                },
            },
        };
        DecodedSoundFile {
            index,
            path: path.clone(),
            outcome,
        }
    }

    /// Gather the decoded files, in whatever order they finished, back into
    /// the scan's sorted path order. A file decoded twice keeps one result;
    /// one never decoded is left out.
    pub fn finish(self, mut decoded: Vec<DecodedSoundFile>) -> DecodedLevelSounds {
        decoded.sort_by_key(|file| file.index);
        decoded.dedup_by_key(|file| file.index);
        DecodedLevelSounds {
            sounds_root: self.sounds_root,
            files: self.files.map(|_| decoded),
            scan_warnings: self.scan_warnings,
        }
    }
}

impl DecodedSoundFile {
    /// The file's position in its scan's sorted list.
    pub fn index(&self) -> usize {
        self.index
    }
}

impl DecodedLevelSounds {
    /// How many files the set holds, decodable or not.
    pub fn file_count(&self) -> usize {
        self.files.as_ref().map_or(0, Vec::len)
    }

    /// The scan's warnings, then one line per file in registration order
    /// (its path and what decoding made of it): a comparable view of the set
    /// for tests and diagnostics.
    pub fn summary(&self) -> Vec<String> {
        let Some(files) = &self.files else {
            return vec![format!(
                "no sound directory at {}",
                self.sounds_root.display()
            )];
        };
        let mut lines = self.scan_warnings.clone();
        lines.extend(files.iter().map(|file| {
            let outcome = match &file.outcome {
                DecodeOutcome::OutsideRoot => "outside the sounds root".to_string(),
                DecodeOutcome::Failed { streaming, error } => {
                    format!("undecodable (streaming: {streaming}): {error}")
                }
                DecodeOutcome::Decoded { key, sound } => {
                    format!("{key}: {}", sound_summary(sound))
                }
            };
            format!("{} -> {outcome}", file.path.display())
        }));
        lines
    }

    /// What registering the set leaves in a registry, one line per key,
    /// sorted: registration's own code on a scratch registry, its warnings
    /// included. For tests and diagnostics on a machine with no audio device.
    pub fn registry_summary(self) -> Vec<String> {
        let mut registry = SoundRegistry::new();
        registry.register_decoded(self);
        registry.summary()
    }
}

// Whether a registry key belongs to the streaming collection. The rule is a
/// simple first-segment match against `music/`; subdirectories under it (e.g.
/// `music/ambient/track`) stream too.
fn is_streaming(key: &str) -> bool {
    key.split('/').next() == Some(STREAMING_COLLECTION)
}

/// Map an absolute sound path to its content-relative registry key: the path
/// relative to `sounds/`, with the extension stripped and separators
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
/// subdirectories are skipped rather than aborting the scan; each one's
/// warning goes to `warnings`, for registration to log.
fn collect_sound_files(dir: &Path, out: &mut Vec<PathBuf>, warnings: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            warnings.push(format!(
                "[Audio] cannot read sound directory {}: {err} — skipped",
                dir.display(),
            ));
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sound_files(&path, out, warnings);
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
        let root = Path::new("/content/dev/sounds");
        let path = Path::new("/content/dev/sounds/sfx/test_tone.wav");
        assert_eq!(registry_key(root, path).as_deref(), Some("sfx/test_tone"));
    }

    #[test]
    fn registry_key_rejects_path_outside_root() {
        let root = Path::new("/content/dev/sounds");
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

        // The committed fixtures cover both decoders and both load paths: a
        // static WAV tone, a static Ogg Vorbis placeholder, and a streaming Ogg
        // Vorbis music loop.
        assert!(
            matches!(registry.get("fixtures/jump"), Some(LoadedSound::Static(_))),
            "a static Ogg Vorbis fixture decodes and registers",
        );
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
        // A content root with no `sounds/` subdirectory: warn-free, no panic.
        let no_sounds = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        registry.load_from_content_root(&no_sounds);
        assert_eq!(registry.len(), 0, "absent sounds/ leaves registry empty");
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

    /// A mono 16-bit PCM WAV of `frames` silent frames at 8 kHz.
    fn wav_bytes(frames: u32) -> Vec<u8> {
        let data_len = frames * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&8000u32.to_le_bytes());
        bytes.extend_from_slice(&16000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        bytes.resize(bytes.len() + data_len as usize, 0);
        bytes
    }

    /// A fresh scratch directory under the system temp dir, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "postretro_audio_{label}_{}_{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A content root whose `sounds/` holds a shared key (`sfx/dup` from a
    /// `.ogg` and a `.wav`), an undecodable file, a nested clip and a music
    /// track.
    fn mixed_content_root() -> Scratch {
        let root = Scratch::new("mixed");
        let sounds = root.path().join(SOUNDS_DIR);
        for dir in ["sfx/nested", "music"] {
            std::fs::create_dir_all(sounds.join(dir)).unwrap();
        }
        let dev = dev_content_root().join(SOUNDS_DIR);
        std::fs::copy(dev.join("fixtures/jump.ogg"), sounds.join("sfx/dup.ogg")).unwrap();
        std::fs::write(sounds.join("sfx/dup.wav"), wav_bytes(40)).unwrap();
        std::fs::write(sounds.join("sfx/broken.wav"), b"not a wav").unwrap();
        std::fs::write(sounds.join("sfx/nested/a.wav"), wav_bytes(10)).unwrap();
        std::fs::copy(
            dev.join("music/test_loop.ogg"),
            sounds.join("music/loop.ogg"),
        )
        .unwrap();
        root
    }

    #[test]
    fn decode_then_register_matches_load_from_content_root() {
        let mixed = mixed_content_root();
        for root in [dev_content_root(), mixed.path().to_path_buf()] {
            let mut loaded = SoundRegistry::new();
            loaded.load_from_content_root(&root);
            let mut registered = SoundRegistry::new();
            registered.register_decoded(decode_level_sounds(&root));
            assert_eq!(registered.summary(), loaded.summary(), "{}", root.display());
            assert!(!loaded.summary().is_empty());
        }
    }

    // Registration follows the scan's sorted path order, never the order the
    // files finished decoding: the later path wins a shared key.
    #[test]
    fn registration_order_is_sorted_path_order_whatever_the_decode_order() {
        let root = mixed_content_root();
        let in_order = decode_level_sounds(root.path());
        let scan = scan_level_sounds(root.path());
        assert_eq!(scan.file_count(), 5);
        let reversed: Vec<DecodedSoundFile> = (0..scan.file_count())
            .rev()
            .map(|index| scan.decode_file(index))
            .collect();
        assert_eq!(reversed[0].index(), 4);
        let finished = scan.finish(reversed);
        assert_eq!(finished.summary(), in_order.summary());
        let registry = finished.registry_summary();
        assert_eq!(registry, in_order.registry_summary());
        assert!(
            registry.contains(&"sfx/dup: static 40 frames @ 8000 Hz".to_string()),
            "the sorted-later `.wav` wins the shared key: {registry:?}"
        );
        assert!(
            !registry.iter().any(|line| line.starts_with("sfx/broken")),
            "an undecodable file is skipped: {registry:?}"
        );
        assert_eq!(registry.len(), 3, "{registry:?}");
    }

    #[test]
    fn empty_and_missing_sound_directories_decode_to_empty_sets() {
        let empty = Scratch::new("empty");
        std::fs::create_dir(empty.path().join(SOUNDS_DIR)).unwrap();
        let decoded = decode_level_sounds(empty.path());
        assert_eq!(decoded.file_count(), 0);
        assert!(decoded.summary().is_empty());
        assert!(decoded.registry_summary().is_empty());

        let missing = Scratch::new("missing");
        let decoded = decode_level_sounds(missing.path());
        assert_eq!(decoded.file_count(), 0);
        assert!(decoded.summary()[0].starts_with("no sound directory"));
        assert!(decoded.registry_summary().is_empty());
    }
}
