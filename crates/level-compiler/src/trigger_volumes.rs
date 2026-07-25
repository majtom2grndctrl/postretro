use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use glam::DVec3;
use postretro_level_format::trigger_volumes::{TriggerVolumeRecord, TriggerVolumesSection};
use shambler::GeoMap;
use shambler::brush::{BrushId, brush_hulls};
use shambler::face::{face_planes, face_vertices};

use crate::map_data::MapTriggerVolume;
use crate::parse::{quake_to_engine, shambler_to_dvec3};

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
    let activation = match props.get("activation").map(|v| v.trim()).unwrap_or("touch") {
        "touch" | "0" => 0,
        "use" | "1" => 1,
        other => anyhow::bail!("{classname} `{name}` has unknown `activation` `{other}`"),
    };
    let command = match props.get("command").map(|v| v.trim()).unwrap_or("start") {
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
    let fire_mode = match props.get("fire_mode").map(|v| v.trim()).unwrap_or("once") {
        "once" | "0" => 0,
        "multiple" | "1" => 1,
        other => anyhow::bail!("{classname} `{name}` has unknown `fire_mode` `{other}`"),
    };
    let rearm_ms = props
        .get("rearm_ms")
        .map(|v| v.trim().parse::<f32>())
        .transpose()
        .map_err(|e| anyhow::anyhow!("{classname} `{name}` invalid `rearm_ms`: {e}"))?
        .unwrap_or(0.0);
    if !rearm_ms.is_finite() || rearm_ms < 0.0 {
        anyhow::bail!(
            "{classname} `{name}` `rearm_ms` must be finite and non-negative, got {rearm_ms}"
        );
    }
    let enabled_on_spawn = match props
        .get("enabled_on_spawn")
        .map(|v| v.trim())
        .unwrap_or("1")
    {
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
