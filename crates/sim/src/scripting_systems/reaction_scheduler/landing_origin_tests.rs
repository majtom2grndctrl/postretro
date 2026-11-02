// Host landings route as the same step does before the wait: the parked
// origin's subject, its seat resolved at landing, the host's own player
// presented on the host and a departed one on no machine.
// See: context/lib/scripting.md §12 (Steps after a `wait`)

use std::cell::Cell;
use std::rc::Rc;

use postretro_entities::{
    EntityId, FireOrigin, FireSubject, Recipients, ScriptCtx, SequenceStep, SequenceTarget,
    SystemCommandFireContext, SystemReactionCommand, Transform,
};
use postretro_foundation::Seat;
use postretro_scripting_core::data_descriptors::ReactionDescriptor;
use postretro_scripting_core::reaction_dispatch::{
    PrepartitionedReactionStep, ResidualOrigin, fire_prepartitioned_reactions_with_sequences,
};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_test_log_capture::LogCapture;
use serde_json::json;

use super::{ReactionScheduler, SchedulerRole, register_reaction_control_primitives};
use crate::scripting_systems::system_reactions::register_system_reaction_primitives;
use crate::trigger_system::PlayerId;

const CLIENT: u64 = 7;

fn wait(duration_ms: f64) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Wait,
        primitive: "wait".into(),
        args: json!({ "durationMs": duration_ms, "interruptible": false }),
    }
}
fn system(primitive: &str, args: serde_json::Value) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::System,
        primitive: primitive.into(),
        args,
    }
}
fn flash() -> SequenceStep {
    system(
        "flashScreen",
        json!({ "color": [1, 1, 1, 1], "durationMs": 50 }),
    )
}
fn sound(name: &str) -> SequenceStep {
    system("playSound", json!({ "sound": name }))
}

/// A host's reaction machinery: the scheduler, system primitives, `wait`, and
/// a `note` member step counting its runs, over a registry where the host's
/// pawn holds `Seat(0)` and the client's pawn `Seat(1)`.
struct Rig {
    ctx: ScriptCtx,
    scheduler: ReactionScheduler,
    sequence: SequencedPrimitiveRegistry,
    reaction: ReactionPrimitiveRegistry,
    system: SystemReactionRegistry,
    notes: Rc<Cell<usize>>,
    host_pawn: EntityId,
    client_pawn: EntityId,
    client_connected: bool,
}

impl Rig {
    fn new() -> Self {
        let ctx = ScriptCtx::new();
        let scheduler = ReactionScheduler::default();
        scheduler.set_role(SchedulerRole::Host);
        let mut sequence = SequencedPrimitiveRegistry::new();
        register_reaction_control_primitives(&mut sequence, scheduler.clone());
        let notes = Rc::new(Cell::new(0));
        {
            let notes = notes.clone();
            sequence.register("note", move |_, _| {
                notes.set(notes.get() + 1);
                Ok(())
            });
        }
        let mut system = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system);
        let (host_pawn, client_pawn) = {
            let mut registry = ctx.registry.borrow_mut();
            let host = registry.spawn(Transform::default());
            let client = registry.spawn(Transform::default());
            registry.bind_pawn_seat(host, Seat(0));
            registry.bind_pawn_seat(client, Seat(1));
            (host, client)
        };
        Self {
            ctx,
            scheduler,
            sequence,
            reaction: ReactionPrimitiveRegistry::new(),
            system,
            notes,
            host_pawn,
            client_pawn,
            client_connected: true,
        }
    }

    fn note(&self) -> SequenceStep {
        SequenceStep {
            id: SequenceTarget::Entity(self.host_pawn),
            primitive: "note".into(),
            args: json!({}),
        }
    }

    /// Drain one residual as the host's trigger, teleport or player-event
    /// drain does: keyed to `(source, player)`, under `origin`.
    fn fire(
        &self,
        source: EntityId,
        player: PlayerId,
        origin: FireOrigin,
        steps: Vec<SequenceStep>,
    ) {
        let _origin = self.scheduler.begin_origin(source, player, false);
        let previous = self
            .ctx
            .system_commands
            .replace_fire_context(SystemCommandFireContext {
                source: "residual".into(),
                origin,
                ..Default::default()
            });
        let _ = fire_prepartitioned_reactions_with_sequences(
            &[PrepartitionedReactionStep::Descriptor(
                "beat".into(),
                0,
                ReactionDescriptor::Sequence(steps),
            )],
            &self.sequence,
            &self.reaction,
            &self.system,
            &self.ctx,
            ResidualOrigin::TriggerBinding,
        );
        self.ctx.system_commands.replace_fire_context(previous);
    }

    fn trigger_by(&self, player: PlayerId, seat: Seat, steps: Vec<SequenceStep>) {
        let trigger = EntityId::from_raw(90);
        self.fire(
            trigger,
            player,
            FireOrigin::HostOnly(FireSubject::Trigger {
                player: Some(seat),
                trigger,
                occupants: vec![seat],
            }),
            steps,
        );
    }

    fn teleport_by(&self, player: PlayerId, seat: Seat, steps: Vec<SequenceStep>) {
        let teleporter = EntityId::from_raw(91);
        self.fire(
            teleporter,
            player,
            FireOrigin::HostOnly(FireSubject::Teleport {
                player: Some(seat),
                teleporter,
            }),
            steps,
        );
    }

    fn player_event_for(&self, pawn: EntityId, seat: Seat, steps: Vec<SequenceStep>) {
        self.fire(
            pawn,
            PlayerId::Local(pawn),
            FireOrigin::HostOnly(FireSubject::PlayerEvent { player: Some(seat) }),
            steps,
        );
    }

    /// Run frames until every tail lands, resolving seats as the host's frame
    /// loop does, and return what the landings queued.
    fn land(&self) -> Vec<(Recipients, SystemReactionCommand)> {
        let connected = self.client_connected;
        let _ = self.ctx.system_commands.take_ordered();
        for _ in 0..60 {
            self.scheduler.begin_frame();
            self.scheduler.evaluate(&[]);
            self.scheduler.drain_landings_resolving_seats(
                &self.ctx.data_registry.borrow(),
                &self.sequence,
                &self.reaction,
                &self.system,
                &self.ctx,
                &|player, registry| match player {
                    PlayerId::Local(pawn) => registry.seat_for_pawn(pawn),
                    PlayerId::Remote(CLIENT) if connected => Some(Seat(1)),
                    PlayerId::Remote(_) => None,
                },
            );
        }
        assert_eq!(self.scheduler.pending_len(), 0, "every tail landed");
        self.ctx.system_commands.take_ordered()
    }

    fn disconnect_client(&mut self) {
        self.client_connected = false;
        self.ctx
            .registry
            .borrow_mut()
            .despawn(self.client_pawn)
            .unwrap();
    }
}

fn routes(commands: &[(Recipients, SystemReactionCommand)]) -> Vec<(&'static str, Recipients)> {
    commands
        .iter()
        .map(|(recipients, command)| {
            (
                command.presentation_primitive().unwrap_or("other"),
                recipients.clone(),
            )
        })
        .collect()
}

// S14: a trigger tail's flash reaches the activator, its sound every machine.
#[test]
fn s14_trigger_tail_routes_to_the_activator_and_sounds_everywhere() {
    let rig = Rig::new();
    rig.trigger_by(
        PlayerId::Remote(CLIENT),
        Seat(1),
        vec![wait(300.0), flash(), sound("y")],
    );
    assert_eq!(
        routes(&rig.land()),
        [
            ("flashScreen", Recipients::Seats(vec![Seat(1)])),
            ("playSound", Recipients::Everyone),
        ]
    );
}

// S15: a teleport tail routes the same way.
#[test]
fn s15_teleport_tail_routes_to_the_teleported_player_and_sounds_everywhere() {
    let rig = Rig::new();
    rig.teleport_by(
        PlayerId::Remote(CLIENT),
        Seat(1),
        vec![wait(300.0), flash(), sound("y")],
    );
    assert_eq!(
        routes(&rig.land()),
        [
            ("flashScreen", Recipients::Seats(vec![Seat(1)])),
            ("playSound", Recipients::Everyone),
        ]
    );
}

// S16: a player event's tail reaches its player; after that player leaves it
// reaches no machine, the host included, and says nothing above debug.
#[test]
fn s16_player_event_tail_reaches_its_player_and_no_machine_once_they_leave() {
    let rig = Rig::new();
    rig.player_event_for(rig.client_pawn, Seat(1), vec![wait(300.0), flash()]);
    assert_eq!(
        routes(&rig.land()),
        [("flashScreen", Recipients::Seats(vec![Seat(1)]))]
    );

    let mut rig = Rig::new();
    rig.player_event_for(
        rig.client_pawn,
        Seat(1),
        vec![wait(300.0), flash(), sound("y")],
    );
    rig.disconnect_client();
    let capture = LogCapture::start();
    assert_eq!(
        routes(&rig.land()),
        [
            ("flashScreen", Recipients::Seats(Vec::new())),
            ("playSound", Recipients::Seats(Vec::new())),
        ]
    );
    capture.assert_nothing_above(log::Level::Debug);
}

// S22: routed to the host's own player, a landing presents on the host only.
#[test]
fn s22_tail_for_the_hosts_own_player_presents_on_the_host() {
    let rig = Rig::new();
    rig.player_event_for(rig.host_pawn, Seat(0), vec![wait(300.0), flash()]);
    rig.teleport_by(
        PlayerId::Local(rig.host_pawn),
        Seat(0),
        vec![wait(300.0), flash()],
    );
    assert_eq!(
        routes(&rig.land()),
        [
            ("flashScreen", Recipients::Seats(vec![Seat(0)])),
            ("flashScreen", Recipients::Seats(vec![Seat(0)])),
        ],
        "the host seat presents locally"
    );
}

// S23: the teleported player leaves during the wait: the flash reaches no
// machine, the member step still runs, and nothing logs above debug.
#[test]
fn s23_teleport_tail_after_its_player_leaves_flashes_nowhere_and_still_runs() {
    let mut rig = Rig::new();
    let note = rig.note();
    rig.teleport_by(
        PlayerId::Remote(CLIENT),
        Seat(1),
        vec![wait(300.0), flash(), note],
    );
    rig.disconnect_client();
    let capture = LogCapture::start();
    assert_eq!(
        routes(&rig.land()),
        [("flashScreen", Recipients::Seats(Vec::new()))]
    );
    assert_eq!(rig.notes.get(), 1, "the member step runs on the host");
    capture.assert_nothing_above(log::Level::Debug);
}

// S24: the activator leaves during the wait: the flash reaches no machine;
// the sound still plays on every remaining machine.
#[test]
fn s24_trigger_tail_after_its_activator_leaves_still_sounds_everywhere() {
    let mut rig = Rig::new();
    rig.trigger_by(
        PlayerId::Remote(CLIENT),
        Seat(1),
        vec![wait(300.0), flash(), sound("y")],
    );
    rig.disconnect_client();
    assert_eq!(
        routes(&rig.land()),
        [
            ("flashScreen", Recipients::Seats(Vec::new())),
            ("playSound", Recipients::Everyone),
        ]
    );
}

// AC 8, trigger half: `audience: "everyone"` names no subject, so a flash
// reaches every machine before a wait and after one alike.
#[test]
fn everyone_audience_flash_reaches_every_machine_before_and_after_a_wait() {
    let rig = Rig::new();
    let everyone = || {
        system(
            "flashScreen",
            json!({ "color": [1, 1, 1, 1], "durationMs": 50, "audience": "everyone" }),
        )
    };
    rig.trigger_by(
        PlayerId::Remote(CLIENT),
        Seat(1),
        vec![everyone(), wait(300.0), everyone()],
    );
    let before = rig.ctx.system_commands.take_ordered();
    assert_eq!(routes(&before), [("flashScreen", Recipients::Everyone)]);
    let after = rig.land();
    assert_eq!(routes(&after), [("flashScreen", Recipients::Everyone)]);
}

// AC 9, host half: after a wait the host runs a replicated write, the ghost
// bus and a level load, as the same steps do before it.
#[test]
fn host_landing_runs_host_consequences() {
    let rig = Rig::new();
    rig.trigger_by(
        PlayerId::Remote(CLIENT),
        Seat(1),
        vec![
            wait(300.0),
            system("setGhostAudio", json!({ "volume": 0.5 })),
            system("loadLevel", json!({ "map": "next" })),
        ],
    );
    let landed = rig.land();
    assert!(matches!(
        landed[0].1,
        SystemReactionCommand::SetGhostAudio(_)
    ));
    assert!(matches!(
        landed[1].1,
        SystemReactionCommand::LoadLevel { .. }
    ));
}

// AC 8, token half: before a wait, `audience: on.activators` reaches the
// activator only. (After a wait the token drops the reaction at install.)
#[test]
fn activators_audience_flash_before_a_wait_reaches_the_activator_only() {
    let rig = Rig::new();
    rig.trigger_by(
        PlayerId::Remote(CLIENT),
        Seat(1),
        vec![system(
            "flashScreen",
            json!({ "color": [1, 1, 1, 1], "durationMs": 50, "audience": "@activators" }),
        )],
    );
    assert_eq!(
        routes(&rig.ctx.system_commands.take_ordered()),
        [("flashScreen", Recipients::Seats(vec![Seat(1)]))]
    );
}
