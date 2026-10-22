//! `setGhostAudio` argument validation: the one judge every source shares.
//! See: context/lib/audio.md §1 (Ghost bus) · context/lib/scripting.md §10.4

use postretro_foundation::GhostAudioTargets;
use postretro_scripting_core::runtime::ModGhostAudio;
use serde_json::Value;

/// Validate one `setGhostAudio` call's `args` into the targets it sets. The
/// `Err` reason is the author-facing warning; the dispatcher drops the whole
/// reaction with it. Crossed cutoffs depend on the bus state at the applying
/// tick, so the host judges those when it applies the step.
pub fn parse_set_ghost_audio(args: &Value) -> Result<GhostAudioTargets, String> {
    let mut targets = GhostAudioTargets {
        volume: None,
        high_pass_hz: None,
        low_pass_hz: None,
        drive: None,
        period_ms: 0.0,
    };
    for (key, value) in authored_fields(args)? {
        match key {
            "volume" | "drive" => {
                let field = number_in(value, 0.0..=1.0).ok_or_else(|| {
                    format!("setGhostAudio `{key}` must be a finite number in 0..=1, got {value}")
                })?;
                if key == "volume" {
                    targets.volume = Some(field);
                } else {
                    targets.drive = Some(field);
                }
            }
            "highPassHz" | "lowPassHz" => {
                let range = ModGhostAudio::MIN_CUTOFF_HZ..=ModGhostAudio::MAX_CUTOFF_HZ;
                let field = number_in(value, range).ok_or_else(|| {
                    format!(
                        "setGhostAudio `{key}` must be a finite number in {}..={} Hz, got {value}",
                        ModGhostAudio::MIN_CUTOFF_HZ,
                        ModGhostAudio::MAX_CUTOFF_HZ,
                    )
                })?;
                if key == "highPassHz" {
                    targets.high_pass_hz = Some(field);
                } else {
                    targets.low_pass_hz = Some(field);
                }
            }
            "periodMs" => {
                targets.period_ms = value
                    .as_f64()
                    .filter(|ms| *ms >= 0.0 && (*ms as f32).is_finite())
                    .map(|ms| ms as f32)
                    .ok_or_else(|| {
                        format!(
                            "setGhostAudio `periodMs` must be a finite number >= 0, got {value}"
                        )
                    })?;
            }
            other => {
                return Err(format!(
                    "setGhostAudio has no field `{other}`; expected volume, highPassHz, lowPassHz, drive or periodMs"
                ));
            }
        }
    }
    if !targets.names_a_field() {
        return Err(
            "setGhostAudio names no field; expected one or more of volume, highPassHz, lowPassHz or drive"
                .to_string(),
        );
    }
    Ok(targets)
}

/// The authored fields, `null` (an omitted Luau or JS value) read as absent.
/// An empty Luau table may lower to `[]`.
fn authored_fields(args: &Value) -> Result<Vec<(&str, &Value)>, String> {
    match args {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) if items.is_empty() => Ok(Vec::new()),
        Value::Object(fields) => Ok(fields
            .iter()
            .filter(|(_, value)| !value.is_null())
            .map(|(key, value)| (key.as_str(), value))
            .collect()),
        other => Err(format!(
            "setGhostAudio takes a table of fields, got {other}"
        )),
    }
}

/// A finite number inside `range`.
fn number_in(value: &Value, range: std::ops::RangeInclusive<f32>) -> Option<f32> {
    // Judged in f64 before the cast, like `ghost_link_commands::unit`, so a
    // value just past a bound is refused rather than rounded onto it.
    let (low, high) = (f64::from(*range.start()), f64::from(*range.end()));
    value
        .as_f64()
        .filter(|number| number.is_finite() && (low..=high).contains(number))
        .map(|number| number as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn named_fields_parse_and_omitted_fields_stay_unset() {
        let targets =
            parse_set_ghost_audio(&json!({ "volume": 0.9, "lowPassHz": 5000, "periodMs": 1500 }))
                .expect("valid call");
        assert_eq!(
            targets,
            GhostAudioTargets {
                volume: Some(0.9),
                high_pass_hz: None,
                low_pass_hz: Some(5000.0),
                drive: None,
                period_ms: 1500.0,
            }
        );
    }

    // Out-of-range arguments warn and drop. A non-finite number never reaches
    // the parser (conversion rejects it at load); an out-of-f32 value overflows
    // to infinity.
    #[test]
    fn a_value_just_above_a_bound_is_refused_before_the_f32_cast() {
        assert!(parse_set_ghost_audio(&json!({ "volume": 1.0000000001 })).is_err());
        assert!(parse_set_ghost_audio(&json!({ "volume": 1.0 })).is_ok());
    }

    #[test]
    fn out_of_range_and_non_finite_arguments_are_refused_with_the_field_named() {
        for (args, field) in [
            (json!({ "volume": 1.5 }), "`volume`"),
            (json!({ "drive": -0.1 }), "`drive`"),
            (json!({ "highPassHz": 10 }), "`highPassHz`"),
            (json!({ "lowPassHz": 25000 }), "`lowPassHz`"),
            (json!({ "lowPassHz": 1e300 }), "`lowPassHz`"),
            (json!({ "volume": "NaN" }), "`volume`"),
            (json!({ "volume": 0.5, "periodMs": -1 }), "`periodMs`"),
            (json!({ "volume": 0.5, "periodMs": 1e300 }), "`periodMs`"),
        ] {
            let reason = parse_set_ghost_audio(&args).expect_err("refused");
            assert!(reason.contains(field), "{args}: {reason}");
        }
    }

    // The registered reaction body queues one typed command for a
    // valid call, and a malformed one (here, naming no field) fails dispatch,
    // which the dispatcher warns about, and queues nothing.
    #[test]
    fn the_reaction_body_queues_valid_calls_and_drops_malformed_ones() {
        use postretro_entities::reactions::system_commands::{
            SystemCommandQueue, SystemReactionCommand,
        };
        use postretro_scripting_core::reaction_registry::SystemReactionRegistry;

        let mut registry = SystemReactionRegistry::new();
        crate::scripting_systems::system_reactions::register_system_reaction_primitives(
            &mut registry,
        );
        let queue = SystemCommandQueue::new();
        for args in [
            json!({}),
            json!({ "volume": f64::INFINITY }),
            json!({ "drive": 2 }),
        ] {
            assert!(
                registry.dispatch("setGhostAudio", &args, &queue).is_err(),
                "{args}"
            );
        }
        assert!(queue.is_empty(), "a dropped call queues nothing");
        assert!(
            registry
                .dispatch("setGhostAudio", &json!({ "highPassHz": 300 }), &queue)
                .unwrap()
        );
        assert_eq!(
            queue.take(),
            [SystemReactionCommand::SetGhostAudio(GhostAudioTargets {
                volume: None,
                high_pass_hz: Some(300.0),
                low_pass_hz: None,
                drive: None,
                period_ms: 0.0,
            })]
        );
    }

    // A call naming no field is refused, so no ramp in flight changes.
    #[test]
    fn a_call_naming_no_field_is_refused() {
        for args in [
            json!({}),
            json!({ "periodMs": 500 }),
            json!([]),
            Value::Null,
        ] {
            let reason = parse_set_ghost_audio(&args).expect_err("refused");
            assert!(reason.contains("names no field"), "{args}: {reason}");
        }
        let reason = parse_set_ghost_audio(&json!({ "gain": 1 })).expect_err("unknown field");
        assert!(reason.contains("no field `gain`"), "{reason}");
    }
}
