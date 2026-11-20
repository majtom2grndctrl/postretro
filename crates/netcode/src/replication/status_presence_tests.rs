// Status presence on the wire: the host's live status kinds ride each
// afflicted entity's record as one bitset, mapped on a client by the host's
// status names, and stay out of the state-schema fingerprint.
// See: context/lib/networking.md §Version gates · context/lib/scripting.md §2

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use postretro_combat_model::{EnvironmentTuning, FluidMovementTable, TuningPayload};
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::statuses::{StatusApplier, StatusPresenceComponent};
use postretro_entities::{SlotTable, Transform};
use postretro_foundation::defense::DefenseVocabulary;
use postretro_foundation::status::parse_statuses;
use postretro_net::wire::{
    EntityRecord, NetworkId, SnapshotMessage, WireStatusPresence, WireTransform,
};

use super::*;
use crate::state_slots::ReplicatedSlotSchema;
use crate::{ClientReplication, NetEndpoint, NetRole, ReplicatedSlotIdentity};

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

/// One replicated enemy's components as the host sends them: a transform,
/// and a presence payload only while a status is live.
fn enemy_components(presence: Option<u64>) -> Vec<ComponentPayload> {
    let mut components = vec![ComponentPayload::Transform(WireTransform {
        position: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
    })];
    components.extend(presence.map(|bits| ComponentPayload::StatusPresence(WireStatusPresence { bits })));
    components
}

fn baseline(network_id: u32, baseline_id: u32, presence: Option<u64>) -> EntityRecord {
    EntityRecord::FullBaseline {
        assist_facts: None,
        network_id,
        baseline_id,
        last_processed_client_tick: None,
        local_player: false,
        entity_class: None,
        active_weapon_archetype: None,
        projectile_presentation: None,
        presentation_binding: None,
        components: enemy_components(presence),
    }
}

fn delta(network_id: u32, baseline_ref: u32, presence: Option<u64>) -> EntityRecord {
    EntityRecord::Delta {
        assist_facts: None,
        network_id,
        baseline_ref,
        new_baseline_id: baseline_ref + 1,
        last_processed_client_tick: None,
        local_player: false,
        entity_class: None,
        active_weapon_archetype: None,
        projectile_presentation: None,
        presentation_binding: None,
        components: enemy_components(presence),
    }
}

fn snapshot(sequence: u32, records: Vec<EntityRecord>) -> SnapshotMessage {
    SnapshotMessage {
        sequence,
        server_tick: sequence + 1,
        records,
        state_schema_fingerprint: [0u8; 32],
        state_records: Vec::new(),
        faction_sentiment_record: None,
        ghost_audio: None,
        server_script_time: 0.0,
        level_tick: 0,
    }
}

fn presence_bits(registry: &EntityRegistry, entity: EntityId) -> Option<u64> {
    registry
        .get_component::<StatusPresenceComponent>(entity)
        .ok()
        .map(|presence| presence.bits)
}

// The client apply path: a record carrying presence sets the component, a
// later record for the same entity without it removes it, and the status
// shows only while both the presence and the host's name for its bit are held.
#[test]
fn client_record_apply_sets_and_removes_status_presence() {
    let mut registry = EntityRegistry::new();
    registry.set_defense_vocabulary(vocabulary(&["burning"]));
    let mut client = ClientReplication::new();

    client.apply_snapshot(&mut registry, &snapshot(1, vec![baseline(7, 1, Some(0b10))]));
    let enemy = client
        .entity_for_network_id(NetworkId(7))
        .expect("the baseline maps the enemy");
    assert_eq!(presence_bits(&registry, enemy), Some(0b10));
    assert!(
        !registry.shows_status(enemy, "burning"),
        "no host table yet: nothing shows"
    );

    // Host bit 0 is `wet`, bit 1 `burning`.
    registry.set_presence_status_names(&["wet".to_string(), "burning".to_string()]);
    assert!(registry.shows_status(enemy, "burning"));

    client.apply_snapshot(&mut registry, &snapshot(2, vec![delta(7, 1, Some(0b11))]));
    assert_eq!(presence_bits(&registry, enemy), Some(0b11), "a delta overwrites");
    assert!(registry.shows_status(enemy, "burning"));

    client.apply_snapshot(&mut registry, &snapshot(3, vec![delta(7, 2, None)]));
    assert_eq!(
        presence_bits(&registry, enemy),
        None,
        "a record without presence clears it"
    );
    assert!(!registry.shows_status(enemy, "burning"));
}

fn client_endpoint() -> NetEndpoint {
    NetEndpoint::from_role(
        &NetRole::Connect {
            addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 1)),
        },
        None,
    )
    .unwrap()
    .unwrap()
}

fn tuning_with_statuses(names: &[&str]) -> Vec<u8> {
    crate::tuning_payload::encode_tuning_payload(&TuningPayload::new(
        None,
        std::array::from_fn(|_| None),
        EnvironmentTuning {
            level_gravity: -12.0,
            fluids: FluidMovementTable::default(),
            status_names: names.iter().map(|name| name.to_string()).collect(),
        },
    ))
}

/// Apply one snapshot through the endpoint's own client replication.
fn apply_on(
    endpoint: &mut NetEndpoint,
    registry: &mut EntityRegistry,
    snapshot: &SnapshotMessage,
) -> EntityId {
    let NetEndpoint::Client { replication, .. } = endpoint else {
        unreachable!("a connect role builds a client endpoint");
    };
    replication.apply_snapshot(registry, snapshot);
    let network_id = match &snapshot.records[0] {
        EntityRecord::FullBaseline { network_id, .. } | EntityRecord::Delta { network_id, .. } => {
            *network_id
        }
        EntityRecord::Despawn { .. } => unreachable!("only live records here"),
    };
    replication
        .entity_for_network_id(NetworkId(network_id))
        .expect("the record maps its entity")
}

// Regression: the host's status names outlived the tuning a level unload or
// demotion dropped, so presence that landed before the next payload mapped
// through the old table and showed the wrong status.
#[test]
fn presence_before_the_next_tuning_shows_nothing_after_unload_or_demotion() {
    for demote in [false, true] {
        let mut registry = EntityRegistry::new();
        registry.set_defense_vocabulary(vocabulary(&["burning"]));
        let mut endpoint = client_endpoint();
        endpoint.install_tuning_payload(&tuning_with_statuses(&["burning"]), &mut registry, &[]);
        let before = apply_on(
            &mut endpoint,
            &mut registry,
            &snapshot(1, vec![baseline(7, 1, Some(0b1))]),
        );
        assert!(registry.shows_status(before, "burning"));

        if demote {
            endpoint.demote_client_state(&mut registry);
        } else {
            endpoint.reset_level_scoped_client_state(&mut registry);
            registry.clear_for_level_unload();
        }

        // The next host table puts `burning` at bit 1; bit 0 lands before it.
        let after = apply_on(
            &mut endpoint,
            &mut registry,
            &snapshot(2, vec![baseline(8, 1, Some(0b1))]),
        );
        assert!(
            !registry.shows_status(after, "burning"),
            "presence before the next table shows nothing (demote: {demote})"
        );

        endpoint.install_tuning_payload(
            &tuning_with_statuses(&["wet", "burning"]),
            &mut registry,
            &[],
        );
        assert!(
            !registry.shows_status(after, "burning"),
            "bit 0 names `wet` now"
        );
        apply_on(
            &mut endpoint,
            &mut registry,
            &snapshot(3, vec![delta(8, 1, Some(0b10))]),
        );
        assert!(registry.shows_status(after, "burning"));
    }
}

// A payload that fails to decode drops the installed tuning, so its status
// names go with it.
#[test]
fn undecodable_tuning_payload_drops_the_status_name_table() {
    let mut registry = EntityRegistry::new();
    registry.set_defense_vocabulary(vocabulary(&["burning"]));
    let mut endpoint = client_endpoint();
    endpoint.install_tuning_payload(&tuning_with_statuses(&["burning"]), &mut registry, &[]);
    let enemy = apply_on(
        &mut endpoint,
        &mut registry,
        &snapshot(1, vec![baseline(7, 1, Some(0b1))]),
    );
    assert!(registry.shows_status(enemy, "burning"));

    endpoint.install_tuning_payload(b"not a tuning payload", &mut registry, &[]);
    assert!(endpoint.client_host_environment().is_none());
    assert!(!registry.shows_status(enemy, "burning"));
}
