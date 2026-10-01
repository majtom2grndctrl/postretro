// Graphics-quality tiers and their live `options.*` slot vocabulary.
// See: context/lib/player_options.md §4

use serde::{Deserialize, Serialize};

/// Spot-shadow allocation tier. Changes persist on settle but apply only when
/// the renderer performs a full initialization or installs the next level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowQuality {
    Low,
    Medium,
    #[default]
    High,
}

impl ShadowQuality {
    pub(super) fn slot_value(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub(super) fn from_slot_value(value: &str) -> Option<Self> {
        match value {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

/// Volumetric-fog ray-march density tier. Smaller step sizes produce a denser,
/// higher-quality march and can be applied live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FogQuality {
    Low,
    #[default]
    Medium,
    High,
}

impl FogQuality {
    pub(super) fn slot_value(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub(super) fn from_slot_value(value: &str) -> Option<Self> {
        match value {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

/// Surface Depth (texel-space parallax) on/off switch. Applies live: the
/// renderer rewrites every installed material's uniform buffer, with no level
/// reload.
///
/// Off/on rather than the low/medium/high used by shadows and fog, because this
/// is a pure cost lever rather than a quality ladder — design D5. The middle
/// tier this replaces never changed the carve DEPTH (it only capped the march
/// budget and shortened the fade), so it read as identical to the full effect
/// except at grazing angles, where it read as a shallower carve.
///
/// Defaults to `On`. The feature ships enabled; this setting exists as an
/// escape hatch for hardware that struggles, not as an opt-in.
///
/// **Stale persisted values.** `settings.toml` files written before the
/// collapse carry `"low"` or `"high"` — `"high"` being what every save wrote,
/// since it was the default. Both are accepted as `On` through serde aliases
/// and rewritten as `"on"` on the next save. This is not a compatibility shim
/// for a code API (see `development_guide.md` §1.6): a settings file is PLAYER
/// DATA, and treating the retired names as unrecognized would fall this field
/// back to its default and keep the stale text in the file indefinitely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceDepthQuality {
    /// Force flat — byte-identical to the pre-Surface-Depth render, zero cost.
    Off,
    /// The full effect: every material's per-prefix values verbatim, with the
    /// full self-shadow budget.
    #[default]
    #[serde(alias = "low", alias = "high")]
    On,
}

impl SurfaceDepthQuality {
    /// The live slot vocabulary, which is deliberately only the two current
    /// values: the retired names are tolerated when READING a settings file,
    /// never as a script-writable state.
    pub(super) fn slot_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
        }
    }

    pub(super) fn from_slot_value(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "on" => Some(Self::On),
            _ => None,
        }
    }
}

/// Scene render resolution: an integer divisor of the surface, or Auto. Applies
/// live and persists; never a per-map key. The render-profile chokepoint
/// translates it, Auto's row cap included, into renderer vocabulary.
///
/// TOML and slot values are the same snake_case names: `auto`, `native`,
/// `half`, `third`, `quarter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderResolution {
    /// Logical resolution on HiDPI, native on 1× displays up to the row cap.
    #[default]
    Auto,
    Native,
    Half,
    Third,
    Quarter,
}

impl RenderResolution {
    pub(super) fn slot_value(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Native => "native",
            Self::Half => "half",
            Self::Third => "third",
            Self::Quarter => "quarter",
        }
    }

    pub(super) fn from_slot_value(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "native" => Some(Self::Native),
            "half" => Some(Self::Half),
            "third" => Some(Self::Third),
            "quarter" => Some(Self::Quarter),
            _ => None,
        }
    }
}
