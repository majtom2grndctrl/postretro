//! The budget each drain admits against, and the developer override of the
//! play cap: one seam the dev-tools slider, its launch seed and the walk
//! measurement all set.
//! See: context/lib/rendering_pipeline.md §4 · context/lib/experimental_spikes.md §Tuning levers ·
//! context/lib/development_guide.md §6.4

#[cfg(any(test, feature = "dev-tools"))]
use std::num::NonZeroU64;

use super::LevelStreaming;
use crate::streaming::drain_budget::{
    DrainBytesOverflow, MAX_INSTALL_DECODED_BYTES_PER_DRAIN,
    SETTLING_INSTALL_DECODED_BYTES_PER_DRAIN,
};

/// The dev-tools slider's launch seed, in decoded MiB: the slider's unit.
#[cfg(any(test, feature = "dev-tools"))]
pub(crate) const PLAY_DRAIN_CAP_SEED_ENV: &str = "POSTRETRO_PLAY_DRAIN_CAP_MIB";

#[cfg(any(test, feature = "dev-tools"))]
const MIB: f64 = 1024.0 * 1024.0;

/// Who set the play cap a drain admits against.
#[cfg(any(test, feature = "dev-tools"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum PlayDrainCapSource {
    /// No override: the production constant.
    #[default]
    Production,
    /// [`PLAY_DRAIN_CAP_SEED_ENV`] at launch.
    Seed,
    /// The dev-tools slider, or the walk measurement in tests.
    Slider,
}

/// The playback record's spelling.
#[cfg(feature = "dev-tools")]
impl PlayDrainCapSource {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::Seed => "seed",
            Self::Slider => "slider",
        }
    }
}

/// The play cap in force and who set it, in the budget's own unit.
#[cfg(any(test, feature = "dev-tools"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlayDrainCap {
    pub(crate) source: PlayDrainCapSource,
    pub(crate) decoded_bytes_per_drain: u64,
}

impl LevelStreaming {
    /// The session's streaming owner at launch: the production play cap, or
    /// in a dev-tools build the slider's launch seed. A build without
    /// dev-tools never reads the seed.
    pub(crate) fn at_launch() -> Self {
        #[cfg_attr(not(feature = "dev-tools"), allow(unused_mut))]
        let mut level = Self::default();
        #[cfg(feature = "dev-tools")]
        level.seed_play_drain_cap_from_environment();
        level
    }

    /// Admits the drain's offered items once, against the settling budget
    /// while Settling and the play cap otherwise.
    pub(super) fn admit_offered(&mut self, settling: bool) -> Result<(), DrainBytesOverflow> {
        let budget = if settling {
            SETTLING_INSTALL_DECODED_BYTES_PER_DRAIN
        } else {
            self.play_drain_cap()
        };
        self.drain.admit_within(budget)
    }

    #[cfg(not(any(test, feature = "dev-tools")))]
    fn play_drain_cap(&self) -> u64 {
        MAX_INSTALL_DECODED_BYTES_PER_DRAIN
    }

    #[cfg(any(test, feature = "dev-tools"))]
    fn play_drain_cap(&self) -> u64 {
        self.play_drain_cap_override
            .map_or(MAX_INSTALL_DECODED_BYTES_PER_DRAIN, NonZeroU64::get)
    }

    /// The play cap override; `None` is the production cap.
    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn play_drain_cap_override(&self) -> Option<NonZeroU64> {
        self.play_drain_cap_override
    }

    /// The play cap every later play drain admits against, and its source.
    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn play_drain_cap_in_force(&self) -> PlayDrainCap {
        PlayDrainCap {
            source: if self.play_drain_cap_override.is_some() {
                self.play_drain_cap_source
            } else {
                PlayDrainCapSource::Production
            },
            decoded_bytes_per_drain: self.play_drain_cap(),
        }
    }

    /// Replaces the play cap for every later play drain; `None` restores
    /// production. Settling and the lightmap in-hand caps never read it.
    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn set_play_drain_cap_override(&mut self, cap: Option<NonZeroU64>) {
        self.play_drain_cap_override = cap;
        self.play_drain_cap_source = PlayDrainCapSource::Slider;
    }

    /// Seeds the slider from [`PLAY_DRAIN_CAP_SEED_ENV`]. The override lives
    /// on the session's streaming owner and survives level changes, so the
    /// seed holds for the whole session until the slider moves.
    #[cfg(feature = "dev-tools")]
    pub(crate) fn seed_play_drain_cap_from_environment(&mut self) {
        self.apply_play_drain_cap_seed(std::env::var(PLAY_DRAIN_CAP_SEED_ENV));
    }

    /// Unset or empty leaves production; a malformed value warns once and
    /// leaves production. Never fails the launch.
    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn apply_play_drain_cap_seed(&mut self, value: Result<String, std::env::VarError>) {
        let value = match value {
            Ok(value) if value.trim().is_empty() => return,
            Ok(value) => value,
            Err(std::env::VarError::NotPresent) => return,
            Err(std::env::VarError::NotUnicode(raw)) => {
                warn_malformed_seed(&format!(
                    "{PLAY_DRAIN_CAP_SEED_ENV}={raw:?}: not valid Unicode"
                ));
                return;
            }
        };
        match parse_cap_mib(
            PLAY_DRAIN_CAP_SEED_ENV,
            &value,
            "a zero cap admits only each drain's first item; unset it for production",
        ) {
            Ok((mib, bytes)) => {
                self.play_drain_cap_override = Some(bytes);
                self.play_drain_cap_source = PlayDrainCapSource::Seed;
                log::info!(
                    "[Streaming] play drain cap seeded from {PLAY_DRAIN_CAP_SEED_ENV}: {mib} MiB ({} decoded bytes per drain)",
                    bytes.get(),
                );
            }
            Err(why) => warn_malformed_seed(&why),
        }
    }
}

#[cfg(any(test, feature = "dev-tools"))]
fn warn_malformed_seed(why: &str) {
    log::warn!(
        "[Streaming] ignoring malformed {PLAY_DRAIN_CAP_SEED_ENV} ({why}); play drains keep the production cap of {} MiB",
        MAX_INSTALL_DECODED_BYTES_PER_DRAIN as f64 / MIB,
    );
}

/// `env`'s value as a positive decimal count of MiB (`4`, `16`, `0.5`) and
/// its whole bytes. Zero (refused with `zero_why`), a negative, a non-number,
/// a value past `u64` bytes and one rounding to zero bytes are refused by
/// name, never clamped. The slider's seed and the walk's levers share it.
#[cfg(any(test, feature = "dev-tools"))]
pub(crate) fn parse_cap_mib(
    env: &str,
    value: &str,
    zero_why: &str,
) -> Result<(f64, NonZeroU64), String> {
    let refuse = |why: &str| format!("{env}={value:?}: {why}");
    let mib: f64 = value
        .trim()
        .parse()
        .ok()
        .filter(|mib: &f64| mib.is_finite())
        .ok_or_else(|| refuse("not a number of MiB"))?;
    if mib < 0.0 {
        return Err(refuse("a cap is a positive number of MiB"));
    }
    if mib == 0.0 {
        return Err(refuse(zero_why));
    }
    let bytes = (mib * MIB).round();
    if bytes >= u64::MAX as f64 {
        return Err(refuse("too large to represent in bytes"));
    }
    let bytes = NonZeroU64::new(bytes as u64).ok_or_else(|| refuse("rounds to zero bytes"))?;
    Ok((mib, bytes))
}

/// The dev-tools Streaming tab's view of the override.
#[cfg(feature = "dev-tools")]
impl LevelStreaming {
    pub(crate) fn drain_cap_lever(&self) -> crate::render::debug_ui::StreamingDrainCapLever {
        crate::render::debug_ui::StreamingDrainCapLever {
            cap_bytes: self.play_drain_cap_override().map(NonZeroU64::get),
            production_bytes: MAX_INSTALL_DECODED_BYTES_PER_DRAIN,
        }
    }

    pub(crate) fn apply_drain_cap_lever(
        &mut self,
        lever: &crate::render::debug_ui::StreamingDrainCapLever,
    ) {
        self.set_play_drain_cap_override(lever.cap_bytes.and_then(NonZeroU64::new));
    }
}
