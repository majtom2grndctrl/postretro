// Status presence on the wire: the host's live status kinds ride each
// afflicted entity's record as one bitset, mapped on a client by the host's
// status names, and stay out of the state-schema fingerprint.
// See: context/lib/networking.md §Version gates · context/lib/scripting.md §2

use std::sync::Arc;

use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::statuses::{StatusApplier, StatusPresenceComponent};
use postretro_entities::{SlotTable, Transform};
use postretro_foundation::defense::DefenseVocabulary;
use postretro_foundation::status::parse_statuses;
use postretro_net::wire::WireStatusPresence;

use super::*;
use crate::ReplicatedSlotIdentity;
use crate::state_slots::ReplicatedSlotSchema;

fn vocabulary(names: &[&str]) -> Arc<DefenseVocabulary> {
    let entries: Vec<_> = names
        .iter()
        .map(|name| serde_json::json!({ "kind": "status", "name": name, "damage": 0, "tickMs": 500 }))
        .collect();
    Arc::new(DefenseVocabulary {
        statuses: parse_statuses(Some(&serde_json::Value::Array(entries)), |_| None).unwrap(),
        ..Default::default()
    })
}

fn presence(
    registry: &EntityRegistry,
    set: &ReplicableSet,
    allocator: &mut NetworkIdAllocator,
) -> Vec<u64> {
    produce_owned_snapshots(
        registry,
        set,
        allocator,
        &MovementOwners::new(),
        &HostCommandQueues::new(),
    )
    .into_iter()
    .flat_map(|snapshot| snapshot.components)
    .filter_map(|payload| match payload {
        ComponentPayload::StatusPresence(WireStatusPresence { bits }) => Some(bits),
        _ => None,
    })
    .collect()
}

// Each snapshot's presence equals the host's live statuses when it
// serializes; an entity with none sends no payload.
#[test]
fn status_presence_payload_matches_host_at_serialize() {
    let mut registry = EntityRegistry::new();
    registry.set_defense_vocabulary(vocabulary(&["wet", "burning"]));
    let enemy = registry.spawn(Transform::default());
    registry
        .set_component(
            enemy,
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
    let mut set = ReplicableSet::new();
    set.register(enemy);
    let mut allocator = NetworkIdAllocator::new();
    assert!(
        presence(&registry, &set, &mut allocator).is_empty(),
        "no status, no payload"
    );

    registry.set_status_applications_immediate(true);
    let source = registry.intern_status_source("weapon.incendiary");
    registry.apply_status(enemy, 1, StatusApplier::Source(source), source);
    assert_eq!(presence(&registry, &set, &mut allocator), [0b10]);
    registry.apply_status(enemy, 0, StatusApplier::Source(source), source);
    assert_eq!(presence(&registry, &set, &mut allocator), [0b11]);
    registry.clear_status(enemy, 0);
    registry.clear_status(enemy, 1);
    assert!(presence(&registry, &set, &mut allocator).is_empty());
}

// A client shows a host status by name through the host's table: a reorder
// maps by the last installed table, a status the client does not declare
// shows nothing, and presence before any table shows nothing.
#[test]
fn client_maps_status_presence_by_host_name() {
    let mut client = EntityRegistry::new();
    client.set_defense_vocabulary(vocabulary(&["burning", "wet", "chilled"]));
    let enemy = client.spawn(Transform::default());
    // Host bit 0 is `wet`, bit 1 `burning`, bit 2 `shocked`.
    client
        .set_component(enemy, StatusPresenceComponent { bits: 0b110 })
        .unwrap();
    assert!(
        !client.shows_status(enemy, "burning"),
        "no host table yet: nothing shows"
    );

    let host = [
        "wet".to_string(),
        "burning".to_string(),
        "shocked".to_string(),
    ];
    client.set_presence_status_names(&host);
    assert!(client.shows_status(enemy, "burning"));
    assert!(!client.shows_status(enemy, "wet"));
    assert!(!client.shows_status(enemy, "chilled"));
    assert!(
        !client.shows_status(enemy, "shocked"),
        "the client does not declare it"
    );

    // The host reloads with a reordered table; the new names map the bits.
    client.set_presence_status_names(&["burning".to_string(), "wet".to_string()]);
    assert!(client.shows_status(enemy, "wet"), "bit 1 now names `wet`");
    assert!(!client.shows_status(enemy, "burning"));
}

// Declaring statuses adds only local-only `player.status.*` slots, so the
// state-schema fingerprint is unchanged and no client is held below
// participation by a status difference.
#[test]
fn statuses_change_neither_fingerprint_nor_replicated_schema() {
    let identity = ReplicatedSlotIdentity::default();
    let slots = SlotTable::new();
    let before = *ReplicatedSlotSchema::build(&slots, &identity).fingerprint();
    let mut with_statuses = SlotTable::new();
    with_statuses.replace_player_status_slots(&["burning".to_string(), "wet".to_string()]);
    assert!(with_statuses.get("player.status.burning").is_some());
    let after = *ReplicatedSlotSchema::build(&with_statuses, &identity).fingerprint();
    assert_eq!(before, after);
    assert!(
        postretro_scripting_core::player_event_scope::is_local_player_slot("player.status.burning")
    );
}
