// Validate the effective reload snapshot and retained movement before commit.
// See: context/lib/scripting.md and context/lib/movement.md.

use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry, EntityTypeDescriptor};
use postretro_foundation::{LifeStatePosture, PlayerMovementComponent};

use crate::refresh_plan::{DescriptorRefreshAction, DescriptorRefreshPlan};

pub(super) fn validate_life_state_names(
    descriptors: &[EntityTypeDescriptor],
    snapshot: &str,
) -> Result<(), String> {
    crate::data_descriptors::validate_life_state_name_limit(descriptors)
        .map_err(|error| format!("{snapshot}: {error}"))
}

/// A rejected or skipped movement refresh still adopts the committed profiles.
/// Add only missing crouch support, preserving the retained standing shape and
/// integration state. A shrink needs no collision query at commit; the movement
/// tick applies its normal anchored resize and eye easing.
pub(super) fn plan_retained_crouch_support(
    descriptors: &[EntityTypeDescriptor],
    registry: &EntityRegistry,
    plan: &mut DescriptorRefreshPlan,
) -> Result<(), String> {
    let mut support = Vec::new();
    for (pawn, _) in registry.query_by_component_and_tag(ComponentKind::PlayerMovement, None) {
        let Ok(provenance) = registry.get_component::<DescriptorProvenance>(pawn) else {
            continue;
        };
        let Some((index, descriptor)) = descriptors.iter().enumerate().find(|(_, descriptor)| {
            descriptor.canonical_name.as_deref() == Some(provenance.canonical_name.as_str())
        }) else {
            continue;
        };
        let Some((state, _)) = descriptor.life_states.as_ref().and_then(|block| {
            block
                .states
                .iter()
                .find(|(_, profile)| profile.movement.posture == LifeStatePosture::Crouch)
        }) else {
            continue;
        };

        let mut effective = registry.get_component::<PlayerMovementComponent>(pawn).ok();
        for action in &plan.actions {
            match action {
                DescriptorRefreshAction::Replace {
                    entity,
                    component: ComponentValue::PlayerMovement(movement),
                } if *entity == pawn => effective = Some(movement.as_ref()),
                DescriptorRefreshAction::Remove { entity, kind }
                    if *entity == pawn && *kind == ComponentKind::PlayerMovement =>
                {
                    effective = None;
                }
                _ => {}
            }
        }
        let Some(effective) = effective else {
            continue;
        };
        if effective.crouch.is_some() {
            continue;
        }
        let path =
            format!("entities[{index}].components.lifeStates.states.{state}.movement.posture");
        let Some(crouch) = descriptor
            .movement
            .as_ref()
            .and_then(|movement| movement.crouch.as_ref())
        else {
            return Err(format!(
                "{path}: pawn {pawn} `{}` retains movement without crouch support; declare components.movement.crouch",
                provenance.canonical_name,
            ));
        };
        if crouch.half_height > effective.standing_half_height
            || crouch.eye_height > effective.standing_eye_height
            || crouch.eye_height > crouch.half_height + effective.capsule.radius
        {
            return Err(format!(
                "{path}: pawn {pawn} `{}` retains its previous movement; components.movement.crouch must fit its retained standing half-height {}, eye-height {}, and radius {} (incoming crouch: {}, {}); reduce the crouch dimensions or reload the level",
                provenance.canonical_name,
                effective.standing_half_height,
                effective.standing_eye_height,
                effective.capsule.radius,
                crouch.half_height,
                crouch.eye_height,
            ));
        }
        let mut patched = effective.clone();
        patched.crouch = Some(crouch.clone());
        support.push(DescriptorRefreshAction::Replace {
            entity: pawn,
            component: ComponentValue::PlayerMovement(Box::new(patched)),
        });
    }
    plan.actions.extend(support);
    Ok(())
}

#[cfg(test)]
mod tests;
