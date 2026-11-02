// Shared-edge census on the gate maps' pre-cut and post-cut geometry, and on hand-built edges.
// See: context/lib/build_pipeline.md §Ray traversal

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use glam::{DVec3, Vec3};
use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;

use super::census::{
    Census, GAP_ACCURACY_METERS, MismatchedEdge, SEARCH_RADII_METERS, T_JUNCTION_ULPS, census,
};
use super::test_fixtures::triangles_geometry;
use crate::buried_lights::BuriedLights;
use crate::geometry::GeometryResult;
use crate::light_namespaces::StaticBakedLights;
use crate::pipeline::runtime_cells;

/// Maps [`shared_edge_census_on_gate_maps`] measures unless
/// `POSTRETRO_CENSUS_MAPS` names others (comma-separated, under
/// `content/dev/maps/`).
const GATE_MAPS: [&str; 8] = [
    "stress-warren-hallway-inspection",
    "stress-warren-hallway-inspection-mini",
    "movement-feel",
    "kinematic-platform",
    "gate-heavily-lit",
    "combat-demo",
    "soft_shadow_test",
    "reflection-probes-test",
];

/// A mismatched edge no wider than this reads as a crease crack rather
/// than an open boundary.
const CRACK_LIMIT_METERS: f64 = 5e-3;

/// Upper ends of the gap bands after "exactly 0"; the last band is open.
const BAND_TOPS_METERS: [f64; 7] = [1e-6, 1e-5, 1e-4, 2.5e-4, 5e-4, 1e-3, 5e-3];
const BAND_LABELS: [&str; 9] = [
    "exactly 0",
    "(0, 1 µm]",
    "(1, 10 µm]",
    "(10, 100 µm]",
    "(100 µm, 0.25 mm]",
    "(0.25, 0.5 mm]",
    "(0.5, 1 mm]",
    "(1, 5 mm]",
    "> 5 mm",
];

/// The census of every gate map, from geometry alone: no bake runs. Each
/// map's report prints, and lands in `POSTRETRO_CENSUS_OUT/<map>.txt` when
/// that directory is set. Run with `--release --nocapture`.
#[test]
#[ignore = "builds gate-map geometry cold; run on demand with --release --nocapture"]
fn shared_edge_census_on_gate_maps() {
    let maps: Vec<String> = match std::env::var("POSTRETRO_CENSUS_MAPS") {
        Ok(list) => list.split(',').map(|m| m.trim().to_owned()).collect(),
        Err(_) => GATE_MAPS.iter().map(|&m| m.to_owned()).collect(),
    };
    let out_dir = std::env::var_os("POSTRETRO_CENSUS_OUT").map(std::path::PathBuf::from);
    for map in maps {
        let report = census_report(&map);
        println!("{report}");
        if let Some(dir) = &out_dir {
            std::fs::create_dir_all(dir).expect("census output dir");
            std::fs::write(dir.join(format!("{map}.txt")), &report).expect("census report");
        }
    }
}

/// The two geometries the bake tracers trace, built by the pipeline's own
/// stage functions in its order: the pre-cut set (SH, delta and direct SH,
/// the chunk light list, the emitter tests) and the post-cut set (lightmap,
/// shadowmask, weight maps, reflection probes).
struct MapGeometries {
    pre_cut: GeometryResult,
    post_cut: GeometryResult,
    pre_cut_brushes: Vec<Option<u32>>,
    post_cut_brushes: Vec<Option<u32>>,
    /// Post-cut face → pre-cut face, when a face was cut.
    face_parent: Option<Vec<usize>>,
    lightmap_density: f32,
    built_in: Duration,
}

/// Parse → partition with occluders → buried lights → data script and its
/// membership manifest → static light namespace → runtime cells → geometry,
/// then atlas preparation's chart planning and oversize-face cut, with the
/// defaults `prl-build <map> --release` uses.
fn map_geometries(map: &str) -> MapGeometries {
    let started = Instant::now();
    let input = crate::fixture_pipeline::fixture_path(map);
    let argv = [
        input.to_str().expect("UTF-8 path").to_owned(),
        "--release".to_owned(),
        "--no-tui".to_owned(),
    ];
    let args = crate::parse_args_from(argv.into_iter()).expect("arguments parse");
    let mut map_data = crate::parse::parse_map_file(&args.input, args.format).expect("map parses");
    let runtime_cells::OccluderPartition { result, tier } =
        runtime_cells::partition_with_occluders(
            &map_data.brush_volumes,
            &map_data.kinematic_movers,
        )
        .expect("map partitions");
    let buried_lights = BuriedLights::classify(&result.tree, &map_data.lights);
    // Membership can turn static lights animated, and a map with no static
    // light cuts nothing, so the manifest applies before the namespace forms.
    let compiled = crate::compile_worldspawn_data_script(
        &args.input,
        map_data.data_script.as_deref(),
        &map_data.lights,
        &buried_lights,
        crate::script_light_membership::map_members_from_map(
            &map_data.kinematic_movers,
            &map_data.trigger_volumes,
            &map_data.map_entities,
        ),
    )
    .expect("data script compiles");
    if let Some(script) = compiled {
        crate::script_light_membership::apply_manifest(
            &mut map_data.lights,
            &map_data.light_start_active_defaults,
            &script.membership_manifest,
        )
        .expect("membership applies");
    }
    let static_lights = StaticBakedLights::from_lights_excluding(&map_data.lights, &buried_lights);
    let (runtime, _) = runtime_cells::RuntimeCells::of_partition(&result.tree, &tier);
    let (mut post_cut, pre_cut_brushes) = crate::geometry::extract_geometry_with_brushes(
        &result.faces,
        &runtime.cells.tree,
        &runtime.cells.exterior_cells,
    );
    let pre_cut = post_cut.clone();
    let config = crate::lightmap_config(&args, map_data.lightmap_density);
    let cut = crate::lightmap_bake::plan_cut_charts(
        &mut post_cut,
        &static_lights,
        config.lightmap_density,
        &map_data.lightmap_scale_regions,
        LIGHTMAP_POOL_LAYER_EDGE,
        args.working_set_max_bytes,
        config.uncompressed_irradiance,
    )
    .expect("charts plan");
    let (post_cut_brushes, face_parent) = match &cut.face_remap {
        Some(remap) => {
            let mut parent = vec![usize::MAX; remap.last().map_or(0, |r| r.end)];
            for (old, range) in remap.iter().enumerate() {
                parent[range.clone()].fill(old);
            }
            (
                crate::geometry::remap_face_brushes(&pre_cut_brushes, remap),
                Some(parent),
            )
        }
        None => (pre_cut_brushes.clone(), None),
    };
    MapGeometries {
        pre_cut,
        post_cut,
        pre_cut_brushes,
        post_cut_brushes,
        face_parent,
        lightmap_density: config.lightmap_density,
        built_in: started.elapsed(),
    }
}

fn census_report(map: &str) -> String {
    let geometries = map_geometries(map);
    let mut out = String::new();
    let _ = writeln!(out, "== shared-edge census: {map} ==");
    let cut_faces = geometries.face_parent.as_ref().map_or(0, |parent| {
        let mut sizes = vec![0usize; geometries.pre_cut.geometry.faces.len()];
        for &old in parent {
            sizes[old] += 1;
        }
        sizes.iter().filter(|&&n| n != 1).count()
    });
    let _ = writeln!(
        out,
        "lightmap density {} m/texel; oversize cut: {} face(s) cut, {} → {} faces; geometry built in {:.3} s",
        geometries.lightmap_density,
        cut_faces,
        geometries.pre_cut.geometry.faces.len(),
        geometries.post_cut.geometry.faces.len(),
        geometries.built_in.as_secs_f64(),
    );
    let _ = writeln!(
        out,
        "gap: largest distance from a point of the edge to the nearest triangle of another face, f64 branch and bound to {:.1} µm; search radii {:?} m",
        GAP_ACCURACY_METERS * 1e6,
        SEARCH_RADII_METERS,
    );
    let _ = writeln!(
        out,
        "T-junction: an unshared edge whose shorter partner's endpoints lie within {T_JUNCTION_ULPS} f32 ulps of the largest endpoint coordinate (at least 1 m) of another face's triangle edge line, overlapping it by more than that",
    );
    for (name, geometry, brushes, parent) in [
        (
            "pre-cut (SH, delta and direct SH, chunk light list, emitter tests)",
            &geometries.pre_cut,
            &geometries.pre_cut_brushes,
            None,
        ),
        (
            "post-cut (lightmap, shadowmask, weight maps, reflection probes)",
            &geometries.post_cut,
            &geometries.post_cut_brushes,
            geometries.face_parent.as_deref(),
        ),
    ] {
        let started = Instant::now();
        let result = census(geometry, parent);
        let elapsed = started.elapsed();
        let labels = FaceLabels { geometry, brushes };
        write_geometry_report(&mut out, name, &result, &labels, parent);
        let _ = writeln!(out, "census time: {:.3} s", elapsed.as_secs_f64());
    }
    out
}

struct FaceLabels<'a> {
    geometry: &'a GeometryResult,
    brushes: &'a [Option<u32>],
}

impl FaceLabels<'_> {
    fn describe(&self, face: u32) -> String {
        let meta = &self.geometry.geometry.faces[face as usize];
        let texture = self
            .geometry
            .texture_names
            .names
            .get(meta.texture_index as usize)
            .map_or("?", String::as_str);
        let brush = self.brushes[face as usize].map_or("?".to_owned(), |b| b.to_string());
        format!(
            "face {face} [{texture}, brush {brush}, cell {}]",
            meta.leaf_index
        )
    }
}

fn write_geometry_report(
    out: &mut String,
    name: &str,
    census: &Census,
    labels: &FaceLabels<'_>,
    parent: Option<&[usize]>,
) {
    let t_junctions = census
        .mismatched
        .iter()
        .filter(|e| e.t_junction_with.is_some())
        .count();
    let others = census.mismatched.len() - t_junctions;
    let siblings = |a: u32, b: u32| parent.is_some_and(|p| p[a as usize] == p[b as usize]);
    let sibling_t_junctions = census
        .mismatched
        .iter()
        .filter(|e| e.t_junction_with.is_some_and(|f| siblings(e.face, f)))
        .count();
    let rounded_apart = census.mismatched.iter().filter(|e| e.rounded_apart).count();
    let _ = writeln!(out, "\n-- {name} --");
    let _ = writeln!(
        out,
        "triangles {}, faces {}, triangle edges {}",
        census.triangles, census.faces, census.edges
    );
    let _ = writeln!(
        out,
        "degenerate edges (bit-identical endpoints): {}",
        census.degenerate
    );
    let _ = writeln!(
        out,
        "shared bit-identically: {} (within a face {}, across faces {} [between cut siblings {}]; with more than one sharer {})",
        census.shared_within_face + census.shared_across_faces,
        census.shared_within_face,
        census.shared_across_faces,
        census.shared_across_cut_siblings,
        census.shared_non_manifold,
    );
    let _ = writeln!(
        out,
        "mismatched: {} (T-junctions {} [partner a cut sibling {}; a partner matching end for end, one edge rounded apart, {}], other {})",
        census.mismatched.len(),
        t_junctions,
        sibling_t_junctions,
        rounded_apart,
        others,
    );
    let _ = writeln!(
        out,
        "widest T-junction threshold used: {:.3} µm",
        census.widest_t_junction_threshold * 1e6
    );
    let unconverged = census
        .mismatched
        .iter()
        .filter(|e| !e.gap.converged)
        .count();
    let _ = writeln!(out, "unconverged gaps: {unconverged}");

    let mut bands = [[0usize; 2]; BAND_LABELS.len()];
    for edge in &census.mismatched {
        let column = usize::from(edge.t_junction_with.is_none());
        bands[band(edge)][column] += 1;
    }
    let _ = writeln!(
        out,
        "{:<20} {:>12} {:>12}",
        "widest gap band", "T-junction", "other"
    );
    for (label, [t, o]) in BAND_LABELS.iter().zip(bands) {
        let _ = writeln!(out, "{label:<20} {t:>12} {o:>12}");
    }

    let mut by_width: Vec<&MismatchedEdge> = census.mismatched.iter().collect();
    by_width.sort_by(|a, b| {
        b.gap
            .width()
            .total_cmp(&a.gap.width())
            .then(b.gap.upper.total_cmp(&a.gap.upper))
            .then(a.triangle.cmp(&b.triangle))
    });
    match by_width
        .iter()
        .find(|e| e.gap.beyond.is_none() && e.gap.width() <= CRACK_LIMIT_METERS)
    {
        Some(edge) => {
            let _ = writeln!(out, "widest crease crack (≤ 5 mm):");
            let _ = writeln!(out, "  {}", describe_edge(edge, labels));
        }
        None => {
            let _ = writeln!(out, "widest crease crack (≤ 5 mm): none");
        }
    }
    let _ = writeln!(out, "10 widest mismatched edges:");
    for (rank, edge) in by_width.iter().take(10).enumerate() {
        let _ = writeln!(out, "  {:>2}. {}", rank + 1, describe_edge(edge, labels));
    }
}

fn band(edge: &MismatchedEdge) -> usize {
    let width = edge.gap.width();
    if edge.gap.beyond.is_some() {
        return BAND_LABELS.len() - 1;
    }
    if width == 0.0 {
        return 0;
    }
    BAND_TOPS_METERS
        .iter()
        .position(|&top| width <= top)
        .map_or(BAND_LABELS.len() - 1, |i| i + 1)
}

fn length(meters: f64) -> String {
    if meters >= 1e-3 {
        format!("{:.4} mm", meters * 1e3)
    } else {
        format!("{:.3} µm", meters * 1e6)
    }
}

fn point(p: DVec3) -> String {
    format!("({:.6}, {:.6}, {:.6})", p.x, p.y, p.z)
}

fn describe_edge(edge: &MismatchedEdge, labels: &FaceLabels<'_>) -> String {
    let gap = &edge.gap;
    let width = match gap.beyond {
        Some(radius) => format!("> {radius} m (bound {})", length(gap.upper)),
        None => format!("{} (≤ {})", length(gap.width()), length(gap.upper)),
    };
    let kind = match edge.t_junction_with {
        Some(face) if edge.rounded_apart => {
            format!("T-junction with face {face}, rounded apart end for end")
        }
        Some(face) => format!("T-junction with face {face}"),
        None => "other".to_owned(),
    };
    let nearest = gap
        .nearest_face
        .map_or("none within radius".to_owned(), |f| labels.describe(f));
    let [p, q] = edge.endpoints;
    format!(
        "gap {width}{}, {kind}; {} triangle {}; edge {} → {}, length {:.4} m; widest at {}, nearest {}",
        if gap.converged { "" } else { " UNCONVERGED" },
        labels.describe(edge.face),
        edge.triangle,
        point(p),
        point(q),
        (q - p).length(),
        point(gap.at),
        nearest,
    )
}

fn edge_record(census: &Census, p: Vec3, q: Vec3) -> &MismatchedEdge {
    let (p, q) = (p.as_dvec3(), q.as_dvec3());
    census
        .mismatched
        .iter()
        .find(|e| e.endpoints == [p, q] || e.endpoints == [q, p])
        .expect("edge is mismatched")
}

/// Shared edges count shared; a T-junction counts as one with a gap of
/// rounding; a crease whose edges miss by d reports d. The crease's gap
/// peaks mid-edge, between two triangles that each touch one end, so only
/// the branch and bound finds it.
#[test]
fn census_counts_shared_edges_and_t_junctions_and_measures_a_crease_gap() {
    let v = Vec3::new;

    let shared = census(
        &triangles_geometry(&[
            [v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0)],
            [v(1.0, 0.0, 0.0), v(1.0, 1.0, 0.0), v(0.0, 1.0, 0.0)],
        ]),
        None,
    );
    assert_eq!(shared.edges, 6);
    assert_eq!(shared.shared_across_faces, 2);
    assert_eq!(shared.shared_within_face, 0);
    assert_eq!(shared.mismatched.len(), 4);
    // An open boundary: the quad's corner lies √2 / 2 from the other
    // triangle, past every radius below 1 m.
    let open = edge_record(&shared, v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0));
    assert_eq!(open.t_junction_with, None);
    assert!((open.gap.width() - 0.5f64.sqrt()).abs() <= GAP_ACCURACY_METERS);
    assert_eq!(open.gap.beyond, None);

    // Wall face 0 runs a → b; floor faces 1 and 2 meet it at x, rounded to
    // f32 off the line, a vertex the wall lacks.
    let (a, b) = (v(0.1, 0.2, 0.3), v(2.7, 1.9, 0.3));
    let x = (a.as_dvec3() + (b - a).as_dvec3() / 3.0).as_vec3();
    let out = v(-0.5, 0.8, 0.0);
    let junction = census(
        &triangles_geometry(&[[a, b, v(0.1, 0.2, 1.3)], [a, x, a + out], [x, b, b + out]]),
        None,
    );
    for (p, q) in [(a, x), (x, b), (a, b)] {
        let edge = edge_record(&junction, p, q);
        assert!(edge.t_junction_with.is_some(), "{p} → {q} is a T-junction");
        assert!(!edge.rounded_apart, "{p} → {q} meets a longer edge");
        assert!(edge.gap.upper < 1e-6, "{p} → {q} gap {:?}", edge.gap);
    }

    // Face 0's edge runs (0,0,0) → (2,0,0). Faces 1 and 2 are walls whose
    // bottom edges leave it at slope s from either end, so the gap is
    // min(x, 2 − x)·s/√(1 + s²), widest at x = 1. Every corner is dyadic,
    // exact in f32.
    let s = 2.0f32.powi(-12);
    let crease = census(
        &triangles_geometry(&[
            [v(0.0, 0.0, 0.0), v(2.0, 0.0, 0.0), v(0.0, 0.0, 1.0)],
            [
                v(-0.5, 0.0, 0.5 * s),
                v(3.0, 0.0, -3.0 * s),
                v(-0.5, 1.0, 0.5 * s),
            ],
            [
                v(2.5, 0.0, 0.5 * s),
                v(-1.0, 0.0, -3.0 * s),
                v(2.5, 1.0, 0.5 * s),
            ],
        ]),
        None,
    );
    let d = f64::from(s) / (1.0 + f64::from(s).powi(2)).sqrt();
    let edge = edge_record(&crease, v(0.0, 0.0, 0.0), v(2.0, 0.0, 0.0));
    assert_eq!(edge.t_junction_with, None, "a crease is not a T-junction");
    assert!(edge.gap.converged);
    assert!(
        (edge.gap.width() - d).abs() <= GAP_ACCURACY_METERS,
        "gap {} against {d}",
        edge.gap.width()
    );
    assert!(edge.gap.upper - edge.gap.width() <= GAP_ACCURACY_METERS);
    assert!(
        (edge.gap.at.x - 1.0).abs() < 1e-3,
        "widest at {}",
        edge.gap.at
    );
}
