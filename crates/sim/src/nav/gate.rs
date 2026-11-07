// The live side of baked mover records: a read view over the mover dock table,
// so a portal closes only while its mover is docked at its waypoint.
// See: context/lib/build_pipeline.md §Navigation bake

use postretro_entities::components::agent::{CrossingKind, PathCrossing};
use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry};

use super::NavGraph;
use super::path::edge_side;
use crate::collision::CollisionCapsule;
use crate::collision::moving::{
    EMPTY_MOVER_POSES, MoverCollider, MoverPoseSource, capsule_overlaps_mover,
};
use crate::kinematic_mover::{MoverDockTable, MoverTickStateTable};
use glam::Vec3;

/// What baked records need from the level's movers: a mover and the height of
/// each waypoint along its path.
#[derive(Debug, Clone, PartialEq)]
pub struct NavMoverBound {
    pub mover_id: u32,
    pub waypoint_heights: Vec<f32>,
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

    /// Changes on every dock-state change of a mover with baked records: a
    /// dock, an undock, or a change in whether it moves on its own.
    pub fn generation(self) -> u64 {
        self.docks.map_or(0, MoverDockTable::generation)
    }

    /// The [`Self::generation`] of the latest dock change that could open a
    /// route: a mover leaving a dock that closes a portal, docking where a link
    /// opens, or starting to move on its own there. Never ahead of the
    /// generation, so an agent that took the live generation is not owed.
    pub fn opened_generation(self) -> u64 {
        self.docks.map_or(0, MoverDockTable::opened_generation)
    }

    pub fn docked_at(self, mover_id: u32, waypoint: u32) -> bool {
        self.docks
            .is_some_and(|docks| docks.docked_waypoint(mover_id) == Some(waypoint))
    }

    pub fn is_docked(self, mover_id: u32) -> bool {
        self.docked_waypoint(mover_id).is_some()
    }

    pub fn docked_waypoint(self, mover_id: u32) -> Option<u32> {
        self.docks?.docked_waypoint(mover_id)
    }

    /// Whether the mover will leave its dock by itself, so a ride can start.
    pub fn moves_on_its_own(self, mover_id: u32) -> bool {
        self.docks
            .is_some_and(|docks| docks.moves_on_its_own(mover_id))
    }

    /// Whether baked records name this mover, so its docks are tracked.
    pub fn watches(self, mover_id: u32) -> bool {
        self.docks.is_some_and(|docks| docks.watches(mover_id))
    }

    /// Whether a mover docked at the portal's gate waypoint closes it now.
    pub fn portal_closed(self, graph: &NavGraph, portal_index: usize) -> bool {
        self.docks.is_some()
            && graph
                .portal_gates(portal_index)
                .iter()
                .any(|&(mover_id, waypoint)| self.docked_at(mover_id, waypoint))
    }

    /// Whether a link is closed now: its mover rests anywhere but the link's
    /// waypoint, as A* reads it.
    pub(crate) fn link_closed(self, graph: &NavGraph, link_index: usize) -> bool {
        graph
            .links()
            .get(link_index)
            .is_none_or(|link| !self.docked_at(link.mover_id, link.waypoint))
    }

    /// Whether a mover's dock now closes a crossing still ahead of a body at
    /// `position` walking `path` toward `path[cursor]`. A crossing toward an
    /// earlier waypoint is behind it; one toward the current waypoint is
    /// behind once the body stands on that waypoint's side of the edge, so a
    /// door closing at a chaser's back cuts nothing. Allocation-free.
    pub(crate) fn closes_route_ahead(
        self,
        graph: &NavGraph,
        crossings: &[PathCrossing],
        path: &[Vec3],
        cursor: usize,
        position: Vec3,
    ) -> bool {
        crossings.iter().any(|crossing| {
            let index = crossing.index as usize;
            let (closed, edge) = match crossing.kind {
                CrossingKind::Portal => (
                    self.portal_closed(graph, index),
                    graph.portals().get(index).map(|p| (p.left, p.right)),
                ),
                CrossingKind::Link => (
                    self.link_closed(graph, index),
                    graph.links().get(index).map(|l| (l.left, l.right)),
                ),
            };
            if !closed {
                return false;
            }
            let waypoint = crossing.waypoint as usize;
            if waypoint != cursor {
                return waypoint > cursor;
            }
            let (Some((left, right)), Some(&target)) = (edge, path.get(waypoint)) else {
                return true;
            };
            let edge = (Vec3::from_array(left), Vec3::from_array(right));
            match (edge_side(edge, position), edge_side(edge, target)) {
                (Some(here), Some(there)) => here == 0.0 || there == 0.0 || here * there < 0.0,
                _ => true,
            }
        })
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
                waypoint_heights: mover.waypoints.iter().map(|at| at.y).collect(),
            }),
            _ => None,
        })
        .collect();
    graph.validate_mover_records(&bounds);
    let gated: Vec<(u32, u32)> = graph
        .gates()
        .iter()
        .map(|gate| (gate.mover_id, gate.waypoint))
        .collect();
    let linked: Vec<(u32, u32)> = graph
        .links()
        .iter()
        .map(|link| (link.mover_id, link.waypoint))
        .collect();
    mover_tick_states.watch_mover_docks(&gated, &linked);
}
