//! The drain-cap slider's launch seed, `POSTRETRO_PLAY_DRAIN_CAP_MIB`: parsed
//! in the slider's unit, warned once when malformed, held for the session,
//! and read only by dev-tools builds. Its admission at the seeded cap runs
//! with the other levers in `drain_cap_tests`.
//! See: context/lib/development_guide.md §6.4 · context/lib/testing_guide.md §3

use std::env::VarError;
use std::num::NonZeroU64;
use std::path::Path;

use log::Level;
use postretro_test_log_capture::LogCapture;

use super::drain_cap_tests::{DEV_TOOLS, TEST, TEST_OR_DEV_TOOLS, strip_gated};
use super::*;
use crate::streaming::drain_budget::MAX_INSTALL_DECODED_BYTES_PER_DRAIN;

const MIB: u64 = 1024 * 1024;
const PRODUCTION: PlayDrainCap = PlayDrainCap {
    source: PlayDrainCapSource::Production,
    decoded_bytes_per_drain: MAX_INSTALL_DECODED_BYTES_PER_DRAIN,
};

fn seeded(value: Result<String, VarError>) -> LevelStreaming {
    let mut level = LevelStreaming::default();
    level.apply_play_drain_cap_seed(value);
    level
}

#[test]
fn play_drain_cap_seed_sets_the_cap_in_decoded_mib_and_names_its_source() {
    let capture = LogCapture::start();
    for (value, bytes) in [
        ("4", 4 * MIB),
        (" 16 ", 16 * MIB),
        ("1", MIB),
        ("2.5", 2 * MIB + MIB / 2),
        ("32", 32 * MIB),
    ] {
        let level = seeded(Ok(value.to_string()));
        assert_eq!(
            level.play_drain_cap_override(),
            NonZeroU64::new(bytes),
            "{value}"
        );
        assert_eq!(
            level.play_drain_cap_in_force(),
            PlayDrainCap {
                source: PlayDrainCapSource::Seed,
                decoded_bytes_per_drain: bytes,
            },
            "{value}"
        );
    }
    capture.assert_logged_once(
        Level::Info,
        "play drain cap seeded from POSTRETRO_PLAY_DRAIN_CAP_MIB: 4 MiB (4194304 decoded bytes per drain)",
    );
    capture.assert_not_logged(Level::Warn, PLAY_DRAIN_CAP_SEED_ENV);
}

#[test]
fn malformed_play_drain_cap_seeds_each_warn_once_and_keep_production() {
    let capture = LogCapture::start();
    for (value, why) in [
        ("0", "zero cap"),
        ("-4", "positive"),
        ("abc", "not a number"),
        ("4MiB", "not a number"),
        ("NaN", "not a number"),
        ("1e300", "too large"),
        ("1e-12", "rounds to zero"),
    ] {
        capture.clear();
        let level = seeded(Ok(value.to_string()));
        assert_eq!(level.play_drain_cap_override(), None, "{value}");
        assert_eq!(level.play_drain_cap_in_force(), PRODUCTION, "{value}");
        capture.assert_logged_once(
            Level::Warn,
            &format!("ignoring malformed POSTRETRO_PLAY_DRAIN_CAP_MIB (POSTRETRO_PLAY_DRAIN_CAP_MIB={value:?}: "),
        );
        capture.assert_logged_once(Level::Warn, why);
        capture.assert_logged_once(Level::Warn, "keep the production cap of 8 MiB");
    }
}

// A seed the slider cannot hold would be rewritten on the Streaming tab's
// first draw (egui clamps and snaps an existing value and reports it as
// changed), flipping the cap's source to the slider. It is refused instead.
#[test]
fn play_drain_cap_seeds_outside_the_slider_are_refused_and_keep_production() {
    let capture = LogCapture::start();
    for value in ["0.5", "0.999", "32.5", "64", "2.25", "4.1"] {
        capture.clear();
        let level = seeded(Ok(value.to_string()));
        assert_eq!(level.play_drain_cap_override(), None, "{value}");
        assert_eq!(level.play_drain_cap_in_force(), PRODUCTION, "{value}");
        capture.assert_logged_once(
            Level::Warn,
            &format!("POSTRETRO_PLAY_DRAIN_CAP_MIB={value:?}: outside the slider's range of 1 to 32 MiB in steps of 0.5 MiB"),
        );
        capture.assert_logged_once(Level::Warn, "keep the production cap of 8 MiB");
    }
}

// The seed's accepted set is the slider's own span and step.
#[cfg(feature = "dev-tools")]
#[test]
fn the_seed_range_is_the_slider_range() {
    use super::drain_cap::{SEED_MAX_BYTES, SEED_MIN_BYTES, SEED_STEP_BYTES};

    let lever = LevelStreaming::default().drain_cap_lever();
    let range = lever.slider_range_mib();
    assert_eq!(*range.start() * MIB as f64, SEED_MIN_BYTES as f64);
    assert_eq!(*range.end() * MIB as f64, SEED_MAX_BYTES as f64);
    assert_eq!(
        crate::render::debug_ui::StreamingDrainCapLever::SLIDER_STEP_MIB * MIB as f64,
        SEED_STEP_BYTES as f64
    );
}

#[test]
fn absent_or_empty_play_drain_cap_seed_keeps_production_and_logs_nothing() {
    let capture = LogCapture::start();
    for value in [
        Err(VarError::NotPresent),
        Ok(String::new()),
        Ok("  ".to_string()),
    ] {
        let level = seeded(value);
        assert_eq!(level.play_drain_cap_in_force(), PRODUCTION);
    }
    assert!(capture.records().is_empty(), "an unset seed logs nothing");
}

// The override lives on the session's streaming owner, which a level change
// clears without touching it: the seed holds for every level of the session.
#[test]
fn play_drain_cap_seed_survives_a_level_change() {
    let mut level = seeded(Ok("4".to_string()));
    level.clear(&mut None);
    assert_eq!(
        level.play_drain_cap_in_force(),
        PlayDrainCap {
            source: PlayDrainCapSource::Seed,
            decoded_bytes_per_drain: 4 * MIB,
        }
    );
}

// The slider reads the override the seed set, so it shows the seeded cap at
// launch; a move makes the source the slider, and its reset production.
#[cfg(feature = "dev-tools")]
#[test]
fn drain_cap_slider_shows_the_seeded_cap_at_launch() {
    let mut level = seeded(Ok("4".to_string()));
    let mut lever = level.drain_cap_lever();
    assert_eq!(lever.cap_bytes, Some(4 * MIB));
    assert_eq!(lever.effective_bytes(), 4 * MIB);

    lever.set_cap_mib(6.0);
    level.apply_drain_cap_lever(&lever);
    assert_eq!(
        level.play_drain_cap_in_force(),
        PlayDrainCap {
            source: PlayDrainCapSource::Slider,
            decoded_bytes_per_drain: 6 * MIB,
        }
    );
    lever.reset();
    level.apply_drain_cap_lever(&lever);
    assert_eq!(level.play_drain_cap_in_force(), PRODUCTION);
}

// A build without dev-tools never reads the seed: the launch owner's seed
// call and the variable's read are gated items.
#[test]
fn a_build_without_dev_tools_never_reads_the_play_drain_cap_seed() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/session/level_streaming/drain_cap.rs");
    let source = std::fs::read_to_string(&path).unwrap();
    assert!(source.contains("level.seed_play_drain_cap_from_environment();"));
    assert!(source.contains("std::env::var(PLAY_DRAIN_CAP_SEED_ENV)"));
    let production = strip_gated(
        "drain_cap.rs",
        &source,
        &[DEV_TOOLS, TEST, TEST_OR_DEV_TOOLS],
    );
    assert!(production.contains("pub(crate) fn at_launch() -> Self {"));
    let code: String = production
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("
");
    for read in [
        "seed_play_drain_cap",
        "PLAY_DRAIN_CAP_SEED_ENV",
        "POSTRETRO_PLAY_DRAIN_CAP_MIB",
        "std::env::var",
    ] {
        assert!(!code.contains(read), "ungated `{read}`");
    }
    let session =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/session/mod.rs"))
            .unwrap();
    assert!(
        session.contains("level_streaming: LevelStreaming::at_launch(),"),
        "the session's one streaming owner is the launch owner"
    );
}
