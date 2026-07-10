//! Engine-owned declarative trigger state. Spatial AABBs live in TriggerVolumeBridge.

use super::kinematic_mover::MoverCommand;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerActivation {
    Touch,
    Use,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerFireMode {
    Once,
    Multiple,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerVolumeComponent {
    pub activation: TriggerActivation,
    pub target_tag: String,
    pub command: MoverCommand,
    pub fire_mode: TriggerFireMode,
    pub rearm_ms: f32,
    pub enabled_on_spawn: bool,
    pub armed: bool,
    pub latched: bool,
    pub rearm_remaining_ms: f32,
}

impl TriggerVolumeComponent {
    pub fn new(
        activation: TriggerActivation,
        target_tag: String,
        command: MoverCommand,
        fire_mode: TriggerFireMode,
        rearm_ms: f32,
        enabled_on_spawn: bool,
    ) -> Self {
        Self {
            activation,
            target_tag,
            command,
            fire_mode,
            rearm_ms,
            enabled_on_spawn,
            armed: enabled_on_spawn,
            latched: false,
            rearm_remaining_ms: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Component, ComponentKind, ComponentValue};
    #[test]
    fn mutable_state_serde_round_trips() {
        let mut trigger = TriggerVolumeComponent::new(
            TriggerActivation::Use,
            "door".into(),
            MoverCommand::Reverse,
            TriggerFireMode::Multiple,
            150.0,
            true,
        );
        trigger.armed = false;
        trigger.latched = true;
        trigger.rearm_remaining_ms = 42.0;
        let value = trigger.into_value();
        assert_eq!(value.kind(), ComponentKind::TriggerVolume);
        assert_eq!(
            serde_json::from_value::<ComponentValue>(serde_json::to_value(&value).unwrap())
                .unwrap(),
            value
        );
    }
}
