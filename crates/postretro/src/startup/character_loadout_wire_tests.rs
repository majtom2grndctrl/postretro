//! A character's loadout between a host and its guests: the host's check of a
//! join seed's loadout, a held seat's loadout winning on rejoin, the final
//! character's loadout, and per-owner loadout messages; the roster names a
//! joiner by its character; two characters from one device keep their seats.
//! See: context/lib/networking.md §Session-state ledger · §Slot lifecycle

use std::collections::BTreeMap;

use crate::startup::{LevelRequest, LevelSource};
use log::Level;
use postretro_combat_model::CarriedState;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{AmmoReserve, EntityId};
use postretro_foundation::{Seat, WeaponResource};
use postretro_net::wire::{
    CarriedLoadoutWire, ConnectClaim, DISPLAY_NAME_MAX_BYTES, PlayerClaimId, ServerControlMessage,
};
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_scripting_core::runtime::SavesManifest;
use postretro_test_log_capture::LogCapture;

use super::lifecycle_test_fixtures::{script_ctx, test_app};
use super::spawn_pose_handoff_fixtures::{Relay, spawn_points};
use crate::coop_fixture_tests::initialize_dev_mod;
use crate::scripting::primitives::register_all;

/// The dev mod's gear, read from its own registry.
struct DevGear {
    /// Two weapons on ammo: canonical name, ammo type, magazine size.
    weapons: [(String, String, u32); 2],
    health_max: f32,
    /// A registered defense layer.
    layer: String,
    /// A player life state other than the initial one.
    non_initial_life_state: String,
}

/// A listen host running the dev mod, with no level and no clients.
fn dev_mod_host() -> Relay {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    let mut primitives = PrimitiveRegistry::new();
    register_all(&mut primitives, ctx.clone());
    initialize_dev_mod(&mut app, &primitives);
    ctx.data_registry.borrow_mut().recompose(&[]);
    ctx.registry.borrow_mut().clear_for_level_unload();
    Relay::host_on(app)
}

fn dev_gear(relay: &Relay) -> DevGear {
    let ctx = script_ctx(&relay.app);
    let data = ctx.data_registry.borrow();
    let mut ammo_weapons = data.entities.iter().filter_map(|descriptor| {
        match descriptor.weapon.as_ref()?.resource.as_ref()? {
            WeaponResource::Ammo(ammo) if ammo.magazine > 0 => Some((
                descriptor.canonical_name.clone()?,
                ammo.ammo_type.clone(),
                ammo.magazine,
            )),
            _ => None,
        }
    });
    let weapons = [
        ammo_weapons.next().expect("fixture: a dev weapon on ammo"),
        ammo_weapons
            .next()
            .expect("fixture: a second dev weapon on ammo"),
    ];
    let player = data
        .entities
        .iter()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some("player"))
        .expect("fixture: the dev mod's player");
    let life_states = player
        .life_states
        .as_ref()
        .expect("fixture: player life states");
    DevGear {
        weapons,
        health_max: player.health.as_ref().expect("fixture: player health").max,
        layer: data
            .defense
            .layers
            .first()
            .cloned()
            .expect("fixture: a dev layer"),
        non_initial_life_state: life_states
            .states
            .keys()
            .find(|state| **state != life_states.initial)
            .cloned()
            .expect("fixture: a non-initial life state"),
    }
}

/// A loadout this mod fully defines: the first weapon in slot 0 with one
/// round short of a full magazine, its ammo, `health` and half a layer.
fn valid_loadout(gear: &DevGear, health: f32) -> CarriedLoadoutWire {
    let (weapon, ammo, magazine) = &gear.weapons[0];
    CarriedLoadoutWire {
        health_centi: Some((health * 100.0).round() as i32),
        life_state: None,
        reserve: vec![(ammo.clone(), 11)],
        wieldables: vec![Some(weapon.clone())],
        magazines: vec![Some(magazine - 1)],
        active_slot: 0,
        layers: vec![(gear.layer.clone(), 1_000)],
    }
}

/// Connect `client_id` asserting `claim_of`'s identity, carrying `loadout` in
/// its join seed, and pump until its pawn stands in the host's level.
fn join_with(
    relay: &mut Relay,
    client_id: u64,
    claim_of: u64,
    loadout: CarriedLoadoutWire,
) -> EntityId {
    relay
        .connect_as(client_id, claim_of)
        .set_join_seed(BTreeMap::new(), Some(loadout));
    relay.reveal(client_id);
    relay.await_pawn(client_id)
}

fn hosting_dev_level() -> (Relay, DevGear, EntityId) {
    let mut relay = dev_mod_host();
    let gear = dev_gear(&relay);
    let (host_pawn, _) = relay.install(spawn_points(4)).expect("the host's pawn");
    relay.host_reveal();
    (relay, gear, host_pawn)
}

fn carry(relay: &Relay, client_id: u64) -> Option<CarriedState> {
    let seat = relay.seat(client_id).expect("a seated client");
    relay.seats().carried_state(seat).cloned()
}

fn health(relay: &Relay, pawn: EntityId) -> f32 {
    script_ctx(&relay.app)
        .registry
        .borrow()
        .get_component::<HealthComponent>(pawn)
        .unwrap()
        .current
}

/// The canonical name of the weapon in `pawn`'s `slot`.
fn weapon_in(relay: &Relay, pawn: EntityId, slot: usize) -> Option<String> {
    let ctx = script_ctx(&relay.app);
    let registry = ctx.registry.borrow();
    let weapon = registry.get_component::<Inventory>(pawn).ok()?.wieldables[slot]?;
    Some(
        registry
            .get_component::<DescriptorProvenance>(weapon)
            .ok()?
            .canonical_name
            .clone(),
    )
}

/// Every loadout `messages` carry, by the message that carried it.
fn loadouts(messages: &[ServerControlMessage]) -> Vec<CarriedLoadoutWire> {
    messages
        .iter()
        .filter_map(|message| match message {
            ServerControlMessage::CharacterLoadout { loadout } => Some(loadout.clone()),
            ServerControlMessage::FinalCharacter { loadout, .. } => loadout.clone(),
            // No other message carries a loadout; the roster frame included.
            ServerControlMessage::Divergence(_)
            | ServerControlMessage::Tuning(_)
            | ServerControlMessage::Relevel { .. }
            | ServerControlMessage::SessionRoster(_)
            | ServerControlMessage::SpawnPoseHint(_)
            | ServerControlMessage::PasswordChallenge { .. } => None,
        })
        .collect()
}

// N1: a join seed's loadout is checked against the host's own mod. Parts the
// mod lacks drop and over-range values clamp, each with a warning naming what
// and why; a loadout the mod fully defines restores whole, silently.
#[test]
fn join_seed_loadout_is_checked_against_host_mod() {
    const CHECKED: u64 = 51;
    const VALID: u64 = 52;
    let capture = LogCapture::start();
    let (mut relay, gear, _) = hosting_dev_level();
    let (weapon, ammo, magazine) = gear.weapons[0].clone();

    let unchecked = CarriedLoadoutWire {
        health_centi: Some((gear.health_max * 300.0) as i32),
        life_state: Some(gear.non_initial_life_state.clone()),
        reserve: vec![(ammo.clone(), 7), ("not-an-ammo-type".into(), 3)],
        wieldables: vec![Some(weapon.clone()), Some("not-a-weapon".into())],
        magazines: vec![Some(magazine + 5), Some(1)],
        active_slot: 1,
        layers: vec![(gear.layer.clone(), 500), ("not-a-layer".into(), 100)],
    };
    let pawn = join_with(&mut relay, CHECKED, CHECKED, unchecked);

    let mut expected = CarriedState {
        health_current: Some(gear.health_max),
        layers: vec![(gear.layer.clone(), 5.0)],
        ..CarriedState::default()
    };
    expected.wieldables[0] = Some(weapon.clone());
    expected.magazines[0] = Some(magazine);
    expected.reserve.set_exact(&ammo, 7);
    assert_eq!(carry(&relay, CHECKED), Some(expected), "the checked carry");
    // The pawn spawned with the checked carry.
    assert_eq!(weapon_in(&relay, pawn, 0).as_deref(), Some(weapon.as_str()));
    assert_eq!(weapon_in(&relay, pawn, 1), None);
    assert!((health(&relay, pawn) - gear.health_max).abs() < 1e-4);
    {
        let ctx = script_ctx(&relay.app);
        let registry = ctx.registry.borrow();
        let reserve = registry.get_component::<AmmoReserve>(pawn).unwrap();
        assert_eq!(reserve.available(&ammo), 7);
        assert_eq!(reserve.available("not-an-ammo-type"), 0);
        let inventory = registry.get_component::<Inventory>(pawn).unwrap();
        assert_eq!(inventory.active_slot, 0);
        let held = inventory.wieldables[0].unwrap();
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(held)
                .unwrap()
                .magazine,
            magazine
        );
    }
    let seat = relay.seat(CHECKED).unwrap().0;
    for warning in [
        "dropped ammo `not-an-ammo-type`".to_string(),
        "dropped weapon `not-a-weapon` in slot 1".to_string(),
        format!(
            "clamped weapon `{weapon}` magazine in slot 0 from {} to its magazine size {magazine}",
            magazine + 5
        ),
        format!(
            "clamped health from {} to the player's maximum {}",
            gear.health_max * 3.0,
            gear.health_max
        ),
        format!(
            "life state `{}` arrives as the initial state",
            gear.non_initial_life_state
        ),
        "dropped defense layer `not-a-layer`".to_string(),
        "active slot 1 holds no kept weapon; selecting slot 0".to_string(),
    ] {
        capture.assert_logged_once(Level::Warn, &format!("seat {seat} join loadout: {warning}"));
    }

    let valid = valid_loadout(&gear, 61.5);
    join_with(&mut relay, VALID, VALID, valid.clone());
    let mut warnings = Vec::new();
    assert_eq!(
        carry(&relay, VALID),
        crate::netcode::carried_from_wire(&valid, &mut warnings),
        "a loadout the host's mod fully defines restores whole"
    );
    let valid_seat = relay.seat(VALID).unwrap().0;
    capture.assert_not_logged(Level::Warn, &format!("seat {valid_seat} join loadout"));
}

// N2: a guest that drops mid-level and rejoins within the hold spawns with
// the held seat's live loadout, not the one its join seed carries.
#[test]
fn rejoin_within_hold_keeps_held_loadout() {
    const FIRST: u64 = 61;
    const REJOIN: u64 = 62;
    let (mut relay, gear, _) = hosting_dev_level();
    let pawn = join_with(&mut relay, FIRST, FIRST, valid_loadout(&gear, 80.0));
    let seat = relay.seat(FIRST).unwrap();
    // Play changed the pawn since its spawn.
    postretro_entities::components::health::set_health_absolute(
        &mut script_ctx(&relay.app).registry.borrow_mut(),
        pawn,
        42.0,
    );

    relay.disconnect(FIRST);
    assert_eq!(
        relay
            .seats()
            .carried_state(seat)
            .and_then(|carry| carry.health_current),
        Some(42.0),
        "the hold harvested the live pawn"
    );

    // The rejoin's seed names other gear: its file's, saved before the drop.
    let (other_weapon, _, _) = gear.weapons[1].clone();
    let file = CarriedLoadoutWire {
        health_centi: Some(9_900),
        wieldables: vec![Some(other_weapon)],
        ..CarriedLoadoutWire::default()
    };
    let rejoined = join_with(&mut relay, REJOIN, FIRST, file);

    assert_eq!(
        relay.seat(REJOIN),
        Some(seat),
        "the rejoin reclaims its held seat"
    );
    assert!((health(&relay, rejoined) - 42.0).abs() < 1e-4);
    assert_eq!(
        weapon_in(&relay, rejoined, 0).as_deref(),
        Some(gear.weapons[0].0.as_str()),
        "the held seat's gear, not the seed's"
    );
}

/// Move the host pawn's weapon in `from` into `pawn`'s empty `to`: a drop and
/// a pickup through the inventory chokepoints. Returns its canonical name.
fn pick_up_host_weapon(
    relay: &Relay,
    host_pawn: EntityId,
    from: usize,
    pawn: EntityId,
    to: usize,
) -> String {
    let ctx = script_ctx(&relay.app);
    let mut registry = ctx.registry.borrow_mut();
    let item = postretro_sim::scripting::builtins::release_wieldable_for_test(
        &mut registry,
        host_pawn,
        from,
    )
    .expect("fixture: the host's pawn holds a weapon there");
    assert!(
        postretro_sim::scripting::builtins::acquire_wieldable_at_for_test(
            &mut registry,
            pawn,
            to,
            item
        )
    );
    registry
        .get_component::<DescriptorProvenance>(item)
        .unwrap()
        .canonical_name
        .clone()
}

// N3, host side (P3): the final character holds gear picked up after the last
// level start, whether the host ends the session or the guest asks for it at
// its own leave.
#[test]
fn final_character_includes_gear_since_last_level_start() {
    const LEAVING: u64 = 71;
    const STAYING: u64 = 72;
    const PICKUP_SLOT: usize = 3;
    let (mut relay, gear, host_pawn) = hosting_dev_level();
    let leaving_pawn = join_with(&mut relay, LEAVING, LEAVING, valid_loadout(&gear, 70.0));
    let staying_pawn = join_with(&mut relay, STAYING, STAYING, valid_loadout(&gear, 70.0));
    for client in [LEAVING, STAYING] {
        let _ = relay.forward(client);
    }
    let first_pickup = pick_up_host_weapon(&relay, host_pawn, 0, leaving_pawn, PICKUP_SLOT);
    let second_pickup = pick_up_host_weapon(&relay, host_pawn, 1, staying_pawn, PICKUP_SLOT);
    for client in [LEAVING, STAYING] {
        assert_eq!(
            carry(&relay, client).unwrap().wieldables[PICKUP_SLOT],
            None,
            "fixture: the seat's carry predates the pickup"
        );
    }

    // The guest's own leave: it asks, and the host harvests and answers.
    assert!(relay.client(LEAVING).request_final_character());
    relay.pump_until(|relay| {
        relay
            .received(LEAVING)
            .iter()
            .any(|message| matches!(message, ServerControlMessage::FinalCharacter { .. }))
    });
    let answered = loadouts(&relay.forward(LEAVING));
    assert_eq!(answered.len(), 1, "one final character answers the request");
    assert_eq!(
        answered[0]
            .wieldables
            .get(PICKUP_SLOT)
            .cloned()
            .flatten()
            .as_deref(),
        Some(first_pickup.as_str())
    );
    assert_eq!(
        loadouts(&relay.forward(STAYING)),
        Vec::new(),
        "only the asking seat is answered"
    );

    // The host ends the session.
    relay.app.end_session();
    for _ in 0..16 {
        if !relay.step_closing() {
            break;
        }
    }
    let ended = loadouts(&relay.forward(STAYING));
    assert_eq!(ended.len(), 1, "the final character carries the loadout");
    assert_eq!(
        ended[0]
            .wieldables
            .get(PICKUP_SLOT)
            .cloned()
            .flatten()
            .as_deref(),
        Some(second_pickup.as_str())
    );
}

// N6: at a checkpoint and at a level start the host sends each guest its own
// seat's loadout, and no other guest's; no roster frame carries one.
#[test]
fn loadout_messages_reach_only_their_owner() {
    const ALPHA: u64 = 81;
    const BRAVO: u64 = 82;
    let (mut relay, gear, _) = hosting_dev_level();
    join_with(&mut relay, ALPHA, ALPHA, valid_loadout(&gear, 30.0));
    let mut bravo = valid_loadout(&gear, 90.0);
    bravo.wieldables = vec![None, Some(gear.weapons[1].0.clone())];
    bravo.magazines = vec![None, Some(1)];
    bravo.active_slot = 1;
    join_with(&mut relay, BRAVO, BRAVO, bravo);
    for client in [ALPHA, BRAVO] {
        let _ = relay.forward(client);
    }

    let checkpoint = postretro_entities::SystemReactionCommand::Checkpoint {
        name: "bridge".into(),
        dispatch_source: "named:test".into(),
        origin: postretro_entities::FireOrigin::EveryMachine(
            postretro_entities::MachineSource::OwnState,
        ),
    };
    script_ctx(&relay.app).system_commands.push(checkpoint);
    relay.app.dispatch_system_commands();
    relay.settle();
    let own = |relay: &Relay, client| {
        crate::netcode::carried_to_wire(relay.seats().carried_state(relay.seat(client).unwrap()))
            .unwrap()
    };
    let (alpha_own, bravo_own) = (own(&relay, ALPHA), own(&relay, BRAVO));
    assert_ne!(
        alpha_own, bravo_own,
        "fixture: the two seats carry different gear"
    );
    assert_eq!(loadouts(&relay.forward(ALPHA)), vec![alpha_own]);
    assert_eq!(loadouts(&relay.forward(BRAVO)), vec![bravo_own]);

    // A level start: the unload harvests, then each guest hears its own.
    script_ctx(&relay.app).data_registry.borrow_mut().maps.push(
        postretro_foundation::ModMapEntry {
            id: "next".into(),
            path: "maps/never-read.prl".into(),
            name: "Next".into(),
            tags: Vec::new(),
            loading_tree: Vec::new(),
        },
    );
    relay
        .app
        .enqueue_level_request(LevelRequest::Load(LevelSource::Catalog("next".into())));
    relay.app.drain_level_requests();
    relay.world_less = true;
    relay.settle();
    let (alpha_own, bravo_own) = (own(&relay, ALPHA), own(&relay, BRAVO));
    assert_eq!(loadouts(&relay.forward(ALPHA)), vec![alpha_own]);
    assert_eq!(loadouts(&relay.forward(BRAVO)), vec![bravo_own]);
}

/// A client app whose save directory holds the characters `names` in slots
/// 0.., all from one device; each one's claim is what it joins with.
fn device_characters(dir: &std::path::Path, names: &[&str]) -> Vec<ConnectClaim> {
    use crate::session::saves::test_fixtures::{app_with_store, boot};
    let mut app = app_with_store(dir);
    boot(
        &mut app,
        Some(SavesManifest {
            character_slots: names.len(),
            run_slots: 1,
        }),
    );
    let session = app.session.as_mut().unwrap();
    names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            assert!(session.saves.create_character(index, (*name).to_string()));
            session.saves.select_character(Some(index));
            let (id, name) = session.character_claim();
            // As `net_entry::enter_role` builds the connection claim.
            ConnectClaim {
                player_id: PlayerClaimId(id.expect("a selected character has an id")),
                display_name: name,
            }
        })
        .collect()
}

// N7: a joiner's roster entry shows its character's name, truncated at the
// claim's byte cap on a character boundary.
#[test]
fn roster_shows_character_name_truncated() {
    const CLIENT: u64 = 91;
    let dir = tempfile::tempdir().unwrap();
    // A multi-byte character straddles the cap.
    let long_name = format!(
        "{}é{}",
        "a".repeat(DISPLAY_NAME_MAX_BYTES - 1),
        "b".repeat(20)
    );
    let claim = device_characters(dir.path(), &[&long_name]).remove(0);
    assert_eq!(
        claim.display_name, long_name,
        "the claim names the character"
    );

    let mut relay = Relay::host();
    relay.connect_with_claim(CLIENT, claim);
    relay.pump_until(|relay| relay.seat(CLIENT).is_some());
    relay.settle();
    let expected = "a".repeat(DISPLAY_NAME_MAX_BYTES - 1);
    let seat = relay.seat(CLIENT).unwrap();
    let names: Vec<String> = relay
        .forward(CLIENT)
        .into_iter()
        .filter_map(|message| match message {
            ServerControlMessage::SessionRoster(roster) => roster
                .entries
                .into_iter()
                .find(|entry| entry.seat == seat.0)
                .map(|entry| entry.name),
            _ => None,
        })
        .collect();
    assert!(!names.is_empty(), "the joiner received the roster");
    assert!(names.iter().all(|name| *name == expected), "{names:?}");
}

// N9: two characters from one device join a host in turn; each reclaims
// only its own held seat.
#[test]
fn two_characters_one_device_reclaim_own_seats() {
    let dir = tempfile::tempdir().unwrap();
    let claims = device_characters(dir.path(), &["Ash", "Birch"]);
    assert_ne!(
        claims[0].player_id, claims[1].player_id,
        "each character has its own id"
    );

    let mut relay = Relay::host();
    fn seated(relay: &mut Relay, client_id: u64, claim: &ConnectClaim) -> Seat {
        relay.connect_with_claim(client_id, claim.clone());
        relay.pump_until(|relay| relay.seat(client_id).is_some());
        relay.seat(client_id).unwrap()
    }
    let ash = seated(&mut relay, 101, &claims[0]);
    relay.disconnect(101);
    let birch = seated(&mut relay, 102, &claims[1]);
    assert_ne!(
        birch, ash,
        "a second character never takes the first one's held seat"
    );
    relay.disconnect(102);

    assert_eq!(seated(&mut relay, 103, &claims[1]), birch);
    assert_eq!(seated(&mut relay, 104, &claims[0]), ash);
    let seats = relay.seats();
    assert_eq!(
        seats.claim(birch).map(|claim| claim.display_name.as_str()),
        Some("Birch")
    );
    assert_eq!(
        seats.claim(ash).map(|claim| claim.display_name.as_str()),
        Some("Ash")
    );
}
