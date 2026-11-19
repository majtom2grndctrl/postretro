// Test-only reference: the A* search exactly as it ran before route scratch,
// allocating and filling graph-sized arrays per query. Route queries with a
// reused `NavScratch` must return what this returns.
// See: context/lib/build_pipeline.md §Navigation bake

use std::collections::BinaryHeap;

use glam::Vec3;

use super::{CorridorHop, Frontier, RIDE_COST, SearchStart, link_midpoint, portal_midpoint};
use crate::nav::{NavGate, NavGraph, distance_xz};

pub(super) fn astar_corridor(
    graph: &NavGraph,
    gate: NavGate<'_>,
    start: Vec3,
    goal: Vec3,
    search_start: SearchStart,
    goal_region: usize,
) -> Option<Vec<CorridorHop>> {
    let portals = graph.portals();
    let links = graph.links();
    // Node ids: `2 * portal + dir`, then per link `board` and `exit`.
    let link_base = portals.len() * 2;
    let board_node = |link: usize| link_base + 2 * link;
    let exit_node = |link: usize| link_base + 2 * link + 1;
    let link_of = |node: usize| (node - link_base) / 2;
    let is_board = |node: usize| node >= link_base && (node - link_base).is_multiple_of(2);
    let link_mid = |link: usize| link_midpoint(&links[link]);
    let mid = |node: usize| {
        if node < link_base {
            portal_midpoint(&portals[node / 2])
        } else {
            link_mid(link_of(node))
        }
    };
    // The region a node arrives in; a boarding node arrives on a mover.
    let arrived_region = |node: usize| -> Option<usize> {
        if node >= link_base {
            return (!is_board(node)).then(|| links[link_of(node)].region as usize);
        }
        let p = &portals[node / 2];
        Some(if node.is_multiple_of(2) {
            p.region_b as usize
        } else {
            p.region_a as usize
        })
    };
    let from_region = |node: usize| -> usize {
        let p = &portals[node / 2];
        if node.is_multiple_of(2) {
            p.region_a as usize
        } else {
            p.region_b as usize
        }
    };
    // The directed node that leaves `region` through portal `portal_index`, or
    // `None` when the portal does not touch `region`.
    let directed_leaving = |portal_index: usize, region: usize| -> Option<usize> {
        let p = &portals[portal_index];
        if p.region_a as usize == region {
            Some(2 * portal_index)
        } else if p.region_b as usize == region {
            Some(2 * portal_index + 1)
        } else {
            None
        }
    };
    let surface_tolerance = graph.agent.step_height.max(0.01);
    // A link of the mover a body stands on at `dock`, its feet at world height
    // `foot`: its cost to reach from there, and the dock it rides from when it
    // is not this one. A ride costs its standing price plus the climb, so a
    // search that starts on the lift prices it as one that boards it does.
    let leave_mover = |exit: usize, dock: u32, foot: f32| -> Option<(f32, Option<u32>)> {
        let link = &links[exit];
        let surface = graph.mover_surface(link.mover_id, dock, foot);
        if (graph.link_surface(exit) - surface).abs() > surface_tolerance {
            return None;
        }
        if link.waypoint == dock {
            return Some((0.0, None));
        }
        let rides = gate.rests_at_stops(link.mover_id)
            && gate.moves_on_its_own(link.mover_id)
            && graph.is_terminus(link.mover_id, dock)
            && graph.is_terminus(link.mover_id, link.waypoint);
        let rise = (link.left[1] - foot).abs();
        rides.then_some((RIDE_COST + rise, Some(dock)))
    };

    let node_count = link_base + 2 * links.len();
    let mut g_score = vec![f32::INFINITY; node_count];
    // `came_from[node] = Some(previous_node)`, or `None` when reached from the
    // virtual start (the first crossing out of `start_region`).
    let mut came_from: Vec<Option<usize>> = vec![None; node_count];
    // The dock a mover carries the body from before this exit node, if any.
    let mut rode_from: Vec<Option<u32>> = vec![None; links.len()];
    let mut open = BinaryHeap::new();

    match search_start {
        SearchStart::Region(start_region) => {
            // Seed: cross each portal of the start region, costed from the true start.
            for &portal_index in graph.region_portal_indices(start_region) {
                if gate.portal_closed(graph, portal_index) {
                    continue;
                }
                let Some(node) = directed_leaving(portal_index, start_region) else {
                    continue;
                };
                let g = distance_xz(start, mid(node));
                if g < g_score[node] {
                    g_score[node] = g;
                    came_from[node] = None;
                    open.push(Frontier {
                        node,
                        f: g + distance_xz(mid(node), goal),
                    });
                }
            }
            for &link in graph.region_links(start_region) {
                if !gate.docked_at(links[link].mover_id, links[link].waypoint) {
                    continue;
                }
                let node = board_node(link);
                let g = distance_xz(start, link_mid(link));
                if g < g_score[node] {
                    g_score[node] = g;
                    open.push(Frontier {
                        node,
                        f: g + distance_xz(link_mid(link), goal),
                    });
                }
            }
        }
        SearchStart::Mover {
            mover_id,
            dock,
            foot,
        } => {
            for link in graph.mover_links(mover_id) {
                let Some((cost, ride)) = leave_mover(link, dock, foot) else {
                    continue;
                };
                let node = exit_node(link);
                let g = distance_xz(start, link_mid(link)) + cost;
                if g < g_score[node] {
                    g_score[node] = g;
                    rode_from[link] = ride;
                    open.push(Frontier {
                        node,
                        f: g + distance_xz(link_mid(link), goal),
                    });
                }
            }
        }
    }

    while let Some(Frontier { node, .. }) = open.pop() {
        let Some(region) = arrived_region(node) else {
            // Standing on a docked mover: every way off it is one step more.
            let boarded = link_of(node);
            let link = &links[boarded];
            for next_link in graph.mover_links(link.mover_id) {
                if next_link == boarded {
                    continue;
                }
                // The boarding edge lies on the surface the body stands on.
                let Some((cost, ride)) = leave_mover(next_link, link.waypoint, link.left[1]) else {
                    continue;
                };
                let next = exit_node(next_link);
                let tentative = g_score[node] + distance_xz(mid(node), link_mid(next_link)) + cost;
                if tentative < g_score[next] {
                    g_score[next] = tentative;
                    came_from[next] = Some(node);
                    rode_from[next_link] = ride;
                    open.push(Frontier {
                        node: next,
                        f: tentative + distance_xz(link_mid(next_link), goal),
                    });
                }
            }
            continue;
        };
        if region == goal_region {
            return Some(reconstruct(
                &came_from,
                &rode_from,
                link_base,
                &from_region,
                node,
            ));
        }

        for &portal_index in graph.region_portal_indices(region) {
            if node < link_base && portal_index == node / 2 {
                continue; // don't immediately re-cross the portal just crossed
            }
            if gate.portal_closed(graph, portal_index) {
                continue;
            }
            let Some(next) = directed_leaving(portal_index, region) else {
                continue;
            };
            let tentative = g_score[node] + distance_xz(mid(node), mid(next));
            if tentative < g_score[next] {
                g_score[next] = tentative;
                came_from[next] = Some(node);
                open.push(Frontier {
                    node: next,
                    f: tentative + distance_xz(mid(next), goal),
                });
            }
        }
        for &link in graph.region_links(region) {
            if !gate.docked_at(links[link].mover_id, links[link].waypoint) {
                continue;
            }
            let next = board_node(link);
            let tentative = g_score[node] + distance_xz(mid(node), link_mid(link));
            if tentative < g_score[next] {
                g_score[next] = tentative;
                came_from[next] = Some(node);
                open.push(Frontier {
                    node: next,
                    f: tentative + distance_xz(link_mid(link), goal),
                });
            }
        }
    }

    None
}

/// Walk `came_from` back from the closing crossing to build the forward-ordered
/// corridor.
fn reconstruct(
    came_from: &[Option<usize>],
    rode_from: &[Option<u32>],
    link_base: usize,
    from_region: &impl Fn(usize) -> usize,
    end_node: usize,
) -> Vec<CorridorHop> {
    let mut hops = Vec::new();
    let mut current = Some(end_node);
    while let Some(node) = current {
        hops.push(if node < link_base {
            CorridorHop::Portal {
                portal_index: node / 2,
                from_region: from_region(node),
            }
        } else {
            let link = (node - link_base) / 2;
            if (node - link_base).is_multiple_of(2) {
                CorridorHop::Board { link }
            } else {
                CorridorHop::Exit {
                    link,
                    ride_from: rode_from[link],
                }
            }
        });
        current = came_from[node];
    }
    hops.reverse();
    hops
}
