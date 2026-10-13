//! Closed authoring data for weapon actions. Runtime programs live beside the component.
use serde::{Deserialize, Serialize};

use crate::{DescriptorError, NumberOrIr, validate_sound_key};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActivationTrigger {
    Press,
    Hold,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivationCharge {
    pub min_ms: f32,
    pub full_ms: f32,
    /// Which sources may cut the charge short before its shot fires. Omitted:
    /// every source does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupted_by: Option<ChargeInterrupts>,
}

impl ActivationCharge {
    /// Whether a vault that blocks fire cancels this charge. Defaults to true.
    pub fn interrupted_by_vault(&self) -> bool {
        self.interrupted_by
            .as_ref()
            .is_none_or(|interrupts| interrupts.vault)
    }
}

/// Closed map from an interrupt source to whether it cancels a charge still
/// charging. Every key is optional and defaults to `true`. Only `vault` exists
/// today; further sources (taking damage, NPC interactions) join this map, so
/// an unknown key rejects rather than being read as a source it is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChargeInterrupts {
    /// A vault with `allowFire` off. `false` keeps the charge charging through
    /// the climb; its release fires once the climb ends.
    #[serde(default = "interrupts_by_default")]
    pub vault: bool,
}

impl Default for ChargeInterrupts {
    fn default() -> Self {
        Self { vault: true }
    }
}

fn interrupts_by_default() -> bool {
    true
}

/// Interrupt sources `ChargeInterrupts` accepts, in authored spelling.
pub const CHARGE_INTERRUPT_SOURCES: [&str; 1] = ["vault"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivationSounds {
    #[serde(default)]
    pub fire: Option<String>,
    #[serde(default)]
    pub impact: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivationEmits {
    #[serde(default)]
    pub activate: Option<String>,
    #[serde(default)]
    pub impact: Option<String>,
}

/// Independent multipliers. Omitted axes retain one; only charge is readable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct ShotScaleDescriptor {
    pub damage: NumberOrIr,
    pub range: NumberOrIr,
    pub projectile_speed: NumberOrIr,
    pub projectile_radius: NumberOrIr,
    pub projectile_size: NumberOrIr,
    pub knockback_speed: NumberOrIr,
    pub resource_cost: NumberOrIr,
}

impl Default for ShotScaleDescriptor {
    fn default() -> Self {
        Self {
            damage: 1.0.into(),
            range: 1.0.into(),
            projectile_speed: 1.0.into(),
            projectile_radius: 1.0.into(),
            projectile_size: 1.0.into(),
            knockback_speed: 1.0.into(),
            resource_cost: 1.0.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ActivationStepDescriptor {
    Shot {
        #[serde(default)]
        // Authoring-only indirection; serde retains the same scale object.
        // Fixed-tick execution reads the separately installed compiled program.
        scale: Box<ShotScaleDescriptor>,
    },
    Wait {
        #[serde(rename = "durationMs")]
        duration_ms: f32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WeaponActivationDescriptor {
    pub trigger: ActivationTrigger,
    pub recovery_ms: f32,
    pub steps: Vec<ActivationStepDescriptor>,
    #[serde(default)]
    pub charge: Option<ActivationCharge>,
    #[serde(default)]
    pub sounds: Option<ActivationSounds>,
    #[serde(default)]
    pub emits: Option<ActivationEmits>,
}

impl WeaponActivationDescriptor {
    /// Construct the ordinary single-shot action used by engine fixtures.
    pub fn single(trigger: ActivationTrigger, recovery_ms: f32) -> Self {
        Self {
            trigger,
            recovery_ms,
            steps: vec![ActivationStepDescriptor::Shot {
                scale: Box::default(),
            }],
            charge: None,
            sounds: None,
            emits: None,
        }
    }

    /// Whether this action's charge keeps charging through a vault that blocks
    /// fire (`charge.interruptedBy.vault: false`). False for an uncharged action.
    pub fn charge_survives_vault(&self) -> bool {
        self.charge
            .as_ref()
            .is_some_and(|charge| !charge.interrupted_by_vault())
    }

    pub fn validate(&self, path: &str) -> Result<(), DescriptorError> {
        crate::weapon_activation::CompiledActivation::compile(self, path)?;
        if let Some(sounds) = &self.sounds {
            for (field, key) in [
                ("fire", sounds.fire.as_deref()),
                ("impact", sounds.impact.as_deref()),
            ] {
                if let Some(key) = key {
                    validate_sound_key(&format!("{path}.sounds.{field}"), key)?;
                }
            }
        }
        if let Some(emits) = &self.emits {
            for (field, alias) in [
                ("activate", emits.activate.as_deref()),
                ("impact", emits.impact.as_deref()),
            ] {
                if let Some(alias) = alias
                    && (alias.trim().is_empty()
                        || alias.len() > crate::MAX_DESCRIPTOR_CUE_NAME_BYTES
                        || alias == field)
                {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`{path}.emits.{field}` must be non-empty, at most 256 UTF-8 bytes, and differ from the built-in `{field}` address"
                        ),
                    });
                }
            }
        }
        Ok(())
    }
}
