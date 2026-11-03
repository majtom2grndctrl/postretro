// A suspension cancel latched on a frame with no fixed tick rides the next
// ticked command: it reaches the host with the same semantics and takes no
// client tick of its own.
// See: context/lib/networking.md §Combat authority · §Host input command queue

use super::*;
use crate::input::{Action, ActionSnapshot, ButtonState, GameplayInputLatch};

const START: u32 = 800;

/// The client's side of the frame loop's input capture, through the
/// production latch: a capturing frame suspends local execution and hands its
/// token to the latch; each fixed tick reads the active token off the
/// predicted weapon and takes the latch's activation edges; a frame with no
/// fixed tick builds and sends nothing.
struct Client {
    latch: GameplayInputLatch,
    next_tick: u32,
}

impl Client {
    fn new() -> Self {
        Self {
            latch: GameplayInputLatch::new(),
            next_tick: START,
        }
    }

    fn frame(&mut self, fixture: &mut Fixture, alt_fire: ButtonState, ticks: u32, captures: bool) {
        if captures {
            fixture.frame.suspend(&fixture.local.borrow());
            let token = fixture.frame.suppressed.map(|(_, token)| token);
            self.latch.activation.set_active(token);
        }
        let buttons = ActionSnapshot::with_button_state(Action::AltFire, alt_fire);
        let sent = fixture.sent.len();
        let Some(snapshot) =
            crate::gameplay_snapshot_for_capture_state(&mut self.latch, &buttons, ticks, captures)
        else {
            assert_eq!(ticks, 0);
            assert_eq!(fixture.sent.len(), sent, "a zero-tick frame sends nothing");
            return;
        };
        for _ in 0..ticks {
            let tick = self.next_tick;
            self.next_tick += 1;
            let state = snapshot.button(Action::AltFire);
            let mut command = if state.is_active() {
                held(ActivationLane::Secondary, state == ButtonState::Pressed)
            } else {
                neutral()
            };
            let active = fixture
                .local
                .borrow()
                .get_component::<WeaponComponent>(fixture.local_actors.weapon)
                .ok()
                .and_then(|weapon| weapon.state.activation_cursor().map(|cursor| cursor.token));
            self.latch.activation.set_active(active);
            command.activation = self.latch.activation.command(tick);
            fixture.step(tick, command);
        }
    }

    /// Charge the secondary lane: press on one fixed tick, then hold across
    /// a 144 Hz frame cadence (most frames run no tick) until the host charges.
    fn charge(&mut self, fixture: &mut Fixture) -> ActivationToken {
        let start = token(self.next_tick, ActivationLane::Secondary);
        self.frame(fixture, ButtonState::Pressed, 1, false);
        for ticks in [0, 1, 0, 0, 1].into_iter().cycle().take(60) {
            self.frame(fixture, ButtonState::Held, ticks, false);
        }
        assert_eq!(fixture.accepted(start), 1);
        assert!(fixture.host_charging());
        start
    }

    fn idle(&mut self, fixture: &mut Fixture, frames: u32) {
        for _ in 0..frames {
            self.frame(fixture, ButtonState::Inactive, 1, false);
        }
    }
}

/// Every command the client put on Input, by tick: one per fixed tick, none
/// skipped and none extra.
fn assert_one_command_per_fixed_tick(fixture: &Fixture, client: &Client) {
    let ticks: Vec<u32> = fixture.sent.iter().map(|input| input.client_tick).collect();
    assert_eq!(ticks, (START..client.next_tick).collect::<Vec<_>>());
}

/// The commands that carried `token`'s cancel, by client tick.
fn cancel_ticks(fixture: &Fixture, token: ActivationToken) -> Vec<u32> {
    fixture
        .sent
        .iter()
        .filter(|input| input.activation.cancel == Some(wire_token(token)))
        .map(|input| input.client_tick)
        .collect()
}

// Focus lost mid-charge on a frame with no fixed tick, the button released on
// that same frame: the cancel wins, rides the next tick's command alone, and
// the host cancels the charge without a shot.
#[test]
fn a_zero_tick_suspension_cancels_a_remote_charge_on_the_next_tick() {
    let mut fixture = Fixture::new(LinkConfig::perfect(), descriptor(false, true, 3, "press"));
    let mut client = Client::new();
    let start = client.charge(&mut fixture);

    client.frame(&mut fixture, ButtonState::Released, 0, true);
    let cancel_tick = client.next_tick;
    client.frame(&mut fixture, ButtonState::Inactive, 1, true);
    client.idle(&mut fixture, 120);

    assert_eq!(cancel_ticks(&fixture, start), vec![cancel_tick]);
    let carrier = &fixture.sent[(cancel_tick - START) as usize];
    assert!(carrier.activation.release.is_none(), "the cancel wins");
    assert_one_command_per_fixed_tick(&fixture, &client);
    assert!(fixture.cancelled(start));
    fixture.assert_no_host_shot();
    assert!(!fixture.host_charging());
}

// A charge released on one fixed tick, then focus lost on a frame with no
// fixed tick while its burst runs: the cancel, stamped with the next tick,
// waits behind the release, so the host mints exactly the rounds the client
// fired before it and no more.
#[test]
fn a_zero_tick_suspension_after_a_release_keeps_the_released_charge_shot() {
    let mut fixture = Fixture::new(LinkConfig::perfect(), descriptor(false, true, 3, "press"));
    let mut client = Client::new();
    let start = client.charge(&mut fixture);

    let release_tick = client.next_tick;
    client.frame(&mut fixture, ButtonState::Released, 1, false);
    client.frame(&mut fixture, ButtonState::Inactive, 0, true);
    client.frame(&mut fixture, ButtonState::Inactive, 1, true);
    client.idle(&mut fixture, 120);

    assert_eq!(cancel_ticks(&fixture, start), vec![release_tick + 1]);
    assert_one_command_per_fixed_tick(&fixture, &client);
    assert!(fixture.cancelled(start));
    assert_eq!(
        fixture.snapshots.len(),
        1,
        "the client fired the charged round"
    );
    assert_host_fires_what_the_client_fired(&fixture);
}
