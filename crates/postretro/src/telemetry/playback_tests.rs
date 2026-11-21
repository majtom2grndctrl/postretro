//! `run_start` names playback's producer: enabled with a route in a windowed
//! run of a dev-tools build, disabled naming `--camera-route` without one,
//! and unsupported with a reason where playback cannot run, a build
//! without the feature first.
//! See: context/lib/development_guide.md §6.5

use std::sync::Arc;

use postretro_telemetry::{Envelope, Hub, HubConfig, ProducerState, RunStart, Source};

use super::*;

fn playback(producers: &[ProducerEntry]) -> ProducerEntry {
    producers
        .iter()
        .find(|entry| entry.producer == producers::PLAYBACK)
        .cloned()
        .expect("playback listed")
}

#[test]
fn playback_producer_reads_enabled_with_a_route_and_disabled_without_one() {
    let enabled = playback_producer(RunMode::Windowed, true, true);
    assert_eq!(enabled.state, ProducerState::Enabled);
    assert_eq!(enabled.variable.as_deref(), Some(CAMERA_ROUTE_FLAG));
    let disabled = playback_producer(RunMode::Windowed, false, true);
    assert_eq!(disabled.state, ProducerState::Disabled);
    assert_eq!(disabled.variable.as_deref(), Some(CAMERA_ROUTE_FLAG));
}

#[test]
fn playback_producer_reads_unsupported_with_a_reason_where_playback_cannot_run() {
    for (mode, route, feature, reason) in [
        (RunMode::Windowed, true, false, "dev-tools"),
        (RunMode::Windowed, false, false, "dev-tools"),
        (RunMode::Headless, true, true, "headless"),
        (RunMode::Capture, true, true, "capture"),
    ] {
        let entry = playback_producer(mode, route, feature);
        assert_eq!(
            entry.state,
            ProducerState::Unsupported,
            "{mode:?} {feature}"
        );
        assert!(
            entry
                .reason
                .as_deref()
                .is_some_and(|why| why.contains(reason)),
            "{entry:?}"
        );
        assert_eq!(entry.variable.as_deref(), Some(CAMERA_ROUTE_FLAG));
    }
}

#[test]
fn run_start_names_playback_state_for_this_build() {
    for route in [true, false] {
        let hub = Arc::new(Hub::start(HubConfig {
            memory: true,
            producers: engine_producers(RunMode::Windowed, None, None, route),
            ..HubConfig::new(Source::Engine, "t")
        }));
        let first = Envelope::parse_line(&hub.lines()[0]).unwrap();
        let start: RunStart = serde_json::from_value(first.payload).unwrap();
        let expected = match (cfg!(feature = "dev-tools"), route) {
            (false, _) => ProducerState::Unsupported,
            (true, true) => ProducerState::Enabled,
            (true, false) => ProducerState::Disabled,
        };
        assert_eq!(playback(&start.producers).state, expected, "route {route}");
    }
}
