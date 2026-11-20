// Route queries with reused scratch against the pre-scratch search, which
// allocates per query: same path, cost and failure on a 4,216-region level with
// stacked floors, a gated corridor and a lift; the P5 orderings; and a search
// that allocates nothing and resets only what it touches.
// See: context/lib/build_pipeline.md §Navigation bake

use glam::Vec3;
use postretro_foundation::GroundRef;
use postretro_level_format::navmesh::{NavMoverGate, NavMoverLink, NavPortal, NavRegion};

use super::*;
use crate::alloc_probe::AllocSnapshot;
use crate::kinematic_mover::MoverDockTable;
use crate::nav::arena_fixture::{multi_arena_map, tile_grid_section};
use crate::nav::{NavGate, NavGraph, NavMoverBound};

const LIFT: u32 = 7;
const DOOR: u32 = 9;
const BALCONY_Y: f32 = 3.0;
/// Standing centre height above a floor.
const STAND: f32 = 0.9 + SKIN_DISTANCE;

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

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next() * (hi - lo)
    }
}

fn tile_region(regions: &[NavRegion], x0: u32, z0: u32) -> u32 {
    regions
        .iter()
        .position(|region| region.x0 == x0 && region.z0 == z0 && region.floor_y_max < 1.0)
        .expect("tile exists") as u32
}

/// The multi-arena level plus: a balcony (two regions at 3 m) stacked over
/// arena 1, reached by a lift from the floor at x = 120; and a door gating
/// the corridor from arena 1 to arena 2.
fn featured_level() -> NavGraph {
    let mut section = multi_arena_map().section;
    let landing = tile_region(&section.regions, 118, 10);
    let balcony = section.regions.len() as u32;
    let balcony_region = |x0, x1| NavRegion {
        x0,
        z0: 8,
        x1,
        z1: 16,
        floor_y_min: BALCONY_Y,
        floor_y_max: BALCONY_Y,
    };
    section.regions.push(balcony_region(124, 132));
    section.regions.push(balcony_region(132, 140));
    section.portals.push(NavPortal {
        region_a: balcony,
        region_b: balcony + 1,
        left: [132.0, BALCONY_Y, 16.0],
        right: [132.0, BALCONY_Y, 8.0],
    });
    section.links = vec![
        NavMoverLink {
            mover_id: LIFT,
            waypoint: 0,
            region: landing,
            left: [120.0, 0.0, 10.0],
            right: [120.0, 0.0, 12.0],
        },
        NavMoverLink {
            mover_id: LIFT,
            waypoint: 1,
            region: balcony,
            left: [124.0, BALCONY_Y, 12.0],
            right: [124.0, BALCONY_Y, 10.0],
        },
    ];
    section.gates = section
        .portals
        .iter()
        .enumerate()
        .filter(|(_, portal)| portal.left[0] == 168.0 && portal.right[0] == 168.0)
        .map(|(portal, _)| NavMoverGate {
            mover_id: DOOR,
            waypoint: 0,
            portal: portal as u32,
        })
        .collect();
    assert_eq!(section.gates.len(), 2, "the corridor is two tiles wide");
    let mut graph = NavGraph::from_section(&section);
    graph.validate_mover_records(&[
        NavMoverBound {
            mover_id: LIFT,
            waypoint_heights: vec![0.0, BALCONY_Y],
        },
        NavMoverBound {
            mover_id: DOOR,
            waypoint_heights: vec![0.0, 4.0],
        },
    ]);
    graph
}

fn mover(id: u32, rise: f32, started: bool) -> postretro_entities::KinematicMoverComponent {
    postretro_entities::KinematicMoverComponent::new(
        id,
        postretro_entities::KinematicMoverConfig {
            waypoints: vec![Vec3::ZERO, Vec3::Y * rise],
            waypoint_names: vec!["low".into(), "high".into()],
            speed_mps: 2.0,
            wait_ms: 500.0,
            mode: postretro_entities::KinematicMoverMode::PingPong,
            started,
            spin_axis: Vec3::ZERO,
            initial_spin_rate_rad_s: 0.0,
            spin_accel_rad_s2: 0.0,
            carry_yaw: false,
        },
    )
}

/// The lift rests at its low dock and will leave; with `door_closed` the door
/// is watched and docked shut, otherwise it is not watched and stays open.
fn docks(door_closed: bool) -> MoverDockTable {
    let mut movers = crate::kinematic_mover::MoverTickStateTable::default();
    let gated: &[(u32, u32)] = if door_closed { &[(DOOR, 0)] } else { &[] };
    movers.watch_mover_docks(gated, &[(LIFT, 0), (LIFT, 1)]);
    let mut docks = movers.docks().clone();
    docks.update(&mover(LIFT, BALCONY_Y, true));
    if door_closed {
        docks.update(&mover(DOOR, 4.0, false));
    }
    docks
}

fn floor(x: f32, z: f32) -> Vec3 {
    Vec3::new(x, STAND, z)
}

fn on_ground(position: Vec3) -> NavStart {
    NavStart {
        position,
        ground: GroundRef::World,
        height: 1.8,
    }
}

/// A random endpoint: mostly on the floor, some on the balcony, some off the
/// mesh within and beyond snap tolerance of an arena wall.
fn endpoint(rng: &mut Lcg) -> Vec3 {
    match (rng.next() * 10.0) as u32 {
        0 => Vec3::new(
            rng.range(124.5, 139.5),
            BALCONY_Y + STAND,
            rng.range(8.5, 15.5),
        ),
        1 => floor(rng.range(-3.0, 0.0), rng.range(1.0, 63.0)),
        2 => floor(rng.range(1.0, 63.0), rng.range(64.0, 67.0)),
        _ => floor(rng.range(0.5, 375.5), rng.range(0.5, 63.5)),
    }
}

fn assert_scratch_matches_fresh(
    scratch: &mut NavScratch,
    graph: &NavGraph,
    gate: NavGate<'_>,
    from: NavStart,
    goal: Vec3,
) -> Option<NavPath> {
    let reused = find_path_from_in(scratch, graph, gate, from, goal);
    let fresh = find_path_from_fresh(graph, gate, from, goal);
    assert_eq!(reused, fresh, "from {from:?} to {goal:?}");
    reused
}

#[test]
fn reused_scratch_routes_like_fresh_allocation_on_a_featured_large_level() {
    let graph = featured_level();
    let closed = docks(true);
    let open = docks(false);
    let gates = [NavGate::new(&closed), NavGate::new(&open), NavGate::OPEN];
    let door_portal = graph.gates()[0].portal as usize;
    assert!(
        gates[0].portal_closed(&graph, door_portal),
        "the door docks shut"
    );
    assert!(
        !gates[1].portal_closed(&graph, door_portal),
        "the door is open"
    );

    let mut scratch = NavScratch::new();
    let mut rng = Lcg(11);
    let mut outcomes = [0usize; 3];
    for i in 0..1_500 {
        let gate = gates[i % gates.len()];
        let start = endpoint(&mut rng);
        let goal = endpoint(&mut rng);
        let from = if i % 7 == 0 {
            // Aboard the lift at its low dock: the search leaves by its links.
            NavStart {
                position: Vec3::new(122.0, STAND, 11.0),
                ground: GroundRef::Mover(LIFT),
                height: 1.8,
            }
        } else {
            on_ground(start)
        };
        let path = assert_scratch_matches_fresh(&mut scratch, &graph, gate, from, goal);
        outcomes[match path {
            None => 0,
            Some(ref path) if path.ride().is_some() => 1,
            Some(_) => 2,
        }] += 1;
    }
    let [failed, rides, walks] = outcomes;
    assert!(failed > 50, "some queries fail: {outcomes:?}");
    assert!(rides > 10, "some queries end in a ride: {outcomes:?}");
    assert!(walks > 500, "most queries walk: {outcomes:?}");

    // A query straight after one that failed part way: the closed door ends
    // the first search after it explored arenas 0 and 1.
    let gate = gates[0];
    let failed = find_path_from_in(
        &mut scratch,
        &graph,
        gate,
        on_ground(floor(30.0, 30.0)),
        floor(250.0, 30.0),
    );
    assert!(failed.is_none(), "the door cuts arena 2 off");
    let after = assert_scratch_matches_fresh(
        &mut scratch,
        &graph,
        gate,
        on_ground(floor(110.0, 20.0)),
        floor(20.0, 40.0),
    );
    assert!(after.is_some());
}

// P5(a): query 1 reaches the lift's boarding node through a portal and returns
// with its frontier non-empty; query 2 seeds that node straight from its start
// region, writing no predecessor. A reset of `g` alone would leave query 1's
// predecessor there and splice its corridor onto query 2's.
#[test]
fn a_boarding_node_seeded_from_the_start_region_forgets_the_previous_predecessor() {
    let graph = featured_level();
    let docks = docks(false);
    let gate = NavGate::new(&docks);
    let link_base = graph.portals().len() * 2;
    let board = link_base; // link 0's boarding node

    let mut scratch = NavScratch::new();
    let first = find_path_from_in(
        &mut scratch,
        &graph,
        gate,
        on_ground(floor(110.0, 11.0)),
        floor(160.0, 11.0),
    );
    assert!(first.is_some());
    assert!(
        scratch.came_from(board).is_some(),
        "query 1 reached the boarding node through a portal"
    );
    assert!(
        !scratch.open.is_empty(),
        "query 1 returned with frontier left"
    );

    let landing = floor(119.0, 11.0);
    let goal = Vec3::new(130.0, BALCONY_Y + STAND, 12.0);
    let second = assert_scratch_matches_fresh(&mut scratch, &graph, gate, on_ground(landing), goal)
        .expect("the lift carries it up");
    assert!(
        second.ride().is_some(),
        "the route ends in a ride: {second:?}"
    );
}

// P5(b): stamps written before the generation wraps must not read as current
// after it. Query A runs west from arena 2 and leaves small `g` on the nodes
// near its start; after the wrap, query C runs east through those same nodes,
// where a stale small `g` would refuse every improvement.
#[test]
fn a_wrapping_generation_clears_every_stamp() {
    let graph = featured_level();
    let docks = docks(false);
    let gate = NavGate::new(&docks);
    let mut scratch = NavScratch::new();
    // Generation 1.
    assert_scratch_matches_fresh(
        &mut scratch,
        &graph,
        gate,
        on_ground(floor(220.0, 31.0)),
        floor(30.0, 31.0),
    )
    .expect("arena 2 reaches arena 0");
    scratch.set_generation_for_test(u32::MAX - 1);
    // Generation u32::MAX, far from both routes.
    assert_scratch_matches_fresh(
        &mut scratch,
        &graph,
        gate,
        on_ground(floor(60.0, 60.0)),
        floor(62.0, 60.0),
    );
    // After the wrap.
    assert_scratch_matches_fresh(
        &mut scratch,
        &graph,
        gate,
        on_ground(floor(10.0, 31.0)),
        floor(350.0, 31.0),
    )
    .expect("arena 0 reaches arena 3");
    assert_eq!(scratch.generation_for_test(), 1);
}

/// A 3 × 3 block of tiles plus one more: ten regions, the centre tile with
/// four neighbours, as an interior arena tile has.
fn ten_region_graph() -> NavGraph {
    let graph = NavGraph::from_section(&tile_grid_section(4, 3, |tx, tz| {
        tx < 3 || (tx == 3 && tz == 1)
    }));
    assert_eq!(graph.regions().len(), 10);
    graph
}

// P5(c): one scratch serves the large level, then a ten-region graph, then
// the large level again.
#[test]
fn one_scratch_serves_graphs_of_different_sizes_in_turn() {
    let large = featured_level();
    let small = ten_region_graph();
    let mut scratch = NavScratch::new();
    let mut rng = Lcg(5);
    for graph in [&large, &small, &large, &small] {
        for _ in 0..200 {
            let (start, goal) = if graph.regions().len() == 10 {
                (
                    floor(rng.range(0.1, 7.9), rng.range(0.1, 5.9)),
                    floor(rng.range(0.1, 7.9), rng.range(0.1, 5.9)),
                )
            } else {
                (endpoint(&mut rng), endpoint(&mut rng))
            };
            assert_scratch_matches_fresh(
                &mut scratch,
                graph,
                NavGate::OPEN,
                on_ground(start),
                goal,
            );
        }
    }
}

/// Run only the search step of a route between two points of `graph`.
fn search(scratch: &mut NavScratch, graph: &NavGraph, start: Vec3, goal: Vec3) -> Option<usize> {
    let gate = NavGate::OPEN;
    let goal_region = graph.resolve_region_at(goal).expect("goal on the mesh");
    let search_start = search_start(graph, gate, on_ground(start)).expect("start on the mesh");
    astar_search(scratch, graph, gate, start, goal, search_start, goal_region)
}

// Once warm, a search between adjacent regions allocates nothing, and the
// per-node state it resets is what it touches, not the graph's size.
#[test]
fn a_warm_adjacent_region_search_allocates_nothing_and_resets_only_what_it_touches() {
    let large = NavGraph::from_section(&multi_arena_map().section);
    let small = ten_region_graph();
    // An interior tile and its east neighbour: tiles (1, 1) → (2, 1) of the
    // small graph, and an arena-0 tile pair with the same four-portal shape.
    let route = |offset: f32| (floor(offset + 3.0, 3.0), floor(offset + 5.0, 3.0));

    let mut resets = Vec::new();
    for (graph, offset) in [(&large, 20.0), (&small, 0.0)] {
        let (start, goal) = route(offset);
        let mut scratch = NavScratch::new();
        // Warm-up: grows the scratch to this graph.
        assert!(search(&mut scratch, graph, start, goal).is_some());
        let snapshot = AllocSnapshot::arm();
        let end = search(&mut scratch, graph, start, goal);
        let allocations = snapshot.allocs_since();
        assert!(end.is_some());
        assert_eq!(
            allocations,
            0,
            "warm search allocated on {} regions",
            graph.regions().len()
        );
        resets.push(scratch.resets);
    }
    assert_eq!(
        resets[0], resets[1],
        "the 4,216-region search resets what the 10-region one does"
    );
    assert!(
        resets[0] <= 8,
        "a two-region route touches its seeds only: {resets:?}"
    );
}
