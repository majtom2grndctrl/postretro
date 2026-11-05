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
    /// A lifecycle refill waiting for the next evaluation of this layer's
    /// parameters: seat bind, or the tick start after a join seed or a hot
    /// reload. Host-only bookkeeping, never replicated.
    #[serde(default)]
    pub refill: LayerRefill,
}

/// What the next evaluation does to a layer's value, after it clamps.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum LayerRefill {
    #[default]
    None,
    /// Fill to the capacity just evaluated: a layer a hot reload added.
    Fill,
    /// Fill only if the layer is full at the capacity in force before the
    /// evaluation, 0 / 0 included: the spawn fill redone with a seat's
    /// per-owner values once they arrive.
    IfFull,
    /// A carried value, clamped to the capacity just evaluated.
    Restore(f32),
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
                    refill: LayerRefill::None,
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

    /// Mark every layer with no pending refill as `refill`.
    pub fn mark_unmarked(&mut self, refill: LayerRefill) {
        for layer in &mut self.layers {
            if layer.refill == LayerRefill::None {
                layer.refill = refill;
            }
        }
    }

    /// Before an evaluation: settle each `IfFull` mark against the capacity in
    /// force now, as a fill or nothing.
    pub fn resolve_refills(&mut self) {
        for layer in &mut self.layers {
            if layer.refill == LayerRefill::IfFull {
                layer.refill = if layer.value >= layer.capacity {
                    LayerRefill::Fill
                } else {
                    LayerRefill::None
                };
            }
        }
    }

    /// After an evaluation: clamp each layer to the capacity just evaluated,
    /// then apply and clear its pending refill.
    pub fn settle(&mut self) {
        for layer in &mut self.layers {
            layer.value = match layer.refill {
                LayerRefill::Fill => layer.capacity,
                LayerRefill::Restore(carried) => carried.max(0.0).min(layer.capacity),
                LayerRefill::None | LayerRefill::IfFull => layer.value.min(layer.capacity),
            };
            layer.refill = LayerRefill::None;
        }
    }

    /// Rebuild from an edited descriptor on hot reload, matching layers by
    /// name in the new order. A kept layer keeps its value and, for each
    /// expression parameter, its last good value; a literal takes the new
    /// literal. A new layer fills when its parameters next evaluate. The
    /// last-hit stamp stays. `None` when the edit lists no layers and no
    /// health resistance.
    pub fn refreshed(live: Option<&Self>, desc: &HealthDescriptor) -> Option<Self> {
        let mut fresh = Self::from_descriptor(desc)?;
        let Some(live) = live else {
            fresh.mark_unmarked(LayerRefill::Fill);
            return Some(fresh);
        };
        for (layer, entry) in fresh.layers.iter_mut().zip(&desc.layers) {
            let Some(old) = live.layer(&layer.name) else {
                layer.refill = LayerRefill::Fill;
                continue;
            };
            layer.value = old.value;
            layer.refill = old.refill;
            if is_ir(&entry.capacity) {
                layer.capacity = old.capacity;
            }
            if entry.recharge_delay_ms.as_ref().is_some_and(is_ir) {
                layer.recharge_delay_ms = old.recharge_delay_ms;
            }
            if entry.recharge_rate.as_ref().is_some_and(is_ir) {
                layer.recharge_rate = old.recharge_rate;
            }
            keep_last_good_resists(&mut layer.resist, &old.resist, &entry.resist);
        }
        keep_last_good_resists(&mut fresh.health_resist, &live.health_resist, &desc.resist);
        fresh.descriptor = live.descriptor.clone();
        fresh.last_hit_tick = live.last_hit_tick;
        Some(fresh)
    }
}

fn is_ir(value: &NumberOrIr) -> bool {
    matches!(value, NumberOrIr::Ir(_))
}

/// An expression resist keeps the live entity's last good value for its type.
fn keep_last_good_resists(
    fresh: &mut [Resist],
    old: &[Resist],
    authored: &BTreeMap<String, NumberOrIr>,
) {
    for resist in fresh {
        if authored.get(&resist.damage_type).is_some_and(is_ir)
            && let Some(old) = old.iter().find(|old| old.damage_type == resist.damage_type)
        {
            resist.value = old.value;
        }
    }
}

/// Evaluate `id`'s layer parameters now and settle its pending refills,
/// after `prepare` marks them. Paths where the next tick start is too late
/// use this: seat bind and respawn.
pub fn settle_layers_now(
    registry: &mut EntityRegistry,
    id: EntityId,
    prepare: impl FnOnce(&mut DefenseLayersComponent),
) {
    let Ok(component) = registry.get_component::<DefenseLayersComponent>(id) else {
        return;
    };
    let mut component = component.clone();
    prepare(&mut component);
    component.resolve_refills();
    if let Some(source) = registry.defense_parameters().cloned() {
        source.evaluate(registry, id, &mut component);
    }
    component.settle();
    let _ = registry.set_component(id, component);
}

/// A seat just bound `pawn`: its per-owner values are now readable. Redo the
/// spawn fill for each layer still full at the capacity it had, and apply a
/// carried value waiting on the seat.
pub fn settle_layers_on_seat_bind(registry: &mut EntityRegistry, pawn: EntityId) {
    settle_layers_now(registry, pawn, |component| {
        component.mark_unmarked(LayerRefill::IfFull);
    });
}

/// Respawn evaluates capacity itself and refills every layer. The last-hit
/// stamp is neither set nor cleared.
pub fn refill_layers_on_respawn(registry: &mut EntityRegistry, pawn: EntityId) {
    settle_layers_now(registry, pawn, |component| {
        for layer in &mut component.layers {
            layer.refill = LayerRefill::Fill;
        }
    });
}

/// A join seed landed for `pawn`'s seat: at the next tick start, before any
/// hit, redo the spawn fill for each layer full at its pre-seed capacity.
pub fn mark_layers_for_join_seed(registry: &mut EntityRegistry, pawn: EntityId) {
    let Ok(component) = registry.get_component::<DefenseLayersComponent>(pawn) else {
        return;
    };
    let mut component = component.clone();
    component.mark_unmarked(LayerRefill::IfFull);
    let _ = registry.set_component(pawn, component);
}

/// The layer reads one IR scope has bound — `(layer, reads capacity)` — and
/// each one's value for the entity last seeded. Binding grows the lists;
/// seeding refills the values in place, so a per-fire seed allocates
/// nothing. A read of a layer the entity lacks is `None`.
#[derive(Debug, Default)]
pub struct DefenseReads {
    reads: std::cell::RefCell<Vec<(String, bool)>>,
    values: std::cell::RefCell<Vec<Option<f32>>>,
}

impl DefenseReads {
    /// Intern one read, returning its stable index.
    pub fn bind(&self, layer: &str, capacity: bool) -> usize {
        let mut reads = self.reads.borrow_mut();
        if let Some(index) = reads
            .iter()
            .position(|(bound, cap)| bound == layer && *cap == capacity)
        {
            return index;
        }
        reads.push((layer.to_string(), capacity));
        self.values.borrow_mut().push(None);
        reads.len() - 1
    }

    /// Snapshot every bound read for `entity` (none: every read absent).
    pub fn seed(&self, registry: &EntityRegistry, entity: Option<EntityId>) {
        let component = entity.and_then(|entity| {
            registry
                .get_component::<DefenseLayersComponent>(entity)
                .ok()
        });
        let reads = self.reads.borrow();
        let mut values = self.values.borrow_mut();
        for (value, (layer, capacity)) in values.iter_mut().zip(reads.iter()) {
            *value = component
                .and_then(|component| component.layer(layer))
                .map(|layer| {
                    if *capacity {
                        layer.capacity
                    } else {
                        layer.value
                    }
                });
        }
    }

    /// The seeded value of read `index`, `None` when the entity lacks it.
    pub fn get(&self, index: usize) -> Option<f32> {
        self.values.borrow().get(index).copied().flatten()
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

    /// [`Self::evaluate`] from the tick-start step: an expression that reads
    /// only global values may evaluate once per descriptor and field that
    /// tick and share its result. Only the step calls this; spawn, respawn,
    /// seat bind and grants evaluate per entity against the live store.
    fn evaluate_at_tick_start(
        &self,
        registry: &EntityRegistry,
        entity: EntityId,
        component: &mut DefenseLayersComponent,
    ) {
        self.evaluate(registry, entity, component);
    }

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
