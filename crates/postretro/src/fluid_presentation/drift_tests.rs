// Install drift resolve per surface face: which push a face samples, and the
// steady-state speed it drifts at.
// See: context/lib/rendering_pipeline.md §7.3a

use glam::Vec3;
use postretro_collision::{ConvexLeaf, EnvironmentVolume, EnvironmentVolumes, LeafPlane};
use postretro_combat_model::{FluidMovementTable, FluidMovementTuning};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};

use super::resolve_fluid_surface_drift;

const DT: f32 = 1.0 / 60.0;
const TERMINAL: f32 = 40.0;
/// The waterline of every fixture pool, meters.
const WATERLINE: f32 = 0.0;

fn axis_leaf(min: Vec3, max: Vec3) -> ConvexLeaf {
    let mut planes = Vec::new();
    for axis in 0..3 {
        let mut normal = Vec3::ZERO;
        normal[axis] = 1.0;
        planes.push(LeafPlane {
            normal,
            distance: max[axis],
        });
        planes.push(LeafPlane {
            normal: -normal,
            distance: -min[axis],
        });
    }
    ConvexLeaf {
        planes,
        aabb_min: min,
        aabb_max: max,
    }
}

fn water_table() -> FluidMovementTable {
    FluidMovementTable::new(vec![
        FluidMovementTuning {
            name: "sludge".into(),
            viscosity: 6.0,
        },
        FluidMovementTuning {
            name: "water".into(),
            viscosity: 2.0,
        },
    ])
    .expect("valid table")
}

/// An up-facing quad at the waterline over x `x0..x1`, z `-1..1`, wound
/// counter-clockwise seen from above.
fn top_face(x0: f32, x1: f32) -> EnvironmentSurfaceFace {
    let corners = [
        Vec3::new(x0, WATERLINE, 1.0),
        Vec3::new(x1, WATERLINE, 1.0),
        Vec3::new(x1, WATERLINE, -1.0),
        Vec3::new(x0, WATERLINE, -1.0),
    ];
    EnvironmentSurfaceFace {
        texture_index: 0,
        vertices: corners
            .iter()
            .map(|corner| EnvironmentSurfaceVertex {
                position: corner.to_array(),
                uv: [corner.x * 39.37, corner.z * 39.37],
            })
            .collect(),
    }
}

fn fluid_record(fluid: &str, faces: Vec<EnvironmentSurfaceFace>) -> EnvironmentVolumeRecord {
    EnvironmentVolumeRecord {
        source_entity: 1,
        priority: 0,
        gravity: None,
        push: None,
        fluid: fluid.into(),
        brushes: Vec::new(),
        surface_faces: faces,
    }
}

fn push_volume(priority: i32, push: Vec3, min: Vec3, max: Vec3) -> EnvironmentVolume {
    EnvironmentVolume {
        source_entity: 2,
        priority,
        gravity: None,
        push: Some(push),
        force: 1.0,
        fluid: None,
        brushes: vec![axis_leaf(min, max)],
    }
}

fn drifts(
    records: &[EnvironmentVolumeRecord],
    pushes: Vec<EnvironmentVolume>,
    table: &FluidMovementTable,
) -> Vec<Vec3> {
    let volumes = EnvironmentVolumes::build(pushes);
    resolve_fluid_surface_drift(records, &volumes, table, TERMINAL, DT)
}

#[track_caller]
fn assert_near(got: Vec3, want: Vec3) {
    assert!(
        (got - want).length() < 1.0e-4,
        "drift {got}, expected {want}"
    );
}

/// A current under the waterline over x `x0..x1`, its top flush with it.
fn current(push: Vec3, x0: f32, x1: f32) -> EnvironmentVolume {
    push_volume(
        0,
        push,
        Vec3::new(x0, -2.0, -1.0),
        Vec3::new(x1, WATERLINE, 1.0),
    )
}

#[test]
fn a_face_over_a_push_drifts_at_its_steady_state_and_a_face_beside_it_is_still() {
    let records = [fluid_record(
        "water",
        vec![top_face(0.0, 1.0), top_face(1.0, 2.0)],
    )];
    let got = drifts(
        &records,
        vec![current(Vec3::new(6.0, 0.0, 0.0), 0.0, 1.0)],
        &water_table(),
    );
    // push 6 ÷ water viscosity 2.
    assert_near(got[0], Vec3::new(3.0, 0.0, 0.0));
    assert_near(got[1], Vec3::ZERO);
}

// A wind whose bottom sits on the waterline moves no water.
#[test]
fn a_push_resting_on_the_waterline_from_above_gives_no_drift() {
    let records = [fluid_record("water", vec![top_face(0.0, 1.0)])];
    let wind = push_volume(
        0,
        Vec3::new(8.0, 0.0, 0.0),
        Vec3::new(-1.0, WATERLINE, -2.0),
        Vec3::new(2.0, 3.0, 2.0),
    );
    assert_near(drifts(&records, vec![wind], &water_table())[0], Vec3::ZERO);
}

#[test]
fn two_pushes_over_one_face_drift_with_the_overlap_winner() {
    let records = [fluid_record("water", vec![top_face(0.0, 1.0)])];
    let low = current(Vec3::new(6.0, 0.0, 0.0), -1.0, 2.0);
    let mut high = current(Vec3::new(0.0, 0.0, 6.0), -1.0, 2.0);
    high.priority = 1;
    assert_near(
        drifts(&records, vec![low.clone(), high.clone()], &water_table())[0],
        Vec3::new(0.0, 0.0, 3.0),
    );
    // At equal priority the smaller volume wins.
    let small = current(Vec3::new(0.0, 0.0, -6.0), -0.5, 1.5);
    assert_near(
        drifts(&records, vec![low, small], &water_table())[0],
        Vec3::new(0.0, 0.0, -3.0),
    );
}

#[test]
fn viscosity_is_read_by_fluid_name_and_an_undeclared_fluid_drifts_none() {
    let push = || vec![current(Vec3::new(6.0, 0.0, 0.0), -1.0, 2.0)];
    let sludge = [fluid_record("sludge", vec![top_face(0.0, 1.0)])];
    assert_near(
        drifts(&sludge, push(), &water_table())[0],
        Vec3::new(1.0, 0.0, 0.0),
    );

    // The same names in another order resolve the same viscosity.
    let reordered = FluidMovementTable::new(vec![
        FluidMovementTuning {
            name: "water".into(),
            viscosity: 2.0,
        },
        FluidMovementTuning {
            name: "sludge".into(),
            viscosity: 6.0,
        },
    ])
    .expect("valid table");
    assert_near(
        drifts(&sludge, push(), &reordered)[0],
        Vec3::new(1.0, 0.0, 0.0),
    );

    let lava = [fluid_record("lava", vec![top_face(0.0, 1.0)])];
    assert_near(drifts(&lava, push(), &water_table())[0], Vec3::ZERO);
    assert_near(
        drifts(&lava, push(), &FluidMovementTable::default())[0],
        Vec3::ZERO,
    );
}

#[test]
fn drift_slots_follow_record_and_face_order_across_dry_records() {
    let dry = fluid_record("", Vec::new());
    let records = [
        fluid_record("water", vec![top_face(0.0, 1.0)]),
        dry,
        fluid_record("water", vec![top_face(1.0, 2.0), top_face(2.0, 3.0)]),
    ];
    let got = drifts(
        &records,
        vec![current(Vec3::new(6.0, 0.0, 0.0), 2.0, 3.0)],
        &water_table(),
    );
    assert_eq!(got.len(), 3);
    assert_near(got[0], Vec3::ZERO);
    assert_near(got[1], Vec3::ZERO);
    assert_near(got[2], Vec3::new(3.0, 0.0, 0.0));
}

#[test]
fn force_never_changes_the_drift() {
    let records = [fluid_record("water", vec![top_face(0.0, 1.0)])];
    for force in [0.0, 0.5, 3.0] {
        let mut push = current(Vec3::new(6.0, 0.0, 0.0), -1.0, 2.0);
        push.force = force;
        assert_near(
            drifts(&records, vec![push], &water_table())[0],
            Vec3::new(3.0, 0.0, 0.0),
        );
    }
}

// Frame capture loads no fluid table, so it resolves no drift: its surfaces
// install with the renderer's zeroed drift buffer and draw still, at the fixed
// capture opacity. This is a regression tripwire on capture's source: capture
// must never resolve drift. The renderer's zeroed drift buffer and the GPU
// frame test cover the drawn result.
#[test]
fn frame_capture_installs_fluid_surfaces_still_at_the_capture_opacity() {
    let prepared = include_str!("../capture/prepared.rs");
    assert!(prepared.contains("renderer.install_fluid_surfaces("));
    assert!(prepared.contains("CAPTURE_FLUID_SURFACE_OPACITY"));
    assert!(!prepared.contains("set_fluid_surface_drift"));
    assert!(!prepared.contains("resolve_fluid_surface_drift"));
}

// A map built before the compiler split faces along pushes carries one
// face under part of a push. It drifts whole when its sample point lies under
// the push, and stays still when it does not.
#[test]
fn an_unsplit_face_under_part_of_a_push_drifts_whole_by_its_sample_point() {
    let records = [fluid_record("water", vec![top_face(0.0, 2.0)])];
    let covering_centre = current(Vec3::new(6.0, 0.0, 0.0), 0.0, 1.5);
    assert_near(
        drifts(&records, vec![covering_centre], &water_table())[0],
        Vec3::new(3.0, 0.0, 0.0),
    );
    let missing_centre = current(Vec3::new(6.0, 0.0, 0.0), 0.0, 0.5);
    assert_near(
        drifts(&records, vec![missing_centre], &water_table())[0],
        Vec3::ZERO,
    );
}
