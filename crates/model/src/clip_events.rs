// Clip events: named moments a glTF animation carries in `extras.events`.
// See: context/lib/resource_management.md §7 (Clip events)

use serde_json::Value;

// One name rule for the loader and descriptor `action.at` alike.
pub use postretro_foundation::is_clip_event_name;

/// A named moment in a clip.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipEvent {
    /// Unique within its clip; matches [`is_clip_event_name`].
    pub name: String,
    /// Seconds from the clip's start, within `0..=duration`.
    pub time: f32,
}

/// Slack on the `time ≤ duration` bound: exporters write key times through
/// `f32`, and an event on a clip's last key must not read as past its end.
const DURATION_SLACK: f32 = 1.0e-4;

/// Read an animation's `extras.events`: a list of `{ name, time }`, `time` in
/// seconds from the clip's start. An `end` field, reserved for span events, is
/// accepted and ignored.
///
/// Events are author metadata, not load-critical data. Any malformed list warns
/// once and the clip loads with its keys and no events: not a list, an entry
/// without a valid name or a finite time, a time outside `0..=duration`, or a
/// duplicate name.
pub(crate) fn read_clip_events(
    extras: &gltf::json::Extras,
    duration: f32,
    clip_name: &str,
    path_str: &str,
) -> Vec<ClipEvent> {
    let Some(raw) = extras.as_ref() else {
        return Vec::new();
    };
    let Ok(Value::Object(extras)) = serde_json::from_str::<Value>(raw.get()) else {
        return Vec::new();
    };
    let Some(events) = extras.get("events") else {
        return Vec::new();
    };
    match parse_events(events, duration) {
        Ok(events) => events,
        Err(reason) => {
            log::warn!(
                "[Model] malformed extras.events on clip '{clip_name}' in {path_str}: {reason}; \
                 loading the clip without events"
            );
            Vec::new()
        }
    }
}

fn parse_events(events: &Value, duration: f32) -> Result<Vec<ClipEvent>, String> {
    let Value::Array(entries) = events else {
        return Err("expected a list".to_string());
    };
    let mut parsed: Vec<ClipEvent> = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| is_clip_event_name(name))
            .ok_or_else(|| format!("event {index} needs a name matching [A-Za-z_][A-Za-z0-9_]*"))?;
        let time = entry
            .get("time")
            .and_then(Value::as_f64)
            .map(|time| time as f32)
            .filter(|time| time.is_finite())
            .ok_or_else(|| format!("event '{name}' needs a numeric time"))?;
        if time < 0.0 || time > duration + DURATION_SLACK {
            return Err(format!(
                "event '{name}' at {time} s is outside the clip (0 to {duration} s)"
            ));
        }
        if parsed.iter().any(|event| event.name == name) {
            return Err(format!("event '{name}' is named twice"));
        }
        parsed.push(ClipEvent {
            name: name.to_string(),
            time: time.min(duration),
        });
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extras(json: &str) -> gltf::json::Extras {
        Some(serde_json::from_str(json).expect("test raw JSON parses"))
    }

    #[test]
    fn read_clip_events_loads_name_and_time_and_ignores_span_end() {
        let events = read_clip_events(
            &extras(
                r#"{"events":[{"name":"hit","time":0.3},{"name":"lift","time":0.1,"end":0.4}]}"#,
            ),
            1.0,
            "punch",
            "lib.gltf",
        );
        assert_eq!(
            events,
            vec![
                ClipEvent {
                    name: "hit".into(),
                    time: 0.3
                },
                ClipEvent {
                    name: "lift".into(),
                    time: 0.1
                },
            ]
        );
    }

    #[test]
    fn read_clip_events_accepts_an_event_on_the_last_key() {
        let events = read_clip_events(
            &extras(r#"{"events":[{"name":"done","time":0.50001}]}"#),
            0.5,
            "punch",
            "lib.gltf",
        );
        assert_eq!(
            events,
            vec![ClipEvent {
                name: "done".into(),
                time: 0.5
            }]
        );
    }
}
