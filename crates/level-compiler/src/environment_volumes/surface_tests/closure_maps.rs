//! Seam closure on the authored maps: every shared fluid boundary closes, and
//! closure moves nothing else.

use super::closure::assert_nothing_else_moves;
use super::*;
use crate::environment_volumes::closure::{ClosureStats, close_seams};
use crate::environment_volumes::surface_pieces;

/// A dev map compiled before and after closure, with what closure did.
struct ClosedMap {
    unclosed: EnvironmentVolumesSection,
    closed: EnvironmentVolumesSection,
    stats: ClosureStats,
}

fn closed_map(name: &str) -> ClosedMap {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/maps")
        .join(format!("{name}.map"));
    let map = parse_map_file(&path, MapFormat::IdTech2).expect("dev map parses");
    let encode = |close: bool| {
        let mut texture_names = TextureNamesSection { names: Vec::new() };
        let encode = if close {
            encode_environment_volumes_section
        } else {
            encode_environment_volumes_section_unclosed
        };
        encode(
            &map.environment_volumes,
            map.initial_gravity,
            &map.brush_volumes,
            &mut texture_names,
        )
    };
    let mut pieces = surface_pieces(&map.environment_volumes, &map.brush_volumes);
    let stats = close_seams(&mut pieces);
    ClosedMap {
        unclosed: encode(false),
        closed: encode(true),
        stats,
    }
}

/// First slice: how many seams each map leaves open today, and what closing
/// them takes.
#[test]
#[ignore = "compiles the stress map; run on demand"]
fn report_open_seams_before_and_after_closure() {
    for name in [
        "e24-environment-test",
        "campaign-test",
        "reflection-probes-test",
        "vault-test",
        "stress-env-volumes",
    ] {
        let map = closed_map(name);
        let faces: usize = map
            .unclosed
            .volumes
            .iter()
            .map(|v| v.surface_faces.len())
            .sum();
        let before = open_seams(&map.unclosed);
        let corners = before
            .iter()
            .filter(|seam| {
                let (record, face) = seam.face;
                positions(&map.unclosed.volumes[record].surface_faces[face])
                    .iter()
                    .any(|corner| corner.distance(seam.vertex) <= SEAM_TOLERANCE)
            })
            .count();
        eprintln!(
            "SLICE {name}: {faces} faces; open before {} ({corners} near a corner), after {}; {:?}",
            before.len(),
            open_seams(&map.closed).len(),
            map.stats,
        );
    }
}

/// The maps the shared-boundary rows run on in this suite; e24 runs in the
/// routine suite.
const MAPS: [&str; 4] = [
    "campaign-test",
    "reflection-probes-test",
    "vault-test",
    "stress-env-volumes",
];

#[test]
#[ignore = "compiles the stress map; run on demand"]
fn authored_maps_close_every_seam_and_move_nothing_else() {
    for name in MAPS {
        let map = closed_map(name);
        assert_eq!(open_seams(&map.closed), Vec::new(), "{name}");
        assert_nothing_else_moves(&map.unclosed, &map.closed, 2.0 * SEAM_TOLERANCE);
    }
}

/// Indices of the push records whose brushes hold `point`: the same set
/// resolves the same winning push.
fn push_records_at(section: &EnvironmentVolumesSection, point: DVec3) -> Vec<usize> {
    (0..section.volumes.len())
        .filter(|&record| {
            section.volumes[record].push.is_some()
                && record_contains(&section.volumes[record], point)
        })
        .collect()
}

/// Where the app samples a face's drift push: its area centroid, 2 mm into
/// the fluid.
fn drift_sample(face: &EnvironmentSurfaceFace) -> Option<DVec3> {
    let normal = postretro_render_cpu::fluid_surface::face_normal(face)?;
    let centroid = postretro_render_cpu::fluid_surface::face_area_centroid(face, normal);
    Some(centroid - normal.as_dvec3() * SAMPLE_DEPTH)
}

/// Each face's drift map, packed ungridded with texel UVs, by (record, face).
fn drift_maps(
    section: &EnvironmentVolumesSection,
) -> Vec<Vec<Option<postretro_render_cpu::fluid_surface::FluidFaceDriftMap>>> {
    section
        .volumes
        .iter()
        .map(|record| {
            postretro_render_cpu::fluid_surface::pack_fluid_surfaces(
                std::slice::from_ref(record),
                &[],
                1,
                None,
            )
            .face_drift_maps
        })
        .collect()
}

/// The widest span of `polygon`, meters.
fn span(polygon: &[DVec3]) -> f64 {
    polygon
        .iter()
        .flat_map(|a| polygon.iter().map(move |b| a.distance(*b)))
        .fold(0.0, f64::max)
}

// Closure moves no face's drift push, drift map or probe binding: every face
// samples the same push records at its drift point, takes a map that predicts
// its UVs across it as the map before closure does, within two fits'
// tolerance, and binds the same probe, except a piece the weld tilts off
// up-facing, which spans under 6 mm (P14).
#[test]
#[ignore = "compiles the stress map; run on demand"]
fn closure_keeps_every_faces_drift_push_drift_map_and_probe_binding() {
    use crate::reflection_probes::placed_bindings;
    use crate::reflection_probes::placement::FaceGeometry;
    use postretro_render_cpu::fluid_surface::DRIFT_UV_TOLERANCE_TEXELS;

    for name in ["reflection-probes-test", "stress-env-volumes"] {
        let map = closed_map(name);
        let (before, after) = (&map.unclosed, &map.closed);
        let (maps_before, maps_after) = (drift_maps(before), drift_maps(after));
        let mut tilted = Vec::new();
        let mut worst_texels = 0.0f64;
        for (record, (was, now)) in before.volumes.iter().zip(&after.volumes).enumerate() {
            for (face, (old, new)) in was.surface_faces.iter().zip(&now.surface_faces).enumerate() {
                let sample = drift_sample(old).expect("a fixture face has a normal");
                let moved = drift_sample(new).expect("a closed face keeps a normal");
                assert_eq!(
                    push_records_at(before, sample),
                    push_records_at(after, moved),
                    "{name} record {record} face {face}: drift push"
                );
                if let (Some(old_map), Some(new_map)) =
                    (maps_before[record][face], maps_after[record][face])
                {
                    let origin = positions(new)[0];
                    for point in positions(new) {
                        for axis in 0..2 {
                            let gap = new_map.uv_gradient[axis].as_dvec3()
                                - old_map.uv_gradient[axis].as_dvec3();
                            worst_texels = worst_texels.max(gap.dot(point - origin).abs());
                        }
                    }
                } else {
                    assert_eq!(
                        maps_before[record][face].is_some(),
                        maps_after[record][face].is_some(),
                        "{name} record {record} face {face}: drift map"
                    );
                }
                let up = |points: &[DVec3]| {
                    FaceGeometry::from_polygon(points).is_some_and(|g| g.is_up_facing())
                };
                if up(&positions(old)) != up(&positions(new)) {
                    tilted.push((record as u32, face as u32));
                    assert!(
                        span(&positions(old)) < 6.0e-3,
                        "{name} record {record} face {face} tilts off up-facing"
                    );
                }
            }
        }
        assert!(
            worst_texels <= 2.0 * DRIFT_UV_TOLERANCE_TEXELS,
            "{name}: a drift map parts by {worst_texels} texels"
        );

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/dev/maps")
            .join(format!("{name}.map"));
        let parsed = parse_map_file(&path, MapFormat::IdTech2).expect("dev map parses");
        let world = crate::fixture_pipeline::load_fixture(name);
        let bound = |section: &EnvironmentVolumesSection| {
            let mut bindings: Vec<(u32, u32, u32)> =
                placed_bindings(section, &parsed.reflection_volumes, &world)
                    .into_iter()
                    .map(|binding| (binding.volume, binding.face, binding.probe))
                    .filter(|&(volume, face, _)| !tilted.contains(&(volume, face)))
                    .collect();
            bindings.sort_unstable();
            bindings
        };
        let bound_before = bound(before);
        assert!(!bound_before.is_empty(), "{name} binds no face");
        assert_eq!(bound_before, bound(after), "{name}: probe bindings");
        eprintln!(
            "{name}: {} bindings kept, {} tilted, drift maps within {worst_texels} texels",
            bound_before.len(),
            tilted.len()
        );
    }
}
