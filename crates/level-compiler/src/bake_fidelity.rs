//! Interactive mover-bounce fidelity, and the release intent that fixes it.
//! See: context/lib/build_pipeline.md §Build Cache (Bake fidelity)

/// `--bake-fidelity`: how many bounces a mover re-bakes posed in an
/// interactive build. It wires only mover bounce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BakeFidelity {
    /// Every mover's `light_bounces` capped at 1: each pose bakes bounce one
    /// posed and keeps id 34's later bounces.
    Low,
    /// Each mover's own `light_bounces`, as `--release` bakes.
    High,
}

impl BakeFidelity {
    pub(crate) const FLAG: &'static str = "--bake-fidelity";

    pub(crate) fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "low" => Ok(Self::Low),
            "high" => Ok(Self::High),
            _ => anyhow::bail!("{} must be `low` or `high`, got `{value}`", Self::FLAG),
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::High => "high",
        }
    }

    /// The posed bounces a mover bakes at this fidelity from `own`, its
    /// `light_bounces` clamped to id 34's count: `own` at high, at most 1 at
    /// low. 0 stays 0.
    pub(crate) fn bounces(self, own: u32) -> u32 {
        match self {
            Self::Low => own.min(1),
            Self::High => own,
        }
    }
}

/// What a build bakes, read from the flags the author typed (or the TUI's
/// build mode), never from whether a stage cache opened: an interactive
/// build at a fidelity, or the ship bake, which always bakes high.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BakeMode {
    Interactive(BakeFidelity),
    Release,
}

impl BakeMode {
    /// `--release` bakes high; otherwise the named fidelity, `low` when none
    /// is named. The parser refuses both together before this is asked.
    pub(crate) fn of(release: bool, fidelity: Option<BakeFidelity>) -> Self {
        if release {
            Self::Release
        } else {
            Self::Interactive(fidelity.unwrap_or(BakeFidelity::Low))
        }
    }

    pub(crate) fn fidelity(self) -> BakeFidelity {
        match self {
            Self::Interactive(fidelity) => fidelity,
            Self::Release => BakeFidelity::High,
        }
    }

    /// How the Build Summary names the mode.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Interactive(BakeFidelity::Low) => "low",
            Self::Interactive(BakeFidelity::High) => "high",
            Self::Release => "high (--release)",
        }
    }
}
