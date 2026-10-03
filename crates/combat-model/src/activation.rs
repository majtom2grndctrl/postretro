//! Pure bounded weapon timing shared by authoritative and predicted execution.
//! Program installation allocates; fixed-tick advancement does not.

pub use postretro_foundation::{ActivationCursor, ActivationPhase};
use postretro_foundation::{ActivationInput, ActivationToken, ShotId};

pub use postretro_foundation::{
    ActivationProgram, ActivationStep, ChargeTiming, MAX_ACTIVATION_SHOTS, MAX_ACTIVATION_STEPS,
    MAX_ACTIVATION_WAIT_TICKS,
};
pub const ACTIVATION_RETENTION_TICKS: u32 = 120;
pub const CHARGE_TOLERANCE_TICKS: u32 = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationTermination {
    Completed,
    Cancelled,
    EarlyRelease,
    InputExpired,
    ChargeExpired,
    InvalidRelease,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DueActivationShot {
    pub shot_id: ShotId,
    pub charge: f32,
}
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ActivationAdvance {
    pub execution_charge: Option<f32>,
    pub shot: Option<DueActivationShot>,
    pub terminal: Option<ActivationTermination>,
}
/// Admission is deliberately separate from advancement: callers may start only from
/// an admitted real command, never from a synthesized held/neutral command.
pub fn start_activation(
    token: ActivationToken,
    pawn: u32,
    host_tick: u32,
    program: &ActivationProgram,
) -> ActivationCursor {
    ActivationCursor {
        token,
        pawn,
        accepted_tick: host_tick,
        last_real_tick: host_tick,
        last_advanced_tick: None,
        phase: if program.charge.is_some() {
            ActivationPhase::Charging
        } else {
            ActivationPhase::Executing
        },
        step: 0,
        ordinal: 0,
        due_tick: host_tick,
        charge: if program.charge.is_some() { 0.0 } else { 1.0 },
    }
}
/// One call per fixed tick. Holds/jumps of movement time never change authored waits.
/// A duplicate call on the same host tick cannot emit another shot.
pub fn advance_activation(
    cursor: &mut ActivationCursor,
    program: &ActivationProgram,
    host_tick: u32,
    real_command: bool,
    input: ActivationInput,
) -> ActivationAdvance {
    let mut result = ActivationAdvance::default();
    if cursor.phase == ActivationPhase::Terminal || cursor.last_advanced_tick == Some(host_tick) {
        return result;
    }
    cursor.last_advanced_tick = Some(host_tick);
    if real_command {
        cursor.last_real_tick = host_tick;
    }
    let termination = if input.cancel == Some(cursor.token) {
        Some(ActivationTermination::Cancelled)
    } else if host_tick.wrapping_sub(cursor.last_real_tick) >= ACTIVATION_RETENTION_TICKS {
        Some(ActivationTermination::InputExpired)
    } else {
        None
    };
    if let Some(reason) = termination {
        cursor.phase = ActivationPhase::Terminal;
        result.terminal = Some(reason);
        return result;
    }
    if cursor.phase == ActivationPhase::Charging {
        let timing = program
            .charge
            .expect("charging cursor belongs to charged program");
        if host_tick.wrapping_sub(cursor.accepted_tick)
            >= timing.full_ticks + MAX_ACTIVATION_WAIT_TICKS
        {
            cursor.phase = ActivationPhase::Terminal;
            result.terminal = Some(ActivationTermination::ChargeExpired);
            return result;
        }
        let Some(release) = input.release.filter(|r| r.token == cursor.token) else {
            return result;
        };
        let elapsed = release.release_tick.wrapping_sub(cursor.token.start_tick);
        if elapsed > timing.full_ticks + MAX_ACTIVATION_WAIT_TICKS {
            cursor.phase = ActivationPhase::Terminal;
            result.terminal = Some(ActivationTermination::InvalidRelease);
            return result;
        }
        let bounded = elapsed
            .min(
                host_tick
                    .wrapping_sub(cursor.accepted_tick)
                    .saturating_add(CHARGE_TOLERANCE_TICKS),
            )
            .min(timing.full_ticks);
        if bounded < timing.min_ticks {
            cursor.phase = ActivationPhase::Terminal;
            result.terminal = Some(ActivationTermination::EarlyRelease);
            return result;
        }
        cursor.charge = bounded as f32 / timing.full_ticks as f32;
        cursor.phase = ActivationPhase::Executing;
        cursor.due_tick = host_tick;
        result.execution_charge = Some(cursor.charge);
    }
    if host_tick.wrapping_sub(cursor.due_tick) >= 1 << 31 {
        return result;
    }
    // Positive waits between authored shots limit advancement to one shot per tick.
    if let Some(step) = program.steps.get(usize::from(cursor.step)) {
        cursor.step += 1;
        match step {
            ActivationStep::Wait { ticks } => {
                cursor.due_tick = host_tick.wrapping_add(*ticks);
                return result;
            }
            ActivationStep::Shot => {
                result.shot = Some(DueActivationShot {
                    shot_id: ShotId::from_parts(
                        cursor.pawn,
                        cursor.token.start_tick,
                        cursor.token.lane,
                        cursor.ordinal,
                    ),
                    charge: cursor.charge,
                });
                cursor.ordinal += 1;
                if usize::from(cursor.step) == program.steps.len() {
                    cursor.phase = ActivationPhase::Terminal;
                    result.terminal = Some(ActivationTermination::Completed);
                } else if let Some(ActivationStep::Wait { ticks }) =
                    program.steps.get(usize::from(cursor.step))
                {
                    cursor.step += 1;
                    cursor.due_tick = host_tick.wrapping_add(*ticks);
                }
                return result;
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_foundation::{ActivationLane, ActivationRelease};
    fn token(tick: u32) -> ActivationToken {
        ActivationToken {
            start_tick: tick,
            lane: ActivationLane::Secondary,
        }
    }
    #[test]
    fn activation_waits_use_fixed_ticks_and_ordinals_across_wrap() {
        let program = ActivationProgram::new(
            vec![
                ActivationStep::Shot,
                ActivationStep::Wait { ticks: 2 },
                ActivationStep::Shot,
                ActivationStep::Wait { ticks: 2 },
                ActivationStep::Shot,
            ],
            None,
            18,
        )
        .unwrap();
        let mut cursor = start_activation(token(u32::MAX - 1), 7, u32::MAX - 1, &program);
        let mut shots = Vec::new();
        for offset in 0..6 {
            let tick = (u32::MAX - 1).wrapping_add(offset);
            let result = advance_activation(
                &mut cursor,
                &program,
                tick,
                false,
                ActivationInput::default(),
            );
            if let Some(shot) = result.shot {
                shots.push((offset, shot.shot_id));
            }
            assert!(
                advance_activation(
                    &mut cursor,
                    &program,
                    tick,
                    false,
                    ActivationInput::default()
                )
                .shot
                .is_none()
            );
        }
        assert_eq!(
            shots
                .iter()
                .map(|(tick, id)| (*tick, id.ordinal))
                .collect::<Vec<_>>(),
            vec![(0, 0), (2, 1), (4, 2)]
        );
        assert!(shots.iter().all(|(_, id)| id.start_tick == u32::MAX - 1));
    }
    #[test]
    fn activation_charge_missing_samples_never_release_and_input_duration_is_bounded() {
        let program = ActivationProgram::new(
            vec![ActivationStep::Shot],
            Some(ChargeTiming {
                min_ticks: 12,
                full_ticks: 60,
            }),
            24,
        )
        .unwrap();
        let mut cursor = start_activation(token(5), 7, 20, &program);
        for host in 20..80 {
            assert!(
                advance_activation(
                    &mut cursor,
                    &program,
                    host,
                    false,
                    ActivationInput::default()
                )
                .shot
                .is_none()
            );
        }
        let result = advance_activation(
            &mut cursor,
            &program,
            80,
            true,
            ActivationInput {
                release: Some(ActivationRelease {
                    token: token(5),
                    release_tick: 65,
                }),
                ..ActivationInput::default()
            },
        );
        assert!((result.execution_charge.unwrap() - 1.0).abs() < 1e-6);
        assert_eq!(result.shot.unwrap().shot_id.ordinal, 0);
        assert!(
            advance_activation(
                &mut cursor,
                &program,
                200,
                false,
                ActivationInput::default()
            )
            .terminal
            .is_none()
        );
        let mut compressed = start_activation(token(5), 7, 20, &program);
        let result = advance_activation(
            &mut compressed,
            &program,
            30,
            true,
            ActivationInput {
                release: Some(ActivationRelease {
                    token: token(5),
                    release_tick: 65,
                }),
                ..ActivationInput::default()
            },
        );
        assert!((result.execution_charge.unwrap() - 19.0 / 60.0).abs() < 1e-6);
    }
    #[test]
    fn activation_cancel_wins_release_and_early_release_spends_no_shot() {
        let program = ActivationProgram::new(
            vec![ActivationStep::Shot],
            Some(ChargeTiming {
                min_ticks: 12,
                full_ticks: 60,
            }),
            24,
        )
        .unwrap();
        let mut cursor = start_activation(token(5), 7, 0, &program);
        let result = advance_activation(
            &mut cursor,
            &program,
            1,
            true,
            ActivationInput {
                release: Some(ActivationRelease {
                    token: token(5),
                    release_tick: 6,
                }),
                ..ActivationInput::default()
            },
        );
        assert_eq!(result.terminal, Some(ActivationTermination::EarlyRelease));
        assert!(result.shot.is_none());
        let mut cursor = start_activation(token(5), 7, 0, &program);
        let result = advance_activation(
            &mut cursor,
            &program,
            60,
            true,
            ActivationInput {
                release: Some(ActivationRelease {
                    token: token(5),
                    release_tick: 65,
                }),
                cancel: Some(token(5)),
                initiation: None,
            },
        );
        assert_eq!(result.terminal, Some(ActivationTermination::Cancelled));
        assert!(result.shot.is_none());
    }
    #[test]
    fn activation_input_outage_cancels_without_a_ghost_shot() {
        let program = ActivationProgram::new(
            vec![
                ActivationStep::Shot,
                ActivationStep::Wait { ticks: 200 },
                ActivationStep::Shot,
            ],
            None,
            18,
        )
        .unwrap();
        let mut cursor = start_activation(token(1), 7, 0, &program);
        assert!(
            advance_activation(&mut cursor, &program, 0, true, ActivationInput::default())
                .shot
                .is_some()
        );
        assert_eq!(
            advance_activation(
                &mut cursor,
                &program,
                120,
                false,
                ActivationInput::default()
            )
            .terminal,
            Some(ActivationTermination::InputExpired)
        );
        assert!(
            advance_activation(&mut cursor, &program, 200, true, ActivationInput::default())
                .shot
                .is_none()
        );
    }
}
