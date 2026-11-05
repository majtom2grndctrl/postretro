// The per-tick defense-layer step: clamp each layer to its capacity in force,
// then recharge it once its delay has passed since the entity's last hit.
// See: context/lib/scripting.md §11 (Defense layers) · context/lib/entity_model.md §Components

use postretro_entities::registry::{ComponentKind, ComponentValue, EntityRegistry};
use postretro_foundation::defense::recharged_value;

/// Run at tick start, after the respawn drain and before any hit, on the host
/// and in single player only: a connected client recharges nothing. Visits
/// only entities carrying layers and allocates nothing. Every layer clamps to
/// its capacity; only an entity above zero health recharges.
pub fn recharge_defense_layers(registry: &mut EntityRegistry) {
    let tick = registry.host_tick();
    let dt = postretro_foundation::TICK_DURATION.as_secs_f32();
    registry.for_each_with_kind_mut_reading(
        ComponentKind::DefenseLayers,
        ComponentKind::Health,
        |_, value, health| {
            let ComponentValue::DefenseLayers(component) = value else {
                return;
            };
            for layer in &mut component.layers {
                layer.value = layer.value.min(layer.capacity);
            }
            let live =
                matches!(health, Some(ComponentValue::Health(health)) if health.current > 0.0);
            if !live {
                return;
            }
            let last_hit = component.last_hit_tick;
            for layer in &mut component.layers {
                layer.value = recharged_value(
                    layer.value,
                    layer.capacity,
                    layer.recharge_delay_ms,
                    layer.recharge_rate,
                    last_hit,
                    tick,
                    dt,
                );
            }
        },
    );
}
