// Mover ghost sets: each link's `ghost_tags` resolved against this level's movers.
// See: context/lib/entity_model.md §7d

use std::collections::HashMap;

use glam::Vec3;
use postretro_entities::{EntityId, EntityRegistry, KinematicMoverComponent, Transform};
use postretro_level_loader::LevelWorld;

use super::eligibility::GhostCategory;
use crate::runtime_movers::{bounds_center_under, mover_local_bounds_center};

/// The unmatched-tag install warning's stable wording (acceptance contract).
pub(crate) const UNMATCHED_GHOST_TAG_WARNING: &str = "matches no mover";

/// Which movers each link ghosts, resolved once at level install from the
/// level file's mover `_tags`, so a host and a client that load the same level
/// resolve the same sets. Movers have no descriptor; a link's `ghost_tags` is
/// their only opt-in, and it is per link.
#[derive(Debug, Default)]
pub(crate) struct GhostMovers {
    /// Every mover some link lists, by mover id: its local bounds center, or
    /// `None` for a mover with no geometry, which joins at its origin.
    centers: HashMap<u32, Option<Vec3>>,
    /// Per link, in map order: the mover ids its tags name, sorted and
    /// deduplicated, so a mover matching several of one link's tags is one
    /// member of it.
    per_link: Vec<Vec<u32>>,
}

/// Where a candidate's membership is judged this frame, and which mover it is.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MemberPoint {
    /// The body point for pawns, the origin for descriptor entities and
    /// projectiles, the presented bounds center for movers.
    pub position: Vec3,
    /// A mover's id: only a link whose own tags name it may show it.
    /// `None` for every other member, which any link may show.
    mover: Option<u32>,
}

impl GhostMovers {
    /// Resolve every link's `ghost_tags`. A listed tag no mover carries warns
    /// once at install, on every role, and the level loads without it.
    pub(crate) fn resolve(world: &LevelWorld) -> Self {
        let movers = &world.kinematic_geometry.movers;
        let mut centers = HashMap::new();
        let per_link = world
            .ghost_links
            .links
            .iter()
            .enumerate()
            .map(|(index, record)| {
                let mut ids = Vec::new();
                for tag in &record.ghost_tags {
                    let before = ids.len();
                    ids.extend(
                        movers
                            .iter()
                            .filter(|mover| mover.tags.iter().any(|carried| carried == tag))
                            .map(|mover| mover.mover_id),
                    );
                    if ids.len() == before {
                        log::warn!(
                            "[Ghost] ghost link {index} (entity {}): ghost tag `{tag}` \
                             {UNMATCHED_GHOST_TAG_WARNING}; the link ghosts no mover by it",
                            record.source_entity
                        );
                    }
                }
                ids.sort_unstable();
                ids.dedup();
                for &id in &ids {
                    centers.entry(id).or_insert_with(|| {
                        movers
                            .iter()
                            .find(|mover| mover.mover_id == id)
                            .and_then(mover_local_bounds_center)
                    });
                }
                ids
            })
            .collect();
        Self { centers, per_link }
    }

    /// No link ghosts any mover: candidate enumeration skips movers entirely.
    pub(crate) fn is_empty(&self) -> bool {
        self.centers.is_empty()
    }

    /// Whether some link's tags name mover `mover_id`: the mover's opt-in.
    pub(crate) fn can_ghost(&self, mover_id: u32) -> bool {
        self.centers.contains_key(&mover_id)
    }

    /// Where `entity` presented at `presented` is judged against link volumes.
    pub(crate) fn member_point(
        &self,
        registry: &EntityRegistry,
        entity: EntityId,
        category: GhostCategory,
        presented: Transform,
    ) -> MemberPoint {
        if category != GhostCategory::Movers {
            return MemberPoint {
                position: presented.position,
                mover: None,
            };
        }
        // Classification read this component; a mover id no link lists (the
        // component gone mid-frame) matches no link's set.
        let mover = registry
            .get_component::<KinematicMoverComponent>(entity)
            .map_or(u32::MAX, |mover| mover.mover_id);
        let center = self.centers.get(&mover).copied().flatten();
        MemberPoint {
            position: center.map_or(presented.position, |center| {
                bounds_center_under(center, presented)
            }),
            mover: Some(mover),
        }
    }

    /// Whether link `link` may show the member at `point`: always for a
    /// descriptor member, and for a mover only through a link whose own tags
    /// name it.
    pub(crate) fn listed_by(&self, point: &MemberPoint, link: usize) -> bool {
        let Some(mover) = point.mover else {
            return true;
        };
        self.per_link
            .get(link)
            .is_some_and(|ids| ids.binary_search(&mover).is_ok())
    }
}

#[cfg(test)]
#[path = "movers_tests.rs"]
mod tests;
