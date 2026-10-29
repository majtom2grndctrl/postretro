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

/// Per machine, in install order (host, client A, client B): how many times a
/// `note` member step ran. Registered on every machine before install.
fn counted_notes(
    world: &postretro_level_loader::LevelWorld,
) -> Vec<std::rc::Rc<std::cell::Cell<usize>>> {
    let _ = world;
    let counters: Vec<_> = (0..3)
        .map(|_| std::rc::Rc::new(std::cell::Cell::new(0)))
        .collect();
    let shared = counters.clone();
    set_install_hook(move |index, scripting, ctx| {
        let counter = shared[index].clone();
        scripting.sequence_registry.register("note", move |_, _| {
            counter.set(counter.get() + 1);
            Ok(())
        });
        postretro_sim::scripting::reactions::registry::register_npc_state_reaction_primitives(
            &mut scripting.reaction_registry,
        );
        for (slot, network) in [
            (
                "tail.shared",
                postretro_entities::ReplicationScope::SharedGlobal,
            ),
            ("tail.local", postretro_entities::ReplicationScope::None),
            ("tail.entry", postretro_entities::ReplicationScope::None),
        ] {
            ctx.slot_table
                .borrow_mut()
                .insert(
                    slot.into(),
                    postretro_entities::SlotRecord::new(postretro_entities::SlotSchema {
                        slot_type: if slot == "tail.entry" {
                            postretro_entities::SlotType::String
                        } else {
                            postretro_entities::SlotType::Number
                        },
                        default: Some(if slot == "tail.entry" {
                            postretro_entities::SlotValue::String("typed".into())
                        } else {
                            postretro_entities::SlotValue::Number(0.0)
                        }),
                        range: None,
                        persist: false,
                        readonly: false,
                        ownership: postretro_entities::SlotOwnership::Mod,
                        network,
                        per_owner: false,
                        accumulate: None,
                    }),
                )
                .unwrap();
        }
    });
    counters
}
fn note_step() -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Entity(EntityId::from_raw(0)),
        primitive: "note".into(),
        args: json!({}),
    }
}
fn fire_step(event: &str) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Fire,
        primitive: "fire".into(),
        args: json!({ "event": event }),
    }
}
fn group_update() -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Group(postretro_entities::GroupTarget {
            kind: postretro_entities::GroupKind::Npc,
            tag: None,
        }),
        primitive: "updateNpcState".into(),
        args: json!({ "aggro": true }),
    }
}
fn number(app: &App, slot: &str) -> f32 {
    match script_ctx(app)
        .slot_table
        .borrow()
        .get(slot)
        .and_then(|record| record.value.clone())
    {
        Some(postretro_entities::SlotValue::Number(value)) => value,
        other => panic!("{slot} is {other:?}"),
    }
}
/// The client-local map light tagged `prl`, by its install-order id, which
/// every machine shares because each installs the same level the same way.
fn prl_light() -> EntityId {
    let probe = installed_with_reactions(&fixture_world(), false, Vec::new());
    tagged(&script_ctx(&probe), "prl")
}
fn fade(light: EntityId) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Entity(light),
        primitive: "setLightAnimation".into(),
        args: json!({ "periodMs": 500, "playCount": 1, "brightness": [1.0, 0.2], "startActive": true }),
    }
}
fn captured(app: &App, light: EntityId) -> bool {
    script_ctx(app)
        .presentation_overrides
        .borrow()
        .records
        .contains_key(&(light, ComponentKind::Light))
}

// S9: a host-mirrored tail on a client plays its sound and dispatches neither
// the member step nor the group command, which reach it by replication; the
// host lands all three, presenting the sound locally.
#[test]
fn s9_host_mirrored_tail_lands_only_machine_local_steps_on_the_client() {
    let notes = counted_notes(&fixture_world());
    let mut h = Harness::with_shared_reactions(
        fixture_world(),
        vec![body(
            "levelLoad",
            vec![
                wait_step(500.0),
                group_update(),
                system_step("playSound", json!({ "sound": "x" })),
                note_step(),
            ],
        )],
    );
    let capture = LogCapture::start();
    h.advance(60);
    assert_eq!(
        (notes[0].get(), plays(&h.host_commands, "x")),
        (1, 1),
        "the host lands every step"
    );
    assert_eq!(
        (notes[1].get(), plays(&h.peers[0].commands, "x")),
        (0, 1),
        "the client plays `x` and leaves the member step to the host"
    );
    assert_tail_quiet(&capture);
}

// S10 (second form): a `fire` from the tail lands through the same filter, so
// each machine hears `x` once, from its own landing.
#[test]
fn s10_fire_from_a_level_load_tail_plays_once_on_each_machine() {
    let mut h = Harness::with_shared_reactions(
        fixture_world(),
        vec![
            body("levelLoad", vec![wait_step(500.0), fire_step("r")]),
            body("r", vec![system_step("playSound", json!({ "sound": "x" }))]),
        ],
    );
    let capture = LogCapture::start();
    h.advance(60);
    assert_eq!(plays(&h.host_commands, "x"), 1);
    assert_eq!(plays(&h.peers[0].commands, "x"), 1);
    assert_tail_quiet(&capture);
}

// S11: a member light step in a `levelLoad` landing is captured as a host-only
// override, inline and through `fire`; the sound plays once per machine.
#[test]
fn s11_member_step_in_a_landing_is_captured_on_the_host_and_replicates() {
    let light = prl_light();
    for reactions in [
        vec![body(
            "levelLoad",
            vec![
                wait_step(500.0),
                fade(light),
                system_step("playSound", json!({ "sound": "x" })),
            ],
        )],
        vec![
            body("levelLoad", vec![wait_step(500.0), fire_step("r")]),
            body(
                "r",
                vec![
                    fade(light),
                    system_step("playSound", json!({ "sound": "x" })),
                ],
            ),
        ],
    ] {
        let mut h = Harness::with_shared_reactions(fixture_world(), reactions);
        h.advance(32);
        assert!(captured(&h.host, light), "the host records the override");
        let before = h.peers[0].sample();
        h.advance(20);
        let host = sample(&mut h.host, h.clock);
        let client = h.peers[0].sample();
        assert_ne!(client, before, "the client's light changes by replication");
        assert!(
            host.iter().zip(&client).all(|(a, b)| (a - b).abs() < 0.05),
            "host {host:?} and client {client:?} agree"
        );
        assert_eq!(plays(&h.host_commands, "x"), 1);
        assert_eq!(plays(&h.peers[0].commands, "x"), 1);
    }
}

// S38: a tail with nothing machine-local parks nothing on the client and logs
// nothing above debug; the host lands it.
#[test]
fn s38_client_parks_nothing_for_a_tail_with_nothing_to_land() {
    let notes = counted_notes(&fixture_world());
    let capture = LogCapture::start();
    let mut h = Harness::with_shared_reactions(
        fixture_world(),
        vec![body(
            "levelLoad",
            vec![wait_step(500.0), note_step(), group_update()],
        )],
    );
    assert_eq!(pending(&h.host), 1);
    assert_eq!(pending(&h.peers[0].app), 0, "the client parks nothing");
    h.advance(60);
    assert_eq!((notes[0].get(), notes[1].get()), (1, 0));
    assert_tail_quiet(&capture);
}

// S36: the client drops a replicated write, so a later step reads the
// client's own value at its own landing rather than waiting for replication.
// The host's write reaching the client is slot replication's own contract.
#[test]
fn s36_client_reads_across_its_own_dropped_write() {
    let _notes = counted_notes(&fixture_world());
    let mut h = Harness::with_shared_reactions(
        fixture_world(),
        vec![body(
            "levelLoad",
            vec![
                wait_step(500.0),
                system_step("setState", json!({ "slot": "tail.shared", "value": 1 })),
                system_step(
                    "setState",
                    json!({ "slot": "tail.local", "value": { "op": "input", "name": "tail.shared" } }),
                ),
            ],
        )],
    );
    h.apply_slot_writes = true;
    for app in std::iter::once(&mut h.host).chain(h.peers.iter_mut().map(|peer| &mut peer.app)) {
        let ctx = script_ctx(app);
        let session = app.session.as_mut().unwrap();
        session
            .scripting
            .system_reaction_ir_bindings
            .rebuild(&ctx.data_registry.borrow(), &ctx);
    }
    let capture = LogCapture::start();
    h.advance(31);
    assert_eq!(number(&h.host, "tail.shared"), 1.0);
    assert_eq!(
        number(&h.host, "tail.local"),
        1.0,
        "the host reads its own write"
    );
    h.advance(1);
    assert_eq!(
        number(&h.peers[0].app, "tail.local"),
        0.0,
        "the client dropped the shared write and read its own value at landing"
    );
    assert!(
        !h.peers[0].commands.iter().any(|command| matches!(
            command,
            SystemReactionCommand::SetState { slot, .. } if slot == "tail.shared"
        )),
        "the client never writes the shared slot itself; it is the host's to replicate"
    );
    assert_tail_quiet(&capture);
}

/// Fire `reaction` from a crossing over the replicated `tail.shared` slot on
/// every connected machine, as each machine's crossing detector does when the
/// host's write replicates.
fn cross_everywhere(h: &mut Harness, reaction: &str) {
    for app in std::iter::once(&h.host).chain(
        h.peers
            .iter()
            .filter(|peer| peer.active)
            .map(|peer| &peer.app),
    ) {
        let ctx = script_ctx(app);
        {
            let mut data = ctx.data_registry.borrow_mut();
            if data.crossings.is_empty() {
                data.crossings.push(postretro_entities::CrossingDescriptor {
                    slot: Some("tail.shared".into()),
                    condition: postretro_entities::CrossingCondition::Above { threshold: 0.5 },
                    max: 1.0,
                    edge: None,
                    fire: vec![reaction.into()],
                });
            }
        }
        let scripting = &app.session.as_ref().unwrap().scripting;
        let _ = postretro_sim::scripting::reactions::dispatch_crossing_fires_with_sequences(
            vec![postretro_scripting_core::state_crossings::CrossingFire {
                reaction: reaction.into(),
                rising: true,
                source_id: 0,
            }],
            &ctx.data_registry.borrow(),
            &scripting.sequence_registry,
            &scripting.reaction_registry,
            &scripting.system_registry,
            &ctx,
        );
    }
}
fn r_tail(sound: &str, wait_ms: f64) -> NamedReaction {
    body(
        "r",
        vec![
            wait_step(wait_ms),
            system_step("playSound", json!({ "sound": sound })),
        ],
    )
}

// S12: a trigger's `fire(r)` and a shared-state crossing's fire of `r` inside
// its wait park separately, so each machine plays `x` twice, as it would with
// `x` before the wait.
#[test]
fn s12_one_reaction_under_two_source_classes_parks_twice() {
    let _notes = counted_notes(&fixture_world());
    let mut h = Harness::with_shared_reactions(
        fixture_world(),
        vec![body("plate", vec![fire_step("r")]), r_tail("x", 2000.0)],
    );
    h.advance(10);
    h.fire_plate(100.0, &[1]);
    h.advance(30);
    cross_everywhere(&mut h, "r");
    h.advance(150);
    assert_eq!(
        plays(&h.host_commands, "x"),
        2,
        "the host lands both instances"
    );
    assert_eq!(
        plays(&h.peers[0].commands, "x"),
        2,
        "the trigger's broadcast and its own landing"
    );
}

// S31: two shared-state crossing fires under one class and clock share an
// instance: the second, inside the first's wait, is a re-fire and ignored.
#[test]
fn s31_two_crossings_under_one_class_and_clock_share_an_instance() {
    let _notes = counted_notes(&fixture_world());
    let mut h = Harness::with_shared_reactions(fixture_world(), vec![r_tail("x", 2000.0)]);
    h.advance(10);
    let capture = LogCapture::start();
    cross_everywhere(&mut h, "r");
    h.advance(30);
    cross_everywhere(&mut h, "r");
    h.advance(150);
    assert_eq!(plays(&h.host_commands, "x"), 1);
    assert_eq!(
        plays(&h.peers[0].commands, "x"),
        1,
        "from its own landing, nothing forwarded"
    );
    assert_tail_quiet(&capture);
}

// S33 (`levelLoad` form): a `fire`-seeded tail keeps its parent's class and
// clock: the host captures the light, the client gets it by replication, and
// `x` plays once per machine.
#[test]
fn s33_fire_seeded_level_load_tail_keeps_its_class() {
    let light = prl_light();
    let mut h = Harness::with_shared_reactions(
        fixture_world(),
        vec![
            body("levelLoad", vec![wait_step(500.0), fire_step("r")]),
            body(
                "r",
                vec![
                    wait_step(500.0),
                    fade(light),
                    system_step("playSound", json!({ "sound": "x" })),
                ],
            ),
        ],
    );
    h.advance(58);
    assert_eq!(plays(&h.host_commands, "x"), 0, "1000 ms is 60 ticks");
    h.advance(4);
    assert!(captured(&h.host, light));
    assert_eq!(plays(&h.host_commands, "x"), 1);
    h.advance(10);
    assert_eq!(plays(&h.peers[0].commands, "x"), 1);
}

// AC 9: after a wait a client runs none of a replicated write, the ghost bus
// or a level load, and a `fire` lands only its machine-local steps; the host
// runs every one. UI-stack and local text edits land on each machine.
#[test]
fn client_landing_drops_host_consequences_and_lands_machine_local_steps() {
    let _notes = counted_notes(&fixture_world());
    let mut h = Harness::with_shared_reactions(
        fixture_world(),
        vec![
            body(
                "levelLoad",
                vec![
                    wait_step(500.0),
                    system_step("setState", json!({ "slot": "tail.shared", "value": 1 })),
                    system_step("setGhostAudio", json!({ "volume": 0.5 })),
                    system_step("loadLevel", json!({ "map": "next" })),
                    fire_step("r"),
                ],
            ),
            body(
                "r",
                vec![
                    system_step("setState", json!({ "slot": "tail.shared", "value": 2 })),
                    system_step("playSound", json!({ "sound": "y" })),
                ],
            ),
            body(
                "levelLoad",
                vec![
                    wait_step(3000.0),
                    system_step("showDialog", json!({ "tree": "objective" })),
                    system_step("setState", json!({ "slot": "tail.entry", "value": "" })),
                ],
            ),
        ],
    );
    h.advance(200);
    let kinds = |commands: &[SystemReactionCommand]| -> Vec<String> {
        commands
            .iter()
            .map(|command| match command {
                SystemReactionCommand::SetState { slot, .. } => format!("setState {slot}"),
                SystemReactionCommand::SetGhostAudio(_) => "setGhostAudio".into(),
                SystemReactionCommand::LoadLevel { .. } => "loadLevel".into(),
                SystemReactionCommand::PlaySound { sound, .. } => format!("playSound {sound}"),
                SystemReactionCommand::PushTree { tree, .. } => format!("showDialog {tree}"),
                other => format!("{other:?}"),
            })
            .collect()
    };
    assert_eq!(
        kinds(&h.host_commands),
        [
            "setState tail.shared",
            "setGhostAudio",
            "loadLevel",
            "setState tail.shared",
            "playSound y",
            "showDialog objective",
            "setState tail.entry",
        ]
    );
    assert_eq!(
        kinds(&h.peers[0].commands),
        ["playSound y", "showDialog objective", "setState tail.entry"]
    );
}

/// Fire `reaction` from a crossing on `player.health` (and, with `mixed`, a
/// replicated slot too) on one machine only: each machine crosses for its own
/// player.
fn cross_own(app: &App, reaction: &str, mixed: bool) {
    let ctx = script_ctx(app);
    {
        let mut data = ctx.data_registry.borrow_mut();
        data.crossings.clear();
        let health_low = postretro_foundation::IrNode::Lt {
            a: Box::new(postretro_foundation::IrNode::Input {
                name: "player.health".into(),
                owner: None,
            }),
            b: Box::new(postretro_foundation::IrNode::Const {
                value: postretro_foundation::ir::IrValue::Number(25.0),
            }),
        };
        let condition = if mixed {
            postretro_foundation::IrNode::And {
                a: Box::new(health_low),
                b: Box::new(postretro_foundation::IrNode::Eq {
                    a: Box::new(postretro_foundation::IrNode::Input {
                        name: "tail.shared".into(),
                        owner: None,
                    }),
                    b: Box::new(postretro_foundation::IrNode::Const {
                        value: postretro_foundation::ir::IrValue::Number(1.0),
                    }),
                }),
            }
        } else {
            health_low
        };
        data.crossings.push(postretro_entities::CrossingDescriptor {
            slot: None,
            condition: postretro_entities::CrossingCondition::Ir(condition),
            max: 1.0,
            edge: None,
            fire: vec![reaction.into()],
        });
    }
    let scripting = &app.session.as_ref().unwrap().scripting;
    let _ = postretro_sim::scripting::reactions::dispatch_crossing_fires_with_sequences(
        vec![postretro_scripting_core::state_crossings::CrossingFire {
            reaction: reaction.into(),
            rising: true,
            source_id: 0,
        }],
        &ctx.data_registry.borrow(),
        &scripting.sequence_registry,
        &scripting.reaction_registry,
        &scripting.system_registry,
        &ctx,
    );
}
fn own_state_warnings(capture: &LogCapture) -> Vec<String> {
    capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level == log::Level::Warn && record.message.contains("own-state tail")
        })
        .map(|record| record.message)
        .collect()
}

// S13: an own-state crossing's tail plays on the machine that crossed, from
// its own landing; the host never sends it.
#[test]
fn s13_own_state_crossing_tail_plays_on_the_machine_that_crossed() {
    let _notes = counted_notes(&fixture_world());
    let vignette = system_step("vignette", json!({ "strength": 0.5, "durationMs": 100 }));
    let mut h = Harness::with_shared_reactions(
        fixture_world(),
        vec![body(
            "low",
            vec![
                vignette,
                wait_step(600.0),
                system_step("playSound", json!({ "sound": "x" })),
            ],
        )],
    );
    h.advance(10);
    cross_own(&h.peers[0].app, "low", false);
    h.advance(45);
    assert_eq!(
        (
            plays(&h.host_commands, "x"),
            plays(&h.peers[0].commands, "x")
        ),
        (0, 1)
    );
    cross_own(&h.host, "low", false);
    h.advance(45);
    assert_eq!(
        (
            plays(&h.host_commands, "x"),
            plays(&h.peers[0].commands, "x")
        ),
        (1, 1),
        "the host's crossing plays on the host alone"
    );
}

// S28, S33 (crossing form) and S37: an own-state tail's member step on a
// client-local light runs on the host's own crossing uncaptured, so no client
// sees the host's fade; a client's crossing drops its fade and warns once,
// naming the reaction that landed and the step. A crossing that also reads a
// replicated slot is still own-state.
#[test]
fn own_state_tail_member_step_stays_on_the_machine_that_crossed() {
    let light = prl_light();
    let cases: [(&str, bool, Vec<NamedReaction>, &str); 3] = [
        (
            "S28",
            false,
            vec![body("low", vec![wait_step(600.0), fade(light)])],
            "reaction `low`: step `setLightAnimation`",
        ),
        (
            "S33",
            false,
            vec![
                body("low", vec![wait_step(100.0), fire_step("r")]),
                body(
                    "r",
                    vec![
                        wait_step(500.0),
                        fade(light),
                        system_step("playSound", json!({ "sound": "x" })),
                    ],
                ),
            ],
            "reaction `r`: step `setLightAnimation`",
        ),
        (
            "S37",
            true,
            vec![body(
                "low",
                vec![
                    wait_step(600.0),
                    fade(light),
                    system_step("playSound", json!({ "sound": "x" })),
                ],
            )],
            "reaction `low`: step `setLightAnimation`",
        ),
    ];
    for (pin, mixed, reactions, warned) in cases {
        let _notes = counted_notes(&fixture_world());
        let mut h = Harness::with_shared_reactions(fixture_world(), reactions);
        h.advance(10);
        let capture = LogCapture::start();
        let client_before = h.peers[0].sample();
        cross_own(&h.host, "low", mixed);
        h.advance(50);
        assert!(
            !captured(&h.host, light),
            "{pin}: the host records no override"
        );
        assert_eq!(
            h.peers[0].sample(),
            client_before,
            "{pin}: no client sees the host's fade"
        );
        assert!(
            own_state_warnings(&capture).is_empty(),
            "{pin}: the host never warns"
        );

        cross_own(&h.peers[0].app, "low", mixed);
        h.advance(50);
        let warnings = own_state_warnings(&capture);
        assert_eq!(warnings.len(), 1, "{pin}: {warnings:?}");
        assert!(warnings[0].contains(warned), "{pin}: {warnings:?}");
        if pin != "S28" {
            assert_eq!(
                plays(&h.host_commands, "x"),
                1,
                "{pin}: for the host's own crossing"
            );
            assert_eq!(
                plays(&h.peers[0].commands, "x"),
                1,
                "{pin}: for the client's"
            );
        }
    }
}

// S40: an own-state tail's step on a networked light is captured, as the same
// step is before the wait, so every client sees the host's fade; none warns.
#[test]
fn s40_own_state_tail_on_a_networked_light_is_captured() {
    let lit = {
        let probe = installed_with_reactions(&fixture_world(), false, Vec::new());
        tagged(&script_ctx(&probe), "lit_prop")
    };
    let _notes = counted_notes(&fixture_world());
    let mut h = Harness::with_shared_reactions(
        fixture_world(),
        vec![body("low", vec![wait_step(600.0), fade(lit)])],
    );
    h.advance(10);
    let capture = LogCapture::start();
    cross_own(&h.host, "low", false);
    h.advance(40);
    assert!(
        captured(&h.host, lit),
        "the networked light's change is captured"
    );
    assert!(own_state_warnings(&capture).is_empty());
}

/// The shipped `ghost-links` level script's reactions, evaluated in one
/// runtime with no map: member lists come back empty, so only the bodies'
/// group, control and system steps remain.
fn ghost_links_reactions(luau: bool) -> Vec<NamedReaction> {
    use postretro_level_format::data_script::DataScriptSection;
    use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
    use postretro_scripting_core::runtime::{ScriptRuntime, ScriptRuntimeConfig};

    let scripts =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev/scripts");
    let entry = scripts.join(if luau {
        "ghost-links.luau"
    } else {
        "ghost-links.ts"
    });
    let bundled = if luau {
        std::fs::read_to_string(&entry).expect("the Luau twin reads")
    } else {
        postretro_script_compiler::bundle_entry(&entry).expect("the TS twin bundles")
    };
    let ctx = ScriptCtx::new();
    let mut primitives = PrimitiveRegistry::new();
    crate::scripting::primitives::register_all(&mut primitives, ctx.clone());
    let runtime = ScriptRuntime::new(&primitives, &ScriptRuntimeConfig::default(), &ctx)
        .expect("script runtime constructs");
    runtime
        .run_data_script(
            &DataScriptSection {
                compiled_bytes: bundled.into_bytes(),
                source_path: entry.to_string_lossy().into_owned(),
            },
            &scripts,
        )
        .reactions
        .into_iter()
        .filter(|reaction| ["levelLoad", "closet.a1"].contains(&reaction.name.as_str()))
        .collect()
}

// AC 20: the ghost-links north-closet klaxon, authored after the timer's
// `wait(15000)`, sounds once on the host and once on the client connected at
// 15 s, on the host's timeline; a client that joins afterwards never hears
// it. No machine refuses the tail's enrollment.
#[test]
fn ghost_links_klaxon_plays_once_per_connected_machine_and_never_for_a_late_joiner() {
    let reactions = ghost_links_reactions(false);
    assert_eq!(reactions.len(), 2, "levelLoad and closet.a1 install");
    assert_eq!(reactions, ghost_links_reactions(true), "the twins agree");
    // `closet.a1` wakes its knights through the npc group verb.
    set_install_hook(|_, scripting, _| {
        postretro_sim::scripting::reactions::registry::register_npc_state_reaction_primitives(
            &mut scripting.reaction_registry,
        );
    });
    let capture = LogCapture::start();
    let mut h = Harness::with_shared_reactions(fixture_world(), reactions);
    assert_eq!(pending(&h.host), 1, "the host parks the timer's tail");
    assert_eq!(pending(&h.peers[0].app), 1, "the client parks its own copy");

    // 15 s is 900 ticks; nothing sounds a tick early.
    h.advance(898);
    assert_eq!(plays(&h.host_commands, "sfx/test_tone"), 0);
    assert_eq!(plays(&h.peers[0].commands, "sfx/test_tone"), 0);
    h.advance(60);
    assert_eq!(
        plays(&h.host_commands, "sfx/test_tone"),
        1,
        "the host hears it once"
    );
    assert_eq!(
        plays(&h.peers[0].commands, "sfx/test_tone"),
        1,
        "the connected client hears it once, from its own landing"
    );

    h.connect_late();
    h.advance(120);
    assert_eq!(
        plays(&h.peers[1].commands, "sfx/test_tone"),
        0,
        "the late joiner never hears it"
    );
    assert_eq!(plays(&h.host_commands, "sfx/test_tone"), 1);
    assert_eq!(plays(&h.peers[0].commands, "sfx/test_tone"), 1);
    capture.assert_not_logged(log::Level::Warn, "enrollment refused");
    assert_tail_quiet(&capture);
}
