//! Engine-side codec for host-resolved pawn tuning and weapon placement.
//!
//! The net crate carries these bytes opaquely. Keeping the descriptor types here
//! avoids a wire mirror that would make the transport registry-aware.

use postretro_entities::components::inventory::WIELDABLE_SLOT_CAPACITY;
use postretro_foundation::{
    FireMode, PlayerMovementDescriptor, ResolutionMode, WeaponPlacementDescriptor,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Bump whenever the payload's semantic contract changes. This is independent
/// of the bitcode wire version because the payload itself is JSON.
pub(crate) const TUNING_PAYLOAD_EPOCH: u32 = 6;

/// Host-resolved values for one occupied wieldable slot.
///
/// The archetype is part of the payload because a connected client owns local
/// wieldable instances. It needs the canonical identity to materialize each
/// slot and select its presentation without consulting a host-only entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct WieldableTuningPayload {
    pub(crate) canonical_name: String,
    /// Effective host placement after mod-default and per-weapon resolution.
    pub(crate) placement: WeaponPlacementDescriptor,
    /// Host-authored model-local projectile origin. Clients pair this with
    /// `placement` from this same row rather than consulting local content.
    pub(crate) muzzle_offset: Option<[f32; 3]>,
    pub(crate) range: f32,
    pub(crate) cooldown_ms: f32,
    pub(crate) pellet_count: u32,
    pub(crate) spread_degrees: f32,
    pub(crate) fire_mode: FireMode,
    pub(crate) resolution: ResolutionMode,
    pub(crate) lower_ms: u32,
    pub(crate) raise_ms: u32,
}

/// Host-resolved tuning for one participating pawn.
///
/// Movement is optional for pawn classes without a movement descriptor. The
/// wieldable array is capacity-sized so a slot's identity survives empty
/// positions. `movement.view_feel` is always cleared because view feel is local
/// presentation rather than predicted simulation tuning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct TuningPayload {
    epoch: u32,
    pub(crate) movement: Option<PlayerMovementDescriptor>,
    pub(crate) wieldables: [Option<WieldableTuningPayload>; WIELDABLE_SLOT_CAPACITY],
}

impl TuningPayload {
    pub(crate) fn new(
        mut movement: Option<PlayerMovementDescriptor>,
        wieldables: [Option<WieldableTuningPayload>; WIELDABLE_SLOT_CAPACITY],
    ) -> Self {
        if let Some(descriptor) = movement.as_mut() {
            descriptor.view_feel = None;
        }
        Self {
            epoch: TUNING_PAYLOAD_EPOCH,
            movement,
            wieldables,
        }
    }

    pub(crate) fn placement_for_slot(&self, slot: usize) -> Option<&WeaponPlacementDescriptor> {
        self.wieldables
            .get(slot)?
            .as_ref()
            .map(|wieldable| &wieldable.placement)
    }

    pub(crate) fn placement_for_archetype(
        &self,
        archetype: &str,
    ) -> Option<&WeaponPlacementDescriptor> {
        self.wieldables
            .iter()
            .flatten()
            .find(|wieldable| wieldable.canonical_name == archetype)
            .map(|wieldable| &wieldable.placement)
    }

    pub(crate) fn muzzle_for_slot(&self, slot: usize) -> Option<&[f32; 3]> {
        self.wieldables.get(slot)?.as_ref()?.muzzle_offset.as_ref()
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

#[derive(Deserialize)]
struct PayloadEpoch {
    epoch: u32,
}

fn payload_json_error(source: serde_json::Error) -> TuningPayloadError {
    if source.is_eof() {
        TuningPayloadError::Truncated
    } else {
        TuningPayloadError::Malformed { source }
    }
}

/// Serialize a payload in its canonical JSON form.
pub(crate) fn encode_tuning_payload(payload: &TuningPayload) -> Vec<u8> {
    let canonical = TuningPayload::new(payload.movement.clone(), payload.wieldables.clone());
    serde_json::to_vec(&canonical)
        .expect("tuning payload only contains validated descriptor values")
}

/// Decode and validate an opaque tuning payload received over Control.
pub(crate) fn decode_tuning_payload(data: &[u8]) -> Result<TuningPayload, TuningPayloadError> {
    // Read the epoch before the full shape. A valid legacy payload has no
    // `wieldables` field, but it should explain its stale epoch rather than
    // degrade into an unhelpful missing-field diagnostic.
    let received = serde_json::from_slice::<PayloadEpoch>(data)
        .map_err(payload_json_error)?
        .epoch;
    if received != TUNING_PAYLOAD_EPOCH {
        return Err(TuningPayloadError::EpochMismatch {
            expected: TUNING_PAYLOAD_EPOCH,
            received,
        });
    }
    let mut payload: TuningPayload = serde_json::from_slice(data).map_err(payload_json_error)?;
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
                        owner: None,
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

    fn weapon_slots() -> [Option<WieldableTuningPayload>; WIELDABLE_SLOT_CAPACITY] {
        let mut slots = std::array::from_fn(|_| None);
        slots[0] = Some(WieldableTuningPayload {
            canonical_name: "reference_pistol".to_string(),
            placement: WeaponPlacementDescriptor::default(),
            muzzle_offset: Some([0.1, -0.2, -0.7]),
            range: 128.0,
            cooldown_ms: 125.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            fire_mode: FireMode::Auto,
            resolution: ResolutionMode::Hitscan,
            lower_ms: 40,
            raise_ms: 60,
        });
        slots[2] = Some(WieldableTuningPayload {
            canonical_name: "ion_rifle".to_string(),
            placement: WeaponPlacementDescriptor::default(),
            muzzle_offset: None,
            range: 256.0,
            cooldown_ms: 240.0,
            pellet_count: 8,
            spread_degrees: 4.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            lower_ms: 75,
            raise_ms: 90,
        });
        slots
    }

    fn full_payload() -> TuningPayload {
        TuningPayload::new(Some(movement_descriptor()), weapon_slots())
    }

    #[test]
    fn payload_round_trips_nested_and_ir_tuning_without_view_feel() {
        let descriptor = movement_descriptor();
        assert!(descriptor.view_feel.is_some());
        let payload = TuningPayload {
            epoch: TUNING_PAYLOAD_EPOCH,
            movement: Some(descriptor),
            wieldables: weapon_slots(),
        };

        let encoded = encode_tuning_payload(&payload);
        let json: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert!(json["movement"]["view_feel"].is_null());
        let wieldables = json["wieldables"].as_array().unwrap();
        assert_eq!(wieldables.len(), WIELDABLE_SLOT_CAPACITY);
        assert_eq!(wieldables[0]["canonical_name"], "reference_pistol");
        assert_eq!(
            wieldables[0]["placement"]["positionFromCenter"]["right"],
            0.0
        );
        assert_eq!(wieldables[0]["muzzle_offset"][2], -0.7);
        assert_eq!(wieldables[2]["pellet_count"], 8);
        assert_eq!(wieldables[2]["spread_degrees"], 4.0);
        assert!(wieldables[1].is_null());
        assert_eq!(wieldables[2]["lower_ms"], 75);

        assert_eq!(
            payload.placement_for_slot(0),
            Some(&WeaponPlacementDescriptor::default())
        );
        assert_eq!(
            payload.placement_for_archetype("ion_rifle"),
            Some(&WeaponPlacementDescriptor::default())
        );
        assert_eq!(payload.muzzle_for_slot(0), Some(&[0.1, -0.2, -0.7]));

        let decoded = decode_tuning_payload(&encoded).unwrap();
        assert_eq!(
            decoded,
            TuningPayload::new(payload.movement, payload.wieldables)
        );
        let dash = decoded.movement.unwrap().dash.unwrap();
        assert_eq!(dash.boost_speed, NumberOrIr::Literal(18.0));
        assert!(matches!(dash.momentum_retention, NumberOrIr::Ir(_)));
    }

    #[test]
    fn payload_round_trips_absent_halves() {
        let payload = TuningPayload::new(None, std::array::from_fn(|_| None));
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

        let stale_shape = br#"{"epoch":1,"movement":null,"default_weapon":null}"#;
        assert!(matches!(
            decode_tuning_payload(stale_shape),
            Err(TuningPayloadError::EpochMismatch {
                expected: TUNING_PAYLOAD_EPOCH,
                received: 1,
            })
        ));
    }

    #[test]
    fn payload_rejects_previous_epoch() {
        let mut json: serde_json::Value =
            serde_json::from_slice(&encode_tuning_payload(&full_payload())).unwrap();
        json["epoch"] = serde_json::json!(4);
        let previous_epoch = serde_json::to_vec(&json).unwrap();

        assert!(matches!(
            decode_tuning_payload(&previous_epoch),
            Err(TuningPayloadError::EpochMismatch {
                expected: 6,
                received: 4,
            })
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
            include_str!("tests/fixtures/tuning_payload.expected.json").trim_end_matches('\n'),
            "tuning payload JSON changed; bump TUNING_PAYLOAD_EPOCH for a semantic payload change, or re-bless with {BLESS_ENV}=1 for a non-semantic rendering change"
        );
    }
}
