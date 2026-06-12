// Descriptor-spawn provenance carried by live entities for hot reload planning.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::scripting::registry::ComponentKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DescriptorComponentKind {
    Weapon,
    Movement,
    Light,
    Emitter,
    Mesh,
    Health,
}

impl DescriptorComponentKind {
    pub(crate) const ALL: [Self; 6] = [
        Self::Weapon,
        Self::Movement,
        Self::Light,
        Self::Emitter,
        Self::Mesh,
        Self::Health,
    ];

    pub(crate) fn component_kind(self) -> ComponentKind {
        match self {
            Self::Weapon => ComponentKind::Weapon,
            Self::Movement => ComponentKind::PlayerMovement,
            Self::Light => ComponentKind::Light,
            Self::Emitter => ComponentKind::BillboardEmitter,
            Self::Mesh => ComponentKind::Mesh,
            Self::Health => ComponentKind::Health,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Weapon => "weapon",
            Self::Movement => "movement",
            Self::Light => "light",
            Self::Emitter => "emitter",
            Self::Mesh => "mesh",
            Self::Health => "health",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DescriptorSpawnPath {
    MapPlacement,
    PlayerSpawn,
    DefaultWeapon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DescriptorMapOverride {
    LightInitialIntensity,
    LightInitialRange,
    LightInitialIsDynamic,
    LightInitialColor,
    EmitterInitialRate,
    EmitterInitialSpread,
    EmitterInitialLifetime,
    EmitterInitialBuoyancy,
    EmitterInitialDrag,
    EmitterInitialSpinRate,
    EmitterInitialBurst,
    EmitterInitialSprite,
    EmitterInitialColor,
    EmitterInitialVelocity,
}

impl DescriptorMapOverride {
    /// Returns the map-property key string for this override field.
    ///
    /// `LightInitialColor` and `EmitterInitialColor` intentionally return the same
    /// string (`"initial_color"`). The collision is safe because callers always
    /// filter by component kind first — `overrides_for(component)` in
    /// `DescriptorProvenance` partitions the override set by component before
    /// `key()` is used — so the two variants are never in the same lookup space.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::LightInitialIntensity => "initial_intensity",
            Self::LightInitialRange => "initial_range",
            Self::LightInitialIsDynamic => "initial_is_dynamic",
            Self::LightInitialColor => "initial_color",
            Self::EmitterInitialRate => "initial_rate",
            Self::EmitterInitialSpread => "initial_spread",
            Self::EmitterInitialLifetime => "initial_lifetime",
            Self::EmitterInitialBuoyancy => "initial_buoyancy",
            Self::EmitterInitialDrag => "initial_drag",
            Self::EmitterInitialSpinRate => "initial_spin_rate",
            Self::EmitterInitialBurst => "initial_burst",
            Self::EmitterInitialSprite => "initial_sprite",
            Self::EmitterInitialColor => "initial_color",
            Self::EmitterInitialVelocity => "initial_velocity",
        }
    }

    pub(crate) fn component(self) -> DescriptorComponentKind {
        match self {
            Self::LightInitialIntensity
            | Self::LightInitialRange
            | Self::LightInitialIsDynamic
            | Self::LightInitialColor => DescriptorComponentKind::Light,
            Self::EmitterInitialRate
            | Self::EmitterInitialSpread
            | Self::EmitterInitialLifetime
            | Self::EmitterInitialBuoyancy
            | Self::EmitterInitialDrag
            | Self::EmitterInitialSpinRate
            | Self::EmitterInitialBurst
            | Self::EmitterInitialSprite
            | Self::EmitterInitialColor
            | Self::EmitterInitialVelocity => DescriptorComponentKind::Emitter,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DescriptorProvenance {
    pub(crate) canonical_name: String,
    pub(crate) owned_components: BTreeSet<DescriptorComponentKind>,
    pub(crate) map_overrides: BTreeSet<DescriptorMapOverride>,
    pub(crate) spawn_path: DescriptorSpawnPath,
}

impl DescriptorProvenance {
    pub(crate) fn owns(&self, component: DescriptorComponentKind) -> bool {
        self.owned_components.contains(&component)
    }

    pub(crate) fn overrides_for(
        &self,
        component: DescriptorComponentKind,
    ) -> impl Iterator<Item = DescriptorMapOverride> + '_ {
        self.map_overrides
            .iter()
            .copied()
            .filter(move |field| field.component() == component)
    }
}

pub(crate) fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

pub(crate) fn parse_f32(raw: &str) -> Option<f32> {
    match raw.trim().parse::<f32>() {
        Ok(v) if v.is_finite() => Some(v),
        _ => None,
    }
}

pub(crate) fn parse_nonneg_f32(raw: &str) -> Option<f32> {
    parse_f32(raw).filter(|v| *v >= 0.0)
}

pub(crate) fn parse_vec3(raw: &str) -> Option<[f32; 3]> {
    let mut out = [0.0f32; 3];
    let mut parts = raw.split_whitespace();
    for slot in &mut out {
        let value = parts.next()?.parse::<f32>().ok()?;
        if !value.is_finite() {
            return None;
        }
        *slot = value;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(out)
}
