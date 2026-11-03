//! Placement occupancy: whether a pawn arriving at a spawn placement finds it
//! free. Join, reconnect and respawn all judge placements through this one rule.
//! See: context/lib/scripting.md §12 · context/lib/networking.md §Slot lifecycle

use std::collections::BTreeSet;

use glam::Vec3;
use postretro_entities::{ComponentKind, EntityId, EntityRegistry, Transform};
use postretro_foundation::{CapsuleParams, PlayerMovementComponent};

/// A placement a joiner may take, with the standing capsule its class arrives with.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacementSite {
    /// Original map index, the key seat assignments store.
    pub map_index: usize,
    pub foot_anchor: Vec3,
    /// `None` when the class has no movement capsule; such an arrival overlaps no pawn.
    pub capsule: Option<CapsuleParams>,
}

/// Capsule centre for a pawn standing on `foot_anchor`.
#[must_use]
pub fn standing_centre(foot_anchor: Vec3, capsule: &CapsuleParams) -> Vec3 {
    foot_anchor + Vec3::Y * (capsule.radius + capsule.half_height)
}

/// Placement occupancy as seen by one arriving pawn. Reads live positions when
/// queried, so a caller that relocates pawns between picks sees the result.
pub struct PlacementOccupancy<'a> {
    registry: &'a EntityRegistry,
    arriving: Option<EntityId>,
    held_elsewhere: BTreeSet<usize>,
}

impl<'a> PlacementOccupancy<'a> {
    /// `arriving` is excluded from the live test (`None` before a joiner's pawn
    /// exists). `held_elsewhere` lists placements other seats hold, held
    /// (disconnected) seats included.
    pub fn new(
        registry: &'a EntityRegistry,
        arriving: Option<EntityId>,
        held_elsewhere: impl IntoIterator<Item = usize>,
    ) -> Self {
        Self {
            registry,
            arriving,
            held_elsewhere: held_elsewhere.into_iter().collect(),
        }
    }

    /// Free: no other seat holds the placement and no live pawn overlaps the
    /// arriving capsule standing on it.
    #[must_use]
    pub fn is_free(
        &self,
        map_index: usize,
        foot_anchor: Vec3,
        capsule: Option<&CapsuleParams>,
    ) -> bool {
        !self.is_held_elsewhere(map_index) && !self.live_pawn_overlaps(foot_anchor, capsule)
    }

    #[must_use]
    pub fn is_held_elsewhere(&self, map_index: usize) -> bool {
        self.held_elsewhere.contains(&map_index)
    }

    /// Origin distance under the summed capsule radii with overlapping vertical
    /// extents. No physics query exists for pawn against pawn. Pawns committed
    /// to removal never block.
    #[must_use]
    pub fn live_pawn_overlaps(&self, foot_anchor: Vec3, capsule: Option<&CapsuleParams>) -> bool {
        let Some(capsule) = capsule else {
            return false;
        };
        let position = standing_centre(foot_anchor, capsule);
        let registry = self.registry;
        registry
            .query_by_component_and_tag(ComponentKind::PlayerMovement, None)
            .any(|(pawn, _)| {
                if Some(pawn) == self.arriving
                    || crate::scripting_systems::health::is_terminally_committed_to_removal(
                        registry, pawn,
                    )
                {
                    return false;
                }
                let (Ok(other), Ok(transform)) = (
                    registry.get_component::<PlayerMovementComponent>(pawn),
                    registry.get_component::<Transform>(pawn),
                ) else {
                    return false;
                };
                let radius = capsule.radius + other.capsule.radius;
                let lower = capsule.half_height + capsule.radius;
                let other_lower = other.capsule.half_height + other.capsule.radius;
                let delta = position - transform.position;
                delta.x * delta.x + delta.z * delta.z < radius * radius
                    && delta.y.abs() < lower + other_lower
            })
    }
}
