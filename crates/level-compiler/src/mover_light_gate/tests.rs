use glam::DVec3;

use super::pose::{MoverPoses, ONE_WAYPOINT_REASON, PosedBrush, SPIN_REASON, Terminus};
use super::segment::{CONTACT_TOLERANCE_M, point_touches_brush, segment_touches_brush};
use super::texels::{CandidateMover, light_candidates};
use crate::lightmap_reach::test_fixtures::point_light;
use crate::map_data::{
    BrushPlane, BrushVolume, KinematicMoveMode, MapKinematicMover, MapKinematicWaypoint,
};
use crate::partition::Aabb;

fn box_brush(min: DVec3, max: DVec3) -> BrushVolume {
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
        sides: Vec::new(),
        aabb: Aabb { min, max },
    }
}

fn posed(min: DVec3, max: DVec3) -> PosedBrush {
    PosedBrush::translated(&box_brush(min, max), DVec3::ZERO)
}

fn mover(mover_id: u32, name: &str, path: &str, brushes: Vec<BrushVolume>) -> MapKinematicMover {
    MapKinematicMover {
        mover_id,
        name: name.to_string(),
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
        move_mode: KinematicMoveMode::Once,
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

/// A 1 m door slab at waypoint `a`, sliding 3 m along +x to `b`.
fn sliding_door() -> (Vec<MapKinematicMover>, Vec<MapKinematicWaypoint>) {
    (
        vec![mover(
            0,
            "door",
            "a",
            vec![box_brush(
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(1.0, 2.0, 0.25),
            )],
        )],
        vec![
            waypoint("a", "b", DVec3::new(0.5, 1.0, 0.125)),
            waypoint("b", "", DVec3::new(3.5, 1.0, 0.125)),
        ],
    )
}

#[test]
fn last_terminus_poses_the_authored_brushes_moved_by_the_waypoint_offset() {
    let (movers, waypoints) = sliding_door();
    let poses = MoverPoses::resolve(&movers, &waypoints);
    assert!(poses.not_analysed.is_empty());
    let door = &poses.analysed[0];
    let first = door.pose(Terminus::First);
    let last = door.pose(Terminus::Last);
    assert_eq!(
        (first.waypoint.as_str(), last.waypoint.as_str()),
        ("a", "b")
    );
    assert_eq!(last.bounds.min, DVec3::new(3.0, 0.0, 0.0));
    assert_eq!(last.bounds.max, DVec3::new(4.0, 2.0, 0.25));

    let authored_centre = DVec3::new(0.5, 1.0, 0.125);
    let moved_centre = authored_centre + DVec3::new(3.0, 0.0, 0.0);
    assert!(point_touches_brush(authored_centre, &first.brushes[0]));
    assert!(!point_touches_brush(authored_centre, &last.brushes[0]));
    assert!(point_touches_brush(moved_centre, &last.brushes[0]));
}

#[test]
fn termini_are_the_first_and_last_waypoints_of_a_three_waypoint_chain() {
    let movers = vec![mover(
        0,
        "lift",
        "bottom",
        vec![box_brush(DVec3::ZERO, DVec3::ONE)],
    )];
    // Listed out of chain order: the walk follows `next`, not the list.
    let waypoints = vec![
        waypoint("top", "", DVec3::new(0.0, 5.0, 0.0)),
        waypoint("bottom", "middle", DVec3::ZERO),
        waypoint("middle", "top", DVec3::new(9.0, 2.0, 0.0)),
    ];
    let poses = MoverPoses::resolve(&movers, &waypoints);
    let lift = &poses.analysed[0];
    assert_eq!(lift.pose(Terminus::First).waypoint, "bottom");
    assert_eq!(lift.pose(Terminus::Last).waypoint, "top");
    assert_eq!(
        lift.pose(Terminus::Last).bounds.min,
        DVec3::new(0.0, 5.0, 0.0)
    );

    // A light the middle waypoint alone would bury meets neither terminus.
    let at_middle = point_light(DVec3::new(9.5, 2.5, 0.5), 0.2);
    assert!(light_candidates(&at_middle, &poses).is_empty());
}

#[test]
fn segment_test_counts_touching_and_flush_contact() {
    let brush = posed(DVec3::ZERO, DVec3::ONE);
    // Crossing.
    assert!(segment_touches_brush(
        DVec3::new(-1.0, 0.5, 0.5),
        DVec3::new(2.0, 0.5, 0.5),
        &brush
    ));
    // Ending exactly on a face.
    assert!(segment_touches_brush(
        DVec3::new(-1.0, 0.5, 0.5),
        DVec3::new(0.0, 0.5, 0.5),
        &brush
    ));
    // Grazing an edge: it meets the box only along x = 0, y = 1.
    assert!(segment_touches_brush(
        DVec3::new(-1.0, 0.0, 0.5),
        DVec3::new(1.0, 2.0, 0.5),
        &brush
    ));
    // Flush: lying in a face's plane, across the face.
    assert!(segment_touches_brush(
        DVec3::new(-1.0, 1.0, 0.5),
        DVec3::new(2.0, 1.0, 0.5),
        &brush
    ));
    // Starting inside.
    assert!(segment_touches_brush(
        DVec3::splat(0.5),
        DVec3::new(0.5, 9.0, 0.5),
        &brush
    ));
    // Passing a millimetre clear, past the contact band.
    let clear = 1.0 + 10.0 * CONTACT_TOLERANCE_M;
    assert!(!segment_touches_brush(
        DVec3::new(-1.0, clear, 0.5),
        DVec3::new(2.0, clear, 0.5),
        &brush
    ));
    // Stopping short of the brush.
    assert!(!segment_touches_brush(
        DVec3::new(-2.0, 0.5, 0.5),
        DVec3::new(-0.01, 0.5, 0.5),
        &brush
    ));
}

/// Two 1 m pillars with a 1 m doorway between them, x 1..2, as one mover.
fn hollow_mover() -> (Vec<MapKinematicMover>, Vec<MapKinematicWaypoint>) {
    (
        vec![mover(
            0,
            "",
            "a",
            vec![
                box_brush(DVec3::ZERO, DVec3::new(1.0, 2.0, 1.0)),
                box_brush(DVec3::new(2.0, 0.0, 0.0), DVec3::new(3.0, 2.0, 1.0)),
            ],
        )],
        vec![
            waypoint("a", "b", DVec3::ZERO),
            waypoint("b", "", DVec3::new(0.0, 0.0, 10.0)),
        ],
    )
}

#[test]
fn a_segment_through_a_hollow_movers_doorway_touches_no_brush() {
    let (movers, waypoints) = hollow_mover();
    let poses = MoverPoses::resolve(&movers, &waypoints);
    let pose = poses.analysed[0].pose(Terminus::First);
    // Its hull would block this segment; its brushes do not.
    let (start, end) = (DVec3::new(1.5, 1.0, -2.0), DVec3::new(1.5, 1.0, 3.0));
    assert!(
        !pose
            .brushes
            .iter()
            .any(|brush| segment_touches_brush(start, end, brush))
    );
    // Through a pillar it counts.
    let (start, end) = (DVec3::new(0.5, 1.0, -2.0), DVec3::new(0.5, 1.0, 3.0));
    assert!(
        pose.brushes
            .iter()
            .any(|brush| segment_touches_brush(start, end, brush))
    );
}

#[test]
fn a_light_inside_a_posed_brush_is_flagged_and_one_in_a_doorway_is_not() {
    let (movers, waypoints) = hollow_mover();
    let poses = MoverPoses::resolve(&movers, &waypoints);

    let buried = point_light(DVec3::new(0.5, 1.0, 0.5), 2.0);
    assert_eq!(
        light_candidates(&buried, &poses),
        vec![CandidateMover {
            mover: 0,
            // Raised 10 m, the last pose lies past the light's reach.
            inside: [Some(true), None],
        }]
    );

    let on_face = point_light(DVec3::new(1.0, 1.0, 0.5), 2.0);
    assert_eq!(
        light_candidates(&on_face, &poses)[0].inside[0],
        Some(true),
        "a light on a posed face counts as inside"
    );

    let in_doorway = point_light(DVec3::new(1.5, 1.0, 0.5), 2.0);
    assert_eq!(
        light_candidates(&in_doorway, &poses)[0].inside[0],
        Some(false),
        "a light between a hollow mover's brushes is not inside"
    );
}

#[test]
fn a_mover_with_a_spin_axis_is_not_analysed_and_a_zero_axis_is() {
    let (mut movers, mut waypoints) = sliding_door();
    // A spin axis with zero speed: scripts may still spin it.
    let mut idle_spinner = mover(
        1,
        "fan",
        "fan_hub",
        vec![box_brush(DVec3::ZERO, DVec3::ONE)],
    );
    idle_spinner.spin_axis = [0.0, 1.0, 0.0];
    waypoints.push(waypoint("fan_hub", "fan_end", DVec3::ZERO));
    waypoints.push(waypoint("fan_end", "", DVec3::X));
    // A one-waypoint spinner.
    let mut spinner = mover(2, "", "spin_hub", vec![box_brush(DVec3::ZERO, DVec3::ONE)]);
    spinner.spin_axis = [0.0, 0.0, 1.0];
    spinner.spin_speed_deg_s = 90.0;
    waypoints.push(waypoint("spin_hub", "", DVec3::new(5.0, 0.0, 0.0)));
    movers.extend([idle_spinner, spinner]);

    let poses = MoverPoses::resolve(&movers, &waypoints);
    assert_eq!(
        poses
            .analysed
            .iter()
            .map(|mover| mover.mover_id)
            .collect::<Vec<_>>(),
        vec![0]
    );
    let reasons: Vec<(u32, &str)> = poses
        .not_analysed
        .iter()
        .map(|mover| (mover.mover_id, mover.reason))
        .collect();
    assert_eq!(reasons, vec![(1, SPIN_REASON), (2, SPIN_REASON)]);
    assert_eq!(poses.not_analysed[0].label, "kinematic_mover 'fan'");
    assert_eq!(
        poses.not_analysed[1].label, "kinematic_mover at (0.500, 0.500, 0.500) m",
        "an unnamed mover is named by its brush-bounds centre"
    );
}

#[test]
fn a_one_waypoint_path_without_spin_is_not_analysed() {
    let movers = vec![mover(
        0,
        "stub",
        "only",
        vec![box_brush(DVec3::ZERO, DVec3::ONE)],
    )];
    let waypoints = vec![waypoint("only", "", DVec3::ZERO)];
    let poses = MoverPoses::resolve(&movers, &waypoints);
    assert!(poses.analysed.is_empty());
    assert_eq!(poses.not_analysed[0].reason, ONE_WAYPOINT_REASON);
}
