//! Quake `level_seam` authoring: regions, keys, landmarks resolved to destinations.
//! See: context/lib/build_pipeline.md §Custom FGD, §PRL section IDs (Seams, id 61)
use super::teleporters::{
    Destination, enabled_on_spawn, optional, region_brushes, region_companion, required,
};
use crate::map_data::MapEntityRecord;
use anyhow::{Result, bail};
use postretro_level_format::seams::{
    MAX_SEAMS, SEAM_CLASSNAME, SEAM_SOURCE_ENTITY_KEY, SeamActivation, SeamMode, SeamRecord,
    SeamsSection,
};
use shambler::{GeoMap, brush::BrushId};
use std::collections::HashMap;

/// The classname a level start places players at.
const PLAYER_SPAWN_CLASSNAME: &str = "player_spawn";

/// A seam read from its entity, its landmark not yet resolved.
struct Pending {
    record: SeamRecord,
    /// The authored `teleport_destination` name; `None` keeps the bounds
    /// centre at yaw 0 the record already holds.
    landmark: Option<String>,
}

#[derive(Default)]
pub(super) struct SeamParser {
    pending: Vec<Pending>,
}

impl SeamParser {
    /// Read one `level_seam` entity. An authored landmark resolves at
    /// [`Self::finish`], so its destination may follow the seam in map order.
    // Mirrors `TeleporterParser::extract`, which every region parser shares.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn extract(
        &mut self,
        geo: &GeoMap,
        ids: &[BrushId],
        props: &HashMap<String, String>,
        scale: f64,
        index: usize,
        companions: &mut Vec<MapEntityRecord>,
    ) -> Result<()> {
        let entity = entity_label(index);
        if self.pending.len() >= MAX_SEAMS {
            bail!("{entity} exceeds the {MAX_SEAMS} seam limit");
        }
        let source_entity =
            u32::try_from(index).map_err(|_| anyhow::anyhow!("{entity} index exceeds u32"))?;
        let name = required(props, "name", &entity)?.to_owned();
        if self.pending.iter().any(|seam| seam.record.name == name) {
            bail!("{entity} duplicate seam name `{name}`");
        }
        // A catalog id: the compiler sees one map, so the catalog checks it
        // at activation.
        let target = required(props, "target", &entity)?.to_owned();
        let activation = match optional(props, "activation").unwrap_or("touch") {
            "touch" => SeamActivation::Touch,
            "use" => SeamActivation::Use,
            other => bail!("{entity} unknown activation `{other}`"),
        };
        // `seamless` compiles as written and crosses as classic until the
        // seamless unit lands; neither warns.
        let mode = match optional(props, "mode").unwrap_or("classic") {
            "classic" => SeamMode::Classic,
            "seamless" => SeamMode::Seamless,
            other => bail!("{entity} unknown mode `{other}`"),
        };
        let enabled_on_spawn = enabled_on_spawn(props, &entity)?;
        let landmark = optional(props, "landmark").map(str::to_owned);
        let brushes = region_brushes(geo, ids, scale, SEAM_CLASSNAME, &entity)?;
        let companion = region_companion(
            props,
            SEAM_CLASSNAME,
            &brushes,
            SEAM_SOURCE_ENTITY_KEY,
            source_entity,
        );
        // The companion stands at the brushes' union AABB centre: the
        // defaulted landmark, at yaw 0.
        let centre = companion.origin.as_vec3().to_array();
        if centre.iter().any(|v| !v.is_finite()) {
            bail!("{entity} bounds centre exceeds engine precision");
        }
        companions.push(companion);
        self.pending.push(Pending {
            record: SeamRecord {
                source_entity,
                name,
                target,
                activation,
                mode,
                enabled_on_spawn,
                brushes,
                landmark_origin: centre,
                landmark_yaw: 0.0,
                on_arrive: optional(props, "on_arrive").unwrap_or_default().to_owned(),
            },
            landmark,
        });
        Ok(())
    }

    /// Warn once when the map holds a seam but no player start: a blocked
    /// arrival, or a crossing into a map without the seam, falls back to one.
    pub(super) fn warn_without_player_spawn(&self, map_entities: &[MapEntityRecord]) {
        let Some(first) = self.pending.first() else {
            return;
        };
        if map_entities
            .iter()
            .any(|entity| entity.classname == PLAYER_SPAWN_CLASSNAME)
        {
            return;
        }
        log::warn!(
            "[Compiler] {} `{}`: map has a level_seam but no player_spawn; an arrival \
             that is blocked or finds no matching seam has no player start to fall back to",
            entity_label(first.record.source_entity as usize),
            first.record.name,
        );
    }

    /// Resolve every authored landmark against the map's teleport
    /// destinations, the same table teleporters and ghost links resolve against.
    pub(super) fn finish(
        self,
        destinations: &HashMap<String, Destination>,
    ) -> Result<SeamsSection> {
        let mut seams = Vec::with_capacity(self.pending.len());
        for pending in self.pending {
            let mut record = pending.record;
            if let Some(landmark) = pending.landmark {
                let entity = entity_label(record.source_entity as usize);
                let destination = destinations
                    .get(&landmark)
                    .ok_or_else(|| anyhow::anyhow!("{entity} unknown landmark `{landmark}`"))?;
                record.landmark_origin = destination.origin;
                record.landmark_yaw = destination.yaw;
            }
            seams.push(record);
        }
        Ok(SeamsSection { seams })
    }
}

fn entity_label(index: usize) -> String {
    format!("entity {index} '{SEAM_CLASSNAME}'")
}

#[cfg(test)]
#[path = "seams/tests.rs"]
mod tests;
