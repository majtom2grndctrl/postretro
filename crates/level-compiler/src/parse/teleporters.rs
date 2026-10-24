//! Quake teleporter authoring; downstream data uses engine axes and metres.
//! See: context/lib/build_pipeline.md §Custom FGD, §PRL section IDs (Teleporters, id 55)
use crate::map_data::MapEntityRecord;
use anyhow::{Result, bail};
use glam::DVec3;
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
use postretro_level_format::teleporters::{
    MAX_TELEPORTERS, TELEPORTER_SOURCE_ENTITY_KEY, TeleportMode, TeleporterRecord,
    TeleportersSection,
};
use shambler::{GeoMap, brush::BrushId};
use std::collections::HashMap;

/// A resolved `teleport_destination`: teleporters and ghost links both name these.
pub(super) struct Destination {
    pub(super) origin: [f32; 3],
    pub(super) yaw: f32,
}
struct Pending {
    record: TeleporterRecord,
    target: String,
    landmark: String,
}
#[derive(Default)]
pub(super) struct TeleporterParser {
    destinations: HashMap<String, Destination>,
    pending: Vec<Pending>,
}
impl TeleporterParser {
    /// Return true for semantic teleporter entities, before brush classification.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn extract(
        &mut self,
        geo: &GeoMap,
        ids: &[BrushId],
        props: &HashMap<String, String>,
        scale: f64,
        classname: &str,
        index: usize,
        companions: &mut Vec<MapEntityRecord>,
    ) -> Result<bool> {
        if !matches!(classname, "trigger_teleport" | "teleport_destination") {
            return Ok(false);
        }
        let entity = format!("entity {index} '{classname}'");
        if classname == "teleport_destination" {
            if !ids.is_empty() {
                bail!("{entity} is a point and must have no brushes");
            }
            let name = required(props, "name", &entity)?;
            let origin = numbers::<3>(required(props, "origin", &entity)?, "origin", &entity)?;
            let origin = super::quake_to_engine(DVec3::from_array(origin.map(f64::from))) * scale;
            let origin = origin.as_vec3().to_array();
            if origin.iter().any(|v| !v.is_finite()) {
                bail!("{entity} origin exceeds engine precision");
            }
            // The point-entity angle converter, so `angle`, `angles` and `mangle`
            // face where TrenchBroom draws the arrow, as on any other entity.
            // Unlike map entities, a malformed value fails the build.
            let [pitch, yaw, roll] = crate::format::quake_map::try_quake_to_engine_angles(props)
                .map_err(|reason| anyhow::anyhow!("{entity} {reason}"))?;
            if pitch != 0.0 || roll != 0.0 {
                log::warn!(
                    "[Compiler] {entity} angles pitch and roll are ignored; only yaw is used"
                );
            }
            if self
                .destinations
                .insert(name.to_owned(), Destination { origin, yaw })
                .is_some()
            {
                bail!("{entity} duplicate teleport_destination name `{name}`");
            }
            return Ok(true);
        }
        if self.pending.len() >= MAX_TELEPORTERS {
            bail!("{entity} exceeds the {MAX_TELEPORTERS} teleporter limit");
        }
        let source_entity =
            u32::try_from(index).map_err(|_| anyhow::anyhow!("{entity} index exceeds u32"))?;
        let mode = match optional(props, "mode").unwrap_or("classic") {
            "classic" => TeleportMode::Classic,
            "seamless" => TeleportMode::Seamless,
            other => bail!("{entity} unknown mode `{other}`"),
        };
        let target = required(props, "target", &entity)?.to_owned();
        let landmark = optional(props, "landmark").unwrap_or_default().to_owned();
        match mode {
            TeleportMode::Classic if !landmark.is_empty() => {
                bail!("{entity} classic mode forbids landmark")
            }
            TeleportMode::Seamless if landmark.is_empty() => {
                bail!("{entity} seamless mode requires landmark")
            }
            TeleportMode::Seamless if landmark == target => {
                bail!("{entity} landmark must differ from target")
            }
            _ => {}
        }
        let enabled_on_spawn = enabled_on_spawn(props, &entity)?;
        let brushes = region_brushes(geo, ids, scale, classname, &entity)?;
        companions.push(region_companion(
            props,
            classname,
            &brushes,
            TELEPORTER_SOURCE_ENTITY_KEY,
            source_entity,
        ));
        self.pending.push(Pending {
            record: TeleporterRecord {
                source_entity,
                mode,
                enabled_on_spawn,
                brushes,
                destination_origin: [0.0; 3],
                destination_yaw: 0.0,
                landmark_origin: [0.0; 3],
                landmark_yaw: 0.0,
                arrival_sound: optional(props, "arrival_sound")
                    .unwrap_or_default()
                    .to_owned(),
                on_teleport: optional(props, "on_teleport")
                    .unwrap_or_default()
                    .to_owned(),
            },
            target,
            landmark,
        });
        Ok(true)
    }
    /// Every `teleport_destination` by name. Complete once every entity has
    /// been extracted, so sibling region parsers resolve against it at finish.
    pub(super) fn destinations(&self) -> &HashMap<String, Destination> {
        &self.destinations
    }
    pub(super) fn finish(self) -> Result<TeleportersSection> {
        let mut teleporters = Vec::with_capacity(self.pending.len());
        for pending in self.pending {
            let mut record = pending.record;
            let entity = format!("entity {} 'trigger_teleport'", record.source_entity);
            let destination = self
                .destinations
                .get(&pending.target)
                .ok_or_else(|| anyhow::anyhow!("{entity} unknown target `{}`", pending.target))?;
            record.destination_origin = destination.origin;
            record.destination_yaw = destination.yaw;
            if record.mode == TeleportMode::Seamless {
                let landmark = self.destinations.get(&pending.landmark).ok_or_else(|| {
                    anyhow::anyhow!("{entity} unknown landmark `{}`", pending.landmark)
                })?;
                record.landmark_origin = landmark.origin;
                record.landmark_yaw = landmark.yaw;
            }
            teleporters.push(record);
        }
        Ok(TeleportersSection { teleporters })
    }
}
/// A trimmed value; a blank one reads as absent.
pub(super) fn optional<'a>(props: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    props.get(key).map(|s| s.trim()).filter(|s| !s.is_empty())
}
pub(super) fn required<'a>(
    props: &'a HashMap<String, String>,
    key: &str,
    entity: &str,
) -> Result<&'a str> {
    optional(props, key).ok_or_else(|| anyhow::anyhow!("{entity} requires non-blank `{key}`"))
}
/// `enabled_on_spawn`, 0 or 1, default 1.
pub(super) fn enabled_on_spawn(props: &HashMap<String, String>, entity: &str) -> Result<bool> {
    match optional(props, "enabled_on_spawn").unwrap_or("1") {
        "0" => Ok(false),
        "1" => Ok(true),
        other => bail!("{entity} enabled_on_spawn must be 0/1, got `{other}`"),
    }
}
/// A script-armed region's brushes as convex leaves, peeled before BSP.
pub(super) fn region_brushes(
    geo: &GeoMap,
    ids: &[BrushId],
    scale: f64,
    classname: &str,
    entity: &str,
) -> Result<Vec<EnvironmentBrush>> {
    if ids.is_empty() {
        bail!("{entity} has no brushes");
    }
    ids.iter()
        .enumerate()
        .map(|(b, id)| {
            let leaf =
                super::environment_volumes::resolve_leaf(geo, *id, scale, classname, entity, b)?;
            Ok(EnvironmentBrush {
                aabb_min: leaf.min,
                aabb_max: leaf.max,
                planes: leaf
                    .planes
                    .into_iter()
                    .map(|[x, y, z, d]| EnvironmentPlane {
                        normal: [x, y, z],
                        distance: d,
                    })
                    .collect(),
            })
        })
        .collect()
}
/// A region's id-29 companion: its runtime keys plus `source_key`, its `_tags`,
/// at the centre of its brushes' union AABB. Runtime install finds it by
/// classname and `source_key`.
pub(super) fn region_companion(
    props: &HashMap<String, String>,
    classname: &str,
    brushes: &[EnvironmentBrush],
    source_key: &str,
    source_entity: u32,
) -> MapEntityRecord {
    let min = brushes.iter().fold(DVec3::splat(f64::INFINITY), |a, b| {
        a.min(DVec3::from(b.aabb_min.map(f64::from)))
    });
    let max = brushes
        .iter()
        .fold(DVec3::splat(f64::NEG_INFINITY), |a, b| {
            a.max(DVec3::from(b.aabb_max.map(f64::from)))
        });
    let mut key_values: Vec<_> = props
        .iter()
        .filter(|(key, _)| super::is_runtime_map_entity_key(key) && key.as_str() != source_key)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    key_values.push((source_key.to_owned(), source_entity.to_string()));
    key_values.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    MapEntityRecord {
        classname: classname.to_owned(),
        origin: (min + max) * 0.5,
        angles: [0.0; 3],
        key_values,
        tags: optional(props, "_tags")
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
    }
}
fn numbers<const N: usize>(raw: &str, key: &str, entity: &str) -> Result<[f32; N]> {
    let values: Vec<_> = raw
        .split_whitespace()
        .map(|s| s.parse::<f32>())
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| anyhow::anyhow!("{entity} invalid `{key}` `{raw}`"))?;
    if values.len() != N || values.iter().any(|v| !v.is_finite()) {
        bail!("{entity} `{key}` requires {N} finite numbers, got `{raw}`");
    }
    Ok(values.try_into().expect("checked number count"))
}

#[cfg(test)]
#[path = "teleporters/tests.rs"]
mod tests;
