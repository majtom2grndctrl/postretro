//! Install-time trap weapon resolution and the `start_on_spawn` request.
//! See: context/lib/entity_model.md §Components (ProjectileTrap, Weapon vocabulary)

use postretro_entities::components::projectile_trap::{TrapRequest, TrapRuntime};
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry};
use postretro_foundation::ResolutionMode;
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;

use super::fold_trap_request;
use crate::scripting::builtins::data_archetype::find_descriptor;

/// Counts of one install's traps by outcome.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TrapInstallReport {
    pub armed: usize,
    pub inert: usize,
}

/// Why a trap's weapon cannot arm it.
fn refusal(descriptors: &[EntityTypeDescriptor], name: &str) -> Result<(), &'static str> {
    let weapon = find_descriptor(descriptors, name)
        .and_then(|descriptor| descriptor.weapon.as_ref())
        .ok_or("names no weapon descriptor")?;
    if weapon.resolution != ResolutionMode::Projectile || weapon.projectile.is_none() {
        return Err("is not a projectile weapon");
    }
    if weapon.primary.charge.is_some() {
        return Err("has a charged primary action, which a trap cannot release");
    }
    Ok(())
}

/// Resolve every trap's weapon against this level's descriptors, once per
/// install. A valid projectile weapon arms the trap with a private component
/// built from a resource-free copy of its descriptor; anything else leaves it
/// inert with one warning. `authoritative` (host or single player) also issues
/// each `start_on_spawn` trap's `start`, the install's first trap request; a
/// connected client issues nothing.
pub fn resolve_traps_for_level(
    registry: &mut EntityRegistry,
    descriptors: &[EntityTypeDescriptor],
    authoritative: bool,
) -> TrapInstallReport {
    let mut report = TrapInstallReport::default();
    let traps: Vec<EntityId> = registry
        .iter_with_kind(ComponentKind::ProjectileTrap)
        .map(|(id, _)| id)
        .collect();
    for id in traps {
        let Ok(ComponentValue::ProjectileTrap(trap)) =
            registry.get_component_value_mut(id, ComponentKind::ProjectileTrap)
        else {
            continue;
        };
        trap.runtime = None;
        trap.pending = Default::default();
        if !trap.timing_valid {
            // Parse already warned once for the invalid timing KVP.
            report.inert += 1;
            continue;
        }
        if let Err(reason) = refusal(descriptors, &trap.weapon_name) {
            log::warn!(
                "[Loader] projectile_trap {id}: weapon `{}` {reason}; the trap is inert",
                trap.weapon_name
            );
            report.inert += 1;
            continue;
        }
        let mut descriptor = find_descriptor(descriptors, &trap.weapon_name)
            .and_then(|descriptor| descriptor.weapon.clone())
            .expect("refusal admitted a weapon descriptor");
        // A trap pays nothing: with no resource every shot's cost is `None`,
        // so it never reloads, dry-fires or overheats.
        descriptor.resource = None;
        trap.runtime = Some(Box::new(TrapRuntime {
            canonical_weapon: trap.weapon_name.clone(),
            weapon: WeaponComponent::from_descriptor_with_canonical(
                &descriptor,
                Some(&trap.weapon_name),
            ),
            schedule: None,
            clock: 0,
        }));
        if authoritative && trap.start_on_spawn {
            fold_trap_request(
                &mut trap.pending,
                TrapRequest::Start {
                    interval_ms: None,
                    phase_ms: None,
                },
            );
        }
        report.armed += 1;
    }
    report
}
