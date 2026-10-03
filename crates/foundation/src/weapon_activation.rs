//! Stable activation identities and input facts; no execution policy or VM types.
mod compiled;
mod scaled;
pub use compiled::*;
pub use scaled::*;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[repr(u8)]
pub enum ActivationLane {
    #[default]
    Primary = 0,
    Secondary = 1,
}
impl ActivationLane {
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::Primary),
            1 => Some(Self::Secondary),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ActivationToken {
    pub start_tick: u32,
    pub lane: ActivationLane,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ActivationId {
    pub pawn: u32,
    pub token: ActivationToken,
}
/// Authored attempts retain their initiating tick, including across held cursor ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ShotId {
    pub pawn: u32,
    pub start_tick: u32,
    pub lane: ActivationLane,
    pub ordinal: u8,
}
impl ShotId {
    pub fn from_parts(pawn: u32, start_tick: u32, lane: ActivationLane, ordinal: u8) -> Self {
        Self {
            pawn,
            start_tick,
            lane,
            ordinal,
        }
    }
    pub fn activation(self) -> ActivationId {
        ActivationId {
            pawn: self.pawn,
            token: ActivationToken {
                start_tick: self.start_tick,
                lane: self.lane,
            },
        }
    }
    pub fn client_tick(self) -> u32 {
        self.start_tick
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivationRelease {
    pub token: ActivationToken,
    pub release_tick: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ActivationInput {
    pub initiation: Option<ActivationToken>,
    pub release: Option<ActivationRelease>,
    pub cancel: Option<ActivationToken>,
}
impl ActivationInput {
    pub fn without_edges(self) -> Self {
        Self::default()
    }
}

/// POD execution cursor suitable for central weapon-state payloads. Immutable
/// installed programs share storage across component clones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationPhase {
    Charging,
    Executing,
    Terminal,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActivationCursor {
    pub token: ActivationToken,
    pub pawn: u32,
    pub accepted_tick: u32,
    pub last_real_tick: u32,
    pub last_advanced_tick: Option<u32>,
    pub phase: ActivationPhase,
    pub step: u8,
    pub ordinal: u8,
    pub due_tick: u32,
    pub charge: f32,
}

use std::sync::Arc;
pub const MAX_ACTIVATION_STEPS: usize = 64;
pub const MAX_ACTIVATION_SHOTS: usize = 16;
pub const MAX_ACTIVATION_WAIT_TICKS: u32 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationStep {
    Shot,
    Wait { ticks: u32 },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChargeTiming {
    pub min_ticks: u32,
    pub full_ticks: u32,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ActivationProgram {
    pub steps: Arc<[ActivationStep]>,
    pub charge: Option<ChargeTiming>,
    pub recovery_ticks: u32,
}
impl ActivationProgram {
    pub fn new(
        steps: Vec<ActivationStep>,
        charge: Option<ChargeTiming>,
        recovery_ticks: u32,
    ) -> Result<Self, &'static str> {
        if steps.is_empty()
            || steps.len() > MAX_ACTIVATION_STEPS
            || steps.first() != Some(&ActivationStep::Shot)
            || steps.last() != Some(&ActivationStep::Shot)
        {
            return Err("activation must begin and end with a shot and contain at most 64 steps");
        }
        let mut shots = 0;
        let mut waits = 0u32;
        let mut previous_shot = false;
        for step in &steps {
            match step {
                ActivationStep::Shot => {
                    if previous_shot {
                        return Err("shots need a positive wait");
                    }
                    shots += 1;
                    previous_shot = true;
                }
                ActivationStep::Wait { ticks } => {
                    if *ticks == 0 {
                        return Err("wait must be positive");
                    }
                    waits = waits.checked_add(*ticks).ok_or("wait overflow")?;
                    previous_shot = false;
                }
            }
        }
        if shots > MAX_ACTIVATION_SHOTS || waits > MAX_ACTIVATION_WAIT_TICKS {
            return Err("activation exceeds its bounded duration or shot count");
        }
        if recovery_ticks > MAX_ACTIVATION_WAIT_TICKS {
            return Err("recovery exceeds its bounded duration");
        }
        if charge.is_some_and(|c| {
            c.full_ticks == 0
                || c.min_ticks > c.full_ticks
                || c.full_ticks > MAX_ACTIVATION_WAIT_TICKS
        }) {
            return Err("invalid charge duration");
        }
        Ok(Self {
            steps: steps.into(),
            charge,
            recovery_ticks,
        })
    }
    pub fn shot_count(&self) -> u8 {
        self.steps
            .iter()
            .filter(|s| matches!(s, ActivationStep::Shot))
            .count() as u8
    }
}

#[cfg(test)]
mod authoring_tests;
