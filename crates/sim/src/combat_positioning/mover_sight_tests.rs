// Combat-slot sight sees movers at the tick's pose; slot placement stays static.
// See: context/lib/entity_model.md §7c (Sight sees movers)
use glam::{Quat, Vec3};

use super::tests::{REST_Y, floor_world, open_nav_graph, query_with};
use super::*;
use crate::collision::test_movers::{DoorPoses, door_slab};
use crate::nav::{MoverNav, NavGate, NavScratch};

const DOOR: u32 = 9;

/// A door at x = 4 across the line from a slot at x = 3 to a target at x = 5.
fn door_between(open: bool) -> DoorPoses {
    let y = if open { 10.0 } else { 0.0 };
    DoorPoses::default().place(DOOR, Vec3::new(4.0, y, 5.0), Quat::IDENTITY)
}

#[test]
fn slot_behind_a_closed_mover_scores_no_sight_and_one_through_the_gap_is_clear() {
    let nav_graph = open_nav_graph();
    let world = floor_world();
    let movers = [door_slab(DOOR, 0.1, 4.0, 2.0)];
    let held_slot = Vec3::new(3.0, REST_Y, 5.0);
    for open in [false, true] {
        let poses = door_between(open);
        let mut incumbent = query_with(
            1,
            &nav_graph,
            &world,
            held_slot,
            Vec3::new(5.0, REST_Y, 5.0),
            Some(held_slot),
            &[],
        );
        incumbent.mover_nav = MoverNav {
            gate: NavGate::OPEN,
            movers: &movers,
            poses: &poses,
        };
        incumbent.scan_challengers = false;
        assert_eq!(
            combat_candidates(&incumbent).is_empty(),
            !open,
            "door open: {open}"
        );
    }
}

/// With a closed door across some candidate lines, the cached and uncached
/// batches agree, and every slot that survives grounds where it does with no
/// mover at all.
#[test]
fn closed_mover_changes_slot_sight_only() {
    let nav_graph = open_nav_graph();
    let world = floor_world();
    let movers = [door_slab(DOOR, 0.1, 4.0, 2.0)];
    let poses = door_between(false);
    let target = Vec3::new(5.0, REST_Y, 5.0);
    let others: [CombatAgentSnapshot; 0] = [];
    let with_door = batch(
        &nav_graph,
        &world,
        target,
        &others,
        MoverNav {
            gate: NavGate::OPEN,
            movers: &movers,
            poses: &poses,
        },
    );
    let live = select_combat_positions_batch_in(&with_door, &mut NavScratch::new());
    let reference = select_combat_positions_batch_uncached(&with_door, &mut NavScratch::new());
    assert_eq!(live, reference);

    let without = batch(&nav_graph, &world, target, &others, MoverNav::none());
    let mut blocked = 0;
    for (with_door, without) in with_door.iter().zip(&without) {
        let clear = combat_candidates(without);
        let seen = combat_candidates(with_door);
        blocked += clear.len() - seen.len();
        for candidate in &seen {
            assert!(
                clear
                    .iter()
                    .any(|other| other.position == candidate.position),
                "{:?} grounds where it does with no mover",
                candidate.position
            );
        }
    }
    assert!(blocked > 0, "the door hides at least one candidate");
}

/// Two enemies west and east of `target`, scanning for slots.
fn batch<'a>(
    nav_graph: &'a NavGraph,
    world: &'a CollisionWorld,
    target: Vec3,
    others: &'a [CombatAgentSnapshot],
    mover_nav: MoverNav<'a>,
) -> Vec<CombatQuery<'a>> {
    [Vec3::new(1.0, REST_Y, 5.0), Vec3::new(8.0, REST_Y, 6.0)]
        .into_iter()
        .enumerate()
        .map(|(index, agent)| {
            let mut query = query_with(
                1 + index as u32,
                nav_graph,
                world,
                agent,
                target,
                None,
                others,
            );
            query.mover_nav = mover_nav;
            query
        })
        .collect()
}
