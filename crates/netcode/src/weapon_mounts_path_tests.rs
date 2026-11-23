// Tests: weapon mounts on every path a pawn presents through: single player,
// the listen host's own and slot pawns, and a connected client's own and
// remote pawns fed by host-produced snapshots.
// See: context/lib/networking.md §Weapon placement is content

use log::Level;
use postretro_entities::provenance::DescriptorSpawnPath;
use postretro_net::replication::ServerReplication;
use postretro_test_log_capture::LogCapture;

use super::tests::{
    BACKPACK, GRIP_TAPE, PISTOL_EXO, RIFLE_EXO, RIFLE_TROOPER, SLING, TRIMMER, descriptors, equip,
    mounted, pairs, select, spawn_pawn, store,
};
use super::*;
use crate::netcode::{HostCommandQueues, MovementOwners, NetworkIdAllocator};
use crate::replication::{ReplicableSet, produce_owned_snapshots};

const CLIENT_ID: u64 = 1;

/// One peer's mounted (socket, model) pairs.
type Mounts = Vec<(String, String)>;

/// A listen host and one connected client sharing a descriptor table. The
/// host runs its dirty-queue synchronizer before each snapshot, as
/// `net_serialize_and_send` does; the client applies each snapshot and runs
/// the presentation pass `client_receive_and_apply` runs after it.
struct Session {
    descriptors: Vec<EntityTypeDescriptor>,
    host: EntityRegistry,
    host_store: HitZoneStore,
    allocator: NetworkIdAllocator,
    replicable: ReplicableSet,
    owners: MovementOwners,
    weapon_owners: WeaponOwners,
    server: ServerReplication,
    tick: u32,
    client: EntityRegistry,
    client_store: HitZoneStore,
    replication: ClientReplication,
}

impl Session {
    fn new() -> Self {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_ID);
        Self {
            descriptors: descriptors(),
            host: EntityRegistry::new(),
            host_store: store(),
            allocator: NetworkIdAllocator::new(),
            replicable: ReplicableSet::new(),
            owners: MovementOwners::new(),
            weapon_owners: WeaponOwners::new(),
            server,
            tick: 0,
            client: EntityRegistry::new(),
            client_store: store(),
            replication: ClientReplication::new(),
        }
    }

    /// Spawn and register a host pawn. `owner` makes it the client's own pawn;
    /// otherwise it is the host's own pawn, which reaches the client as remote.
    fn spawn(&mut self, class: &str, owned_by_client: bool, weapons: &[&str]) -> EntityId {
        let spawn_path = if owned_by_client {
            DescriptorSpawnPath::NetworkSlot
        } else {
            DescriptorSpawnPath::PlayerSpawn
        };
        let (pawn, _) = spawn_pawn(
            &mut self.host,
            &self.descriptors,
            class,
            spawn_path,
            weapons,
            (!weapons.is_empty()).then_some(0),
        );
        if owned_by_client {
            self.owners.set(pawn, CLIENT_ID);
        }
        self.replicable.register(pawn);
        self.weapon_owners.mark_attachment_dirty(pawn);
        pawn
    }

    fn switch(&mut self, pawn: EntityId, slot: Option<usize>) {
        select(&mut self.host, pawn, slot);
        self.weapon_owners.mark_attachment_dirty(pawn);
    }

    /// One host tick and its snapshot through the client's apply.
    fn step(&mut self) {
        let changed = synchronize_weapon_owner_attachments(
            &mut self.host,
            &mut self.weapon_owners,
            &self.descriptors,
            &self.host_store,
        );
        crate::sim::resolve_mesh_entity_bindings_for_entities(
            &mut self.host,
            &crate::scripting_systems::mesh_anim::MeshClipTables::default(),
            &self.host_store,
            changed,
        );

        self.tick += 1;
        let owned = produce_owned_snapshots(
            &self.host,
            &self.replicable,
            &mut self.allocator,
            &self.owners,
            &HostCommandQueues::new(),
        );
        self.server.ingest_tick(owned);
        let sequence = self.server.begin_batch();
        let Some(raw) = self.server.encode_in_batch(CLIENT_ID, self.tick, sequence) else {
            return;
        };
        let snapshot = raw.validate().expect("host snapshot validates");
        let outcome = self.replication.apply_snapshot(&mut self.client, &snapshot);
        crate::snapshot_apply::materialize_snapshot_presentation(
            &outcome,
            &self.descriptors,
            &mut self.client,
            &mut self.replication,
            self.tick,
            None,
            &self.client_store,
            None,
            None,
            false,
            &Default::default(),
        );
        if let Some(ack) = outcome.ack {
            self.server.apply_ack(
                CLIENT_ID,
                ack.latest_snapshot_sequence,
                &ack.entity_baselines,
                &ack.despawn_tombstones,
            );
        }
    }

    /// The client entity presenting host pawn `pawn`.
    fn client_copy(&mut self, pawn: EntityId) -> EntityId {
        let network_id = self.allocator.stamp(pawn);
        *self
            .replication
            .map()
            .get(&network_id)
            .expect("the client mapped the pawn")
    }

    /// The client's repoint pass for its own pawn `copy`, as the frame runs it
    /// after a predicted switch and after a refused switch's rollback.
    fn client_repoint(&mut self, copy: EntityId) -> bool {
        synchronize_client_weapon_attachment_for_pawn(
            &mut self.client,
            copy,
            &self.replication,
            &self.descriptors,
            &self.client_store,
        )
    }

    /// Host and client mounts for `pawn`, after one step.
    fn mounts(&mut self, pawn: EntityId) -> (Mounts, Mounts) {
        let copy = self.client_copy(pawn);
        (mounted(&self.host, pawn), mounted(&self.client, copy))
    }

    /// The client's level unload, then the host's next level: the old pawns
    /// leave replication and the client starts from an empty registry.
    fn level_transition(&mut self) {
        let old: Vec<EntityId> = self.replicable.iter().collect();
        for pawn in old {
            self.replicable.unregister(pawn);
            self.owners.remove_pawn(pawn);
            let _ = self.host.despawn(pawn);
        }
        self.step();
        let _ = self.replication.reset_for_level_unload();
        self.client = EntityRegistry::new();
        self.host_store = store();
        self.client_store = store();
    }
}

#[test]
fn an_entry_mounts_its_models_at_their_sockets_on_every_path() {
    let rifle = pairs(&[("back", SLING), ("hand_r", RIFLE_EXO)]);

    // Single player: the install pass and a repoint use the single-pawn sync.
    let descriptors = descriptors();
    let store = store();
    let mut single_player = EntityRegistry::new();
    let (pawn, _) = spawn_pawn(
        &mut single_player,
        &descriptors,
        "player",
        DescriptorSpawnPath::PlayerSpawn,
        &["rifle"],
        Some(0),
    );
    assert!(synchronize_weapon_attachment_for_pawn(
        &mut single_player,
        pawn,
        &descriptors,
        &store
    ));
    assert_eq!(mounted(&single_player, pawn), rifle, "single player");

    // Listen host: its own pawn and a client's slot pawn; the client: its own
    // pawn (the slot pawn) and a remote one (the host's pawn).
    let mut session = Session::new();
    let host_pawn = session.spawn("player", false, &["rifle"]);
    let slot_pawn = session.spawn("player", true, &["rifle"]);
    session.step();
    assert_eq!(
        session.mounts(host_pawn),
        (rifle.clone(), rifle.clone()),
        "host own pawn; client remote pawn"
    );
    assert_eq!(
        session.mounts(slot_pawn),
        (rifle.clone(), rifle),
        "host slot pawn; client own pawn"
    );
}

#[test]
fn two_player_classes_on_different_rigs_mount_their_own_bakes_on_host_and_client() {
    let exo = pairs(&[("back", SLING), ("hand_r", RIFLE_EXO)]);
    let trooper = pairs(&[("back", SLING), ("hand_r", RIFLE_TROOPER)]);
    // Each class takes each role: the client's own pawn and the remote one.
    for (own_class, remote_class) in [("trooper", "player"), ("player", "trooper")] {
        let mut session = Session::new();
        let remote = session.spawn(remote_class, false, &["rifle"]);
        let own = session.spawn(own_class, true, &["rifle"]);
        session.step();
        let expected = |class: &str| {
            if class == "player" {
                exo.clone()
            } else {
                trooper.clone()
            }
        };
        assert_eq!(
            session.mounts(own),
            (expected(own_class), expected(own_class)),
            "own {own_class}"
        );
        assert_eq!(
            session.mounts(remote),
            (expected(remote_class), expected(remote_class)),
            "remote {remote_class}"
        );
    }
}

// The client's own pawn arms with a non-default class and its active
// weapon on one snapshot.
#[test]
fn a_client_own_pawn_of_a_non_default_class_mounts_its_class_entry_on_the_arming_snapshot() {
    let mut session = Session::new();
    let own = session.spawn("trooper", true, &["rifle"]);
    session.step();
    let copy = session.client_copy(own);
    assert_eq!(
        session.client.local_player_pawn(),
        Some(copy),
        "the first snapshot armed the pawn"
    );
    assert_eq!(
        mounted(&session.client, copy),
        pairs(&[("back", SLING), ("hand_r", RIFLE_TROOPER)])
    );
}

// The client's own pawn carries no descriptor provenance, so a predicted
// repoint resolves its class from the arming snapshot, not the default.
#[test]
fn a_client_own_pawn_keeps_its_class_entry_through_a_predicted_switch_and_its_refusal() {
    let mut session = Session::new();
    let own = session.spawn("trooper", true, &["rifle", "carbine"]);
    session.step();
    let copy = session.client_copy(own);
    assert!(
        session
            .client
            .get_component::<DescriptorProvenance>(copy)
            .is_err(),
        "fixture: the armed client pawn has no provenance"
    );
    // Production composes the client inventory from host tuning, which this
    // harness does not send.
    equip(&mut session.client, copy, &["rifle", "carbine"], Some(0));
    let rifle = pairs(&[("back", SLING), ("hand_r", RIFLE_TROOPER)]);
    // The trooper's carbine entry, where the default player's would mount
    // `RIFLE_EXO` and return the backpack.
    let carbine = pairs(&[("hand_r", RIFLE_TROOPER)]);
    assert_eq!(mounted(&session.client, copy), rifle);

    // The predicted lower repoints the local inventory to the carbine.
    select(&mut session.client, copy, Some(1));
    assert!(session.client_repoint(copy));
    assert_eq!(mounted(&session.client, copy), carbine, "predicted switch");

    // The host refuses; the rollback restores the rifle and its mount, and the
    // next snapshot leaves both.
    assert!(crate::sim::refuse_local_wieldable_switch(
        &mut session.client,
        copy,
        1,
        0
    ));
    assert!(session.client_repoint(copy));
    assert_eq!(mounted(&session.client, copy), rifle, "refused switch");
    session.step();
    assert_eq!(mounted(&session.client, copy), rifle, "after the refusal");

    // A predicted switch the host accepts converges on the same entry.
    select(&mut session.client, copy, Some(1));
    assert!(session.client_repoint(copy));
    session.switch(own, Some(1));
    session.step();
    assert_eq!(session.mounts(own), (carbine.clone(), carbine));
}

#[test]
fn a_switch_swaps_entries_and_returns_base_attachments_on_host_and_client() {
    let logs = LogCapture::start();
    let mut session = Session::new();
    let remote = session.spawn("player", false, &["rifle", "pistol", "pickup"]);
    let own = session.spawn("player", true, &["rifle", "pistol", "pickup"]);
    session.step();

    for (slot, expected) in [
        (
            Some(1),
            pairs(&[("back", BACKPACK), ("hand_r", PISTOL_EXO)]),
        ),
        (Some(2), pairs(&[("back", BACKPACK), ("hand_r", GRIP_TAPE)])),
        (Some(0), pairs(&[("back", SLING), ("hand_r", RIFLE_EXO)])),
        // A drop shows every base attachment, the covered hand_r included.
        (None, pairs(&[("back", BACKPACK), ("hand_r", GRIP_TAPE)])),
    ] {
        session.switch(remote, slot);
        session.switch(own, slot);
        session.step();
        for pawn in [remote, own] {
            assert_eq!(
                session.mounts(pawn),
                (expected.clone(), expected.clone()),
                "slot {slot:?}"
            );
        }
    }
    // One warning per peer for the pickup, however many pawns wield it.
    let pickup = "[Weapon mounts] entity `player` has no weaponMounts entry for weapon `pickup`; it mounts nothing";
    assert_eq!(
        logs.records()
            .iter()
            .filter(|record| record.level == Level::Warn && record.message.contains(pickup))
            .count(),
        2,
        "once on the host, once on the client"
    );
}

#[test]
fn two_switches_between_snapshots_leave_only_the_final_entry_on_host_and_client() {
    let mut session = Session::new();
    let remote = session.spawn("player", false, &["rifle", "pistol"]);
    let own = session.spawn("player", true, &["rifle", "pistol"]);
    session.step();
    for pawn in [remote, own] {
        session.switch(pawn, Some(1));
        session.switch(pawn, Some(0));
        session.switch(pawn, Some(1));
    }
    session.step();
    let pistol = pairs(&[("back", BACKPACK), ("hand_r", PISTOL_EXO)]);
    for pawn in [remote, own] {
        assert_eq!(session.mounts(pawn), (pistol.clone(), pistol.clone()));
    }
}

// The next level's player_spawn names another class and the pawn
// carries its active weapon through the transition.
#[test]
fn a_class_change_at_a_level_transition_mounts_the_new_class_entry_on_host_and_client() {
    let logs = LogCapture::start();
    let mut session = Session::new();
    session.spawn("player", false, &["rifle"]);
    session.spawn("player", true, &["rifle"]);
    session.step();

    session.level_transition();
    let remote = session.spawn("trooper", false, &["rifle"]);
    let own = session.spawn("trooper", true, &["rifle"]);
    session.step();
    let trooper = pairs(&[("back", SLING), ("hand_r", RIFLE_TROOPER)]);
    assert_eq!(session.mounts(remote), (trooper.clone(), trooper.clone()));
    assert_eq!(session.mounts(own), (trooper.clone(), trooper));

    // A class without a table shows its base attachments, with no warning.
    session.level_transition();
    let remote = session.spawn("robot", false, &["rifle"]);
    let own = session.spawn("robot", true, &["rifle"]);
    session.step();
    let robot = pairs(&[("hand_r", TRIMMER)]);
    assert_eq!(session.mounts(remote), (robot.clone(), robot.clone()));
    assert_eq!(session.mounts(own), (robot.clone(), robot));
    logs.assert_not_logged(Level::Warn, "[Weapon mounts]");
}
