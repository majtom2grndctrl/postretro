use glam::{DVec3, Vec3};
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

use super::lights::GateLight;
use super::pose::{MoverPoses, ONE_WAYPOINT_REASON, PosedBrush, SPIN_REASON, Terminus};
use super::probes::{ProbeScene, gate_probes};
use super::segment::{CONTACT_TOLERANCE_M, point_touches_brush, segment_touches_brush};
use super::side_count::{SideCount, side_counts};
use super::texels::{CandidateMover, TexelGate, light_candidates};
use crate::bake_control::BakeControl;
use crate::bvh_build::build_bvh;
use crate::geometry::{FaceIndexRange, GeometryResult};
use crate::lightmap_reach::test_fixtures::point_light;
use crate::map_data::{
    BrushPlane, BrushVolume, CarriedLightLink, KinematicMoveMode, LightType, MapKinematicMover,
    MapKinematicWaypoint, MapLight,
};
use crate::partition::Aabb;
use crate::ray_traversal::RAY_EPSILON;
use crate::sh_bake::ProbeGridLayout;

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

fn dynamic_light(origin: DVec3, range: f32) -> MapLight {
    MapLight {
        is_dynamic: true,
        ..point_light(origin, range)
    }
}

fn carried_by(source_light_index: usize, mover_id: u32) -> CarriedLightLink {
    CarriedLightLink {
        source_light_index,
        mover_id,
        local_offset: [0.0; 3],
    }
}

#[test]
fn side_count_reaches_the_swept_bounds_at_every_waypoint_not_only_the_termini() {
    let movers = vec![mover(
        0,
        "lift",
        "bottom",
        vec![box_brush(DVec3::ZERO, DVec3::ONE)],
    )];
    let waypoints = vec![
        waypoint("bottom", "middle", DVec3::ZERO),
        waypoint("middle", "top", DVec3::new(9.0, 0.0, 0.0)),
        waypoint("top", "", DVec3::new(0.0, 5.0, 0.0)),
    ];
    let poses = MoverPoses::resolve(&movers, &waypoints);
    assert_eq!(poses.analysed[0].swept.max, DVec3::new(10.0, 6.0, 1.0));

    let lights = vec![
        // Beside the middle pose only.
        dynamic_light(DVec3::new(10.5, 0.5, 0.5), 1.0),
        // Too far from any pose.
        dynamic_light(DVec3::new(20.0, 20.0, 0.5), 1.0),
        // A baked light beside the middle pose is the gate's, not the side count's.
        point_light(DVec3::new(10.5, 0.5, 0.5), 1.0),
        // A dynamic bake-only light has no runtime shadow to price.
        MapLight {
            bake_only: true,
            ..dynamic_light(DVec3::new(0.5, 0.5, 0.5), 1.0)
        },
    ];
    assert_eq!(
        side_counts(&poses, &lights, &[]),
        vec![SideCount {
            mover_id: 0,
            carried: Vec::new(),
            others: vec![0],
        }]
    );
}

#[test]
fn side_count_splits_lights_this_mover_carries_from_the_rest() {
    let (mut movers, mut waypoints) = sliding_door();
    movers.push(mover(
        1,
        "cage",
        "cage_a",
        vec![box_brush(
            DVec3::new(0.0, 3.0, 0.0),
            DVec3::new(1.0, 4.0, 1.0),
        )],
    ));
    waypoints.push(waypoint("cage_a", "cage_b", DVec3::new(0.5, 3.5, 0.5)));
    waypoints.push(waypoint("cage_b", "", DVec3::new(0.5, 3.5, 4.5)));
    let poses = MoverPoses::resolve(&movers, &waypoints);

    // Light 0 rides the door, light 1 the cage; both reach both movers.
    let lights = vec![
        dynamic_light(DVec3::new(0.5, 1.0, 0.5), 5.0),
        dynamic_light(DVec3::new(0.5, 3.5, 0.5), 5.0),
    ];
    let links = [carried_by(0, 0), carried_by(1, 1)];
    assert_eq!(
        side_counts(&poses, &lights, &links),
        vec![
            SideCount {
                mover_id: 0,
                carried: vec![0],
                others: vec![1],
            },
            SideCount {
                mover_id: 1,
                carried: vec![1],
                others: vec![0],
            },
        ]
    );
}

/// A 9 m plate 1 m above a line of probes at x −4 to 4, y 0, under a light
/// 3 m above the middle probe. The plate covers every probe at its first
/// waypoint and leaves for 100 m up at its last.
fn plate_over_probes() -> MoverPoses {
    let movers = vec![mover(
        0,
        "plate",
        "low",
        vec![box_brush(
            DVec3::new(-4.5, 1.0, -0.5),
            DVec3::new(4.5, 1.1, 0.5),
        )],
    )];
    let waypoints = vec![
        waypoint("low", "high", DVec3::new(0.0, 1.05, 0.0)),
        waypoint("high", "", DVec3::new(0.0, 101.05, 0.0)),
    ];
    MoverPoses::resolve(&movers, &waypoints)
}

const PROBE_LIGHT: DVec3 = DVec3::new(0.0, 3.0, 0.0);

/// Nine valid probes 1 m apart along x, at x −4 to 4.
fn probe_line() -> ProbeGridLayout {
    let world_min = DVec3::new(-4.0, 0.0, 0.0);
    ProbeGridLayout {
        world_min,
        dims: [9, 1, 1],
        cell_size: [1.0; 3],
        far_sentinel: 1.0,
        probe_positions: (0..9)
            .map(|x| world_min + DVec3::X * f64::from(x))
            .collect(),
        validity: vec![1; 9],
    }
}

fn geometry_of(triangles: &[[[f32; 3]; 3]]) -> GeometryResult {
    let vertex = |position| {
        Vertex::new(
            position,
            [0.0; 2],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0; 2],
            0,
        )
    };
    GeometryResult {
        geometry: GeometrySection {
            vertices: triangles.iter().flatten().map(|&p| vertex(p)).collect(),
            indices: (0..triangles.len() as u32 * 3).collect(),
            faces: triangles
                .iter()
                .map(|_| FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                })
                .collect(),
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: (0..triangles.len() as u32)
            .map(|face| FaceIndexRange {
                index_offset: face * 3,
                index_count: 3,
            })
            .collect(),
    }
}

/// Per terminus, the probes `light` gates on `layout` against `triangles`.
fn gated_probes(
    light: MapLight,
    layout: &ProbeGridLayout,
    triangles: &[[[f32; 3]; 3]],
) -> [u64; 2] {
    let poses = plate_over_probes();
    let geometry = geometry_of(triangles);
    let (bvh, primitives, _) = build_bvh(&geometry).expect("bvh");
    let lights = [GateLight {
        source_index: 0,
        light: &light,
    }];
    let probes = gate_probes(
        &poses,
        &lights,
        &ProbeScene {
            layout,
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geometry,
        },
        &BakeControl::unrestricted(),
    );
    match probes.as_slice() {
        [] => [0, 0],
        [light] => {
            assert_eq!(light.movers.len(), 1);
            light.movers[0].gated
        }
        _ => panic!("one light gated: {probes:?}"),
    }
}

#[test]
fn a_probe_behind_a_posed_brush_counts_when_the_light_reaches_it() {
    let gated = gated_probes(point_light(PROBE_LIGHT, 100.0), &probe_line(), &[]);
    assert_eq!(
        gated,
        [9, 0],
        "every probe under the plate, none once it leaves"
    );
}

#[test]
fn an_invalid_probe_behind_a_posed_brush_is_never_counted() {
    let mut layout = probe_line();
    layout.validity[0] = 0;
    layout.validity[4] = 0;
    let gated = gated_probes(point_light(PROBE_LIGHT, 100.0), &layout, &[]);
    assert_eq!(gated, [7, 0]);
}

#[test]
fn a_probe_behind_a_posed_brush_past_the_falloff_range_is_not_counted() {
    // In range when √(x² + 9) ≤ 4, so |x| ≤ 2.
    let gated = gated_probes(point_light(PROBE_LIGHT, 4.0), &probe_line(), &[]);
    assert_eq!(gated, [5, 0]);
}

#[test]
fn a_probe_behind_a_posed_brush_outside_a_spots_outer_cone_is_not_counted() {
    // Aimed straight down: probe x sits atan(|x| / 3) off the axis, 18° at
    // |x| = 1 and 34° at |x| = 2, so a 25° outer cone holds |x| ≤ 1.
    let mut spot = point_light(PROBE_LIGHT, 100.0);
    spot.light_type = LightType::Spot;
    spot.cone_direction = Some([0.0, -1.0, 0.0]);
    spot.cone_angle_inner = Some(20f32.to_radians());
    spot.cone_angle_outer = Some(25f32.to_radians());
    let gated = gated_probes(spot, &probe_line(), &[]);
    assert_eq!(gated, [3, 0]);
}

#[test]
fn a_probe_behind_a_posed_brush_and_static_geometry_is_not_counted() {
    // Probe x's segment crosses y 2 at x / 3, so a static slab there over
    // x 0.2 to 5 shades the probes at x 1 to 4.
    let slab = [
        [[0.2, 2.0, -1.0], [5.0, 2.0, -1.0], [5.0, 2.0, 1.0]],
        [[0.2, 2.0, -1.0], [5.0, 2.0, 1.0], [0.2, 2.0, 1.0]],
    ];
    let gated = gated_probes(point_light(PROBE_LIGHT, 100.0), &probe_line(), &slab);
    assert_eq!(gated, [5, 0]);
}

/// A sun shining straight down, with no soft angle.
fn overhead_sun() -> MapLight {
    let mut sun = point_light(DVec3::ZERO, 0.0);
    sun.light_type = LightType::Directional;
    sun.cone_direction = Some([0.0, -1.0, 0.0]);
    sun
}

#[test]
fn a_directional_light_gates_the_probes_a_mover_covers_toward_it() {
    // A sun's segment runs 10 km toward it, so the plate parked 100 m up
    // still covers every probe.
    let gated = gated_probes(overhead_sun(), &probe_line(), &[]);
    assert_eq!(gated, [9, 9]);
}

#[test]
fn a_directional_light_gates_the_texels_a_mover_covers_toward_it() {
    // Floor texels at y 0: one under the plate, one 10 m along x past its
    // edge. The plate covers the first at both termini, 100 m up included.
    let poses = plate_over_probes();
    let sun = overhead_sun();
    let candidates = light_candidates(&sun, &poses);
    let mut gate = TexelGate::new(&poses, &sun, &candidates);
    let bounds = (Vec3::new(-0.5, -0.1, -0.5), Vec3::new(10.5, 0.1, 0.5));
    assert!(gate.begin_chart(bounds), "the plate lies over the chart");
    let lift = Vec3::Y * RAY_EPSILON;
    for centre in [Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)] {
        gate.add_texel(centre, centre + lift, 3);
    }
    let movers = gate.finish();
    assert_eq!(movers.len(), 1);
    assert_eq!(movers[0].gated, [1, 1]);
    assert_eq!(movers[0].both, 1);
    assert_eq!(movers[0].blocks[0], [3]);
}
