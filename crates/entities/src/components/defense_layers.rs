// Defense layers: an entity's ordered stack of author-named pools that drain
// before health, its pool-local resistances, and the last-hit tick their
// recharge delay counts from.
// See: context/lib/entity_model.md §Components (Defense layers and damage types)

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::data_descriptors::HealthDescriptor;
use crate::registry::{ComponentKind, ComponentValue, EntityId, EntityRegistry};
use postretro_foundation::NumberOrIr;
use postretro_foundation::defense::{
    DEFAULT_RECHARGE_DELAY_MS, DEFAULT_RECHARGE_RATE, DEFAULT_RESIST, DrainOutcome, HEALTH_POOL,
    LAYER_CAPACITY_RANGE, LAYER_DELAY_MS_RANGE, LAYER_RATE_RANGE, MULTIPLIER_RANGE, PoolMultiplier,
    clamp_parameter, drain_layers,
};

/// One layer's live state. `capacity`, the recharge delay and rate and each
/// resist are the parameters in force: literals bound at spawn, or the last
/// good evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DefenseLayer {
    /// The registered layer kind this instances.
    pub name: String,
    pub value: f32,
    pub capacity: f32,
    pub recharge_delay_ms: f32,
    pub recharge_rate: f32,
    /// This layer's own resistance per damage type: scales that type on this
    /// layer only.
    pub resist: Vec<Resist>,
}

/// One pool's resistance to one damage type, in force. A type with no entry
/// is ×1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resist {
    pub damage_type: String,
    pub value: f32,
}

/// Host-owned layer stack in drain order, plus health's own resistances.
/// Only an entity whose descriptor lists layers or a health `resist` carries
/// one, so the per-tick step visits only those.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DefenseLayersComponent {
    /// Canonical name of the descriptor the layers came from: the key the
    /// host's bound expression parameters are found by.
    #[serde(default)]
    pub descriptor: Option<String>,
    pub layers: Vec<DefenseLayer>,
    /// Health's own resistance per damage type: scales health only.
    pub health_resist: Vec<Resist>,
    /// Host tick of the latest hit with a positive raw amount. Only the damage
    /// chokepoint sets it; spawn, respawn, carry and grants never touch it, so
    /// a layer never hit recharges from its first tick.
    pub last_hit_tick: Option<u64>,
}

impl DefenseLayersComponent {
    /// Materialize from a descriptor at spawn, every layer full. `None` when
    /// the descriptor lists no layers and no health resistance.
    pub fn from_descriptor(desc: &HealthDescriptor) -> Option<Self> {
        if desc.layers.is_empty() && desc.resist.is_empty() {
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
                    resist: resists_from(&entry.resist),
                }
            })
            .collect();
        Some(Self {
            descriptor: None,
            layers,
            health_resist: resists_from(&desc.resist),
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

/// The host's evaluator of expression layer parameters, installed on the
/// registry by the simulation. A connected client installs none, so it
/// evaluates nothing.
pub trait DefenseParameterSource {
    /// Evaluate `component`'s expression parameters for `entity`, writing
    /// each good result clamped into its range and keeping the value in force
    /// on a fault (the entity's last good value), with one warning per
    /// descriptor and field. Literal parameters are never evaluated.
    fn evaluate(
        &self,
        registry: &EntityRegistry,
        entity: EntityId,
        component: &mut DefenseLayersComponent,
    );

    /// Evaluate only layer `index`'s capacity, as a grant does: the result
    /// becomes the capacity in force, and the entity's last good value.
    fn evaluate_capacity(
        &self,
        registry: &EntityRegistry,
        entity: EntityId,
        component: &mut DefenseLayersComponent,
        index: usize,
    );
}

fn resist_for(resists: &[Resist], damage_type: &str) -> f32 {
    resists
        .iter()
        .find(|resist| resist.damage_type == damage_type)
        .map_or(DEFAULT_RESIST, |resist| resist.value)
}

fn resists_from(map: &BTreeMap<String, NumberOrIr>) -> Vec<Resist> {
    map.iter()
        .map(|(damage_type, value)| Resist {
            damage_type: damage_type.clone(),
            value: literal_or(value, DEFAULT_RESIST, MULTIPLIER_RANGE),
        })
        .collect()
}

/// Drain one hit's raw amount through `id`'s layers in order and stamp the
/// last-hit tick; health takes the returned `health_loss`. `damage_type` is
/// the producer's type; a hit that names none deals the mod's default type.
/// Returns the type the hit dealt beside the outcome. Untyped damage is ×1 on
/// every pool and no resistance applies. A pool's multiplier is its matrix
/// entry × its own resist; nothing else scales it.
pub(crate) fn drain_hit(
    registry: &mut EntityRegistry,
    id: EntityId,
    raw: f32,
    damage_type: Option<u16>,
) -> (DrainOutcome, Option<u16>) {
    let vocabulary = std::sync::Arc::clone(registry.defense_vocabulary());
    let damage_type = damage_type
        .or(vocabulary
            .default_damage_type
            .and_then(|index| u16::try_from(index).ok()))
        .filter(|index| usize::from(*index) < vocabulary.damage_types.len());
    let declaration = damage_type.map(|index| &vocabulary.damage_types[usize::from(index)]);
    let matrix_entry = |pool: &str| {
        declaration.map_or(PoolMultiplier::ONE, |declaration| {
            declaration
                .matrix
                .get(pool)
                .copied()
                .unwrap_or(PoolMultiplier::ONE)
        })
    };
    let health_matrix = match matrix_entry(HEALTH_POOL) {
        PoolMultiplier::Scale(scale) => scale,
        // Rejected at load: health never bypasses.
        PoolMultiplier::Bypass => 1.0,
    };
    let tick = registry.host_tick();
    let Ok(ComponentValue::DefenseLayers(component)) =
        registry.get_component_value_mut(id, ComponentKind::DefenseLayers)
    else {
        let health_loss = raw * health_matrix;
        let outcome = DrainOutcome {
            raw,
            health_loss,
            points_dealt: health_loss,
            ..DrainOutcome::default()
        };
        return (outcome, damage_type);
    };
    component.last_hit_tick = Some(tick);
    let resist = |resists: &[Resist]| {
        declaration.map_or(DEFAULT_RESIST, |declaration| {
            resist_for(resists, &declaration.name)
        })
    };
    let health_multiplier = health_matrix * resist(&component.health_resist);
    let outcome = drain_layers(
        raw,
        component.layers.iter_mut().map(|layer| {
            let multiplier = matrix_entry(&layer.name).resisted(resist(&layer.resist));
            (&mut layer.value, multiplier)
        }),
        health_multiplier,
    );
    (outcome, damage_type)
}

/// A literal parameter clamped into its range, or `fallback` for an
/// expression or a non-finite literal.
fn literal_or(value: &NumberOrIr, fallback: f32, range: (f32, f32)) -> f32 {
    match value {
        NumberOrIr::Literal(literal) => clamp_parameter(*literal, range).unwrap_or(fallback),
        NumberOrIr::Ir(_) => fallback,
    }
}
