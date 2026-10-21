// Host dispatch of each client's Channel::Input messages, and the shot verdict reply.
// See: context/lib/networking.md §Channel model · §Combat authority

use super::*;

/// Drain and apply one accepted client's reliable `Channel::Input` messages on the
/// host: replication acks advance that client's per-entity baseline / retire
/// tombstones, baseline-refresh requests queue a `FullBaseline` for the named
/// entity, a time-sync probe is echoed back on `Channel::Input` with the
/// current `server_tick`, and a switch declaration from a pawn-owning client is
/// retained in its switch lane. Corrupt or unknown-variant bytes are logged and
/// dropped — never a panic.
///
/// `server_tick` is the host's current monotonic sim tick (sampled at echo);
/// `server_now_us` is the host's monotonic microseconds, carried in the echo as
/// telemetry only. `InputCommand` messages are decoded but not yet applied
/// (Phase 3 gameplay).
#[allow(clippy::too_many_arguments)]
pub fn host_handle_client_messages(
    server: &mut NetServer,
    replication: &mut ServerReplication,
    state_slots: &mut state_slots::HostStateReplication,
    command_queues: &mut HostCommandQueues,
    owners: &MovementOwners,
    pending_hit_declarations: &mut PendingHitDeclarations,
    open_shots: &mut OpenAuthorizedShots,
    client_id: u64,
    server_tick: u32,
    server_now_us: u64,
) {
    for bytes in server.drain_input(client_id) {
        let msg: wire::ClientMessage = match wire::decode(&bytes) {
            Ok(msg) => msg,
            Err(err) => {
                log::warn!("[Net] dropping undecodable client message from {client_id}: {err}");
                continue;
            }
        };
        host_handle_client_message_inner(
            server,
            replication,
            state_slots,
            command_queues,
            owners,
            Some(&mut *pending_hit_declarations),
            Some(&mut *open_shots),
            client_id,
            server_tick,
            server_now_us,
            msg,
        );
    }
}

/// Apply one decoded `ClientMessage` from `client_id` (M15 Phase 3). Split from the
/// drain loop so the duplicate/old-input hardening is testable by injecting a
/// `ClientMessage::Input` directly at this seam — without a reliable-ordered
/// transport producing duplicates. An invalid `Input` (non-finite) is dropped at
/// intake and mutates no queue or registry state.
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) fn host_handle_client_message(
    server: &mut NetServer,
    replication: &mut ServerReplication,
    state_slots: &mut state_slots::HostStateReplication,
    command_queues: &mut HostCommandQueues,
    owners: &MovementOwners,
    client_id: u64,
    server_tick: u32,
    server_now_us: u64,
    msg: wire::ClientMessage,
) {
    host_handle_client_message_inner(
        server,
        replication,
        state_slots,
        command_queues,
        owners,
        None,
        None,
        client_id,
        server_tick,
        server_now_us,
        msg,
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn host_handle_client_message_inner(
    server: &mut NetServer,
    replication: &mut ServerReplication,
    state_slots: &mut state_slots::HostStateReplication,
    command_queues: &mut HostCommandQueues,
    owners: &MovementOwners,
    pending_hit_declarations: Option<&mut PendingHitDeclarations>,
    open_shots: Option<&mut OpenAuthorizedShots>,
    client_id: u64,
    server_tick: u32,
    server_now_us: u64,
    msg: wire::ClientMessage,
) {
    match msg {
        wire::ClientMessage::Ack(ack) => {
            replication.apply_ack(
                client_id,
                ack.latest_snapshot_sequence,
                &ack.entity_baselines,
                &ack.despawn_tombstones,
            );
            // The same ack advances replicated-state baselines (M15 Phase 3.5), keyed
            // by `StateSlotId` rather than `NetworkId`. One ack, one server frame.
            state_slots.apply_ack(
                client_id,
                ack.latest_snapshot_sequence,
                &ack.slot_baselines,
                ack.faction_sentiment_baseline,
            );
        }
        wire::ClientMessage::BaselineRefresh(req) => {
            replication.request_refresh(client_id, req.network_id, req.missing_baseline_ref);
        }
        // Echo the time-sync probe with the server tick sampled now. The echo
        // rides Channel::Input back; the client measures RTT from its own
        // send/receive times and folds the server tick into its estimate.
        wire::ClientMessage::TimeSync(req) => {
            let echo = req.echo(server_tick, server_now_us);
            server.send_input(
                client_id,
                wire::encode(&wire::ServerMessage::TimeSync(echo)),
            );
        }
        // M15 Phase 3 Task 4: sanitize + queue the input command for this client.
        // `ingest` rejects non-finite and out-of-window bootstrap commands, drops
        // stale/duplicate ones, and never mutates any other client's queue. The movement
        // stage resolves them per tick.
        wire::ClientMessage::Input(input) => {
            command_queues.ingest(client_id, &input);
        }
        wire::ClientMessage::HitDeclaration(declaration) => {
            if !wire_convert::valid_wire_shot_id(declaration.shot_id) {
                return;
            }
            if let Some(pending) = pending_hit_declarations
                && !pending.push_at(client_id, declaration.clone(), server_tick)
            {
                if let Some(verdict) = open_shots.and_then(|shots| {
                    shots.refuse_overflowed_hit(
                        &command_queues.activations,
                        client_id,
                        wire_convert::shot_id_from_wire(declaration.shot_id),
                        server_tick,
                    )
                }) {
                    send_shot_verdict(
                        server,
                        client_id,
                        wire_convert::shot_id_from_wire(verdict.shot_id),
                        verdict.accept,
                        verdict.hit_accepted,
                    );
                } else {
                    server.send_input(
                        client_id,
                        wire::encode(&wire::ServerMessage::HitRefused(declaration.shot_id)),
                    );
                }
            }
        }
        // M15 Phase 3.5: a client missing a replicated state-slot baseline. The state
        // tracker schedules a full baseline for that slot in the client's next snapshot.
        // Keyed by `StateSlotId` (distinct from the entity `BaselineRefresh`).
        wire::ClientMessage::StateBaselineRefresh(req) => {
            state_slots.request_refresh(client_id, req.slot_id, req.missing_baseline_ref);
        }
        // Retained in the switch lane, which releases it on a later fixed tick in
        // client-tick order with this client's starts and presses. The Input
        // drain already dropped one framed under a retired participation epoch.
        wire::ClientMessage::SwitchDeclaration(declaration) => {
            host_retain_switch_declaration(command_queues, owners, client_id, declaration);
        }
    }
}

pub fn send_shot_verdict(
    server: &mut NetServer,
    client_id: u64,
    shot_id: postretro_foundation::ShotId,
    accept: bool,
    hit_accepted: bool,
) {
    server.send_input(
        client_id,
        wire::encode(&wire::ServerMessage::ShotVerdicts(
            wire::ShotVerdictsMessage {
                verdicts: vec![wire::ShotVerdict {
                    shot_id: crate::wire_convert::shot_id_to_wire(shot_id),
                    accept,
                    hit_accepted,
                }],
            },
        )),
    );
}
