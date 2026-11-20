//! Emissive volumes' deliberate differences from fog, across co-op
//! (emissive-volumes A12, co-op half): a brightness write clears a running
//! curve on every machine, a cleared-then-reinstalled curve is a new run on
//! host and client, and a `null` clear or a colour write carries what the host
//! presents. The local half is the sim primitive and bridge tests.
//! See: context/lib/networking.md §Presentation state · scripting.md §10.2a
//!
//! Pin map (`context/plans/in-progress/emissive-volumes/research.md`):
//! P1 and P31 `p31_*` · P30 `p30_*` · P32 `p32_*` · P5 and P33 `p33_*` · P44 `p44_*` ·
//! P6 on a finite run's last frame `p06_*`.
use super::parity::{Kind, RED, approx, fire_on, host_write, joined, parity_world, record};
use super::*;
use postretro_entities::presentation_overrides::PresentationComponent;

const SIGN: Kind = Kind::Emissive;

/// A 1 s looping ramp 0 → 1: a sample at `t` seconds into a run reads `t`
/// (mod 1), so a value names its run's start.
fn ramp() -> (&'static str, Value) {
    SIGN.animate(1000.0, None, &[0.0, 1.0])
}

fn ramp_at(clock: f64, start: f64) -> f32 {
    (clock - start).rem_euclid(1.0) as f32
}

/// The sign's value on the host and on peer A at the same clock.
fn both(h: &Harness) -> (f32, f32) {
    (SIGN.shown(&h.host).value, SIGN.shown(&h.peers[0].app).value)
}

// P30: one body clears a running flicker with a brightness write, then
// re-installs the same curve. The capture sees a new run, and host and client
// both restart from the first sample; fog's identical re-install would keep
// its start (`presentation_capture_tests` keeps that pin).
#[test]
fn p30_clear_and_identical_reinstall_in_one_body_restarts_on_host_and_client() {
    let mut h = joined(parity_world());
    host_write(&mut h, vec![SIGN.step(ramp())]);
    let first = h.clock;
    h.advance(24);
    let restart = h.clock;
    host_write(
        &mut h,
        vec![SIGN.step(SIGN.set_value(0.0)), SIGN.step(ramp())],
    );
    assert_eq!(record(&h.host, SIGN).unwrap().anim_start, Some(restart));
    approx(
        SIGN.shown(&h.host).value,
        0.0,
        "the host restarts at its first sample",
    );
    h.advance(12);
    let (host, client) = both(&h);
    approx(host, ramp_at(h.clock, restart), "host on the new run");
    assert!(
        (client - ramp_at(h.clock, restart)).abs() < 0.05,
        "client restarted: {client} vs new run {} (old run {})",
        ramp_at(h.clock, restart),
        ramp_at(h.clock, first)
    );
    assert!((ramp_at(h.clock, restart) - ramp_at(h.clock, first)).abs() > 0.3);
}

// P1 and P31: every machine runs its own `levelLoad` flicker; the host's
// host-only `powerCut` reaches the client as brightness 0 with no curve, so
// the client drops its curve and 0 holds on every machine.
#[test]
fn p31_host_power_cut_drops_every_machines_flicker_and_holds_zero() {
    let flicker = primitive(
        "levelLoad",
        "setEmissiveAnimation",
        Some(SIGN.tag()),
        ramp().1,
    );
    let mut h = Harness::with_shared_reactions(parity_world(), vec![flicker]);
    h.advance(40);
    assert!(h.server.is_participating(CLIENT_A));
    assert!(SIGN.shown(&h.host).animating && SIGN.shown(&h.peers[0].app).animating);
    host_write(&mut h, vec![SIGN.step(SIGN.set_value(0.0))]);
    let cut = record(&h.host, SIGN).unwrap();
    let PresentationComponent::Emissive(written) = &cut.component else {
        panic!("emissive record");
    };
    assert_eq!(
        (written.brightness, written.animation.as_ref()),
        (0.0, None)
    );
    h.advance(12);
    for _ in 0..30 {
        h.step();
        for app in [&h.host, &h.peers[0].app] {
            let shown = SIGN.shown(app);
            assert!(!shown.animating);
            approx(shown.value, 0.0, "0 holds");
        }
    }
}

// P32: a joiner after the host's `powerCut` installs the same `levelLoad`
// flicker, and its first accepted baseline clears it: the joiner holds 0, as
// the host presents.
#[test]
fn p32_late_joiner_baseline_replaces_its_own_level_load_flicker_with_the_hosts_hold() {
    let flicker = primitive(
        "levelLoad",
        "setEmissiveAnimation",
        Some(SIGN.tag()),
        ramp().1,
    );
    let mut h = Harness::with_shared_reactions(parity_world(), vec![flicker]);
    h.advance(40);
    host_write(&mut h, vec![SIGN.step(SIGN.set_value(0.0))]);
    h.advance(12);
    h.connect_late();
    assert!(
        SIGN.shown(&h.peers[1].app).animating,
        "the joiner's own levelLoad flicker runs"
    );
    h.advance(60);
    assert!(h.server.is_participating(CLIENT_B));
    for _ in 0..20 {
        h.step();
        let shown = SIGN.shown(&h.peers[1].app);
        assert!(!shown.animating);
        approx(shown.value, 0.0, "the joiner holds the host's 0");
    }
}

// P5 and P33: `setEmissiveAnimation` args `null` mid-flicker leaves the last
// sampled brightness; the record carries that sample (the scripted value,
// before any limiter), and the client holds it.
#[test]
fn p33_null_clear_holds_the_last_sample_and_the_wire_carries_it() {
    let mut h = joined(parity_world());
    host_write(&mut h, vec![SIGN.step(ramp())]);
    h.advance(20);
    let sampled = SIGN.shown(&h.host).value;
    assert!(sampled > 0.1, "mid-ramp: {sampled}");
    host_write(&mut h, vec![SIGN.step(SIGN.clear_animation())]);
    let cleared = record(&h.host, SIGN).unwrap();
    let PresentationComponent::Emissive(written) = &cleared.component else {
        panic!("emissive record");
    };
    assert_eq!(written.animation, None);
    approx(
        written.brightness,
        sampled,
        "the record carries the last sample",
    );
    h.advance(12);
    for app in [&h.host, &h.peers[0].app] {
        let shown = SIGN.shown(app);
        assert!(!shown.animating);
        approx(shown.value, sampled, "held");
    }
}

// P44: a colour write over a running flicker keeps the flicker's start and
// samples. Host and client present the same sample in the new colour.
#[test]
fn p44_colour_write_keeps_the_running_flicker_on_host_and_client() {
    let mut h = joined(parity_world());
    host_write(&mut h, vec![SIGN.step(ramp())]);
    let start = h.clock;
    h.advance(24);
    host_write(&mut h, vec![SIGN.step(SIGN.set_tint(RED))]);
    assert_eq!(record(&h.host, SIGN).unwrap().anim_start, Some(start));
    h.advance(12);
    let expected = ramp_at(h.clock, start);
    let (host, client) = both(&h);
    approx(host, expected, "the host's run continues");
    assert!(
        (client - expected).abs() < 0.05,
        "the client's run continues: {client} vs {expected}"
    );
    for app in [&h.host, &h.peers[0].app] {
        let shown = SIGN.shown(app);
        assert!(shown.animating);
        assert_eq!(shown.tint, RED);
    }
    // A client's own colour write leaves its run alone too.
    let clock = h.peers[0].clock;
    fire_on(
        &h.peers[0].app,
        clock,
        postretro_entities::FireOrigin::EveryMachine(
            postretro_entities::MachineSource::HostMirrored,
        ),
        vec![SIGN.step(SIGN.set_tint([0.0, 0.0, 1.0]))],
    );
    h.step();
    let client = SIGN.shown(&h.peers[0].app);
    assert!(client.animating);
    assert!((client.value - ramp_at(h.clock, start)).abs() < 0.05);
}

// P6 on the settle frame, across co-op: a host-only fade re-installed by a
// body that runs on the frame its one play runs out. The drain lands before
// the host's bridge would settle it, so the host restarts it there, and the
// record names that frame as the start: the client replays the fade too,
// rather than settling.
#[test]
fn p06_reinstall_on_a_fades_last_frame_restarts_on_host_and_client() {
    // Ends half a frame before the 30th frame after the install, so that
    // frame is the first the run has finished on.
    let period_ms = 29.5 * DT * 1000.0;
    let fade = || SIGN.animate(period_ms, Some(1), &[0.0, 1.0]);
    let at = |clock: f64, start: f64| ((clock - start) * 1000.0 / period_ms) as f32;
    let mut h = joined(parity_world());
    host_write(&mut h, vec![SIGN.step(fade())]);
    let first = h.clock;
    assert_eq!(record(&h.host, SIGN).unwrap().anim_start, Some(first));
    h.advance(29);
    assert!(SIGN.shown(&h.host).animating, "one frame short of its end");
    // The settle frame on the host: the body's drain, then the bridge tick.
    h.clock += DT;
    host_write(&mut h, vec![SIGN.step(fade())]);
    let restart = h.clock;
    assert_eq!(record(&h.host, SIGN).unwrap().anim_start, Some(restart));
    let host = SIGN.shown(&h.host);
    assert!(host.animating, "the host restarts instead of settling");
    approx(host.value, 0.0, "from the first sample");
    h.advance(12);
    let expected = at(h.clock, restart);
    let (host, client) = both(&h);
    approx(host, expected, "the host's new run");
    assert!(
        SIGN.shown(&h.peers[0].app).animating,
        "the client replays the fade"
    );
    assert!(
        (client - expected).abs() < 0.05,
        "client on the new run: {client} vs {expected}"
    );
}
