// Directional travel clips: under target facing, activity animation `X` plays
// as `X_left`, `X_right` or `X_backward` by travel relative to facing.
// See: context/lib/entity_model.md §7c (Engaged motion and facing)

use glam::Vec3;
use postretro_entities::components::brain::TravelVariant;
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::FacingPolicy;

use super::LocomotionIntent;

/// Travel within this angle of facing plays the base clip.
const FORWARD_SECTOR_RADIANS: f32 = std::f32::consts::FRAC_PI_4;
/// Travel at least this far from facing plays the backward clip; between the
/// two sectors plays the side clip.
const BACKWARD_SECTOR_RADIANS: f32 = 3.0 * std::f32::consts::FRAC_PI_4;

/// The variant this tick's travel asks for. Only a target-facing mover whose
/// steering moves reads its travel against facing: forward within 45°,
/// backward from 135°, and the side between, left being counter-clockwise
/// seen from above (+yaw), the mesh's own left. Travel facing plays `X`.
pub(super) fn classify(
    policy: Option<FacingPolicy>,
    steering_velocity: Option<Vec3>,
    facing_yaw: f32,
) -> TravelVariant {
    let Some(velocity) = steering_velocity.filter(|velocity| {
        policy == Some(FacingPolicy::Target) && LocomotionIntent::from_velocity(*velocity).moving
    }) else {
        return TravelVariant::Base;
    };
    let travel_yaw = velocity.x.atan2(velocity.z);
    let relative = (travel_yaw - facing_yaw + std::f32::consts::PI)
        .rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI;
    if !relative.is_finite() || relative.abs() <= FORWARD_SECTOR_RADIANS {
        TravelVariant::Base
    } else if relative.abs() >= BACKWARD_SECTOR_RADIANS {
        TravelVariant::Backward
    } else if relative > 0.0 {
        TravelVariant::Left
    } else {
        TravelVariant::Right
    }
}

/// The variant the mesh can play: `wanted` when it declares a usable
/// `{base}{suffix}` state, else the base clip. A missing variant is ordinary
/// content, so it never warns.
pub(super) fn presentable(
    registry: &EntityRegistry,
    id: EntityId,
    base: Option<&str>,
    wanted: TravelVariant,
) -> TravelVariant {
    let Some(base) = base.filter(|_| wanted != TravelVariant::Base) else {
        return TravelVariant::Base;
    };
    let suffix = wanted.suffix();
    // Compare the name piecewise rather than composing it: this runs every
    // tick a target-facing mover travels, and the composed name is needed
    // only on the switch edge.
    let usable = registry
        .get_component::<MeshComponent>(id)
        .ok()
        .and_then(|mesh| mesh.animation.as_ref())
        .is_some_and(|animation| {
            animation.states.iter().any(|(name, state)| {
                state.clip_index.is_some()
                    && name.len() == base.len() + suffix.len()
                    && name.starts_with(base)
                    && name.ends_with(suffix)
            })
        });
    if usable { wanted } else { TravelVariant::Base }
}
