//! Timing proof through production input conversion, authoritative playout, and
//! connected-client catch-up. Fixtures author ordinary shot/wait data.
use super::*;
use crate::activation_edges::RETENTION_TICKS;
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
    let resolved = queues.resolve_tick(7).unwrap();
    assert_eq!(resolved.source, command_queue::ResolutionSource::Real);
    // B waits in the retained lane while A is live, so neither B's start nor its
    // release competes; A's correlated cancellation reaches A in this same tick.
    assert!(resolved.command.activation.initiation.is_none());
    assert!(resolved.command.activation.release.is_none());
    assert_eq!(resolved.command.activation.cancel, Some(live));
    assert!(!queues.activations.can_accept(
        7,
        postretro_foundation::ActivationId {
            pawn: id.pawn,
            token: competing,
        },
        102
    ));
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
            start_tick: 103,
            lane: ActivationLane::Primary,
        },
    };
    // Regression: B's cancellation occupied the cancel slot even though the
    // ledger owned A, whose cancellation, stamped on its due shot's own tick,
    // had also arrived. A cancel waits for its own tick on A's clock, so A's
    // is stamped 102: a later one would first owe the client the shot at 102.
    assert!(queues.ingest(
        7,
        &command(
            102,
            ActivationInput {
                cancel: Some(live.token),
                ..ActivationInput::default()
            }
        )
    ));
    assert!(queues.ingest(
        7,
        &command(
            103,
            ActivationInput {
                initiation: Some(competing.token),
                cancel: Some(competing.token),
                ..ActivationInput::default()
            }
        )
    ));
    let resolved = queues.resolve_tick(7).unwrap();
    assert_eq!(resolved.source, command_queue::ResolutionSource::Real);
    assert!(
        resolved.command.activation.initiation.is_none(),
        "B waits in the retained lane while A is live"
    );
    assert_eq!(resolved.command.activation.cancel, Some(live.token));
    assert!(!queues.activations.can_accept(7, competing, 102));
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

// Regression: a start inside the catch-up trimmed prefix vanished, so its later
// HIT was denied as never fired.
#[test]
fn activation_backlog_trimmed_start_is_retained_and_delivered_once_with_its_release() {
    let mut queues = HostCommandQueues::new();
    let start = token(1);
    let release = ActivationRelease {
        token: start,
        release_tick: 3,
    };
    for tick in 1..=12 {
        let input = ActivationInput {
            initiation: (tick == 1).then_some(start),
            release: (tick == 3).then_some(release),
            cancel: None,
        };
        assert!(queues.ingest(7, &command(tick, input)));
    }
    let first = queues.resolve_tick(7).unwrap();
    assert_eq!(
        first.client_tick, 11,
        "movement playout still trims to the newest commands"
    );
    assert_eq!(first.command.activation.initiation, Some(start));
    assert_eq!(
        first.command.activation.release,
        Some(release),
        "the retained release follows its start on the same resolution"
    );
    // A stale retransmit of the delivered start cannot deliver it again.
    queues.ingest(
        7,
        &command(
            1,
            ActivationInput {
                initiation: Some(start),
                ..ActivationInput::default()
            },
        ),
    );
    for _ in 0..8 {
        let resolved = queues.resolve_tick(7).unwrap();
        assert!(resolved.command.activation.initiation.is_none());
        assert!(resolved.command.activation.release.is_none());
    }
}

#[test]
fn activation_retained_starts_wait_for_the_live_execution_and_leave_in_order() {
    let mut queues = HostCommandQueues::new();
    let program = program(false);
    for tick in 1..=12 {
        let input = ActivationInput {
            initiation: matches!(tick, 1 | 2).then_some(token(tick)),
            ..ActivationInput::default()
        };
        assert!(queues.ingest(7, &command(tick, input)));
    }
    let first = queues.resolve_tick(7).unwrap();
    assert_eq!(first.command.activation.initiation, Some(token(1)));
    let live = postretro_foundation::ActivationId {
        pawn: 4,
        token: token(1),
    };
    assert!(queues.activations.accept(
        7,
        live,
        postretro_entities::EntityId::from_raw(9),
        &program,
        100
    ));
    let waiting = queues.resolve_tick(7).unwrap();
    assert!(
        waiting.command.activation.initiation.is_none(),
        "a retained start never competes with the live execution"
    );
    queues.activations.terminal(7, live, 101);
    queues.activation_terminal(7, live.token);
    queues.ingest(7, &command(13, ActivationInput::default()));
    let next = queues.resolve_tick(7).unwrap();
    assert_eq!(next.command.activation.initiation, Some(token(2)));
    assert!(next.rejected_activation.is_none());
}

#[test]
fn activation_retained_start_expires_into_a_refusal_retention_ticks_after_first_due() {
    let mut queues = HostCommandQueues::new();
    for tick in 1..=3 {
        let input = ActivationInput {
            initiation: matches!(tick, 1 | 2).then_some(token(tick)),
            ..ActivationInput::default()
        };
        assert!(queues.ingest(7, &command(tick, input)));
    }
    let first = queues.resolve_tick(7).unwrap();
    assert_eq!(first.command.activation.initiation, Some(token(1)));
    let live = postretro_foundation::ActivationId {
        pawn: 4,
        token: token(1),
    };
    let mut charged = program(true);
    charged.charge = Some(ChargeTiming {
        min_ticks: 2,
        full_ticks: 600,
    });
    assert!(queues.activations.accept(
        7,
        live,
        postretro_entities::EntityId::from_raw(9),
        &charged,
        100
    ));
    // Token 2's command resolves on the loop's first resolution, where it first
    // waits behind the live charge. It has no cadence record, so only retention
    // ages it.
    let first_due = 0;
    let mut refused = Vec::new();
    for (resolution, tick) in (4..4 + RETENTION_TICKS + 20).enumerate() {
        queues.ingest(7, &command(tick, ActivationInput::default()));
        let resolved = queues.resolve_tick(7).unwrap();
        assert!(resolved.command.activation.initiation.is_none());
        refused.extend(
            resolved
                .rejected_activation
                .map(|token| (resolution, token)),
        );
    }
    assert_eq!(
        refused,
        vec![(first_due + RETENTION_TICKS as usize, token(2))],
        "an unadmitted start expires once, as a refusal, `RETENTION_TICKS` after it first became due"
    );
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

// Regression: an unknown edge for a later token aged out while a start below it
// waited in the lane; settling it raised the lane's watermark over that start,
// so the lane refused a start it never judged.
#[test]
fn activation_unknown_edge_expiry_cannot_settle_a_still_retained_start() {
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
    let unknown = token(200);
    assert!(queues.ingest(
        7,
        &command(
            201,
            ActivationInput {
                release: Some(ActivationRelease {
                    token: unknown,
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
        assert!(resolved[0].rejected_activation.is_none());
        assert!(resolved[0].command.activation.release.is_none());
        assert_eq!(
            resolved[0].command.activation.initiation,
            (tick == 180).then_some(start)
        );
        if tick == 180 {
            assert_eq!(resolved[0].source, command_queue::ResolutionSource::Real);
        }
    }
    // Once nothing it covers is retained, the expired edge settles its token:
    // a start naming it on its own command is never retained, so playout
    // neither delivers nor refuses it. Unsettled, it would fire on tick 200.
    assert!(queues.ingest(
        7,
        &command(
            unknown.start_tick,
            ActivationInput {
                initiation: Some(unknown),
                ..ActivationInput::default()
            }
        )
    ));
    let mut played_replay = false;
    for _ in 0..2 * RETENTION_TICKS {
        let resolved = host_resolve_remote_commands(&owners, &mut queues);
        assert!(resolved[0].command.activation.initiation.is_none());
        assert!(resolved[0].command.activation.release.is_none());
        assert!(resolved[0].rejected_activation.is_none());
        if resolved[0].client_tick == unknown.start_tick {
            assert_eq!(resolved[0].source, command_queue::ResolutionSource::Real);
            played_replay = true;
            break;
        }
    }
    assert!(
        played_replay,
        "playout must reach the replayed start's command"
    );
}

// Regression: after a late start's delivery, a cancel the client sent mid-burst
// reached the host on the delivery tick and cut the burst short of the shots
// the client had fired before cancelling.
#[test]
fn activation_cancel_stamped_mid_burst_waits_for_the_late_bursts_own_clock() {
    let program = program(false);
    let start = token(10);
    let id = postretro_foundation::ActivationId {
        pawn: 4,
        token: start,
    };
    let mut queues = HostCommandQueues::new();
    for tick in 0..2 {
        assert!(queues.ingest(7, &command(tick, ActivationInput::default())));
    }
    queues.resolve_tick(7).unwrap();
    // Shots at client ticks 10 and 12 precede the cancel at 13; the third, at
    // 14, never fired on the client. A stall delivers it all at once.
    for tick in 2..30 {
        let input = ActivationInput {
            initiation: (tick == 10).then_some(start),
            cancel: (tick == 13).then_some(start),
            ..ActivationInput::default()
        };
        assert!(queues.ingest(7, &command(tick, input)));
    }
    let mut cursor = None;
    let mut shots = Vec::new();
    let mut terminal = None;
    for (offset, next) in (30..42).enumerate() {
        let tick = 100 + offset as u32;
        queues.ingest(7, &command(next, ActivationInput::default()));
        let resolved = queues.resolve_tick(7).unwrap();
        if resolved.command.activation.initiation == Some(start) {
            assert!(resolved.client_tick > 13, "the start is delivered late");
            assert!(queues.activations.accept(
                7,
                id,
                postretro_entities::EntityId::from_raw(9),
                &program,
                tick
            ));
            queues.activation_admitted(7, start, false);
            cursor = Some(start_activation(start, id.pawn, tick, &program));
        }
        if let Some(live) = cursor.as_mut() {
            let result =
                advance_activation(live, &program, tick, true, resolved.command.activation);
            if let Some(shot) = result.shot {
                shots.push(shot.shot_id.ordinal);
            }
            if let Some(reason) = result.terminal {
                terminal = Some(reason);
                queues.activations.terminal(7, id, tick);
                queues.activation_terminal(7, start);
                cursor = None;
            }
        }
    }
    assert_eq!(
        shots,
        vec![0, 1],
        "every shot the client fired, and no more"
    );
    assert_eq!(
        terminal,
        Some(postretro_combat_model::activation::ActivationTermination::Cancelled)
    );
}
