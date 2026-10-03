//! Engine-side codec for host-resolved pawn tuning and weapon placement.
//!
//! The net crate carries these bytes opaquely. Keeping the JSON codec here
//! avoids a wire mirror that would make the transport registry-aware.

use postretro_combat_model::{TUNING_PAYLOAD_EPOCH, TuningPayload};
use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum TuningPayloadError {
    #[error("tuning payload is truncated")]
    Truncated,
    #[error("tuning payload is malformed: {source}")]
    Malformed {
        #[source]
        source: serde_json::Error,
    },
    #[error("tuning payload has invalid weapon data: {0}")]
    InvalidWeapon(postretro_foundation::DescriptorError),
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
        descriptor.sounds = None;
    }
    for weapon in payload.wieldables.iter_mut().flatten() {
        weapon.primary.sounds = None;
        if let Some(secondary) = &mut weapon.secondary {
            secondary.sounds = None;
        }
        weapon
            .validate()
            .map_err(TuningPayloadError::InvalidWeapon)?;
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use postretro_combat_model::WieldableTuningPayload;
    use postretro_entities::components::inventory::WIELDABLE_SLOT_CAPACITY;
    use postretro_foundation::{
        AirParams, BoolOrIr, CapsuleParams, DashParams, FallParams, ForgivenessParams,
        GroundParams, NumberOrIr, PlayerMovementDescriptor, ResolutionMode, SlideParams,
        SlideViewParams, SpeedParams, ViewFeelParams, WeaponPlacementDescriptor,
    };

    use super::*;

    const BLESS_ENV: &str = "POSTRETRO_BLESS_COMPATIBILITY_FIXTURES";
    const FIXTURE_PATH: &str = "src/tests/fixtures/tuning_payload.expected.json";

    fn movement_descriptor() -> PlayerMovementDescriptor {
        PlayerMovementDescriptor {
            sounds: None,
            knockback: Default::default(),
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
            slide: None,
            view_feel: Some(ViewFeelParams {
                bob: None,
                tilt: None,
                sway: None,
                impulse: None,
                slide: Some(SlideViewParams {
                    eye_drop: 0.15,
                    fov_increase: 5.0,
                    enter_rate: 18.0,
                    exit_rate: 12.0,
                }),
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
            primary: postretro_foundation::WeaponActivationDescriptor::single(
                postretro_foundation::ActivationTrigger::Hold,
                125.0,
            ),
            secondary: None,
            damage: 10.0,
            knockback: None,
            projectile: None,
            splash: None,
            resource: None,
            pellet_count: 1,
            spread_degrees: 0.0,
            bloom_per_shot_degrees: 1.5,
            bloom_max_degrees: 6.0,
            bloom_decay_degrees_per_second: 2.5,
            bloom_decay_delay_ms: 175.0,
            movement_spread_degrees: 3.0,
            spread_vertical_bias: 0.2,
            resolution: ResolutionMode::Hitscan,
            lower_ms: 40,
            raise_ms: 60,
            block_during_reload: None,
        });
        slots[2] = Some(WieldableTuningPayload {
            canonical_name: "ion_rifle".to_string(),
            placement: WeaponPlacementDescriptor::default(),
            muzzle_offset: None,
            range: 256.0,
            primary: postretro_foundation::WeaponActivationDescriptor::single(
                postretro_foundation::ActivationTrigger::Press,
                240.0,
            ),
            secondary: None,
            damage: 10.0,
            knockback: None,
            projectile: None,
            splash: None,
            resource: None,
            pellet_count: 8,
            spread_degrees: 4.0,
            bloom_per_shot_degrees: 2.0,
            bloom_max_degrees: 12.0,
            bloom_decay_degrees_per_second: 1.0,
            bloom_decay_delay_ms: 250.0,
            movement_spread_degrees: 4.0,
            spread_vertical_bias: 0.5,
            resolution: ResolutionMode::Hitscan,
            lower_ms: 75,
            raise_ms: 90,
            block_during_reload: None,
        });
        slots
    }

    fn full_payload() -> TuningPayload {
        TuningPayload::new(Some(movement_descriptor()), weapon_slots())
    }

    #[test]
    fn payload_round_trips_nested_ir_slide_and_knockback_tuning_without_view_feel() {
        let mut descriptor = movement_descriptor();
        descriptor.knockback.air_drag = 3.0;
        descriptor.knockback.control = 0.4;
        descriptor.slide = Some(SlideParams {
            min_speed: 8.0,
            slide_drag: 12.0,
            slope_assist: 1.5,
            steer_rate: 180.0,
            entry_boost: 2.0,
            min_duration_ms: 120.0,
        });
        assert!(descriptor.view_feel.as_ref().unwrap().slide.is_some());
        descriptor.sounds = Some(postretro_foundation::MovementSounds {
            land: Some("sfx/land".to_string()),
            jump: Some("sfx/jump".to_string()),
        });
        let payload =
            TuningPayload::new_for_test_preserving_view_feel(Some(descriptor), weapon_slots());

        let encoded = encode_tuning_payload(&payload);
        let json: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert!(json["movement"]["view_feel"].is_null());
        // This tuning payload omits sound keys; observer cues transport frozen
        // effective keys separately (`audio.md` §4).
        assert!(json["movement"]["sounds"].is_null());
        assert_eq!(json["movement"]["slide"]["min_speed"], 8.0);
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
        let movement = decoded.movement.unwrap();
        assert!((movement.knockback.air_drag - 3.0).abs() < 1.0e-6);
        assert!((movement.knockback.control - 0.4).abs() < 1.0e-6);
        let dash = movement.dash.as_ref().unwrap();
        assert_eq!(dash.boost_speed, NumberOrIr::Literal(18.0));
        assert!(matches!(dash.momentum_retention, NumberOrIr::Ir(_)));
        assert_eq!(movement.slide.unwrap().entry_boost, 2.0);
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
        json["epoch"] = serde_json::json!(9);
        let previous_epoch = serde_json::to_vec(&json).unwrap();

        assert!(matches!(
            decode_tuning_payload(&previous_epoch),
            Err(TuningPayloadError::EpochMismatch {
                expected: 10,
                received: 9,
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
    #[test]
    fn payload_carries_host_scale_bases_and_charge_expression_and_rejects_other_scope() {
        use postretro_foundation::{
            ActivationCharge, ActivationStepDescriptor, ActivationTrigger, CellResource,
            CompiledActivation, IrNode, IrValue, ShotScaleDescriptor, WeaponActivationDescriptor,
            WeaponResource,
        };
        let mut payload = full_payload();
        let row = payload.wieldables[0].as_mut().unwrap();
        row.damage = 13.0;
        row.range = 230.0;
        row.block_during_reload = Some(true);
        row.resource = Some(WeaponResource::Cell(CellResource {
            capacity: 100.0,
            cost_per_shot: 7.0,
            regen_per_second: 2.0,
            regen_delay_ms: 500.0,
        }));
        row.resolution = ResolutionMode::Projectile;
        row.projectile = Some(
            serde_json::from_value(serde_json::json!({
                "speed":57.0,"radius":0.25,"lifetimeMs":2500.0,
                "visual":{"body":{"kind":"sprite","sprite":"effects/host-bolt","size":2.0}}
            }))
            .unwrap(),
        );
        let mut secondary = WeaponActivationDescriptor::single(ActivationTrigger::Press, 400.0);
        secondary.charge = Some(ActivationCharge {
            min_ms: 200.0,
            full_ms: 1000.0,
        });
        secondary.steps = vec![ActivationStepDescriptor::Shot {
            scale: Box::new(ShotScaleDescriptor {
                damage: NumberOrIr::Ir(IrNode::Add {
                    a: Box::new(IrNode::Mul {
                        a: Box::new(IrNode::Input {
                            name: "charge".into(),
                            owner: None,
                        }),
                        b: Box::new(IrNode::Const {
                            value: IrValue::Number(5.0),
                        }),
                    }),
                    b: Box::new(IrNode::Const {
                        value: IrValue::Number(1.0),
                    }),
                }),
                ..Default::default()
            }),
        }];
        row.secondary = Some(secondary);
        let encoded = encode_tuning_payload(&payload);
        let decoded = decode_tuning_payload(&encoded).unwrap();
        assert_eq!(decoded, payload);
        let row = decoded.wieldables[0].as_ref().unwrap();
        assert_eq!(row.block_during_reload, Some(true));
        assert_eq!(row.projectile.as_ref().unwrap().speed, 57.0);
        assert_eq!(row.damage, 13.0);
        assert_eq!(
            row.resource,
            payload.wieldables[0].as_ref().unwrap().resource
        );
        let compiled =
            CompiledActivation::compile(row.secondary.as_ref().unwrap(), "fixture.secondary")
                .unwrap();
        assert_eq!(compiled.resolve_scales(0, 1.0).unwrap().damage, 6.0);
        let mut invalid: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        invalid["wieldables"][0]["secondary"]["steps"][0]["scale"]["damage"] =
            serde_json::json!({"op":"input","name":"player.health"});
        assert!(matches!(
            decode_tuning_payload(&serde_json::to_vec(&invalid).unwrap()),
            Err(TuningPayloadError::InvalidWeapon(_))
        ));
    }
}
