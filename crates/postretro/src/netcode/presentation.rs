// Host-to-client routing and engine conversion for passive presentation events.
// See: context/lib/networking.md

use std::collections::{BTreeMap, HashMap};

use glam::Vec3;
use postretro_entities::{
    EntityId, EntityRegistry, PresentationFact, PresentationFade, PresentationMotion,
    PresentationSpawn, PresentationTemplateHandle,
};
use postretro_foundation::NavAgentParams;
use postretro_net::transport::NetServer;
use postretro_net::wire::{
    NetworkId, PresentationFact as WirePresentationFact, ServerPresentationMessage,
    ServerPresentationPayload,
};
use postretro_scripting_core::data_descriptors::{
    EntityTypeDescriptor, HitboxDescriptor, PresentationTemplate,
};

#[cfg(test)]
use crate::impact_policy::DamagedEnemyOverlayDamage;
use crate::impact_policy::{
    ClientOverlayConfig, DamagedEnemyOverlayFact, DamagedEnemyOverlayFrame,
};
use crate::presentation_pool::PresentationPool;
use crate::scripting::builtins::data_archetype::ai_capsule_center_from_feet_offset;
use crate::scripting_systems::hit_zones::{HitZoneStore, model_matrix};

use super::{ClientReplication, MovementOwners, NetworkIdAllocator, ReplicableSet};

/// Retain terminal enemy ids past the presentation channel's expected reorder
/// horizon. The id allocator never recycles, so this must remain a bounded
/// short-lived guard rather than a session ledger.
const CLIENT_OVERLAY_TERMINAL_TTL_SECONDS: f64 = 0.5;
/// A presentation datagram can beat the unreliable snapshot that establishes
/// its entity mapping. Retain that already-received fact only across the same
/// short reorder horizon; this is not a retransmit or reliable event queue.
const CLIENT_OVERLAY_PENDING_TTL_SECONDS: f64 = 0.5;

/// A decoded overlay fact separated from the wire envelope so the adverse-order
/// behavior is directly testable without transport or registry setup.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ClientOverlayFact {
    enemy_id: NetworkId,
    health_fraction: f32,
    shield_fraction: f32,
    has_shield: bool,
    alive: bool,
}

#[derive(Debug, Clone, Copy)]
struct ClientLiveOverlay {
    entity: EntityId,
    fact: ClientOverlayFact,
}

#[derive(Debug, Clone, Copy)]
struct PendingClientOverlayFact {
    fact: ClientOverlayFact,
    queued_at: f64,
}

#[derive(Debug, Clone, Copy)]
struct ClientOverlayHitbox {
    half_extents: Vec3,
    offset: Vec3,
}

impl ClientOverlayFact {
    pub(crate) fn new(
        enemy_id: NetworkId,
        health_fraction: f32,
        shield_fraction: f32,
        has_shield: bool,
        alive: bool,
    ) -> Self {
        Self {
            enemy_id,
            health_fraction,
            shield_fraction,
            has_shield,
            alive,
        }
    }
}

/// Per-recipient fact tuple last placed on the unreliable presentation lane.
/// It is deliberately separate from the local keyed-overlay map: host state
/// remembers delivery suppression, not any client-side presentation state.
#[derive(Debug, Clone, Copy, PartialEq)]
struct OverlayFactTuple {
    health_fraction: f32,
    shield_fraction: f32,
    has_shield: bool,
    alive: bool,
}

impl From<DamagedEnemyOverlayFact> for OverlayFactTuple {
    fn from(fact: DamagedEnemyOverlayFact) -> Self {
        Self {
            health_fraction: fact.health_fraction,
            shield_fraction: fact.shield_fraction,
            has_shield: fact.has_shield,
            alive: fact.alive,
        }
    }
}

/// Client-only ordering state for host-pushed enemy-overlay facts. A live
/// instance is keyed by stable `NetworkId`; the pool itself still uses the
/// current local `EntityId` for drawing and anchor storage.
#[derive(Debug, Default)]
pub(crate) struct ClientOverlayFactState {
    terminal_ids: HashMap<NetworkId, f64>,
    live_overlays: HashMap<NetworkId, ClientLiveOverlay>,
    pending_live_facts: HashMap<NetworkId, PendingClientOverlayFact>,
    elapsed_seconds: f64,
}

impl ClientOverlayFactState {
    /// Retire all ordering and identity facts at a level, participation, or
    /// overlay-authoring boundary. None of these ids or pending values may be
    /// interpreted against the next lifecycle's entity/template set.
    pub(crate) fn clear(&mut self) {
        self.terminal_ids.clear();
        self.live_overlays.clear();
        self.pending_live_facts.clear();
        self.elapsed_seconds = 0.0;
    }

    /// Advance the injected game-time clock and drop terminal guards or
    /// pre-baseline facts past the unordered channel's reorder horizon.
    pub(crate) fn advance_terminal_ttl(&mut self, frame_dt_seconds: f32) {
        if frame_dt_seconds.is_finite() && frame_dt_seconds > 0.0 {
            self.elapsed_seconds += f64::from(frame_dt_seconds);
        }
        let now = self.elapsed_seconds;
        self.terminal_ids
            .retain(|_, marked_at| now - *marked_at < CLIENT_OVERLAY_TERMINAL_TTL_SECONDS);
        self.pending_live_facts
            .retain(|_, pending| now - pending.queued_at < CLIENT_OVERLAY_PENDING_TTL_SECONDS);
    }

    fn discard_expired_live_overlays(&mut self, pool: &PresentationPool) {
        self.live_overlays
            .retain(|_, live| pool.has_overlay(live.entity));
    }

    fn queue_pending_live(&mut self, fact: ClientOverlayFact, max_visible: usize) {
        if max_visible == 0 || self.terminal_ids.contains_key(&fact.enemy_id) {
            return;
        }
        if !self.pending_live_facts.contains_key(&fact.enemy_id)
            && self.pending_live_facts.len() >= max_visible
        {
            let victim =
                self.pending_live_facts
                    .iter()
                    .min_by(|(left_id, left), (right_id, right)| {
                        left.queued_at
                            .total_cmp(&right.queued_at)
                            .then_with(|| left_id.0.cmp(&right_id.0))
                    });
            let Some((victim_id, victim)) = victim else {
                return;
            };
            let incoming_rank = (self.elapsed_seconds, fact.enemy_id.0);
            let victim_rank = (victim.queued_at, victim_id.0);
            if incoming_rank <= victim_rank {
                return;
            }
            let victim_id = *victim_id;
            self.pending_live_facts.remove(&victim_id);
        }
        self.pending_live_facts.insert(
            fact.enemy_id,
            PendingClientOverlayFact {
                fact,
                queued_at: self.elapsed_seconds,
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn terminal_len(&self) -> usize {
        self.terminal_ids.len()
    }

    #[cfg(test)]
    pub(crate) fn advance_terminal_ttl_for_test(&mut self, frame_dt_seconds: f32) {
        self.advance_terminal_ttl(frame_dt_seconds);
    }

    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending_live_facts.len()
    }
}
#[derive(Debug, Clone, Copy)]
struct HostLiveOverlay {
    entity: EntityId,
    last_damaged_time_seconds: f64,
}

/// Host-only suppression and ownership bookkeeping for damaged-enemy facts.
///
/// Each recipient owns an independently capped/lingering target set. A late
/// joiner never enters it, because only a source currently owned by that client
/// records a target.
#[derive(Debug, Default)]
pub(crate) struct HostOverlayFactTracker {
    live_overlays: HashMap<(u64, NetworkId), HostLiveOverlay>,
    last_sent: HashMap<(u64, NetworkId), OverlayFactTuple>,
    elapsed_seconds: f64,
}

impl HostOverlayFactTracker {
    /// Retire all level-local recipients and delivery suppression facts.
    pub(crate) fn clear(&mut self) {
        self.live_overlays.clear();
        self.last_sent.clear();
        self.elapsed_seconds = 0.0;
    }

    /// Advance and prune each remote recipient's private overlay stream. This
    /// lifecycle is independent of the host renderer's local overlay pool.
    pub(crate) fn begin_frame(
        &mut self,
        frame_dt_seconds: f32,
        linger_seconds: f64,
        owners: &MovementOwners,
    ) {
        if frame_dt_seconds.is_finite() && frame_dt_seconds > 0.0 {
            self.elapsed_seconds += f64::from(frame_dt_seconds);
        }
        let now = self.elapsed_seconds;
        self.live_overlays.retain(|(client_id, _), live| {
            now - live.last_damaged_time_seconds < linger_seconds
                && owners
                    .iter()
                    .any(|(_, owner_client_id)| owner_client_id == *client_id)
        });
        self.prune_last_sent();
    }

    pub(crate) fn tracked_entities(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.live_overlays.values().map(|live| live.entity)
    }

    /// Fold this frame's dispatch ownership into the tracked recipient set,
    /// then produce only fact tuples that differ for the exact client/enemy
    /// pair. The caller sends each returned event on `Channel::Presentation`.
    fn collect_changed(
        &mut self,
        frame: &DamagedEnemyOverlayFrame,
        allocator: &mut NetworkIdAllocator,
        replicable: &ReplicableSet,
        owners: &MovementOwners,
        max_visible: usize,
    ) -> Vec<(u64, ServerPresentationMessage)> {
        for damage in &frame.damage {
            let Some(client_id) = damage.source.and_then(|source| owners.owner_of(source)) else {
                continue;
            };
            if !replicable.contains(damage.entity) {
                continue;
            }
            // A just-registered dynamic enemy may not have reached the later
            // snapshot-production pass yet. Stamp it here so its first earned
            // overlay fact still carries the same stable NetworkId that pass
            // will serialize this frame.
            let network_id = allocator.stamp(damage.entity);
            self.refresh_remote_overlay(client_id, network_id, damage.entity, max_visible);
        }

        let mut messages = Vec::new();
        for fact in &frame.facts {
            let tuple = OverlayFactTuple::from(*fact);
            let mut recipients: Vec<_> = self
                .live_overlays
                .iter()
                .filter_map(|(key, live)| (live.entity == fact.entity).then_some(*key))
                .collect();
            recipients.sort_unstable_by_key(|(client_id, network_id)| (*client_id, network_id.0));
            for key @ (client_id, network_id) in recipients {
                if self.last_sent.get(&key) == Some(&tuple) {
                    continue;
                }
                self.last_sent.insert(key, tuple);
                messages.push((client_id, overlay_fact_message(network_id, tuple)));
            }
            if !tuple.alive {
                self.remove_entity(fact.entity);
            }
        }
        messages
    }

    fn refresh_remote_overlay(
        &mut self,
        client_id: u64,
        network_id: NetworkId,
        entity: EntityId,
        max_visible: usize,
    ) {
        if max_visible == 0 {
            return;
        }
        let key = (client_id, network_id);
        if let Some(live) = self.live_overlays.get_mut(&key) {
            live.entity = entity;
            live.last_damaged_time_seconds = self.elapsed_seconds;
            return;
        }

        let recipient_count = self
            .live_overlays
            .keys()
            .filter(|(existing_client, _)| *existing_client == client_id)
            .count();
        if recipient_count >= max_visible {
            let victim = self
                .live_overlays
                .iter()
                .filter(|((existing_client, _), _)| *existing_client == client_id)
                .min_by(|((_, left_id), left), ((_, right_id), right)| {
                    left.last_damaged_time_seconds
                        .total_cmp(&right.last_damaged_time_seconds)
                        .then_with(|| left_id.0.cmp(&right_id.0))
                })
                .map(|(key, live)| (*key, *live));
            let Some((victim_key, victim)) = victim else {
                return;
            };
            let incoming_rank = (self.elapsed_seconds, network_id.0);
            let victim_rank = (victim.last_damaged_time_seconds, victim_key.1.0);
            if incoming_rank <= victim_rank {
                return;
            }
            self.live_overlays.remove(&victim_key);
            self.last_sent.remove(&victim_key);
        }

        self.live_overlays.insert(
            key,
            HostLiveOverlay {
                entity,
                last_damaged_time_seconds: self.elapsed_seconds,
            },
        );
    }

    fn remove_entity(&mut self, entity: EntityId) {
        self.live_overlays.retain(|_, live| live.entity != entity);
        self.prune_last_sent();
    }

    fn prune_last_sent(&mut self) {
        let live_overlays = &self.live_overlays;
        self.last_sent
            .retain(|key, _| live_overlays.contains_key(key));
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.last_sent.len()
    }

    #[cfg(test)]
    fn live_ids_for_client(&self, client_id: u64) -> Vec<NetworkId> {
        let mut ids: Vec<_> = self
            .live_overlays
            .keys()
            .filter_map(|(owner, network_id)| (*owner == client_id).then_some(*network_id))
            .collect();
        ids.sort_by_key(|id| id.0);
        ids
    }
}

fn overlay_fact_message(enemy_id: NetworkId, fact: OverlayFactTuple) -> ServerPresentationMessage {
    ServerPresentationMessage {
        payload: ServerPresentationPayload::OverlayFact {
            enemy_id,
            health_fraction: fact.health_fraction,
            shield_fraction: fact.shield_fraction,
            has_shield: fact.has_shield,
            alive: fact.alive,
        },
    }
}

/// Send the frame's changed host facts to the exact remote clients that
/// damaged the still-tracked enemy. Presentation delivery is intentionally
/// fire-and-forget: a failed send is a dropped cosmetic, never retained work.
pub(crate) fn send_host_overlay_facts(
    tracker: &mut HostOverlayFactTracker,
    server: &mut NetServer,
    allocator: &mut NetworkIdAllocator,
    replicable: &ReplicableSet,
    owners: &MovementOwners,
    frame: &DamagedEnemyOverlayFrame,
    max_visible: usize,
) {
    for (client_id, message) in
        tracker.collect_changed(frame, allocator, replicable, owners, max_visible)
    {
        let _ = server.send_presentation(client_id, message);
    }
}

/// Drain the host's presentation intake once per frame and route each transient
/// to exactly one screen. A remote pawn owner receives one unreliable packet;
/// host-owned, absent, and non-pawn presenters remain host-local.
pub(crate) fn route_host_presentation_spawns(
    registry: &mut EntityRegistry,
    server: &mut NetServer,
    owners: &MovementOwners,
) {
    registry.retain_presentation_spawns(|spawn| {
        match presentation_recipient(spawn, owners) {
            Some(client_id) => {
                // A failed addressed send is intentionally a dropped cosmetic.
                let _ = server.send_presentation(client_id, presentation_message_from_spawn(spawn));
                false
            }
            None => true,
        }
    });
}

/// Convert received passive presentation events into client-local state. Spawn
/// messages enter registry intake. Overlay facts update the keyed pool from
/// host-authored values or wait briefly for an outrun entity baseline.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ingest_client_presentation_messages(
    registry: &mut EntityRegistry,
    messages: Vec<ServerPresentationMessage>,
    descriptors: &[EntityTypeDescriptor],
    templates: &HashMap<String, PresentationTemplate>,
    overlay_state: &mut ClientOverlayFactState,
    replication: &ClientReplication,
    pool: &mut PresentationPool,
    overlay_config: Option<&ClientOverlayConfig>,
    hit_zones: &HitZoneStore,
    agent_params: Option<NavAgentParams>,
    anim_time: f64,
    frame_dt_seconds: f32,
) {
    overlay_state.advance_terminal_ttl(frame_dt_seconds);
    overlay_state.discard_expired_live_overlays(pool);
    for message in messages {
        match message.payload {
            ServerPresentationPayload::Spawn {
                template_id,
                anchor,
                value,
                facts,
            } => {
                let Some(template) = templates.get(&template_id) else {
                    continue;
                };

                let mut facts = facts
                    .into_iter()
                    .map(|(name, fact)| (name, presentation_fact_from_wire(fact)))
                    .collect::<BTreeMap<_, _>>();
                // `value` is the conventional impact fact. Preserve an explicitly
                // stamped same-named fact if a future producer supplies one.
                facts
                    .entry("value".to_string())
                    .or_insert(PresentationFact::Number(value));

                registry.push_presentation_spawn(PresentationSpawn {
                    world_anchor: Vec3::new(anchor[0], anchor[1], anchor[2]),
                    template: PresentationTemplateHandle::from(template.id.clone()),
                    facts,
                    presenter: None,
                    lifetime_seconds: template.lifetime_ms as f32 / 1_000.0,
                    motion: PresentationMotion {
                        rise_pixels: template.motion.rise,
                        easing: template.motion.easing,
                    },
                    fade: PresentationFade {
                        duration_seconds: template
                            .lifetime_ms
                            .saturating_sub(template.fade.start_ms)
                            as f32
                            / 1_000.0,
                    },
                    scatter_radius: template.spawn_scatter.radius,
                });
            }
            ServerPresentationPayload::OverlayFact {
                enemy_id,
                health_fraction,
                shield_fraction,
                has_shield,
                alive,
            } => {
                let fact = ClientOverlayFact::new(
                    enemy_id,
                    health_fraction,
                    shield_fraction,
                    has_shield,
                    alive,
                );
                ingest_or_queue_client_overlay_fact(
                    registry,
                    descriptors,
                    overlay_state,
                    replication,
                    pool,
                    overlay_config,
                    hit_zones,
                    agent_params,
                    anim_time,
                    fact,
                );
            }
        }
    }
    retry_pending_client_overlay_facts(
        registry,
        descriptors,
        overlay_state,
        replication,
        pool,
        overlay_config,
        hit_zones,
        agent_params,
        anim_time,
    );
    overlay_state.discard_expired_live_overlays(pool);
}

#[allow(clippy::too_many_arguments)]
fn ingest_or_queue_client_overlay_fact(
    registry: &EntityRegistry,
    descriptors: &[EntityTypeDescriptor],
    state: &mut ClientOverlayFactState,
    replication: &ClientReplication,
    pool: &mut PresentationPool,
    config: Option<&ClientOverlayConfig>,
    hit_zones: &HitZoneStore,
    agent_params: Option<NavAgentParams>,
    anim_time: f64,
    fact: ClientOverlayFact,
) {
    if !fact.alive {
        ingest_client_overlay_fact(state, pool, fact, None, None, config);
        return;
    }
    if state.terminal_ids.contains_key(&fact.enemy_id) {
        return;
    }
    let Some(config) = config else {
        return;
    };
    let entity = replication.entity_for_network_id(fact.enemy_id);
    let anchor = entity.and_then(|entity| {
        client_overlay_anchor(
            registry,
            hit_zones,
            entity,
            &config.world_anchor,
            client_overlay_hitbox(replication, descriptors, fact.enemy_id, agent_params),
            anim_time,
        )
    });
    if entity.is_none() || anchor.is_none() {
        state.queue_pending_live(fact, config.max_visible);
        return;
    }
    state.pending_live_facts.remove(&fact.enemy_id);
    ingest_client_overlay_fact(state, pool, fact, entity, anchor, Some(config));
}

#[allow(clippy::too_many_arguments)]
fn retry_pending_client_overlay_facts(
    registry: &EntityRegistry,
    descriptors: &[EntityTypeDescriptor],
    state: &mut ClientOverlayFactState,
    replication: &ClientReplication,
    pool: &mut PresentationPool,
    config: Option<&ClientOverlayConfig>,
    hit_zones: &HitZoneStore,
    agent_params: Option<NavAgentParams>,
    anim_time: f64,
) {
    let Some(config) = config else {
        state.pending_live_facts.clear();
        return;
    };
    if state.pending_live_facts.is_empty() {
        return;
    }
    let mut pending: Vec<_> = state.pending_live_facts.values().copied().collect();
    pending.sort_by(|left, right| {
        left.queued_at
            .total_cmp(&right.queued_at)
            .then_with(|| left.fact.enemy_id.0.cmp(&right.fact.enemy_id.0))
    });
    for pending in pending {
        let fact = pending.fact;
        if state.terminal_ids.contains_key(&fact.enemy_id) {
            state.pending_live_facts.remove(&fact.enemy_id);
            continue;
        }
        let Some(entity) = replication.entity_for_network_id(fact.enemy_id) else {
            continue;
        };
        let Some(anchor) = client_overlay_anchor(
            registry,
            hit_zones,
            entity,
            &config.world_anchor,
            client_overlay_hitbox(replication, descriptors, fact.enemy_id, agent_params),
            anim_time,
        ) else {
            continue;
        };
        state.pending_live_facts.remove(&fact.enemy_id);
        ingest_client_overlay_fact(state, pool, fact, Some(entity), Some(anchor), Some(config));
    }
}

/// Apply one decoded overlay fact. This is intentionally a plain state seam:
/// callers provide the replication-map result and visual anchor, while facts
/// only ever come from the host-pushed payload.
pub(crate) fn ingest_client_overlay_fact(
    state: &mut ClientOverlayFactState,
    pool: &mut PresentationPool,
    fact: ClientOverlayFact,
    entity: Option<EntityId>,
    anchor: Option<Vec3>,
    config: Option<&ClientOverlayConfig>,
) {
    if !fact.alive {
        state.pending_live_facts.remove(&fact.enemy_id);
        state
            .terminal_ids
            .insert(fact.enemy_id, state.elapsed_seconds);
        if let Some(live) = state.live_overlays.remove(&fact.enemy_id) {
            pool.evict_overlay(live.entity);
        }
        return;
    }

    if state.terminal_ids.contains_key(&fact.enemy_id) {
        return;
    }

    let (Some(entity), Some(anchor), Some(config)) = (entity, anchor, config) else {
        return;
    };

    let previous = state.live_overlays.get(&fact.enemy_id).copied();
    if let Some(previous) = previous
        && previous.entity != entity
        && !pool.rekey_overlay(previous.entity, entity)
    {
        pool.evict_overlay(previous.entity);
    }

    // Host facts also move for recharge and regeneration. Only a newly-earned
    // overlay or a decreasing combat fraction represents a hit that should
    // reset the last-damage linger clock.
    let damage_refresh = previous.is_none_or(|previous| {
        fact.health_fraction < previous.fact.health_fraction
            || fact.shield_fraction < previous.fact.shield_fraction
    });
    if damage_refresh || !pool.has_overlay(entity) {
        pool.refresh_overlay(
            entity,
            config.template.clone(),
            config.linger_seconds,
            config.max_visible,
            u64::from(fact.enemy_id.0),
        );
    }
    if !pool.has_overlay(entity) {
        return;
    }
    pool.stamp_damaged_enemy_overlay(
        entity,
        fact.health_fraction,
        fact.shield_fraction,
        fact.has_shield,
        anchor,
        config.hide_at_full && fact.health_fraction == 1.0,
    );
    state
        .live_overlays
        .insert(fact.enemy_id, ClientLiveOverlay { entity, fact });
}

/// Re-anchor live client overlays from the final interpolated remote pose. This
/// never changes facts or the last-hit linger clock.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_client_overlay_anchors(
    registry: &EntityRegistry,
    descriptors: &[EntityTypeDescriptor],
    state: &mut ClientOverlayFactState,
    replication: &ClientReplication,
    pool: &mut PresentationPool,
    config: Option<&ClientOverlayConfig>,
    hit_zones: &HitZoneStore,
    agent_params: Option<NavAgentParams>,
    anim_time: f64,
) {
    state.discard_expired_live_overlays(pool);
    let Some(config) = config else {
        return;
    };
    for (&network_id, live) in &mut state.live_overlays {
        let Some(entity) = replication.entity_for_network_id(network_id) else {
            pool.update_overlay_anchor(live.entity, None, true);
            continue;
        };
        if entity != live.entity {
            if !pool.rekey_overlay(live.entity, entity) {
                continue;
            }
            live.entity = entity;
        }
        let anchor = client_overlay_anchor(
            registry,
            hit_zones,
            entity,
            &config.world_anchor,
            client_overlay_hitbox(replication, descriptors, network_id, agent_params),
            anim_time,
        );
        pool.update_overlay_anchor(
            entity,
            anchor,
            config.hide_at_full && live.fact.health_fraction == 1.0,
        );
    }
}

/// Resolve the visual anchor for an already-replicated entity. Connected
/// clients carry no `HealthComponent`, so the local descriptor supplies the
/// same authored hitbox fallback without synthesizing combat state.
fn client_overlay_anchor(
    registry: &EntityRegistry,
    hit_zones: &HitZoneStore,
    entity: EntityId,
    anchor: &postretro_scripting_core::data_descriptors::PresentationWorldAnchor,
    hitbox: Option<ClientOverlayHitbox>,
    anim_time: f64,
) -> Option<Vec3> {
    let offset = Vec3::Y * anchor.offset_y;
    if let Some(socket) = hit_zones.posed_socket_world(registry, entity, &anchor.socket, anim_time)
    {
        return Some(socket + offset);
    }

    let transform = registry
        .get_component::<postretro_entities::Transform>(entity)
        .ok()?;
    if !transform.position.is_finite() {
        return None;
    }
    if let Some(hitbox) = hitbox {
        if hitbox.half_extents.is_finite() && hitbox.offset.is_finite() {
            let top = transform.position + hitbox.offset + Vec3::Y * hitbox.half_extents.y + offset;
            if top.is_finite() {
                return Some(top);
            }
        }
    }
    let mesh = registry
        .get_component::<postretro_entities::components::mesh::MeshComponent>(entity)
        .ok()?;
    let bound = hit_zones.get_by_name(&mesh.model)?.derived_bound?;
    let model_to_world = model_matrix(transform, mesh.origin_offset)?;
    let local_top = Vec3::new(
        (bound.min.x + bound.max.x) * 0.5,
        bound.max.y,
        (bound.min.z + bound.max.z) * 0.5,
    );
    let top = model_to_world.transform_point3(local_top) + offset;
    top.is_finite().then_some(top)
}

fn client_overlay_hitbox(
    replication: &ClientReplication,
    descriptors: &[EntityTypeDescriptor],
    network_id: NetworkId,
    agent_params: Option<NavAgentParams>,
) -> Option<ClientOverlayHitbox> {
    let entity_class = replication.remote_entity_class(network_id)?;
    let descriptor = descriptors
        .iter()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some(entity_class))?;
    let hitbox: HitboxDescriptor = descriptor.health.as_ref()?.hitbox?;
    Some(ClientOverlayHitbox {
        half_extents: Vec3::from_array(hitbox.half_extents),
        offset: Vec3::from_array(hitbox.offset.unwrap_or([0.0; 3]))
            - ai_capsule_center_from_feet_offset(descriptor, agent_params),
    })
}

fn presentation_recipient(spawn: &PresentationSpawn, owners: &MovementOwners) -> Option<u64> {
    spawn
        .presenter
        .map(|presenter| EntityId::from_raw(presenter.0))
        .and_then(|source| owners.owner_of(source))
}

fn presentation_message_from_spawn(spawn: &PresentationSpawn) -> ServerPresentationMessage {
    ServerPresentationMessage {
        payload: ServerPresentationPayload::Spawn {
            template_id: spawn.template.0.clone(),
            anchor: spawn.world_anchor.to_array(),
            value: match spawn.facts.get("value") {
                Some(PresentationFact::Number(value)) => *value,
                _ => 0.0,
            },
            facts: spawn
                .facts
                .iter()
                .map(|(name, fact)| (name.clone(), presentation_fact_to_wire(fact)))
                .collect(),
        },
    }
}

fn presentation_fact_to_wire(fact: &PresentationFact) -> WirePresentationFact {
    match fact {
        PresentationFact::Number(value) => WirePresentationFact::Number(*value),
        PresentationFact::Text(value) => WirePresentationFact::Text(value.clone()),
        PresentationFact::Bool(value) => WirePresentationFact::Bool(*value),
    }
}

fn presentation_fact_from_wire(fact: WirePresentationFact) -> PresentationFact {
    match fact {
        WirePresentationFact::Number(value) => PresentationFact::Number(value),
        WirePresentationFact::Text(value) => PresentationFact::Text(value),
        WirePresentationFact::Bool(value) => PresentationFact::Bool(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::{PresentationPresenter, PresentationTemplateHandle, Transform};
    use postretro_foundation::PresentationEasing;
    use postretro_net::wire::{ComponentPayload, EntityRecord, SnapshotMessage, WireTransform};
    use postretro_scripting_core::data_descriptors::{
        HealthDescriptor, PresentationTemplateFade, PresentationTemplateMotion,
        PresentationTemplateSpawnScatter, PresentationWorldAnchor,
    };

    const FLOAT_EPSILON: f32 = 1.0e-5;

    fn spawn(presenter: Option<EntityId>) -> PresentationSpawn {
        PresentationSpawn {
            world_anchor: Vec3::new(1.0, 2.0, 3.0),
            template: PresentationTemplateHandle::from("damage-number"),
            facts: BTreeMap::from([
                ("value".to_string(), PresentationFact::Number(40.0)),
                ("critical".to_string(), PresentationFact::Bool(true)),
                (
                    "label".to_string(),
                    PresentationFact::Text("critical hit".to_string()),
                ),
            ]),
            presenter: presenter.map(|id| PresentationPresenter(id.to_raw())),
            lifetime_seconds: 0.9,
            motion: PresentationMotion {
                rise_pixels: 12.0,
                easing: PresentationEasing::EaseOut,
            },
            fade: PresentationFade {
                duration_seconds: 0.4,
            },
            scatter_radius: 0.15,
        }
    }

    fn template() -> PresentationTemplate {
        PresentationTemplate {
            id: "damage-number".to_string(),
            root: postretro_scripting_core::ui::descriptor::Widget::Spacer(
                postretro_scripting_core::ui::descriptor::SpacerWidget {
                    flex_grow: 0.0,
                    id: None,
                    visible_when: None,
                    role: None,
                },
            ),
            world_anchor: None,
            lifetime_ms: 900,
            motion: PresentationTemplateMotion {
                rise: 12.0,
                easing: PresentationEasing::EaseOut,
            },
            fade: PresentationTemplateFade { start_ms: 500 },
            spawn_scatter: PresentationTemplateSpawnScatter { radius: 0.15 },
        }
    }

    fn overlay_config() -> ClientOverlayConfig {
        ClientOverlayConfig {
            template: PresentationTemplateHandle::from("enemy-status"),
            world_anchor: PresentationWorldAnchor {
                socket: "head".to_string(),
                offset_y: 0.25,
            },
            max_visible: 2,
            linger_seconds: 2.5,
            hide_at_full: false,
        }
    }

    fn descriptor_with_hitbox(class: &str) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(class.to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: Some(HealthDescriptor {
                max: 100.0,
                hitbox: Some(HitboxDescriptor {
                    half_extents: [0.5, 1.5, 0.5],
                    offset: Some([0.0, 0.25, 0.0]),
                }),
                zone_multipliers: HashMap::new(),
            }),
            behavior: None,
        }
    }

    fn remote_baseline(network_id: u32, entity_class: &str, position: Vec3) -> SnapshotMessage {
        SnapshotMessage {
            sequence: 0,
            server_tick: 1,
            records: vec![EntityRecord::FullBaseline {
                network_id,
                baseline_id: 1,
                last_processed_client_tick: None,
                local_player: false,
                entity_class: Some(entity_class.to_string()),
                active_weapon_archetype: None,
                components: vec![ComponentPayload::Transform(WireTransform {
                    position: position.to_array(),
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0; 3],
                })],
            }],
            state_schema_fingerprint: [0; 32],
            state_records: Vec::new(),
        }
    }

    fn overlay_fact(enemy_id: u32, health_fraction: f32, alive: bool) -> ClientOverlayFact {
        ClientOverlayFact {
            enemy_id: NetworkId(enemy_id),
            health_fraction,
            shield_fraction: 0.4,
            has_shield: true,
            alive,
        }
    }

    fn overlay_health_fraction(pool: &PresentationPool, entity: EntityId) -> Option<f32> {
        match pool.overlay_facts(entity)?.get("healthFraction") {
            Some(PresentationFact::Number(value)) => Some(*value),
            _ => None,
        }
    }

    #[test]
    fn presentation_recipient_addresses_only_the_owning_remote_pawn() {
        let remote = EntityId::from_raw(7);
        let host_pawn = EntityId::from_raw(8);
        let mut owners = MovementOwners::new();
        owners.set(remote, 41);

        assert_eq!(
            presentation_recipient(&spawn(Some(remote)), &owners),
            Some(41)
        );
        assert_eq!(
            presentation_recipient(&spawn(Some(host_pawn)), &owners),
            None
        );
        assert_eq!(presentation_recipient(&spawn(None), &owners), None);
    }

    #[test]
    fn spawn_message_preserves_template_anchor_value_and_all_facts() {
        let original = spawn(Some(EntityId::from_raw(7)));
        let message = presentation_message_from_spawn(&original);
        let ServerPresentationPayload::Spawn {
            template_id,
            anchor,
            value,
            facts,
        } = message.payload
        else {
            panic!("spawn conversion must produce a Spawn payload");
        };

        assert_eq!(template_id, original.template.0);
        assert_eq!(anchor, original.world_anchor.to_array());
        assert_eq!(value, 40.0);
        assert_eq!(
            facts,
            BTreeMap::from([
                ("value".to_string(), WirePresentationFact::Number(40.0)),
                ("critical".to_string(), WirePresentationFact::Bool(true)),
                (
                    "label".to_string(),
                    WirePresentationFact::Text("critical hit".to_string()),
                ),
            ])
        );
    }

    #[test]
    fn client_spawn_ingest_restores_local_pool_values_without_presenter_identity() {
        let mut registry = EntityRegistry::new();
        let mut overlay_state = ClientOverlayFactState::default();
        let replication = ClientReplication::new();
        let mut pool = PresentationPool::new(1);
        let hit_zones = HitZoneStore::new();
        let templates = HashMap::from([("damage-number".to_string(), template())]);
        ingest_client_presentation_messages(
            &mut registry,
            vec![presentation_message_from_spawn(&spawn(Some(
                EntityId::from_raw(7),
            )))],
            &[],
            &templates,
            &mut overlay_state,
            &replication,
            &mut pool,
            None,
            &hit_zones,
            None,
            0.0,
            0.0,
        );

        let spawned = registry.take_presentation_spawns();
        assert_eq!(spawned.len(), 1);
        let spawned = &spawned[0];
        assert_eq!(spawned.world_anchor, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(spawned.template.0, "damage-number");
        assert_eq!(
            spawned.facts.get("value"),
            Some(&PresentationFact::Number(40.0))
        );
        assert_eq!(
            spawned.facts.get("critical"),
            Some(&PresentationFact::Bool(true))
        );
        assert_eq!(
            spawned.facts.get("label"),
            Some(&PresentationFact::Text("critical hit".to_string()))
        );
        assert_eq!(spawned.presenter, None);
        assert_eq!(spawned.lifetime_seconds, 0.9);
        assert_eq!(spawned.motion.rise_pixels, 12.0);
        assert_eq!(spawned.fade.duration_seconds, 0.4);
        assert_eq!(spawned.scatter_radius, 0.15);
    }

    #[test]
    fn client_overlay_death_then_stale_alive_does_not_resurrect() {
        let entity = EntityId::from_raw(17);
        let config = overlay_config();
        let mut state = ClientOverlayFactState::default();
        let mut pool = PresentationPool::new(1);

        ingest_client_overlay_fact(
            &mut state,
            &mut pool,
            overlay_fact(4, 0.2, true),
            Some(entity),
            Some(Vec3::new(1.0, 2.0, 3.0)),
            Some(&config),
        );
        ingest_client_overlay_fact(
            &mut state,
            &mut pool,
            overlay_fact(4, 0.0, false),
            None,
            None,
            Some(&config),
        );
        ingest_client_overlay_fact(
            &mut state,
            &mut pool,
            overlay_fact(4, 0.8, true),
            Some(entity),
            Some(Vec3::new(1.0, 2.0, 3.0)),
            Some(&config),
        );

        assert!(pool.overlay_facts(entity).is_none());
        assert_eq!(state.terminal_len(), 1);
    }

    #[test]
    fn client_overlay_death_for_never_seen_enemy_creates_no_bar() {
        let entity = EntityId::from_raw(18);
        let config = overlay_config();
        let mut state = ClientOverlayFactState::default();
        let mut pool = PresentationPool::new(1);

        ingest_client_overlay_fact(
            &mut state,
            &mut pool,
            overlay_fact(5, 0.0, false),
            Some(entity),
            Some(Vec3::ZERO),
            Some(&config),
        );

        assert!(pool.overlay_facts(entity).is_none());
        assert_eq!(state.terminal_len(), 1);
    }

    #[test]
    fn client_overlay_live_reorder_keeps_last_ingested_fact() {
        let entity = EntityId::from_raw(19);
        let config = overlay_config();
        let mut state = ClientOverlayFactState::default();
        let mut pool = PresentationPool::new(1);

        ingest_client_overlay_fact(
            &mut state,
            &mut pool,
            overlay_fact(6, 0.2, true),
            Some(entity),
            Some(Vec3::ZERO),
            Some(&config),
        );
        ingest_client_overlay_fact(
            &mut state,
            &mut pool,
            overlay_fact(6, 0.5, true),
            Some(entity),
            Some(Vec3::ZERO),
            Some(&config),
        );

        let health = overlay_health_fraction(&pool, entity).expect("last fact stamps a bar");
        assert!((health - 0.5).abs() < FLOAT_EPSILON);
    }

    #[test]
    fn client_overlay_terminal_ids_expire_after_reorder_horizon() {
        let config = overlay_config();
        let mut state = ClientOverlayFactState::default();
        let mut pool = PresentationPool::new(1);

        ingest_client_overlay_fact(
            &mut state,
            &mut pool,
            overlay_fact(7, 0.0, false),
            None,
            None,
            Some(&config),
        );
        state.advance_terminal_ttl_for_test(0.49);
        assert_eq!(state.terminal_len(), 1);
        state.advance_terminal_ttl_for_test(0.02);

        assert_eq!(state.terminal_len(), 0);
    }

    // Regression: a lifecycle reset retained terminal and pre-baseline facts,
    // allowing the old stream to affect a replacement level/template.
    #[test]
    fn client_overlay_clear_retires_terminal_live_and_pending_facts() {
        let entity = EntityId::from_raw(20);
        let config = overlay_config();
        let mut state = ClientOverlayFactState::default();
        let mut pool = PresentationPool::new(1);
        ingest_client_overlay_fact(
            &mut state,
            &mut pool,
            overlay_fact(8, 0.5, true),
            Some(entity),
            Some(Vec3::ZERO),
            Some(&config),
        );
        ingest_client_overlay_fact(
            &mut state,
            &mut pool,
            overlay_fact(9, 0.0, false),
            None,
            None,
            Some(&config),
        );
        state.queue_pending_live(overlay_fact(10, 0.75, true), config.max_visible);

        state.clear();

        assert!(state.live_overlays.is_empty());
        assert_eq!(state.terminal_len(), 0);
        assert_eq!(state.pending_len(), 0);
        assert_eq!(state.elapsed_seconds, 0.0);
    }

    #[test]
    fn client_overlay_fact_ingest_uses_pushed_values_without_health_registry() {
        let entity = EntityId::from_raw(20);
        let config = overlay_config();
        let mut state = ClientOverlayFactState::default();
        let mut pool = PresentationPool::new(1);

        ingest_client_overlay_fact(
            &mut state,
            &mut pool,
            ClientOverlayFact {
                enemy_id: NetworkId(8),
                health_fraction: 0.35,
                shield_fraction: 0.6,
                has_shield: true,
                alive: true,
            },
            Some(entity),
            Some(Vec3::ZERO),
            Some(&config),
        );

        let facts = pool
            .overlay_facts(entity)
            .expect("fact created the keyed overlay");
        let health = match facts.get("healthFraction") {
            Some(PresentationFact::Number(value)) => *value,
            _ => panic!("host-pushed health fraction was not retained"),
        };
        let shield = match facts.get("shieldFraction") {
            Some(PresentationFact::Number(value)) => *value,
            _ => panic!("host-pushed shield fraction was not retained"),
        };
        assert!((health - 0.35).abs() < FLOAT_EPSILON);
        assert!((shield - 0.6).abs() < FLOAT_EPSILON);
        assert_eq!(facts.get("hasShield"), Some(&PresentationFact::Bool(true)));
    }

    #[test]
    fn client_overlay_fact_waits_for_baseline_then_tracks_interpolated_hitbox_anchor() {
        let descriptors = vec![descriptor_with_hitbox("remote_enemy")];
        let config = overlay_config();
        let mut registry = EntityRegistry::new();
        let mut replication = ClientReplication::new();
        let mut state = ClientOverlayFactState::default();
        let mut pool = PresentationPool::new(2);
        let hit_zones = HitZoneStore::new();
        let fact = overlay_fact(21, 0.65, true);

        ingest_client_presentation_messages(
            &mut registry,
            vec![ServerPresentationMessage {
                payload: ServerPresentationPayload::OverlayFact {
                    enemy_id: fact.enemy_id,
                    health_fraction: fact.health_fraction,
                    shield_fraction: fact.shield_fraction,
                    has_shield: fact.has_shield,
                    alive: fact.alive,
                },
            }],
            &descriptors,
            &HashMap::new(),
            &mut state,
            &replication,
            &mut pool,
            Some(&config),
            &hit_zones,
            None,
            0.0,
            0.0,
        );
        assert_eq!(state.pending_len(), 1);

        let outcome = replication.apply_snapshot(
            &mut registry,
            &remote_baseline(21, "descriptor:remote_enemy", Vec3::new(1.0, 2.0, 3.0)),
        );
        let remote = outcome
            .remote_entities
            .first()
            .expect("class-bearing baseline surfaces remote materialization");
        replication.cache_remote_entity_class(remote.network_id, &remote.entity_class);
        let entity = remote.entity_id;

        ingest_client_presentation_messages(
            &mut registry,
            Vec::new(),
            &descriptors,
            &HashMap::new(),
            &mut state,
            &replication,
            &mut pool,
            Some(&config),
            &hit_zones,
            None,
            0.0,
            0.0,
        );
        assert_eq!(state.pending_len(), 0);
        assert_eq!(overlay_health_fraction(&pool, entity), Some(0.65));
        assert_eq!(pool.overlay_anchor(entity), Some(Vec3::new(1.0, 4.0, 3.0)));

        registry
            .set_component(
                entity,
                Transform {
                    position: Vec3::new(4.0, 5.0, 6.0),
                    ..Transform::default()
                },
            )
            .expect("mapped remote transform remains live");
        update_client_overlay_anchors(
            &registry,
            &descriptors,
            &mut state,
            &replication,
            &mut pool,
            Some(&config),
            &hit_zones,
            None,
            0.0,
        );
        assert_eq!(pool.overlay_anchor(entity), Some(Vec3::new(4.0, 7.0, 6.0)));
    }

    #[test]
    fn client_overlay_pending_facts_are_capacity_and_ttl_bounded() {
        let mut config = overlay_config();
        config.max_visible = 2;
        let registry = EntityRegistry::new();
        let descriptors = Vec::new();
        let replication = ClientReplication::new();
        let hit_zones = HitZoneStore::new();
        let mut state = ClientOverlayFactState::default();
        let mut pool = PresentationPool::new(2);

        for enemy_id in 1..=3 {
            ingest_or_queue_client_overlay_fact(
                &registry,
                &descriptors,
                &mut state,
                &replication,
                &mut pool,
                Some(&config),
                &hit_zones,
                None,
                0.0,
                overlay_fact(enemy_id, 0.75, true),
            );
        }
        assert_eq!(state.pending_len(), 2);
        let mut pending_ids: Vec<_> = state.pending_live_facts.keys().map(|id| id.0).collect();
        pending_ids.sort_unstable();
        assert_eq!(pending_ids, [2, 3]);
        state.advance_terminal_ttl_for_test(0.51);
        assert_eq!(state.pending_len(), 0);
    }

    // Regression: equal-time facts created FIFO by packet arrival, so two
    // clients receiving the same datagrams in different orders kept different
    // capped target sets.
    #[test]
    fn client_overlay_equal_time_eviction_is_independent_of_fact_arrival() {
        let config = overlay_config();
        let entities = [
            EntityId::from_raw(31),
            EntityId::from_raw(32),
            EntityId::from_raw(33),
        ];
        let mut forward_state = ClientOverlayFactState::default();
        let mut reverse_state = ClientOverlayFactState::default();
        let mut forward_pool = PresentationPool::new(0);
        let mut reverse_pool = PresentationPool::new(0);

        for index in [0, 1, 2] {
            ingest_client_overlay_fact(
                &mut forward_state,
                &mut forward_pool,
                overlay_fact((index + 1) as u32, 0.75, true),
                Some(entities[index]),
                Some(Vec3::ZERO),
                Some(&config),
            );
        }
        for index in [2, 1, 0] {
            ingest_client_overlay_fact(
                &mut reverse_state,
                &mut reverse_pool,
                overlay_fact((index + 1) as u32, 0.75, true),
                Some(entities[index]),
                Some(Vec3::ZERO),
                Some(&config),
            );
        }
        forward_state.discard_expired_live_overlays(&forward_pool);
        reverse_state.discard_expired_live_overlays(&reverse_pool);

        let retained = |state: &ClientOverlayFactState| {
            let mut ids: Vec<_> = state.live_overlays.keys().map(|id| id.0).collect();
            ids.sort_unstable();
            ids
        };
        assert_eq!(retained(&forward_state), [2, 3]);
        assert_eq!(retained(&reverse_state), [2, 3]);
    }

    #[test]
    fn client_overlay_recharge_updates_facts_without_extending_last_hit_linger() {
        let entity = EntityId::from_raw(22);
        let config = overlay_config();
        let mut state = ClientOverlayFactState::default();
        let mut pool = PresentationPool::new(1);
        let mut registry = EntityRegistry::new();

        ingest_client_overlay_fact(
            &mut state,
            &mut pool,
            ClientOverlayFact {
                enemy_id: NetworkId(9),
                health_fraction: 0.5,
                shield_fraction: 0.2,
                has_shield: true,
                alive: true,
            },
            Some(entity),
            Some(Vec3::ZERO),
            Some(&config),
        );
        pool.advance_and_collect_inputs(&mut registry, 2.0, glam::Mat4::IDENTITY, [100, 100]);

        ingest_client_overlay_fact(
            &mut state,
            &mut pool,
            ClientOverlayFact {
                enemy_id: NetworkId(9),
                health_fraction: 0.5,
                shield_fraction: 0.8,
                has_shield: true,
                alive: true,
            },
            Some(entity),
            Some(Vec3::ZERO),
            Some(&config),
        );
        let shield = pool
            .overlay_facts(entity)
            .and_then(|facts| facts.get("shieldFraction"));
        assert_eq!(shield, Some(&PresentationFact::Number(0.8)));

        pool.advance_and_collect_inputs(&mut registry, 0.6, glam::Mat4::IDENTITY, [100, 100]);
        assert!(!pool.has_overlay(entity));
    }

    #[test]
    fn client_overlay_decreasing_fraction_resets_last_hit_linger() {
        let entity = EntityId::from_raw(23);
        let config = overlay_config();
        let mut state = ClientOverlayFactState::default();
        let mut pool = PresentationPool::new(1);
        let mut registry = EntityRegistry::new();

        ingest_client_overlay_fact(
            &mut state,
            &mut pool,
            overlay_fact(10, 0.8, true),
            Some(entity),
            Some(Vec3::ZERO),
            Some(&config),
        );
        pool.advance_and_collect_inputs(&mut registry, 2.0, glam::Mat4::IDENTITY, [100, 100]);
        ingest_client_overlay_fact(
            &mut state,
            &mut pool,
            overlay_fact(10, 0.6, true),
            Some(entity),
            Some(Vec3::ZERO),
            Some(&config),
        );
        pool.advance_and_collect_inputs(&mut registry, 0.6, glam::Mat4::IDENTITY, [100, 100]);

        assert!(pool.has_overlay(entity));
    }

    fn overlay_frame(
        damage: Vec<DamagedEnemyOverlayDamage>,
        fact: DamagedEnemyOverlayFact,
    ) -> DamagedEnemyOverlayFrame {
        DamagedEnemyOverlayFrame {
            damage,
            facts: vec![fact],
        }
    }

    fn live_overlay_fact(entity: EntityId, health_fraction: f32) -> DamagedEnemyOverlayFact {
        DamagedEnemyOverlayFact {
            entity,
            health_fraction,
            shield_fraction: 0.25,
            has_shield: true,
            alive: true,
        }
    }

    #[test]
    fn overlay_facts_reach_only_damaging_clients_and_only_on_change() {
        let enemy = EntityId::from_raw(10);
        let first_pawn = EntityId::from_raw(11);
        let second_pawn = EntityId::from_raw(12);
        let host_pawn = EntityId::from_raw(13);
        let mut allocator = NetworkIdAllocator::new();
        let enemy_id = allocator.stamp(enemy);
        let mut replicable = ReplicableSet::new();
        replicable.register(enemy);
        let mut owners = MovementOwners::new();
        owners.set(first_pawn, 41);
        owners.set(second_pawn, 73);
        let mut tracker = HostOverlayFactTracker::default();
        let fact = live_overlay_fact(enemy, 0.8);

        let first = tracker.collect_changed(
            &overlay_frame(
                vec![DamagedEnemyOverlayDamage {
                    entity: enemy,
                    source: Some(first_pawn),
                }],
                fact,
            ),
            &mut allocator,
            &replicable,
            &owners,
            8,
        );
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].0, 41);
        assert_eq!(
            &first[0].1.payload,
            &ServerPresentationPayload::OverlayFact {
                enemy_id,
                health_fraction: 0.8,
                shield_fraction: 0.25,
                has_shield: true,
                alive: true,
            }
        );

        // A host-owned hit is local-only. It must not cause an addressed
        // event, and an unchanged fact never re-sends to the first client.
        let unchanged = tracker.collect_changed(
            &overlay_frame(
                vec![DamagedEnemyOverlayDamage {
                    entity: enemy,
                    source: Some(host_pawn),
                }],
                fact,
            ),
            &mut allocator,
            &replicable,
            &owners,
            8,
        );
        assert!(unchanged.is_empty());

        // A second remote client earns its own initial fact; the first client
        // remains suppressed because its tuple did not change.
        let second = tracker.collect_changed(
            &overlay_frame(
                vec![DamagedEnemyOverlayDamage {
                    entity: enemy,
                    source: Some(second_pawn),
                }],
                fact,
            ),
            &mut allocator,
            &replicable,
            &owners,
            8,
        );
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].0, 73);

        // A terminal fact reaches each earned recipient once and immediately
        // prunes the host's suppression state.
        let dead = DamagedEnemyOverlayFact {
            alive: false,
            ..fact
        };
        let terminal = tracker.collect_changed(
            &overlay_frame(Vec::new(), dead),
            &mut allocator,
            &replicable,
            &owners,
            8,
        );
        assert_eq!(
            terminal
                .iter()
                .map(|(client_id, _)| *client_id)
                .collect::<Vec<_>>(),
            vec![41, 73]
        );
        assert!(terminal.iter().all(|(_, message)| matches!(
            &message.payload,
            ServerPresentationPayload::OverlayFact { alive: false, .. }
        )));
        assert_eq!(tracker.len(), 0);
    }

    #[test]
    fn overlay_fact_bookkeeping_prunes_when_tracking_expires() {
        let enemy = EntityId::from_raw(10);
        let pawn = EntityId::from_raw(11);
        let mut allocator = NetworkIdAllocator::new();
        allocator.stamp(enemy);
        let mut replicable = ReplicableSet::new();
        replicable.register(enemy);
        let mut owners = MovementOwners::new();
        owners.set(pawn, 41);
        let mut tracker = HostOverlayFactTracker::default();
        let fact = live_overlay_fact(enemy, 0.8);

        let _ = tracker.collect_changed(
            &overlay_frame(
                vec![DamagedEnemyOverlayDamage {
                    entity: enemy,
                    source: Some(pawn),
                }],
                fact,
            ),
            &mut allocator,
            &replicable,
            &owners,
            8,
        );
        assert_eq!(tracker.len(), 1);

        tracker.begin_frame(2.6, 2.5, &owners);
        assert_eq!(tracker.len(), 0);
    }

    // Regression: one global host overlay cap let a target earned by one
    // client evict and prune another client's otherwise-private stream.
    #[test]
    fn host_overlay_caps_are_independent_per_recipient() {
        let first = EntityId::from_raw(40);
        let second = EntityId::from_raw(41);
        let replacement = EntityId::from_raw(42);
        let first_pawn = EntityId::from_raw(50);
        let second_pawn = EntityId::from_raw(51);
        let mut allocator = NetworkIdAllocator::new();
        let first_id = allocator.stamp(first);
        let second_id = allocator.stamp(second);
        let replacement_id = allocator.stamp(replacement);
        let mut replicable = ReplicableSet::new();
        replicable.register(first);
        replicable.register(second);
        replicable.register(replacement);
        let mut owners = MovementOwners::new();
        owners.set(first_pawn, 41);
        owners.set(second_pawn, 73);
        let mut tracker = HostOverlayFactTracker::default();

        let first_frame = DamagedEnemyOverlayFrame {
            damage: vec![
                DamagedEnemyOverlayDamage {
                    entity: first,
                    source: Some(first_pawn),
                },
                DamagedEnemyOverlayDamage {
                    entity: second,
                    source: Some(second_pawn),
                },
            ],
            facts: vec![
                live_overlay_fact(first, 0.8),
                live_overlay_fact(second, 0.7),
            ],
        };
        let sent = tracker.collect_changed(&first_frame, &mut allocator, &replicable, &owners, 1);
        assert_eq!(sent.len(), 2);
        assert_eq!(tracker.live_ids_for_client(41), [first_id]);
        assert_eq!(tracker.live_ids_for_client(73), [second_id]);

        let replacement_frame = DamagedEnemyOverlayFrame {
            damage: vec![DamagedEnemyOverlayDamage {
                entity: replacement,
                source: Some(first_pawn),
            }],
            facts: vec![
                live_overlay_fact(first, 0.8),
                live_overlay_fact(second, 0.7),
                live_overlay_fact(replacement, 0.6),
            ],
        };
        let sent =
            tracker.collect_changed(&replacement_frame, &mut allocator, &replicable, &owners, 1);

        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, 41);
        assert_eq!(tracker.live_ids_for_client(41), [replacement_id]);
        assert_eq!(tracker.live_ids_for_client(73), [second_id]);
    }

    // Regression: level teardown preserved recipient and last-sent facts keyed
    // by entities from the retired map.
    #[test]
    fn host_overlay_clear_retires_recipients_and_last_sent_facts() {
        let enemy = EntityId::from_raw(10);
        let pawn = EntityId::from_raw(11);
        let mut allocator = NetworkIdAllocator::new();
        allocator.stamp(enemy);
        let mut replicable = ReplicableSet::new();
        replicable.register(enemy);
        let mut owners = MovementOwners::new();
        owners.set(pawn, 41);
        let mut tracker = HostOverlayFactTracker::default();
        let _ = tracker.collect_changed(
            &overlay_frame(
                vec![DamagedEnemyOverlayDamage {
                    entity: enemy,
                    source: Some(pawn),
                }],
                live_overlay_fact(enemy, 0.8),
            ),
            &mut allocator,
            &replicable,
            &owners,
            8,
        );

        tracker.clear();

        assert!(tracker.live_overlays.is_empty());
        assert_eq!(tracker.len(), 0);
    }
}
