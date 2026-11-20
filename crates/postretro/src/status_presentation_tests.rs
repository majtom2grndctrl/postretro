// Status presentation follows presence on each machine: one emitter per
// afflicted entity (none on this machine's own pawn) at its body centre, one
// anchored tick sound one status tick after presence appears and every status
// tick after, both stopping when presence clears.
// See: context/lib/scripting.md §2 (Status declarations)

use std::sync::Arc;

use glam::Vec3;
use postretro_audio::{SoundAnchor, SoundRequest};
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::components::health::{HealthComponent, Hitbox};
use postretro_entities::components::hit_volume::HitVolumeComponent;
use postretro_entities::components::statuses::{StatusApplier, StatusPresenceComponent};
use postretro_entities::{ComponentKind, EntityId, EntityRegistry, Transform};
use postretro_foundation::defense::DefenseVocabulary;
use postretro_foundation::status::parse_statuses;
use postretro_sim::particle_pool::ParticlePool;

use super::StatusPresentation;
use crate::scripting_systems::emitter_bridge::EmitterBridge;
use crate::scripting_systems::particle_render::ParticleRenderCollector;
use crate::sprite_collection::derive_collection_id;

/// Frame length most tests advance by, in milliseconds.
const FRAME_MS: f32 = 100.0;

fn vocabulary() -> Arc<DefenseVocabulary> {
    Arc::new(DefenseVocabulary {
        statuses: parse_statuses(
            Some(&serde_json::json!([
                { "kind": "status", "name": "wet", "damage": 0, "tickMs": 500 },
                { "kind": "status", "name": "burning", "damage": 4, "tickMs": 500,
                  "presentation": {
                    "emitter": { "kind": "billboard_emitter", "rate": 24.0, "spread": 0.2,
                      "lifetime": 0.6, "velocity": [0.0, 1.5, 0.0], "buoyancy": 0.5, "drag": 0.5,
                      "size_over_lifetime": [1.0], "opacity_over_lifetime": [1.0, 0.0],
                      "color": [1.0, 1.0, 1.0], "sprite": "ember", "spin_rate": 0.0 },
                    "tickSound": "burn_tick" } },
                { "kind": "status", "name": "zapped", "damage": 0, "tickMs": 17,
                  "presentation": { "tickSound": "zap_tick" } },
            ])),
            |_| None,
        )
        .unwrap(),
        ..Default::default()
    })
}

fn health(hitbox: Option<Hitbox>) -> HealthComponent {
    HealthComponent {
        max: 100.0,
        current: 100.0,
        hitbox,
        death_handled: false,
        pending_kill_credit: None,
        zone_multipliers: Default::default(),
        contributor_ledger: Default::default(),
    }
}

fn spawn(registry: &mut EntityRegistry, x: f32) -> EntityId {
    let entity = registry.spawn(Transform {
        position: Vec3::new(x, 1.0, 0.0),
        ..Transform::default()
    });
    registry.set_component(entity, health(None)).unwrap();
    entity
}

/// `n` frames of `dt_ms` each, returning the sounds requested per frame.
fn frames_of(
    presentation: &mut StatusPresentation,
    registry: &EntityRegistry,
    local: Option<EntityId>,
    n: usize,
    dt_ms: f32,
) -> Vec<Vec<SoundRequest>> {
    (0..n)
        .map(|_| {
            let mut sounds = Vec::new();
            presentation.update(registry, local, dt_ms, &mut sounds);
            sounds
        })
        .collect()
}

/// `n` frames of [`FRAME_MS`].
fn frames(
    presentation: &mut StatusPresentation,
    registry: &EntityRegistry,
    local: Option<EntityId>,
    n: usize,
) -> Vec<Vec<SoundRequest>> {
    frames_of(presentation, registry, local, n, FRAME_MS)
}

/// The frames that requested any sound.
fn sounding(sounds: &[Vec<SoundRequest>]) -> Vec<usize> {
    sounds
        .iter()
        .enumerate()
        .filter(|(_, sounds)| !sounds.is_empty())
        .map(|(frame, _)| frame)
        .collect()
}

/// Apply `status` to each of `entities` on a host registry, by one source.
fn ignite(registry: &mut EntityRegistry, entities: &[EntityId], status: u16) {
    registry.set_status_applications_immediate(true);
    let source = registry.intern_status_source("weapon.incendiary");
    for &entity in entities {
        registry.apply_status(entity, status, StatusApplier::Source(source), source);
    }
}

fn anchored_to(sound: &SoundRequest) -> u64 {
    match &sound.anchor {
        Some(SoundAnchor::Entity { key, .. }) => *key,
        other => panic!("tick sounds anchor on the entity, got {other:?}"),
    }
}

// The host: an enemy and the host's own pawn both burn. The enemy gets an
// emitter, the own pawn none; each requests `burn_tick` one status tick after
// presence appears and every status tick after; clearing presence stops both,
// and no emitter component is ever added to the entity.
#[test]
fn status_presentation_follows_presence_on_the_host() {
    let mut registry = EntityRegistry::new();
    registry.set_defense_vocabulary(vocabulary());
    let enemy = spawn(&mut registry, 0.0);
    let own = spawn(&mut registry, 5.0);
    ignite(&mut registry, &[enemy, own], 1);
    let mut presentation = StatusPresentation::default();

    let sounds = frames(&mut presentation, &registry, Some(own), 11);
    // `tickMs` 500 is 30 fixed ticks of 16.667 ms, 500.01 ms: the first frame
    // at or past each is 600 ms, then 1100 ms.
    assert_eq!(sounding(&sounds), [5, 10]);
    let keys: Vec<u64> = sounds[5].iter().map(anchored_to).collect();
    assert_eq!(
        keys,
        [
            crate::sound_events::entity_key(enemy),
            crate::sound_events::entity_key(own)
        ]
    );
    assert!(
        sounds[5]
            .iter()
            .all(|sound| sound.sound == "burn_tick" && !sound.looping)
    );
    let attached: Vec<EntityId> = presentation.attached().iter().map(|a| a.entity).collect();
    assert_eq!(attached, [enemy], "one emitter, none on the own pawn");
    assert!(presentation.emitters()[1].is_some());
    assert!(
        !registry
            .has_component_kind(enemy, ComponentKind::BillboardEmitter)
            .unwrap(),
        "presentation is local: nothing joins the entity"
    );

    registry.clear_status(enemy, 1);
    registry.clear_status(own, 1);
    let after = frames(&mut presentation, &registry, Some(own), 10);
    assert!(
        after.iter().all(Vec::is_empty),
        "both stop when presence clears"
    );
    assert!(presentation.attached().is_empty());
}

// A connected client presents from replicated presence through the host's
// names: the same cadence from when presence appears, an emitter on every
// entity but its own pawn, and nothing for a status it does not declare.
#[test]
fn status_presentation_follows_replicated_presence_on_a_client() {
    let mut registry = EntityRegistry::new();
    registry.set_defense_vocabulary(vocabulary());
    let enemy = spawn(&mut registry, 0.0);
    let teammate = spawn(&mut registry, 3.0);
    let own = spawn(&mut registry, 6.0);
    // Host bit 0 is `burning`, bit 1 `shocked` (not declared here).
    registry.set_presence_status_names(&["burning".to_string(), "shocked".to_string()]);
    for entity in [enemy, teammate, own] {
        registry
            .set_component(entity, StatusPresenceComponent { bits: 0b11 })
            .unwrap();
    }
    let mut presentation = StatusPresentation::default();
    let sounds = frames(&mut presentation, &registry, Some(own), 6);
    assert_eq!(sounding(&sounds), [5], "one status tick, 500.01 ms, after");
    assert_eq!(
        sounds[5].len(),
        3,
        "every afflicted entity sounds, its own pawn too"
    );
    let mut attached: Vec<EntityId> = presentation.attached().iter().map(|a| a.entity).collect();
    attached.sort();
    let mut expected = vec![enemy, teammate];
    expected.sort();
    assert_eq!(attached, expected);

    // Presence reappearing restarts the count.
    registry
        .remove_component::<StatusPresenceComponent>(enemy)
        .unwrap();
    frames(&mut presentation, &registry, Some(own), 1);
    registry
        .set_component(enemy, StatusPresenceComponent { bits: 0b1 })
        .unwrap();
    let sounds = frames(&mut presentation, &registry, Some(own), 6);
    let enemy_key = crate::sound_events::entity_key(enemy);
    let enemy_frames: Vec<usize> = sounds
        .iter()
        .enumerate()
        .filter(|(_, sounds)| sounds.iter().any(|sound| anchored_to(sound) == enemy_key))
        .map(|(frame, _)| frame)
        .collect();
    assert_eq!(
        enemy_frames,
        [5],
        "one status tick after presence reappeared"
    );
}

// The tick sound keeps the damage cadence: `tickMs` 17 rounds up to two fixed
// ticks (33.33 ms), so at 10 ms frames it sounds at 40 ms and 70 ms, not every
// 17 ms.
#[test]
fn status_tick_sound_spaces_by_whole_fixed_ticks() {
    let mut registry = EntityRegistry::new();
    registry.set_defense_vocabulary(vocabulary());
    let enemy = spawn(&mut registry, 0.0);
    ignite(&mut registry, &[enemy], 2);
    let mut presentation = StatusPresentation::default();

    let sounds = frames_of(&mut presentation, &registry, None, 7, 10.0);
    assert_eq!(sounding(&sounds), [3, 6]);
    assert!(sounds[3].iter().all(|sound| sound.sound == "zap_tick"));
}

// Regression: an enemy's embers sat higher on the host than on a connected
// client, because only the host's copy carried an `AgentComponent`. The host's
// enemy and a client's display copy of it (no `Agent`, no `Health`, the
// descriptor hitbox on `HitVolume`) attach the emitter at the same body
// centre: the transform, already the agent capsule's centre, plus the hitbox
// offset.
#[test]
fn status_emitter_sits_at_the_same_body_centre_on_host_and_client() {
    // Authored from the feet as `[0, 1.0, 0]`; the transform sits 0.9 above
    // the feet, at the capsule centre of a 1.8 agent, so 0.1 remains.
    let hitbox = Hitbox {
        half_extents: Vec3::new(0.4, 0.9, 0.4),
        offset: Vec3::new(0.0, 0.1, 0.0),
    };
    let position = Vec3::new(2.0, 0.9, -1.0);
    let expected = Vec3::new(2.0, 1.0, -1.0);
    let body = Transform {
        position,
        ..Transform::default()
    };

    let mut host = EntityRegistry::new();
    host.set_defense_vocabulary(vocabulary());
    let enemy = host.spawn(body);
    host.set_component(enemy, health(Some(hitbox))).unwrap();
    host.set_component(enemy, AgentComponent::new(0.4, 1.8, 0.3, 3.0))
        .unwrap();
    ignite(&mut host, &[enemy], 1);
    let mut on_host = StatusPresentation::default();
    frames(&mut on_host, &host, None, 1);

    let mut client = EntityRegistry::new();
    client.set_defense_vocabulary(vocabulary());
    client.set_presence_status_names(&["burning".to_string()]);
    let copy = client.spawn(body);
    client
        .set_component(copy, HitVolumeComponent { hitbox })
        .unwrap();
    client
        .set_component(copy, StatusPresenceComponent { bits: 0b1 })
        .unwrap();
    let mut on_client = StatusPresentation::default();
    frames(&mut on_client, &client, None, 1);

    for (role, presentation) in [("host", &on_host), ("client", &on_client)] {
        let [attached] = presentation.attached() else {
            panic!("{role}: one emitter, got {:?}", presentation.attached());
        };
        assert!(
            (attached.origin - expected).length() < 1.0e-5,
            "{role}: emitter at {}, expected the body centre {expected}",
            attached.origin
        );
    }
}

// Regression: an entity despawned by the frame-end pass, after presentation
// built this frame's attached list, still emitted its status particles that
// frame.
#[test]
fn status_emitter_spawns_nothing_for_an_entity_despawned_before_render() {
    let mut registry = EntityRegistry::new();
    registry.set_defense_vocabulary(vocabulary());
    let despawned = spawn(&mut registry, 0.0);
    let kept = spawn(&mut registry, 5.0);
    ignite(&mut registry, &[despawned, kept], 1);
    let mut presentation = StatusPresentation::default();
    frames(&mut presentation, &registry, None, 1);
    assert_eq!(presentation.attached().len(), 2);

    registry.despawn(despawned).unwrap();
    let mut collector = ParticleRenderCollector::new();
    collector.register_sprite(&derive_collection_id(
        "ember",
        Some(0.6),
        None,
        0.0,
        None,
        None,
    ));
    let mut pool = ParticlePool::new();
    let mut bridge = EmitterBridge::new();
    bridge.update_attached(
        &registry,
        &presentation,
        FRAME_MS / 1000.0,
        0.0,
        &mut pool,
        &mut collector,
    );
    assert_eq!(pool.live_count(despawned), 0);
    assert!(pool.live_count(kept) > 0, "a live entity still emits");
}
