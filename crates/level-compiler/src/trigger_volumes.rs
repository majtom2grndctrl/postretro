use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use glam::DVec3;
use postretro_level_format::trigger_volumes::{TriggerVolumeRecord, TriggerVolumesSection};
use shambler::GeoMap;
use shambler::brush::{BrushId, brush_hulls};
use shambler::face::{face_planes, face_vertices};

use crate::map_data::MapTriggerVolume;
use crate::parse::{quake_to_engine, shambler_to_dvec3};

/// An optional key's authored value, or `None` when the author left it blank.
///
/// TrenchBroom writes `""` for a field the author cleared rather than dropping the
/// key, so blank means "unset" for every key read through here: it falls back to
/// that key's FGD default instead of failing the compile over a value the author
/// cannot see — a float parse error on the empty string, or an "unknown value"
/// diagnostic that prints as empty backticks. Same posture as worldspawn
/// `_lightmap_density`.
fn authored<'a>(props: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    props
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}

/// Resolve a brush entity's world-space trigger AABB and activation data.
///
/// `classname` names the authored entity in every diagnostic: `switch` desugars
/// through this same resolver, and an error attributed to `trigger_volume` sends
/// the author hunting for an entity their map may not contain.
pub(crate) fn resolve_trigger_volume(
    geo_map: &GeoMap,
    brush_ids: &[BrushId],
    props: &HashMap<String, String>,
    scale: f64,
    classname: &str,
) -> Result<MapTriggerVolume> {
    let name = props
        .get("name")
        .map(|v| v.trim().to_owned())
        .unwrap_or_default();
    let activation = match authored(props, "activation").unwrap_or("touch") {
        "touch" | "0" => 0,
        "use" | "1" => 1,
        other => anyhow::bail!("{classname} `{name}` has unknown `activation` `{other}`"),
    };
    let command = match authored(props, "command").unwrap_or("start") {
        "start" | "0" => 0,
        "stop" | "1" => 1,
        "reverse" | "2" => 2,
        "go_to_path_node" | "goToPathNode" | "3" => 3,
        other => anyhow::bail!("{classname} `{name}` has unknown `command` `{other}`"),
    };
    let command_arg = props
        .get("command_arg")
        .map(|v| v.trim().to_owned())
        .unwrap_or_default();
    let target_tag = props
        .get("target_tag")
        .map(|v| v.trim().to_owned())
        .unwrap_or_default();
    let on_fire = props
        .get("on_fire")
        .map(|v| v.trim().to_owned())
        .unwrap_or_default();
    let on_exit = props
        .get("on_exit")
        .map(|v| v.trim().to_owned())
        .unwrap_or_default();
    if target_tag.is_empty() && on_fire.is_empty() && on_exit.is_empty() {
        log::warn!(
            "[LevelCompiler] {classname} `{name}` has neither `target_tag`, `on_fire`, nor `on_exit`; it will be inert"
        );
    }
    if command == 3 && command_arg.is_empty() {
        anyhow::bail!("{classname} `{name}` `go_to_path_node` requires `command_arg`");
    }
    let fire_mode = match authored(props, "fire_mode").unwrap_or("once") {
        "once" | "0" => 0,
        "multiple" | "1" => 1,
        other => anyhow::bail!("{classname} `{name}` has unknown `fire_mode` `{other}`"),
    };
    let rearm_ms = authored(props, "rearm_ms")
        .map(str::parse::<f32>)
        .transpose()
        .map_err(|e| anyhow::anyhow!("{classname} `{name}` invalid `rearm_ms`: {e}"))?
        .unwrap_or(0.0);
    if !rearm_ms.is_finite() || rearm_ms < 0.0 {
        anyhow::bail!(
            "{classname} `{name}` `rearm_ms` must be finite and non-negative, got {rearm_ms}"
        );
    }
    let enabled_on_spawn = match authored(props, "enabled_on_spawn").unwrap_or("1") {
        "1" | "true" | "True" => true,
        "0" | "false" | "False" => false,
        other => {
            anyhow::bail!("{classname} `{name}` `enabled_on_spawn` must be 0/1, got `{other}`")
        }
    };
    let geo_planes = face_planes(&geo_map.face_planes);
    let entity_brush_faces: BTreeMap<BrushId, Vec<shambler::face::FaceId>> = brush_ids
        .iter()
        .filter_map(|id| {
            geo_map
                .brush_faces
                .get(id)
                .map(|faces| (*id, faces.clone()))
        })
        .collect();
    let hulls = brush_hulls(&entity_brush_faces, &geo_planes);
    let (face_verts, _) = face_vertices(&entity_brush_faces, &geo_planes, &hulls);
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for verts in face_verts.values() {
        for v in verts {
            let p = quake_to_engine(shambler_to_dvec3(v)) * scale;
            min = min.min(p);
            max = max.max(p);
        }
    }
    if !min.is_finite() {
        anyhow::bail!("{classname} `{name}` brushes produced no usable vertices");
    }
    if (max - min).min_element() <= 0.0 {
        anyhow::bail!("{classname} `{name}` AABB has zero extent");
    }
    Ok(MapTriggerVolume {
        name,
        tags: props
            .get("_tags")
            .map(|s| s.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default(),
        aabb_min: min.to_array().map(|v| v as f32),
        aabb_max: max.to_array().map(|v| v as f32),
        activation,
        target_tag,
        command,
        command_arg,
        fire_mode,
        rearm_ms,
        enabled_on_spawn,
        on_fire,
        on_exit,
    })
}

pub fn encode_trigger_volumes_section(
    triggers: &[MapTriggerVolume],
) -> Option<TriggerVolumesSection> {
    (!triggers.is_empty()).then(|| TriggerVolumesSection {
        triggers: triggers
            .iter()
            .map(|t| TriggerVolumeRecord {
                name: t.name.clone(),
                tags: t.tags.clone(),
                aabb_min: t.aabb_min,
                aabb_max: t.aabb_max,
                activation: t.activation,
                target_tag: t.target_tag.clone(),
                command: t.command,
                command_arg: t.command_arg.clone(),
                fire_mode: t.fire_mode,
                rearm_ms: t.rearm_ms,
                enabled_on_spawn: t.enabled_on_spawn,
                on_fire: t.on_fire.clone(),
                on_exit: t.on_exit.clone(),
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One 32-unit axis-aligned box brush, each face's point triple wound so its
    /// plane normal points out of the box. The resolver only needs vertices for the
    /// AABB, so every test drives the same brush and varies the properties.
    ///
    /// No blank line may appear inside the entity block — shalrath silently drops
    /// the remainder of the map on one.
    const BOX_BRUSH_MAP: &str = r#"// entity 0
{
"classname" "trigger_volume"
{
( 0 0 0 ) ( 0 1 0 ) ( 0 0 1 ) trigger_tex 0 0 0 1 1
( 32 0 0 ) ( 32 0 1 ) ( 32 1 0 ) trigger_tex 0 0 0 1 1
( 0 0 0 ) ( 0 0 1 ) ( 1 0 0 ) trigger_tex 0 0 0 1 1
( 0 32 0 ) ( 1 32 0 ) ( 0 32 1 ) trigger_tex 0 0 0 1 1
( 0 0 0 ) ( 1 0 0 ) ( 0 1 0 ) trigger_tex 0 0 0 1 1
( 0 0 32 ) ( 0 1 32 ) ( 1 0 32 ) trigger_tex 0 0 0 1 1
}
}
"#;

    /// Resolve a `trigger_volume` from `kvps` over the fixture brush.
    fn resolve(kvps: &[(&str, &str)]) -> Result<MapTriggerVolume> {
        let parsed: shambler::shalrath::repr::Map =
            BOX_BRUSH_MAP.parse().expect("fixture brush parses");
        let geo_map = GeoMap::new(parsed);
        let brush_ids: Vec<BrushId> = geo_map.brushes.iter().copied().collect();
        let props: HashMap<String, String> = kvps
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        resolve_trigger_volume(&geo_map, &brush_ids, &props, 1.0, "trigger_volume")
    }

    fn assert_resolves_like_absent_keys(cleared: &MapTriggerVolume, absent: &MapTriggerVolume) {
        assert_eq!(cleared.activation, absent.activation, "activation");
        assert_eq!(cleared.command, absent.command, "command");
        assert_eq!(cleared.fire_mode, absent.fire_mode, "fire_mode");
        assert_eq!(cleared.enabled_on_spawn, absent.enabled_on_spawn, "enabled_on_spawn");
        assert!(
            (cleared.rearm_ms - absent.rearm_ms).abs() < 1e-6,
            "rearm_ms: {} vs {}",
            cleared.rearm_ms,
            absent.rearm_ms
        );
    }

    // Regression: clearing `rearm_ms` in TrenchBroom failed the compile with
    // "cannot parse float from empty string", and clearing `activation`, `command`,
    // `fire_mode`, or `enabled_on_spawn` reported an unknown value that printed as
    // empty backticks — all for a value the author could no longer see.
    #[test]
    fn trigger_volume_cleared_optional_keys_resolve_like_absent_ones() {
        let absent = resolve(&[("on_fire", "open_gate")]).expect("bare trigger resolves");
        for blank in ["", " ", "\t", "  \t "] {
            let cleared = resolve(&[
                ("on_fire", "open_gate"),
                ("activation", blank),
                ("command", blank),
                ("fire_mode", blank),
                ("rearm_ms", blank),
                ("enabled_on_spawn", blank),
            ])
            .unwrap_or_else(|e| panic!("cleared keys must fall back to defaults, got: {e}"));
            assert_resolves_like_absent_keys(&cleared, &absent);
        }
    }

    #[test]
    fn trigger_volume_defaults_match_the_fgd_when_optional_keys_are_cleared() {
        let cleared = resolve(&[
            ("on_fire", "open_gate"),
            ("activation", ""),
            ("command", ""),
            ("fire_mode", ""),
            ("rearm_ms", ""),
            ("enabled_on_spawn", ""),
        ])
        .expect("cleared keys must fall back to defaults");
        // `touch`, `start`, `once`, no rearm delay, enabled — the defaults
        // `postretro.fgd` shows the author in the editor.
        assert_eq!(cleared.activation, 0);
        assert_eq!(cleared.command, 0);
        assert_eq!(cleared.fire_mode, 0);
        assert!(cleared.rearm_ms.abs() < 1e-6);
        assert!(cleared.enabled_on_spawn);
    }

    #[test]
    fn trigger_volume_cleared_command_arg_still_fails_go_to_path_node() {
        // The blank-means-absent posture must not reach the one key that is
        // *required* by another key's value: a cleared `command_arg` under
        // `go_to_path_node` has no default to fall back to.
        for blank in ["", "   "] {
            let error = resolve(&[
                ("command", "go_to_path_node"),
                ("command_arg", blank),
                ("on_fire", "open_gate"),
            ])
            .expect_err("go_to_path_node without a command_arg must not compile");
            let message = error.to_string();
            assert!(
                message.contains("`go_to_path_node` requires `command_arg`"),
                "error must name the missing key: {message}"
            );
        }
        // Omitting the key entirely fails the same way.
        let error = resolve(&[("command", "go_to_path_node"), ("on_fire", "open_gate")])
            .expect_err("go_to_path_node without a command_arg must not compile");
        assert!(error.to_string().contains("`go_to_path_node` requires `command_arg`"));
    }

    #[test]
    fn trigger_volume_still_rejects_authored_values_it_cannot_resolve() {
        // The fallback covers blank only. A value the author can read in the editor
        // still has to be reported rather than silently replaced by a default.
        let error = resolve(&[("rearm_ms", "soon")]).expect_err("unparsable rearm_ms must fail");
        assert!(error.to_string().contains("invalid `rearm_ms`"), "{error}");

        let error = resolve(&[("rearm_ms", "-1")]).expect_err("negative rearm_ms must fail");
        assert!(error.to_string().contains("finite and non-negative"), "{error}");

        let error = resolve(&[("activation", "toggle")]).expect_err("unknown activation fails");
        assert!(error.to_string().contains("unknown `activation` `toggle`"), "{error}");

        let error = resolve(&[("command", "detonate")]).expect_err("unknown command must fail");
        assert!(error.to_string().contains("unknown `command` `detonate`"), "{error}");

        let error = resolve(&[("fire_mode", "twice")]).expect_err("unknown fire_mode must fail");
        assert!(error.to_string().contains("unknown `fire_mode` `twice`"), "{error}");

        let error =
            resolve(&[("enabled_on_spawn", "maybe")]).expect_err("unknown enabled_on_spawn fails");
        assert!(error.to_string().contains("`enabled_on_spawn` must be 0/1"), "{error}");
    }
}
