// Ghost-link member steps through the registered primitives: validation, host-only queueing,
// next-tick apply in call order and per-field colour-filter ramps.
// See: context/lib/scripting.md §12 (Entity addressing) · context/lib/entity_model.md §7d

use postretro_entities::{GhostLinkComponent, Transform};
use postretro_foundation::GhostColorFilter;
use serde_json::{Value, json};

use super::*;

/// The link's style look, as the binary resolves it from the manifest.
const STYLE: GhostLook = GhostLook {
    tint: [1.0, 1.0, 1.0],
    opacity: 0.5,
    saturation: 0.0,
};

fn style(_index: u16) -> GhostLook {
    STYLE
}

struct Host {
    ctx: ScriptCtx,
    steps: SequencedPrimitiveRegistry,
    link: EntityId,
}

impl Host {
    fn new() -> Self {
        let ctx = ScriptCtx::new();
        let link = {
            let mut registry = ctx.registry.borrow_mut();
            let id = registry.spawn(Transform::default());
            registry
                .set_component(
                    id,
                    GhostLinkComponent {
                        index: 0,
                        source_entity: 9,
                        armed: false,
                        members: GhostMembers::all().bits(),
                        filter: GhostColorFilter::default(),
                    },
                )
                .unwrap();
            id
        };
        let mut steps = SequencedPrimitiveRegistry::new();
        register_sequenced_ghost_link_primitives(&mut steps, ctx.clone());
        Self { ctx, steps, link }
    }

    fn step(&self, primitive: &str, args: Value) -> Result<(), SequenceError> {
        self.steps.get(primitive).expect("registered")(self.link, &args)
    }

    /// One authoritative tick start at script time `time`.
    fn tick(&self, time: f64) {
        apply_pending_ghost_link_commands(&self.ctx, time, style);
    }

    fn state(&self) -> GhostLinkComponent {
        *self
            .ctx
            .registry
            .borrow()
            .get_component::<GhostLinkComponent>(self.link)
            .unwrap()
    }

    fn look(&self, time: f64) -> GhostLook {
        self.state().filter.resolve(STYLE, time)
    }

    fn queued(&self) -> usize {
        self.ctx.pending_ghost_link_commands.borrow().len()
    }
}

fn reason(result: Result<(), SequenceError>) -> String {
    match result {
        Err(SequenceError::InvalidArgument { reason }) => reason,
        other => panic!("expected an invalid-argument rejection, got {other:?}"),
    }
}

// P2 / P44: steps queue and apply at the next tick in call order, so arm then
// disarm, or a category off then on, in one tick end in the last call's state.
#[test]
fn ghost_link_steps_apply_at_the_next_tick_in_call_order_and_the_last_call_wins() {
    let host = Host::new();
    host.step(ARM_GHOST_LINK, json!({})).unwrap();
    host.step(DISARM_GHOST_LINK, json!({})).unwrap();
    host.step(SET_GHOST_LINK_MEMBERS, json!({ "players": false }))
        .unwrap();
    host.step(
        SET_GHOST_LINK_MEMBERS,
        json!({ "players": true, "movers": false }),
    )
    .unwrap();
    assert!(!host.state().armed, "nothing applies before the tick");
    assert_eq!(host.queued(), 4);
    host.tick(1.0);
    let state = host.state();
    assert!(!state.armed, "arm then disarm ends disarmed");
    assert_eq!(
        GhostMembers::from_bits_truncate(state.members),
        GhostMembers::all() - GhostMembers::MOVERS,
        "players off then on ends on; the omitted categories keep their value"
    );
    assert_eq!(host.queued(), 0);

    host.step(ARM_GHOST_LINK, json!({})).unwrap();
    host.tick(1.1);
    assert!(host.state().armed);
}

// AC40: the same steps run on a connected client change nothing.
#[test]
fn ghost_link_steps_on_a_connected_client_change_nothing() {
    let host = Host::new();
    host.ctx.owner_slot_writes_enabled.set(false);
    host.step(ARM_GHOST_LINK, json!({})).unwrap();
    host.step(SET_GHOST_LINK_MEMBERS, json!({ "players": false }))
        .unwrap();
    host.step(
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "opacity": 1.0, "periodMs": 500 }),
    )
    .unwrap();
    assert_eq!(host.queued(), 0, "a client queues nothing");
    // Even a stray queued command is dropped, not applied, on a client.
    host.ctx
        .pending_ghost_link_commands
        .borrow_mut()
        .push((host.link, GhostLinkCommand::Armed(true)));
    host.tick(1.0);
    assert_eq!(host.queued(), 0);
    let state = host.state();
    assert!(!state.armed);
    assert_eq!(state.members, GhostMembers::all().bits());
    assert_eq!(state.filter, GhostColorFilter::default());
}

// AC27 / P5: each named field ramps on its own; an opacity-only set mid tint
// ramp leaves the tint ramp running to its target.
#[test]
fn an_opacity_only_filter_mid_tint_ramp_leaves_the_tint_ramp_running() {
    let host = Host::new();
    host.step(
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "tint": [0.0, 0.0, 0.0], "periodMs": 1000 }),
    )
    .unwrap();
    host.tick(10.0);
    host.step(
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "opacity": 1.0, "periodMs": 1000 }),
    )
    .unwrap();
    host.tick(10.5);
    let mid = host.look(10.5);
    assert!((mid.tint[0] - 0.5).abs() < 1e-6, "tint ramp in flight");
    assert!((mid.opacity - 0.5).abs() < 1e-6, "opacity ramp starts here");
    assert_eq!(
        mid.saturation, STYLE.saturation,
        "unset field keeps the style"
    );
    let end = host.look(11.0);
    assert_eq!(
        end.tint, [0.0; 3],
        "tint reaches its target on its own clock"
    );
    assert!((end.opacity - 0.75).abs() < 1e-6);
    assert_eq!(host.look(11.5).opacity, 1.0);
}

// P7: two calls in one tick apply in order; the later wins per field and its
// ramp starts from the value before that tick.
#[test]
fn two_filters_in_one_tick_apply_in_order_the_later_winning_from_the_pre_tick_value() {
    let host = Host::new();
    host.step(
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "opacity": 0.1, "saturation": 1.0 }),
    )
    .unwrap();
    host.step(
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "opacity": 0.9, "periodMs": 1000 }),
    )
    .unwrap();
    host.tick(4.0);
    let filter = host.state().filter;
    let opacity = filter.opacity.unwrap();
    assert_eq!(opacity.to, 0.9, "the later call wins opacity");
    assert_eq!(
        opacity.from, STYLE.opacity,
        "from the value before the tick"
    );
    assert_eq!(
        filter.saturation.unwrap().to,
        1.0,
        "the earlier call's other field stands"
    );
    assert_eq!(host.look(4.0).saturation, 1.0, "periodMs 0 lands at once");
}

// AC29 / P6: a second set mid-ramp starts from the unclamped value at that moment.
#[test]
fn a_second_filter_mid_ramp_starts_from_the_ramp_value_at_its_tick() {
    let host = Host::new();
    host.step(
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "opacity": 1.0, "periodMs": 2000 }),
    )
    .unwrap();
    host.tick(0.0);
    host.step(SET_GHOST_LINK_COLOR_FILTER, json!({ "opacity": 0.0 }))
        .unwrap();
    host.tick(1.0);
    let opacity = host.state().filter.opacity.unwrap();
    assert!((opacity.from - 0.75).abs() < 1e-6, "{opacity:?}");
    assert_eq!(opacity.start_time, 1.0);
    assert_eq!(opacity.duration_ms, 0.0, "periodMs defaults to 0");
    assert_eq!(host.look(1.0).opacity, 0.0);
}

// AC27 / P41 and AC29: a call naming no field, an out-of-range or non-finite-
// as-f32 argument, a non-boolean category and an unknown field each warn with
// a stable reason and queue nothing, so no ramp in flight changes.
#[test]
fn malformed_ghost_link_steps_are_rejected_with_their_reason_and_change_nothing() {
    let host = Host::new();
    host.step(
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "opacity": 1.0, "periodMs": 1000 }),
    )
    .unwrap();
    host.tick(0.0);
    let ramp = host.state().filter;

    let cases = [
        (
            SET_GHOST_LINK_COLOR_FILTER,
            json!({ "periodMs": 500 }),
            "setColorFilter names no field; expected one or more of tint, opacity or saturation",
        ),
        (
            SET_GHOST_LINK_COLOR_FILTER,
            json!({}),
            "setColorFilter names no field",
        ),
        (
            SET_GHOST_LINK_COLOR_FILTER,
            json!({ "opacity": 1.5 }),
            "setColorFilter `opacity` must be a finite number in 0..=1, got 1.5",
        ),
        (
            SET_GHOST_LINK_COLOR_FILTER,
            json!({ "saturation": -0.1 }),
            "setColorFilter `saturation` must be a finite number in 0..=1, got -0.1",
        ),
        (
            SET_GHOST_LINK_COLOR_FILTER,
            json!({ "tint": [1.0, 2.0, 0.0] }),
            "setColorFilter `tint` must be [r, g, b] with each component a finite number in 0..=1",
        ),
        (
            SET_GHOST_LINK_COLOR_FILTER,
            json!({ "tint": [1.0, 1.0] }),
            "setColorFilter `tint` must be [r, g, b]",
        ),
        (
            SET_GHOST_LINK_COLOR_FILTER,
            json!({ "opacity": 0.5, "periodMs": -1 }),
            "setColorFilter `periodMs` must be a finite number >= 0, got -1",
        ),
        (
            SET_GHOST_LINK_COLOR_FILTER,
            json!({ "opacity": 0.5, "periodMs": 1e300 }),
            "setColorFilter `periodMs` must be a finite number >= 0",
        ),
        (
            SET_GHOST_LINK_COLOR_FILTER,
            json!({ "alpha": 0.5 }),
            "setColorFilter has no field `alpha`",
        ),
        (
            SET_GHOST_LINK_MEMBERS,
            json!({ "projectiles": 0 }),
            "setMembers `projectiles` must be a boolean, got 0",
        ),
        (
            SET_GHOST_LINK_MEMBERS,
            json!({}),
            "setMembers names no category; expected one or more of players, entities, projectiles or movers",
        ),
        (
            SET_GHOST_LINK_MEMBERS,
            json!({ "player": false }),
            "setMembers has no field `player`",
        ),
    ];
    for (primitive, args, expected) in cases {
        let got = reason(host.step(primitive, args.clone()));
        assert!(got.contains(expected), "{args}: {got}");
    }
    assert_eq!(host.queued(), 0);
    host.tick(0.5);
    assert_eq!(host.state().filter, ramp, "no ramp in flight changed");
    // Both ends of every range are legal.
    host.step(
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "tint": [0, 1, 0.5], "opacity": 0, "saturation": 1, "periodMs": 0 }),
    )
    .unwrap();
}

#[test]
fn a_step_naming_something_other_than_a_ghost_link_warns_and_queues_nothing() {
    let host = Host::new();
    let other = host.ctx.registry.borrow_mut().spawn(Transform::default());
    let capture = postretro_test_log_capture::LogCapture::start();
    host.steps.get(ARM_GHOST_LINK).unwrap()(other, &json!({})).unwrap();
    capture.assert_logged_once(log::Level::Warn, "is not a live ghost link; skipping");
    assert_eq!(host.queued(), 0);
}
