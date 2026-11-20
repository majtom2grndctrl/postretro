// Brain damage perception: recency, attacker threat, bearing and last-known
// position, updated once per landed hit at the damage chokepoint.
//
// See: context/lib/entity_model.md §7c (Enemy AI)

use glam::Vec3;

use crate::registry::{ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform};

/// Record one landed hit in the damaged entity's brain, when it has one.
/// Called only for hits on a live target, so the lethal hit freezes perception.
/// `amount` is the hit's points dealt: layer-absorbed plus health loss.
pub(crate) fn record_damage_perception(
    registry: &mut EntityRegistry,
    id: EntityId,
    attacker: Option<EntityId>,
    attacker_life: Option<u32>,
    amount: f32,
) {
    // A shot fired before its pawn's latest respawn lands as damage, but this
    // brain perceives no attacker in it: no ledger entry, no meter, no bearing.
    let attacker = attacker.filter(|&attacker| {
        attacker_life.is_none_or(|life| {
            life == crate::components::player_movement::life_counter(registry, attacker)
        })
    });
    let life = attacker.map_or(0, |attacker| {
        crate::components::player_movement::life_counter(registry, attacker)
    });
    let attacker_transform =
        attacker.and_then(|attacker| registry.get_component::<Transform>(attacker).ok().copied());
    let damaged_transform = registry.get_component::<Transform>(id).ok().copied();
    // The held target's life now. A respawn lands at tick start, before this
    // brain's next run drops a target from the earlier life, so a hit in
    // between finds a target that is already gone: it counts as none.
    let held_target_life = registry
        .get_component::<crate::components::brain::BrainComponent>(id)
        .ok()
        .and_then(|brain| brain.acquired_target)
        .map(|target| crate::components::player_movement::life_counter(registry, target));
    if let Ok(ComponentValue::Brain(brain)) =
        registry.get_component_value_mut(id, ComponentKind::Brain)
    {
        brain.time_since_damage_ms = 0.0;
        // Every producer reaches this chokepoint, so the per-activity sum
        // counts hitscan, projectile, splash, melee and `applyDamage` alike.
        brain.record_damage_in_activity(amount);
        // A concrete attacker is valuable even when it lacks a transform: the
        // per-candidate ledger is identity-based, while bearing/last-known
        // position below deliberately require spatial provenance. Contextless
        // damage still resets generic recency but creates no ledger entry.
        if let Some(attacker) = attacker {
            brain.record_attacker_damage_in_life(attacker, amount, life);
        }
        // On an NPC without a target, a hit sets its attacker's awareness
        // meter to 1, draining from this hit. One holding a target changes
        // no meter; damage with no attacker changes none. A target whose
        // pawn respawned since it was acquired is held no longer.
        let holds_target =
            brain.acquired_target.is_some() && held_target_life == Some(brain.acquired_target_life);
        if let Some(attacker) = attacker
            && attacker != id
            && !holds_target
        {
            let distance = match (attacker_transform, damaged_transform) {
                (Some(attacker), Some(damaged)) => {
                    let delta = attacker.position - damaged.position;
                    (delta.x * delta.x + delta.z * delta.z).sqrt()
                }
                _ => f32::INFINITY,
            };
            brain.awareness.record_hit(attacker, distance, life);
        }
        // Bearing is meaningful only for this hit. A contextless hit must not
        // make an older directional stimulus look recent again.
        brain.damage_bearing = 0.0;
        brain.damage_source_known = attacker_transform.is_some();
        if let Some(attacker_transform) = attacker_transform {
            brain.last_known_target_pos = Some(attacker_transform.position);
            if let Some(damaged_transform) = damaged_transform {
                let forward = damaged_transform.rotation * Vec3::Z;
                let toward_attacker = attacker_transform.position - damaged_transform.position;
                let forward_xz_len_sq = forward.x * forward.x + forward.z * forward.z;
                let attacker_xz_len_sq =
                    toward_attacker.x * toward_attacker.x + toward_attacker.z * toward_attacker.z;
                if forward_xz_len_sq.is_finite()
                    && attacker_xz_len_sq.is_finite()
                    && forward_xz_len_sq > 1.0e-8
                    && attacker_xz_len_sq > 1.0e-8
                {
                    let forward_inverse_length = forward_xz_len_sq.sqrt().recip();
                    let attacker_inverse_length = attacker_xz_len_sq.sqrt().recip();
                    let forward_x = forward.x * forward_inverse_length;
                    let forward_z = forward.z * forward_inverse_length;
                    let attacker_x = toward_attacker.x * attacker_inverse_length;
                    let attacker_z = toward_attacker.z * attacker_inverse_length;
                    let dot = forward_x * attacker_x + forward_z * attacker_z;
                    let signed_cross = forward_z * attacker_x - forward_x * attacker_z;
                    brain.damage_bearing = signed_cross
                        .atan2(dot)
                        .clamp(-std::f32::consts::PI, std::f32::consts::PI);
                }
            }
        }
    }
}
