// Batched combat positioning against the uncached batch (the scorer as it ran
// before the per-batch placement cache), kept test-only: every assignment's
// slot, score terms, generation index and incumbency, across randomized scenes
// in the door rig's two rooms.
// See: context/lib/entity_model.md §7c

use super::*;
use crate::combat_positioning::{
    CombatAgentSnapshot, CombatAssignment, CombatQuery, KeepAway, PATH_LENGTH_SCORE_WEIGHT,
    SlotRing, select_combat_positions_batch_in, select_combat_positions_batch_uncached,
};
use crate::nav::NavScratch;

/// Agent capsule spacing: twice the radius.
const SPACING: f32 = 2.0 * RADIUS;
/// Eye heights above a slot's centre: one under the low wall's top, one over.
const LOW_EYE: f32 = 0.6;
const HIGH_EYE: f32 = 1.4;

/// Deterministic LCG in [0, 1).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
    }

    fn below(&mut self, n: usize) -> usize {
        ((self.next() * n as f32) as usize).min(n - 1)
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }

    /// A half-metre grid point in either room.
    fn room_point(&mut self) -> Vec3 {
        let x = if self.next() < 0.5 {
            1.0 + self.below(17) as f32 * 0.5
        } else {
            12.0 + self.below(17) as f32 * 0.5
        };
        Vec3::new(x, REST_Y, 1.0 + self.below(17) as f32 * 0.5)
    }
}

/// The rig's walls plus a 1.6 m wall across the west room at z = 2.5, x 2–8:
/// a low eye behind it cannot see over; a high one can.
fn low_wall_world() -> CollisionWorld {
    let mut points = Vec::new();
    let mut tris = Vec::new();
    quad(
        &mut points,
        &mut tris,
        [
            Vec3::new(-5.0, 0.0, -5.0),
            Vec3::new(30.0, 0.0, -5.0),
            Vec3::new(30.0, 0.0, 15.0),
            Vec3::new(-5.0, 0.0, 15.0),
        ],
    );
    for (z0, z1) in [(-5.0, 4.0), (6.0, 15.0)] {
        quad(
            &mut points,
            &mut tris,
            [
                Vec3::new(10.0, 0.0, z0),
                Vec3::new(10.0, 4.0, z0),
                Vec3::new(10.0, 4.0, z1),
                Vec3::new(10.0, 0.0, z1),
            ],
        );
    }
    quad(
        &mut points,
        &mut tris,
        [
            Vec3::new(2.0, 0.0, 2.5),
            Vec3::new(2.0, 1.6, 2.5),
            Vec3::new(8.0, 1.6, 2.5),
            Vec3::new(8.0, 0.0, 2.5),
        ],
    );
    CollisionWorld::from_triangles_for_test(points, tris)
}

/// One enemy's query inputs, before borrowing the shared surfaces.
#[derive(Clone, Copy)]
struct Enemy {
    claimant: u32,
    position: Vec3,
    target: Vec3,
    eye: f32,
    standoff: f32,
    slot: Option<Vec3>,
    ring: Option<SlotRing>,
}

fn query<'a>(
    enemy: &Enemy,
    others: &'a [CombatAgentSnapshot],
    rig: &'a Rig,
    world: &'a CollisionWorld,
    mover_nav: crate::nav::MoverNav<'a>,
) -> CombatQuery<'a> {
    CombatQuery {
        claimant_id: enemy.claimant,
        agent_pos: enemy.position,
        agent_ground: postretro_foundation::GroundRef::World,
        agent_height: HEIGHT,
        engagement_radius: enemy.standoff,
        target_pos: enemy.target,
        enemy_eye_offset: Vec3::Y * enemy.eye,
        target_aim: enemy.target + Vec3::Y * 0.5,
        combat_slot: enemy.slot,
        scan_challengers: enemy.slot.is_none(),
        other_agents: others,
        nav_graph: &rig.graph,
        mover_nav,
        collision_world: world,
        path_length_score_weight: PATH_LENGTH_SCORE_WEIGHT,
        ring: enemy.ring,
    }
}

/// Run the live batch on `scratch` and the uncached reference on a fresh one,
/// and require the same assignments.
fn assert_batch_matches_uncached(
    queries: &[CombatQuery<'_>],
    scratch: &mut NavScratch,
    label: &str,
) -> Vec<CombatAssignment> {
    let live = select_combat_positions_batch_in(queries, scratch);
    let reference = select_combat_positions_batch_uncached(queries, &mut NavScratch::new());
    assert_eq!(live, reference, "{label}");
    live
}

fn random_enemies(rng: &mut Lcg) -> Vec<Enemy> {
    let targets: Vec<Vec3> = (0..1 + rng.below(3)).map(|_| rng.room_point()).collect();
    (0..1 + rng.below(16))
        .map(|i| {
            let target = rng.pick(&targets);
            let standoff = rng.pick(&[1.5, 2.0, 3.0, 4.0]);
            let ring = (rng.next() < 0.35).then(|| {
                let angle = rng.next() * std::f32::consts::TAU;
                let bearing = Vec3::new(angle.cos(), 0.0, angle.sin());
                let ring = if rng.next() < 0.5 {
                    SlotRing::by_score(bearing, (rng.below(255) + 1) as u8)
                } else {
                    SlotRing::listed(bearing, &[0, 1, 7, 2, 6])
                };
                if rng.next() < 0.3 {
                    ring.with_keep_away(KeepAway {
                        point: rng.room_point(),
                        distance: 1.5,
                    })
                } else {
                    ring
                }
            });
            // An incumbent on the shared ring: a generated position (inside
            // or outside the switch margin, as it lands) or anywhere.
            let slot = (ring.is_none() && rng.next() < 0.4).then(|| match rng.below(3) {
                0 => rng.room_point(),
                _ => {
                    let angle = rng.below(8) as f32 * std::f32::consts::FRAC_PI_4;
                    target + Vec3::new(angle.cos(), 0.0, angle.sin()) * standoff
                }
            });
            Enemy {
                claimant: 1 + i as u32,
                position: rng.room_point(),
                target,
                eye: rng.pick(&[LOW_EYE, HIGH_EYE]),
                standoff,
                slot,
                ring,
            }
        })
        .collect()
}

/// Every enemy, plus a peer exactly one spacing east of the first enemy's
/// target's east slot (clear: spacing is a strict bound), one just inside it,
/// and one at a non-finite position.
fn snapshots(enemies: &[Enemy]) -> Vec<CombatAgentSnapshot> {
    let first = enemies[0];
    let east_slot = first.target + Vec3::X * first.standoff;
    enemies
        .iter()
        .map(|enemy| CombatAgentSnapshot {
            claimant_id: enemy.claimant,
            position: enemy.position,
        })
        .chain([
            CombatAgentSnapshot {
                claimant_id: 1_000,
                position: east_slot + Vec3::X * SPACING,
            },
            CombatAgentSnapshot {
                claimant_id: 1_001,
                position: east_slot - Vec3::Z * (SPACING * 0.99),
            },
            CombatAgentSnapshot {
                claimant_id: 1_002,
                position: Vec3::new(f32::NAN, REST_Y, 5.0),
            },
        ])
        .collect()
}

#[test]
fn batched_positioning_matches_the_uncached_batch_across_random_scenes() {
    let mut rig = closed_door();
    rig.tick();
    let world = low_wall_world();
    let mut scratch = NavScratch::new();
    let mut rng = Lcg(0xb47c4);
    let (mut assigned, mut unassigned, mut incumbents, mut rotated) = (0, 0, 0, 0);
    for case in 0..600 {
        let enemies = random_enemies(&mut rng);
        let others = snapshots(&enemies);
        let docked = rng.next() < 0.5;
        let mover_nav = if docked {
            crate::nav::MoverNav {
                gate: rig.gate(),
                movers: &rig.colliders,
                poses: &rig.movers,
            }
        } else {
            crate::nav::MoverNav::none()
        };
        let queries: Vec<CombatQuery<'_>> = enemies
            .iter()
            .map(|enemy| query(enemy, &others, &rig, &world, mover_nav))
            .collect();
        let assignments =
            assert_batch_matches_uncached(&queries, &mut scratch, &format!("case {case}"));
        for (assignment, enemy) in assignments.iter().zip(&enemies) {
            match assignment.candidate {
                Some(candidate) => {
                    assigned += 1;
                    incumbents += usize::from(candidate.is_incumbent);
                    rotated += usize::from(enemy.ring.is_some());
                }
                None => unassigned += 1,
            }
        }
    }
    let coverage = (assigned, unassigned, incumbents, rotated);
    assert!(
        assigned > 1_500 && unassigned > 300 && incumbents > 100 && rotated > 300,
        "{coverage:?}"
    );
}

/// Two enemies on one target south of the low wall, one with each eye height.
fn eye_pair(target: Vec3) -> [Enemy; 2] {
    let enemy = |claimant, eye| Enemy {
        claimant,
        position: Vec3::new(5.0, REST_Y, 1.0),
        target,
        eye,
        standoff: 3.0,
        slot: None,
        ring: None,
    };
    [enemy(1, LOW_EYE), enemy(2, HIGH_EYE)]
}

// Slot-to-target sight casts from each enemy's own eye: a low wall hides a
// slot from one enemy and not the other, and the batch must keep that apart.
#[test]
fn eye_offsets_on_one_target_keep_their_own_line_of_sight() {
    let rig = closed_door();
    let world = low_wall_world();
    // North of the wall; its south ring slot sits behind it.
    let target = Vec3::new(5.0, REST_Y, 4.5);
    let [low, high] = eye_pair(target);
    let behind_wall = target - Vec3::Z * 3.0;
    let mut scratch = NavScratch::new();
    for (enemy, sees) in [(low, false), (high, true)] {
        let others = [];
        let solo = [query(
            &enemy,
            &others,
            &rig,
            &world,
            crate::nav::MoverNav::none(),
        )];
        let candidates = crate::combat_positioning::combat_candidates(&solo[0]);
        assert_eq!(
            candidates
                .iter()
                .any(|candidate| candidate.position.z < 2.5),
            sees,
            "eye {}: slot behind the wall {behind_wall:?}",
            enemy.eye
        );
    }
    let others = snapshots(&[low, high]);
    let queries =
        [low, high].map(|enemy| query(&enemy, &others, &rig, &world, crate::nav::MoverNav::none()));
    assert_batch_matches_uncached(&queries, &mut scratch, "two eyes, one target");
}

// Standoffs 3 and 4 put the 4 m ring's 0.75 slot on the 3 m ring's 1.0 slot:
// one position, two generation indices.
#[test]
fn coinciding_positions_across_standoffs_keep_their_generation_indices() {
    let rig = closed_door();
    let world = low_wall_world();
    let target = Vec3::new(15.0, REST_Y, 5.0);
    let enemies: Vec<Enemy> = [3.0, 4.0, 3.0, 4.0]
        .into_iter()
        .enumerate()
        .map(|(i, standoff)| Enemy {
            claimant: 1 + i as u32,
            position: Vec3::new(13.0 + i as f32, REST_Y, 1.5),
            target,
            eye: HIGH_EYE,
            standoff,
            slot: None,
            ring: None,
        })
        .collect();
    let others = snapshots(&enemies);
    let queries: Vec<CombatQuery<'_>> = enemies
        .iter()
        .map(|enemy| query(enemy, &others, &rig, &world, crate::nav::MoverNav::none()))
        .collect();
    let three = crate::combat_positioning::combat_candidates(&queries[0]);
    let four = crate::combat_positioning::combat_candidates(&queries[1]);
    assert!(
        three.iter().any(|a| four
            .iter()
            .any(|b| a.position == b.position && a.generation_index != b.generation_index)),
        "the rings share a position under different generation indices"
    );
    assert_batch_matches_uncached(&queries, &mut NavScratch::new(), "coinciding standoffs");
}

// Nothing carries between batches: the door opens for the first batch, then
// docks shut over the doorway slots and closes the gate before the second.
#[test]
fn a_batch_after_the_door_closes_matches_the_uncached_batch() {
    let mut rig = closed_door();
    rig.command(MoverCommand::Start);
    for _ in 0..80 {
        rig.tick();
    }
    assert!(!rig.gate().portal_closed(&rig.graph, 0), "the door is open");
    let world = low_wall_world();
    let target = Vec3::new(11.5, REST_Y, 5.0);
    let enemies: Vec<Enemy> = (0..6)
        .map(|i| Enemy {
            claimant: 1 + i,
            position: Vec3::new(3.0 + i as f32, REST_Y, 7.0),
            target,
            eye: HIGH_EYE,
            standoff: 1.0,
            slot: None,
            ring: None,
        })
        .collect();
    let others = snapshots(&enemies);
    let mut scratch = NavScratch::new();
    let in_doorway = |assignments: &[CombatAssignment]| {
        assignments.iter().any(|assignment| {
            assignment
                .candidate
                .is_some_and(|candidate| (9.5..11.5).contains(&candidate.position.x))
        })
    };

    let open = {
        let mover_nav = crate::nav::MoverNav {
            gate: rig.gate(),
            movers: &rig.colliders,
            poses: &rig.movers,
        };
        let queries: Vec<CombatQuery<'_>> = enemies
            .iter()
            .map(|enemy| query(enemy, &others, &rig, &world, mover_nav))
            .collect();
        assert_batch_matches_uncached(&queries, &mut scratch, "door open")
    };
    assert!(in_doorway(&open), "an open doorway holds a slot: {open:?}");

    rig.command(MoverCommand::Reverse);
    for _ in 0..120 {
        rig.tick();
    }
    assert!(
        rig.gate().portal_closed(&rig.graph, 0),
        "the door docked shut"
    );
    let mover_nav = crate::nav::MoverNav {
        gate: rig.gate(),
        movers: &rig.colliders,
        poses: &rig.movers,
    };
    let queries: Vec<CombatQuery<'_>> = enemies
        .iter()
        .map(|enemy| query(enemy, &others, &rig, &world, mover_nav))
        .collect();
    let closed = assert_batch_matches_uncached(&queries, &mut scratch, "door closed");
    assert!(
        !in_doorway(&closed),
        "the docked door holds none: {closed:?}"
    );
}

fn work_during(batch: impl FnOnce()) -> crate::combat_positioning::PlacementWork {
    use crate::combat_positioning::{PLACEMENT_WORK, PlacementWork};
    PLACEMENT_WORK.with(|work| work.set(PlacementWork::default()));
    batch();
    PLACEMENT_WORK.with(|work| work.get())
}

/// The shared ring's generated positions: 8 directions at 3 radii.
const RING_POSITIONS: usize = 24;

// A fresh chase batch on one target and standoff grounds, tests against
// docked movers, and routes to the target once per candidate position, for
// one enemy or sixteen.
#[test]
fn a_chase_batch_does_target_side_placement_work_once_per_position() {
    let mut rig = closed_door();
    rig.command(MoverCommand::Start);
    for _ in 0..80 {
        rig.tick();
    }
    let world = low_wall_world();
    let target = Vec3::new(16.0, REST_Y, 5.0);
    let mut counts = Vec::new();
    for enemies in [1usize, 16] {
        // West of the door, clear of the ring.
        let enemies: Vec<Enemy> = (0..enemies)
            .map(|i| Enemy {
                claimant: 1 + i as u32,
                position: Vec3::new(1.0 + (i % 8) as f32, REST_Y, 7.5 + (i / 8) as f32),
                target,
                eye: HIGH_EYE,
                standoff: 3.0,
                slot: None,
                ring: None,
            })
            .collect();
        let others: Vec<CombatAgentSnapshot> = enemies
            .iter()
            .map(|enemy| CombatAgentSnapshot {
                claimant_id: enemy.claimant,
                position: enemy.position,
            })
            .collect();
        let mover_nav = crate::nav::MoverNav {
            gate: rig.gate(),
            movers: &rig.colliders,
            poses: &rig.movers,
        };
        let queries: Vec<CombatQuery<'_>> = enemies
            .iter()
            .map(|enemy| query(enemy, &others, &rig, &world, mover_nav))
            .collect();
        let mut scratch = NavScratch::new();
        let mut assignments = Vec::new();
        let work = work_during(|| {
            assignments = select_combat_positions_batch_in(&queries, &mut scratch);
        });
        assert!(
            assignments
                .iter()
                .all(|assignment| assignment.candidate.is_some()),
            "every enemy gets a slot: {assignments:?}"
        );
        for (label, count) in [
            ("groundings", work.groundings),
            ("docked-mover tests", work.dock_tests),
            ("target routes", work.target_routes),
        ] {
            assert!(
                (1..=RING_POSITIONS).contains(&count),
                "{label}: {count} for {} enemies",
                enemies.len()
            );
        }
        counts.push(work.groundings);
    }
    assert_eq!(counts[0], counts[1], "sixteen enemies ground what one does");
}

// P4: in a batch of 128 agents, a clearance test examines the agents in the
// cells its spacing spans, never every agent.
#[test]
fn clearance_in_a_large_batch_examines_only_nearby_agents() {
    let rig = closed_door();
    let world = low_wall_world();
    let target = Vec3::new(5.0, REST_Y, 7.0);
    // A 16 × 8 block 0.6 m apart, south of the target; its ring overlaps it.
    let enemies: Vec<Enemy> = (0..128)
        .map(|i| Enemy {
            claimant: 1 + i as u32,
            position: Vec3::new(
                0.5 + 0.6 * (i % 16) as f32,
                REST_Y,
                0.5 + 0.6 * (i / 16) as f32,
            ),
            target,
            eye: HIGH_EYE,
            standoff: 2.0,
            slot: None,
            ring: None,
        })
        .collect();
    let others: Vec<CombatAgentSnapshot> = enemies
        .iter()
        .map(|enemy| CombatAgentSnapshot {
            claimant_id: enemy.claimant,
            position: enemy.position,
        })
        .collect();
    let queries: Vec<CombatQuery<'_>> = enemies
        .iter()
        .map(|enemy| query(enemy, &others, &rig, &world, crate::nav::MoverNav::none()))
        .collect();
    assert_batch_matches_uncached(&queries, &mut NavScratch::new(), "128 agents");
    let live = work_during(|| {
        select_combat_positions_batch_in(&queries, &mut NavScratch::new());
    });
    assert!(live.clearance_tests > 0);
    // Cells span 2 × spacing + 2 cells (3.5 m) each way: at 0.6 m apart at
    // most 6 × 6 agents fall inside.
    assert!(
        (1..=36).contains(&live.most_peers_per_test),
        "most agents one test examined: {}",
        live.most_peers_per_test
    );
}

// One slot on two targets' rings: (8, 5) is 3 m east of a west-room target
// and 4 m west of an east-room one. With the door shut, only the first has a
// route from it, so the shared work must keep each target's own verdict.
#[test]
fn a_slot_on_two_targets_rings_keeps_each_targets_route_verdict() {
    let mut rig = closed_door();
    rig.tick();
    let world = low_wall_world();
    let west = Vec3::new(5.0, REST_Y, 5.0);
    let east = Vec3::new(12.0, REST_Y, 5.0);
    let shared_slot = |candidate: &crate::combat_positioning::CombatCandidate| {
        (candidate.position.x - 8.0).abs() < 1.0e-4 && (candidate.position.z - 5.0).abs() < 1.0e-4
    };
    let enemies = [(west, 3.0), (east, 4.0)].map(|(target, standoff)| Enemy {
        claimant: if target == west { 1 } else { 2 },
        position: Vec3::new(2.0, REST_Y, 8.0),
        target,
        eye: HIGH_EYE,
        standoff,
        slot: None,
        ring: None,
    });
    // No blocker by the slot: `snapshots` places one there on purpose.
    let others = enemies.map(|enemy| CombatAgentSnapshot {
        claimant_id: enemy.claimant,
        position: enemy.position,
    });
    let mover_nav = crate::nav::MoverNav {
        gate: rig.gate(),
        movers: &rig.colliders,
        poses: &rig.movers,
    };
    let queries = enemies.map(|enemy| query(&enemy, &others, &rig, &world, mover_nav));
    let alone = |query: &CombatQuery<'_>| {
        crate::combat_positioning::combat_candidates(query)
            .iter()
            .any(shared_slot)
    };
    assert!(alone(&queries[0]), "the west target can use the slot");
    assert!(
        !alone(&queries[1]),
        "the east target cannot, behind the shut door"
    );
    assert_batch_matches_uncached(&queries, &mut NavScratch::new(), "one slot, two targets");
    let reversed = [queries[1], queries[0]];
    assert_batch_matches_uncached(&reversed, &mut NavScratch::new(), "the other order");
}

// A batch whose queries read different worlds shares no placement work: one
// sees the door open (gate open, leaf posed nowhere) and one shut, then each
// carries its own agent slice. Sharing (8, 5)'s route verdict to the east
// target across them would hand the shut-door query the open door's answer.
// Slot sight sees the leaf at its pose, so the open world must not pose it.
#[test]
fn queries_reading_different_worlds_do_not_share_placement_work() {
    let mut rig = closed_door();
    rig.tick();
    let world = low_wall_world();
    let target = Vec3::new(12.0, REST_Y, 5.0);
    let enemies = [1, 2].map(|claimant| Enemy {
        claimant,
        position: Vec3::new(2.0, REST_Y, 8.0 - claimant as f32),
        target,
        eye: HIGH_EYE,
        standoff: 4.0,
        slot: None,
        ring: None,
    });
    let others = enemies.map(|enemy| CombatAgentSnapshot {
        claimant_id: enemy.claimant,
        position: enemy.position,
    });
    let open = crate::nav::MoverNav {
        gate: crate::nav::NavGate::OPEN,
        movers: &rig.colliders,
        poses: &crate::collision::moving::EMPTY_MOVER_POSES,
    };
    let shut = crate::nav::MoverNav {
        gate: rig.gate(),
        movers: &rig.colliders,
        poses: &rig.movers,
    };
    let queries = [
        query(&enemies[0], &others, &rig, &world, open),
        query(&enemies[1], &others, &rig, &world, shut),
    ];
    assert!(!crate::combat_positioning::shares_placement_work(&queries));
    let assignments = assert_batch_matches_uncached(&queries, &mut NavScratch::new(), "two gates");
    let west_slot = |assignment: &CombatAssignment| {
        assignment
            .candidate
            .is_some_and(|candidate| candidate.position.x < 10.0)
    };
    assert!(
        west_slot(&assignments[0]),
        "the open door lets a west slot reach the target"
    );
    assert!(!west_slot(&assignments[1]), "the shut door does not");

    // One world, but each query carries its own agent slice.
    let own = [[others[1]], [others[0]]];
    let queries = [
        query(&enemies[0], &own[0], &rig, &world, open),
        query(&enemies[1], &own[1], &rig, &world, open),
    ];
    assert!(crate::combat_positioning::shares_placement_work(&queries));
    assert_batch_matches_uncached(&queries, &mut NavScratch::new(), "own agent slices");
}
