// Runspec input vocabulary: the tool-facing JSON a headless run is driven from.
// See: context/plans/in-progress/agentic-observability

use glam::Vec2;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use postretro_entities::ComponentKind;

use super::{DumpError, parse_component_kind};
use crate::movement::MovementInput;

/// Default entry cap when a runspec omits `dump.cap`. Bounds the dumped entity
/// list so a large world cannot produce an unbounded document; the driver still
/// learns how many entries were dropped (see [`super::document::DumpSelection`]).
const DEFAULT_DUMP_CAP: usize = 1000;

/// A complete headless run description: which map, how many fixed ticks, the
/// scripted per-tick commands, and how to filter the resulting state dump.
///
/// `deny_unknown_fields` makes a typo or stale key a hard parse error rather than
/// a silently-ignored field — the runspec is a stable tool-facing surface, so
/// drift must be loud.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunSpec {
    /// Content-relative path to the `.prl` map to load.
    pub map: String,
    /// Number of fixed game-logic ticks to advance before dumping.
    pub ticks: u32,
    /// Ordered, sparse per-tick command entries. Each entry applies from its
    /// `tick` until the next entry's tick; ticks before the first entry (and any
    /// unset field within an entry) use neutral input.
    #[serde(default)]
    pub commands: Vec<CommandEntry>,
    /// How to filter and cap the entity-state dump.
    #[serde(default)]
    pub dump: DumpSpec,
}

/// One scripted command in the sparse per-tick timeline.
///
/// Movement fields carry the same snake_case names as the engine's
/// `MovementInput`; `facing_yaw` is intentionally absent — the driver derives it
/// from `aim.direction` and threads it into the movement input, mirroring how the
/// windowed engine derives facing from the camera.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandEntry {
    /// Tick this command takes effect on (0-based).
    pub tick: u32,
    /// Movement intent. Absent → neutral (no wish direction, no buttons).
    #[serde(default)]
    pub movement: MovementCommand,
    /// Aim origin/direction feeding the post-movement command. Absent → the
    /// driver keeps the prior aim (aim carries no neutral, unlike movement).
    #[serde(default)]
    pub aim: Option<AimCommand>,
    /// Fire button held this entry. Absent → not firing.
    #[serde(default)]
    pub fire: bool,
    /// Reload requested this entry. Absent → no reload.
    #[serde(default)]
    pub reload: bool,
}

impl CommandEntry {
    /// Build the engine `MovementInput` for this command, supplying the
    /// `facing_yaw` the driver derived from the active aim direction. Centralizes
    /// the runspec-vocabulary → engine-input mapping so the driver never
    /// hand-assembles a `MovementInput`.
    pub(crate) fn movement_input(&self, facing_yaw: f32) -> MovementInput {
        let m = &self.movement;
        MovementInput {
            wish_dir: Vec2::new(m.wish_dir[0], m.wish_dir[1]),
            jump_pressed: m.jump_pressed,
            dash_pressed: m.dash_pressed,
            running: m.running,
            crouch_intent: m.crouch_intent,
            facing_yaw,
        }
    }
}

/// Movement intent, field-for-field mirroring the engine `MovementInput` (minus
/// the driver-derived `facing_yaw`). Every field defaults to neutral so a
/// partial `movement` block reads cleanly.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MovementCommand {
    /// `[right, forward]` wish direction; component magnitudes in `[0, 1]`.
    pub wish_dir: [f32; 2],
    pub jump_pressed: bool,
    pub dash_pressed: bool,
    pub running: bool,
    pub crouch_intent: bool,
}

/// Aim ray for the post-movement command: `SimCommand` carries no pitch, so aim
/// is authored here and fed in after movement, matching the windowed engine's
/// camera-derived aim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AimCommand {
    pub origin: [f32; 3],
    pub direction: [f32; 3],
}

/// Dump filter: which component column(s), which tag, which entity ids, and the
/// entry cap. All fields default so a runspec may omit `dump` entirely.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct DumpSpec {
    /// Snake_case component-kind filter (same strings as `ComponentValue`'s
    /// serde `kind` tag). `None` dumps every component of every entity.
    pub component: Option<String>,
    /// Tag filter. `None` matches every entity; `Some` keeps only entities
    /// carrying that tag.
    pub tag: Option<String>,
    /// Raw entity-id allowlist. `None` matches every entity; `Some` keeps only
    /// the listed ids.
    pub entities: Option<Vec<u32>>,
    /// Maximum number of dumped records; overflow is truncated and counted.
    pub cap: usize,
    /// Whether the output document includes the per-tick event lists.
    pub events: bool,
}

impl Default for DumpSpec {
    fn default() -> Self {
        Self {
            component: None,
            tag: None,
            entities: None,
            cap: DEFAULT_DUMP_CAP,
            events: true,
        }
    }
}

impl DumpSpec {
    /// Resolve the component-kind filter string to a [`ComponentKind`]. `Ok(None)`
    /// means "no component filter" (dump all kinds); an unrecognized string is a
    /// [`DumpError::UnknownComponentKind`].
    pub(crate) fn resolve_component(&self) -> Result<Option<ComponentKind>, DumpError> {
        match &self.component {
            None => Ok(None),
            Some(name) => parse_component_kind(name)
                .map(Some)
                .ok_or_else(|| DumpError::UnknownComponentKind(name.clone())),
        }
    }
}

/// Failure parsing a runspec document. Wraps the underlying serde_json error,
/// which already carries a line/column and, for `deny_unknown_fields`, the
/// offending field name — a useful diagnostic the driver can print before
/// exiting non-zero.
#[derive(Debug, Error)]
pub(crate) enum RunSpecError {
    #[error("invalid runspec: {0}")]
    Parse(#[from] serde_json::Error),
    /// `active_command_at`/`effective_aim_at` (`driver.rs`) select the active
    /// entry with a reverse scan that is only correct if `commands` is sorted
    /// strictly ascending by tick. An out-of-order or duplicate tick silently
    /// shadows a command instead of erroring, so it is rejected here at parse
    /// rather than sorted or de-duplicated for the caller.
    #[error(
        "invalid runspec: commands must be strictly ascending by tick; \
         commands[{index}].tick = {tick} is not greater than the previous tick {previous}"
    )]
    UnorderedCommandTick {
        index: usize,
        tick: u32,
        previous: u32,
    },
    /// `wish_dir` feeds `MovementInput::wish_dir` which the movement system
    /// normalizes; a non-finite component (e.g. from an oversized authored
    /// value overflowing to `inf`) normalizes to NaN, propagating to a NaN
    /// pawn position that `serde_json` silently serializes as `null`. Rejected
    /// here instead of clamped so authoring bugs surface loudly.
    #[error("invalid runspec: commands[{index}].movement.wish_dir must be finite, got [{x}, {y}]")]
    NonFiniteWishDir { index: usize, x: f32, y: f32 },
}

/// Parse a runspec from JSON text. Malformed JSON, unknown fields, out-of-order
/// or duplicate command ticks, and non-finite `wish_dir` components all yield an
/// `Err` with a diagnostic message.
pub(crate) fn parse_runspec(json: &str) -> Result<RunSpec, RunSpecError> {
    let spec: RunSpec = serde_json::from_str(json)?;
    validate_commands(&spec.commands)?;
    Ok(spec)
}

/// Validate the sparse command timeline: ticks must be strictly ascending (no
/// out-of-order or duplicate entries), and every `wish_dir` component must be
/// finite. See [`RunSpecError::UnorderedCommandTick`] and
/// [`RunSpecError::NonFiniteWishDir`] for why these are parse-time rejections
/// rather than silent sort/clamp.
fn validate_commands(commands: &[CommandEntry]) -> Result<(), RunSpecError> {
    let mut previous_tick: Option<u32> = None;
    for (index, entry) in commands.iter().enumerate() {
        if let Some(previous) = previous_tick {
            if entry.tick <= previous {
                return Err(RunSpecError::UnorderedCommandTick {
                    index,
                    tick: entry.tick,
                    previous,
                });
            }
        }
        previous_tick = Some(entry.tick);

        let [x, y] = entry.movement.wish_dir;
        if !x.is_finite() || !y.is_finite() {
            return Err(RunSpecError::NonFiniteWishDir { index, x, y });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_runspec() -> RunSpec {
        RunSpec {
            map: "content/dev/maps/campaign-test.prl".to_string(),
            ticks: 300,
            commands: vec![
                CommandEntry {
                    tick: 0,
                    movement: MovementCommand {
                        wish_dir: [0.0, 1.0],
                        jump_pressed: false,
                        dash_pressed: false,
                        running: true,
                        crouch_intent: false,
                    },
                    aim: Some(AimCommand {
                        origin: [0.0, 1.6, 0.0],
                        direction: [0.0, 0.0, -1.0],
                    }),
                    fire: false,
                    reload: false,
                },
                CommandEntry {
                    tick: 30,
                    movement: MovementCommand::default(),
                    aim: None,
                    fire: true,
                    reload: true,
                },
            ],
            dump: DumpSpec {
                component: Some("health".to_string()),
                tag: None,
                entities: None,
                cap: 500,
                events: true,
            },
        }
    }

    #[test]
    fn runspec_round_trips_through_json() {
        let original = sample_runspec();
        let json = serde_json::to_string(&original).unwrap();
        let parsed = parse_runspec(&json).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn parse_runspec_rejects_unknown_top_level_field() {
        let json = r#"{ "map": "m.prl", "ticks": 10, "bogus": 1 }"#;
        let err = parse_runspec(json).unwrap_err();
        assert!(
            err.to_string().contains("bogus"),
            "diagnostic should name the offending field, got: {err}"
        );
    }

    #[test]
    fn parse_runspec_rejects_unknown_movement_field() {
        let json = r#"{
            "map": "m.prl", "ticks": 10,
            "commands": [ { "tick": 0, "movement": { "sprint": true } } ]
        }"#;
        assert!(parse_runspec(json).is_err());
    }

    #[test]
    fn parse_runspec_rejects_malformed_json() {
        assert!(parse_runspec("{ not json").is_err());
    }

    #[test]
    fn minimal_runspec_defaults_commands_and_dump() {
        let spec = parse_runspec(r#"{ "map": "m.prl", "ticks": 5 }"#).unwrap();
        assert!(spec.commands.is_empty());
        assert_eq!(spec.dump, DumpSpec::default());
        assert_eq!(spec.dump.cap, DEFAULT_DUMP_CAP);
        assert!(spec.dump.events, "events default on");
    }

    #[test]
    fn absent_command_fields_read_as_neutral_input() {
        let spec =
            parse_runspec(r#"{ "map": "m.prl", "ticks": 1, "commands": [ { "tick": 0 } ] }"#)
                .unwrap();
        let entry = &spec.commands[0];
        assert_eq!(entry.movement, MovementCommand::default());
        assert!(entry.aim.is_none());
        assert!(!entry.fire);
        assert!(!entry.reload);

        let input = entry.movement_input(0.25);
        assert_eq!(input.wish_dir, Vec2::ZERO);
        assert!(!input.jump_pressed);
        assert_eq!(
            input.facing_yaw, 0.25,
            "driver-supplied yaw threads through"
        );
    }

    #[test]
    fn reload_flag_passes_through_parse() {
        let spec = parse_runspec(
            r#"{ "map": "m.prl", "ticks": 1,
                 "commands": [ { "tick": 0, "reload": true } ] }"#,
        )
        .unwrap();
        assert!(spec.commands[0].reload, "reload must survive the parse");
    }

    #[test]
    fn resolve_component_maps_snake_string_to_kind() {
        let spec = DumpSpec {
            component: Some("kinematic_mover".to_string()),
            ..DumpSpec::default()
        };
        assert_eq!(
            spec.resolve_component(),
            Ok(Some(ComponentKind::KinematicMover))
        );
    }

    #[test]
    fn resolve_component_none_means_all_kinds() {
        assert_eq!(DumpSpec::default().resolve_component(), Ok(None));
    }

    #[test]
    fn resolve_component_rejects_unknown_kind() {
        let spec = DumpSpec {
            component: Some("shields".to_string()),
            ..DumpSpec::default()
        };
        assert_eq!(
            spec.resolve_component(),
            Err(DumpError::UnknownComponentKind("shields".to_string()))
        );
    }

    // Regression: active_command_at/effective_aim_at (driver.rs) select the
    // active entry via a reverse scan that assumes ascending ticks; an
    // out-of-order entry was silently shadowed instead of rejected.
    #[test]
    fn parse_runspec_rejects_out_of_order_command_ticks() {
        let json = r#"{
            "map": "m.prl", "ticks": 20,
            "commands": [ { "tick": 10 }, { "tick": 0 } ]
        }"#;
        let err = parse_runspec(json).unwrap_err();
        assert!(
            err.to_string().contains("commands[1]") && err.to_string().contains("10"),
            "diagnostic should name the offending index and previous tick, got: {err}"
        );
    }

    #[test]
    fn parse_runspec_rejects_duplicate_command_ticks() {
        let json = r#"{
            "map": "m.prl", "ticks": 20,
            "commands": [ { "tick": 5 }, { "tick": 5 } ]
        }"#;
        let err = parse_runspec(json).unwrap_err();
        assert!(
            err.to_string().contains("commands[1]"),
            "diagnostic should name the offending index, got: {err}"
        );
    }

    #[test]
    fn parse_runspec_accepts_strictly_ascending_command_ticks() {
        let json = r#"{
            "map": "m.prl", "ticks": 20,
            "commands": [ { "tick": 0 }, { "tick": 5 }, { "tick": 30 } ]
        }"#;
        let spec = parse_runspec(json).unwrap();
        assert_eq!(spec.commands.len(), 3);
    }

    // Regression: a huge wish_dir magnitude overflows to inf, then normalizes
    // to NaN downstream, producing a NaN pawn position serialized as `null`.
    #[test]
    fn parse_runspec_rejects_non_finite_wish_dir() {
        let json = r#"{
            "map": "m.prl", "ticks": 1,
            "commands": [ { "tick": 0, "movement": { "wish_dir": [1e40, 0.0] } } ]
        }"#;
        let err = parse_runspec(json).unwrap_err();
        assert!(
            err.to_string().contains("wish_dir"),
            "diagnostic should name wish_dir, got: {err}"
        );
    }

    #[test]
    fn parse_runspec_accepts_normal_wish_dir() {
        let json = r#"{
            "map": "m.prl", "ticks": 1,
            "commands": [ { "tick": 0, "movement": { "wish_dir": [0.5, -1.0] } } ]
        }"#;
        let spec = parse_runspec(json).unwrap();
        assert_eq!(spec.commands[0].movement.wish_dir, [0.5, -1.0]);
    }
}
