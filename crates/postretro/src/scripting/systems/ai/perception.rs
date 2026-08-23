//! Shared enemy-to-pawn perception endpoints and debounced static-world LOS.
//! See: context/lib/entity_model.md §7c.

use std::collections::HashMap;

use glam::Vec3;

use super::targeting::TargetPawn;
use crate::collision::{self, CollisionWorld};
use crate::nav::NavGraph;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::{EntityId, EntityRegistry};

/// Fraction of the baked navigation-agent height used as an enemy eye when an
/// authored health hitbox does not define the exact top-center origin.
pub(super) const EYE_FACTOR: f32 = 0.9;

/// Ticks that a retained target remains visible after raw static-world LOS is
/// first lost. This is host-only feel state; fresh acquisition never uses it.
pub(super) const LOS_GRACE_TICKS: u32 = 3;

/// Persistent visibility state for one enemy. The selected target is retained
/// beside the countdown so a grace window cannot leak from an old target to a
/// newly selected pawn.
#[derive(Debug, Clone, Copy)]
pub(super) struct LosGraceState {
    target: EntityId,
    remaining_ticks: u32,
}

/// The debounced verdict for one enemy's selected target in one AI compute
/// pass. Every LOS caller derives its endpoints through this module's shared
/// helpers rather than casting a second ray with slightly different points.
#[derive(Debug, Clone, Copy)]
pub(super) struct EnemyTargetPerception {
    pub(super) visible: bool,
    /// The shared endpoints Task 1 derived for this enemy/target pair. Combat
    /// positioning receives these values rather than deriving a second ray.
    pub(super) enemy_eye: Vec3,
    pub(super) target_aim: Vec3,
}

/// Derive the one enemy eye point used by LOS consumers. An authored health
/// hitbox is exact; otherwise the baked navigation-agent height supplies the
/// consistent fallback for every enemy on the map.
pub(super) fn enemy_eye(
    registry: &EntityRegistry,
    enemy: EntityId,
    position: Vec3,
    nav_graph: Option<&NavGraph>,
) -> Vec3 {
    if let Ok(health) = registry.get_component::<HealthComponent>(enemy)
        && let Some(hitbox) = health.hitbox
    {
        return position + hitbox.offset + Vec3::Y * hitbox.half_extents.y;
    }

    let agent_height = nav_graph.map_or(0.0, |graph| graph.agent_params().height);
    position + Vec3::Y * (EYE_FACTOR * agent_height)
}

/// Derive the selected pawn's one target-aim point from the transform snapshot
/// carried by targeting and its authored capsule eye height.
pub(super) fn target_aim(registry: &EntityRegistry, target: TargetPawn) -> Option<Vec3> {
    let movement = registry
        .get_component::<PlayerMovementComponent>(target.entity)
        .ok()?;
    Some(target.position + Vec3::Y * movement.capsule.eye_height)
}

/// Compute this tick's one enemy-to-selected-target perception result. A
/// missing collision world intentionally means clear sight, preserving
/// headless and no-world ticks from before LOS existed.
pub(super) fn perceive_target(
    registry: &EntityRegistry,
    grace: &mut HashMap<EntityId, LosGraceState>,
    enemy: EntityId,
    enemy_position: Vec3,
    target: TargetPawn,
    nav_graph: Option<&NavGraph>,
    collision_world: Option<&CollisionWorld>,
) -> Option<EnemyTargetPerception> {
    let target_aim = match target_aim(registry, target) {
        Some(aim) => aim,
        None => {
            grace.remove(&enemy);
            return None;
        }
    };
    let enemy_eye = enemy_eye(registry, enemy, enemy_position, nav_graph);
    let raw_visible = collision_world
        .map(|world| collision::line_of_sight(enemy_eye, target_aim, world))
        .unwrap_or(true);
    let visible = debounce_los(grace, enemy, target.entity, raw_visible);

    Some(EnemyTargetPerception {
        visible,
        enemy_eye,
        target_aim,
    })
}

/// The engine-floor fire gate. It intentionally has no graph or cooldown
/// dependency: the caller layers those gates around this one shared verdict.
pub(super) fn fire_gate(perception: Option<EnemyTargetPerception>) -> bool {
    perception.is_some_and(|perception| perception.visible)
}

fn debounce_los(
    grace: &mut HashMap<EntityId, LosGraceState>,
    enemy: EntityId,
    target: EntityId,
    raw_visible: bool,
) -> bool {
    if raw_visible {
        grace.insert(
            enemy,
            LosGraceState {
                target,
                remaining_ticks: LOS_GRACE_TICKS,
            },
        );
        return true;
    }

    let Some(state) = grace.get_mut(&enemy) else {
        return false;
    };
    if state.target != target {
        grace.remove(&enemy);
        return false;
    }
    if state.remaining_ticks == 0 {
        grace.remove(&enemy);
        return false;
    }

    state.remaining_ticks -= 1;
    if state.remaining_ticks == 0 {
        grace.remove(&enemy);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::EntityRegistry;

    #[test]
    fn debounce_holds_exactly_the_configured_loss_grace_ticks() {
        let mut grace = HashMap::new();
        let mut registry = EntityRegistry::new();
        let enemy = registry.spawn(Default::default());
        let target = registry.spawn(Default::default());

        assert!(debounce_los(&mut grace, enemy, target, true));
        for _ in 0..LOS_GRACE_TICKS {
            assert!(
                debounce_los(&mut grace, enemy, target, false),
                "the previous clear verdict survives its grace window"
            );
        }
        assert!(
            !debounce_los(&mut grace, enemy, target, false),
            "the first loss tick after grace closes the verdict"
        );
        assert!(grace.is_empty(), "expired state is pruned immediately");
    }

    #[test]
    fn debounce_does_not_carry_visibility_to_a_replaced_target() {
        let mut grace = HashMap::new();
        let mut registry = EntityRegistry::new();
        let enemy = registry.spawn(Default::default());
        let first_target = registry.spawn(Default::default());
        let second_target = registry.spawn(Default::default());

        assert!(debounce_los(&mut grace, enemy, first_target, true));
        assert!(
            !debounce_los(&mut grace, enemy, second_target, false),
            "a newly selected target has no inherited grace"
        );
        assert!(grace.is_empty(), "mismatched state is pruned");
    }

    #[test]
    fn fire_gate_projects_the_shared_perception_verdict() {
        // P4: the floor's fire gate consumes the same already-debounced scalar
        // that the brain-fact refresh receives from the AI compute pass.
        assert!(fire_gate(Some(EnemyTargetPerception { visible: true })));
        assert!(!fire_gate(Some(EnemyTargetPerception { visible: false })));
        assert!(
            !fire_gate(None),
            "no selected target cannot inherit visibility"
        );
    }
}
