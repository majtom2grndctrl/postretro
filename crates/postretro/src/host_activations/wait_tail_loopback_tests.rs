//! Loopback proof that a `wait` changes when a step lands, never who sees or
//! hears it: host plus client over real UDP, each running its own scheduler.
//! See: context/lib/scripting.md §12 (Steps after a `wait`)
include!("presentation_conditioned_fixture.rs");

use postretro_test_log_capture::LogCapture;

fn wait_step(duration_ms: f64) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Wait,
        primitive: "wait".into(),
        args: json!({ "durationMs": duration_ms, "interruptible": false }),
    }
}
fn system_step(primitive: &str, args: Value) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::System,
        primitive: primitive.into(),
        args,
    }
}
fn body(name: &str, steps: Vec<SequenceStep>) -> NamedReaction {
    NamedReaction {
        name: name.into(),
        descriptor: ReactionDescriptor::Sequence(steps),
    }
}
fn plays(commands: &[SystemReactionCommand], name: &str) -> usize {
    commands
        .iter()
        .filter(|command| matches!(command, SystemReactionCommand::PlaySound { sound, .. } if sound == name))
        .count()
}
/// "Nothing above debug about the tail": the scheduler and the reaction
/// dispatch it drives stay quiet; unrelated net chatter is not the tail's.
fn assert_tail_quiet(capture: &LogCapture) {
    let loud: Vec<_> = capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level < log::Level::Debug
                && (record.message.contains("[Scheduler]")
                    || record.message.contains("[Scripting]"))
        })
        .collect();
    assert!(loud.is_empty(), "tail logged above debug: {loud:?}");
}
fn pending(app: &App) -> usize {
    app.session
        .as_ref()
        .unwrap()
        .scripting
        .scheduler
        .pending_len()
}

// S10 (first form): `levelLoad = [wait(500), playSound(x)]`. The host lands
// the tail and presents locally; the client parks its own copy and lands it
// on the host timeline. Each machine hears `x` once, and the host forwards no
// Presentation command for it.
#[test]
fn level_load_tail_sound_plays_once_on_each_machine_from_its_own_landing() {
    let mut h = Harness::with_shared_reactions(
        fixture_world(),
        vec![body(
            "levelLoad",
            vec![
                wait_step(500.0),
                system_step("playSound", json!({ "sound": "x" })),
            ],
        )],
    );
    assert_eq!(pending(&h.host), 1, "the host parks the tail");
    assert_eq!(pending(&h.peers[0].app), 1, "the client parks its own copy");
    let capture = LogCapture::start();

    // 500 ms is 30 ticks; nothing lands a tick early on either machine.
    h.advance(28);
    assert_eq!(plays(&h.host_commands, "x"), 0);
    assert_eq!(plays(&h.peers[0].commands, "x"), 0);

    h.advance(60);
    assert_eq!(plays(&h.host_commands, "x"), 1, "the host hears x once");
    assert_eq!(
        plays(&h.peers[0].commands, "x"),
        1,
        "the client hears x once, from its own landing, with nothing forwarded"
    );
    assert_eq!(pending(&h.peers[0].app), 0);
    assert_tail_quiet(&capture);
}
