//! Seam closure on the authored maps: every shared fluid boundary closes, and
//! closure moves nothing else.

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
