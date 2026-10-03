// See: context/lib/entity_model.md §4, §5 · context/lib/networking.md
//! Central activation transitions and per-shot resource authority.
use super::{FireButtonState, WeaponFireAuthorization};
use postretro_combat_model::activation::{
    ActivationTermination, advance_activation, start_activation,
};
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::components::wieldable_state::WieldableState;
use postretro_foundation::{
    ActivationInput, ActivationLane, ActivationPhase, ActivationToken, CompiledActivation,
    ProjectileBodyVisual, ScaledShotValues, ShotId, ShotResourceCost, ShotScaleInputs,
    WeaponActivationDescriptor,
};
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
pub struct ActivationCommand {
    pub tick: u32,
    pub pawn: u32,
    pub real_command: bool,
    pub input: ActivationInput,
    /// Only the local/AI controller may synthesize explicit requests from held input.
    pub controller_starts: bool,
    pub primary: FireButtonState,
    pub secondary: FireButtonState,
}

#[derive(Debug, Clone)]
pub struct ResolvedActivationShot {
    pub shot_id: ShotId,
    pub charge: f32,
    /// Reserved in fixed-tick order before this shot grows bloom.
    pub shell_counter: u32,
    pub bloom_degrees: f32,
    pub program: Arc<CompiledActivation>,
    pub base: ShotScaleInputs,
    pub values: ScaledShotValues,
    pub action: Arc<WeaponActivationDescriptor>,
    pub recovery_ms: f32,
}
#[derive(Debug, Clone, Default)]
pub struct WeaponActivationAdvance {
    /// Installed program bound before any later descriptor replacement.
    pub program: Option<Arc<CompiledActivation>>,
    pub initiated: Option<ActivationToken>,
    pub rejected: Option<ActivationToken>,
    pub execution_charge: Option<f32>,
    /// Retained even when this attempt is refused before debit.
    pub attempted: Option<ShotId>,
    pub shot: Option<ResolvedActivationShot>,
    pub authorization: Option<WeaponFireAuthorization>,
    pub terminal: Option<(ActivationToken, ActivationTermination)>,
    pub overheat: bool,
}

impl PartialEq for ResolvedActivationShot {
    fn eq(&self, other: &Self) -> bool {
        self.shot_id == other.shot_id
            && self.charge == other.charge
            && self.shell_counter == other.shell_counter
            && self.bloom_degrees == other.bloom_degrees
            && self.base == other.base
            && self.values == other.values
            && self.action == other.action
            && self.recovery_ms == other.recovery_ms
    }
}
impl ResolvedActivationShot {
    pub fn with_authoritative_charge(
        &self,
        charge: f32,
    ) -> Result<Self, postretro_foundation::ActivationScaleError> {
        let charge = if self.program.timing.charge.is_some() {
            charge
        } else {
            1.0
        };
        let values = self
            .program
            .resolve_scales(self.shot_id.ordinal, charge)?
            .apply(self.base)?;
        let mut corrected = self.clone();
        corrected.charge = charge;
        corrected.values = values;
        Ok(corrected)
    }
}
impl PartialEq for WeaponActivationAdvance {
    fn eq(&self, other: &Self) -> bool {
        self.initiated == other.initiated
            && self.rejected == other.rejected
            && self.execution_charge == other.execution_charge
            && self.attempted == other.attempted
            && self.shot == other.shot
            && self.authorization == other.authorization
            && self.terminal == other.terminal
            && self.overheat == other.overheat
    }
}

pub fn action_program(
    weapon: &WeaponComponent,
    lane: ActivationLane,
) -> Option<&Arc<CompiledActivation>> {
    match lane {
        ActivationLane::Primary => weapon.activation_programs.primary.as_ref(),
        ActivationLane::Secondary => weapon.activation_programs.secondary.as_ref(),
    }
}
pub fn action_descriptor(
    weapon: &WeaponComponent,
    lane: ActivationLane,
) -> Option<&Arc<WeaponActivationDescriptor>> {
    match lane {
        ActivationLane::Primary => Some(&weapon.primary),
        ActivationLane::Secondary => weapon.secondary.as_ref(),
    }
}

/// The controller chooses a request; remote authority never manufactures held restarts.
pub fn controller_activation_request(
    weapon: &WeaponComponent,
    primary: FireButtonState,
    secondary: FireButtonState,
    tick: u32,
) -> Option<ActivationToken> {
    let eligible = |lane, button: FireButtonState, consumed: bool| {
        action_program(weapon, lane).is_some_and(|program| match program.trigger {
            postretro_foundation::ActivationTrigger::Press => button.pressed && !consumed,
            postretro_foundation::ActivationTrigger::Hold => {
                button.active
                    && (weapon.state.allows_fire() || weapon.state == WieldableState::ShellLoading)
                    && weapon.cooldown_remaining_ms <= 0.0
            }
        })
    };
    let lane = if eligible(
        ActivationLane::Secondary,
        secondary,
        weapon.secondary_press_consumed,
    ) {
        ActivationLane::Secondary
    } else if eligible(
        ActivationLane::Primary,
        primary,
        weapon.shoot_press_consumed,
    ) {
        ActivationLane::Primary
    } else {
        return None;
    };
    Some(ActivationToken {
        start_tick: tick,
        lane,
    })
}

pub fn shot_scale_inputs(weapon: &WeaponComponent) -> ShotScaleInputs {
    let stats = weapon.effective();
    ShotScaleInputs {
        damage: stats.damage,
        range: stats.range,
        projectile_speed: stats.projectile.map(|p| p.speed),
        projectile_radius: stats.projectile.map(|p| p.radius),
        projectile_size: stats.projectile.map(|p| match p.visual.body {
            ProjectileBodyVisual::Sprite { size, .. } => size,
            ProjectileBodyVisual::Model { .. } => 1.0,
        }),
        knockback_speed: stats.knockback.map(|k| k.speed),
        splash_knockback_speed: stats.splash.and_then(|s| s.knockback.map(|k| k.speed)),
        resource_cost: if let Some(ammo) = stats.ammo {
            ShotResourceCost::Ammo(ammo.cost_per_shot)
        } else if let Some(heat) = stats.heat {
            ShotResourceCost::Heat(heat.heat_per_shot)
        } else if let Some(cell) = stats.cell {
            ShotResourceCost::Cell(cell.cost_per_shot)
        } else {
            ShotResourceCost::None
        },
    }
}

pub fn shot_resource_verdict(
    weapon: &WeaponComponent,
    cost: ShotResourceCost,
) -> WeaponFireAuthorization {
    if weapon.heat.is_some_and(|heat| heat.overheated) {
        return WeaponFireAuthorization::Rejected;
    }
    match cost {
        ShotResourceCost::Ammo(cost) if weapon.magazine < cost => WeaponFireAuthorization::Empty,
        ShotResourceCost::Cell(cost) if weapon.cell.is_none_or(|cell| cell.charge < cost) => {
            WeaponFireAuthorization::Empty
        }
        _ => WeaponFireAuthorization::Accepted,
    }
}
fn debit(weapon: &mut WeaponComponent, cost: ShotResourceCost) -> bool {
    match cost {
        ShotResourceCost::Ammo(cost) => weapon.magazine -= cost,
        ShotResourceCost::Heat(cost) => {
            if let Some(heat) = weapon.heat.as_mut() {
                heat.heat += cost;
                heat.idle_ms = 0.0;
                if heat.heat >= heat.tuning.overheat_at {
                    heat.heat = heat.tuning.overheat_at;
                    heat.overheated = true;
                    return true;
                }
            }
        }
        ShotResourceCost::Cell(cost) => {
            if let Some(cell) = weapon.cell.as_mut() {
                cell.charge = (cell.charge - cost).max(0.0);
                cell.idle_ms = 0.0;
            }
        }
        ShotResourceCost::None => {}
    }
    false
}

/// Advance once per fixed tick. Prediction runs identical timing and validation,
/// while resources remain authoritative; stale projections choose cosmetics only.
pub fn advance_weapon_activation(
    weapon: &mut WeaponComponent,
    command: ActivationCommand,
    dt_ms: f32,
    predicted: bool,
    allow_start: bool,
) -> WeaponActivationAdvance {
    advance_weapon_activation_with_shell_preemption(
        weapon,
        command,
        dt_ms,
        predicted,
        allow_start,
        true,
        false,
        |_| {},
    )
}

/// Shell interruption and its first due shot share one evaluation and resource gate.
#[allow(clippy::too_many_arguments)]
pub(crate) fn advance_weapon_activation_with_shell_preemption(
    weapon: &mut WeaponComponent,
    mut command: ActivationCommand,
    dt_ms: f32,
    predicted: bool,
    allow_start: bool,
    allow_due_shot: bool,
    allow_shell_preemption: bool,
    mut preempt: impl FnMut(&mut WeaponComponent),
) -> WeaponActivationAdvance {
    let mut result = WeaponActivationAdvance::default();
    let mut prepared = None;
    if weapon.last_activation_tick == Some(command.tick) {
        return result;
    }
    weapon.last_activation_tick = Some(command.tick);
    weapon.activation_clock = command.tick.wrapping_add(1);
    weapon.tick_bloom(dt_ms.max(0.0));
    weapon.cooldown_remaining_ms = (weapon.cooldown_remaining_ms - dt_ms.max(0.0)).max(0.0);
    if command.controller_starts && command.input.initiation.is_none() {
        command.input.initiation =
            controller_activation_request(weapon, command.primary, command.secondary, command.tick);
    }
    // Fresh blocked presses are consumed before any gate decision.
    if command.primary.pressed {
        weapon.shoot_press_consumed = true;
    } else if !command.primary.active {
        weapon.shoot_press_consumed = false;
    }
    if command.secondary.pressed {
        weapon.secondary_press_consumed = true;
    } else if !command.secondary.active {
        weapon.secondary_press_consumed = false;
    }
    if let Some(token) = command.input.initiation {
        let program = action_program(weapon, token.lane).cloned();
        let shell = weapon.state == WieldableState::ShellLoading && allow_shell_preemption;
        let can_start = command.real_command
            && allow_start
            && weapon.cooldown_remaining_ms <= 0.0
            && (weapon.state.allows_fire() || shell)
            && program.is_some();
        if can_start {
            let program = program.unwrap();
            let mut cursor = start_activation(token, command.pawn, command.tick, &program.timing);
            let mut accepted = true;
            if shell {
                // Charged actions cannot interrupt the existing per-shell loop.
                accepted = program.timing.charge.is_none();
                if accepted {
                    let advanced = advance_activation(
                        &mut cursor,
                        &program.timing,
                        command.tick,
                        command.real_command,
                        command.input,
                    );
                    if let Some(due) = advanced.shot {
                        let base = shot_scale_inputs(weapon);
                        let values = program
                            .resolve_scales(due.shot_id.ordinal, due.charge)
                            .and_then(|scales| scales.apply(base));
                        accepted = values.as_ref().is_ok_and(|values| {
                            predicted
                                || shot_resource_verdict(weapon, values.resource_cost)
                                    == WeaponFireAuthorization::Accepted
                        });
                        if accepted {
                            prepared = Some((advanced, values, base));
                        } else if let Err(error) = values {
                            warn_invalid_scale(weapon, error.field);
                        }
                    } else {
                        accepted = false;
                    }
                }
            }
            if accepted {
                if shell {
                    preempt(weapon);
                }
                weapon.state = if cursor.phase == ActivationPhase::Charging {
                    WieldableState::Charging(cursor)
                } else {
                    WieldableState::Executing(cursor)
                };
                result.program = Some(program.clone());
                result.initiated = Some(token);
                if cursor.phase != ActivationPhase::Charging {
                    result.execution_charge = Some(1.0);
                }
            } else {
                result.rejected = Some(token);
            }
        } else {
            result.rejected = Some(token);
        }
    }
    let Some(mut cursor) = weapon.state.activation_cursor() else {
        return result;
    };
    let Some(program) = action_program(weapon, cursor.token.lane).cloned() else {
        weapon.cancel_activation();
        result.terminal = Some((cursor.token, ActivationTermination::Cancelled));
        return result;
    };
    result.program = Some(program.clone());
    let (advanced, prepared_values) = match prepared {
        Some((advanced, values, base)) => (advanced, Some((values, base))),
        None => (
            advance_activation(
                &mut cursor,
                &program.timing,
                command.tick,
                command.real_command,
                command.input,
            ),
            None,
        ),
    };
    result.execution_charge = advanced.execution_charge.or(result.execution_charge);
    if let Some(due) = advanced.shot {
        result.attempted = Some(due.shot_id);
        if !allow_due_shot {
            weapon.cancel_activation();
            result.authorization = Some(WeaponFireAuthorization::Rejected);
            result.terminal = Some((cursor.token, ActivationTermination::Cancelled));
            return result;
        }
        let (values, base) = prepared_values.unwrap_or_else(|| {
            let base = shot_scale_inputs(weapon);
            (
                program
                    .resolve_scales(due.shot_id.ordinal, due.charge)
                    .and_then(|scales| scales.apply(base)),
                base,
            )
        });
        match values {
            Ok(values) => {
                let verdict = if predicted {
                    WeaponFireAuthorization::Accepted
                } else {
                    shot_resource_verdict(weapon, values.resource_cost)
                };
                result.authorization = Some(verdict);
                if verdict != WeaponFireAuthorization::Rejected {
                    weapon.cooldown_remaining_ms = program.recovery_ms;
                }
                if verdict == WeaponFireAuthorization::Accepted {
                    let shell_counter = weapon.shells_fired;
                    let bloom_degrees = weapon.bloom_accumulator_degrees;
                    weapon.shells_fired = weapon.shells_fired.wrapping_add(1);
                    if weapon.resolution == postretro_foundation::ResolutionMode::Hitscan {
                        weapon.apply_bloom_shot();
                    }
                    result.overheat = !predicted && debit(weapon, values.resource_cost);
                    result.shot = Some(ResolvedActivationShot {
                        shot_id: due.shot_id,
                        charge: due.charge,
                        shell_counter,
                        bloom_degrees,
                        values,
                        base,
                        program: program.clone(),
                        action: action_descriptor(weapon, cursor.token.lane)
                            .unwrap()
                            .clone(),
                        recovery_ms: program.recovery_ms,
                    });
                }
                if verdict != WeaponFireAuthorization::Accepted || result.overheat {
                    cursor.phase = ActivationPhase::Terminal;
                    result.terminal = Some((cursor.token, ActivationTermination::Cancelled));
                }
            }
            Err(error) => {
                warn_invalid_scale(weapon, error.field);
                cursor.phase = ActivationPhase::Terminal;
                result.authorization = Some(WeaponFireAuthorization::Rejected);
                result.terminal = Some((cursor.token, ActivationTermination::Cancelled));
            }
        }
    }
    if result.terminal.is_none() {
        result.terminal = advanced.terminal.map(|reason| (cursor.token, reason));
    }
    weapon.state = match cursor.phase {
        ActivationPhase::Charging => WieldableState::Charging(cursor),
        ActivationPhase::Executing => WieldableState::Executing(cursor),
        ActivationPhase::Terminal => WieldableState::Idle,
    };
    result
}
fn warn_invalid_scale(weapon: &WeaponComponent, field: &'static str) {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static WARNED: OnceLock<Mutex<HashMap<Arc<str>, u16>>> = OnceLock::new();
    let bit = match field {
        "damage" => 1,
        "range" => 2,
        "projectileSpeed" => 4,
        "projectileRadius" => 8,
        "projectileSize" => 16,
        "knockbackSpeed" => 32,
        "splashKnockbackSpeed" => 64,
        "resourceCost" => 128,
        _ => 256,
    };
    let mut warned = WARNED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if warned
        .get(weapon.descriptor_identity.as_ref())
        .is_some_and(|bits| bits & bit != 0)
    {
        return;
    }
    *warned
        .entry(weapon.descriptor_identity.clone())
        .or_default() |= bit;
    log::warn!(
        "[Weapon] invalid activation value for {}.{field}; cancelling execution",
        weapon.descriptor_identity
    );
}

/// Authoritative passive update shared by carried and private AI weapon instances.
pub fn advance_authoritative_weapon_resource(weapon: &mut WeaponComponent, dt_ms: f32) {
    crate::sim::weapon_stage::resource::advance_weapon_resource(weapon, dt_ms.max(0.0));
}

/// Mutable state checkpoint for an effect that can fail to materialize. Installed
/// tuning is shared and unchanged; passive updates precede this transaction.
#[derive(Debug, Clone, Copy)]
pub struct WeaponActivationCheckpoint {
    state: WieldableState,
    activation_clock: u32,
    last_activation_tick: Option<u32>,
    shoot_press_consumed: bool,
    secondary_press_consumed: bool,
    reload_press_consumed: bool,
    cooldown_remaining_ms: f32,
    magazine: u32,
    heat: Option<postretro_entities::components::weapon_resource::WeaponHeat>,
    cell: Option<postretro_entities::components::weapon_resource::WeaponCell>,
    shells_fired: u32,
    bloom_accumulator_degrees: f32,
    bloom_idle_ms: f32,
}
impl WeaponActivationCheckpoint {
    pub fn capture(weapon: &WeaponComponent) -> Self {
        Self {
            state: weapon.state,
            activation_clock: weapon.activation_clock,
            last_activation_tick: weapon.last_activation_tick,
            shoot_press_consumed: weapon.shoot_press_consumed,
            secondary_press_consumed: weapon.secondary_press_consumed,
            reload_press_consumed: weapon.reload_press_consumed,
            cooldown_remaining_ms: weapon.cooldown_remaining_ms,
            magazine: weapon.magazine,
            heat: weapon.heat,
            cell: weapon.cell,
            shells_fired: weapon.shells_fired,
            bloom_accumulator_degrees: weapon.bloom_accumulator_degrees,
            bloom_idle_ms: weapon.bloom_idle_ms,
        }
    }
    pub fn restore(self, weapon: &mut WeaponComponent) {
        weapon.state = self.state;
        weapon.activation_clock = self.activation_clock;
        weapon.last_activation_tick = self.last_activation_tick;
        weapon.shoot_press_consumed = self.shoot_press_consumed;
        weapon.secondary_press_consumed = self.secondary_press_consumed;
        weapon.reload_press_consumed = self.reload_press_consumed;
        weapon.cooldown_remaining_ms = self.cooldown_remaining_ms;
        weapon.magazine = self.magazine;
        weapon.heat = self.heat;
        weapon.cell = self.cell;
        weapon.shells_fired = self.shells_fired;
        weapon.bloom_accumulator_degrees = self.bloom_accumulator_degrees;
        weapon.bloom_idle_ms = self.bloom_idle_ms;
    }
}

/// Level teardown and platform suspension stop pending work on retained instances.
/// Paid resource and recovery state remain owed; ordinary reloads are unaffected.
pub fn cancel_weapon_activations(registry: &mut postretro_entities::EntityRegistry) {
    registry.for_each_with_kind_mut(postretro_entities::ComponentKind::Weapon, |_, value| {
        if let postretro_entities::ComponentValue::Weapon(weapon) = value {
            weapon.cancel_activation();
        }
    });
}
