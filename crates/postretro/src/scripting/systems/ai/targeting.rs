// Enemy target-pawn selection: nearest/retained candidate ranking with switch
// hysteresis, feeding the brain tick's per-enemy target.
// See: context/lib/entity_model.md §7c (enemy brain component)

use glam::Vec3;

use super::engine_floor::{is_meaningfully_closer, think_stride_for_distance};
use crate::nav::distance_xz;
use postretro_entities::ComponentKind;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::{BoundProgram, IrValue, eval_value};

use super::candidate_scope::CandidateScope;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TargetPawn {
    pub(crate) entity: EntityId,
    pub(crate) position: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct TargetCandidate {
    pub(super) target: TargetPawn,
    pub(super) distance: f32,
}

pub(super) fn target_candidate(
    registry: &EntityRegistry,
    entity: EntityId,
    from: Vec3,
    visible: Option<&dyn Fn(EntityId) -> bool>,
) -> Option<TargetCandidate> {
    if visible.is_some_and(|is_visible| !is_visible(entity)) {
        return None;
    }
    registry
        .get_component::<PlayerMovementComponent>(entity)
        .ok()?;
    let position = registry.get_component::<Transform>(entity).ok()?.position;
    Some(TargetCandidate {
        target: TargetPawn { entity, position },
        distance: distance_xz(position, from),
    })
}

fn nearest_target_candidate(
    registry: &EntityRegistry,
    from: Vec3,
    visible: Option<&dyn Fn(EntityId) -> bool>,
    exclude: Option<EntityId>,
    candidate_filter: Option<&BoundProgram<CandidateScope>>,
    candidate_scope: &mut CandidateScope,
) -> (Option<TargetCandidate>, Option<TargetCandidate>) {
    registry
        .iter_with_kind(ComponentKind::PlayerMovement)
        .filter_map(|(entity, _)| {
            if exclude == Some(entity) {
                return None;
            }
            target_candidate(registry, entity, from, visible)
        })
        .fold((None, None), |(mut nearest, mut eligible), candidate| {
            if nearest.is_none_or(|current: TargetCandidate| {
                candidate.distance.total_cmp(&current.distance).is_lt()
            }) {
                nearest = Some(candidate);
            }
            // Eligibility is per offered candidate only: retained lookup stays
            // above this scan and never consults the graph's policy.
            let filter_allows = candidate_filter.is_none_or(|filter| {
                candidate_scope.refresh(registry, candidate.target.entity, candidate.distance);
                eval_value(filter, candidate_scope) == IrValue::Bool(true)
            });
            if filter_allows
                && eligible.is_none_or(|current: TargetCandidate| {
                    candidate.distance.total_cmp(&current.distance).is_lt()
                })
            {
                eligible = Some(candidate);
            }
            (nearest, eligible)
        })
}

pub(super) fn target_distance(target: TargetPawn, from: Vec3) -> f32 {
    distance_xz(target.position, from)
}

pub(super) fn acquisition_due(brain: &BrainComponent, distance: Option<f32>) -> bool {
    distance
        .map(|distance| {
            let stride = think_stride_for_distance(distance);
            stride <= 1 || brain.think_stride_counter.wrapping_add(1) % stride == 0
        })
        .unwrap_or(true)
}

pub(super) fn selected_target_alive(registry: &EntityRegistry, target: EntityId) -> bool {
    registry
        .get_component::<HealthComponent>(target)
        .map(|health| health.current > 0.0 && health.current.is_finite())
        .unwrap_or(false)
}

/// Select the player pawn this enemy should pursue.
///
/// This is the AI targeting extension point: v1 ranks all
/// [`ComponentKind::PlayerMovement`] pawns by nearest XZ distance from `from`.
/// The optional predicate is the future visibility/relevance seam intended for
/// `context/research/cell-visibility-substrate.md` (and exact LOS work) without
/// re-threading the FSM. It returns the unfiltered nearest offered candidate
/// for think-stride pricing and the selected target. Candidate filters admit
/// fresh candidates only; a retained candidate is resolved independently and
/// stays eligible until graph state policy stands it down. On a due tick, a
/// meaningfully closer eligible candidate may replace the retained target.
/// This path intentionally does not consult the registry's local-player marker,
/// which is client-side convenience state.
pub(crate) fn select_target(
    registry: &EntityRegistry,
    from: Vec3,
    retained_target: Option<EntityId>,
    visible: Option<&dyn Fn(EntityId) -> bool>,
    candidate_filter: Option<&BoundProgram<CandidateScope>>,
    candidate_scope: &mut CandidateScope,
) -> (Option<TargetCandidate>, Option<TargetPawn>) {
    let retained =
        retained_target.and_then(|entity| target_candidate(registry, entity, from, visible));
    let (nearest_offered, nearest_eligible) = nearest_target_candidate(
        registry,
        from,
        visible,
        retained_target,
        candidate_filter,
        candidate_scope,
    );

    let selected = match (retained, nearest_eligible) {
        (Some(retained), Some(nearest))
            if is_meaningfully_closer(nearest.distance, retained.distance) =>
        {
            Some(nearest.target)
        }
        (Some(retained), _) => Some(retained.target),
        (None, Some(nearest)) => Some(nearest.target),
        (None, None) => None,
    };

    (nearest_offered, selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_foundation::{
        AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementDescriptor, SpeedParams,
    };

    fn movement() -> PlayerMovementComponent {
        PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.35,
                half_height: 0.9,
                eye_height: 1.1,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 4.0,
                    run: 6.0,
                    crouch: 2.0,
                },
                accel: 20.0,
                step_height: 0.4,
                max_slope: 45.0,
            },
            air: AirParams {
                forward_steer: 0.0,
                accel: 1.0,
                max_control_speed: 1.0,
                bunny_hop: false,
                jumps: 0,
                jump_velocity: 5.0,
                jump_ceiling: 0.0,
            },
            fall: FallParams {
                terminal_velocity: 40.0,
            },
            stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
            stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
            dash: None,
            forgiveness: None,
            crouch: None,
            view_feel: None,
        })
    }

    fn pawn(registry: &mut EntityRegistry, x: f32) -> EntityId {
        let entity = registry.spawn(Transform {
            position: Vec3::new(x, 0.0, 0.0),
            ..Transform::default()
        });
        registry.set_component(entity, movement()).unwrap();
        entity
    }

    #[test]
    fn selection_keeps_retained_target_until_a_fresh_candidate_beats_hysteresis() {
        let mut registry = EntityRegistry::new();
        let retained = pawn(&mut registry, 10.0);
        let near_but_not_meaningfully_closer = pawn(&mut registry, 9.5);
        let (_, selected) = select_target(
            &registry,
            Vec3::ZERO,
            Some(retained),
            None,
            None,
            &mut CandidateScope::for_validation(),
        );
        assert_eq!(selected.map(|target| target.entity), Some(retained));

        registry
            .set_component(
                near_but_not_meaningfully_closer,
                Transform {
                    position: Vec3::new(8.0, 0.0, 0.0),
                    ..Transform::default()
                },
            )
            .unwrap();
        let (_, selected) = select_target(
            &registry,
            Vec3::ZERO,
            Some(retained),
            None,
            None,
            &mut CandidateScope::for_validation(),
        );
        assert_eq!(
            selected.map(|target| target.entity),
            Some(near_but_not_meaningfully_closer)
        );
    }
}
