// App-side wiring of the client assist step: candidate gathering from the
// registry and replication, sample consumption, and the per-role projection.
// See: context/lib/networking.md §What gates, and what replicates instead

use postretro_entities::{EntityRegistry, Transform};
use postretro_foundation::{Capability, PlayerMovementComponent};

use super::{ClientAssist, UiProjection};
use crate::netcode::{self, AssistProgressSample, ClientReplication};
use crate::sim::assist::{AssistCandidate, AssistHelper, PawnLife};
use crate::sim::assist_pairs::PairValues;

/// The local pawn as helper. Its elapsed is never passed: the client runner
/// compares ids alone.
fn local_helper(registry: &EntityRegistry) -> Option<AssistHelper> {
    let pawn = registry.local_player_movement_pawn()?;
    let movement = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .ok()?;
    let position = registry.get_component::<Transform>(pawn).ok()?.position;
    Some(AssistHelper {
        id: pawn,
        position,
        permits_use: crate::sim::player_life::permits(registry, pawn, Capability::Use),
        life: PawnLife {
            life_state: movement.life_state,
            elapsed: None,
        },
    })
}

/// Every remote pawn with retained facts, at its interpolated pose, with its
/// pair-table entry when one has arrived.
fn gather_candidates(
    replication: &ClientReplication,
    registry: &EntityRegistry,
    helper: &AssistHelper,
    pawns: &mut Vec<(postretro_entities::EntityId, netcode::RemoteAssistFacts)>,
    out: &mut Vec<AssistCandidate>,
) {
    out.clear();
    replication.remote_assist_pawns(pawns);
    if pawns.is_empty() {
        return;
    }
    let table = replication.assist_pair_table();
    out.extend(pawns.iter().filter_map(|(id, facts)| {
        if *id == helper.id {
            return None;
        }
        let position = registry.get_component::<Transform>(*id).ok()?.position;
        Some(AssistCandidate {
            id: *id,
            position,
            life: PawnLife {
                life_state: facts.life_state,
                elapsed: None,
            },
            assistable: facts.assistable,
            pair: table
                .iter()
                .find(|entry| entry.target == *id)
                .map(|entry| PairValues {
                    radius: entry.radius,
                    required_ticks: entry.required_ticks,
                }),
        })
    }));
}

/// Reused per-tick buffers, so a held tick allocates only the pair table.
#[derive(Debug, Default)]
pub(crate) struct Scratch {
    pawns: Vec<(postretro_entities::EntityId, netcode::RemoteAssistFacts)>,
    candidates: Vec<AssistCandidate>,
}

impl ClientAssist {
    /// Consume the receive step's samples, after it, in order. Reads the
    /// current pair table only when a sample arrived.
    pub(crate) fn consume_samples(
        &mut self,
        samples: &[AssistProgressSample],
        replication: &ClientReplication,
    ) {
        if samples.is_empty() {
            return;
        }
        self.apply_samples(samples, &replication.assist_pair_table());
    }
}

impl crate::App {
    /// The client step for one forward-predicted tick, inside the `predict`
    /// closure beside `predict_client_weapon_command`, with that tick's
    /// command and input tick. Own pawn only; never during reconcile replay.
    pub(crate) fn predict_client_assist(
        &mut self,
        command: &crate::sim::SimCommand,
        input_tick: u32,
    ) {
        let Some(session) = self.session.as_ref() else {
            self.client_assist.reset();
            return;
        };
        let Some(netcode::NetEndpoint::Client { replication, .. }) = session.net_endpoint.as_ref()
        else {
            return;
        };
        let registry = session.scripting.script_ctx.registry.borrow();
        let Some(helper) = local_helper(&registry) else {
            self.client_assist.reset();
            return;
        };
        let mut scratch = std::mem::take(&mut self.client_assist.scratch);
        if command.assist_held {
            gather_candidates(
                replication,
                &registry,
                &helper,
                &mut scratch.pawns,
                &mut scratch.candidates,
            );
        } else {
            scratch.candidates.clear();
        }
        self.client_assist.predict_tick(
            input_tick,
            command.assist_held,
            helper,
            &scratch.candidates,
        );
        self.client_assist.scratch = scratch;
    }
}

/// This frame's `player.assistProgress` projection: the client step's on a
/// connected client; the host accumulator's row-6 record for the local pawn on
/// a listen host or in single player.
pub(crate) fn ui_projection(
    client: &ClientAssist,
    session: &crate::session::Session,
    alpha: f32,
) -> UiProjection {
    if matches!(
        session.net_endpoint,
        Some(netcode::NetEndpoint::Client { .. })
    ) {
        return client.client_projection(alpha);
    }
    let row6 = session
        .scripting
        .script_ctx
        .registry
        .borrow()
        .local_player_movement_pawn()
        .map(|pawn| session.assist_state.row6_record(pawn))
        .unwrap_or_default();
    UiProjection::Host { row6, alpha }
}
