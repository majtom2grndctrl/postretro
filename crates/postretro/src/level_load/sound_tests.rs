//! Sound decode on the pool: the files register in sorted path order
//! whatever order the pool decodes them, the later path wins a shared key,
//! and a bad file, a missing directory and an empty one each degrade exactly
//! as the oracle's `load_level_sounds` does, never failing the load.
//! See: context/plans/done/level-load-pipeline/index.md (Equivalence)

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use postretro_test_log_capture::LogCapture;

use super::test_support::{inputs_for, run_prep, test_pool, write_textured_level};

/// A mono 16-bit PCM WAV of `frames` silent frames at 8 kHz.
fn wav_bytes(frames: u32) -> Vec<u8> {
    let data_len = frames * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&8000u32.to_le_bytes());
    bytes.extend_from_slice(&16000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    bytes.resize(bytes.len() + data_len as usize, 0);
    bytes
}

fn dev_sounds() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev/sounds")
}

/// A content root whose `sounds/` holds a shared key (`sfx/dup` from an
/// `.ogg` and a `.wav`), nested clips and a music track: more files than
/// the pool's width, so a reordered decode really finishes out of order.
fn mixed_content_root() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let sounds = root.path().join("sounds");
    for dir in ["sfx/nested", "music", "ui"] {
        std::fs::create_dir_all(sounds.join(dir)).unwrap();
    }
    std::fs::copy(
        dev_sounds().join("fixtures/jump.ogg"),
        sounds.join("sfx/dup.ogg"),
    )
    .unwrap();
    std::fs::write(sounds.join("sfx/dup.wav"), wav_bytes(40)).unwrap();
    std::fs::write(sounds.join("sfx/nested/a.wav"), wav_bytes(10)).unwrap();
    std::fs::write(sounds.join("sfx/nested/b.wav"), wav_bytes(20)).unwrap();
    std::fs::write(sounds.join("ui/click.wav"), wav_bytes(30)).unwrap();
    std::fs::copy(
        dev_sounds().join("music/test_loop.ogg"),
        sounds.join("music/loop.ogg"),
    )
    .unwrap();
    root
}

/// Prep the textured fixture with `content_root` as the mod's root, and
/// return its decoded sounds.
fn prep_sounds(
    content_root: &Path,
    sound_decode_order: Option<fn(&mut [usize])>,
) -> postretro_audio::DecodedLevelSounds {
    let (level, prm_root) = write_textured_level();
    let pool = test_pool();
    let mut inputs = inputs_for(level.path(), prm_root.path());
    inputs.content_root = content_root.to_path_buf();
    inputs.hooks.sound_decode_order = sound_decode_order;
    let (_prep, outcome, _progress) = run_prep(&pool, inputs);
    let prepared =
        outcome.unwrap_or_else(|failure| panic!("sound decode failed a load: {failure}"));
    prepared
        .presentation
        .sounds
        .expect("a dispatch with audio decodes sounds")
}

/// The warnings registration logs, in order, on this thread.
fn registration_warnings(
    sounds: postretro_audio::DecodedLevelSounds,
) -> (Vec<String>, Vec<String>) {
    let capture = LogCapture::start();
    let registry = sounds.registry_summary();
    let warnings = capture
        .records()
        .into_iter()
        .filter(|record| record.level == log::Level::Warn)
        .map(|record| record.message)
        .collect();
    (registry, warnings)
}

static REORDERED_FILES: AtomicUsize = AtomicUsize::new(0);

/// Decode back to front: the last path finishes first.
fn reverse(order: &mut [usize]) {
    REORDERED_FILES.store(order.len(), Ordering::Release);
    order.reverse();
}

// Equivalence 4: the pool decodes the files back to front, and the set
// still registers in sorted path order, identical to the oracle's
// `load_from_content_root` (decode then register in sequence): the same
// files in the same order, the same registry, and the sorted-later `.wav`
// winning the shared key with the same overwrite warning.
#[test]
fn pooled_sound_decode_registers_in_path_order() {
    let root = mixed_content_root();
    let pooled = prep_sounds(root.path(), Some(reverse));
    assert_eq!(
        REORDERED_FILES.load(Ordering::Acquire),
        6,
        "the pool decoded all six files, back to front"
    );
    let oracle = postretro_audio::decode_level_sounds(root.path());

    let summary = pooled.summary();
    assert_eq!(summary, oracle.summary());
    let paths: Vec<&str> = summary
        .iter()
        .map(|line| line.split(" -> ").next().unwrap())
        .collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted, "registration order is sorted path order");

    let (registry, warnings) = registration_warnings(pooled);
    let (oracle_registry, oracle_warnings) = registration_warnings(oracle);
    assert_eq!(registry, oracle_registry);
    assert_eq!(warnings, oracle_warnings);
    assert!(
        registry.contains(&"sfx/dup: static 40 frames @ 8000 Hz".to_string()),
        "the sorted-later `.wav` wins the shared key: {registry:?}"
    );
    assert!(
        registry
            .iter()
            .any(|line| line.starts_with("music/loop: streaming")),
        "{registry:?}"
    );
    assert_eq!(registry.len(), 5, "{registry:?}");
    assert!(
        warnings.len() == 1 && warnings[0].contains("registry key 'sfx/dup' already occupied"),
        "{warnings:?}"
    );
}

// A bad file warns and is skipped; a missing or an empty `sounds/` decodes
// to no sounds. Each matches the oracle's registry and warnings, and none
// fails the load.
#[test]
fn pooled_sound_decode_degrades_like_the_oracle() {
    let bad = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(bad.path().join("sounds/sfx")).unwrap();
    std::fs::write(bad.path().join("sounds/sfx/broken.wav"), b"not a wav").unwrap();
    std::fs::write(bad.path().join("sounds/sfx/fine.wav"), wav_bytes(12)).unwrap();
    let empty = tempfile::tempdir().unwrap();
    std::fs::create_dir(empty.path().join("sounds")).unwrap();
    let missing = tempfile::tempdir().unwrap();

    for (label, root, expected) in [
        ("bad file", bad.path(), 1),
        ("empty directory", empty.path(), 0),
        ("missing directory", missing.path(), 0),
    ] {
        let pooled = prep_sounds(root, None);
        let oracle = postretro_audio::decode_level_sounds(root);
        assert_eq!(pooled.summary(), oracle.summary(), "{label}");
        let (registry, warnings) = registration_warnings(pooled);
        let (oracle_registry, oracle_warnings) = registration_warnings(oracle);
        assert_eq!(registry, oracle_registry, "{label}");
        assert_eq!(warnings, oracle_warnings, "{label}");
        assert_eq!(registry.len(), expected, "{label}: {registry:?}");
        if label == "bad file" {
            assert!(
                warnings.len() == 1 && warnings[0].contains("cannot decode sound"),
                "{label}: {warnings:?}"
            );
        } else {
            assert!(warnings.is_empty(), "{label}: {warnings:?}");
        }
    }
}

// A dispatch with no audio decodes nothing, as the install loads no sounds
// without it, and the load still prepares.
#[test]
fn a_dispatch_without_audio_decodes_no_sounds() {
    let root = mixed_content_root();
    let (level, prm_root) = write_textured_level();
    let pool = test_pool();
    let mut inputs = inputs_for(level.path(), prm_root.path());
    inputs.content_root = root.path().to_path_buf();
    inputs.audio_enabled = false;
    let (prep, outcome, _progress) = run_prep(&pool, inputs);
    let prepared = outcome.expect("a load without audio prepares");
    assert!(prepared.presentation.sounds.is_none());
    let state = prep.probe().state();
    assert!(state.finished.contains(&super::PrepKind::Sounds));
    assert!(!state.started.contains(&super::PrepKind::Sounds));
}
