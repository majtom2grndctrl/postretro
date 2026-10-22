//! Quake `ghost_link` authoring: regions, landmark and target resolved against
//! the teleport destinations, starting state and ghosted mover tags.
//! See: context/lib/build_pipeline.md §Custom FGD, §PRL section IDs (GhostLinks, id 56)
use super::teleporters::{
    Destination, enabled_on_spawn, optional, region_brushes, region_companion, required,
};
use crate::map_data::MapEntityRecord;
use anyhow::{Result, bail};
use postretro_level_format::ghost_links::{
    GHOST_LINK_CLASSNAME, GHOST_LINK_SOURCE_ENTITY_KEY, GhostLinkRecord, GhostLinksSection,
    GhostMembers, MAX_GHOST_LINKS,
};
use shambler::{GeoMap, brush::BrushId};
use std::collections::HashMap;

struct Pending {
    record: GhostLinkRecord,
    landmark: String,
    target: String,
}

#[derive(Default)]
pub(super) struct GhostLinkParser {
    pending: Vec<Pending>,
}

impl GhostLinkParser {
    /// Read one `ghost_link` entity. Poses resolve at [`Self::finish`], so a
    /// destination may follow the link in map order.
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
        if self.pending.len() >= MAX_GHOST_LINKS {
            bail!("{entity} exceeds the {MAX_GHOST_LINKS} ghost link limit");
        }
        let source_entity =
            u32::try_from(index).map_err(|_| anyhow::anyhow!("{entity} index exceeds u32"))?;
        let landmark = required(props, "landmark", &entity)?.to_owned();
        let target = required(props, "target", &entity)?.to_owned();
        if landmark == target {
            bail!("{entity} landmark must differ from target");
        }
        let enabled_on_spawn = enabled_on_spawn(props, &entity)?;
        let members = members(props, &entity)?;
        let brushes = region_brushes(geo, ids, scale, GHOST_LINK_CLASSNAME, &entity)?;
        companions.push(region_companion(
            props,
            GHOST_LINK_CLASSNAME,
            &brushes,
            GHOST_LINK_SOURCE_ENTITY_KEY,
            source_entity,
        ));
        self.pending.push(Pending {
            record: GhostLinkRecord {
                source_entity,
                enabled_on_spawn,
                members,
                // An unknown style or an unmatched tag warns at install, where
                // the manifest and the movers are known; neither fails a build.
                style: optional(props, "ghost_style")
                    .unwrap_or_default()
                    .to_owned(),
                brushes,
                landmark_origin: [0.0; 3],
                landmark_yaw: 0.0,
                target_origin: [0.0; 3],
                target_yaw: 0.0,
                ghost_tags: ghost_tags(props),
            },
            landmark,
            target,
        });
        Ok(())
    }

    /// Resolve every link's landmark and target against the map's teleport
    /// destinations, the same table teleporters resolve against.
    pub(super) fn finish(
        self,
        destinations: &HashMap<String, Destination>,
    ) -> Result<GhostLinksSection> {
        let mut links = Vec::with_capacity(self.pending.len());
        for pending in self.pending {
            let mut record = pending.record;
            let entity = entity_label(record.source_entity as usize);
            let target = destinations
                .get(&pending.target)
                .ok_or_else(|| anyhow::anyhow!("{entity} unknown target `{}`", pending.target))?;
            let landmark = destinations.get(&pending.landmark).ok_or_else(|| {
                anyhow::anyhow!("{entity} unknown landmark `{}`", pending.landmark)
            })?;
            record.target_origin = target.origin;
            record.target_yaw = target.yaw;
            record.landmark_origin = landmark.origin;
            record.landmark_yaw = landmark.yaw;
            links.push(record);
        }
        Ok(GhostLinksSection { links })
    }
}

fn entity_label(index: usize) -> String {
    format!("entity {index} '{GHOST_LINK_CLASSNAME}'")
}

/// `ghost_members`: whitespace-delimited category names. Absent or blank is all
/// four, as every blank region key reads as absent.
fn members(props: &HashMap<String, String>, entity: &str) -> Result<GhostMembers> {
    let Some(raw) = optional(props, "ghost_members") else {
        return Ok(GhostMembers::all());
    };
    raw.split_whitespace()
        .try_fold(
            GhostMembers::empty(),
            |members, name| match GhostMembers::from_fgd_name(name) {
                Some(category) => Ok(members | category),
                None => {
                    let known: Vec<_> = GhostMembers::NAMED.iter().map(|(n, _)| *n).collect();
                    bail!(
                        "{entity} unknown ghost_members category `{name}`; expected any of `{}`",
                        known.join(" ")
                    )
                }
            },
        )
}

/// `ghost_tags`: whitespace-delimited mover tags, duplicates dropped, authored
/// order kept.
fn ghost_tags(props: &HashMap<String, String>) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in optional(props, "ghost_tags")
        .unwrap_or_default()
        .split_whitespace()
    {
        if !tags.iter().any(|seen| seen == tag) {
            tags.push(tag.to_owned());
        }
    }
    tags
}

#[cfg(test)]
#[path = "ghost_links/tests.rs"]
mod tests;
