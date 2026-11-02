// Validate the effective reload snapshot and retained movement before commit.
// See: context/lib/scripting.md and context/lib/movement.md.

use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry, EntityTypeDescriptor};
use postretro_foundation::{LifeStatePosture, PlayerMovementComponent};

use crate::refresh_plan::{DescriptorRefreshAction, DescriptorRefreshPlan};

/// Check the 255-name limit on the authored next-level snapshot, then on the
/// active level's union, which also keeps removed presentation-bearing
/// descriptors until the next level load. Neither slice is the manifest
/// array, so errors name descriptors, never indexes.
pub(super) fn validate_life_state_snapshots(
    incoming: &[EntityTypeDescriptor],
    active: &[EntityTypeDescriptor],
) -> Result<(), String> {
    use crate::data_descriptors::{LIFE_STATE_NAME_LIMIT, first_life_state_name_overflow};

    if let Some((index, name)) = first_life_state_name_overflow(incoming) {
        return Err(format!(
            "next-level descriptor snapshot: {} `components.lifeStates.states.{name}` {LIFE_STATE_NAME_LIMIT}",
            descriptor_label(&incoming[index]),
        ));
    }
    let Some((index, name)) = first_life_state_name_overflow(active) else {
        return Ok(());
    };
    let descriptor = &active[index];
    let retained = descriptor
        .canonical_name
        .as_deref()
        .is_some_and(|canonical| {
            !incoming
                .iter()
                .any(|next| next.canonical_name.as_deref() == Some(canonical))
        });
    let origin = if retained {
        ", retained from the previous snapshot until the next level load,"
    } else {
        ""
    };
    Err(format!(
        "active-level descriptor snapshot (including retained presentation descriptors): {}{origin} keeps `components.lifeStates.states.{name}`, which {LIFE_STATE_NAME_LIMIT}; reload the level to drop retained descriptors",
        descriptor_label(descriptor),
    ))
}

fn descriptor_label(descriptor: &EntityTypeDescriptor) -> String {
    match descriptor.canonical_name.as_deref() {
        Some(name) => format!("descriptor `{name}`"),
        None => "an unnamed descriptor".to_string(),
    }
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
        let Some(descriptor) = descriptors.iter().find(|descriptor| {
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
        // The descriptor list is the active union, not the manifest array, so
        // name the descriptor rather than an index into it.
        let path = format!(
            "descriptor `{}` components.lifeStates.states.{state}.movement.posture",
            provenance.canonical_name,
        );
        let Some(crouch) = descriptor
            .movement
            .as_ref()
            .and_then(|movement| movement.crouch.as_ref())
        else {
            return Err(format!(
                "{path}: pawn {pawn} retains movement without crouch support; declare components.movement.crouch or reload the level"
            ));
        };
        if crouch.half_height > effective.standing_half_height
            || crouch.eye_height > effective.standing_eye_height
            || crouch.eye_height > crouch.half_height + effective.capsule.radius
        {
            return Err(format!(
                "{path}: pawn {pawn} retains its previous movement; components.movement.crouch must fit its retained standing half-height {}, eye-height {}, and radius {} (incoming crouch: {}, {}); reduce the crouch dimensions or reload the level",
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
