//! Per-batch sharing of combat-slot placement work. Grounding, the
//! docked-mover test and the tactically-direct target route depend only on
//! the target and the slot, so a batch computes each once per position and
//! every enemy reads it; clearance reads a proximity grid of the batch's
//! agents. Nothing outlives the batch: movers, gates and agents move between
//! batches.
//!
//! See: context/lib/entity_model.md §7c · context/lib/entity_model.md §9

use std::collections::HashMap;

use glam::Vec3;

use super::scoring::{
    Direct, PlacementSource, capsule_placement, dynamic_min_spacing, peer_is_clear,
};
use super::{CombatAgentSnapshot, CombatQuery};
use crate::collision::CapsulePlacement;
use crate::nav::NavScratch;

type Bits = [u32; 3];

fn bits(v: Vec3) -> Bits {
    [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]
}

/// One batch's shared placement answers. Built only when every query reads
/// the same nav graph, collision world, mover state and agent snapshot, the
/// state the cached answers depend on.
pub(super) struct BatchPlacements<'a> {
    grounded: HashMap<Bits, Option<Vec3>>,
    docked: HashMap<Bits, bool>,
    direct: HashMap<(Bits, Bits), bool>,
    peers: Option<PeerGrid<'a>>,
}

impl<'a> BatchPlacements<'a> {
    pub(super) fn for_batch(queries: &[CombatQuery<'a>]) -> Option<Self> {
        let first = queries.first()?;
        let shared = queries.iter().all(|query| {
            std::ptr::eq(query.nav_graph, first.nav_graph)
                && std::ptr::eq(query.collision_world, first.collision_world)
                && std::ptr::eq(query.mover_nav.movers, first.mover_nav.movers)
                && std::ptr::addr_eq(query.mover_nav.poses, first.mover_nav.poses)
                && query.mover_nav.gate.same_state(first.mover_nav.gate)
        });
        if !shared {
            return None;
        }
        let same_agents = queries
            .iter()
            .all(|query| std::ptr::eq(query.other_agents, first.other_agents));
        let peers = capsule_placement(first.nav_graph)
            .filter(|_| same_agents)
            .and_then(|placement| {
                PeerGrid::build(first.other_agents, dynamic_min_spacing(placement))
            });
        Some(Self {
            grounded: HashMap::new(),
            docked: HashMap::new(),
            direct: HashMap::new(),
            peers,
        })
    }
}

impl PlacementSource for BatchPlacements<'_> {
    fn grounded(
        &mut self,
        query: &CombatQuery<'_>,
        placement: CapsulePlacement,
        position: Vec3,
    ) -> Option<Vec3> {
        *self
            .grounded
            .entry(bits(position))
            .or_insert_with(|| Direct.grounded(query, placement, position))
    }

    fn clear(
        &mut self,
        query: &CombatQuery<'_>,
        placement: CapsulePlacement,
        position: Vec3,
    ) -> bool {
        match &self.peers {
            Some(grid) => grid.clear(query, position),
            None => Direct.clear(query, placement, position),
        }
    }

    fn overlaps_docked_mover(
        &mut self,
        query: &CombatQuery<'_>,
        placement: CapsulePlacement,
        position: Vec3,
    ) -> bool {
        *self
            .docked
            .entry(bits(position))
            .or_insert_with(|| Direct.overlaps_docked_mover(query, placement, position))
    }

    fn tactically_direct(
        &mut self,
        scratch: &mut NavScratch,
        query: &CombatQuery<'_>,
        placement: CapsulePlacement,
        position: Vec3,
        target_distance: f32,
    ) -> bool {
        *self
            .direct
            .entry((bits(query.target_pos), bits(position)))
            .or_insert_with(|| {
                Direct.tactically_direct(scratch, query, placement, position, target_distance)
            })
    }
}

/// The batch's agents bucketed on an XZ grid one spacing wide: a clearance
/// test reads the agents in the cells its spacing spans, with the same rule
/// as the full scan. A non-finite agent never blocks, so it is left out.
struct PeerGrid<'a> {
    agents: &'a [CombatAgentSnapshot],
    spacing: f32,
    /// `(cell x, cell z, agent index)`, sorted.
    cells: Vec<(i64, i64, u32)>,
}

impl<'a> PeerGrid<'a> {
    fn build(agents: &'a [CombatAgentSnapshot], spacing: f32) -> Option<Self> {
        if !spacing.is_finite() || spacing <= 0.0 {
            return None;
        }
        let mut grid = Self {
            agents,
            spacing,
            cells: Vec::with_capacity(agents.len()),
        };
        for (index, agent) in agents.iter().enumerate() {
            if agent.position.is_finite() {
                let (x, z) = (grid.cell(agent.position.x), grid.cell(agent.position.z));
                grid.cells.push((x, z, index as u32));
            }
        }
        grid.cells.sort_unstable();
        Some(grid)
    }

    /// Rounding is monotonic, so an agent within the spacing of a point lies
    /// in a cell between those of `point ∓ spacing`; one more cell each side
    /// absorbs the rounding of those bounds.
    fn cell(&self, value: f32) -> i64 {
        (value / self.spacing).floor() as i64
    }

    fn clear(&self, query: &CombatQuery<'_>, position: Vec3) -> bool {
        if !position.is_finite() {
            return Direct.clear_against(query, self.agents, position, self.spacing);
        }
        let (clear, _examined) = self.scan(query, position);
        #[cfg(test)]
        super::scoring::count_clearance(_examined);
        clear
    }

    /// Whether the slot is clear, and how many agents the scan examined.
    fn scan(&self, query: &CombatQuery<'_>, position: Vec3) -> (bool, usize) {
        let x0 = self.cell(position.x - self.spacing).saturating_sub(1);
        let x1 = self.cell(position.x + self.spacing).saturating_add(1);
        let z0 = self.cell(position.z - self.spacing).saturating_sub(1);
        let z1 = self.cell(position.z + self.spacing).saturating_add(1);
        let mut examined = 0;
        for x in x0..=x1 {
            let start = self
                .cells
                .partition_point(|&(cx, cz, _)| (cx, cz) < (x, z0));
            for &(cx, cz, index) in &self.cells[start..] {
                if cx != x || cz > z1 {
                    break;
                }
                examined += 1;
                if !peer_is_clear(query, &self.agents[index as usize], position, self.spacing) {
                    return (false, examined);
                }
            }
        }
        (true, examined)
    }
}

impl Direct {
    /// The full clearance scan over `agents`.
    fn clear_against(
        &self,
        query: &CombatQuery<'_>,
        agents: &[CombatAgentSnapshot],
        position: Vec3,
        spacing: f32,
    ) -> bool {
        agents
            .iter()
            .all(|other| peer_is_clear(query, other, position, spacing))
    }
}
