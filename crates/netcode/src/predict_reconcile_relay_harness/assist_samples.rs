//! The receive loop's (acknowledged tick, `player.assistProgress`) list on
//! `ClientApplyFrameOutcome`, through the production relay.
use super::super::predict_reconcile_harness_test_fixtures::idle_command;
use super::*;
use postretro_foundation::PlayerMovementComponent;
use std::cell::RefCell;
use std::rc::Rc;

/// Host ticks after which the host stops changing the pawn's progress, so
/// the channel omits the unchanged slot from later batches.
const RAMP_TICKS: u32 = 120;

/// Each sample pairs the input tick its snapshot acknowledged with the
/// progress the host wrote on the tick that processed that input; batches that
/// omit the unchanged slot still sample its committed value (O30's netcode
/// half).
#[test]
fn relay_assist_samples_pair_each_acknowledged_tick_with_the_host_progress() {
    let link = LinkConfig {
        delay: 50,
        ..LinkConfig::perfect()
    };
    let mut h = RelayHarness::new(
        link,
        LinkConfig {
            seed: link.seed ^ 1,
            ..link
        },
    );
    h.join();
    let written: Rc<RefCell<Vec<(u32, f32)>>> = Rc::default();
    let log = Rc::clone(&written);
    let start = h.host_tick;
    h.post_movement = Some(Box::new(move |registry, pawn, tick| {
        let mut movement = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .clone();
        if tick < start + RAMP_TICKS {
            movement.assist_progress = (tick - start + 1) as f32 / 256.0;
            registry.set_component(pawn, movement.clone()).unwrap();
        }
        log.borrow_mut().push((tick, movement.assist_progress));
    }));
    let first_sample = h.assist_samples.len();
    for _ in 0..(RAMP_TICKS + 120) {
        h.frame(&idle_command());
    }
    let samples = &h.assist_samples[first_sample..];
    assert!(samples.len() > 60, "a sample per accepted snapshot");
    assert!(
        samples
            .windows(2)
            .all(|pair| pair[0].acked_tick <= pair[1].acked_tick),
        "samples arrive in acknowledged-tick order"
    );
    let written = written.borrow();
    for sample in samples {
        let matches = h.host_trace.iter().any(|trace| {
            trace
                .resolved
                .as_ref()
                .is_some_and(|resolved| resolved.client_tick == sample.acked_tick)
                && written
                    .iter()
                    .any(|(tick, value)| *tick == trace.server_tick && *value == sample.progress)
        });
        assert!(
            matches || sample.progress == 0.0,
            "sample {sample:?} is the value written on a tick that processed its input"
        );
    }
    let last = (RAMP_TICKS as f32) / 256.0;
    let settled = samples
        .iter()
        .filter(|sample| sample.progress == last)
        .count();
    assert!(
        settled > 30,
        "batches omitting the unchanged slot still sample it: {settled}"
    );
    assert_eq!(samples.last().unwrap().progress, last);
}
