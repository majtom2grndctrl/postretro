// The per-tick defense-layer step: evaluate each layer's expression
// parameters, clamp the layer to its capacity in force, then recharge it once
// its delay has passed since the entity's last hit.
// See: context/lib/scripting.md §11 (Defense layers) · context/lib/entity_model.md §Components

use postretro_entities::components::health::HealthComponent;
use postretro_entities::registry::EntityRegistry;
use postretro_foundation::defense::recharged_value;

#[cfg(test)]
thread_local! {
    /// Entities the step visited, for the per-tick cost tests.
    pub(crate) static VISITED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Run at tick start, after the respawn drain and before any hit, on the host
/// and in single player only: a connected client recharges nothing. Visits
/// only entities carrying layers and allocates nothing. Every entity's
/// parameters evaluate and its layers clamp; only one above zero health
/// recharges.
pub fn recharge_defense_layers(registry: &mut EntityRegistry) {
    let tick = registry.host_tick();
    let dt = postretro_foundation::TICK_DURATION.as_secs_f32();
    let source = registry.defense_parameters().cloned();
    registry.for_each_defense_layers_lent(|registry, id, component| {
        #[cfg(test)]
        VISITED.with(|count| count.set(count.get() + 1));
        if let Some(source) = &source {
            source.evaluate(registry, id, component);
        }
        for layer in &mut component.layers {
            layer.value = layer.value.min(layer.capacity);
        }
        let live = registry
            .get_component::<HealthComponent>(id)
            .is_ok_and(|health| health.current > 0.0);
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
    });
}
