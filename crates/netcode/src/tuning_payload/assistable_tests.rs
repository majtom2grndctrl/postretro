// A connected client refuses an `assistable` literal fault or unknown key in a
// tuning payload, and carries payload IR without binding or evaluating it.
use super::tests::full_payload;
use super::*;
use postretro_foundation::{IrNode, LifeStateProfile, NumberOrIr};

fn with_down(assistable: Assistable) -> TuningPayload {
    let mut payload = full_payload();
    payload.life_state_names = vec!["alive".into(), "down".into()];
    payload.life_state_profiles = vec![
        (0, LifeStateProfile::default()),
        (
            1,
            LifeStateProfile {
                assistable: Some(assistable),
                ..Default::default()
            },
        ),
    ];
    payload
}

fn literals(radius: f32, hold_ms: f32) -> Assistable {
    Assistable {
        radius: NumberOrIr::Literal(radius),
        hold_ms: NumberOrIr::Literal(hold_ms),
    }
}

#[test]
fn payload_refuses_assistable_literals_outside_their_range() {
    for (radius, hold_ms) in [(0.0, 3000.0), (16.5, 3000.0), (1.5, -1.0), (1.5, 60001.0)] {
        let encoded = encode_tuning_payload(&with_down(literals(radius, hold_ms)));
        assert!(
            matches!(
                decode_tuning_payload(&encoded),
                Err(TuningPayloadError::InvalidLifeStates(_))
            ),
            "radius {radius}, holdMs {hold_ms}"
        );
    }
    // JSON cannot carry a non-finite literal, so the validator is driven directly.
    for (radius, hold_ms) in [
        (f32::NAN, 3000.0),
        (f32::INFINITY, 3000.0),
        (1.5, f32::NAN),
        (1.5, f32::NEG_INFINITY),
    ] {
        assert!(matches!(
            validate_life_states(&with_down(literals(radius, hold_ms))),
            Err(TuningPayloadError::InvalidLifeStates(_))
        ));
    }
    let bounds = with_down(literals(16.0, 60000.0));
    assert_eq!(
        decode_tuning_payload(&encode_tuning_payload(&bounds)).unwrap(),
        bounds
    );
}

#[test]
fn payload_carries_assistable_ir_unevaluated() {
    // An input no client scope could bind: the client never binds payload IR.
    let expression = NumberOrIr::Ir(IrNode::Div {
        a: Box::new(IrNode::Input {
            name: "medic.reviveMs".into(),
            owner: Some("@assist.helper".into()),
        }),
        b: Box::new(IrNode::Const {
            value: postretro_foundation::IrValue::Number(0.0),
        }),
    });
    let payload = with_down(Assistable {
        radius: expression.clone(),
        hold_ms: expression,
    });
    let encoded = encode_tuning_payload(&payload);
    let json: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        json["life_state_profiles"][1][1]["assistable"]["holdMs"]["op"],
        "div"
    );
    assert_eq!(decode_tuning_payload(&encoded).unwrap(), payload);
}

#[test]
fn payload_with_an_unknown_assistable_key_fails_to_deserialize() {
    let mut json: serde_json::Value =
        serde_json::from_slice(&encode_tuning_payload(&with_down(Assistable::default()))).unwrap();
    json["life_state_profiles"][1][1]["assistable"]["range"] = serde_json::json!(2.0);
    assert!(matches!(
        decode_tuning_payload(&serde_json::to_vec(&json).unwrap()),
        Err(TuningPayloadError::Malformed { .. })
    ));
}
