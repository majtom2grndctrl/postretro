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
