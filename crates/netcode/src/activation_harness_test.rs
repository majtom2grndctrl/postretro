//! Timing proof through production input conversion, authoritative playout, and
//! connected-client catch-up. Fixtures author ordinary shot/wait data.
use super::*;
use postretro_combat_model::activation::{advance_activation, start_activation};
use postretro_foundation::{
    ActivationInput, ActivationLane, ActivationProgram, ActivationRelease, ActivationStep,
    ActivationToken, ChargeTiming,
};
use postretro_sim::weapon::activation_prediction::ClientActivationTiming;

fn program(charge: bool) -> ActivationProgram {
    ActivationProgram::new(
        vec![
            ActivationStep::Shot,
            ActivationStep::Wait { ticks: 2 },
            ActivationStep::Shot,
            ActivationStep::Wait { ticks: 2 },
            ActivationStep::Shot,
        ],
        charge.then_some(ChargeTiming {
            min_ticks: 2,
            full_ticks: 6,
        }),
        18,
    )
    .unwrap()
}
fn command(tick: u32, activation: ActivationInput) -> wire::InputCommand {
    let command = SimCommand {
        input_tick: 0,
        movement: crate::movement::MovementInput {
            wish_dir: glam::Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            use_pressed: false,
            drop_pressed: false,
        },
        fire_button: weapon::FireButtonState {
            pressed: false,
            active: false,
        },
        secondary_button: weapon::FireButtonState {
            pressed: activation.initiation.is_some(),
            active: true,
        },
        activation,
        reload: false,
        firing_slot: 0,
        select_slot: None,
        use_pressed: false,
        drop_pressed: false,
    };
    wire_convert::sim_command_to_input(&command, tick, 0.0)
}
fn token(start_tick: u32) -> ActivationToken {
    ActivationToken {
        start_tick,
        lane: ActivationLane::Secondary,
    }
}

fn queued_live_activation_fixture(
    charge: bool,
) -> (
    HostCommandQueues,
    ActivationProgram,
    postretro_foundation::ActivationCursor,
    postretro_foundation::ActivationId,
) {
    let program = program(charge);
    let mut queues = HostCommandQueues::new();
    for tick in 100..=101 {
        assert!(queues.ingest(
            7,
            &command(
                tick,
                ActivationInput {
                    initiation: (tick == 100).then_some(token(100)),
                    ..ActivationInput::default()
                }
            )
        ));
    }
    let initial = queues.resolve_tick(7).unwrap();
    assert_eq!(initial.source, command_queue::ResolutionSource::Real);
    let id = postretro_foundation::ActivationId {
        pawn: 4,
        token: initial.command.activation.initiation.unwrap(),
    };
    assert!(queues.activations.accept(
        7,
        id,
        postretro_entities::EntityId::from_raw(9),
        &program,
        100
    ));
    let mut cursor = start_activation(id.token, id.pawn, 100, &program);
    let first = advance_activation(&mut cursor, &program, 100, true, initial.command.activation);
    if charge {
        assert!(first.shot.is_none());
    } else {
        assert_eq!(first.shot.unwrap().shot_id.ordinal, 0);
    }
    let waiting = queues.resolve_tick(7).unwrap();
    assert_eq!(waiting.source, command_queue::ResolutionSource::Real);
    assert!(
        advance_activation(&mut cursor, &program, 101, true, waiting.command.activation)
            .shot
            .is_none()
    );
    (queues, program, cursor, id)
}

#[test]
fn activation_fixture_real_queue_and_client_catch_up_preserve_every_ordinal() {
    let program = program(false);
    let start = token(u32::MAX - 1);
    let mut queues = HostCommandQueues::new();
    for offset in 0..6 {
        let tick = start.start_tick.wrapping_add(offset);
        assert!(queues.ingest(
            7,
            &command(
                tick,
                ActivationInput {
                    initiation: (offset == 0).then_some(start),
                    ..ActivationInput::default()
                }
            )
        ));
    }
    let mut client = ClientActivationTiming;
    let mut client_state = postretro_entities::components::wieldable_state::WieldableState::Idle;
    assert!(client.start(&mut client_state, start, 4, 100, &program));
    let mut host = None;
    let mut host_shots = Vec::new();
    let mut client_shots = Vec::new();
    // One rendered frame catches up all six ticks. Every due shot reaches the caller.
    for offset in 0..6 {
        let tick = 100 + offset;
        let resolved = queues.resolve_tick(7).unwrap();
        if let Some(initiation) = resolved.command.activation.initiation {
            assert_eq!(resolved.source, command_queue::ResolutionSource::Real);
            host = Some(start_activation(initiation, 4, tick, &program));
        }
        if let Some(cursor) = host.as_mut() {
            let result = advance_activation(
                cursor,
                &program,
                tick,
                resolved.source == command_queue::ResolutionSource::Real,
                resolved.command.activation,
            );
            if let Some(shot) = result.shot {
                host_shots.push(shot.shot_id);
            }
        }
        let result = client.tick(
            &mut client_state,
            &program,
            tick,
            ActivationInput::default(),
        );
        if let Some(shot) = result.shot {
            client_shots.push(shot.shot_id);
        }
    }
    assert_eq!(host_shots, client_shots);
    assert_eq!(
        host_shots.iter().map(|s| s.ordinal).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert!(
        host_shots
            .iter()
            .all(|s| s.start_tick == start.start_tick && s.lane == ActivationLane::Secondary)
    );
    assert!(
        client
            .tick(&mut client_state, &program, 106, ActivationInput::default())
            .shot
            .is_none()
    );
}

#[test]
fn activation_release_received_before_start_and_stale_drop_is_delivered_once() {
    let mut queues = HostCommandQueues::new();
    let start = token(10);
    // Reordering may deliver the correlated edge before the real start. It must wait.
    let release = ActivationRelease {
        token: start,
        release_tick: 16,
    };
    assert!(queues.ingest(
        7,
        &command(
            16,
            ActivationInput {
                release: Some(release),
                ..ActivationInput::default()
            }
        )
    ));
    assert!(!queues.ingest(
        7,
        &command(
            16,
            ActivationInput {
                release: Some(release),
                ..ActivationInput::default()
            }
        )
    ));
    // Bootstrap's reliable-order invariant admits only forward ticks; use a second
    // stream whose first command anchors start, then receive release before playout.
    queues = HostCommandQueues::new();
    assert!(queues.ingest(
        7,
        &command(
            10,
            ActivationInput {
                initiation: Some(start),
                ..ActivationInput::default()
            }
        )
    ));
    assert!(queues.ingest(
        7,
        &command(
            16,
            ActivationInput {
                release: Some(release),
                ..ActivationInput::default()
            }
        )
    ));
    let first = queues.resolve_tick(7).unwrap();
    assert_eq!(first.command.activation.release, Some(release));
    assert!(!queues.ingest(
        7,
        &command(
            10,
            ActivationInput {
                cancel: Some(start),
                ..ActivationInput::default()
            }
        )
    ));
    let held = queues.resolve_tick(7).unwrap();
    assert!(held.command.activation.initiation.is_none());
    assert!(held.command.activation.release.is_none());
    assert_eq!(held.command.activation.cancel, Some(start));
    assert!(
        queues
            .resolve_tick(7)
            .unwrap()
            .command
            .activation
            .cancel
            .is_none()
    );
}

#[test]
fn activation_competing_release_delivers_live_cancel_before_due_shot() {
    let program = program(false);
    let live = token(10);
    let competing = token(12);
    let id = postretro_foundation::ActivationId {
        pawn: 4,
        token: live,
    };
    let mut queues = HostCommandQueues::new();
    assert!(queues.ingest(
        7,
        &command(
            10,
            ActivationInput {
                initiation: Some(live),
                ..ActivationInput::default()
            }
        )
    ));
    assert!(queues.ingest(7, &command(11, ActivationInput::default())));
    let initial = queues.resolve_tick(7).unwrap();
    assert_eq!(initial.source, command_queue::ResolutionSource::Real);
    assert_eq!(initial.command.activation.initiation, Some(live));
    assert!(queues.activations.accept(
        7,
        id,
        postretro_entities::EntityId::from_raw(9),
        &program,
        100
    ));
    let mut cursor = start_activation(live, id.pawn, 100, &program);
    let first = advance_activation(&mut cursor, &program, 100, true, initial.command.activation);
    assert_eq!(first.shot.unwrap().shot_id.ordinal, 0);
    let waiting = queues.resolve_tick(7).unwrap();
    assert!(
        advance_activation(
            &mut cursor,
            &program,
            101,
            waiting.source == command_queue::ResolutionSource::Real,
            waiting.command.activation,
        )
        .shot
        .is_none()
    );

    // Regression: the competing release consumed the live cancellation's delivery
    // slot, allowing the already-due second shot before cancellation on the next tick.
    let input = ActivationInput {
        initiation: Some(competing),
        release: Some(ActivationRelease {
            token: competing,
            release_tick: competing.start_tick,
        }),
        cancel: Some(live),
    };
    assert!(queues.ingest(7, &command(competing.start_tick, input)));
    let mut resolved = queues.resolve_tick(7).unwrap();
    assert_eq!(resolved.source, command_queue::ResolutionSource::Real);
    assert_eq!(resolved.command.activation.initiation, Some(competing));
    assert_eq!(resolved.command.activation.release, input.release);
    assert_eq!(resolved.command.activation.cancel, Some(live));
    // Command admission precedes the host's concurrency guard. B is refused while
    // the correlated cancellation must still reach A in this same fixed tick.
    assert!(!queues.activations.can_accept(
        7,
        postretro_foundation::ActivationId {
            pawn: id.pawn,
            token: competing,
        },
        102
    ));
    resolved.command.activation.initiation = None;
    let cancelled = advance_activation(
        &mut cursor,
        &program,
        102,
        true,
        resolved.command.activation,
    );
    assert!(cancelled.shot.is_none());
    assert_eq!(
        cancelled.terminal,
        Some(postretro_combat_model::activation::ActivationTermination::Cancelled)
    );
    assert_eq!(cursor.ordinal, 1);
    assert_eq!(
        queues.resolve_tick(7).unwrap().command.activation,
        ActivationInput::default()
    );
}

#[test]
fn activation_competing_cancel_cannot_delay_live_cancel_before_due_shot() {
    let (mut queues, program, mut cursor, live) = queued_live_activation_fixture(false);
    let competing = postretro_foundation::ActivationId {
        pawn: live.pawn,
        token: ActivationToken {
            start_tick: 102,
            lane: ActivationLane::Primary,
        },
    };
    // Regression: B's earlier cancellation occupied the cancel slot even though
    // the ledger owned A, whose cancellation had also arrived before its due shot.
    assert!(queues.ingest(
        7,
        &command(
            102,
            ActivationInput {
                initiation: Some(competing.token),
                cancel: Some(competing.token),
                ..ActivationInput::default()
            }
        )
    ));
    assert!(queues.ingest(
        7,
        &command(
            103,
            ActivationInput {
                cancel: Some(live.token),
                ..ActivationInput::default()
            }
        )
    ));
    let mut resolved = queues.resolve_tick(7).unwrap();
    assert_eq!(resolved.source, command_queue::ResolutionSource::Real);
    assert_eq!(
        resolved.command.activation.initiation,
        Some(competing.token)
    );
    assert_eq!(resolved.command.activation.cancel, Some(live.token));
    assert!(!queues.activations.can_accept(7, competing, 102));
    resolved.command.activation.initiation = None;
    let cancelled = advance_activation(
        &mut cursor,
        &program,
        102,
        true,
        resolved.command.activation,
    );
    assert!(cancelled.shot.is_none());
    assert_eq!(
        cancelled.terminal,
        Some(postretro_combat_model::activation::ActivationTermination::Cancelled)
    );
    assert_eq!(cursor.ordinal, 1);
    queues.activations.terminal(7, competing, 102);
    queues.activation_terminal(7, competing.token);
    queues.activations.terminal(7, live, 102);
    queues.activation_terminal(7, live.token);
    assert_eq!(
        queues.resolve_tick(7).unwrap().command.activation,
        ActivationInput::default()
    );
}

#[test]
fn activation_competing_release_cannot_delay_live_charge_release() {
    let (mut queues, program, mut cursor, live) = queued_live_activation_fixture(true);
    let competing = postretro_foundation::ActivationId {
        pawn: live.pawn,
        token: ActivationToken {
            start_tick: 102,
            lane: ActivationLane::Primary,
        },
    };
    assert!(queues.ingest(
        7,
        &command(
            102,
            ActivationInput {
                initiation: Some(competing.token),
                release: Some(ActivationRelease {
                    token: competing.token,
                    release_tick: 102,
                }),
                ..ActivationInput::default()
            }
        )
    ));
    let release = ActivationRelease {
        token: live.token,
        release_tick: 103,
    };
    assert!(queues.ingest(
        7,
        &command(
            103,
            ActivationInput {
                release: Some(release),
                ..ActivationInput::default()
            }
        )
    ));
    let mut resolved = queues.resolve_tick(7).unwrap();
    assert_eq!(resolved.source, command_queue::ResolutionSource::Real);
    assert_eq!(resolved.command.activation.release, Some(release));
    assert!(!queues.activations.can_accept(7, competing, 102));
    resolved.command.activation.initiation = None;
    let released = advance_activation(
        &mut cursor,
        &program,
        102,
        true,
        resolved.command.activation,
    );
    assert_eq!(released.shot.unwrap().shot_id.activation(), live);
    assert!((released.execution_charge.unwrap() - 0.5).abs() < 1e-6);
    queues.activations.terminal(7, competing, 102);
    queues.activation_terminal(7, competing.token);
    assert_eq!(
        queues.resolve_tick(7).unwrap().command.activation,
        ActivationInput::default()
    );
}

#[test]
fn activation_live_overflow_cancel_precedes_competing_admitted_cancel() {
    let (mut queues, program, mut cursor, live) = queued_live_activation_fixture(false);
    let competing = postretro_foundation::ActivationId {
        pawn: live.pawn,
        token: ActivationToken {
            start_tick: 102,
            lane: ActivationLane::Primary,
        },
    };
    // Stale commands still feed retained edges. Fill the bound while B remains
    // unknown; overflowing A's later release must cancel the admitted live A.
    for offset in 0..64 {
        let unknown = if offset == 0 {
            competing.token
        } else {
            token(199 + offset)
        };
        assert!(!queues.ingest(
            7,
            &command(
                101,
                ActivationInput {
                    cancel: Some(unknown),
                    ..ActivationInput::default()
                }
            )
        ));
    }
    assert!(!queues.ingest(
        7,
        &command(
            101,
            ActivationInput {
                release: Some(ActivationRelease {
                    token: live.token,
                    release_tick: 102,
                }),
                ..ActivationInput::default()
            }
        )
    ));
    assert!(queues.ingest(
        7,
        &command(
            102,
            ActivationInput {
                initiation: Some(competing.token),
                ..ActivationInput::default()
            }
        )
    ));
    let mut resolved = queues.resolve_tick(7).unwrap();
    assert_eq!(resolved.source, command_queue::ResolutionSource::Real);
    assert_eq!(resolved.command.activation.cancel, Some(live.token));
    assert!(resolved.command.activation.release.is_none());
    assert!(!queues.activations.can_accept(7, competing, 102));
    resolved.command.activation.initiation = None;
    let cancelled = advance_activation(
        &mut cursor,
        &program,
        102,
        true,
        resolved.command.activation,
    );
    assert!(cancelled.shot.is_none());
    assert_eq!(
        cancelled.terminal,
        Some(postretro_combat_model::activation::ActivationTermination::Cancelled)
    );
    queues.activations.terminal(7, competing, 102);
    queues.activation_terminal(7, competing.token);
    queues.activations.terminal(7, live, 102);
    queues.activation_terminal(7, live.token);
    assert_eq!(
        queues.resolve_tick(7).unwrap().command.activation,
        ActivationInput::default()
    );
}

#[test]
fn activation_backlog_discarded_start_never_creates_deferred_execution() {
    let mut queues = HostCommandQueues::new();
    let start = token(1);
    for tick in 1..=12 {
        let input = ActivationInput {
            initiation: (tick == 1).then_some(start),
            release: (tick == 3).then_some(ActivationRelease {
                token: start,
                release_tick: 3,
            }),
            cancel: None,
        };
        assert!(queues.ingest(7, &command(tick, input)));
    }
    for _ in 0..8 {
        let resolved = queues.resolve_tick(7).unwrap();
        assert!(resolved.command.activation.initiation.is_none());
        assert!(resolved.command.activation.release.is_none());
    }
}

#[test]
fn activation_charge_survives_frontier_holds_and_neutral_gaps_without_release() {
    let program = program(true);
    let mut queues = HostCommandQueues::new();
    let start = token(10);
    queues.ingest(
        7,
        &command(
            10,
            ActivationInput {
                initiation: Some(start),
                ..ActivationInput::default()
            },
        ),
    );
    queues.ingest(7, &command(11, ActivationInput::default()));
    let real = queues.resolve_tick(7).unwrap();
    let mut cursor = start_activation(
        real.command.activation.initiation.unwrap(),
        4,
        100,
        &program,
    );
    for tick in 100..110 {
        let resolved = if tick == 100 {
            real.clone()
        } else {
            queues.resolve_tick(7).unwrap()
        };
        let result = advance_activation(
            &mut cursor,
            &program,
            tick,
            resolved.source == command_queue::ResolutionSource::Real,
            resolved.command.activation,
        );
        assert!(result.shot.is_none());
    }
    let release = ActivationRelease {
        token: start,
        release_tick: 16,
    };
    // Release command is stale to movement playout; its edge still arrives at the next tick.
    queues.ingest(
        7,
        &command(
            12,
            ActivationInput {
                release: Some(release),
                ..ActivationInput::default()
            },
        ),
    );
    let resolved = queues.resolve_tick(7).unwrap();
    let result = advance_activation(
        &mut cursor,
        &program,
        110,
        false,
        resolved.command.activation,
    );
    assert!((result.execution_charge.unwrap() - 1.0).abs() < 1e-6);
}

#[test]
fn activation_future_declaration_waits_for_ordinal_decision_and_cancellation_rejects_it() {
    let program = program(false);
    let start = token(10);
    let id = postretro_foundation::ActivationId {
        pawn: 4,
        token: start,
    };
    let mut queues = HostCommandQueues::new();
    queues.ingest(7, &command(100, ActivationInput::default()));
    queues.ingest(7, &command(101, ActivationInput::default()));
    queues.resolve_tick(7);
    assert!(queues.activations.accept(
        7,
        id,
        postretro_entities::EntityId::from_raw(9),
        &program,
        100
    ));
    let future = ShotId::from_parts(4, 10, ActivationLane::Secondary, 2);
    let mut pending = PendingHitDeclarations::new();
    assert!(pending.push_at(
        7,
        wire::HitDeclaration {
            shot_id: wire_convert::shot_id_to_wire(future),
            records: Vec::new()
        },
        100
    ));
    assert!(
        pending
            .drain_ready(&queues, &OpenAuthorizedShots::new(), 101)
            .is_empty()
    );
    assert_eq!(pending.len(), 1);
    queues.activations.terminal(7, id, 102);
    assert_eq!(
        pending
            .drain_ready(&queues, &OpenAuthorizedShots::new(), 102)
            .len(),
        1
    );
    assert_eq!(
        queues.activations.status(7, future, 102),
        activation_ledger::OrdinalStatus::Rejected
    );
}

#[test]
fn activation_pending_unknown_duplicate_expiry_and_overflow_reject_newest() {
    let mut pending = PendingHitDeclarations::new();
    let make = |tick| wire::HitDeclaration {
        shot_id: wire::WireShotId {
            pawn: 4,
            start_tick: tick,
            lane: 1,
            ordinal: 0,
        },
        records: Vec::new(),
    };
    assert!(pending.push_at(7, make(1), 0));
    assert!(pending.push_at(7, make(1), 119));
    assert_eq!(
        pending
            .drain_ready(&HostCommandQueues::new(), &OpenAuthorizedShots::new(), 120)
            .len(),
        1
    );
    for tick in 0..64 {
        assert!(pending.push_at(7, make(tick), 200));
    }
    assert!(!pending.push_at(7, make(64), 201));
    assert_eq!(pending.len(), 64);
}

#[test]
fn activation_queued_start_settled_by_unknown_edge_expiry_cannot_execute() {
    let mut queues = HostCommandQueues::new();
    let start = token(180);
    for tick in [0, 1, 180] {
        assert!(queues.ingest(
            7,
            &command(
                tick,
                ActivationInput {
                    initiation: (tick == 180).then_some(start),
                    ..ActivationInput::default()
                }
            )
        ));
    }
    // The later unknown token's edge expires while playout walks the older gap.
    assert!(queues.ingest(
        7,
        &command(
            201,
            ActivationInput {
                release: Some(ActivationRelease {
                    token: token(200),
                    release_tick: 201
                }),
                ..ActivationInput::default()
            }
        )
    ));
    let mut owners = MovementOwners::new();
    owners.set(postretro_entities::EntityId::from_raw(4), 7);
    for tick in 0..=180 {
        let resolved = host_resolve_remote_commands(&owners, &mut queues);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].client_tick, tick);
        assert!(resolved[0].command.activation.initiation.is_none());
        assert_eq!(
            resolved[0].rejected_activation,
            (tick == 180).then_some(start)
        );
        if tick == 180 {
            assert_eq!(resolved[0].source, command_queue::ResolutionSource::Real);
        }
    }
    assert!(queues.ingest(
        7,
        &command(
            181,
            ActivationInput {
                cancel: Some(start),
                ..ActivationInput::default()
            }
        )
    ));
    let resolved = host_resolve_remote_commands(&owners, &mut queues);
    assert!(resolved[0].command.activation.cancel.is_none());
    assert!(resolved[0].rejected_activation.is_none());
}
