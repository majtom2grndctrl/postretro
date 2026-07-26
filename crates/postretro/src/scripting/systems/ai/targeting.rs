// Enemy target-pawn selection: nearest/retained candidate ranking with
// switch hysteresis, feeding the brain tick's per-enemy target.
// See: context/lib/entity_model.md §7c (enemy brain component)

use glam::Vec3;

use super::engine_floor::{is_meaningfully_closer, think_stride_for_distance};
use crate::nav::distance_xz;
use postretro_entities::ComponentKind;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::{EntityId, EntityRegistry, Transform};

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
    leash_range: Option<f32>,
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
            if leash_range.is_none_or(|leash| candidate.distance <= leash)
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

pub(super) fn retained_is_outside_leash(
    candidate: TargetCandidate,
    leash_range: Option<f32>,
) -> bool {
    leash_range.is_some_and(|leash| candidate.distance > leash)
}

/// Select the player pawn this enemy should pursue.
///
/// This is the AI targeting extension point: v1 ranks all
/// [`ComponentKind::PlayerMovement`] pawns by nearest XZ distance from `from`.
/// The optional predicate is the future visibility/relevance seam intended for
/// `context/research/cell-visibility-substrate.md` (and exact LOS work) without
/// re-threading the FSM. It returns both the nearest offered candidate, without
/// applying the leash, and the leash-eligible selected target. The former prices
/// the think stride; the latter governs acquisition. If `retained_target` is
/// still a valid, leash-eligible player pawn, it is preferred unless another
/// pawn is meaningfully closer by
/// [`super::engine_floor::TARGET_SWITCH_HYSTERESIS_DISTANCE`]. When
/// `retained_outside_leash` is true, the retained pawn is excluded from both
/// scan results for this acquisition tick. This targeting path intentionally
/// does not consult the registry's local-player marker, which is client-side
/// convenience state.
pub(crate) fn select_target(
    registry: &EntityRegistry,
    from: Vec3,
    retained_target: Option<EntityId>,
    retained_outside_leash: bool,
    visible: Option<&dyn Fn(EntityId) -> bool>,
    leash_range: Option<f32>,
) -> (Option<TargetCandidate>, Option<TargetPawn>) {
    let retained = retained_target
        .filter(|_| !retained_outside_leash)
        .and_then(|entity| target_candidate(registry, entity, from, visible))
        .filter(|candidate| !retained_is_outside_leash(*candidate, leash_range));
    let (nearest_offered, nearest_eligible) = nearest_target_candidate(
        registry,
        from,
        visible,
        retained_target.filter(|_| retained_outside_leash),
        leash_range,
    );

    let selected = match (retained, nearest_eligible) {
        (Some(retained), Some(nearest))
            if nearest.target.entity != retained.target.entity
                && is_meaningfully_closer(nearest.distance, retained.distance) =>
        {
            Some(nearest.target)
        }
        (Some(retained), _) => Some(retained.target),
        (None, Some(nearest)) => Some(nearest.target),
        (None, None) => None,
    };

    (nearest_offered, selected)
}
