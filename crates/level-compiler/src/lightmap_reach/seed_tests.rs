// Reach flood seeding: an emitter sphere straddling a portal seeds the open
// leaves on both sides, whatever R allows the flood to cross.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use glam::DVec3;

use super::closed_leaves;
use super::flood::{Emitter, PortalGraph, ball_flood};
use super::test_fixtures::{ReachFixture, Wall};
use crate::partition::find_leaf_for_point;

/// Twice the polygon's vector area: its normal times twice its area.
fn twice_area(polygon: &[DVec3]) -> DVec3 {
    (0..polygon.len())
        .map(|i| polygon[i].cross(polygon[(i + 1) % polygon.len()]))
        .sum()
}

// The largest portal between two open leaves of the doorway fixture, with a
// light 5 cm off it on one side and an R of 1 cm, so the flood crosses no
// portal by distance: a 10 cm emitter sphere seeds both leaves; a 2 cm one
// only its own.
#[test]
fn reach_flood_seeds_both_leaves_an_emitter_sphere_straddles() {
    let fixture = ReachFixture::two_rooms(Wall::doorway());
    let tree = fixture.tree();
    let closed = closed_leaves(tree, fixture.exterior());
    let graph = PortalGraph::new(tree);
    let portal = graph
        .portals()
        .iter()
        .filter(|portal| !closed[portal.front_leaf] && !closed[portal.back_leaf])
        .max_by(|a, b| {
            twice_area(&a.polygon)
                .length()
                .total_cmp(&twice_area(&b.polygon).length())
        })
        .expect("an open portal");
    let normal = twice_area(&portal.polygon).normalize();
    let centroid = portal.polygon.iter().sum::<DVec3>() / portal.polygon.len() as f64;
    let mut centre = centroid + normal * 0.05;
    let mut far = portal.back_leaf;
    if find_leaf_for_point(tree, centre) != portal.front_leaf {
        centre = centroid - normal * 0.05;
        far = portal.front_leaf;
    }
    let near = find_leaf_for_point(tree, centre);
    assert!(near != far && [portal.front_leaf, portal.back_leaf].contains(&near));

    let flood = |size: f64| {
        ball_flood(tree, &closed, &graph, Emitter { centre, size }, 0.01)
            .expect("the centre's leaf is open")
    };
    let straddling = flood(0.1);
    assert!(straddling.contains(near) && straddling.contains(far));
    let clear = flood(0.02);
    assert!(clear.contains(near) && !clear.contains(far));
}
