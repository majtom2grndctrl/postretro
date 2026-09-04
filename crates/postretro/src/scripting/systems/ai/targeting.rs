// Enemy target-pawn selection: nearest/retained candidate ranking with switch
// hysteresis, feeding the brain tick's per-enemy target.
// See: context/lib/entity_model.md §7c (enemy brain component)

use glam::Vec3;

use super::engine_floor::{is_meaningfully_closer, think_stride_for_distance};
use super::perception::RawTargetPerception;
use crate::nav::distance_xz;
use postretro_entities::ComponentKind;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::{EntityId, EntityRegistry, EntityStateComponent, Transform};
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

/// This tick's selected pawn plus the raw LOS already computed when a fresh
/// candidate won the scan. Retained selections carry no fresh verdict and run
/// the normal per-tick perception query.
#[derive(Debug, Clone, Copy)]
pub(super) struct TargetSelection {
    pub(super) target: TargetPawn,
    pub(super) fresh_perception: Option<RawTargetPerception>,
}

/// The raw hostile offer set from one registry walk. `nearest` prices the
/// acquisition stride; `candidates` is retained so a due tick can apply
/// eligibility without walking the registry a second time.
#[derive(Debug)]
pub(super) struct TargetOffers {
    pub(super) nearest: Option<TargetCandidate>,
    candidates: Vec<TargetCandidate>,
}

pub(super) fn target_candidate(
    registry: &EntityRegistry,
    entity: EntityId,
    from: Vec3,
) -> Option<TargetCandidate> {
    registry
        .get_component::<PlayerMovementComponent>(entity)
        .ok()?;
    let position = registry.get_component::<Transform>(entity).ok()?.position;
    Some(TargetCandidate {
        target: TargetPawn { entity, position },
        distance: distance_xz(position, from),
    })
}

/// Collect hostile candidates without applying either authored or engine-floor
/// eligibility. The raw nearest hostile offer is the think-stride price, so it
/// must remain independent of candidacy and LOS.
pub(super) fn target_offers(
    registry: &EntityRegistry,
    from: Vec3,
    enemy_faction: f32,
    exclude: Option<EntityId>,
) -> TargetOffers {
    let mut nearest = None;
    let mut candidates = Vec::new();
    for (entity, _) in registry.iter_with_kind(ComponentKind::PlayerMovement) {
        if exclude == Some(entity) {
            continue;
        }
        let Some(candidate) = target_candidate(registry, entity, from) else {
            continue;
        };
        // Hostility defines the engine's offered set for fresh acquisition. A
        // friendly pawn therefore prices neither selection nor its think stride.
        // Retained lookup stays above this scan and deliberately never re-gates
        // its target on hostility.
        let hostile = registry
            .get_component::<EntityStateComponent>(candidate.target.entity)
            .map_or(0.0, |state| state.get(super::FACTION_STATE_FIELD))
            != enemy_faction;
        if !hostile {
            continue;
        }
        if nearest.is_none_or(|current: TargetCandidate| {
            candidate.distance.total_cmp(&current.distance).is_lt()
        }) {
            nearest = Some(candidate);
        }
        candidates.push(candidate);
    }
    TargetOffers {
        nearest,
        candidates,
    }
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

/// Choose from a raw offer set on an acquisition tick. Authored candidacy and
/// engine-floor LOS both narrow fresh eligibility, while `offers.nearest` stays
/// untouched for stride pricing. The retained candidate is deliberately supplied
/// separately and never passes either fresh-acquisition gate.
pub(super) fn select_target(
    retained: Option<TargetCandidate>,
    offers: &TargetOffers,
    registry: &EntityRegistry,
    candidate_filter: Option<&BoundProgram<CandidateScope>>,
    candidate_scope: &mut CandidateScope,
    candidate_perception: &mut dyn FnMut(TargetPawn) -> Option<RawTargetPerception>,
) -> Option<TargetSelection> {
    let nearest_eligible = offers
        .candidates
        .iter()
        .copied()
        .fold(None, |eligible, candidate| {
            // Both predicates apply only to fresh candidacy. Keep this after
            // the raw offer calculation so LOS never reprices the stride.
            let filter_allows = candidate_filter.is_none_or(|filter| {
                candidate_scope.refresh(registry, candidate.target.entity, candidate.distance);
                eval_value(filter, candidate_scope) == IrValue::Bool(true)
            });
            let fresh_perception = filter_allows
                .then(|| candidate_perception(candidate.target))
                .flatten()
                .filter(|perception| perception.visible);
            match fresh_perception {
                Some(fresh_perception)
                    if eligible.is_none_or(
                        |(current, _): (TargetCandidate, RawTargetPerception)| {
                            candidate.distance.total_cmp(&current.distance).is_lt()
                        },
                    ) =>
                {
                    Some((candidate, fresh_perception))
                }
                _ => eligible,
            }
        });

    match (retained, nearest_eligible) {
        (Some(retained), Some((nearest, fresh_perception)))
            if is_meaningfully_closer(nearest.distance, retained.distance) =>
        {
            Some(TargetSelection {
                target: nearest.target,
                fresh_perception: Some(fresh_perception),
            })
        }
        (Some(retained), _) => Some(TargetSelection {
            target: retained.target,
            fresh_perception: None,
        }),
        (None, Some((nearest, fresh_perception))) => Some(TargetSelection {
            target: nearest.target,
            fresh_perception: Some(fresh_perception),
        }),
        (None, None) => None,
    }
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
            slide: None,
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

    fn select_target_for_test(
        registry: &EntityRegistry,
        from: Vec3,
        enemy_faction: f32,
        retained_target: Option<EntityId>,
        candidate_filter: Option<&BoundProgram<CandidateScope>>,
        candidate_scope: &mut CandidateScope,
    ) -> (Option<TargetCandidate>, Option<TargetPawn>) {
        let retained = retained_target.and_then(|entity| target_candidate(registry, entity, from));
        let offers = target_offers(registry, from, enemy_faction, retained_target);
        let mut candidate_perception = |target: TargetPawn| {
            Some(RawTargetPerception {
                target: target.entity,
                visible: true,
                enemy_eye: from,
                target_aim: target.position,
            })
        };
        let selected = select_target(
            retained,
            &offers,
            registry,
            candidate_filter,
            candidate_scope,
            &mut candidate_perception,
        )
        .map(|selection| selection.target);
        (offers.nearest, selected)
    }

    #[test]
    fn fresh_selection_carries_the_candidate_los_result_but_retention_does_not() {
        let mut registry = EntityRegistry::new();
        let pawn = pawn(&mut registry, 4.0);
        let offers = target_offers(&registry, Vec3::ZERO, 1.0, None);
        let expected = RawTargetPerception {
            target: pawn,
            visible: true,
            enemy_eye: Vec3::new(0.0, 1.0, 0.0),
            target_aim: Vec3::new(4.0, 1.1, 0.0),
        };
        let mut calls = 0;
        let mut candidate_perception = |_: TargetPawn| {
            calls += 1;
            Some(expected)
        };

        let selected = select_target(
            None,
            &offers,
            &registry,
            None,
            &mut CandidateScope::for_validation(),
            &mut candidate_perception,
        )
        .expect("fresh target");

        assert_eq!(selected.target.entity, pawn);
        assert_eq!(selected.fresh_perception, Some(expected));

        let retained = target_candidate(&registry, pawn, Vec3::ZERO).expect("retained target");
        let empty_offers = target_offers(&registry, Vec3::ZERO, 1.0, Some(pawn));
        let retained_selection = select_target(
            Some(retained),
            &empty_offers,
            &registry,
            None,
            &mut CandidateScope::for_validation(),
            &mut candidate_perception,
        )
        .expect("retained target");
        assert_eq!(calls, 1, "retention does not evaluate fresh-candidate LOS");
        assert_eq!(retained_selection.target.entity, pawn);
        assert_eq!(retained_selection.fresh_perception, None);
    }

    #[test]
    fn retained_due_switch_carries_the_challengers_los_result() {
        let mut registry = EntityRegistry::new();
        let retained_entity = pawn(&mut registry, 10.0);
        let challenger = pawn(&mut registry, 2.0);
        let retained =
            target_candidate(&registry, retained_entity, Vec3::ZERO).expect("retained target");
        let offers = target_offers(&registry, Vec3::ZERO, 1.0, Some(retained_entity));
        let mut candidate_perception = |target: TargetPawn| {
            Some(RawTargetPerception {
                target: target.entity,
                visible: true,
                enemy_eye: Vec3::Y,
                target_aim: target.position + Vec3::Y,
            })
        };

        let selected = select_target(
            Some(retained),
            &offers,
            &registry,
            None,
            &mut CandidateScope::for_validation(),
            &mut candidate_perception,
        )
        .expect("closer fresh challenger");

        assert_eq!(selected.target.entity, challenger);
        assert_eq!(
            selected
                .fresh_perception
                .map(|perception| perception.target),
            Some(challenger),
            "the retained-due switch reuses the challenger's acquisition ray",
        );
    }

    #[test]
    fn selection_keeps_retained_target_until_a_fresh_candidate_beats_hysteresis() {
        let mut registry = EntityRegistry::new();
        let retained = pawn(&mut registry, 10.0);
        let near_but_not_meaningfully_closer = pawn(&mut registry, 9.5);
        let (_, selected) = select_target_for_test(
            &registry,
            Vec3::ZERO,
            1.0,
            Some(retained),
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
        let (_, selected) = select_target_for_test(
            &registry,
            Vec3::ZERO,
            1.0,
            Some(retained),
            None,
            &mut CandidateScope::for_validation(),
        );
        assert_eq!(
            selected.map(|target| target.entity),
            Some(near_but_not_meaningfully_closer)
        );
    }

    #[test]
    fn fresh_acquisition_skips_friendlies_so_they_do_not_mask_hostiles() {
        let mut registry = EntityRegistry::new();
        let friendly = pawn(&mut registry, 2.0);
        let hostile = pawn(&mut registry, 5.0);
        registry
            .entity_state_mut(friendly)
            .unwrap()
            .set(super::super::FACTION_STATE_FIELD, 1.0);

        let (nearest_for_stride, selected) = select_target_for_test(
            &registry,
            Vec3::ZERO,
            1.0,
            None,
            None,
            &mut CandidateScope::for_validation(),
        );
        assert_eq!(
            selected.map(|target| target.entity),
            Some(hostile),
            "a nearer friendly cannot mask a farther hostile candidate"
        );
        assert_eq!(
            nearest_for_stride.map(|candidate| candidate.target.entity),
            Some(hostile),
            "a friendly is not an offered candidate and cannot price the stride"
        );

        registry
            .entity_state_mut(hostile)
            .unwrap()
            .set(super::super::FACTION_STATE_FIELD, 1.0);
        let (_, selected) = select_target_for_test(
            &registry,
            Vec3::ZERO,
            1.0,
            None,
            None,
            &mut CandidateScope::for_validation(),
        );
        assert!(selected.is_none(), "a friendly is never freshly acquired");
    }

    #[test]
    fn retained_target_stays_selected_after_its_faction_turns_friendly() {
        let mut registry = EntityRegistry::new();
        let retained = pawn(&mut registry, 2.0);
        registry
            .entity_state_mut(retained)
            .unwrap()
            .set(super::super::FACTION_STATE_FIELD, 1.0);

        let (_, selected) = select_target_for_test(
            &registry,
            Vec3::ZERO,
            1.0,
            Some(retained),
            None,
            &mut CandidateScope::for_validation(),
        );
        assert_eq!(
            selected.map(|target| target.entity),
            Some(retained),
            "retention deliberately bypasses the fresh-acquisition hostility filter"
        );
    }
}
