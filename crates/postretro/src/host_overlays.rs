//! Hosted overlay damage intake and scoped canonical network identities.
//! See: context/lib/networking.md, shared combat presentation.

use std::cell::{Ref, RefCell};
use std::collections::HashSet;

use postretro_entities::presentation_overrides::PresentationOverrides;
use postretro_entities::{EntityId, EntityRegistry};

use crate::impact_policy::{ClientOverlayConfig, DamagedEnemyOverlayDamage};
use crate::netcode::{
    HostOverlayFactTracker, NetworkIdAllocator, OpenAuthorizedShots, ReplicableSet,
};

struct IdentityState<'a> {
    allocator: &'a mut NetworkIdAllocator,
    replicable: &'a mut ReplicableSet,
    tracked: &'a mut HashSet<EntityId>,
}

pub(crate) struct HostOverlayIdentities<'a> {
    state: RefCell<IdentityState<'a>>,
}

#[cfg(test)]
mod tests;

/// Whether remote hits can land this tick, so registration must run before
/// intake: a ready declaration, or an open march, which can release a blast on
/// a tick that ingests no declaration.
pub(crate) fn remote_hits_may_land(
    declarations_ready: bool,
    open_shots: &OpenAuthorizedShots,
) -> bool {
    declarations_ready || open_shots.has_marches()
}

impl<'a> HostOverlayIdentities<'a> {
    pub(crate) fn new(
        allocator: &'a mut NetworkIdAllocator,
        replicable: &'a mut ReplicableSet,
        tracked: &'a mut HashSet<EntityId>,
    ) -> Self {
        Self {
            state: RefCell::new(IdentityState {
                allocator,
                replicable,
                tracked,
            }),
        }
    }

    pub(crate) fn allocator(&self) -> Ref<'_, NetworkIdAllocator> {
        Ref::map(self.state.borrow(), |state| &*state.allocator)
    }

    /// A remote splash can hit NPCs spawned by triggers earlier this tick.
    /// Register them before HIT intake holds its immutable allocator borrow.
    /// Call only when [`remote_hits_may_land`]; ordinary damage registers one target.
    pub(crate) fn register_before_remote_hits(
        &self,
        registry: &EntityRegistry,
        overrides: &mut PresentationOverrides,
    ) {
        let mut state = self.state.borrow_mut();
        let IdentityState {
            allocator,
            replicable,
            tracked,
        } = &mut *state;
        crate::netcode::host_register_descriptor_entities(
            registry, allocator, replicable, tracked, overrides,
        );
    }

    pub(crate) fn record_damage(
        &self,
        registry: &EntityRegistry,
        damage: DamagedEnemyOverlayDamage,
        overrides: &mut PresentationOverrides,
        tracker: &mut HostOverlayFactTracker,
        config: &ClientOverlayConfig,
    ) {
        // Consequences may already have removed this target or acquired it.
        if !crate::netcode::is_host_replicated_descriptor_entity(registry, damage.entity) {
            return;
        }
        let existing = self.allocator().network_id_for_entity(damage.entity);
        let network_id = match existing {
            Some(network_id) => network_id,
            None => {
                let mut state = self.state.borrow_mut();
                let IdentityState {
                    allocator,
                    replicable,
                    tracked,
                } = &mut *state;
                crate::netcode::host_register_descriptor_entity(
                    registry,
                    allocator,
                    replicable,
                    tracked,
                    overrides,
                    damage.entity,
                );
                let Some(network_id) = allocator.network_id_for_entity(damage.entity) else {
                    return;
                };
                network_id
            }
        };
        tracker.record_damage(
            registry,
            &self.allocator(),
            network_id,
            damage,
            config.linger_seconds,
            config.max_visible,
        );
    }
}
