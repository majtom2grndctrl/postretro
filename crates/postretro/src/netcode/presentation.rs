// Host-to-client routing and engine conversion for passive presentation events.
// See: context/lib/networking.md

use std::collections::{BTreeMap, HashMap};

use glam::Vec3;
use postretro_entities::{
    EntityId, EntityRegistry, PresentationFact, PresentationFade, PresentationMotion,
    PresentationSpawn, PresentationTemplateHandle,
};
use postretro_net::transport::NetServer;
use postretro_net::wire::{
    NetworkId, PresentationFact as WirePresentationFact, ServerPresentationMessage,
    ServerPresentationPayload,
};
use postretro_scripting_core::data_descriptors::PresentationTemplate;

use crate::impact_policy::ClientOverlayConfig;
use crate::presentation_pool::PresentationPool;
use crate::scripting_systems::hit_zones::{HitZoneStore, model_matrix};

use super::{ClientReplication, MovementOwners};

/// Retain terminal enemy ids past the presentation channel's expected reorder
/// horizon. The id allocator never recycles, so this must remain a bounded
/// short-lived guard rather than a session ledger.
const CLIENT_OVERLAY_TERMINAL_TTL_SECONDS: f64 = 0.5;

/// A decoded overlay fact separated from the wire envelope so the adverse-order
/// behavior is directly testable without transport or registry setup.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClientOverlayFact {
    enemy_id: NetworkId,
    health_fraction: f32,
    shield_fraction: f32,
    has_shield: bool,
    alive: bool,
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

/// Client-only ordering state for host-pushed enemy-overlay facts. A live
/// instance is keyed by stable `NetworkId`; the pool itself still uses the
/// current local `EntityId` for drawing and anchor storage.
#[derive(Debug, Default)]
pub(crate) struct ClientOverlayFactState {
    terminal_ids: HashMap<NetworkId, f64>,
    live_overlay_entities: HashMap<NetworkId, EntityId>,
    elapsed_seconds: f64,
}

impl ClientOverlayFactState {
    /// Advance the injected game-time clock and drop terminal guards that have
    /// outlived the unordered channel's reorder horizon.
    pub(crate) fn advance_terminal_ttl(&mut self, frame_dt_seconds: f32) {
        if frame_dt_seconds.is_finite() && frame_dt_seconds > 0.0 {
            self.elapsed_seconds += f64::from(frame_dt_seconds);
        }
        let now = self.elapsed_seconds;
        self.terminal_ids
            .retain(|_, marked_at| now - *marked_at < CLIENT_OVERLAY_TERMINAL_TTL_SECONDS);
    }

    fn discard_expired_live_overlays(&mut self, pool: &PresentationPool) {
        self.live_overlay_entities
            .retain(|_, entity| pool.has_overlay(*entity));
    }

    #[cfg(test)]
    pub(crate) fn terminal_len(&self) -> usize {
        self.terminal_ids.len()
    }

    #[cfg(test)]
    pub(crate) fn advance_terminal_ttl_for_test(&mut self, frame_dt_seconds: f32) {
        self.advance_terminal_ttl(frame_dt_seconds);
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
    let spawns = registry.take_presentation_spawns();
    for spawn in spawns {
        match presentation_recipient(&spawn, owners) {
            Some(client_id) => {
                // A failed addressed send is intentionally a dropped cosmetic.
                let _ =
                    server.send_presentation(client_id, presentation_message_from_spawn(&spawn));
            }
            None => registry.push_presentation_spawn(spawn),
        }
    }
}

/// Convert received passive presentation events into client-local state. Spawn
/// messages enter the registry intake; overlay facts directly refresh the
/// keyed pool from host-authored values.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ingest_client_presentation_messages(
    registry: &mut EntityRegistry,
    messages: Vec<ServerPresentationMessage>,
    templates: &[PresentationTemplate],
    overlay_state: &mut ClientOverlayFactState,
    replication: &ClientReplication,
    pool: &mut PresentationPool,
    overlay_config: Option<&ClientOverlayConfig>,
    hit_zones: &HitZoneStore,
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
                let Some(template) = templates.iter().find(|template| template.id == template_id)
                else {
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
                let entity = replication.entity_for_network_id(fact.enemy_id);
                let anchor = overlay_config.and_then(|config| {
                    entity.and_then(|entity| {
                        client_overlay_anchor(
                            registry,
                            hit_zones,
                            entity,
                            &config.world_anchor,
                            anim_time,
                        )
                    })
                });
                ingest_client_overlay_fact(
                    overlay_state,
                    pool,
                    fact,
                    entity,
                    anchor,
                    overlay_config,
                );
            }
        }
    }
    overlay_state.discard_expired_live_overlays(pool);
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
        state
            .terminal_ids
            .insert(fact.enemy_id, state.elapsed_seconds);
        if let Some(entity) = state.live_overlay_entities.remove(&fact.enemy_id) {
            pool.evict_overlay(entity);
        }
        return;
    }

    if state.terminal_ids.contains_key(&fact.enemy_id) {
        return;
    }

    let (Some(entity), Some(anchor), Some(config)) = (entity, anchor, config) else {
        return;
    };

    if let Some(previous_entity) = state.live_overlay_entities.insert(fact.enemy_id, entity)
        && previous_entity != entity
    {
        pool.evict_overlay(previous_entity);
    }

    pool.refresh_overlay(
        entity,
        config.template.clone(),
        config.linger_seconds,
        config.max_visible,
    );
    let mut facts = BTreeMap::new();
    facts.insert(
        "healthFraction".to_string(),
        PresentationFact::Number(fact.health_fraction),
    );
    facts.insert(
        "shieldFraction".to_string(),
        PresentationFact::Number(fact.shield_fraction),
    );
    facts.insert(
        "hasShield".to_string(),
        PresentationFact::Bool(fact.has_shield),
    );
    pool.stamp_overlay(
        entity,
        facts,
        anchor,
        config.hide_at_full && fact.health_fraction == 1.0,
    );
}

/// Resolve the visual anchor for an already-replicated entity. Unlike the host
/// path, this deliberately has no health-hitbox fallback: connected clients do
/// not materialize enemy health and must not synthesize overlay facts from it.
fn client_overlay_anchor(
    registry: &EntityRegistry,
    hit_zones: &HitZoneStore,
    entity: EntityId,
    anchor: &postretro_scripting_core::data_descriptors::PresentationWorldAnchor,
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
    use postretro_entities::{PresentationPresenter, PresentationTemplateHandle};
    use postretro_foundation::PresentationEasing;
    use postretro_scripting_core::data_descriptors::{
        PresentationTemplateFade, PresentationTemplateMotion, PresentationTemplateSpawnScatter,
        PresentationWorldAnchor,
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
        ingest_client_presentation_messages(
            &mut registry,
            vec![presentation_message_from_spawn(&spawn(Some(
                EntityId::from_raw(7),
            )))],
            &[template()],
            &mut overlay_state,
            &replication,
            &mut pool,
            None,
            &hit_zones,
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
}
