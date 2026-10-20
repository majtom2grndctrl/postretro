//! The e24 fixture's pools sample the pushes over them.

use super::*;

// The e24 fixture's current channel samples its coincident push on every
// face, so no face of it is left still; the fluid-motion wing's pools split
// exactly where their pushes are.
#[test]
fn e24_fixture_current_and_wing_pools_sample_the_pushes_over_them() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/maps/e24-environment-test.map");
    let map = parse_map_file(&path, MapFormat::IdTech2).expect("e24 fixture parses");
    let mut texture_names = TextureNamesSection { names: Vec::new() };
    let section = encode_environment_volumes_section(
        &map.environment_volumes,
        map.initial_gravity,
        &map.brush_volumes,
        &mut texture_names,
    );
    let fluid_at = |quake_point: [f64; 3]| {
        section
            .volumes
            .iter()
            .find(|record| !record.fluid.is_empty() && record_contains(record, engine(quake_point)))
            .expect("a fluid record holds the point")
    };
    // `0 6 0` in map axes is a current toward map +Y: engine -X.
    let north = [-6.0f32, 0.0, 0.0];

    let channel = fluid_at([1360.0, 784.0, -64.0]);
    assert!(!channel.surface_faces.is_empty());
    for face in &channel.surface_faces {
        assert_eq!(pushes_at(&section, inside_sample(face)), vec![north]);
    }

    // The two-brush pool: its east brush's top face splits at the push edge
    // (x 1904); only the part west of it samples the push.
    let partial = fluid_at([1700.0, 544.0, -64.0]);
    let tops: Vec<&EnvironmentSurfaceFace> = partial
        .surface_faces
        .iter()
        .filter(|face| facing(face) == "+z")
        .collect();
    assert_eq!(
        tops.len(),
        3,
        "west top, east top's pushed part, east top's still part"
    );
    let pushed = tops
        .iter()
        .filter(|face| !pushes_at(&section, inside_sample(face)).is_empty())
        .count();
    assert_eq!(pushed, 2);
    // The pieces come from two brushes; closure closes the seam between them.
    assert_eq!(
        t_junctions(partial),
        Vec::<DVec3>::new(),
        "the two-brush partial-push pool has no T-junctions"
    );

    // The bending river: every top part samples exactly one push, east then
    // north, and no surface is drawn between its two brushes.
    let river = fluid_at([2200.0, 512.0, -64.0]);
    let mut directions = std::collections::BTreeSet::new();
    for face in river
        .surface_faces
        .iter()
        .filter(|face| facing(face) == "+z")
    {
        let pushes = pushes_at(&section, inside_sample(face));
        assert_eq!(pushes.len(), 1, "each river part lies under one push");
        directions.insert(format!("{:?}", pushes[0]));
    }
    assert_eq!(
        directions.len(),
        2,
        "the river drifts along both legs: {directions:?}"
    );
    // The legs share the plane y 576 over x 2384..2512.
    for face in &river.surface_faces {
        let points: Vec<DVec3> = face.vertices.iter().map(|v| quake(v.position)).collect();
        let centroid_x = points.iter().map(|p| p.x).sum::<f64>() / points.len() as f64;
        let on_shared_plane = points.iter().all(|p| (p.y - 576.0).abs() < 1e-2);
        assert!(
            !(on_shared_plane && centroid_x > 2384.0),
            "a surface is drawn between the river's brushes: {points:?}"
        );
    }
}
