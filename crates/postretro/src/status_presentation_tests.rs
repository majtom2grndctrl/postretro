// Status presentation follows presence on each machine: one emitter per
// afflicted entity (none on this machine's own pawn), one anchored tick sound
// one `tickMs` after presence appears and every `tickMs` after, both stopping
// when presence clears.
// See: context/lib/scripting.md §2 (Status declarations)

use std::sync::Arc;

use glam::Vec3;
use postretro_audio::{SoundAnchor, SoundRequest};
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::statuses::{StatusApplier, StatusPresenceComponent};
use postretro_entities::{ComponentKind, EntityId, EntityRegistry, Transform};
use postretro_foundation::defense::DefenseVocabulary;
use postretro_foundation::status::parse_statuses;

use super::StatusPresentation;

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
            ])),
            |_| None,
        )
        .unwrap(),
        ..Default::default()
    })
}

fn spawn(registry: &mut EntityRegistry, x: f32) -> EntityId {
    let entity = registry.spawn(Transform {
        position: Vec3::new(x, 1.0, 0.0),
        ..Transform::default()
    });
    registry
        .set_component(
            entity,
            HealthComponent {
                max: 100.0,
                current: 100.0,
                hitbox: None,
                death_handled: false,
                pending_kill_credit: None,
                zone_multipliers: Default::default(),
                contributor_ledger: Default::default(),
            },
        )
        .unwrap();
    entity
}

/// Frames of `dt_ms`, returning the sounds requested per frame.
fn frames(
    presentation: &mut StatusPresentation,
    registry: &EntityRegistry,
    local: Option<EntityId>,
    n: usize,
) -> Vec<Vec<SoundRequest>> {
    (0..n)
        .map(|_| {
            let mut sounds = Vec::new();
            presentation.update(registry, local, 100.0, &mut sounds);
            sounds
        })
        .collect()
}

fn anchored_to(sound: &SoundRequest) -> u64 {
    match &sound.anchor {
        Some(SoundAnchor::Entity { key, .. }) => *key,
        other => panic!("tick sounds anchor on the entity, got {other:?}"),
    }
}

// The host: an enemy and the host's own pawn both burn. The enemy gets an
// emitter, the own pawn none; each requests `burn_tick` at 500 ms and every
// 500 ms after; clearing presence stops both, and no emitter component is
// ever added to the entity.
#[test]
fn status_presentation_follows_presence_on_the_host() {
    let mut registry = EntityRegistry::new();
    registry.set_defense_vocabulary(vocabulary());
    let enemy = spawn(&mut registry, 0.0);
    let own = spawn(&mut registry, 5.0);
    registry.set_status_applications_immediate(true);
    let source = registry.intern_status_source("weapon.incendiary");
    for entity in [enemy, own] {
        registry.apply_status(entity, 1, StatusApplier::Source(source), source);
    }
    let mut presentation = StatusPresentation::default();

    let sounds = frames(&mut presentation, &registry, Some(own), 10);
    let sounding: Vec<usize> = sounds
        .iter()
        .enumerate()
        .filter(|(_, sounds)| !sounds.is_empty())
        .map(|(frame, _)| frame)
        .collect();
    assert_eq!(sounding, [4, 9], "at 500 ms and 1000 ms");
    let keys: Vec<u64> = sounds[4].iter().map(anchored_to).collect();
    assert_eq!(
        keys,
        [
            crate::sound_events::entity_key(enemy),
            crate::sound_events::entity_key(own)
        ]
    );
    assert!(
        sounds[4]
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
    let sounds = frames(&mut presentation, &registry, Some(own), 5);
    assert_eq!(
        sounds[4].len(),
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
    let sounds = frames(&mut presentation, &registry, Some(own), 5);
    let enemy_key = crate::sound_events::entity_key(enemy);
    let enemy_frames: Vec<usize> = sounds
        .iter()
        .enumerate()
        .filter(|(_, sounds)| sounds.iter().any(|sound| anchored_to(sound) == enemy_key))
        .map(|(frame, _)| frame)
        .collect();
    assert_eq!(enemy_frames, [4], "one tickMs after presence reappeared");
}
