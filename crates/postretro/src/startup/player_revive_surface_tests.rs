//! The dev mod's Borderlands revive end to end on the two-seat production tick
//! loop: a teammate's hold-`assist` stands the downed seat up (AC11), a kill made
//! while down is a second wind (AC11), a completion the target's own second wind
//! overtakes in the same tick fires no assist source (O3), and the engine-default
//! hold-E resolves `assist` after the hold threshold, so completion follows
//! `holdMs` after that point rather than after key-down (O27).
//!
//! See: context/lib/scripting.md §12 (Player events, Assist)
use super::player_death_surface_tests::{Consumer, command};
use crate::input::{Action, ButtonState};
use glam::Vec3;
use postretro_entities::{
    EntityId, Transform,
    components::health::{
        DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
        set_health_absolute,
    },
    data_descriptors::HealthDescriptor,
};
use postretro_foundation::{DamagePayload, Seat};
use postretro_sim::sim::{SimCommand, assist::required_count};
use winit::keyboard::KeyCode;

const DT: f64 = 1.0 / 60.0;
/// Engine default `holdMs` (3000) in whole fixed ticks.
const HOLD_TICKS: u32 = 180;

fn hold() -> SimCommand {
    let mut held = command();
    held.assist_held = true;
    held
}

/// Seat `n`'s `progression.xp` (a per-owner slot, default 0).
fn xp(h: &Consumer, seat: u16) -> f32 {
    let table = h.ctx.slot_table.borrow();
    match table
        .get("progression.xp")
        .expect("the dev mod registers progression")
        .per_seat_value(Seat(seat))
    {
        Some(postretro_entities::SlotValue::Number(value)) => *value,
        _ => 0.0,
    }
}

/// Put the other seat in range of the host, then down it (one tick).
fn down_the_other_seat_beside_the_host(h: &mut Consumer) {
    let mut pose = h.pose(h.other);
    pose.position = h.pose(h.host).position + Vec3::X;
    h.ctx
        .registry
        .borrow_mut()
        .set_component(h.other, pose)
        .unwrap();
    set_health_absolute(&mut h.ctx.registry.borrow_mut(), h.other, 0.0);
    h.tick(command());
    assert_eq!(h.name(h.other), "down");
    assert_eq!(h.name(h.host), "alive");
}

/// A lethal in-tick hit on a fresh `enemy`-tagged target, `pawn` the damager,
/// then the impact policies, as the post-tick hit flush runs them.
fn kill_an_enemy_as(h: &Consumer, pawn: EntityId) {
    let enemy = {
        let mut registry = h.ctx.registry.borrow_mut();
        let enemy = registry.spawn(Transform::default());
        registry
            .set_component(
                enemy,
                HealthComponent::from_descriptor(&HealthDescriptor {
                    max: 100.0,
                    hitbox: None,
                    zone_multipliers: Default::default(),
                }),
            )
            .unwrap();
        registry.set_tags(enemy, vec!["enemy".to_string()]).unwrap();
        enemy
    };
    let mut context = DamageContext::new("player-revive-test", DamageProducer::InTick);
    context.attacker = Some(pawn);
    apply_damage_with_context(
        &mut h.ctx.registry.borrow_mut(),
        enemy,
        &DamagePayload {
            amount: 200.0,
            impulse: Vec3::ZERO,
        },
        context,
    );
    h.impact
        .borrow_mut()
        .evaluate_pending_in_registry(&mut h.ctx.registry.borrow_mut());
}

/// AC11, rows `assistable: {}` / `assistReceived` / `assistGiven`: the teammate's
/// hold completes on exactly the 180th held tick (the engine default 3000 ms),
/// not before, and stands the target up with 30 health, the helper earning 25 XP.
#[test]
fn player_revive_down_assist_alive_stands_the_target_up_and_pays_the_helper() {
    let mut h = Consumer::new();
    down_the_other_seat_beside_the_host(&mut h);
    assert_eq!(required_count(3000.0), HOLD_TICKS);
    for held in 1..HOLD_TICKS {
        let events = h.tick(hold());
        assert!(events.assist_completions.is_empty(), "held tick {held}");
        assert_eq!(h.name(h.other), "down", "held tick {held}");
        assert_eq!(h.health(h.other), 0.0);
    }
    let events = h.tick(hold());
    assert_eq!(events.assist_completions.len(), 1, "the 180th held tick");
    assert_eq!(h.name(h.other), "alive");
    assert_eq!(h.health(h.other), 30.0, "assistReceived: grantHealth(30)");
    assert_eq!(xp(&h, 0), 25.0, "assistGiven: the helper earns 25 XP");
    assert_eq!(xp(&h, 1), 0.0);
    assert_eq!(h.name(h.host), "alive");
    assert_eq!(h.health(h.host), 100.0);
}

/// AC11, row `second-wind`: a kill the downed seat makes stands it up with 40
/// health and 50 XP; the same kill by an upright seat pays nothing, and the
/// teammate is untouched either way.
#[test]
fn player_revive_down_self_revive_second_wind_on_a_kill() {
    let mut h = Consumer::new();
    // An upright seat's kill is not a second wind.
    h.tick_then(command(), |h| kill_an_enemy_as(h, h.host));
    assert_eq!(
        (h.name(h.host), h.health(h.host), xp(&h, 0)),
        ("alive".into(), 100.0, 0.0)
    );

    down_the_other_seat_beside_the_host(&mut h);
    let other = h.other;
    h.tick_then(command(), |h| kill_an_enemy_as(h, other));
    assert_eq!(h.name(h.other), "alive");
    assert_eq!(h.health(h.other), 40.0, "grantHealth(40)");
    assert_eq!(
        xp(&h, 1),
        50.0,
        "the XP bump lands on the killer's own seat"
    );
    assert_eq!(xp(&h, 0), 0.0);
    // Revived, so bleed-out never fires: stay up past the 15 s window.
    for _ in 0..30 {
        h.tick(command());
    }
    assert_eq!(h.name(h.other), "alive");
    assert!(h.ctx.pending_respawns.borrow().is_empty());
}

/// O3: the assist completes in the tick's accumulator pass, then the target's
/// own second-wind kill flushes before the player-event pass. The completion is
/// dropped at application because the target left `down`: no `assistReceived`
/// (no extra 30 health) and no `assistGiven` (no helper XP).
#[test]
fn player_revive_o3_self_revive_flush_drops_the_recorded_assist_completion() {
    let mut h = Consumer::new();
    down_the_other_seat_beside_the_host(&mut h);
    for _ in 1..HOLD_TICKS {
        h.tick(hold());
    }
    assert_eq!(h.name(h.other), "down");
    let other = h.other;
    let events = h.tick_then(hold(), |h| kill_an_enemy_as(h, other));
    assert_eq!(
        events.assist_completions.len(),
        1,
        "the accumulator recorded the completion in the tick"
    );
    assert_eq!(h.name(h.other), "alive");
    assert_eq!(
        h.health(h.other),
        40.0,
        "only the second wind's health: `assistReceived` did not fire"
    );
    assert_eq!(xp(&h, 0), 0.0, "`assistGiven` did not fire");
    assert_eq!(xp(&h, 1), 50.0);
}

/// O27: hold-E on the dev mod's effective table. Its input block authors a
/// label for `assist` and no bindings, so the engine default (`hold` on E) is
/// what resolves. `assist` resolves once the default hold threshold passes and
/// the pair then needs `holdMs` of held ticks: completion follows the
/// resolution point by 180 held ticks, not key-down by 3000 ms.
#[test]
fn player_revive_o27_hold_e_resolves_assist_after_the_threshold_then_holds_for_hold_ms() {
    let mut h = Consumer::new();
    h._app.load_author_bindings(h.input_block.as_ref());
    h._app.refresh_effective_bindings();
    {
        let session = h._app.session.as_ref().unwrap();
        let rows = crate::app::controls_panel::controls_rows(
            session.bindings.table(),
            session.bindings.author(),
        );
        let row = rows
            .iter()
            .find(|row| row.command == crate::input::Command::Assist)
            .expect("the dev mod lists `assist`: a declared `assistable` makes it relevant");
        assert_eq!(row.label, "Revive teammate");
        assert!(
            row.inputs[0].contains(&(
                crate::input::PhysicalInput::Key(KeyCode::KeyE),
                crate::input::ActivatorKind::Hold
            )),
            "the engine default hold-E, with no authored binding: {:?}",
            row.inputs
        );
    }
    down_the_other_seat_beside_the_host(&mut h);

    let threshold = f64::from(crate::input::DEFAULT_ACTIVATOR_THRESHOLD);
    h._app
        .session
        .as_mut()
        .unwrap()
        .input_system
        .handle_keyboard_event_at(KeyCode::KeyE, true, 0.0);
    let (mut first_held, mut completed) = (None, None);
    for tick in 1..=(HOLD_TICKS + 40) {
        let at = f64::from(tick) * DT;
        let snapshot = h
            ._app
            .session
            .as_mut()
            .unwrap()
            .input_system
            .snapshot_at(at);
        assert_ne!(
            snapshot.button(Action::Use),
            ButtonState::Pressed,
            "a hold never fires Use"
        );
        let mut input = command();
        input.assist_held = snapshot.button(Action::Assist).is_active();
        if input.assist_held && first_held.is_none() {
            first_held = Some(tick);
        }
        let events = h.tick(input);
        if !events.assist_completions.is_empty() && completed.is_none() {
            completed = Some(tick);
        }
    }
    let (first_held, completed) = (
        first_held.expect("assist resolves"),
        completed.expect("completes"),
    );
    let resolved_at = f64::from(first_held) * DT;
    assert!(
        resolved_at >= threshold && resolved_at - DT < threshold,
        "assist resolves on the first tick past the {threshold} s threshold, not at key-down: {resolved_at}"
    );
    assert_eq!(
        completed - first_held + 1,
        HOLD_TICKS,
        "completion is `holdMs` of held ticks after resolution"
    );
    let completed_at = f64::from(completed) * DT;
    assert!(
        completed_at >= threshold + 3.0 - DT,
        "completion at {completed_at} s is the threshold plus 3 s, not 3 s after key-down"
    );
    assert_eq!(h.name(h.other), "alive");
}
