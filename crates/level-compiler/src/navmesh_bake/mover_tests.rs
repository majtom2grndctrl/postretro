// Mover bake tests: region splits, gates, links, determinism and the stage key.
// See: context/lib/build_pipeline.md §Navigation bake

use glam::DVec3;
use postretro_level_format::navmesh::{NavMeshSection, NavMoverGate, NavMoverLink};

use super::stage::stage_key;
use super::tests::{floor_quad, geo_from_triangles};
use super::{bake_navmesh, bake_navmesh_with_movers, bake_or_load_navmesh};
use crate::cache::StageCache;
use crate::geometry::GeometryResult;
use crate::map_data::{
    BrushPlane, BrushSide, BrushVolume, KinematicMoveMode, MapKinematicMover, MapKinematicWaypoint,
    NavParams, TextureProjection,
};
use crate::partition::Aabb;

/// An axis-aligned brush with all six textured sides, as parse emits one.
fn box_brush(min: DVec3, max: DVec3) -> BrushVolume {
    let side = |normal: DVec3, distance: f64, vertices: [DVec3; 4]| BrushSide {
        vertices: vertices.to_vec(),
        normal,
        distance,
        texture: String::new(),
        tex_projection: TextureProjection::default(),
    };
    let at = DVec3::new;
    let sides = vec![
        side(
            DVec3::Y,
            max.y,
            [
                at(min.x, max.y, min.z),
                at(min.x, max.y, max.z),
                at(max.x, max.y, max.z),
                at(max.x, max.y, min.z),
            ],
        ),
        side(
            DVec3::NEG_Y,
            -min.y,
            [
                at(min.x, min.y, min.z),
                at(max.x, min.y, min.z),
                at(max.x, min.y, max.z),
                at(min.x, min.y, max.z),
            ],
        ),
        side(
            DVec3::X,
            max.x,
            [
                at(max.x, min.y, min.z),
                at(max.x, max.y, min.z),
                at(max.x, max.y, max.z),
                at(max.x, min.y, max.z),
            ],
        ),
        side(
            DVec3::NEG_X,
            -min.x,
            [
                at(min.x, min.y, min.z),
                at(min.x, min.y, max.z),
                at(min.x, max.y, max.z),
                at(min.x, max.y, min.z),
            ],
        ),
        side(
            DVec3::Z,
            max.z,
            [
                at(min.x, min.y, max.z),
                at(max.x, min.y, max.z),
                at(max.x, max.y, max.z),
                at(min.x, max.y, max.z),
            ],
        ),
        side(
            DVec3::NEG_Z,
            -min.z,
            [
                at(min.x, min.y, min.z),
                at(min.x, max.y, min.z),
                at(max.x, max.y, min.z),
                at(max.x, min.y, min.z),
            ],
        ),
    ];
    let planes = [
        (DVec3::X, max.x),
        (DVec3::NEG_X, -min.x),
        (DVec3::Y, max.y),
        (DVec3::NEG_Y, -min.y),
        (DVec3::Z, max.z),
        (DVec3::NEG_Z, -min.z),
    ];
    BrushVolume {
        planes: planes
            .into_iter()
            .map(|(normal, distance)| BrushPlane { normal, distance })
            .collect(),
        sides,
        aabb: Aabb { min, max },
    }
}

fn mover(mover_id: u32, path: &str, brushes: Vec<BrushVolume>) -> MapKinematicMover {
    MapKinematicMover {
        mover_id,
        name: format!("mover_{mover_id}"),
        tags: Vec::new(),
        origin: DVec3::ZERO,
        authored_origin: None,
        path: path.to_string(),
        speed: 1.0,
        wait_ms: 0.0,
        spin_axis: [0.0; 3],
        spin_speed_deg_s: 0.0,
        spin_accel_deg_s2: 0.0,
        carry_yaw: false,
        block_policy: "displace".to_string(),
        crush_damage: 0.0,
        crush_interval_ms: 0.0,
        auto_close_ms: None,
        open_event: None,
        close_event: None,
        blocked_event: None,
        crush_event: None,
        open_sound: None,
        close_sound: None,
        blocked_sound: None,
        crush_sound: None,
        crush_damage_type: None,
        light_ramp_start: 0.0,
        light_ramp_end: 1.0,
        move_mode: KinematicMoveMode::PingPong,
        start_on_spawn: false,
        brush_volumes: brushes,
    }
}

fn waypoint(name: &str, next: &str, origin: DVec3) -> MapKinematicWaypoint {
    MapKinematicWaypoint {
        name: name.to_string(),
        next: next.to_string(),
        origin,
    }
}

/// Engine default agent: radius 0.4, height 1.8, step 0.5, cell 0.25.
fn params() -> NavParams {
    NavParams::default()
}

fn bake(
    geo: &GeometryResult,
    movers: &[MapKinematicMover],
    waypoints: &[MapKinematicWaypoint],
) -> NavMeshSection {
    bake_navmesh_with_movers(geo, &params(), movers, waypoints).expect("fixture must bake")
}

/// The region on floor level `floor` covering world `(x, z)`.
fn region_at(section: &NavMeshSection, x: f32, z: f32, floor: f32) -> u32 {
    let cell_x = (x - section.origin[0]) / section.cell_size;
    let cell_z = (z - section.origin[2]) / section.cell_size;
    section
        .regions
        .iter()
        .position(|r| {
            r.x0 as f32 <= cell_x
                && cell_x < r.x1 as f32
                && r.z0 as f32 <= cell_z
                && cell_z < r.z1 as f32
                && (r.floor_y_min - floor).abs() < 0.01
        })
        .expect("a region covers the point") as u32
}

/// The constant-X portal at `x` whose span holds `z`.
fn portal_at(section: &NavMeshSection, x: f32, z: f32) -> u32 {
    section
        .portals
        .iter()
        .position(|p| {
            (p.left[0] - x).abs() < 1.0e-3
                && (p.right[0] - x).abs() < 1.0e-3
                && p.left[2].min(p.right[2]) <= z
                && z <= p.left[2].max(p.right[2])
        })
        .expect("a portal lies on the line") as u32
}

/// The portals gated at `waypoint` of mover 0.
fn gated_at(section: &NavMeshSection, waypoint: u32) -> Vec<u32> {
    section
        .gates
        .iter()
        .filter(|gate| gate.mover_id == 0 && gate.waypoint == waypoint)
        .map(|gate| gate.portal)
        .collect()
}

/// Whether `to` is reachable from `from` with the `closed` portals shut.
fn reaches(section: &NavMeshSection, from: u32, to: u32, closed: &[u32]) -> bool {
    let mut seen = vec![false; section.regions.len()];
    seen[from as usize] = true;
    let mut pending = vec![from];
    while let Some(region) = pending.pop() {
        for (index, portal) in section.portals.iter().enumerate() {
            if closed.contains(&(index as u32)) {
                continue;
            }
            let next = if portal.region_a == region {
                portal.region_b
            } else if portal.region_b == region {
                portal.region_a
            } else {
                continue;
            };
            if !seen[next as usize] {
                seen[next as usize] = true;
                pending.push(next);
            }
        }
    }
    seen[to as usize]
}

fn assert_point(actual: [f32; 3], expected: [f32; 3]) {
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 1.0e-4, "{actual:?} != {expected:?}");
    }
}

/// A 10 x 3 m corridor crossed by a door at x = 5 that opens straight up. The
/// spare waypoint is on no path until a test relinks to it.
fn door_map() -> (
    GeometryResult,
    Vec<MapKinematicMover>,
    Vec<MapKinematicWaypoint>,
) {
    let geo = geo_from_triangles(&floor_quad(0.0, 0.0, 10.0, 3.0, 0.0));
    let door = box_brush(DVec3::new(5.0, 0.0, -1.0), DVec3::new(5.2, 2.5, 4.0));
    let movers = vec![mover(0, "door_closed", vec![door])];
    let waypoints = vec![
        waypoint("door_closed", "door_open", DVec3::new(5.1, 1.25, 1.5)),
        waypoint("door_open", "", DVec3::new(5.1, 4.25, 1.5)),
        waypoint("door_spare", "", DVec3::new(5.1, 8.0, 1.5)),
    ];
    (geo, movers, waypoints)
}

/// Two floors 3 m apart across a 2 m shaft, with a lift docking flush with each.
fn lift_map(
    extra_triangles: &[[[f32; 3]; 3]],
) -> (
    GeometryResult,
    Vec<MapKinematicMover>,
    Vec<MapKinematicWaypoint>,
) {
    let mut triangles = Vec::new();
    triangles.extend_from_slice(&floor_quad(0.0, 0.0, 4.0, 4.0, 0.0));
    triangles.extend_from_slice(&floor_quad(6.0, 0.0, 10.0, 4.0, 3.0));
    triangles.extend_from_slice(extra_triangles);
    let deck = box_brush(DVec3::new(4.0, -0.2, 0.0), DVec3::new(6.0, 0.0, 4.0));
    let movers = vec![mover(0, "lift_low", vec![deck])];
    let waypoints = vec![
        waypoint("lift_low", "lift_high", DVec3::new(5.0, 0.0, 2.0)),
        waypoint("lift_high", "", DVec3::new(5.0, 3.0, 2.0)),
    ];
    (geo_from_triangles(&triangles), movers, waypoints)
}

/// A closed 4 x 4 m room with a 2 m doorway in its west wall, in a shaft through
/// an arena at y = 0. Docked lowered its roof is flush with the arena and its
/// floor with a corridor at y = -3 west of it; docked raised its floor is flush
/// with the arena.
fn rising_room_map() -> (
    GeometryResult,
    Vec<MapKinematicMover>,
    Vec<MapKinematicWaypoint>,
) {
    let mut triangles = Vec::new();
    triangles.extend_from_slice(&floor_quad(0.0, 0.0, 12.0, 4.0, 0.0));
    triangles.extend_from_slice(&floor_quad(0.0, 8.0, 12.0, 12.0, 0.0));
    triangles.extend_from_slice(&floor_quad(0.0, 4.0, 4.0, 8.0, 0.0));
    triangles.extend_from_slice(&floor_quad(8.0, 4.0, 12.0, 8.0, 0.0));
    triangles.extend_from_slice(&floor_quad(0.0, 4.5, 4.0, 7.5, -3.0));
    let brushes = vec![
        // Floor and roof.
        box_brush(DVec3::new(4.0, -3.25, 4.0), DVec3::new(8.0, -3.0, 8.0)),
        box_brush(DVec3::new(4.0, -0.25, 4.0), DVec3::new(8.0, 0.0, 8.0)),
        // South, north and east walls.
        box_brush(DVec3::new(4.0, -3.0, 4.0), DVec3::new(8.0, -0.25, 4.25)),
        box_brush(DVec3::new(4.0, -3.0, 7.75), DVec3::new(8.0, -0.25, 8.0)),
        box_brush(DVec3::new(7.75, -3.0, 4.0), DVec3::new(8.0, -0.25, 8.0)),
        // West wall, either side of the doorway.
        box_brush(DVec3::new(4.0, -3.0, 4.0), DVec3::new(4.25, -0.25, 5.0)),
        box_brush(DVec3::new(4.0, -3.0, 7.0), DVec3::new(4.25, -0.25, 8.0)),
    ];
    let movers = vec![mover(0, "room_low", brushes)];
    let waypoints = vec![
        waypoint("room_low", "room_high", DVec3::new(6.0, -1.5, 6.0)),
        waypoint("room_high", "", DVec3::new(6.0, 1.5, 6.0)),
    ];
    (geo_from_triangles(&triangles), movers, waypoints)
}

#[test]
fn a_map_with_no_movers_bakes_empty_tables() {
    let (geo, _, waypoints) = door_map();

    let plain = bake_navmesh(&geo, &params()).expect("floor must bake");
    let unlinked = bake(&geo, &[], &waypoints);

    assert_eq!(plain.regions.len(), 1);
    assert!(plain.gates.is_empty() && plain.links.is_empty());
    assert_eq!(unlinked, plain, "waypoints alone bake nothing");
}

#[test]
fn a_spinner_produces_no_gate_or_link_and_no_split_at_any_authored_speed() {
    let (geo, mut movers, waypoints) = door_map();
    let plain = bake_navmesh(&geo, &params()).expect("floor must bake");
    movers[0].spin_axis = [0.0, 1.0, 0.0];

    for spin_speed_deg_s in [0.0, 90.0] {
        movers[0].spin_speed_deg_s = spin_speed_deg_s;
        assert_eq!(bake(&geo, &movers, &waypoints), plain);
    }
}

#[test]
fn a_door_gates_its_doorway_at_the_closed_waypoint_and_not_at_the_open_one() {
    let (geo, movers, waypoints) = door_map();

    let section = bake(&geo, &movers, &waypoints);

    assert_eq!(
        section.regions.len(),
        3,
        "the door's footprint is a region of its own, between the two sides"
    );
    assert_eq!(section.portals.len(), 2);
    let gate = |portal| NavMoverGate {
        mover_id: 0,
        waypoint: 0,
        portal,
    };
    assert_eq!(section.gates, vec![gate(0), gate(1)]);

    let west = region_at(&section, 2.0, 1.5, 0.0);
    let east = region_at(&section, 8.0, 1.5, 0.0);
    assert!(reaches(&section, west, east, &gated_at(&section, 1)));
    assert!(!reaches(&section, west, east, &gated_at(&section, 0)));
}

#[test]
fn a_gate_tests_each_brush_so_a_hollow_room_gates_only_what_its_walls_block() {
    // Walls and a roof standing on an open floor, doorway to the west.
    let geo = geo_from_triangles(&floor_quad(0.0, 0.0, 12.0, 12.0, 0.0));
    let wall = |min: [f64; 2], max: [f64; 2]| {
        box_brush(
            DVec3::new(min[0], 0.0, min[1]),
            DVec3::new(max[0], 2.5, max[1]),
        )
    };
    let brushes = vec![
        wall([4.0, 4.0], [8.0, 4.25]),
        wall([4.0, 7.75], [8.0, 8.0]),
        wall([7.75, 4.0], [8.0, 8.0]),
        wall([4.0, 4.0], [4.25, 5.25]),
        wall([4.0, 6.75], [4.25, 8.0]),
        box_brush(DVec3::new(4.0, 2.5, 4.0), DVec3::new(8.0, 2.75, 8.0)),
    ];
    let movers = vec![mover(0, "room_down", brushes)];
    let waypoints = vec![
        waypoint("room_down", "room_up", DVec3::new(6.0, 1.25, 6.0)),
        waypoint("room_up", "", DVec3::new(6.0, 6.25, 6.0)),
    ];

    let section = bake(&geo, &movers, &waypoints);

    let closed = gated_at(&section, 0);
    assert!(
        closed.contains(&portal_at(&section, 8.0, 6.0)),
        "the east wall blocks its portal"
    );
    for doorway_x in [4.0, 4.25] {
        assert!(
            !closed.contains(&portal_at(&section, doorway_x, 6.0)),
            "the doorway stays open at x = {doorway_x}"
        );
    }
    let inside = region_at(&section, 6.0, 6.0, 0.0);
    let outside = region_at(&section, 2.0, 6.0, 0.0);
    assert!(reaches(&section, inside, outside, &closed));
    assert!(
        gated_at(&section, 1).is_empty(),
        "raised clear, its walls block nothing"
    );
}

#[test]
fn a_lift_top_at_floor_level_splits_nothing_and_gates_nothing() {
    let geo = geo_from_triangles(&floor_quad(0.0, 0.0, 10.0, 3.0, 0.0));
    let plain = bake_navmesh(&geo, &params()).expect("floor must bake");
    // Top within step height of the floor: an agent steps over it.
    let deck = box_brush(DVec3::new(4.0, 0.0, 1.0), DVec3::new(6.0, 0.3, 2.0));
    let movers = vec![mover(0, "deck_low", vec![deck])];
    let waypoints = vec![
        waypoint("deck_low", "deck_high", DVec3::new(5.0, 0.15, 1.5)),
        waypoint("deck_high", "", DVec3::new(5.0, 3.15, 1.5)),
    ];

    let section = bake(&geo, &movers, &waypoints);

    assert_eq!(section.regions, plain.regions);
    assert_eq!(section.portals, plain.portals);
    assert!(section.gates.is_empty());
}

#[test]
fn a_lift_links_to_the_floor_it_docks_flush_with_at_each_dock() {
    let (geo, movers, waypoints) = lift_map(&[]);

    let section = bake(&geo, &movers, &waypoints);

    assert!(section.gates.is_empty());
    let [low, high] = section.links.as_slice() else {
        panic!("one link per dock, got {:?}", section.links);
    };
    let link_ids = |link: &NavMoverLink| (link.mover_id, link.waypoint, link.region);
    assert_eq!(link_ids(low), (0, 0, region_at(&section, 2.0, 2.0, 0.0)));
    assert_eq!(link_ids(high), (0, 1, region_at(&section, 8.0, 2.0, 3.0)));
    // Spans the landing's width on the deck's rim. Leaving the deck toward -X,
    // `left` is the lesser-Z end; toward +X, the greater-Z end, as for a portal.
    assert_point(low.left, [4.0, 0.0, 0.75]);
    assert_point(low.right, [4.0, 0.0, 3.25]);
    assert_point(high.left, [6.0, 3.0, 3.25]);
    assert_point(high.right, [6.0, 3.0, 0.75]);
}

#[test]
fn a_static_wall_between_a_lift_and_a_landing_breaks_the_link() {
    let wall = [
        [[3.6, 0.0, 0.0], [3.6, 3.0, 0.0], [3.6, 3.0, 4.0]],
        [[3.6, 0.0, 0.0], [3.6, 3.0, 4.0], [3.6, 0.0, 4.0]],
    ];
    let (geo, movers, waypoints) = lift_map(&wall);

    let section = bake(&geo, &movers, &waypoints);

    assert_eq!(section.links.len(), 1, "got {:?}", section.links);
    assert_eq!(
        section.links[0].waypoint, 1,
        "only the walled dock loses it"
    );
}

#[test]
fn a_rising_rooms_floor_and_roof_link_apart_and_its_walls_leave_only_the_doorway() {
    let (geo, movers, waypoints) = rising_room_map();

    let section = bake(&geo, &movers, &waypoints);

    let arena = region_at(&section, 2.0, 6.0, 0.0);
    let corridor = region_at(&section, 2.0, 6.0, -3.0);
    let at_height = |waypoint: u32, height: f32| -> Vec<NavMoverLink> {
        section
            .links
            .iter()
            .filter(|link| link.waypoint == waypoint && (link.left[1] - height).abs() < 1.0e-3)
            .copied()
            .collect()
    };

    // Raised: the floor is flush with the arena, and the walls leave the
    // doorway, less the agent radius at either jamb.
    assert_eq!(section.links.iter().filter(|l| l.waypoint == 1).count(), 1);
    let raised = at_height(1, 0.0);
    assert_eq!(raised[0].region, arena);
    assert_point(raised[0].left, [4.0, 0.0, 5.5]);
    assert_point(raised[0].right, [4.0, 0.0, 6.5]);

    // Lowered: the same doorway serves the corridor at the floor's own height,
    // and the roof, flush with the arena, links apart from it.
    let lowered_floor = at_height(0, -3.0);
    assert_eq!(lowered_floor.len(), 1, "got {lowered_floor:?}");
    assert_eq!(lowered_floor[0].region, corridor);
    assert_point(lowered_floor[0].left, [4.0, -3.0, 5.5]);
    assert_point(lowered_floor[0].right, [4.0, -3.0, 6.5]);
    assert!(at_height(0, 0.0).iter().any(|link| link.region == arena));

    // The wall tops under the roof are within step of the arena but hidden.
    assert!(at_height(0, -0.25).is_empty());
}

#[test]
fn two_bakes_are_byte_identical_and_already_in_wire_order() {
    let (geo, movers, waypoints) = rising_room_map();

    let first = bake(&geo, &movers, &waypoints);
    let second = bake(&geo, &movers, &waypoints);

    assert_eq!(first.to_bytes(), second.to_bytes());
    // `to_bytes` sorts the tables, so a decode equals the bake only if the bake
    // already held them sorted and unique.
    let decoded = NavMeshSection::from_bytes(&first.to_bytes()).expect("section decodes");
    assert_eq!(decoded, first);
}

#[test]
fn each_mover_edit_misses_the_stage_and_equals_a_cold_bake() {
    let (geo, movers, waypoints) = door_map();
    let params = params();
    let cache_dir = tempfile::tempdir().expect("cache dir");
    let cache = StageCache::new(cache_dir.path()).expect("cache");
    let key = |movers: &[MapKinematicMover], waypoints: &[MapKinematicWaypoint]| {
        stage_key(&geo, &params, movers, waypoints).as_filename()
    };

    let base = bake_or_load_navmesh(&geo, &params, &movers, &waypoints, Some(&cache));
    assert_eq!(
        bake_or_load_navmesh(&geo, &params, &movers, &waypoints, Some(&cache)),
        base,
        "a repeat build hits and matches"
    );

    let mut moved_waypoint = waypoints.clone();
    moved_waypoint[0].origin.x += 0.5;
    let mut edited_brush = movers.clone();
    edited_brush[0].brush_volumes[0] =
        box_brush(DVec3::new(5.0, 0.0, -1.0), DVec3::new(5.5, 2.5, 4.0));
    let mut relinked_path = waypoints.clone();
    relinked_path[0].next = "door_spare".to_string();
    let mut spin_axis = movers.clone();
    spin_axis[0].spin_axis = [0.0, 1.0, 0.0];

    let edits = [
        ("moved waypoint", movers.clone(), moved_waypoint),
        ("edited brush", edited_brush, waypoints.clone()),
        ("relinked path", movers.clone(), relinked_path),
        ("spin axis", spin_axis, waypoints.clone()),
    ];
    for (edit, edited_movers, edited_waypoints) in &edits {
        assert_ne!(
            key(edited_movers, edited_waypoints),
            key(&movers, &waypoints),
            "{edit} must miss the stage"
        );
        let cold = bake_navmesh_with_movers(&geo, &params, edited_movers, edited_waypoints);
        for pass in ["miss", "hit"] {
            let warm =
                bake_or_load_navmesh(&geo, &params, edited_movers, edited_waypoints, Some(&cache));
            assert_eq!(warm, cold, "{edit}, warm {pass}");
        }
    }
}

#[test]
fn a_mover_edit_the_bake_never_reads_keeps_the_stage_key() {
    let (geo, movers, waypoints) = door_map();
    let params = params();
    let mut faster = movers.clone();
    faster[0].speed = 9.0;

    assert_eq!(
        stage_key(&geo, &params, &faster, &waypoints).as_filename(),
        stage_key(&geo, &params, &movers, &waypoints).as_filename(),
    );
}
