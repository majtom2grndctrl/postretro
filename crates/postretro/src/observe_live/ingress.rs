//! Main-thread request service for the live introspection transport.
//!
//! The transport delivers only bytes. This module parses those bytes and reads
//! the registry at the Input-stage frame boundary.

use std::sync::mpsc;

use postretro_entities::EntityRegistry;
use postretro_level_loader::LevelWorld;
use serde::{Deserialize, Serialize};

use super::ServiceRequest;
use crate::observability::{
    DumpSpec, OutOfFrame, OutputDocument, build_output_document, build_player_summary,
    to_deterministic_json,
};

/// JSON request vocabulary for the localhost live-introspection channel.
///
/// Internally tagged enums are sound for this JSON-only channel. The bitcode
/// wire-format rule against them does not apply here.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "verb", rename_all = "snake_case")]
pub(crate) enum ObserveRequest {
    Dump {
        #[serde(default)]
        spec: DumpSpec,
    },
}

/// JSON response vocabulary for the localhost live-introspection channel.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum ObserveResponse {
    Ok { dump: OutputDocument },
    Error { message: String },
}

/// Drain every currently queued request at the Input-stage frame boundary.
///
/// The service closure runs only after a request is dequeued, so an idle frame
/// need not borrow engine state. Neither engine state nor typed protocol data
/// crosses the transport's mpsc channel.
pub(crate) fn run_observe_ingress_stage(
    requests: &mpsc::Receiver<ServiceRequest>,
    mut service: impl FnMut(&[u8]) -> Vec<u8>,
) -> usize {
    let mut serviced = 0;

    while let Ok(request) = requests.try_recv() {
        let response = service(&request.payload);
        // A transport timeout can drop this request's receiver while it waits in
        // the queue. The next request must still be serviced in that case.
        let _ = request.reply.send(response);
        serviced += 1;
    }

    serviced
}

/// Parse and service one request using the engine state borrowed at the frame
/// boundary. Called only after [`run_observe_ingress_stage`] dequeues a request,
/// so an idle frame does not borrow the registry.
pub(crate) fn service_observe_request(
    payload: &[u8],
    map: &str,
    registry: Option<&EntityRegistry>,
    world: Option<&LevelWorld>,
    facing_yaw: f32,
) -> Vec<u8> {
    service_payload(
        payload,
        IngressContext {
            map,
            registry,
            world,
            facing_yaw,
        },
    )
}

#[derive(Clone, Copy)]
struct IngressContext<'a> {
    map: &'a str,
    registry: Option<&'a EntityRegistry>,
    world: Option<&'a LevelWorld>,
    facing_yaw: f32,
}

fn service_payload(payload: &[u8], context: IngressContext<'_>) -> Vec<u8> {
    let response = match serde_json::from_slice(payload) {
        Ok(ObserveRequest::Dump { spec }) => match build_live_document(context, &spec) {
            Ok(dump) => ObserveResponse::Ok { dump },
            Err(error) => ObserveResponse::Error {
                message: error.to_string(),
            },
        },
        Err(error) => ObserveResponse::Error {
            message: error.to_string(),
        },
    };

    // Every ObserveResponse field has a JSON representation. This shared
    // serializer is the byte-determinism boundary used by headless dumps too.
    to_deterministic_json(&response)
        .expect("ObserveResponse contains only JSON-serializable values")
        .into_bytes()
}

fn build_live_document(
    context: IngressContext<'_>,
    spec: &DumpSpec,
) -> Result<OutputDocument, crate::observability::DumpError> {
    let (Some(registry), Some(world)) = (context.registry, context.world) else {
        return Ok(no_world_document());
    };

    let mut output = build_output_document(
        context.map,
        0,
        registry,
        spec,
        world,
        Vec::new(),
        build_player_summary(registry, context.facing_yaw),
    )?;
    output
        .out_of_frame
        .present_not_dumped
        .push("events".to_string());
    Ok(output)
}

fn no_world_document() -> OutputDocument {
    let mut out_of_frame = OutOfFrame::headless();
    out_of_frame.present_not_dumped.push("events".to_string());
    OutputDocument {
        map: String::new(),
        ticks_run: 0,
        entities: Vec::new(),
        truncated: 0,
        events: Vec::new(),
        player: None,
        cell_visibility: None,
        out_of_frame,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::components::health::HealthComponent;
    use postretro_entities::{ComponentValue, Transform};
    use postretro_level_loader::{CellData, CellLocatorChild};
    use std::collections::HashMap;

    const MAP: &str = "path:fixture.prl";
    const FACING_YAW: f32 = 1.25;

    fn test_world() -> LevelWorld {
        let cells = vec![CellData {
            bounds_min: glam::Vec3::ZERO,
            bounds_max: glam::Vec3::ONE,
            face_start: 0,
            face_count: 0,
            portal_ref_start: 0,
            portal_ref_count: 0,
            is_solid: false,
            is_exterior: false,
            is_drawable: false,
        }];
        LevelWorld::new_visibility_only(
            cells,
            Vec::new(),
            CellLocatorChild::Cell(0),
            Vec::new(),
            Vec::new(),
            false,
        )
        .expect("minimal live-observe world is valid")
    }

    fn fixture_registry() -> EntityRegistry {
        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(Transform::default());
        registry
            .set_component_value(
                entity,
                ComponentValue::Health(HealthComponent {
                    max: 100.0,
                    current: 75.0,
                    hitbox: None,
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: HashMap::new(),
                    contributor_ledger: Default::default(),
                }),
            )
            .expect("fixture entity accepts health");
        registry
    }

    fn dump_payload(spec: DumpSpec) -> Vec<u8> {
        serde_json::to_vec(&ObserveRequest::Dump { spec }).expect("serialize dump request")
    }

    fn queue_request(
        requests: &mpsc::Sender<ServiceRequest>,
        payload: Vec<u8>,
    ) -> mpsc::Receiver<Vec<u8>> {
        let (reply, response) = mpsc::channel();
        requests
            .send(ServiceRequest { payload, reply })
            .expect("queue service request");
        response
    }

    fn decode_response(bytes: &[u8]) -> ObserveResponse {
        serde_json::from_slice(bytes).expect("deserialize observe response")
    }

    #[test]
    fn dump_protocol_defaults_its_spec() {
        let request: ObserveRequest =
            serde_json::from_slice(br#"{"verb":"dump"}"#).expect("parse request");
        assert!(matches!(request, ObserveRequest::Dump { spec } if spec == DumpSpec::default()));
    }

    #[test]
    fn service_matches_shared_builder_except_live_events_declaration() {
        let registry = fixture_registry();
        let world = test_world();
        let spec = DumpSpec::default();
        let (requests, receiver) = mpsc::channel();
        let response = queue_request(&requests, dump_payload(spec.clone()));

        assert_eq!(
            run_observe_ingress_stage(&receiver, |payload| {
                service_observe_request(payload, MAP, Some(&registry), Some(&world), FACING_YAW)
            }),
            1
        );

        let ObserveResponse::Ok { dump } =
            decode_response(&response.recv().expect("receive live dump response"))
        else {
            panic!("valid dump request must return an OK response");
        };
        let mut expected = build_output_document(
            MAP,
            0,
            &registry,
            &spec,
            &world,
            Vec::new(),
            build_player_summary(&registry, FACING_YAW),
        )
        .expect("fixture dump builds");
        expected
            .out_of_frame
            .present_not_dumped
            .push("events".to_string());
        assert_eq!(dump, expected);
    }

    #[test]
    fn service_repeats_identical_bytes_for_frozen_state() {
        let registry = fixture_registry();
        let world = test_world();
        let (requests, receiver) = mpsc::channel();
        let first = queue_request(&requests, dump_payload(DumpSpec::default()));
        let second = queue_request(&requests, dump_payload(DumpSpec::default()));

        assert_eq!(
            run_observe_ingress_stage(&receiver, |payload| {
                service_observe_request(payload, MAP, Some(&registry), Some(&world), FACING_YAW)
            }),
            2
        );
        assert_eq!(
            first.recv().expect("receive first response"),
            second.recv().expect("receive second response")
        );
    }

    #[test]
    fn service_returns_a_valid_document_without_a_world() {
        let (requests, receiver) = mpsc::channel();
        let response = queue_request(&requests, dump_payload(DumpSpec::default()));

        assert_eq!(
            run_observe_ingress_stage(&receiver, |payload| {
                service_observe_request(payload, MAP, None, None, FACING_YAW)
            }),
            1
        );
        let ObserveResponse::Ok { dump } =
            decode_response(&response.recv().expect("receive no-world response"))
        else {
            panic!("valid no-world dump must return an OK response");
        };
        assert_eq!(dump.map, "");
        assert_eq!(dump.ticks_run, 0);
        assert!(dump.entities.is_empty());
        assert_eq!(dump.truncated, 0);
        assert!(dump.events.is_empty());
        assert!(dump.player.is_none());
        assert!(dump.cell_visibility.is_none());
        assert_eq!(
            dump.out_of_frame.present_not_dumped.last(),
            Some(&"events".to_string())
        );
    }

    #[test]
    fn stale_reply_does_not_prevent_the_next_request_from_being_serviced() {
        let registry = fixture_registry();
        let world = test_world();
        let (requests, receiver) = mpsc::channel();
        let stale = queue_request(&requests, dump_payload(DumpSpec::default()));
        drop(stale);
        let live = queue_request(&requests, dump_payload(DumpSpec::default()));

        assert_eq!(
            run_observe_ingress_stage(&receiver, |payload| {
                service_observe_request(payload, MAP, Some(&registry), Some(&world), FACING_YAW)
            }),
            2
        );
        assert!(matches!(
            decode_response(&live.recv().expect("receive response after stale request")),
            ObserveResponse::Ok { .. }
        ));
    }

    #[test]
    fn empty_queue_is_a_non_blocking_no_op() {
        let (_requests, receiver) = mpsc::channel();
        assert_eq!(
            run_observe_ingress_stage(&receiver, |_| {
                panic!("an empty queue must not invoke the service")
            }),
            0
        );
    }

    #[test]
    fn service_reports_zero_ticks_without_simulating() {
        let registry = fixture_registry();
        let world = test_world();
        let (requests, receiver) = mpsc::channel();
        let response = queue_request(&requests, dump_payload(DumpSpec::default()));

        let _ = run_observe_ingress_stage(&receiver, |payload| {
            service_observe_request(payload, MAP, Some(&registry), Some(&world), FACING_YAW)
        });
        let ObserveResponse::Ok { dump } =
            decode_response(&response.recv().expect("receive zero-tick response"))
        else {
            panic!("valid dump request must return an OK response");
        };
        assert_eq!(dump.ticks_run, 0);
    }

    #[test]
    fn malformed_request_returns_an_error_response() {
        let (requests, receiver) = mpsc::channel();
        let response = queue_request(&requests, br#"{"verb":"unknown"}"#.to_vec());

        let _ = run_observe_ingress_stage(&receiver, |payload| {
            service_observe_request(payload, MAP, None, None, FACING_YAW)
        });
        assert!(matches!(
            decode_response(&response.recv().expect("receive malformed-request response")),
            ObserveResponse::Error { .. }
        ));
    }

    #[test]
    fn dump_failure_returns_an_error_response() {
        let registry = fixture_registry();
        let world = test_world();
        let (requests, receiver) = mpsc::channel();
        let response = queue_request(
            &requests,
            dump_payload(DumpSpec {
                component: Some("not_a_component_kind".to_string()),
                ..DumpSpec::default()
            }),
        );

        let _ = run_observe_ingress_stage(&receiver, |payload| {
            service_observe_request(payload, MAP, Some(&registry), Some(&world), FACING_YAW)
        });
        assert!(matches!(
            decode_response(&response.recv().expect("receive dump-failure response")),
            ObserveResponse::Error { .. }
        ));
    }
}
