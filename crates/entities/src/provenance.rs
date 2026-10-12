// Descriptor-spawn provenance carried by live entities for hot reload planning.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::registry::ComponentKind;

// Keep the enum and iteration inventory together: adding a kind must reach
// every exhaustive consumer, including the replication-role classification.
macro_rules! descriptor_component_kinds {
    ($($kind:ident),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum DescriptorComponentKind {
            $($kind),+
        }

        impl DescriptorComponentKind {
            pub const ALL: [Self; [$(stringify!($kind)),+].len()] = [$(Self::$kind),+];
        }
    };
}

descriptor_component_kinds! {
    Weapon,
    Movement,
    Light,
    Emitter,
    Mesh,
    Health,
    Touchable,
}

/// How a descriptor component participates in entity replication.
/// Behavior is classified separately because it is not a descriptor component kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicationRole {
    NetworksEntity,
    ClientLocalPresentation,
    PawnPath,
    HeldOrPawn,
}

impl DescriptorComponentKind {
    /// One exhaustive policy shared by descriptor and live-entity classifiers.
    #[deny(clippy::wildcard_enum_match_arm)]
    pub fn replication_role(self) -> ReplicationRole {
        match self {
            Self::Mesh | Self::Health | Self::Touchable => ReplicationRole::NetworksEntity,
            Self::Light | Self::Emitter => ReplicationRole::ClientLocalPresentation,
            Self::Movement => ReplicationRole::PawnPath,
            Self::Weapon => ReplicationRole::HeldOrPawn,
        }
    }

    pub fn component_kind(self) -> ComponentKind {
        match self {
            Self::Weapon => ComponentKind::Weapon,
            Self::Movement => ComponentKind::PlayerMovement,
            Self::Light => ComponentKind::Light,
            Self::Emitter => ComponentKind::BillboardEmitter,
            Self::Mesh => ComponentKind::Mesh,
            Self::Health => ComponentKind::Health,
            Self::Touchable => ComponentKind::Touchable,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Weapon => "weapon",
            Self::Movement => "movement",
            Self::Light => "light",
            Self::Emitter => "emitter",
            Self::Mesh => "mesh",
            Self::Health => "health",
            Self::Touchable => "touchable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DescriptorSpawnPath {
    MapPlacement,
    /// An engine-owned `entity_spawner` materialized this descriptor during a
    /// fixed tick. Runtime spawns remain local until the replication layer
    /// explicitly registers them.
    RuntimeSpawn,
    PlayerSpawn,
    DefaultWeapon,
    /// A networked co-op slot pawn materialized on client accept (M15 Phase 3).
    /// Distinguishes a host-authoritative remote player pawn from a map-start
    /// single-player spawn: it is descriptor-backed like `PlayerSpawn` but is NOT
    /// marked the local player and carries no global `active_wieldable`.
    NetworkSlot,
    /// A host-created visual flight entity for a weapon descriptor's projectile.
    /// It has only Transform/provenance plus presentation components; no gameplay
    /// projectile state is materialized on a remote client.
    ProjectilePresentation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DescriptorMapOverride {
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
    pub fn key(self) -> &'static str {
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

    pub fn component(self) -> DescriptorComponentKind {
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
pub struct DescriptorProvenance {
    pub canonical_name: String,
    pub owned_components: BTreeSet<DescriptorComponentKind>,
    pub map_overrides: BTreeSet<DescriptorMapOverride>,
    pub spawn_path: DescriptorSpawnPath,
}

impl DescriptorProvenance {
    pub fn owns(&self, component: DescriptorComponentKind) -> bool {
        self.owned_components.contains(&component)
    }

    pub fn overrides_for(
        &self,
        component: DescriptorComponentKind,
    ) -> impl Iterator<Item = DescriptorMapOverride> + '_ {
        self.map_overrides
            .iter()
            .copied()
            .filter(move |field| field.component() == component)
    }
}

pub fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

pub fn parse_f32(raw: &str) -> Option<f32> {
    match raw.trim().parse::<f32>() {
        Ok(v) if v.is_finite() => Some(v),
        _ => None,
    }
}

pub fn parse_nonneg_f32(raw: &str) -> Option<f32> {
    parse_f32(raw).filter(|v| *v >= 0.0)
}

pub fn parse_vec3(raw: &str) -> Option<[f32; 3]> {
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
