//! Owner-scoped activation settlement independent of movement-command progress.
use postretro_foundation::{
    ActivationId, ActivationProgram, ActivationStep, ActivationToken, ShotId,
};
use std::collections::{HashMap, VecDeque};

const RECORD_LIMIT: usize = 64;
const RETENTION_TICKS: u32 = 120;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrdinalStatus {
    Unknown,
    Pending { deadline: u32 },
    Authorized,
    Rejected,
}
#[derive(Debug, Clone)]
struct LiveActivation {
    id: ActivationId,
    weapon: Option<postretro_entities::EntityId>,
    shot_count: u8,
    decided: u16,
    authorized: u16,
    deadlines: [u32; 16],
}
#[derive(Debug, Clone)]
struct TerminalActivation {
    live: LiveActivation,
    terminated_tick: u32,
}
#[derive(Debug, Default)]
struct ClientLedger {
    live: Option<LiveActivation>,
    terminal: VecDeque<TerminalActivation>,
    settled_start: Option<u32>,
}
#[derive(Debug, Default)]
pub struct HostActivationLedger {
    clients: HashMap<u64, ClientLedger>,
}
impl HostActivationLedger {
    /// Bind an admitted initiation to the instance outside this pure identity ledger.
    /// Returning false prevents replay and concurrent execution; it never replaces a live activation.
    pub fn accept(
        &mut self,
        client: u64,
        id: ActivationId,
        weapon: postretro_entities::EntityId,
        program: &ActivationProgram,
        tick: u32,
    ) -> bool {
        let state = self.clients.entry(client).or_default();
        prune(state, tick);
        if state.live.is_some()
            || state.terminal.iter().any(|t| t.live.id == id)
            || state
                .settled_start
                .is_some_and(|watermark| tick_le(id.token.start_tick, watermark))
        {
            return false;
        }
        let lifetime = program.charge.map_or(0, |c| c.full_ticks + 3600);
        let mut live = LiveActivation {
            id,
            weapon: Some(weapon),
            shot_count: program.shot_count(),
            decided: 0,
            authorized: 0,
            deadlines: [tick.wrapping_add(lifetime + RETENTION_TICKS); 16],
        };
        if program.charge.is_none() {
            schedule(&mut live, program, tick);
        }
        state.live = Some(live);
        true
    }
    pub fn execution(
        &mut self,
        client: u64,
        token: ActivationToken,
        program: &ActivationProgram,
        tick: u32,
    ) {
        if let Some(live) = self
            .clients
            .get_mut(&client)
            .and_then(|s| s.live.as_mut())
            .filter(|l| l.id.token == token)
        {
            schedule(live, program, tick);
        }
    }
    pub fn bound_weapon(
        &self,
        client: u64,
        id: ActivationId,
    ) -> Option<postretro_entities::EntityId> {
        let state = self.clients.get(&client)?;
        state
            .live
            .as_ref()
            .filter(|live| live.id == id)
            .and_then(|live| live.weapon)
            .or_else(|| {
                state
                    .terminal
                    .iter()
                    .find(|record| record.live.id == id)
                    .and_then(|record| record.live.weapon)
            })
    }
    pub fn settle_shot(&mut self, client: u64, shot: ShotId, authorized: bool) {
        if shot.ordinal >= 16 {
            return;
        }
        if let Some(live) = self
            .clients
            .get_mut(&client)
            .and_then(|s| s.live.as_mut())
            .filter(|l| l.id == shot.activation() && shot.ordinal < l.shot_count)
        {
            let bit = 1 << shot.ordinal;
            if live.decided & bit != 0 {
                return;
            }
            live.decided |= bit;
            if authorized {
                live.authorized |= bit;
            }
        }
    }
    /// Reject unissued ordinals; previously authorized projectile shots keep their own lifetime.
    pub fn terminal(&mut self, client: u64, id: ActivationId, tick: u32) {
        let state = self.clients.entry(client).or_default();
        prune(state, tick);
        if state.terminal.iter().any(|t| t.live.id == id) {
            return;
        }
        let owns_live = state.live.as_ref().is_some_and(|l| l.id == id);
        if !owns_live
            && state
                .settled_start
                .is_some_and(|watermark| tick_le(id.token.start_tick, watermark))
        {
            return;
        }
        let live = if state.live.as_ref().is_some_and(|l| l.id == id) {
            state.live.take().unwrap()
        } else {
            LiveActivation {
                id,
                weapon: None,
                shot_count: 0,
                decided: 0,
                authorized: 0,
                deadlines: [tick; 16],
            }
        };
        if state
            .settled_start
            .is_none_or(|watermark| !tick_le(id.token.start_tick, watermark))
        {
            state.settled_start = Some(id.token.start_tick);
        }
        if state.terminal.len() == RECORD_LIMIT {
            state.terminal.pop_front();
        }
        state.terminal.push_back(TerminalActivation {
            live,
            terminated_tick: tick,
        });
    }
    pub fn status(&self, client: u64, shot: ShotId, tick: u32) -> OrdinalStatus {
        let Some(state) = self.clients.get(&client) else {
            return OrdinalStatus::Unknown;
        };
        if let Some(live) = state.live.as_ref().filter(|l| l.id == shot.activation()) {
            if shot.ordinal >= live.shot_count {
                return OrdinalStatus::Rejected;
            }
            let bit = 1 << shot.ordinal;
            return if live.authorized & bit != 0 {
                OrdinalStatus::Authorized
            } else if live.decided & bit != 0 {
                OrdinalStatus::Rejected
            } else {
                OrdinalStatus::Pending {
                    deadline: live.deadlines[usize::from(shot.ordinal)],
                }
            };
        }
        if let Some(record) = state.terminal.iter().find(|t| {
            t.live.id == shot.activation() && tick.wrapping_sub(t.terminated_tick) < RETENTION_TICKS
        }) {
            return if shot.ordinal < 16 && record.live.authorized & (1 << shot.ordinal) != 0 {
                OrdinalStatus::Authorized
            } else {
                OrdinalStatus::Rejected
            };
        }
        if state
            .settled_start
            .is_some_and(|watermark| tick_le(shot.start_tick, watermark))
        {
            OrdinalStatus::Rejected
        } else {
            OrdinalStatus::Unknown
        }
    }
    pub fn remove_client(&mut self, client: u64) {
        self.clients.remove(&client);
    }
    pub fn prune(&mut self, tick: u32) {
        for state in self.clients.values_mut() {
            prune(state, tick);
        }
    }
}
fn tick_le(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) <= 0
}
fn prune(state: &mut ClientLedger, tick: u32) {
    state
        .terminal
        .retain(|t| tick.wrapping_sub(t.terminated_tick) < RETENTION_TICKS);
}
fn schedule(live: &mut LiveActivation, program: &ActivationProgram, tick: u32) {
    let mut offset = 0u32;
    let mut ordinal = 0;
    for step in program.steps.iter() {
        match step {
            ActivationStep::Shot => {
                live.deadlines[ordinal] = tick.wrapping_add(offset + RETENTION_TICKS);
                ordinal += 1;
            }
            ActivationStep::Wait { ticks } => offset += ticks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_foundation::ActivationLane;
    fn id(start_tick: u32) -> ActivationId {
        ActivationId {
            pawn: 4,
            token: ActivationToken {
                start_tick,
                lane: ActivationLane::Secondary,
            },
        }
    }
    fn program() -> ActivationProgram {
        ActivationProgram::new(
            vec![
                ActivationStep::Shot,
                ActivationStep::Wait { ticks: 2 },
                ActivationStep::Shot,
            ],
            None,
            18,
        )
        .unwrap()
    }
    fn shot(id: ActivationId, ordinal: u8) -> ShotId {
        ShotId::from_parts(id.pawn, id.token.start_tick, id.token.lane, ordinal)
    }
    #[test]
    fn activation_ledger_terminal_preserves_authorized_and_rejects_future_ordinals() {
        let mut ledger = HostActivationLedger::default();
        assert!(ledger.accept(
            7,
            id(10),
            postretro_entities::EntityId::from_raw(9),
            &program(),
            100
        ));
        ledger.settle_shot(7, shot(id(10), 0), true);
        ledger.terminal(7, id(10), 101);
        assert_eq!(
            ledger.status(7, shot(id(10), 0), 101),
            OrdinalStatus::Authorized
        );
        assert_eq!(
            ledger.status(7, shot(id(10), 1), 101),
            OrdinalStatus::Rejected
        );
        assert!(!ledger.accept(
            7,
            id(10),
            postretro_entities::EntityId::from_raw(9),
            &program(),
            102
        ));
    }
    #[test]
    fn activation_ledger_watermark_survives_eviction_and_does_not_settle_live_future() {
        let mut ledger = HostActivationLedger::default();
        for start in 0..65 {
            ledger.terminal(7, id(start), 100);
        }
        assert_eq!(ledger.clients[&7].terminal.len(), 64);
        assert_eq!(
            ledger.status(7, shot(id(0), 0), 101),
            OrdinalStatus::Rejected
        );
        assert!(ledger.accept(
            7,
            id(65),
            postretro_entities::EntityId::from_raw(9),
            &program(),
            101
        ));
        assert_eq!(
            ledger.status(7, shot(id(65), 1), 101),
            OrdinalStatus::Pending { deadline: 223 }
        );
        // Settling an unknown later request cannot settle a live earlier ordinal.
        ledger.terminal(7, id(66), 102);
        assert_eq!(
            ledger.status(7, shot(id(65), 1), 102),
            OrdinalStatus::Pending { deadline: 223 }
        );
    }
    #[test]
    fn activation_ledger_duplicate_terminal_does_not_refresh_first_termination() {
        let mut ledger = HostActivationLedger::default();
        ledger.terminal(7, id(1), 0);
        ledger.terminal(7, id(1), 119);
        ledger.prune(120);
        assert!(ledger.clients[&7].terminal.is_empty());
        assert!(!ledger.accept(
            7,
            id(1),
            postretro_entities::EntityId::from_raw(9),
            &program(),
            121
        ));
    }
}
