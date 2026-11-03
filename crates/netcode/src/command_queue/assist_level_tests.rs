// The `assist` level on the host command queue: real, gap-held and neutral
// resolutions, and the wire round trip it rides.
// See: context/lib/networking.md §Host input command queue · context/lib/input.md §2

use super::*;
use crate::netcode::wire_convert::{input_command_to_sim, sim_command_to_input};

const CLIENT: u64 = 7;

fn command(client_tick: u32, assist_held: bool, use_pressed: bool) -> InputCommand {
    let mut command = sim_command_to_input(&neutral_sim_command(0.0), client_tick);
    command.movement.wish_dir = [0.0, 1.0];
    command.movement.assist_held = assist_held;
    command.movement.use_pressed = use_pressed;
    command
}

#[test]
fn assist_held_round_trips_through_the_wire_command() {
    for held in [false, true] {
        let mut sim = neutral_sim_command(0.0);
        sim.assist_held = held;
        let wire = sim_command_to_input(&sim, 4);
        assert_eq!(wire.movement.assist_held, held);
        assert_eq!(input_command_to_sim(&wire).assist_held, held);
    }
}

#[test]
fn a_neutral_command_holds_no_assist() {
    assert!(!neutral_sim_command(0.25).assist_held);
}

#[test]
fn held_gap_carries_assist_held_and_still_clears_use_pressed() {
    let held = held_gap_sim_command(&command(9, true, true));
    assert!(held.assist_held, "a level carries through a gap");
    assert!(
        !held.use_pressed && !held.movement.use_pressed,
        "a gap never synthesizes a Use edge"
    );
    assert!(!held_gap_sim_command(&command(9, false, false)).assist_held);
}

#[test]
fn queue_carries_assist_through_the_hold_grace_then_neutral_drops_it() {
    let mut queues = HostCommandQueues::new();
    // Two real commands disarm the buildup latch; the stream then stops.
    assert!(queues.ingest(CLIENT, &command(0, true, false)));
    assert!(queues.ingest(CLIENT, &command(1, true, true)));
    let real = (0..4)
        .map(|_| queues.resolve_tick(CLIENT).expect("resolution"))
        .find(|resolved| resolved.client_tick == 1)
        .expect("the second real command resolves");
    assert_eq!(real.source, ResolutionSource::Real);
    assert!(real.command.assist_held && real.command.use_pressed);

    let mut sources = Vec::new();
    for _ in 0..INPUT_HOLD_TICKS + 2 {
        let resolved = queues.resolve_tick(CLIENT).expect("gap resolution");
        match resolved.source {
            ResolutionSource::Held => assert!(
                resolved.command.assist_held && !resolved.command.use_pressed,
                "a held tick keeps the level and never repeats Use"
            ),
            ResolutionSource::Neutral => assert!(!resolved.command.assist_held),
            ResolutionSource::Real => panic!("nothing real was sent"),
        }
        sources.push(resolved.source);
    }
    assert!(sources.contains(&ResolutionSource::Held));
    assert!(sources.contains(&ResolutionSource::Neutral));
}

#[test]
fn a_real_release_command_carries_assist_held_false() {
    let mut queues = HostCommandQueues::new();
    assert!(queues.ingest(CLIENT, &command(0, true, false)));
    assert!(queues.ingest(CLIENT, &command(1, true, false)));
    assert!(queues.ingest(CLIENT, &command(2, false, false)));
    let resolved: Vec<_> = (0..3)
        .map(|_| queues.resolve_tick(CLIENT).unwrap())
        .collect();
    let release = resolved
        .iter()
        .find(|resolved| resolved.client_tick == 2)
        .expect("the release resolves");
    assert_eq!(release.source, ResolutionSource::Real);
    assert!(!release.command.assist_held);
    assert!(
        resolved
            .iter()
            .filter(|resolved| resolved.client_tick < 2)
            .all(|resolved| resolved.command.assist_held)
    );
}
