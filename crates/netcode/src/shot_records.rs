// Host open authorized shots and pending hit declarations held for their FIRE.
// See: context/lib/networking.md §Combat authority: FIRE vs HIT
use crate::splash_march::SplashMarch;
use crate::{HostCommandQueues, NetworkIdAllocator, activation_ledger, prediction};
use postretro_combat_model::{AuthorizedShot, OpenAuthorizedShot, ShotId};
use postretro_entities::EntityId;
use postretro_net::wire;
use std::collections::{HashMap, VecDeque};

pub(crate) const MAX_PENDING_HIT_DECLARATIONS_PER_CLIENT: usize = 64;

/// Host ticks after FIRE before a projectile declaration may be taken: a
/// projectile cannot have struck anything on its own FIRE tick. A splash
/// declaration then binds to its shot's march, which decides when the blast
/// lands (`splash_march`).
const MIN_PROJECTILE_HOLD_TICKS: u32 = 1;

#[derive(Debug, Default)]
pub struct OpenAuthorizedShots {
    pub(crate) shots: HashMap<ShotId, OpenAuthorizedShot>,
    /// Each open client splash shot's march. It outlives its shot's HIT
    /// intake: binding the declaration retires the shot, and the march
    /// releases the blast when it reaches its deciding point.
    pub(crate) marches: HashMap<ShotId, SplashMarch>,
    /// `splash_march::step_and_release`'s fixed march order, kept between
    /// ticks so a tick does not allocate it.
    pub(crate) march_order: Vec<ShotId>,
}

impl OpenAuthorizedShots {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, shot: AuthorizedShot, owner_client_id: u64) {
        if shot.is_projectile
            && shot.splash.is_some()
            && let Some(march) = SplashMarch::new(shot.clone(), owner_client_id)
        {
            self.marches.insert(shot.shot_id, march);
        }
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
    /// Unknown starts and future ordinals have no FIRE decision yet. Their
    /// caller sends a HIT-only refusal rather than fabricating a FIRE verdict.
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
        if !owned_open
            && matches!(
                status,
                activation_ledger::OrdinalStatus::Unknown
                    | activation_ledger::OrdinalStatus::Pending { .. }
            )
        {
            return None;
        }
        if owned_open {
            // Refused before its declaration bound, its march has nothing to
            // release.
            self.retire(shot_id);
            self.drop_march(shot_id);
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
        self.marches
            .retain(|_, march| march.owner_client_id != client_id);
    }

    pub(crate) fn remove_pawn(&mut self, pawn: EntityId) {
        self.shots.retain(|_, shot| shot.shot.pawn != pawn);
        self.marches.retain(|_, march| march.shot.pawn != pawn);
    }

    /// A march outlives its open shot only while bound to a declaration, and
    /// a bound march resolves within its frozen reach, inside the same budget.
    pub(crate) fn prune_stale(&mut self, current_tick: u32) {
        self.shots.retain(|_, shot| {
            current_tick.wrapping_sub(shot.shot.fire_tick) <= shot.shot.timeout_budget_ticks
        });
        self.marches.retain(|_, march| {
            current_tick.wrapping_sub(march.shot.fire_tick) <= march.shot.timeout_budget_ticks
        });
    }

    pub(crate) fn march(&self, shot_id: ShotId) -> Option<&SplashMarch> {
        self.marches.get(&shot_id)
    }

    /// Whether any client splash shot is marching: one can release a blast on
    /// a tick that ingests no declaration.
    pub fn has_marches(&self) -> bool {
        !self.marches.is_empty()
    }

    /// Drop a march whose shot settled without it.
    pub(crate) fn drop_march(&mut self, shot_id: ShotId) {
        self.marches.remove(&shot_id);
    }

    /// Bind a splash declaration to its shot's march; whether one was open.
    pub(crate) fn bind_splash_declaration(
        &mut self,
        shot_id: ShotId,
        declaration: &wire::HitDeclaration,
    ) -> bool {
        self.marches
            .get_mut(&shot_id)
            .map(|march| march.bind(declaration))
            .is_some()
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
    /// Expiry settles HIT intake without deciding a still-eligible FIRE.
    pub(crate) undecided_expiry: bool,
    /// Owner-scoped FIRE authorization captured before its open HIT record can retire.
    pub(crate) fire_authorized: bool,
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
            undecided_expiry: false,
            fire_authorized: false,
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
        while let Some(mut pending) = self.declarations.pop_front() {
            let shot_id = crate::wire_convert::shot_id_from_wire(pending.declaration.shot_id);
            let open_shot = open_shots.get(shot_id);
            let shot_open = open_shot.is_some();
            let projectile_hold_active = open_shot.as_ref().is_some_and(|open| {
                open.shot.is_projectile
                    && current_tick.wrapping_sub(open.shot.fire_tick) < MIN_PROJECTILE_HOLD_TICKS
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
            if !projectile_hold_active
                && (shot_open
                    || matches!(ordinal, OrdinalStatus::Rejected | OrdinalStatus::Authorized)
                    || expired)
            {
                pending.undecided_expiry = expired && !shot_open;
                pending.fire_authorized = matches!(ordinal, OrdinalStatus::Authorized)
                    || open_shot
                        .as_ref()
                        .is_some_and(|open| open.owner_client_id == pending.client_id);
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

#[cfg(test)]
mod activation_tests {
    use super::*;
    use postretro_foundation::{ActivationLane, ActivationProgram, ActivationStep};

    fn declaration(start_tick: u32, ordinal: u8) -> wire::HitDeclaration {
        wire::HitDeclaration {
            shot_id: wire::WireShotId {
                pawn: 4,
                start_tick,
                lane: 1,
                ordinal,
            },
            records: Vec::new(),
        }
    }

    #[test]
    fn unknown_activation_declarations_ignore_movement_progress_and_expire_from_first_receipt() {
        let mut queues = HostCommandQueues::new();
        for tick in 10..13 {
            assert!(queues.ingest(
                7,
                &wire::InputCommand {
                    teleport_yaw_stamp: 0.0,
                    client_tick: tick,
                    movement: wire::WireMovementInput {
                        assist_held: false,
                        wish_dir: [0.0, 0.0],
                        jump_pressed: false,
                        dash_pressed: false,
                        running: false,
                        crouch_intent: false,
                        facing_yaw: 0.0,
                        use_pressed: false,
                        drop_pressed: false,
                        aim_pitch: 0.0,
                        firing_slot: 0,
                    },
                    fire_button: wire::WireFireButtonState {
                        pressed: false,
                        active: false
                    },
                    secondary_button: wire::WireFireButtonState {
                        pressed: false,
                        active: false
                    },
                    activation: wire::WireActivationInput::default(),
                    reload: false,
                }
            ));
        }
        assert!(queues.resolve_tick(7).is_some());
        assert!(queues.resolved_cursor(7).is_some_and(|tick| tick >= 9));
        let mut pending = PendingHitDeclarations::new();
        let shots = OpenAuthorizedShots::new();
        assert!(pending.push_at(7, declaration(9, 0), 100));
        assert!(pending.drain_ready(&queues, &shots, 100).is_empty());
        assert!(
            pending.push_at(7, declaration(9, 0), 219),
            "a duplicate does not extend retention"
        );
        assert!(pending.drain_ready(&queues, &shots, 219).is_empty());
        let expired = pending.drain_ready(&queues, &shots, 220);
        assert_eq!(expired.len(), 1);
        assert!(expired[0].undecided_expiry);
        assert!(!expired[0].fire_authorized);
        assert_eq!(pending.len(), 0);
    }

    #[test]
    fn accepted_future_ordinal_waits_for_its_decision_instead_of_unknown_start_expiry() {
        let mut queues = HostCommandQueues::new();
        let mut pending = PendingHitDeclarations::new();
        let shots = OpenAuthorizedShots::new();
        assert!(pending.push_at(7, declaration(10, 1), 0));
        let id = crate::wire_convert::shot_id_from_wire(declaration(10, 1).shot_id).activation();
        let program = ActivationProgram::new(
            vec![
                ActivationStep::Shot,
                ActivationStep::Wait { ticks: 2 },
                ActivationStep::Shot,
            ],
            None,
            18,
        )
        .unwrap();
        assert!(
            queues
                .activations
                .accept(7, id, EntityId::from_raw(9), &program, 300)
        );
        assert!(pending.drain_ready(&queues, &shots, 421).is_empty());
        queues.activations.settle_shot(
            7,
            ShotId::from_parts(4, 10, ActivationLane::Secondary, 1),
            false,
        );
        let rejected = pending.drain_ready(&queues, &shots, 421);
        assert_eq!(rejected.len(), 1);
        assert!(!rejected[0].undecided_expiry);
        assert!(!rejected[0].fire_authorized);
    }

    #[test]
    fn accepted_undecided_ordinal_expiry_remains_a_hit_only_refusal() {
        let mut queues = HostCommandQueues::new();
        let mut pending = PendingHitDeclarations::new();
        let shots = OpenAuthorizedShots::new();
        assert!(pending.push_at(7, declaration(10, 0), 0));
        let shot_id = crate::wire_convert::shot_id_from_wire(declaration(10, 0).shot_id);
        let program = ActivationProgram::new(vec![ActivationStep::Shot], None, 18).unwrap();
        assert!(queues.activations.accept(
            7,
            shot_id.activation(),
            EntityId::from_raw(9),
            &program,
            300,
        ));
        assert!(pending.drain_ready(&queues, &shots, 419).is_empty());
        let expired = pending.drain_ready(&queues, &shots, 420);
        assert_eq!(expired.len(), 1);
        assert!(expired[0].undecided_expiry);
        assert!(!expired[0].fire_authorized);
        assert_eq!(
            queues.activations.status(7, shot_id, 420),
            activation_ledger::OrdinalStatus::Pending { deadline: 420 }
        );
    }
}
