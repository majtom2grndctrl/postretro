//! Engine-side codec for host-resolved prediction tuning.
//!
//! The net crate carries these bytes opaquely. Keeping the descriptor types here
//! avoids a wire mirror that would make the transport registry-aware.

use postretro_foundation::{FireMode, PlayerMovementDescriptor, ResolutionMode};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Bump whenever the semantic JSON payload shape changes. This is independent
/// of the bitcode wire version because the payload itself is JSON.
pub(crate) const TUNING_PAYLOAD_EPOCH: u32 = 1;

/// The four default-weapon values a client predicts locally.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DefaultWeaponFirePayload {
    pub(crate) range: f32,
    pub(crate) cooldown_ms: f32,
    pub(crate) fire_mode: FireMode,
    pub(crate) resolution: ResolutionMode,
}

/// Host-resolved tuning for one participating pawn.
///
/// Both halves are optional: a pawn class may have no movement descriptor or
/// no resolvable default weapon. `movement.view_feel` is always cleared because
/// view feel is local presentation rather than predicted simulation tuning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct TuningPayload {
    epoch: u32,
    pub(crate) movement: Option<PlayerMovementDescriptor>,
    pub(crate) default_weapon: Option<DefaultWeaponFirePayload>,
}

impl TuningPayload {
    pub(crate) fn new(
        mut movement: Option<PlayerMovementDescriptor>,
        default_weapon: Option<DefaultWeaponFirePayload>,
    ) -> Self {
        if let Some(descriptor) = movement.as_mut() {
            descriptor.view_feel = None;
        }
        Self {
            epoch: TUNING_PAYLOAD_EPOCH,
            movement,
            default_weapon,
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum TuningPayloadError {
    #[error("tuning payload is truncated")]
    Truncated,
    #[error("tuning payload is malformed: {source}")]
    Malformed {
        #[source]
        source: serde_json::Error,
    },
    #[error("tuning payload epoch mismatch: expected {expected}, received {received}")]
    EpochMismatch { expected: u32, received: u32 },
}

/// Serialize a payload in its canonical JSON form.
pub(crate) fn encode_tuning_payload(payload: &TuningPayload) -> Vec<u8> {
    let canonical = TuningPayload::new(payload.movement.clone(), payload.default_weapon.clone());
    serde_json::to_vec(&canonical)
        .expect("tuning payload only contains validated descriptor values")
}

/// Decode and validate an opaque tuning payload received over Control.
pub(crate) fn decode_tuning_payload(data: &[u8]) -> Result<TuningPayload, TuningPayloadError> {
    let mut payload: TuningPayload = serde_json::from_slice(data).map_err(|source| {
        if source.is_eof() {
            TuningPayloadError::Truncated
        } else {
            TuningPayloadError::Malformed { source }
        }
    })?;
    if payload.epoch != TUNING_PAYLOAD_EPOCH {
        return Err(TuningPayloadError::EpochMismatch {
            expected: TUNING_PAYLOAD_EPOCH,
            received: payload.epoch,
        });
    }
    if let Some(descriptor) = payload.movement.as_mut() {
        descriptor.view_feel = None;
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use postretro_foundation::{
        AirParams, BoolOrIr, CapsuleParams, DashParams, FallParams, FireMode, ForgivenessParams,
        GroundParams, NumberOrIr, PlayerMovementDescriptor, SpeedParams, ViewFeelParams,
    };

    use super::*;

    const BLESS_ENV: &str = "POSTRETRO_BLESS_COMPATIBILITY_FIXTURES";
    const FIXTURE_PATH: &str = "src/netcode/tests/fixtures/tuning_payload.expected.json";

    fn movement_descriptor() -> PlayerMovementDescriptor {
        PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.4,
                half_height: 0.8,
                eye_height: 0.5,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 4.0,
                    run: 7.5,
                    crouch: 2.0,
                },
                accel: 18.0,
                step_height: 0.35,
                max_slope: 48.0,
            },
            air: AirParams {
                forward_steer: 0.25,
                accel: 3.0,
                max_control_speed: 8.0,
                bunny_hop: true,
                jumps: 2,
                jump_velocity: 5.5,
                jump_ceiling: 1.5,
            },
            fall: FallParams {
                terminal_velocity: 42.0,
            },
            stuck_stop_enabled: false,
            stuck_stop_threshold: 0.02,
            dash: Some(DashParams {
                boost_speed: NumberOrIr::Literal(18.0),
                momentum_retention: NumberOrIr::Ir(postretro_foundation::ir::IrNode::Clamp {
                    x: Box::new(postretro_foundation::ir::IrNode::Input {
                        name: "movement.speed".to_string(),
                    }),
                    lo: Box::new(postretro_foundation::ir::IrNode::Const {
                        value: postretro_foundation::ir::IrValue::Number(0.0),
                    }),
                    hi: Box::new(postretro_foundation::ir::IrNode::Const {
                        value: postretro_foundation::ir::IrValue::Number(1.0),
                    }),
                }),
                steer_control: NumberOrIr::Literal(0.2),
                dash_drag: NumberOrIr::Literal(4.0),
                cooldown_ms: NumberOrIr::Literal(300.0),
                air_dashes: 1,
                preserve_vertical: BoolOrIr::Literal(true),
            }),
            forgiveness: Some(ForgivenessParams {
                coyote_ms: 90.0,
                jump_buffer_ms: 110.0,
            }),
            crouch: None,
            view_feel: Some(ViewFeelParams {
                bob: None,
                tilt: None,
                sway: None,
            }),
        }
    }

    fn default_weapon() -> DefaultWeaponFirePayload {
        DefaultWeaponFirePayload {
            range: 128.0,
            cooldown_ms: 125.0,
            fire_mode: FireMode::Auto,
            resolution: ResolutionMode::Hitscan,
        }
    }

    fn full_payload() -> TuningPayload {
        TuningPayload::new(Some(movement_descriptor()), Some(default_weapon()))
    }

    #[test]
    fn payload_round_trips_nested_and_ir_tuning_without_view_feel() {
        let descriptor = movement_descriptor();
        assert!(descriptor.view_feel.is_some());
        let payload = TuningPayload {
            epoch: TUNING_PAYLOAD_EPOCH,
            movement: Some(descriptor),
            default_weapon: Some(default_weapon()),
        };

        let encoded = encode_tuning_payload(&payload);
        let json: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert!(json["movement"]["view_feel"].is_null());

        let decoded = decode_tuning_payload(&encoded).unwrap();
        assert_eq!(
            decoded,
            TuningPayload::new(payload.movement, payload.default_weapon)
        );
        let dash = decoded.movement.unwrap().dash.unwrap();
        assert_eq!(dash.boost_speed, NumberOrIr::Literal(18.0));
        assert!(matches!(dash.momentum_retention, NumberOrIr::Ir(_)));
    }

    #[test]
    fn payload_round_trips_absent_halves() {
        let payload = TuningPayload::new(None, None);
        let decoded = decode_tuning_payload(&encode_tuning_payload(&payload)).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn payload_rejects_truncation_and_epoch_mismatch() {
        let encoded = encode_tuning_payload(&full_payload());
        assert!(matches!(
            decode_tuning_payload(&encoded[..encoded.len() - 1]),
            Err(TuningPayloadError::Truncated)
        ));

        let mut json: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        json["epoch"] = serde_json::json!(TUNING_PAYLOAD_EPOCH + 1);
        let mismatched = serde_json::to_vec(&json).unwrap();
        assert!(matches!(
            decode_tuning_payload(&mismatched),
            Err(TuningPayloadError::EpochMismatch {
                expected: TUNING_PAYLOAD_EPOCH,
                received
            }) if received == TUNING_PAYLOAD_EPOCH + 1
        ));
    }

    #[test]
    fn payload_json_matches_committed_fixture() {
        let actual = String::from_utf8(encode_tuning_payload(&full_payload())).unwrap();
        if std::env::var_os(BLESS_ENV).is_some() {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_PATH);
            fs::write(path, actual).expect("write tuning payload fixture");
            return;
        }

        assert_eq!(
            actual,
            include_str!("tests/fixtures/tuning_payload.expected.json"),
            "tuning payload JSON changed; bump TUNING_PAYLOAD_EPOCH and the wire version for a semantic payload change, or re-bless with {BLESS_ENV}=1 for a non-semantic rendering change"
        );
    }
}
