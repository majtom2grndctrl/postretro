// Client ghost-link binding, the received-state queue, resets and presented pose stamps.
// See: context/lib/networking.md §Current contract
use super::*;
use postretro_entities::GhostLinkComponent;
use postretro_net::wire::{
    WireGhostLinkState, WireGroundRef, WireMovementState, WirePlayerMovementState,
    WireTeleportState, WireTransform,
};

const ALL_MEMBERS: u8 = 0b1111;

fn link_transform() -> ComponentPayload {
    ComponentPayload::Transform(WireTransform {
        position: [1.0, 2.0, 3.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
    })
}

fn link(index: u16, source_entity: u32, armed: bool, members: u8) -> Vec<ComponentPayload> {
    vec![
        link_transform(),
        ComponentPayload::GhostLinkState(WireGhostLinkState::new(
            index,
            source_entity,
            armed,
            members,
        )),
    ]
}

fn baseline(network_id: u32, baseline_id: u32, components: Vec<ComponentPayload>) -> EntityRecord {
    EntityRecord::FullBaseline {
        network_id,
        baseline_id,
        last_processed_client_tick: None,
        local_player: false,
        entity_class: None,
        active_weapon_archetype: None,
        projectile_presentation: None,
        presentation_binding: None,
        components,
    }
}

fn delta(network_id: u32, baseline_ref: u32, components: Vec<ComponentPayload>) -> EntityRecord {
    EntityRecord::Delta {
        network_id,
        baseline_ref,
        new_baseline_id: baseline_ref + 1,
        components,
        local_player: false,
        last_processed_client_tick: None,
        entity_class: None,
        active_weapon_archetype: None,
        projectile_presentation: None,
        presentation_binding: None,
    }
}

fn snapshot(sequence: u32, server_tick: u32, records: Vec<EntityRecord>) -> SnapshotMessage {
    SnapshotMessage {
        sequence,
        server_tick,
        records,
        state_schema_fingerprint: [0u8; 32],
        state_records: Vec::new(),
        faction_sentiment_record: None,
        server_script_time: 0.0,
    }
}

/// An installed link as level install spawns it on every peer: disarmed,
/// showing every category, at its companion's origin.
fn installed_link(registry: &mut EntityRegistry, index: u16, source_entity: u32) -> EntityId {
    let installed = registry.spawn(Transform::default());
    registry
        .set_component(
            installed,
            GhostLinkComponent {
                index,
                source_entity,
                armed: false,
                members: ALL_MEMBERS,
            },
        )
        .unwrap();
    installed
}

fn local_state(registry: &EntityRegistry, installed: EntityId) -> GhostLinkComponent {
    *registry
        .get_component::<GhostLinkComponent>(installed)
        .unwrap()
}

fn drained(client: &mut ClientReplication) -> Vec<(u32, bool, u8)> {
    client
        .drain_ghost_link_updates()
        .map(|update| (update.server_tick, update.state.armed, update.state.members))
        .collect()
}

fn acked(outcome: &ApplyOutcome, network_id: u32) -> bool {
    outcome.ack.as_ref().is_some_and(|ack| {
        ack.entity_baselines
            .iter()
            .any(|(acked, _)| *acked == network_id)
    })
}

// P11: a record naming a link this client has not installed binds nothing,
// warns, and is not acked, so the host keeps sending it and a later baseline
// binds once the link exists.
#[test]
fn ghost_link_record_naming_no_installed_link_binds_nothing_and_is_not_acked() {
    let mut registry = EntityRegistry::new();
    let other = installed_link(&mut registry, 0, 10);
    let mut client = ClientReplication::new();
    let entities_before = registry.iter_with_kind(ComponentKind::Transform).count();

    let outcome = client.apply_snapshot(
        &mut registry,
        &snapshot(1, 20, vec![baseline(7, 1, link(1, 11, true, ALL_MEMBERS))]),
    );
    assert_eq!(
        outcome.ignored,
        vec![IgnoredPayload::UnknownGhostLink {
            network_id: 7,
            index: 1,
            source_entity: 11,
        }]
    );
    assert!(!acked(&outcome, 7), "an unbound record is not acked");
    assert!(client.entity_for_network_id(NetworkId(7)).is_none());
    assert_eq!(
        registry.iter_with_kind(ComponentKind::Transform).count(),
        entities_before,
        "nothing spawns from a link record"
    );
    assert!(drained(&mut client).is_empty(), "nothing queues");
    assert!(!local_state(&registry, other).armed);

    // The link installs; the host's next baseline binds it.
    let installed = installed_link(&mut registry, 1, 11);
    let outcome = client.apply_snapshot(
        &mut registry,
        &snapshot(2, 22, vec![baseline(7, 1, link(1, 11, true, ALL_MEMBERS))]),
    );
    assert!(outcome.ignored.is_empty());
    assert!(acked(&outcome, 7));
    assert_eq!(client.entity_for_network_id(NetworkId(7)), Some(installed));
    assert_eq!(drained(&mut client), [(22, true, ALL_MEMBERS)]);
}

// Received state never writes the installed component, which stays this
// client's local spawn state; every applied record queues with its snapshot's
// server tick, so a state between two frames is not lost.
#[test]
fn ghost_link_records_queue_at_their_snapshot_stamp_and_leave_spawn_state_alone() {
    let mut registry = EntityRegistry::new();
    let installed = installed_link(&mut registry, 0, 10);
    let spawn = local_state(&registry, installed);
    let spawn_pose = *registry.get_component::<Transform>(installed).unwrap();
    let mut client = ClientReplication::new();

    client.apply_snapshot(
        &mut registry,
        &snapshot(1, 40, vec![baseline(5, 1, link(0, 10, true, ALL_MEMBERS))]),
    );
    client.apply_snapshot(
        &mut registry,
        &snapshot(2, 42, vec![delta(5, 1, link(0, 10, false, 0b0001))]),
    );
    client.apply_snapshot(
        &mut registry,
        &snapshot(3, 44, vec![delta(5, 2, link(0, 10, true, 0b0001))]),
    );

    assert_eq!(
        drained(&mut client),
        [
            (40, true, ALL_MEMBERS),
            (42, false, 0b0001),
            (44, true, 0b0001)
        ],
        "every applied state, in order, at its own stamp"
    );
    assert!(drained(&mut client).is_empty(), "a drain empties the queue");
    assert_eq!(local_state(&registry, installed), spawn);
    assert_eq!(
        *registry.get_component::<Transform>(installed).unwrap(),
        spawn_pose,
        "a link is never moved or interpolated by its record"
    );
}

// A record disagreeing with its binding is rejected with a warning: it names
// another link, so nothing it carries may reach this link's history.
#[test]
fn ghost_link_record_disagreeing_with_its_binding_is_rejected() {
    let mut registry = EntityRegistry::new();
    installed_link(&mut registry, 0, 10);
    installed_link(&mut registry, 1, 11);
    let mut client = ClientReplication::new();
    client.apply_snapshot(
        &mut registry,
        &snapshot(1, 10, vec![baseline(5, 1, link(0, 10, false, ALL_MEMBERS))]),
    );
    drained(&mut client);

    for (index, source) in [(1, 11), (0, 99)] {
        client.apply_snapshot(
            &mut registry,
            &snapshot(
                2 + u32::from(index),
                12,
                vec![delta(5, 1, link(index, source, true, ALL_MEMBERS))],
            ),
        );
        assert!(
            drained(&mut client).is_empty(),
            "index {index} source {source} never queues against link 0"
        );
    }
}

// Unbinding a link (a despawn record, demotion) leaves the installed entity in
// place: links are level content, never replication-created copies.
#[test]
fn ghost_link_unbinding_never_despawns_the_installed_link() {
    let mut registry = EntityRegistry::new();
    let installed = installed_link(&mut registry, 0, 10);
    let mut client = ClientReplication::new();
    client.apply_snapshot(
        &mut registry,
        &snapshot(1, 10, vec![baseline(5, 1, link(0, 10, true, ALL_MEMBERS))]),
    );
    client.apply_snapshot(
        &mut registry,
        &snapshot(
            2,
            12,
            vec![EntityRecord::Despawn {
                network_id: 5,
                tombstone_id: 1,
                reason: 0,
            }],
        ),
    );
    assert!(registry.exists(installed));
    assert!(client.entity_for_network_id(NetworkId(5)).is_none());

    client.apply_snapshot(
        &mut registry,
        &snapshot(3, 14, vec![baseline(6, 1, link(0, 10, true, ALL_MEMBERS))]),
    );
    client.despawn_all_mapped(&mut registry);
    client.reset_for_demotion();
    assert!(registry.exists(installed), "demotion only unbinds the link");
}

// AC41 / P38: demotion and level unload drop received link state. Queued
// updates never reach presentation after the reset, and the binding is gone,
// so the next participation's first baseline binds afresh.
#[test]
fn ghost_link_demotion_and_unload_drop_received_state_and_bindings() {
    for unload in [false, true] {
        let mut registry = EntityRegistry::new();
        let installed = installed_link(&mut registry, 0, 10);
        let mut client = ClientReplication::new();
        client.apply_snapshot(
            &mut registry,
            &snapshot(1, 10, vec![baseline(5, 1, link(0, 10, true, ALL_MEMBERS))]),
        );
        if unload {
            let _ = client.reset_for_level_unload();
        } else {
            client.despawn_all_mapped(&mut registry);
            client.reset_for_demotion();
        }
        assert!(
            drained(&mut client).is_empty(),
            "unload={unload}: no received state survives the reset"
        );
        assert!(client.entity_for_network_id(NetworkId(5)).is_none());
        assert!(!local_state(&registry, installed).armed);

        // A new participation binds the same installed link from its baseline.
        client.apply_snapshot(
            &mut registry,
            &snapshot(10, 30, vec![baseline(9, 1, link(0, 10, false, 0b0011))]),
        );
        assert_eq!(client.entity_for_network_id(NetworkId(9)), Some(installed));
        assert_eq!(drained(&mut client), [(30, false, 0b0011)]);
    }
}

fn pawn_record(x: f32, generation: u8) -> Vec<ComponentPayload> {
    vec![
        ComponentPayload::Transform(WireTransform {
            position: [x, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }),
        ComponentPayload::PlayerMovementState(WirePlayerMovementState {
            teleport: WireTeleportState::new(generation, 0.0, 0, &[]).unwrap(),
            knockback_velocity: [0.0; 3],
            push_velocity: [0.0; 3],
            velocity: [0.0; 3],
            ground: WireGroundRef::World,
            air_jumps_remaining: 0,
            air_dashes_remaining: 0,
            dash_cooldown_ms: 0.0,
            air_ticks: 0,
            movement_state: WireMovementState::Normal,
            coyote_timer_ms: 0.0,
            jump_buffer_timer_ms: 0.0,
            jump_spent: false,
            swim_reentry_latched: false,
            capsule_half_height: 0.8,
            capsule_eye_height: 1.5,
            aim_pitch: 0.0,
        }),
    ]
}

// P18 at the netcode seam: each remote reports the host tick its presented pose
// shows. A blend shows its leading sample; a pose held on the near side of a
// seamless cut shows no state of the cut's own tick, and the cut frame does.
#[test]
fn ghost_link_presented_state_tick_holds_before_a_cut_and_leads_a_blend() {
    let mut registry = EntityRegistry::new();
    let mut client = ClientReplication::new();
    let pawn_baseline = |sequence, tick, baseline_id, x, generation| {
        snapshot(
            sequence,
            tick,
            vec![baseline(3, baseline_id, pawn_record(x, generation))],
        )
    };
    client.apply_snapshot(&mut registry, &pawn_baseline(1, 10, 1, 0.0, 0));
    client.apply_snapshot(&mut registry, &pawn_baseline(2, 12, 2, 1.0, 0));
    // The seamless swap lands in the snapshot stamped 14.
    client.apply_snapshot(&mut registry, &pawn_baseline(3, 14, 3, 100.0, 1));
    client.apply_snapshot(&mut registry, &pawn_baseline(4, 16, 4, 101.0, 1));
    let pawn = client.entity_for_network_id(NetworkId(3)).unwrap();

    let mut at = |render_tick: f64| {
        client.sample_into_registry(&mut registry, render_tick, 0.0);
        let x = registry
            .get_component::<Transform>(pawn)
            .unwrap()
            .position
            .x;
        (client.presented_state_tick(pawn).unwrap(), x)
    };
    let (stamp, x) = at(10.5);
    assert_eq!(stamp, 12.0, "blend toward 12 shows 12's state");
    assert!(x > 0.0 && x < 1.0);
    for render_tick in [12.25, 12.9, 13.0, 13.75, 13.999] {
        let (stamp, x) = at(render_tick);
        assert_eq!(x, 1.0, "held on the source side of the cut");
        assert!(
            stamp.ceil() < 14.0,
            "render {render_tick}: a state stamped with the cut stays hidden ({stamp})"
        );
        assert!(stamp >= render_tick - 1.0);
    }
    let (stamp, x) = at(14.0);
    assert_eq!(
        (stamp, x),
        (14.0, 100.0),
        "the cut frame shows the cut's tick"
    );
    let (stamp, _) = at(14.5);
    assert_eq!(stamp, 16.0);

    // Before time sync names a render tick, remotes hold and report nothing.
    client.apply_held_remote_player_presentation(&mut registry, 0.0);
    assert_eq!(client.presented_state_tick(pawn), None);
}
