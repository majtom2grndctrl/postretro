//! Install-time binding of authored actions. Evaluation only reads installed trees.
use super::{ActivationProgram, ActivationStep, ChargeTiming, MAX_ACTIVATION_STEPS};
use crate::{
    ActivationCharge, ActivationStepDescriptor, ActivationTrigger, BakedIr, BindingScope,
    BoundProgram, CURRENT_IR_VERSION, DescriptorError, IrNode, IrType, IrValue, NumberOrIr,
    ResolvedInput, ResolvedOutput, ShotScaleDescriptor, WeaponActivationDescriptor, bind,
    eval_value_checked,
};
use std::sync::Arc;

pub const ACTIVATION_TICKS_PER_SECOND: u32 = 60;
pub const MAX_ACTIVATION_IR_DEPTH: usize = 32;
pub const MAX_ACTIVATION_IR_NODES: usize = 256;

/// Charge is the entire action namespace; it has no store or output capability.
#[derive(Debug)]
pub struct ActivationScope {
    charge: f32,
}
impl BindingScope for ActivationScope {
    type InputHandle = ();
    type OutputHandle = ();
    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<()>> {
        (name == "charge").then_some(ResolvedInput {
            handle: (),
            ir_type: IrType::Number,
        })
    }
    fn resolve_output(&self, _: &str) -> Option<ResolvedOutput<()>> {
        None
    }
    fn read(&self, _: &()) -> IrValue {
        IrValue::Number(self.charge)
    }
    fn write(&mut self, _: &(), _: IrValue) {
        unreachable!("activation scope is read-only")
    }
}

#[derive(Debug)]
enum CompiledScalar {
    Literal(f32),
    Expression(BoundProgram<ActivationScope>),
}
impl CompiledScalar {
    fn compile(
        value: &NumberOrIr,
        field: &str,
        zero_allowed: bool,
    ) -> Result<Self, DescriptorError> {
        match value {
            NumberOrIr::Literal(value) => valid_scale(*value, zero_allowed)
                .then_some(Self::Literal(*value))
                .ok_or_else(|| invalid(field, "scale must be finite and within its 0–64 domain")),
            NumberOrIr::Ir(node) => {
                check_ir(node, 1, &mut 0).map_err(|reason| invalid(field, reason))?;
                let program = bind(
                    &BakedIr {
                        version: CURRENT_IR_VERSION,
                        output: None,
                        root: node.clone(),
                    },
                    &ActivationScope { charge: 1.0 },
                )
                .map_err(|error| invalid(field, &error.to_string()))?;
                if program.root_type != IrType::Number {
                    return Err(invalid(field, "scale expression must produce a number"));
                }
                Ok(Self::Expression(program))
            }
        }
    }
    fn evaluate(&self, scope: &ActivationScope) -> Result<f32, crate::IrEvalError> {
        match self {
            Self::Literal(value) => Ok(*value),
            Self::Expression(program) => match eval_value_checked(program, scope)? {
                IrValue::Number(value) => Ok(value),
                IrValue::Bool(_) => Err(crate::IrEvalError),
            },
        }
    }
}

/// Seven independently bound scale axes; array order stays private to this module.
#[derive(Debug)]
pub struct CompiledShotScales {
    values: [CompiledScalar; 7],
}
const SCALE_FIELDS: [&str; 7] = [
    "damage",
    "range",
    "projectileSpeed",
    "projectileRadius",
    "projectileSize",
    "knockbackSpeed",
    "resourceCost",
];
impl CompiledShotScales {
    fn compile(scale: &ShotScaleDescriptor, path: &str) -> Result<Self, DescriptorError> {
        let inputs = [
            &scale.damage,
            &scale.range,
            &scale.projectile_speed,
            &scale.projectile_radius,
            &scale.projectile_size,
            &scale.knockback_speed,
            &scale.resource_cost,
        ];
        let mut values = Vec::with_capacity(7);
        for (index, input) in inputs.into_iter().enumerate() {
            values.push(CompiledScalar::compile(
                input,
                &format!("{path}.{}", SCALE_FIELDS[index]),
                index == 0 || index == 5,
            )?);
        }
        let values = values
            .try_into()
            .map_err(|_| invalid(path, "invalid scale axis count"))?;
        Ok(Self { values })
    }
    fn evaluate(&self, charge: f32) -> Result<ResolvedShotScales, ActivationScaleError> {
        if !charge.is_finite() || !(0.0..=1.0).contains(&charge) {
            return Err(ActivationScaleError { field: "charge" });
        }
        let scope = ActivationScope { charge };
        let mut result = [1.0; 7];
        for (index, scalar) in self.values.iter().enumerate() {
            result[index] = scalar.evaluate(&scope).map_err(|_| ActivationScaleError {
                field: SCALE_FIELDS[index],
            })?;
            if !valid_scale(result[index], index == 0 || index == 5) {
                return Err(ActivationScaleError {
                    field: SCALE_FIELDS[index],
                });
            }
        }
        Ok(ResolvedShotScales {
            damage: result[0],
            range: result[1],
            projectile_speed: result[2],
            projectile_radius: result[3],
            projectile_size: result[4],
            knockback_speed: result[5],
            resource_cost: result[6],
        })
    }
}

/// One installed action wraps the canonical timing program, with one scale row per shot.
#[derive(Debug)]
pub struct CompiledActivation {
    pub timing: ActivationProgram,
    pub trigger: ActivationTrigger,
    pub recovery_ms: f32,
    pub scales: Arc<[CompiledShotScales]>,
}
impl CompiledActivation {
    pub fn compile(
        action: &WeaponActivationDescriptor,
        path: &str,
    ) -> Result<Self, DescriptorError> {
        if action.steps.is_empty() || action.steps.len() > MAX_ACTIVATION_STEPS {
            return Err(invalid(path, "must contain 1–64 steps"));
        }
        duration(action.recovery_ms, true, &format!("{path}.recoveryMs"))?;
        let charge = action
            .charge
            .as_ref()
            .map(|charge| compile_charge(charge, action.trigger, path))
            .transpose()?;
        let mut steps = Vec::with_capacity(action.steps.len());
        let mut scales = Vec::new();
        for (index, step) in action.steps.iter().enumerate() {
            match step {
                ActivationStepDescriptor::Shot { scale } => {
                    steps.push(ActivationStep::Shot);
                    scales.push(CompiledShotScales::compile(
                        scale,
                        &format!("{path}.steps[{index}].scale"),
                    )?);
                }
                ActivationStepDescriptor::Wait { duration_ms } => {
                    duration(
                        *duration_ms,
                        false,
                        &format!("{path}.steps[{index}].durationMs"),
                    )?;
                    steps.push(ActivationStep::Wait {
                        ticks: activation_duration_ticks(*duration_ms),
                    });
                }
            }
        }
        let timing =
            ActivationProgram::new(steps, charge, activation_duration_ticks(action.recovery_ms))
                .map_err(|reason| invalid(path, reason))?;
        Ok(Self {
            timing,
            trigger: action.trigger,
            recovery_ms: action.recovery_ms,
            scales: scales.into(),
        })
    }
    pub fn resolve_scales(
        &self,
        ordinal: u8,
        charge: f32,
    ) -> Result<ResolvedShotScales, ActivationScaleError> {
        self.scales
            .get(usize::from(ordinal))
            .ok_or(ActivationScaleError { field: "ordinal" })?
            .evaluate(if self.timing.charge.is_some() {
                charge
            } else {
                1.0
            })
    }
}

/// One quantizer for authoring, installation and host-provided actions.
pub fn activation_duration_ticks(ms: f32) -> u32 {
    (f64::from(ms) * f64::from(ACTIVATION_TICKS_PER_SECOND) / 1000.0).ceil() as u32
}

/// Milliseconds in `ticks` fixed activation ticks: the host's recovery unit
/// back in the component's millisecond cooldown.
pub fn activation_ticks_ms(ticks: u32) -> f32 {
    ticks as f32 * 1000.0 / ACTIVATION_TICKS_PER_SECOND as f32
}
fn compile_charge(
    charge: &ActivationCharge,
    trigger: ActivationTrigger,
    path: &str,
) -> Result<ChargeTiming, DescriptorError> {
    if trigger != ActivationTrigger::Press {
        return Err(invalid(path, "charge requires trigger `press`"));
    }
    duration(charge.min_ms, true, &format!("{path}.charge.minMs"))?;
    duration(charge.full_ms, false, &format!("{path}.charge.fullMs"))?;
    if charge.min_ms > charge.full_ms {
        return Err(invalid(
            path,
            "charge minimum must not exceed full duration",
        ));
    }
    Ok(ChargeTiming {
        min_ticks: activation_duration_ticks(charge.min_ms),
        full_ticks: activation_duration_ticks(charge.full_ms),
    })
}
fn duration(value: f32, zero_allowed: bool, path: &str) -> Result<(), DescriptorError> {
    if value.is_finite()
        && value <= 60000.0
        && if zero_allowed {
            value >= 0.0
        } else {
            value > 0.0
        }
    {
        Ok(())
    } else {
        Err(invalid(
            path,
            "duration must be finite, at most 60000 ms, and positive (zero is permitted for recovery/minimum)",
        ))
    }
}
fn valid_scale(value: f32, zero_allowed: bool) -> bool {
    value.is_finite()
        && value <= 64.0
        && if zero_allowed {
            value >= 0.0
        } else {
            value > 0.0
        }
}
fn invalid(path: &str, reason: &str) -> DescriptorError {
    DescriptorError::InvalidShape {
        reason: format!("`{path}` {reason}"),
    }
}
fn check_ir(node: &IrNode, depth: usize, nodes: &mut usize) -> Result<(), &'static str> {
    *nodes += 1;
    if depth > MAX_ACTIVATION_IR_DEPTH || *nodes > MAX_ACTIVATION_IR_NODES {
        return Err("expression exceeds depth 32 or 256 nodes");
    }
    let mut visit = |child: &IrNode| check_ir(child, depth + 1, nodes);
    match node {
        IrNode::Const {
            value: IrValue::Number(value),
        } if !value.is_finite() => Err("expression constants must be finite"),
        IrNode::Const { .. } | IrNode::Input { .. } => Ok(()),
        IrNode::Add { a, b }
        | IrNode::Sub { a, b }
        | IrNode::Mul { a, b }
        | IrNode::Div { a, b }
        | IrNode::Lt { a, b }
        | IrNode::Le { a, b }
        | IrNode::Gt { a, b }
        | IrNode::Ge { a, b }
        | IrNode::Eq { a, b }
        | IrNode::Ne { a, b }
        | IrNode::And { a, b }
        | IrNode::Or { a, b } => {
            visit(a)?;
            visit(b)
        }
        IrNode::Not { x } => visit(x),
        IrNode::Clamp { x, lo, hi } => {
            visit(x)?;
            visit(lo)?;
            visit(hi)
        }
        IrNode::Lerp { a, b, t } => {
            visit(a)?;
            visit(b)?;
            visit(t)
        }
        IrNode::Select { cond, a, b } => {
            visit(cond)?;
            visit(a)?;
            visit(b)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedShotScales {
    pub damage: f32,
    pub range: f32,
    pub projectile_speed: f32,
    pub projectile_radius: f32,
    pub projectile_size: f32,
    pub knockback_speed: f32,
    pub resource_cost: f32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivationScaleError {
    pub field: &'static str,
}

/// Re-derivable caches share storage when a weapon component is cloned. Raw actions
/// participate in component equality/serde; these bound trees deliberately do not.
#[derive(Debug, Clone, Default)]
pub struct WeaponActivationPrograms {
    installed: bool,
    pub primary: Option<Arc<CompiledActivation>>,
    pub secondary: Option<Arc<CompiledActivation>>,
}
impl PartialEq for WeaponActivationPrograms {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl WeaponActivationPrograms {
    pub fn is_installed(&self) -> bool {
        self.installed
    }
    pub fn install(
        primary: &WeaponActivationDescriptor,
        secondary: Option<&WeaponActivationDescriptor>,
    ) -> Self {
        let install = |action, path| match CompiledActivation::compile(action, path) {
            Ok(program) => Some(Arc::new(program)),
            Err(error) => {
                log::warn!("[Weapon] cannot install activation: {error}");
                None
            }
        };
        Self {
            installed: true,
            primary: install(primary, "components.weapon.primary"),
            secondary: secondary.and_then(|action| install(action, "components.weapon.secondary")),
        }
    }
}
