// A connected client whose pawn baseline arms before the host's tuning payload.
// See: context/lib/networking.md §What gates, and what replicates instead · §Combat authority

use super::receive_alternate::{ReceiveSetup, descriptor_named, dev_mod_descriptors};
use super::*;
use crate::input::{Action, ActionSnapshot, ButtonState, GameplayInputLatch};
use postretro_combat_model::CarriedState;
use postretro_entities::AmmoReserve;
use postretro_entities::components::inventory::WIELDABLE_SLOT_CAPACITY;

/// The first client tick: receive fixtures predict from tick zero.
const START: u32 = 0;
const HOST_LOADOUT: [&str; 2] = ["reference_pistol", "reference_rifle"];
/// The client's own player loadout, which the host's never names.
const CLIENT_OWN_WEAPON: &str = "reference_shotgun";
const CLIENT_OWN_AMMO: &str = "shells.buck";
/// The host restored the rifle after a level change.
const CARRIED_SLOT: usize = 1;

/// The client-side gates a production frame runs, in order: the selection
/// latch, the wieldable tick and its declaration, and the active-slot re-sync.
#[derive(Default)]
struct JoinInput {
    latch: GameplayInputLatch,
    resync: crate::client_weapon::ActiveSlotResync,
    resync_declarations: Vec<usize>,
}

impl Fixture {
    /// The host composes the pawn from a carried selection off the first slot
    /// and sends its tuning, which stays in flight while the pawn baseline arms
    /// on a client whose own player loadout differs.
    fn joining_with_carried_selection() -> Self {
        let (ctx, entities) = dev_mod_descriptors();
        let weapons: Vec<_> = HOST_LOADOUT
            .iter()
            .chain([&CLIENT_OWN_WEAPON])
            .map(|name| descriptor_named(&entities, name))
            .collect();
        let player = |loadout: &[&str]| {
            let mut player = descriptor_named(&entities, "player");
            player.inventory.as_mut().unwrap().loadout =
                loadout.iter().map(|name| (*name).to_string()).collect();
            player
        };
        let host_descriptors: Vec<_> = std::iter::once(player(&HOST_LOADOUT))
            .chain(weapons.iter().cloned())
            .collect();
        let client_descriptors: Vec<_> = std::iter::once(player(&[CLIENT_OWN_WEAPON]))
            .chain(weapons.iter().cloned())
            .collect();
        let carried = CarriedState {
            wieldables: std::array::from_fn(|slot| HOST_LOADOUT.get(slot).map(|n| n.to_string())),
            active_slot: CARRIED_SLOT,
            ..Default::default()
        };
        let fixture = Self::with_snapshot_receive(
            ctx,
            ReceiveSetup {
                host_descriptors,
                client_descriptors,
                carried: Some(carried),
                weapon: weapons[CARRIED_SLOT].weapon.clone().unwrap(),
                hold_tuning: true,
            },
        );
        assert_eq!(fixture.host_active_slot(), CARRIED_SLOT);
        fixture
    }

    fn host_active_slot(&self) -> usize {
        self.host
            .borrow()
            .get_component::<Inventory>(self.host_actors.pawn)
            .unwrap()
            .active_slot
    }

    fn client_inventory(&self) -> Option<Inventory> {
        self.local
            .borrow()
            .get_component::<Inventory>(self.local_actors.pawn)
            .ok()
            .cloned()
    }

    /// The canonical name in each of the client's slots.
    fn client_layout(&self) -> Vec<Option<String>> {
        let registry = self.local.borrow();
        let inventory = registry
            .get_component::<Inventory>(self.local_actors.pawn)
            .unwrap();
        inventory
            .wieldables
            .iter()
            .map(|weapon| {
                weapon.map(|weapon| {
                    registry
                        .get_component::<DescriptorProvenance>(weapon)
                        .unwrap()
                        .canonical_name
                        .clone()
                })
            })
            .collect()
    }

    /// No weapon instance from the client's own loadout exists, and no
    /// reserve of its ammo type.
    fn assert_no_client_own_loadout(&self, when: &str) {
        let registry = self.local.borrow();
        let own: Vec<EntityId> = registry
            .iter_with_kind(ComponentKind::Weapon)
            .map(|(id, _)| id)
            .filter(|id| {
                registry
                    .get_component::<DescriptorProvenance>(*id)
                    .is_ok_and(|provenance| provenance.canonical_name == CLIENT_OWN_WEAPON)
            })
            .collect();
        assert!(own.is_empty(), "{when}: own-loadout weapon {own:?}");
        let own_reserve = registry
            .get_component::<AmmoReserve>(self.local_actors.pawn)
            .map_or(0, |reserve| reserve.available(CLIENT_OWN_AMMO));
        assert_eq!(own_reserve, 0, "{when}: own-loadout reserve");
    }

    /// One client frame through the production gates, in frame order: advance
    /// the selection latch on the input-phase occupancy, drain Control (here,
    /// install the late payload when `install`), take the queued commit, run
    /// the wieldable tick on the local movement pawn and declare an accepted
    /// switch, then observe the re-sync. The frame's snapshot apply runs in
    /// `host_tick`'s transport, after the client tick rather than before it;
    /// one apply per frame either way. Returns whether this frame sent any
    /// switch declaration.
    fn join_frame(
        &mut self,
        input: &mut JoinInput,
        tick: u32,
        press: Option<Action>,
        install: bool,
    ) -> bool {
        let snapshot = press.map_or_else(ActionSnapshot::neutral, |action| {
            ActionSnapshot::with_button_state(action, ButtonState::Pressed)
        });
        let (occupied, active) = crate::local_wieldable_occupancy(&self.local.borrow());
        let selection = input.latch.wieldable_selection_mut();
        selection.advance_frame(
            &snapshot,
            &occupied,
            active,
            crate::input::WieldableSelectionPolicy {
                commit_on_direct_select: true,
                cycle_dwell_ms: 0.0,
            },
            DT * 1000.0,
        );
        if install {
            self.install_late_payload();
        }
        let (occupied, active) = crate::local_wieldable_occupancy(&self.local.borrow());
        let selection = input.latch.wieldable_selection_mut();
        let mut command = neutral();
        command.select_slot = selection.take_pending_commit(&occupied, active);
        command.input_tick = tick;

        let pawn = self.local.borrow().local_player_movement_pawn();
        let (switch_accepted, _) = sim::simulate_client_wieldable_tick(
            self.local.clone(),
            &self.world,
            &self.zones,
            pawn,
            false,
            command.select_slot,
            command.fire_button,
            command.reload,
            0.0,
            DT,
        );
        let switched = command.select_slot.filter(|_| switch_accepted);
        if let Some(slot) = switched {
            self.declare_switch(slot, tick);
        }
        // Observed after this frame's declaration, as production's re-sync
        // runs after `client_declare_switch`.
        let observation = pawn.and_then(|pawn| {
            crate::client_weapon::ActiveSlotResyncObservation::read(
                &self.local.borrow(),
                pawn,
                (self.next_switch_id as usize) > self.switch_outcomes.len(),
                self.projection.agreed_slot(),
            )
        });
        let resynced = match observation {
            Some(observation) => input.resync.observe(observation, DT),
            None => {
                input.resync.reset();
                None
            }
        };
        if let Some(slot) = resynced {
            input.resync_declarations.push(slot);
            self.declare_switch(slot, tick);
        }
        self.send_input(tick, &command);
        self.host_tick();
        switched.is_some() || resynced.is_some()
    }

    fn join_tick(&mut self, input: &mut JoinInput, tick: u32, press: Option<Action>) -> bool {
        self.join_frame(input, tick, press, false)
    }

    /// Drive `count` client frames from `START + first`, none pressing a key,
    /// asserting none declares a switch.
    fn join_idle(&mut self, input: &mut JoinInput, first: u32, count: u32) {
        for offset in first..first + count {
            assert!(
                !self.join_tick(input, START + offset, None),
                "tick {offset} declared a switch"
            );
        }
    }

    /// The payload is still held: the armed pawn holds nothing, from either
    /// loadout, while `count` frames run.
    fn wait_for_payload(&mut self, input: &mut JoinInput, count: u32, press: Option<Action>) {
        for offset in 0..count {
            let pressed = press.filter(|_| offset == count / 2);
            assert!(
                !self.join_tick(input, START + offset, pressed),
                "tick {offset} declared a switch before the payload"
            );
            assert!(
                self.client_inventory().is_none(),
                "tick {offset}: the armed pawn composed before its payload"
            );
            self.assert_no_client_own_loadout("before the payload");
        }
    }

    /// Install the held payload and check the pawn composed once, from the
    /// host's layout, active on the carried slot.
    fn install_late_payload(&mut self) {
        self.release_tuning();
        let expected: Vec<Option<String>> = (0..WIELDABLE_SLOT_CAPACITY)
            .map(|slot| HOST_LOADOUT.get(slot).map(|name| name.to_string()))
            .collect();
        assert_eq!(self.client_layout(), expected, "the host's slot layout");
        let inventory = self.client_inventory().unwrap();
        assert_eq!(inventory.active_slot, CARRIED_SLOT, "the carried slot");
        assert_eq!(inventory.spawn_active_slot, CARRIED_SLOT);
        assert_eq!(inventory.active_slot, self.host_active_slot());
        assert_eq!(
            self.local.borrow().local_player_movement_pawn(),
            Some(self.local_actors.pawn),
            "the payload's movement makes the pawn the selection gates' pawn"
        );
        self.assert_no_client_own_loadout("after the payload");
    }
}

/// Frames the payload stays in flight after the baseline arms.
const PAYLOAD_DELAY_TICKS: u32 = 30;
/// About a round trip plus a snapshot interval on the fixture's perfect link.
const ROUND_TRIP_TICKS: u32 = 12;

fn resync_window_ticks() -> u32 {
    (crate::client_weapon::ACTIVE_SLOT_RESYNC_WINDOW_SECS / DT) as u32
}

// A late payload after a level change that carried the rifle selection: the
// armed pawn holds nothing until the payload installs, then composes once
// from the host's layout on the carried slot, never from the client's own
// shotgun loadout. Nothing declares a switch, the re-sync included.
#[test]
fn conditioned_late_payload_join_composes_the_carried_slot_once_from_the_host_layout() {
    let mut fixture = Fixture::joining_with_carried_selection();
    let mut input = JoinInput::default();
    assert!(fixture.client_inventory().is_none(), "armed, payload held");
    fixture.assert_no_client_own_loadout("at the arm");

    fixture.wait_for_payload(&mut input, PAYLOAD_DELAY_TICKS, None);
    assert!(!fixture.join_frame(&mut input, START + PAYLOAD_DELAY_TICKS, None, true));
    let composed = fixture.client_inventory().unwrap().wieldables;

    fixture.join_idle(&mut input, PAYLOAD_DELAY_TICKS + 1, ROUND_TRIP_TICKS);
    assert_eq!(
        fixture.projection.agreed_slot(),
        Some(CARRIED_SLOT),
        "the host's projection names the client's slot, so the re-sync below \
         watches a populated projection"
    );
    fixture.join_idle(
        &mut input,
        PAYLOAD_DELAY_TICKS + 1 + ROUND_TRIP_TICKS,
        2 * resync_window_ticks(),
    );

    assert!(input.resync_declarations.is_empty());
    assert_eq!(fixture.next_switch_id, 0, "no switch declaration was sent");
    assert!(fixture.switch_outcomes.is_empty(), "the host answered none");
    assert_eq!(fixture.host_active_slot(), CARRIED_SLOT);
    let inventory = fixture.client_inventory().unwrap();
    assert_eq!(inventory.active_slot, CARRIED_SLOT);
    assert_eq!(inventory.wieldables, composed, "composed once, never again");
    fixture.assert_no_client_own_loadout("after the join");
}

// The same join, with a slot key pressed while the payload was in flight and
// another on the install frame: neither switches nor declares, so the client
// still lands on the carried slot with no re-sync. The first key pressed
// after the install reaches the host as an ordinary switch declaration.
#[test]
fn conditioned_late_payload_join_drops_a_slot_key_pressed_in_the_window() {
    let mut fixture = Fixture::joining_with_carried_selection();
    let mut input = JoinInput::default();
    fixture.wait_for_payload(
        &mut input,
        PAYLOAD_DELAY_TICKS,
        Some(Action::SelectWieldable1),
    );
    // Selection advances before the frame's Control drain, so a key pressed
    // on the install frame reads the inventory as it stood: empty.
    assert!(!fixture.join_frame(
        &mut input,
        START + PAYLOAD_DELAY_TICKS,
        Some(Action::SelectWieldable1),
        true,
    ));
    let settle = ROUND_TRIP_TICKS + 2 * resync_window_ticks();
    fixture.join_idle(&mut input, PAYLOAD_DELAY_TICKS + 1, settle);
    assert!(input.resync_declarations.is_empty());
    assert_eq!(fixture.next_switch_id, 0, "no switch declaration was sent");
    assert_eq!(fixture.host_active_slot(), CARRIED_SLOT);
    assert_eq!(
        fixture.client_inventory().unwrap().active_slot,
        CARRIED_SLOT
    );

    // The first key after the install: an ordinary switch to the pistol.
    let after = PAYLOAD_DELAY_TICKS + 1 + settle;
    assert!(fixture.join_tick(&mut input, START + after, Some(Action::SelectWieldable1)));
    fixture.join_idle(&mut input, after + 1, 60);
    assert_eq!(
        fixture
            .switch_outcomes
            .iter()
            .map(|&(_, id, accepted)| (id, accepted))
            .collect::<Vec<_>>(),
        [(0, true)],
        "one ordinary declaration, accepted"
    );
    assert!(input.resync_declarations.is_empty());
    assert_eq!(fixture.host_active_slot(), 0);
    assert_eq!(fixture.client_inventory().unwrap().active_slot, 0);
}
