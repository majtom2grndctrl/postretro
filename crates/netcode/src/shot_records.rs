//! Host shot lifetime and ordinal-aware pending declarations.
use crate::{HostCommandQueues, NetworkIdAllocator, activation_ledger, prediction};
use postretro_combat_model::{AuthorizedShot, OpenAuthorizedShot, ShotId};
use postretro_entities::EntityId;
use postretro_net::wire;
use std::collections::{HashMap, VecDeque};

pub(crate) const MAX_PENDING_HIT_DECLARATIONS_PER_CLIENT: usize = 64;

#[derive(Debug, Default)]
pub struct OpenAuthorizedShots {
    pub(crate) shots: HashMap<ShotId, OpenAuthorizedShot>,
}

impl OpenAuthorizedShots {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, shot: AuthorizedShot, owner_client_id: u64) {
        self.shots.insert(
            shot.shot_id,
            OpenAuthorizedShot {
                shot,
                owner_client_id,
            },
        );
    }

    pub(crate) fn get(&self, shot_id: ShotId) -> Option<OpenAuthorizedShot> {
        self.shots.get(&shot_id).cloned()
    }

    pub(crate) fn retire(&mut self, shot_id: ShotId) -> Option<OpenAuthorizedShot> {
        self.shots.remove(&shot_id)
    }

    /// Refusing HIT intake cannot undo FIRE authorization or its resource debit.
    /// A future ordinal has no FIRE decision yet, so its usual outcome settles it later.
    pub(crate) fn refuse_overflowed_hit(
        &mut self,
        ledger: &activation_ledger::HostActivationLedger,
        client_id: u64,
        shot_id: ShotId,
        tick: u32,
    ) -> Option<wire::ShotVerdict> {
        let owned_open = self
            .get(shot_id)
            .is_some_and(|open| open.owner_client_id == client_id);
        let status = ledger.status(client_id, shot_id, tick);
        if !owned_open && matches!(status, activation_ledger::OrdinalStatus::Pending { .. }) {
            return None;
        }
        if owned_open {
            self.retire(shot_id);
        }
        Some(wire::ShotVerdict {
            shot_id: crate::wire_convert::shot_id_to_wire(shot_id),
            accept: owned_open || matches!(status, activation_ledger::OrdinalStatus::Authorized),
            hit_accepted: false,
        })
    }

    pub(crate) fn remove_client(&mut self, client_id: u64) {
        self.shots
            .retain(|_, shot| shot.owner_client_id != client_id);
    }

    pub(crate) fn remove_pawn(&mut self, pawn: EntityId) {
        self.shots.retain(|_, shot| shot.shot.pawn != pawn);
    }

    pub(crate) fn prune_stale(&mut self, current_tick: u32) {
        self.shots.retain(|_, shot| {
            current_tick.wrapping_sub(shot.shot.fire_tick) <= shot.shot.timeout_budget_ticks
        });
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.shots.len()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingHitDeclaration {
    pub client_id: u64,
    pub declaration: wire::HitDeclaration,
    first_received_tick: u32,
}

#[derive(Debug, Default)]
pub struct PendingHitDeclarations {
    pub(crate) declarations: VecDeque<PendingHitDeclaration>,
}

impl PendingHitDeclarations {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn push(&mut self, client_id: u64, declaration: wire::HitDeclaration) {
        let _ = self.push_at(client_id, declaration, 0);
    }
    pub(crate) fn push_at(
        &mut self,
        client_id: u64,
        declaration: wire::HitDeclaration,
        tick: u32,
    ) -> bool {
        if !crate::wire_convert::valid_wire_shot_id(declaration.shot_id) {
            return false;
        }
        if self
            .declarations
            .iter()
            .any(|p| p.client_id == client_id && p.declaration.shot_id == declaration.shot_id)
        {
            return true;
        }
        if self
            .declarations
            .iter()
            .filter(|p| p.client_id == client_id)
            .count()
            >= MAX_PENDING_HIT_DECLARATIONS_PER_CLIENT
        {
            return false;
        }
        self.declarations.push_back(PendingHitDeclaration {
            client_id,
            declaration,
            first_received_tick: tick,
        });
        true
    }

    pub(crate) fn remove_client(&mut self, client_id: u64) {
        self.declarations
            .retain(|pending| pending.client_id != client_id);
    }

    pub(crate) fn remove_pawn_shots(&mut self, allocator: &NetworkIdAllocator, pawn: EntityId) {
        let Some(pawn_net) = allocator.network_id_for_entity(pawn) else {
            return;
        };
        self.declarations.retain(|pending| {
            let shot_id = crate::wire_convert::shot_id_from_wire(pending.declaration.shot_id);
            shot_id.pawn != pawn_net.0
        });
    }

    pub(crate) fn drain_ready(
        &mut self,
        command_queues: &HostCommandQueues,
        open_shots: &OpenAuthorizedShots,
        current_tick: u32,
    ) -> Vec<PendingHitDeclaration> {
        let mut ready = Vec::new();
        let mut waiting = VecDeque::new();
        while let Some(pending) = self.declarations.pop_front() {
            let shot_id = crate::wire_convert::shot_id_from_wire(pending.declaration.shot_id);
            let open_shot = open_shots.get(shot_id);
            let shot_open = open_shot.is_some();
            let projectile_waits_for_later_tick = open_shot.is_some_and(|open| {
                open.shot.is_projectile && current_tick.wrapping_sub(open.shot.fire_tick) == 0
            });
            use activation_ledger::OrdinalStatus;
            let ordinal =
                command_queues
                    .activations
                    .status(pending.client_id, shot_id, current_tick);
            let expired = match ordinal {
                OrdinalStatus::Pending { deadline } => {
                    prediction::client_tick_le(deadline, current_tick)
                }
                OrdinalStatus::Unknown => {
                    current_tick.wrapping_sub(pending.first_received_tick) >= 120
                }
                _ => false,
            };
            // Integration seam for immediate fire until Task3 admits all starts into
            // the ledger. It applies only to unknown starts, never live future ordinals.
            let settled_unknown_start = matches!(ordinal, OrdinalStatus::Unknown)
                && command_queues
                    .resolved_cursor(pending.client_id)
                    .is_some_and(|cursor| prediction::client_tick_le(shot_id.start_tick, cursor));
            if !projectile_waits_for_later_tick
                && (shot_open
                    || matches!(ordinal, OrdinalStatus::Rejected | OrdinalStatus::Authorized)
                    || expired
                    || settled_unknown_start)
            {
                ready.push(pending);
            } else {
                waiting.push_back(pending);
            }
        }
        self.declarations = waiting;
        ready
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.declarations.len()
    }
}
