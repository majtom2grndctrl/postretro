//! Push splitting at the drift sample depth; the split pieces' T-junctions
//! close.

use super::*;

/// The +z faces of a record, as (area in square map units, Quake centroid).
fn top_parts(record: &EnvironmentVolumeRecord) -> Vec<(f64, DVec3)> {
    let mut parts: Vec<(f64, DVec3)> = record
        .surface_faces
        .iter()
        .filter(|face| facing(face) == "+z")
        .map(|face| {
            let centroid = face
                .vertices
                .iter()
                .map(|v| quake(v.position))
                .sum::<DVec3>()
                / face.vertices.len() as f64;
            (area(face), centroid)
        })
        .collect();
    parts.sort_by(|a, b| a.1.x.total_cmp(&b.1.x));
    parts
}

#[track_caller]
fn assert_part_areas(parts: &[(f64, DVec3)], expected: &[f64]) {
    let areas: Vec<f64> = parts.iter().map(|(area, _)| *area).collect();
    assert_eq!(areas.len(), expected.len(), "parts: {parts:?}");
    for (got, want) in areas.iter().zip(expected) {
        assert!(
            (got - want).abs() < 1e-2,
            "areas {areas:?}, expected {expected:?}"
        );
    }
}

/// A fluid box spanning Quake x 0..128, y 0..64, z 0..64 (waterline z 64).
fn channel() -> String {
    fluid(&[box_brush([0, 0, 0], [128, 64, 64], "water_tex")])
}

#[test]
fn a_push_over_half_a_fluid_face_splits_it_at_the_push_boundary() {
    // The FGD's coincident current: the push's top is flush with the waterline.
    let (_, section) = compile(
        &distant_world(),
        &[
            channel(),
            push("0 8 0", &[box_brush([0, 0, 0], [64, 64, 64], "push_tex")]),
        ],
    );
    let parts = top_parts(&section.volumes[0]);
    assert_part_areas(&parts, &[64.0 * 64.0, 64.0 * 64.0]);
    assert!(parts[0].1.x < 64.0 && parts[1].1.x > 64.0);
}

// A push whose bottom sits on the waterline meets the face from outside.
#[test]
fn a_push_resting_on_the_waterline_from_above_leaves_the_face_whole() {
    let (_, unpushed) = compile(&distant_world(), &[channel()]);
    let (_, section) = compile(
        &distant_world(),
        &[
            channel(),
            push("8 0 0", &[box_brush([0, 0, 64], [64, 64, 128], "push_tex")]),
        ],
    );
    assert_part_areas(&top_parts(&section.volumes[0]), &[128.0 * 64.0]);
    assert_eq!(section.volumes[0], unpushed.volumes[0]);
}

#[test]
fn a_push_covering_the_whole_fluid_leaves_every_face_unsplit() {
    let (_, unpushed) = compile(&distant_world(), &[channel()]);
    let (_, section) = compile(
        &distant_world(),
        &[
            channel(),
            push(
                "0 8 0",
                &[box_brush([-16, -16, -16], [144, 80, 64], "push_tex")],
            ),
        ],
    );
    assert_eq!(section.volumes[0], unpushed.volumes[0]);
}

// The face is cut by every push brush over it, not only the winner's.
#[test]
fn overlapping_pushes_split_a_face_into_parts_under_one_set_of_pushes_each() {
    let (_, section) = compile(
        &distant_world(),
        &[
            channel(),
            push("0 8 0", &[box_brush([0, 0, 0], [80, 64, 64], "push_tex")]),
            push(
                "0 -8 0",
                &[box_brush([48, 0, 0], [128, 64, 64], "push_tex")],
            ),
        ],
    );
    assert_part_areas(
        &top_parts(&section.volumes[0]),
        &[48.0 * 64.0, 32.0 * 64.0, 48.0 * 64.0],
    );
}

// Touches bury nothing and slivers are never kept; parts tile the face.
#[test]
fn push_touches_and_sliver_overlaps_leave_the_face_whole() {
    let (_, unpushed) = compile(&distant_world(), &[channel()]);
    for brush in [
        // Edge touch: the push abuts the +x side from outside.
        box_brush([128, 0, 0], [192, 64, 64], "push_tex"),
        // Corner touch.
        box_brush([128, 64, 0], [192, 128, 64], "push_tex"),
    ] {
        let (_, section) = compile(&distant_world(), &[channel(), push("0 8 0", &[brush])]);
        assert_eq!(section.volumes[0], unpushed.volumes[0]);
    }

    // A corner overlap far under 1 mm².
    let sliver = "{\n\
        ( 127.99 0 0 ) ( 127.99 1 0 ) ( 127.99 0 1 ) push_tex 0 0 0 1 1\n\
        ( 192 0 0 ) ( 192 0 1 ) ( 192 1 0 ) push_tex 0 0 0 1 1\n\
        ( 0 -64 0 ) ( 0 -64 1 ) ( 1 -64 0 ) push_tex 0 0 0 1 1\n\
        ( 0 0.01 0 ) ( 1 0.01 0 ) ( 0 0.01 1 ) push_tex 0 0 0 1 1\n\
        ( 0 0 0 ) ( 1 0 0 ) ( 0 1 0 ) push_tex 0 0 0 1 1\n\
        ( 0 0 64 ) ( 0 1 64 ) ( 1 0 64 ) push_tex 0 0 0 1 1\n\
        }"
    .to_string();
    let (_, section) = compile(&distant_world(), &[channel(), push("0 8 0", &[sliver])]);
    let parts = top_parts(&section.volumes[0]);
    assert_eq!(
        parts.len(),
        1,
        "a sub-mm² overlap split the face: {parts:?}"
    );
    assert!((parts[0].0 - 128.0 * 64.0).abs() < 1e-2);
}

#[test]
fn a_thin_push_strip_splits_off_a_thin_part_and_the_parts_tile_the_face() {
    // A 0.1-unit (2.5 mm) strip along the face's +x edge: far above 1 mm².
    let strip = "{\n\
        ( 127.9 0 0 ) ( 127.9 1 0 ) ( 127.9 0 1 ) push_tex 0 0 0 1 1\n\
        ( 160 0 0 ) ( 160 0 1 ) ( 160 1 0 ) push_tex 0 0 0 1 1\n\
        ( 0 -16 0 ) ( 0 -16 1 ) ( 1 -16 0 ) push_tex 0 0 0 1 1\n\
        ( 0 80 0 ) ( 1 80 0 ) ( 0 80 1 ) push_tex 0 0 0 1 1\n\
        ( 0 0 -16 ) ( 1 0 -16 ) ( 0 1 -16 ) push_tex 0 0 0 1 1\n\
        ( 0 0 64 ) ( 0 1 64 ) ( 1 0 64 ) push_tex 0 0 0 1 1\n\
        }"
    .to_string();
    let (_, section) = compile(&distant_world(), &[channel(), push("0 8 0", &[strip])]);
    let parts = top_parts(&section.volumes[0]);
    assert_eq!(parts.len(), 2, "{parts:?}");
    assert!((parts[1].0 - 0.1 * 64.0).abs() < 1e-2, "{parts:?}");
    let total: f64 = parts.iter().map(|(area, _)| area).sum();
    // 1 mm² in square map units.
    assert!((total - 128.0 * 64.0).abs() < 1e-6 / (UNIT * UNIT));
}

/// The +z faces of a record, west to east by centroid.
fn top_faces(record: &EnvironmentVolumeRecord) -> Vec<&EnvironmentSurfaceFace> {
    let centroid_x = |face: &EnvironmentSurfaceFace| {
        face.vertices
            .iter()
            .map(|v| quake(v.position).x)
            .sum::<f64>()
            / face.vertices.len() as f64
    };
    let mut faces: Vec<&EnvironmentSurfaceFace> = record
        .surface_faces
        .iter()
        .filter(|face| facing(face) == "+z")
        .collect();
    faces.sort_by(|a, b| centroid_x(a).total_cmp(&centroid_x(b)));
    faces
}

/// Whether each top face, west to east, samples a push.
fn tops_sampling_a_push(section: &EnvironmentVolumesSection) -> Vec<bool> {
    top_faces(&section.volumes[0])
        .iter()
        .map(|face| !pushes_at(section, inside_sample(face)).is_empty())
        .collect()
}

// The face splits where its 2 mm drift sample is inside the push, not where
// the face itself is: a current whose top sits less than 2 mm under the
// waterline drifts the part it covers, and a deeper one drifts none of it.
#[test]
fn a_current_just_under_the_waterline_splits_the_face_where_its_drift_sample_is_inside() {
    for (depth_mm, splits) in [(0.5, true), (1.5, true), (2.5, false)] {
        let top = 64.0 - depth_mm * MM;
        let current = box_brush_at([0.0, 0.0, 0.0], [64.0, 64.0, top], "push_tex");
        let (_, section) = compile(&distant_world(), &[channel(), push("0 8 0", &[current])]);
        let record = &section.volumes[0];
        if splits {
            assert_part_areas(&top_parts(record), &[64.0 * 64.0, 64.0 * 64.0]);
            assert_eq!(
                tops_sampling_a_push(&section),
                [true, false],
                "{depth_mm} mm"
            );
        } else {
            assert_part_areas(&top_parts(record), &[128.0 * 64.0]);
            assert_eq!(tops_sampling_a_push(&section), [false], "{depth_mm} mm");
        }
        assert_parts_wholly_under_or_clear(&section, record);
        assert_eq!(t_junctions(record), Vec::<DVec3>::new(), "{depth_mm} mm");
    }
}

// A push resting on the water with its bottom sunk under the waterline drifts
// the face only once its bottom is deeper than the 2 mm drift sample.
#[test]
fn a_push_sunk_just_under_the_waterline_splits_the_face_only_past_the_drift_sample() {
    for (depth_mm, splits) in [(0.5, false), (1.5, false), (2.5, true)] {
        let bottom = 64.0 - depth_mm * MM;
        let wind = box_brush_at([0.0, 0.0, bottom], [64.0, 64.0, 128.0], "push_tex");
        let (_, section) = compile(&distant_world(), &[channel(), push("8 0 0", &[wind])]);
        let record = &section.volumes[0];
        if splits {
            assert_part_areas(&top_parts(record), &[64.0 * 64.0, 64.0 * 64.0]);
            assert_eq!(
                tops_sampling_a_push(&section),
                [true, false],
                "{depth_mm} mm"
            );
        } else {
            assert_part_areas(&top_parts(record), &[128.0 * 64.0]);
            assert_eq!(tops_sampling_a_push(&section), [false], "{depth_mm} mm");
        }
        assert_parts_wholly_under_or_clear(&section, record);
        assert_eq!(t_junctions(record), Vec::<DVec3>::new(), "{depth_mm} mm");
    }
}

// Two cuts leave one part's corner inside another part's edge, and on the
// edges of the sides the push does not reach; every face gains the corners
// lying on its edges.
#[test]
fn a_push_over_one_corner_leaves_no_vertex_inside_another_parts_edge() {
    let corner = box_brush([96, 48, 0], [160, 96, 64], "push_tex");
    let (_, section) = compile(&distant_world(), &[channel(), push("0 8 0", &[corner])]);
    let record = &section.volumes[0];
    let parts = top_parts(record);
    assert_eq!(parts.len(), 3, "two cuts make three top parts: {parts:?}");
    let total: f64 = parts.iter().map(|(area, _)| area).sum();
    assert!((total - 128.0 * 64.0).abs() < 1e-2, "{parts:?}");
    assert_eq!(t_junctions(record), Vec::<DVec3>::new());
    assert_parts_wholly_under_or_clear(&section, record);
}

#[test]
fn an_inset_current_flush_with_the_waterline_leaves_no_t_junction() {
    let inset = box_brush([32, 16, 0], [96, 48, 64], "push_tex");
    let (_, section) = compile(&distant_world(), &[channel(), push("0 8 0", &[inset])]);
    let record = &section.volumes[0];
    assert!(top_parts(record).len() > 1, "the current splits the top");
    assert_eq!(t_junctions(record), Vec::<DVec3>::new());
    assert_parts_wholly_under_or_clear(&section, record);
}

// The sunk push splits the sides under it at the waterline but leaves the top
// whole: the top gains the corners the split sides leave on its edges.
#[test]
fn a_whole_top_gains_the_corners_its_split_sides_leave_on_its_edges() {
    let wind = box_brush_at([0.0, 0.0, 64.0 - 0.5 * MM], [64.0, 64.0, 128.0], "push_tex");
    let (_, section) = compile(&distant_world(), &[channel(), push("8 0 0", &[wind])]);
    let record = &section.volumes[0];
    let tops = top_faces(record);
    assert_eq!(tops.len(), 1, "the top draws whole");
    let top: Vec<DVec3> = tops[0].vertices.iter().map(|v| quake(v.position)).collect();
    assert_eq!(
        top.len(),
        6,
        "four corners and one side corner on each long edge: {top:?}"
    );
    for corner in [DVec3::new(64.0, 0.0, 64.0), DVec3::new(64.0, 64.0, 64.0)] {
        assert!(
            top.iter().any(|vertex| vertex.distance(corner) < 1e-2),
            "{corner} missing from {top:?}"
        );
    }
    assert_eq!(t_junctions(record), Vec::<DVec3>::new());
    assert_parts_wholly_under_or_clear(&section, record);
}

// A map with pushes and no fluid has no surface faces to split.
#[test]
fn push_volumes_without_fluid_emit_no_surface_faces() {
    let wind = push("8 0 0", &[box_brush([0, 0, 0], [64, 64, 64], "push_tex")]);
    let (_, section) = compile(&distant_world(), &[wind]);
    assert!(section.volumes[0].surface_faces.is_empty());
}

/// Every face of `record` lies wholly under or wholly clear of each push
/// brush in `section`, as the face's 2 mm inside samples see it, and the
/// centroid sample drift reads agrees: under when it is inside the push,
/// clear when it is not. Clear means one push plane has every vertex's sample
/// on or outside it, which decides it for the axis-aligned fixtures here.
#[track_caller]
fn assert_parts_wholly_under_or_clear(
    section: &EnvironmentVolumesSection,
    record: &EnvironmentVolumeRecord,
) {
    let brushes: Vec<Vec<(DVec3, f64)>> = section
        .volumes
        .iter()
        .filter(|other| other.push.is_some())
        .flat_map(|push| &push.brushes)
        .map(|brush| {
            brush
                .planes
                .iter()
                .map(|plane| {
                    (
                        DVec3::from(plane.normal.map(f64::from)),
                        f64::from(plane.distance),
                    )
                })
                .collect()
        })
        .collect();
    for face in &record.surface_faces {
        let normal = outward_normal(face);
        let samples: Vec<DVec3> = positions(face)
            .into_iter()
            .map(|position| position - normal * SAMPLE_DEPTH)
            .collect();
        let centroid = inside_sample(face);
        for planes in &brushes {
            let under = samples
                .iter()
                .all(|sample| planes.iter().all(|(n, d)| n.dot(*sample) - d <= EPSILON));
            let clear = planes
                .iter()
                .any(|(n, d)| samples.iter().all(|sample| n.dot(*sample) - d >= -EPSILON));
            let sampled_inside = planes.iter().all(|(n, d)| n.dot(centroid) - d <= 0.0);
            if sampled_inside {
                assert!(
                    under,
                    "a face sampling the push is not wholly under it: {face:?}"
                );
            } else {
                assert!(
                    clear,
                    "a face sampling no push is not wholly clear of it: {face:?}"
                );
            }
        }
    }
}
