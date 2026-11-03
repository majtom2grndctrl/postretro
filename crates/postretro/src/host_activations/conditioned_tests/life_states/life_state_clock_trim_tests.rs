//! The life-state HUD clock across a host catch-up trim, snapshot ack/replay and frozen UI projection.
//! See: context/lib/networking.md §What gates, and what replicates instead · §Host input command queue

use super::*;
use crate::life_state_clock::predicted_target_ms;
use crate::life_state_display::{LifeStateDisplay, UiProjection};
use postretro_entities::{SlotTable, SlotValue};

fn client_evidence(life: &LifeFixture) -> (Vec<u32>, Option<u32>, Option<f32>, u64) {
    let netcode::NetEndpoint::Client {
        prediction,
        replication,
        state_slots,
        ..
    } = &life.fixture.receive.as_ref().unwrap().endpoint
    else {
        unreachable!()
    };
    let witness = state_slots.life_state_elapsed_witness();
    (
        prediction.pending_command_ticks_for_test(),
        replication.latest_server_tick(),
        witness.elapsed_ms,
        witness.reset_serial,
    )
}

fn elapsed(life: &LifeFixture) -> u32 {
    life.fixture
        .local
        .borrow()
        .get_component::<PlayerMovementComponent>(life.fixture.local_actors.pawn)
        .unwrap()
        .life_state_elapsed_ticks
}

fn frozen_display(
    life: &LifeFixture,
    ctx: &ScriptCtx,
    slots: &SlotTable,
    display: &mut LifeStateDisplay,
    alpha: f32,
    dt: Duration,
) -> f32 {
    let sample = crate::life_state_display::sample(
        ctx,
        Some(&life.fixture.receive.as_ref().unwrap().endpoint),
        alpha,
    );
    assert!(sample.is_some());
    let values = crate::App::build_ui_display_slot_snapshot(
        slots,
        None,
        Some(UiProjection {
            display,
            sample,
            frame_dt: dt,
        }),
        None,
    );
    match values["player.lifeStateMs"] {
        SlotValue::Number(value) => value,
        _ => unreachable!(),
    }
}

#[test]
fn life_state_actual_backlog_trim_ack_replay_keeps_240hz_display_advancing_and_converges() {
    let mut life = LifeFixture::new(descriptor(false, false, 1, "press"), false);
    life.zero_shooter(false);
    for _ in 0..32 {
        life.step(neutral());
    }
    life.assert_owner_down(false);
    let mut local_ctx = ScriptCtx::new();
    local_ctx.registry = life.fixture.local.clone();
    let slots = SlotTable::new();
    let gameplay_value = slots.get("player.lifeStateMs").unwrap().value.clone();
    let mut display = LifeStateDisplay::default();
    let dt = Duration::from_secs_f64(1.0 / 240.0);
    let tick_ms = postretro_foundation::TICK_DURATION.as_secs_f64() * 1000.0;
    let dt_ms = dt.as_secs_f64() * 1000.0;
    let mut previous = frozen_display(&life, &local_ctx, &slots, &mut display, 0.0, dt);
    let mut previous_target = predicted_target_ms(elapsed(&life), 0.0);
    let initial_witness = client_evidence(&life).3;
    let mut fixed_ticks = 0;
    let mut trim = None;
    let mut jumped_ack = false;
    let mut target_fell = false;
    let mut zero_tick_frames = 0;
    let mut largest_error = 0.0_f64;

    for frame in 1..=8 * 240 {
        if frame % 4 == 0 {
            fixed_ticks += 1;
            if fixed_ticks == 60 {
                life.fixture.cut_up = true;
            }
            if fixed_ticks == 84 {
                life.fixture.cut_up = false;
            }
            let before = client_evidence(&life);
            let before_cursor = life
                .fixture
                .queues
                .resolved_cursor_for_test(life.fixture.shooter_client_id);
            life.step(neutral());
            let after_cursor = life
                .fixture
                .queues
                .resolved_cursor_for_test(life.fixture.shooter_client_id);
            if let (Some(old), Some(new)) = (before_cursor, after_cursor)
                && new.wrapping_sub(old) > 1
            {
                assert!(
                    fixed_ticks >= 84,
                    "only the returning reliable burst may fast-forward"
                );
                trim.get_or_insert((old, new));
            }
            let after = client_evidence(&life);
            let pruned = before
                .0
                .iter()
                .take_while(|tick| !after.0.contains(tick))
                .count();
            if pruned > 2 {
                let (_, trimmed_cursor) =
                    trim.expect("a jumped ack follows an actual host catch-up trim");
                assert_ne!(
                    before.1, after.1,
                    "the skipped prefix is acknowledged by an accepted snapshot"
                );
                assert!(
                    !after.0.is_empty(),
                    "the snapshot still replays a predicted tail"
                );
                assert!(
                    after.0[0] > trimmed_cursor,
                    "the ack pruned commands through the trimmed cursor"
                );
                let authoritative = f64::from(
                    after
                        .2
                        .expect("accepted owner elapsed accompanies movement"),
                );
                let replayed = predicted_target_ms(elapsed(&life), 0.0);
                assert!(
                    (replayed - authoritative - after.0.len() as f64 * tick_ms).abs() < 1.01,
                    "each surviving command contributes exactly one replay tick"
                );
                jumped_ack = true;
            }
        } else {
            zero_tick_frames += 1;
        }

        let alpha = (frame % 4) as f32 / 4.0;
        let target = predicted_target_ms(elapsed(&life), alpha);
        if target < previous_target {
            assert!(
                trim.is_some(),
                "a backwards target follows real backlog correction"
            );
            target_fell = true;
        }
        let next = frozen_display(&life, &local_ctx, &slots, &mut display, alpha, dt);
        let advance = f64::from(next - previous);
        assert!(
            next > previous,
            "positive render frame {frame} held or moved backwards: {previous} -> {next}"
        );
        assert!(
            advance >= dt_ms * 0.25 - 0.01 && advance <= dt_ms * 1.25 + 0.01,
            "ordinary replay correction changes only the bounded positive playback rate: {advance}"
        );
        assert_eq!(
            client_evidence(&life).3,
            initial_witness,
            "replay rewind is not an authoritative reset"
        );
        let still = frozen_display(
            &life,
            &local_ctx,
            &slots,
            &mut display,
            alpha,
            Duration::ZERO,
        );
        assert_eq!(
            still, next,
            "zero-duration frames retain the previous emitted value"
        );
        largest_error = largest_error.max((f64::from(next) - target).abs());
        previous = next;
        previous_target = target;
    }
    assert!(
        life.fixture.cut_up_packets > 0,
        "the reliable Input stream really lost datagrams"
    );
    assert!(
        trim.is_some() && jumped_ack && target_fell,
        "exercise trim, jumped ack/replay and backwards prediction target"
    );
    assert!(zero_tick_frames > 0);
    let final_error = (f64::from(previous) - previous_target).abs();
    assert!(
        largest_error > tick_ms,
        "the correction must challenge the presentation clock"
    );
    assert!(
        final_error < 1.0 && final_error < largest_error * 0.05,
        "positive-rate playback converges after recovery: peak {largest_error}, final {final_error}"
    );
    assert_eq!(
        slots.get("player.lifeStateMs").unwrap().value,
        gameplay_value,
        "the frozen UI projection never writes presentation time into gameplay slots"
    );
    life.assert_owner_down(false);
}
