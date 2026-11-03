//! Reach cull on a chart whose texel centres drift off its face: the slab is
//! built from the walk's texel positions, so a lit texel whose drifted trace
//! start lies in a reached leaf keeps its pair. A slab built from the face
//! plane culls that pair, and verify mode catches the lit texel it zeroes.
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use glam::DMat3;

use super::reach_bake::{ReachRun, reach_bake_with, reach_off};
use super::*;
use crate::chart_raster::{chart_interior_dims, chart_texel_world_position};
use crate::lightmap_bake::{Chart, RAY_EPSILON};
use crate::lightmap_layer::SkipStats;
use crate::lightmap_reach::test_fixtures::{ReachFixture, point_light};
use crate::reflection_probes::placement_test_fixtures::{UNIT, box_brush, engine, sealed_room};

const DENSITY: f32 = 0.25;

/// Map-unit height of the mezzanine floor's top, and its thickness.
const SHEET_TOP: f64 = 64.0;
const SHEET: f64 = 0.5;

/// The chart frame's tilt, in radians, about the mezzanine's far edge:
/// texel centres drop below the sheet linearly toward its near wall, about
/// 6.5 cm at the wall. A real chart drifts the same way when its stored
/// normal is tilted off the face (`drafts/lightmap-chart-plane-drift/`),
/// about 3e-5 rad and 2.5 mm on an 82 m oblique face; the fixture scales
/// the tilt up so a 6.5 m chart drifts through a 1.27 cm sheet.
const TILT: f64 = 0.007;

/// A room 512 u long with a sealed mezzanine, room B, over its far half: a
/// 0.5 u floor sheet at z 64 and a wall at x 248..256 down to the sheet's
/// underside. Under the sheet is open and joins room A, where the light
/// sits just above the sheet's plane.
fn mezzanine() -> ReachFixture {
    let mut world = sealed_room([0.0, 0.0, 0.0], [512.0, 256.0, 160.0]);
    world.push(box_brush(
        [256.0, 0.0, SHEET_TOP - SHEET],
        [512.0, 256.0, SHEET_TOP],
    ));
    world.push(box_brush(
        [248.0, 0.0, SHEET_TOP - SHEET],
        [256.0, 256.0, 160.0],
    ));
    ReachFixture::compile(&world, &[])
}

/// A hard light 0.5 u above the sheet's plane: its emitter clears every
/// sheet slab, tilted or not, so only the slab's leaves decide the pair.
fn light() -> MapLight {
    let mut light = point_light(engine([8.0, 128.0, SHEET_TOP + 0.5]), 10.0);
    light.light_size = 0.0;
    light
}

/// The atlas index of room B's floor: the sheet's top.
fn sheet_chart(charts: &[Chart]) -> usize {
    let top = engine([0.0, 0.0, SHEET_TOP]).y as f32;
    let found: Vec<usize> = (0..charts.len())
        .filter(|&i| {
            let chart = &charts[i];
            let first = chart_texel_world_position(chart, 0, 0);
            chart.normal.y > 0.99 && (first.y - top).abs() < 1.0e-4 && first.z < -6.5
        })
        .collect();
    assert_eq!(found.len(), 1, "one mezzanine floor chart");
    found[0]
}

/// Tilt the sheet chart's frame by [`TILT`] about the sheet's far edge.
fn tilt(charts: &mut [Chart]) {
    let chart = &mut charts[sheet_chart(charts)];
    let pivot = engine([512.0, 0.0, SHEET_TOP]);
    let turn = DMat3::from_rotation_x(TILT);
    let rotate = |v: Vec3| (turn * v.as_dvec3()).as_vec3();
    chart.origin = (pivot + turn * (chart.origin.as_dvec3() - pivot)).as_vec3();
    chart.u_axis = rotate(chart.u_axis);
    chart.v_axis = rotate(chart.v_axis);
    chart.normal = rotate(chart.normal);
}

/// The fixture's charts, prepared as the stage prepares them: the face-plane
/// frames, before the tilt.
fn face_plane_charts(fixture: &ReachFixture) -> &'static [Chart] {
    Box::leak(fixture.atlas(&[light()], DENSITY).charts.into_boxed_slice())
}

/// Bake the tilted fixture with every exact skip, its reach slabs built from
/// `slab_charts` when given, and require: the sheet's centres drift past the
/// slab's lowest lift; a texel the unculled walk lights has its drifted trace
/// start in a leaf the flood reaches; the stage keeps the sheet's pair and
/// culls others; and bytes equal the unculled bake's, verify mode on.
fn check_drift(slab_charts: Option<&'static [Chart]>) -> anyhow::Result<()> {
    let fixture = mezzanine();
    let lights = [light()];
    let face_plane = face_plane_charts(&fixture);
    let sheet = sheet_chart(face_plane);
    let mut tilted = face_plane.to_vec();
    tilt(&mut tilted);
    let chart = &tilted[sheet];
    let (width, height) = chart_interior_dims(chart);
    let top = engine([0.0, 0.0, SHEET_TOP]).y;
    let drift = (0..height)
        .flat_map(|ty| (0..width).map(move |tx| (tx, ty)))
        .map(|(tx, ty)| top - f64::from(chart_texel_world_position(chart, tx, ty).y))
        .fold(f64::NEG_INFINITY, f64::max);
    anyhow::ensure!(
        drift > 3.0 * f64::from(RAY_EPSILON),
        "centres drift {drift} m below the face, past the slab's lowest lift"
    );

    // The drift alone decides the pair: a slab off the face plane would cull
    // it, one off the walk's texel positions reaches the open space under
    // the sheet.
    let index = fixture.index(&lights, &tilted, true);
    let flat = fixture.index(&lights, face_plane, true);
    anyhow::ensure!(
        flat.culled_chart(0, sheet, &face_plane[sheet]).is_some(),
        "a face-plane slab culls the sheet's pair"
    );
    anyhow::ensure!(
        index.culled_chart(0, sheet, chart).is_none()
            && index
                .forced_chart(sheet, chart)
                .expect("an orthonormal frame")
                .open_leaves()
                .iter()
                .any(|&leaf| index.reaches(0, leaf as usize) == Some(true)),
        "the drifted slab reaches a reached leaf"
    );
    let stats = SkipStats::leak();
    for floor in [true, false] {
        let run = |exact_skips: lightmap_layer::ExactSkips| {
            let mut run = ReachRun::new(floor, exact_skips, 4);
            run.edit_charts = Some(&tilt);
            reach_bake_with(&fixture, &lights, &run)
        };
        let unculled = run(lightmap_layer::ExactSkips {
            stats: Some(stats),
            ..reach_off()
        })?;
        let walk = stats
            .charts()
            .into_iter()
            .rfind(|tally| tally.face == sheet)
            .expect("the sheet's pair is walked");
        let lit_below = walk.texels.iter().any(|texel| {
            let (tx, ty) = texel.texel;
            let start = chart_texel_world_position(chart, tx, ty) + chart.normal * RAY_EPSILON;
            texel.visibility.is_some_and(|v| v > 0.0)
                && start.as_dvec3().y < top - SHEET * UNIT
                && index.reaches(0, fixture.leaf_at(start.as_dvec3())) == Some(true)
        });
        anyhow::ensure!(
            lit_below,
            "floor {floor}: a lit texel's drifted trace start lies under the sheet, reached"
        );

        let culled = run(lightmap_layer::ExactSkips {
            stats: Some(stats),
            slab_charts,
            ..lightmap_layer::ExactSkips::ON
        })?;
        let counts = culled.counts;
        anyhow::ensure!(counts.culled > 0, "floor {floor}: {counts:?}");
        anyhow::ensure!(counts.rewalked == counts.culled);
        anyhow::ensure!(
            !stats
                .charts()
                .iter()
                .any(|tally| tally.face == sheet && tally.culled),
            "floor {floor}: the sheet's pair is kept"
        );
        anyhow::ensure!(
            culled.lightmap == unculled.lightmap && culled.shadowmask == unculled.shadowmask,
            "floor {floor}: bytes differ with the reach cull off"
        );
    }
    Ok(())
}

// A chart drifting up to 6.5 cm below its face, through a 1.27 cm sheet into
// the open space under it, which the light reaches: the slab, built from the
// walk's texel positions, holds the drifted trace starts, so the pair is
// kept and bytes equal the unculled bake's.
#[test]
fn reach_keeps_a_drifted_chart_whose_lit_trace_starts_lie_in_a_reached_leaf() {
    check_drift(None).expect("the drift fixture holds");
}

// Negative control: the same fixture with each slab built from the face
// plane, where the untilted frames put the texel centres. The slab misses
// the drifted trace starts under the sheet, the pair is culled, and verify
// mode fails the bake on a lit texel it zeroed.
#[test]
fn reach_drift_check_fails_with_slabs_built_from_the_face_plane() {
    let fixture = mezzanine();
    let error = check_drift(Some(face_plane_charts(&fixture)))
        .expect_err("a face-plane slab misses the drifted trace starts");
    let error = format!("{error:#}");
    assert!(error.contains("lightmap reach verify"), "{error}");
    assert!(
        error.contains("the culled walk stored 0.0,"),
        "a lit texel is zeroed: {error}"
    );
}
