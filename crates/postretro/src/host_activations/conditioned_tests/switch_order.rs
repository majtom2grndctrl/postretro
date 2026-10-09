// Weapon switches applied in client-tick order with retained starts and presses.
// See: context/lib/networking.md §Host input command queue · context/lib/testing_guide.md

use super::*;
use std::ops::Range;

const START: u32 = 1000;

impl Fixture {
    /// Drive client ticks `START..START + ticks` with `command(offset)`, then
    /// idle. Input commands stamped inside `stall` are withheld until it ends,
    /// as when a lost packet stalls the reliable Input stream; switch
    /// declarations still flow on Control, so a switch reaches the host ahead
    /// of the commands stamped before it.
    fn run_stalled(
        &mut self,
        ticks: u32,
        stall: Range<u32>,
        command: impl Fn(u32) -> sim::SimCommand,
    ) {
        let mut withheld = Vec::new();
        for offset in 0..ticks {
            let tick = START + offset;
            let mut command = command(offset);
            self.predict(tick, &mut command);
            if stall.contains(&offset) {
                withheld.push((tick, command));
            } else {
                for (tick, command) in withheld.drain(..) {
                    self.send_input(tick, &command);
                }
                self.send_input(tick, &command);
            }
            self.advance_projectiles();
            self.host_tick();
        }
        self.idle(START + ticks, 120);
    }

    /// Host tick on which the host pawn's active slot last became `slot`.
    fn host_switched_to(&self, slot: usize) -> u32 {
        self.host_active
            .windows(2)
            .rev()
            .find(|pair| pair[0].1 != slot && pair[1].1 == slot)
            .map(|pair| pair[1].0)
            .expect("the host applies the switch")
    }

    /// The host accepted every switch the client declared, and refused none.
    fn assert_switches_accepted(&self, count: u32) {
        let outcomes: Vec<(u32, bool)> = self
            .switch_outcomes
            .iter()
            .map(|&(_, id, accepted)| (id, accepted))
            .collect();
        let expected: Vec<(u32, bool)> = (0..count).map(|id| (id, true)).collect();
        assert_eq!(outcomes, expected, "every switch is accepted, in order");
    }
}

fn switch_to(slot: usize) -> sim::SimCommand {
    let mut command = neutral();
    command.select_slot = Some(slot);
    command
}

/// Stalls that hold the burst's start back until after the client switched:
/// one ending before the switch is made, so the switch reaches the host while
/// its burst is still firing, and one ending after it.
const BURST_STALLS: [Range<u32>; 2] = [8..12, 8..30];

// Regression: a switch rode Control and applied on arrival, so after a stall it
// cancelled the burst the client had already finished, and the host refused
// the HIT of each round it never minted.
#[test]
fn conditioned_burst_then_switch_inside_a_stall_completes_the_burst_then_switches() {
    const TAP: u32 = 10;
    // The burst's last round leaves at TAP + 6.
    const SWITCH: u32 = TAP + 7;
    for stall in BURST_STALLS {
        let mut fixture = Fixture::new(LinkConfig::perfect(), burst_rifle("press", 130.0));
        fixture.mirror_rejection_despawn = true;
        fixture.equip_second(&hitscan_rifle("press", 130.0));
        fixture.run_stalled(60, stall.clone(), |offset| match offset {
            TAP => stamped_start(START + TAP),
            SWITCH => switch_to(1),
            _ => neutral(),
        });
        let predicted: Vec<u8> = fixture
            .snapshots
            .iter()
            .map(|shot| shot.activation.shot_id.ordinal)
            .collect();
        assert_eq!(
            predicted,
            [0, 1, 2],
            "{stall:?}: the client fires its burst"
        );
        assert_host_fires_what_the_client_fired(&fixture);
        let last_round = fixture.authorized.last().unwrap().fire_tick;
        assert!(
            fixture.host_switched_to(1) > last_round,
            "{stall:?}: the switch applies after the burst's last round"
        );
        fixture.assert_switches_accepted(1);
    }
}

// Regression: a switch made after a charged release overtook the stalled
// release and cancelled the charge, so the host never fired the shot the
// client predicted.
#[test]
fn conditioned_charged_release_then_switch_inside_a_stall_fires_the_charge_then_switches() {
    const RELEASE: u32 = 40;
    const SWITCH: u32 = RELEASE + 2;
    let mut fixture = Fixture::new(LinkConfig::perfect(), descriptor(false, true, 1, "press"));
    fixture.mirror_rejection_despawn = true;
    fixture.equip_second(&hitscan_rifle("press", 130.0));
    let start = token(START, ActivationLane::Secondary);
    fixture.run_stalled(70, 35..60, |offset| match offset {
        0 => held(start.lane, true),
        RELEASE => {
            let mut release = neutral();
            release.activation.release = Some(ActivationRelease {
                token: start,
                release_tick: START + RELEASE,
            });
            release
        }
        SWITCH => switch_to(1),
        offset if offset < RELEASE => held(start.lane, false),
        _ => neutral(),
    });
    assert_eq!(fixture.accepted(start), 1);
    assert_eq!(
        fixture.snapshot_ticks,
        [START + RELEASE],
        "the client fires its charge on release"
    );
    assert!(
        !fixture.cancelled(start),
        "the switch never cancels the charge"
    );
    assert_host_fires_what_the_client_fired(&fixture);
    let charged_shot = fixture.authorized.last().unwrap().fire_tick;
    assert!(
        fixture.host_switched_to(1) > charged_shot,
        "the switch applies after the charged shot"
    );
    fixture.assert_switches_accepted(1);
}

/// A drop-capable weapon descriptor: touchable, so the touch stage can place it.
fn touchable(canonical_name: &str) -> postretro_entities::EntityTypeDescriptor {
    postretro_entities::EntityTypeDescriptor {
        faction: None,
        tolerance: None,
        canonical_name: Some(canonical_name.to_owned()),
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: None,
        touchable: Some(postretro_foundation::TouchableDescriptor {
            mode: postretro_foundation::TouchMode::Press,
            radius: 0.3,
        }),
        mesh: None,
        health: None,
        behavior: None,
    }
}

/// Slot 0 a hitscan rifle, slot 1 a second one the client holds; a floor under
/// the pawn's capsule (centred on its origin) that the touch stage can drop onto.
fn two_rifles_on_a_floor() -> (Fixture, EntityId) {
    // Just below the capsule: half height 0.8 plus radius 0.4.
    const FLOOR: f32 = -1.25;
    let mut fixture = Fixture::new(LinkConfig::perfect(), hitscan_rifle("press", 130.0));
    fixture.mirror_rejection_despawn = true;
    fixture.world = crate::collision::CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(-100.0, FLOOR, -100.0),
            Vec3::new(100.0, FLOOR, -100.0),
            Vec3::new(100.0, FLOOR, 100.0),
            Vec3::new(-100.0, FLOOR, 100.0),
        ],
        vec![[0, 1, 2], [0, 2, 3]],
    );
    fixture.descriptors = vec![touchable("conditioned-second")];
    let second = fixture.equip_second(&hitscan_rifle("press", 130.0));
    // Hold slot 1 on a clean link before the stall.
    fixture.step(START - 20, switch_to(1));
    fixture.idle(START - 19, 19);
    assert_eq!(fixture.host_active.last().unwrap().1, 1);
    (fixture, second)
}

const PRESS: u32 = 10;
const SWITCH_BACK: u32 = PRESS + 2;
const FIRE: u32 = SWITCH_BACK + 2;

/// Press on slot 1, switch to slot 0, fire slot 0: all inside one stall.
fn press_switch_fire(fixture: &mut Fixture, press: impl Fn(&mut sim::SimCommand)) {
    fixture.run_stalled(40, 8..30, |offset| match offset {
        PRESS => {
            let mut command = neutral();
            press(&mut command);
            command
        }
        SWITCH_BACK => switch_to(0),
        FIRE => stamped_start(START + FIRE),
        _ => neutral(),
    });
    let fired: Vec<EntityId> = fixture.authorized.iter().map(|shot| shot.weapon).collect();
    assert_eq!(
        fired,
        [fixture.host_actors.weapon],
        "slot 0 fires its one shot"
    );
    assert_host_fires_what_the_client_fired(fixture);
    assert!(
        fixture
            .outcomes
            .iter()
            .all(|outcome| !matches!(outcome, wire::ActivationOutcome::InitiationRejected { .. })),
        "nothing is refused"
    );
    fixture.assert_switches_accepted(2);
}

// Regression: a switch overtook a drop pressed before it, so the touch stage,
// which drops the active slot, dropped the switched-to weapon instead.
#[test]
fn conditioned_drop_then_switch_then_fire_inside_a_stall_drops_the_pressed_slot() {
    let (mut fixture, second) = two_rifles_on_a_floor();
    press_switch_fire(&mut fixture, |command| command.movement.drop_pressed = true);
    let inventory = fixture
        .host
        .borrow()
        .get_component::<Inventory>(fixture.host_actors.pawn)
        .unwrap()
        .clone();
    assert_eq!(
        inventory.wieldables[1], None,
        "slot 1's weapon is the one dropped"
    );
    assert_eq!(inventory.wieldables[0], Some(fixture.host_actors.weapon));
    assert!(
        fixture
            .host
            .borrow()
            .get_component::<postretro_entities::components::touchable::TouchableComponent>(second)
            .is_ok(),
        "the dropped weapon lies in the world"
    );
    assert_eq!(inventory.active_slot, 0);
}

/// A rifle with a small magazine and a 50 ms reload from the pawn's reserve.
fn reloading_rifle() -> WeaponDescriptor {
    serde_json::from_value::<WeaponDescriptor>(json!({
        "damage": 10, "range": 96, "resolution": "hitscan",
        "primary": { "trigger": "press", "recoveryMs": 130, "steps": [{ "kind": "shot" }] },
        "resource": { "kind": "ammo", "type": "rounds", "magazine": 4, "reserve": 0,
            "reloadMs": 50 },
    }))
    .expect("reloading rifle fixture parses")
    .validate()
    .expect("reloading rifle fixture validates")
}

// The same for a reload: the press reaches slot 1's weapon, and the switch
// away from it waits for the press.
#[test]
fn conditioned_reload_then_switch_then_fire_inside_a_stall_reloads_the_pressed_slot() {
    let mut fixture = Fixture::new(LinkConfig::perfect(), hitscan_rifle("press", 130.0));
    fixture.mirror_rejection_despawn = true;
    let second = fixture.equip_second(&reloading_rifle());
    {
        let mut host = fixture.host.borrow_mut();
        host.set_component(
            fixture.host_actors.pawn,
            postretro_entities::AmmoReserve::new(),
        )
        .expect("host pawn takes an ammo reserve");
        // Post-spawn reserve writes route through the grant chokepoint.
        postretro_entities::components::grant::grant_ammo(
            &mut host,
            fixture.host_actors.pawn,
            "rounds",
            100.0,
        );
    }
    // On a clean link: hold slot 1 and spend a round so its reload has work.
    fixture.step(START - 30, switch_to(1));
    fixture.idle(START - 29, 9);
    fixture.step(START - 20, stamped_start(START - 20));
    fixture.idle(START - 19, 19);
    assert_eq!(fixture.host_active.last().unwrap().1, 1);
    assert_eq!(fixture.authorized.len(), 1, "slot 1 spends a round");
    fixture.authorized.clear();
    fixture.snapshots.clear();
    press_switch_fire(&mut fixture, |command| command.reload = true);
    let started: Vec<(u32, EntityId)> = fixture
        .reloads
        .iter()
        .filter(|(_, _, outcome)| *outcome == sim::ReloadOutcome::Started)
        .map(|&(tick, weapon, _)| (tick, weapon))
        .collect();
    let [(reloaded_at, reloaded)] = started.as_slice() else {
        panic!("one reload starts: {:?}", fixture.reloads);
    };
    assert_eq!(*reloaded, second, "slot 1's weapon reloads");
    assert!(
        *reloaded_at < fixture.host_switched_to(0),
        "the reload reaches slot 1 before the switch away from it"
    );
}

// On a clean link a switch adds no latency: it applies on the host tick that
// resolves the command it was made on.
#[test]
fn conditioned_switch_with_nothing_older_pending_applies_on_its_own_tick() {
    const SWITCH: u32 = 10;
    let mut fixture = Fixture::new(LinkConfig::perfect(), hitscan_rifle("press", 130.0));
    fixture.equip_second(&hitscan_rifle("press", 130.0));
    fixture.run_stalled(30, 0..0, |offset| match offset {
        SWITCH => switch_to(1),
        _ => neutral(),
    });
    let resolved_at = fixture
        .resolved_commands
        .iter()
        .find(|(_, resolved)| resolved.client_tick == START + SWITCH)
        .map(|(tick, _)| *tick)
        .expect("the switch's command resolves");
    assert_eq!(fixture.host_switched_to(1), resolved_at);
    fixture.assert_switches_accepted(1);
}

// A switch stamped further ahead of the client's newest command than any
// legitimate lead is refused, in order, and never touches the inventory.
#[test]
fn conditioned_switch_with_a_malformed_tick_is_refused_without_switching() {
    let mut fixture = Fixture::new(LinkConfig::perfect(), hitscan_rifle("press", 130.0));
    fixture.equip_second(&hitscan_rifle("press", 130.0));
    fixture.idle(START, 10);
    let newest = START + 9;
    for (declaration_id, slot, client_tick) in [(0, 1, newest + 1), (1, 0, newest + 100_000)] {
        fixture
            .client
            .send_switch_declaration(wire::ClientSwitchDeclaration {
                declaration_id,
                slot,
                client_tick,
            });
    }
    fixture.idle(START + 10, 30);
    let outcomes: Vec<(u32, bool)> = fixture
        .switch_outcomes
        .iter()
        .map(|&(_, id, accepted)| (id, accepted))
        .collect();
    assert_eq!(outcomes, [(0, true), (1, false)]);
    assert_eq!(
        fixture.host_active.last().unwrap().1,
        1,
        "the well-formed switch applies; the malformed one leaves slot 1 held"
    );
}
