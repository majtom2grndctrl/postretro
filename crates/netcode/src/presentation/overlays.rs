//! Shared, bounded damaged-enemy state. Selection belongs to the host; pools
//! only project that selection. Whole-roster revisions replace per-id histories.

use std::collections::HashMap;

use glam::Vec3;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::NavAgentParams;
use postretro_net::transport::NetServer;
use postretro_net::wire::{
    NetworkId, ServerPresentationMessage, ServerPresentationPayload, WireEnemyOverlay,
    WireOverlayRoster,
};
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;

use super::{ClientOverlayHitbox, client_overlay_anchor, client_overlay_hitbox};
use crate::impact_policy::{
    ClientOverlayConfig, DamagedEnemyOverlayDamage, DamagedEnemyOverlayFact, ImpactPolicyRuntime,
};
use crate::presentation_pool::PresentationPool;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::{ClientReplication, ReplicableSet};

/// Changed state is capped at ten publications/second. Full current state is
/// repaired four times/second, including empty state and new participants.
const CHANGED_INTERVAL_SECONDS: f64 = 0.1;
const REPAIR_INTERVAL_SECONDS: f64 = 0.25;

#[derive(Debug, Clone, Copy)]
struct HostLiveOverlay {
    entity: EntityId,
    network_id: NetworkId,
    damaged_at: f64,
    expires_at: f64,
    fact: DamagedEnemyOverlayFact,
}

/// One host roster, independent of source ownership, recipients and rendering.
/// Live selection, lowered state and scratch each retain at most authored K.
#[derive(Debug)]
pub struct HostOverlayFactTracker {
    live: Vec<HostLiveOverlay>,
    roster: WireOverlayRoster,
    scratch: Vec<WireEnemyOverlay>,
    last_published_revision: u64,
    last_published_at: Option<f64>,
}

impl Default for HostOverlayFactTracker {
    fn default() -> Self {
        Self {
            live: Vec::new(),
            roster: WireOverlayRoster {
                revision: 1,
                server_script_time: 0.0,
                entries: Vec::new(),
            },
            scratch: Vec::new(),
            last_published_revision: 0,
            last_published_at: None,
        }
    }
}

impl HostOverlayFactTracker {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Called synchronously after each impact policy. No per-hit queue or
    /// recipient map accumulates. Equal-time overflow keeps the highest ids.
    pub fn record_damage(
        &mut self,
        registry: &EntityRegistry,
        network_id: NetworkId,
        damage: DamagedEnemyOverlayDamage,
        linger_seconds: f64,
        max_visible: usize,
    ) {
        let now = damage.damaged_at;
        if max_visible == 0
            || !now.is_finite()
            || now < 0.0
            || !linger_seconds.is_finite()
            || linger_seconds <= 0.0
            || !eligible(registry, damage.entity)
        {
            return;
        }
        self.live
            .retain(|entry| entry.expires_at > now && eligible(registry, entry.entity));
        let expires_at = now + linger_seconds;
        if !expires_at.is_finite() {
            return;
        }
        if let Some(entry) = self
            .live
            .iter_mut()
            .find(|entry| entry.network_id == network_id)
        {
            if now >= entry.damaged_at {
                entry.damaged_at = now;
                entry.expires_at = expires_at;
            }
            return;
        }
        if self.live.len() >= max_visible {
            let Some((index, victim)) = self.live.iter().enumerate().min_by(|(_, a), (_, b)| {
                a.damaged_at
                    .total_cmp(&b.damaged_at)
                    .then_with(|| a.network_id.0.cmp(&b.network_id.0))
            }) else {
                return;
            };
            if (now, network_id.0) <= (victim.damaged_at, victim.network_id.0) {
                return;
            }
            self.live.swap_remove(index);
        }
        self.live.push(HostLiveOverlay {
            entity: damage.entity,
            network_id,
            damaged_at: now,
            expires_at,
            fact: DamagedEnemyOverlayFact {
                entity: damage.entity,
                health_fraction: 1.0,
                shield_fraction: 0.0,
                has_shield: false,
                alive: true,
            },
        });
    }

    fn prune(&mut self, registry: &EntityRegistry, replicable: &ReplicableSet, now: f64) {
        self.live.retain(|entry| {
            entry.expires_at > now
                && replicable.contains(entry.entity)
                && eligible(registry, entry.entity)
        });
    }

    fn lower(&mut self, facts: &[DamagedEnemyOverlayFact]) {
        self.live.retain_mut(|entry| {
            let Some(fact) = facts
                .iter()
                .find(|fact| fact.entity == entry.entity && fact.alive)
            else {
                return false;
            };
            entry.fact = *fact;
            true
        });
        self.live.sort_unstable_by_key(|entry| entry.network_id.0);
        self.scratch.clear();
        self.scratch
            .extend(self.live.iter().map(|entry| WireEnemyOverlay {
                enemy_id: entry.network_id,
                health_fraction: entry.fact.health_fraction,
                shield_fraction: entry.fact.shield_fraction,
                has_shield: entry.fact.has_shield,
                damaged_at: entry.damaged_at,
                expires_at: entry.expires_at,
            }));
        if self.scratch != self.roster.entries {
            std::mem::swap(&mut self.scratch, &mut self.roster.entries);
            self.roster.revision = self.roster.revision.saturating_add(1);
        }
    }

    fn publication(&mut self, now: f64) -> Option<ServerPresentationMessage> {
        let interval = if self.last_published_revision != self.roster.revision {
            CHANGED_INTERVAL_SECONDS
        } else {
            REPAIR_INTERVAL_SECONDS
        };
        if self
            .last_published_at
            .is_some_and(|last| now - last + 1.0e-9 < interval)
        {
            return None;
        }
        self.last_published_at = Some(now);
        self.last_published_revision = self.roster.revision;
        self.roster.server_script_time = now;
        debug_assert!(self.roster.is_valid());
        Some(ServerPresentationMessage {
            payload: ServerPresentationPayload::OverlayRoster(self.roster.clone()),
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn tracked_len(&self) -> usize {
        self.live.len()
    }
}

fn eligible(registry: &EntityRegistry, entity: EntityId) -> bool {
    !registry.is_inventory_owned(entity)
        && registry
            .get_component::<HealthComponent>(entity)
            .is_ok_and(|health| health.current.is_finite() && health.current > 0.0)
}

/// Run after terminal frame-end removal. Read selected facts once, project the
/// same roster locally, then offer one shared encoded publication to all peers.
#[allow(clippy::too_many_arguments)]
pub fn update_host_overlay_roster(
    tracker: &mut HostOverlayFactTracker,
    runtime: &mut ImpactPolicyRuntime,
    registry: &EntityRegistry,
    replicable: &ReplicableSet,
    pool: &mut PresentationPool,
    hit_zones: &HitZoneStore,
    server: &mut NetServer,
    now: f64,
    anim_time: f64,
) {
    let Some(config) = runtime.client_overlay_config() else {
        tracker.clear();
        pool.clear_overlays();
        return;
    };
    tracker.prune(registry, replicable, now);
    let facts = runtime.sample_damaged_enemy_overlay_facts(
        registry,
        tracker.live.iter().map(|entry| entry.entity),
    );
    tracker.lower(&facts);
    pool.retain_overlays(|entity| tracker.live.iter().any(|entry| entry.entity == entity));
    for entry in &tracker.live {
        if !pool.upsert_authoritative_overlay(
            entry.entity,
            config.template.clone(),
            config.max_visible,
        ) {
            continue;
        }
        let hitbox = registry
            .get_component::<HealthComponent>(entry.entity)
            .ok()
            .and_then(|health| health.hitbox.as_ref())
            .map(|hitbox| ClientOverlayHitbox {
                half_extents: hitbox.half_extents,
                offset: hitbox.offset,
            });
        let anchor = client_overlay_anchor(
            registry,
            hit_zones,
            entry.entity,
            &config.world_anchor,
            hitbox,
            anim_time,
        );
        stamp(
            pool,
            entry.entity,
            entry.fact.health_fraction,
            entry.fact.shield_fraction,
            entry.fact.has_shield,
            anchor,
            &config,
        );
    }
    if let Some(message) = tracker.publication(now) {
        server.broadcast_presentation(message);
    }
}

/// Latest whole state plus at most K current entity projections. No per-id
/// terminal ledger, fixed pre-baseline TTL, or receive-time linger inference.
#[derive(Debug, Default)]
pub struct ClientOverlayFactState {
    roster: Option<WireOverlayRoster>,
    projected: HashMap<NetworkId, EntityId>,
    host_time_high_water: Option<f64>,
}

impl ClientOverlayFactState {
    pub fn clear(&mut self) {
        self.roster = None;
        self.projected.clear();
        self.host_time_high_water = None;
    }

    pub fn receive(&mut self, roster: WireOverlayRoster, max_visible: usize) -> bool {
        if !roster.is_valid() || roster.entries.len() > max_visible {
            return false;
        }
        match self.roster.as_mut() {
            Some(current) if roster.revision < current.revision => false,
            Some(current) if roster.revision == current.revision => {
                // A repair may advance the clock floor but never reinsert an
                // expired entry or reset original damage/expiry facts.
                current.server_script_time =
                    current.server_script_time.max(roster.server_script_time);
                false
            }
            _ => {
                self.roster = Some(roster);
                true
            }
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn revision(&self) -> Option<u64> {
        self.roster.as_ref().map(|roster| roster.revision)
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn pending_len(&self) -> usize {
        self.roster.as_ref().map_or(0, |roster| {
            roster
                .entries
                .iter()
                .filter(|entry| !self.projected.contains_key(&entry.enemy_id))
                .count()
        })
    }
}

#[allow(clippy::too_many_arguments)]
pub fn update_client_overlay_anchors(
    registry: &EntityRegistry,
    descriptors: &[EntityTypeDescriptor],
    state: &mut ClientOverlayFactState,
    replication: &ClientReplication,
    pool: &mut PresentationPool,
    config: Option<&ClientOverlayConfig>,
    hit_zones: &HitZoneStore,
    agent_params: Option<NavAgentParams>,
    anim_time: f64,
    local_script_time: f64,
    rtt_micros: Option<f64>,
) {
    let Some(config) = config else {
        state.clear();
        pool.clear_overlays();
        return;
    };
    let Some(roster) = state.roster.as_mut() else {
        return;
    };
    let Some(host_now) = replication.presentation_host_time(local_script_time, rtt_micros) else {
        return;
    };
    // Snapshot anchors can regress after stalls/RTT corrections. A bar already
    // expired in this participation must never return on a same-state repair.
    let host_now = host_now
        .max(roster.server_script_time)
        .max(state.host_time_high_water.unwrap_or(0.0));
    state.host_time_high_water = Some(host_now);
    roster.entries.retain(|entry| entry.expires_at > host_now);
    state.projected.retain(|network_id, _| {
        roster
            .entries
            .iter()
            .any(|entry| entry.enemy_id == *network_id)
    });
    for entry in &roster.entries {
        if let Some(entity) = replication
            .entity_for_network_id(entry.enemy_id)
            .filter(|entity| registry.exists(*entity))
        {
            state.projected.insert(entry.enemy_id, entity);
        } else {
            state.projected.remove(&entry.enemy_id);
        }
    }
    pool.retain_overlays(|entity| state.projected.values().any(|current| *current == entity));
    for entry in &roster.entries {
        let Some(&entity) = state.projected.get(&entry.enemy_id) else {
            continue;
        };
        if !pool.upsert_authoritative_overlay(entity, config.template.clone(), config.max_visible) {
            continue;
        }
        let anchor = client_overlay_anchor(
            registry,
            hit_zones,
            entity,
            &config.world_anchor,
            client_overlay_hitbox(replication, descriptors, entry.enemy_id, agent_params),
            anim_time,
        );
        stamp(
            pool,
            entity,
            entry.health_fraction,
            entry.shield_fraction,
            entry.has_shield,
            anchor,
            config,
        );
    }
}

fn stamp(
    pool: &mut PresentationPool,
    entity: EntityId,
    health: f32,
    shield: f32,
    has_shield: bool,
    anchor: Option<Vec3>,
    config: &ClientOverlayConfig,
) {
    if let Some(anchor) = anchor {
        pool.stamp_damaged_enemy_overlay(
            entity,
            health,
            shield,
            has_shield,
            anchor,
            config.hide_at_full && health == 1.0,
        );
    } else {
        pool.update_overlay_anchor(entity, None, true);
    }
}

#[cfg(test)]
#[path = "overlay_tests.rs"]
mod tests;
