// The live side of baked mover records: a read view over the mover dock table,
// so a portal closes only while its mover is docked at its waypoint.
// See: context/lib/build_pipeline.md §Navigation bake

use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry};

use super::NavGraph;
use crate::collision::CollisionCapsule;
use crate::collision::moving::{
    EMPTY_MOVER_POSES, MoverCollider, MoverPoseSource, capsule_overlaps_mover,
};
use crate::kinematic_mover::{MoverDockTable, MoverTickStateTable};
use glam::Vec3;

/// What baked records need from the level's movers: a mover and its path length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavMoverBound {
    pub mover_id: u32,
    pub waypoint_count: u32,
}

/// One tick's gate state. `Copy` and read-only, so Ai, combat scoring and
/// steering all see one generation: dock state changes only in the mover tick.
#[derive(Clone, Copy)]
pub struct NavGate<'a> {
    docks: Option<&'a MoverDockTable>,
}

impl<'a> NavGate<'a> {
    /// No mover docked: every portal open.
    pub const OPEN: NavGate<'static> = NavGate { docks: None };

    pub fn new(docks: &'a MoverDockTable) -> Self {
        Self { docks: Some(docks) }
    }

    /// Changes only on a dock or undock of a mover with baked records.
    pub fn generation(self) -> u64 {
        self.docks.map_or(0, MoverDockTable::generation)
    }

    pub fn docked_at(self, mover_id: u32, waypoint: u32) -> bool {
        self.docks
            .is_some_and(|docks| docks.docked_waypoint(mover_id) == Some(waypoint))
    }

    pub fn is_docked(self, mover_id: u32) -> bool {
        self.docks
            .is_some_and(|docks| docks.docked_waypoint(mover_id).is_some())
    }

    /// Whether a mover docked at the portal's gate waypoint closes it now.
    pub fn portal_closed(self, graph: &NavGraph, portal_index: usize) -> bool {
        self.docks.is_some()
            && graph
                .portal_gates(portal_index)
                .iter()
                .any(|&(mover_id, waypoint)| self.docked_at(mover_id, waypoint))
    }
}

/// Mover state one host tick's navigation consumers read.
#[derive(Clone, Copy)]
pub struct MoverNav<'a> {
    pub gate: NavGate<'a>,
    pub movers: &'a [MoverCollider],
    pub poses: &'a dyn MoverPoseSource,
}

impl std::fmt::Debug for MoverNav<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MoverNav")
            .field("generation", &self.gate.generation())
            .field("movers", &self.movers.len())
            .finish()
    }
}

impl MoverNav<'_> {
    /// A world with no movers.
    pub fn none() -> MoverNav<'static> {
        MoverNav {
            gate: NavGate::OPEN,
            movers: &[],
            poses: &EMPTY_MOVER_POSES,
        }
    }

    /// Whether a capsule at `position` overlaps a mover docked as it stands.
    pub(crate) fn overlaps_docked_mover(self, position: Vec3, capsule: CollisionCapsule) -> bool {
        self.movers.iter().any(|mover| {
            self.gate.is_docked(mover.mover_id())
                && capsule_overlaps_mover(mover, self.poses, position, capsule)
        })
    }
}

/// Bind a level's baked records to its spawned movers: drop the faulty ones and
/// watch the movers the rest name. Runs once per level install, after mover
/// spawn, since the section alone cannot see the movers. A level whose movers
/// failed to spawn has none, so every record drops.
pub fn install_mover_records(
    graph: &mut NavGraph,
    registry: &EntityRegistry,
    mover_tick_states: &mut MoverTickStateTable,
) {
    let bounds: Vec<NavMoverBound> = registry
        .iter_with_kind(ComponentKind::KinematicMover)
        .filter_map(|(_, value)| match value {
            ComponentValue::KinematicMover(mover) => Some(NavMoverBound {
                mover_id: mover.mover_id,
                waypoint_count: mover.waypoints.len() as u32,
            }),
            _ => None,
        })
        .collect();
    graph.validate_mover_records(&bounds);
    mover_tick_states.watch_mover_docks(&graph.record_mover_ids());
}
