// The status stage: due status ticks land as host-authoritative in-tick hits
// through the damage chokepoint, run impact policies and credit the applier;
// then instances whose time ran out end.
// See: context/lib/entity_model.md §5 (Update Order) · context/lib/scripting.md §2

use postretro_entities::EntityRegistry;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, HealthComponent, apply_damage_with_context, is_depleted,
};
use postretro_entities::components::statuses::StatusApplier;
use postretro_foundation::DamagePayload;

/// Run the stage on the current host tick. Each due tick re-checks its
/// instance at its turn, so a policy that clears or applies a status during
/// the stage takes effect at once; a new instance never ticks on the tick it
/// was created. A due tick whose target is already at health ≤ 0 is skipped,
/// so another hit's authored death plays alone. A marker (`damage: 0`) never
/// enters the chokepoint. The stage's own bookkeeping allocates nothing once
/// its scratch has grown; an entity with no instance is never visited. A
/// damaging tick still builds its [`DamageContext`] source-id `String`: that
/// is the chokepoint's cost, measured by the stress run rather than gated.
/// Expiry keeps the instances it ends until the next stage, so an
/// application counting on this tick after the stage (the post-tick
/// remote-hit flush) revives its instance rather than starting a new one.
pub(crate) fn run_status_stage(
    registry: &mut EntityRegistry,
    on_impact: &mut dyn FnMut(&mut EntityRegistry),
) {
    // This tick's fluid applications come first, so re-entering lava on a
    // lingering instance's last tick refreshes it before its expiry is judged.
    registry.apply_fluid_statuses();
    if registry.statuses().is_empty() {
        return;
    }
    let vocabulary = std::sync::Arc::clone(registry.defense_vocabulary());
    let due = registry.collect_due_statuses();
    for &(target, serial) in &due {
        let Some(instance) = registry.status_instance(target, serial) else {
            continue;
        };
        let Some(declaration) = vocabulary.statuses.get(usize::from(instance.status)) else {
            continue;
        };
        if instance.next_due > registry.host_tick() {
            continue;
        }
        // The phase moves on whether or not this tick deals damage.
        registry.advance_status_tick(target, serial, declaration.tick_ticks);
        if declaration.is_marker() {
            continue;
        }
        let live = registry
            .get_component::<HealthComponent>(target)
            .is_ok_and(|health| !is_depleted(health));
        if !live {
            continue;
        }
        let attacker = match instance.applier {
            StatusApplier::Entity(entity) if registry.exists(entity) => Some(entity),
            _ => None,
        };
        let source_id = registry.statuses().source_id(instance.source).to_string();
        let mut context = DamageContext::new(source_id, DamageProducer::InTick);
        context.attacker = attacker;
        context.status = Some(instance.status);
        let amount = declaration.damage * f32::from(instance.stacks.max(1));
        let damage_type = declaration
            .damage_type
            .and_then(|index| u16::try_from(index).ok());
        if apply_damage_with_context(
            registry,
            target,
            &DamagePayload {
                amount,
                impulse: glam::Vec3::ZERO,
                damage_type,
            },
            context,
        ) {
            on_impact(registry);
        }
    }
    registry.restore_status_due(due);
    registry.expire_statuses();
}
