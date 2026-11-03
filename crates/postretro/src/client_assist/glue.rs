// App-side wiring of the client assist step: candidate gathering from the
// registry and replication, sample consumption, and the per-role projection.
// See: context/lib/networking.md §What gates, and what replicates instead

use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::{Capability, PlayerMovementComponent};

use super::{ClientAssist, UiProjection};
use crate::netcode::{self, AssistProgressSample, ClientReplication};
use crate::sim::assist::{AssistCandidate, AssistHelper, PawnLife};
use crate::sim::assist_pairs::PairValues;

/// The local pawn as helper. Its entry count is never passed: the client runner
/// compares ids alone.
pub(super) fn local_helper(registry: &EntityRegistry) -> Option<AssistHelper> {
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
            entry: None,
        },
    })
}

/// Every remote pawn with retained facts, at its interpolated pose as of
/// `render_server_tick`, with its pair-table entry when one has arrived. Every
/// candidate reads the same source, whether or not a snapshot applied this
/// frame. `local` (the local pawn, when there is one) is never a candidate.
pub(super) fn gather_candidates(
    replication: &ClientReplication,
    render_server_tick: Option<f64>,
    local: Option<EntityId>,
    pawns: &mut Vec<(EntityId, netcode::RemoteAssistFacts)>,
    out: &mut Vec<AssistCandidate>,
) {
    out.clear();
    replication.remote_assist_pawns(pawns);
    if pawns.is_empty() {
        return;
    }
    let table = replication.assist_pair_table();
    out.extend(pawns.iter().filter_map(|(id, facts)| {
        if Some(*id) == local {
            return None;
        }
        let position = replication.remote_assist_position(*id, render_server_tick)?;
        Some(AssistCandidate {
            id: *id,
            position,
            life: PawnLife {
                life_state: facts.life_state,
                entry: None,
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
    /// After the receive step: a newly armed local pawn starts with no hold,
    /// then this frame's host samples correct the prediction in order. Every
    /// applied local-pawn record reports its pawn as armed, so only a change of
    /// pawn resets. Reads the current pair table only when a sample arrived.
    pub(crate) fn consume_receive(
        &mut self,
        armed_pawn: Option<EntityId>,
        samples: &[AssistProgressSample],
        replication: &ClientReplication,
    ) {
        self.absorb_receive(armed_pawn, samples, || replication.assist_pair_table());
    }
}

impl crate::App {
    /// The client step for one forward-predicted tick, inside the `predict`
    /// closure after `client_predict_movement_tick`, with that tick's command
    /// and input tick, so the helper stands where this tick's movement put it,
    /// as the host's pass runs after movement. Own pawn only; never during
    /// reconcile replay.
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
                self.remote_player_presentation.render_server_tick,
                Some(helper.id),
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
