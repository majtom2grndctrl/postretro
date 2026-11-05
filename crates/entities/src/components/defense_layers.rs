// Defense layers: an entity's ordered stack of author-named pools that drain
// before health, plus the last-hit tick their recharge delay counts from.
// See: context/lib/entity_model.md §Components (Defense layers and damage types)

use serde::{Deserialize, Serialize};

use crate::data_descriptors::HealthDescriptor;
use crate::registry::{ComponentKind, ComponentValue, EntityId, EntityRegistry};
use postretro_foundation::NumberOrIr;
use postretro_foundation::defense::{
    DEFAULT_RECHARGE_DELAY_MS, DEFAULT_RECHARGE_RATE, DrainOutcome, LAYER_CAPACITY_RANGE,
    LAYER_DELAY_MS_RANGE, LAYER_RATE_RANGE, PoolMultiplier, clamp_parameter, drain_layers,
};

/// One layer's live state. `capacity`, the recharge delay and rate are the
/// parameters in force: literals bound at spawn, or the last good evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DefenseLayer {
    /// The registered layer kind this instances.
    pub name: String,
    pub value: f32,
    pub capacity: f32,
    pub recharge_delay_ms: f32,
    pub recharge_rate: f32,
}

/// Host-owned layer stack in drain order. Only an entity whose descriptor
/// lists layers carries one, so the per-tick step visits only those.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DefenseLayersComponent {
    pub layers: Vec<DefenseLayer>,
    /// Host tick of the latest hit with a positive raw amount. Only the damage
    /// chokepoint sets it; spawn, respawn, carry and grants never touch it, so
    /// a layer never hit recharges from its first tick.
    pub last_hit_tick: Option<u64>,
}

impl DefenseLayersComponent {
    /// Materialize from a descriptor at spawn, every layer full. `None` when
    /// the descriptor lists no layers.
    pub fn from_descriptor(desc: &HealthDescriptor) -> Option<Self> {
        if desc.layers.is_empty() {
            return None;
        }
        let layers = desc
            .layers
            .iter()
            .map(|entry| {
                let name = entry
                    .layer
                    .handle("layer")
                    .map(|handle| handle.name().to_string())
                    .unwrap_or_default();
                let capacity = literal_or(&entry.capacity, 0.0, LAYER_CAPACITY_RANGE);
                DefenseLayer {
                    name,
                    value: capacity,
                    capacity,
                    recharge_delay_ms: entry
                        .recharge_delay_ms
                        .as_ref()
                        .map_or(DEFAULT_RECHARGE_DELAY_MS, |value| {
                            literal_or(value, DEFAULT_RECHARGE_DELAY_MS, LAYER_DELAY_MS_RANGE)
                        }),
                    recharge_rate: entry
                        .recharge_rate
                        .as_ref()
                        .map_or(DEFAULT_RECHARGE_RATE, |value| {
                            literal_or(value, DEFAULT_RECHARGE_RATE, LAYER_RATE_RANGE)
                        }),
                }
            })
            .collect();
        Some(Self {
            layers,
            last_hit_tick: None,
        })
    }

    /// The layer of kind `name`, if this entity carries it.
    pub fn layer(&self, name: &str) -> Option<&DefenseLayer> {
        self.layers.iter().find(|layer| layer.name == name)
    }

    /// Refill every layer to the capacity in force.
    pub fn fill(&mut self) {
        for layer in &mut self.layers {
            layer.value = layer.capacity;
        }
    }
}

/// Drain one hit's raw amount through `id`'s layers in order and stamp the
/// last-hit tick; health takes the returned `health_loss`. An entity without
/// layers passes the whole raw amount to health. Every pool is ×1 until
/// damage types scale them.
pub(crate) fn drain_hit(registry: &mut EntityRegistry, id: EntityId, raw: f32) -> DrainOutcome {
    let tick = registry.host_tick();
    let Ok(ComponentValue::DefenseLayers(component)) =
        registry.get_component_value_mut(id, ComponentKind::DefenseLayers)
    else {
        return DrainOutcome {
            raw,
            health_loss: raw,
            points_dealt: raw,
            ..DrainOutcome::default()
        };
    };
    component.last_hit_tick = Some(tick);
    drain_layers(
        raw,
        component
            .layers
            .iter_mut()
            .map(|layer| (&mut layer.value, PoolMultiplier::ONE)),
        1.0,
    )
}

/// A literal parameter clamped into its range, or `fallback` for an
/// expression or a non-finite literal.
fn literal_or(value: &NumberOrIr, fallback: f32, range: (f32, f32)) -> f32 {
    match value {
        NumberOrIr::Literal(literal) => clamp_parameter(*literal, range).unwrap_or(fallback),
        NumberOrIr::Ir(_) => fallback,
    }
}
