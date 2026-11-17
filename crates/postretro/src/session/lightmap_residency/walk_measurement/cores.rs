//! Emulated core counts for the residency walks (`POSTRETRO_LIGHTMAP_WALK_CORES`).
//! See: context/lib/rendering_pipeline.md §4

// macOS cannot pin a process to cores, so a "1-core" or "2-core" run sizes
// every pool the walk uses as a machine with that many cores would: today the
// level's shared decode pool, `(cores / 2).clamp(1, 3)`, through the one seam
// `crate::parallelism`. The issuer is always one thread. The threads still
// share this machine's cores, so CPU contention is not emulated; every run
// says it was emulated.

use crate::parallelism::{EmulatedCores, available_parallelism};
use crate::streaming::decode_pool::decode_pool_size;

const CORES_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_CORES";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Cores {
    /// This machine's core count.
    #[default]
    Machine,
    /// Pools sized as on a machine with this many cores.
    Emulated(usize),
}

impl Cores {
    pub(super) fn from_env() -> Self {
        match std::env::var(CORES_ENV) {
            Ok(value) => Self::parse(&value).unwrap_or_else(|why| panic!("{why}")),
            Err(_) => Self::Machine,
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            "" | "default" => Ok(Self::Machine),
            count => count
                .parse::<usize>()
                .ok()
                .filter(|&cores| cores >= 1)
                .map(Self::Emulated)
                .ok_or_else(|| format!("{CORES_ENV} = default | N >= 1, not {value:?}")),
        }
    }

    /// Holds the emulation on this thread, which builds every pool the walk
    /// spawns; `None` for the machine's own count.
    pub(super) fn apply(self) -> Option<EmulatedCores> {
        match self {
            Self::Machine => None,
            Self::Emulated(cores) => Some(EmulatedCores::new(cores)),
        }
    }

    /// The core count pools size from under this setting; call while
    /// [`Self::apply`]'s guard is held.
    pub(super) fn count(self) -> usize {
        match self {
            Self::Machine => available_parallelism(),
            Self::Emulated(cores) => cores,
        }
    }

    /// `emulated 2-core` or `default 12-core`.
    pub(super) fn label(self) -> String {
        match self {
            Self::Machine => format!("default {}-core", self.count()),
            Self::Emulated(cores) => format!("emulated {cores}-core"),
        }
    }

    /// The JSON value: `default` or the emulated count.
    pub(super) fn key(self) -> String {
        match self {
            Self::Machine => "default".to_string(),
            Self::Emulated(cores) => cores.to_string(),
        }
    }

    /// What each pool the walk uses gets under this setting.
    pub(super) fn pools_line(self) -> String {
        format!(
            "{}: 1 issuer thread, {} decode threads (shared by SH and lightmap)",
            self.label(),
            decode_pool_size(self.count()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_and_counts() {
        assert_eq!(Cores::parse("default"), Ok(Cores::Machine));
        assert_eq!(Cores::parse(""), Ok(Cores::Machine));
        assert_eq!(Cores::parse(" 2 "), Ok(Cores::Emulated(2)));
        assert!(Cores::parse("0").is_err());
        assert!(Cores::parse("two").is_err());
    }

    #[test]
    fn emulation_sizes_the_shared_decode_pool_through_the_seam() {
        for (cores, decode_threads) in [(1, 1), (2, 1), (4, 2), (6, 3), (16, 3)] {
            let setting = Cores::Emulated(cores);
            let _guard = setting.apply();
            assert_eq!(available_parallelism(), cores);
            assert_eq!(setting.count(), cores);
            assert_eq!(decode_pool_size(available_parallelism()), decode_threads);
        }
        assert!(Cores::Machine.apply().is_none());
        assert_eq!(
            Cores::Machine.count(),
            std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
        );
    }
}
