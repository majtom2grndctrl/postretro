use super::classification_test_fixtures::{
    TierBuild, assert_apart, brush_at_quake_box, classification, occlusion_test_brushes,
};
use crate::map_data::BrushVolume;
use crate::partition::TierRole;
use crate::partition::occluder_classify::test_fixtures::Edge;
use crate::partition::occluder_classify::{OccluderClassification, OccluderVerdict};
use crate::partition::occluder_cluster::test_fixtures::{
    self as fixtures, ClusterFixture, STEP_RUN_STEPS,
};
use crate::partition::occluder_cluster::{OCCLUDER_COVERAGE_FRACTION, Section};

const TURN: f64 = std::f64::consts::FRAC_PI_4;

/// Classify `cf`, assert each brush the row adds is exempt alone, and return
/// the classification.
fn classify_exempt_added(cf: &ClusterFixture) -> OccluderClassification {
    let classification = classification(&cf.fixture.brushes);
    for brush in cf.added() {
        assert_ne!(
            classification.verdicts[brush],
            OccluderVerdict::Occludes,
            "{}: brush {brush} occludes alone",
            cf.fixture.name
        );
    }
    classification
}

/// Sections holding every one of `brushes`.
fn sections_holding(
    classification: &OccluderClassification,
    brushes: impl IntoIterator<Item = usize> + Clone,
) -> Vec<&Section> {
    classification
        .clusters
        .iter()
        .flat_map(|cluster| &cluster.sections)
        .filter(|section| {
            brushes
                .clone()
                .into_iter()
                .all(|brush| section.members.contains(&brush))
        })
        .collect()
}

/// The fixture's occluder-tier interior leaves and tier portals match the
/// room without the row's brushes.
fn assert_adds_no_empty_cell_or_portal(cf: &ClusterFixture) {
    let with = TierBuild::classified(&cf.fixture.brushes).interior_cells_and_portals();
    let without = TierBuild::classified(&cf.without()).interior_cells_and_portals();
    assert_eq!(
        with, without,
        "{}: (interior cells, portals) with the row's brushes vs without",
        cf.fixture.name
    );
}

/// No brush the row adds reaches the occluder tier.
fn assert_added_stay_exempt(cf: &ClusterFixture, classification: &OccluderClassification) {
    let flags = classification.occluder_flags();
    for brush in cf.added() {
        assert!(
            !flags[brush],
            "{}: brush {brush} reaches the occluder tier",
            cf.fixture.name
        );
    }
}

// A3, the cluster half (pin R30): occlusion-test's first-room east wall.

#[test]
fn occlusion_test_east_wall_occludes_as_one_cluster_section() {
    // The doorway's two jambs and lintel are each exempt alone; their
    // section, 6.5 × 3.25 m, covers 81% of its face and occludes.
    let brushes = occlusion_test_brushes();
    let classification = classification(&brushes);
    let wall = [
        brush_at_quake_box(&brushes, [256.0, 0.0, 0.0], [272.0, 96.0, 128.0]),
        brush_at_quake_box(&brushes, [256.0, 160.0, 0.0], [272.0, 256.0, 128.0]),
        brush_at_quake_box(&brushes, [256.0, 96.0, 96.0], [272.0, 160.0, 128.0]),
    ];
    for brush in wall {
        assert_ne!(classification.verdicts[brush], OccluderVerdict::Occludes);
    }
    let sections = sections_holding(&classification, wall);
    assert!(
        sections.iter().any(|section| section.occludes),
        "no occluding section holds the east wall: {sections:#?}"
    );
    let flags = classification.occluder_flags();
    assert!(wall.iter().all(|&brush| flags[brush]));

    report("occlusion-test", &brushes, &classification);
}

/// Print the classifier's cluster output and the tier counts it makes,
/// beside the counts brushes judged alone make.
fn report(name: &str, brushes: &[BrushVolume], classification: &OccluderClassification) {
    let alone_roles: Vec<TierRole> = classification
        .verdicts
        .iter()
        .map(|&verdict| match verdict {
            OccluderVerdict::Occludes => TierRole::Occluder,
            _ => TierRole::Exempt,
        })
        .collect();
    let alone: Vec<bool> = alone_roles
        .iter()
        .map(|r| *r == TierRole::Occluder)
        .collect();
    let flags = classification.occluder_flags();
    let sections: Vec<&Section> = classification
        .clusters
        .iter()
        .flat_map(|cluster| &cluster.sections)
        .collect();
    let occluding = sections.iter().filter(|section| section.occludes).count();
    let promoted = (0..brushes.len())
        .filter(|&i| flags[i] && !alone[i])
        .count();
    let alone_build = TierBuild::new(brushes, &alone_roles);
    let cluster_build = TierBuild::new(brushes, &classification.tier_roles());
    let tier_leaves = |build: &TierBuild| build.tier.leaves.len();
    let today = super::classification_test_fixtures::FirstPass::of(brushes);
    println!(
        "{name}: {} brushes, {} occlude alone; {} clusters ({} brushes), {} sections, \
         {occluding} occlude, {promoted} brushes promoted\n\
         today:    {} leaves, {} portals\n\
         alone:    {} tier leaves, (interior, portals) {:?}, full tree {} leaves\n\
         clusters: {} tier leaves, (interior, portals) {:?}, full tree {} leaves",
        brushes.len(),
        alone.iter().filter(|&&f| f).count(),
        classification.clusters.len(),
        classification
            .clusters
            .iter()
            .map(|cluster| cluster.members.len())
            .sum::<usize>(),
        sections.len(),
        today.tree.leaves.len(),
        crate::portals::generate_portals(&today.tree).len(),
        tier_leaves(&alone_build),
        alone_build.interior_cells_and_portals(),
        alone_build.tree.leaves.len(),
        tier_leaves(&cluster_build),
        cluster_build.interior_cells_and_portals(),
        cluster_build.tree.leaves.len(),
    );
    for section in sections {
        println!(
            "  section {:?} n={:.3?} extents {:.2?} box {:?} coverage {:.3} occludes {}",
            section.members,
            section.normal,
            section.extents,
            section.box_verdict,
            section.coverage,
            section.occludes
        );
    }
}

// A7, the coverage half (pin R12): coverage is strict.

#[test]
fn cluster_coverage_decides_strictly_at_the_constant() {
    for edge in Edge::ALL {
        let cf = fixtures::coverage_edge(edge);
        let classification = classify_exempt_added(&cf);
        let sections = sections_holding(&classification, cf.added());
        assert_eq!(sections.len(), 1, "{}: {sections:#?}", cf.fixture.name);
        let section = sections[0];
        assert_eq!(
            section.box_verdict,
            OccluderVerdict::Occludes,
            "{}: the bounding face must pass the occluder rule",
            cf.fixture.name
        );
        match edge {
            Edge::Under | Edge::At => {
                if edge == Edge::At {
                    assert_eq!(section.coverage, OCCLUDER_COVERAGE_FRACTION);
                }
                assert!(!section.occludes, "{}", cf.fixture.name);
                assert_adds_no_empty_cell_or_portal(&cf);
            }
            Edge::Over => {
                assert!(section.occludes, "{}", cf.fixture.name);
                assert_apart(&cf.fixture);
            }
        }
    }
}

// A11 (pins R13, R23): a strip facade bounds cells; spaced strips add none.

#[test]
fn strip_facade_bounds_cells_as_one_section_square_and_turned_45_degrees() {
    for cf in [
        fixtures::strip_facade(),
        fixtures::strip_facade().turned(TURN),
    ] {
        let classification = classify_exempt_added(&cf);
        let sections = sections_holding(&classification, cf.added());
        assert_eq!(
            sections.len(),
            1,
            "{}: one section holds every strip",
            cf.fixture.name
        );
        assert!(
            sections[0].occludes,
            "{}: {:#?}",
            cf.fixture.name, sections[0]
        );
        assert_apart(&cf.fixture);
    }
}

#[test]
fn strip_facade_windows_join_the_cell_behind_and_cross_through_one_portal_each() {
    // Owner ruling, one portal per window (plan.md §Block (Task 9),
    // resolved). The facade's slab is z 20..20.3 in a hall 40 m deep; its
    // -z face fronts the deeper space, so it bounds the section, and each
    // window's air joins the cell behind it. Before, each window was its own
    // cell with a portal on both faces: twice the windows.
    let cf = fixtures::strip_facade();
    let classification = classify_exempt_added(&cf);
    let sections = sections_holding(&classification, cf.added());
    assert!(sections[0].occludes, "{:#?}", sections[0]);
    assert_eq!(sections[0].bounding_face, glam::DVec3::NEG_Z);
    assert_apart(&cf.fixture);

    let build = TierBuild::classified(&cf.fixture.brushes);
    let (x0, z0, z1) = (15.0, 20.0, 20.0 + 0.3);
    let windows = fixtures::facade_windows(31.0, 15.0);
    let behind = build.cell_at(glam::DVec3::new(30.5, 1.5, 25.0));
    let tier_portals = crate::portals::occluder_tier::generate_occluder_tier_portals(
        &build.tree,
        &build.tier,
        &build.exterior,
    );
    let on_facade: Vec<glam::DVec3> = tier_portals
        .iter()
        .filter(|portal| {
            portal
                .polygon
                .iter()
                .all(|p| (p.z - z0).abs() < 1e-6 || (p.z - z1).abs() < 1e-6)
        })
        .map(|portal| portal.polygon.iter().sum::<glam::DVec3>() / portal.polygon.len() as f64)
        .filter(|c| c.x > x0 && c.x < x0 + 31.0 && c.y > 0.0 && c.y < 15.0)
        .collect();
    assert_eq!(
        on_facade.len(),
        windows.len(),
        "runtime portals across the facade, one per window"
    );
    for [a0, a1, y0, y1] in windows {
        let centre = glam::DVec3::new(x0 + (a0 + a1) / 2.0, (y0 + y1) / 2.0, (z0 + z1) / 2.0);
        assert_eq!(
            build.cell_at(centre),
            behind,
            "window at {centre} is not in the cell behind the facade"
        );
        let inside = |c: &&glam::DVec3| c.x > x0 + a0 && c.x < x0 + a1 && c.y > y0 && c.y < y1;
        assert_eq!(
            on_facade.iter().filter(inside).count(),
            1,
            "window at {centre}"
        );
    }
}

#[test]
fn strips_touching_only_through_the_floor_add_no_empty_cell_square_and_turned_45_degrees() {
    for cf in [
        fixtures::spaced_strips(),
        fixtures::spaced_strips().turned(TURN),
    ] {
        let classification = classify_exempt_added(&cf);
        assert!(
            classification.clusters.is_empty(),
            "{}: the occluding floor joins no cluster",
            cf.fixture.name
        );
        assert_adds_no_empty_cell_or_portal(&cf);
    }
}

// A12 (pin R26): near-planar sections.

#[test]
fn corner_facade_bounds_cells_on_each_arm() {
    // As one cluster its bounding face is the footprint and covers a few
    // percent; each arm is its own section.
    let cf = fixtures::corner_facade();
    let classification = classify_exempt_added(&cf);
    assert_eq!(
        classification.clusters.len(),
        1,
        "the arms touch at the corner"
    );
    let occluding = classification.clusters[0]
        .sections
        .iter()
        .filter(|section| section.occludes)
        .count();
    assert_eq!(occluding, 2, "{:#?}", classification.clusters[0].sections);
    assert_apart(&cf.fixture);
}

#[test]
fn facades_joined_by_a_touching_sign_each_bound_cells() {
    let cf = fixtures::joined_facades();
    let classification = classify_exempt_added(&cf);
    let sign = cf.base;
    assert_eq!(
        classification.clusters.len(),
        1,
        "the sign joins both facades"
    );
    assert!(
        classification.clusters[0].members.contains(&sign),
        "the sign is a cluster member"
    );
    let occluding: Vec<&Section> = classification.clusters[0]
        .sections
        .iter()
        .filter(|section| section.occludes)
        .collect();
    assert_eq!(occluding.len(), 2, "{occluding:#?}");
    assert_added_stay_exempt(&cf, &classification);
    assert_apart(&cf.fixture);
}

// A13: sparse clusters whose bounding face passes the rule add no cell.

#[test]
fn l_lamp_post_in_an_alcove_adds_no_empty_cell() {
    assert_sparse_section_adds_no_empty_cell(&fixtures::lamp_post_in_alcove());
}

#[test]
fn colonnade_under_a_touching_beam_adds_no_empty_cell() {
    assert_sparse_section_adds_no_empty_cell(&fixtures::colonnade());
}

/// Coverage alone decides: one section holds every added brush, its
/// bounding face passes the occluder rule, and its solid covers no more
/// than the constant.
fn assert_sparse_section_adds_no_empty_cell(cf: &ClusterFixture) {
    let classification = classify_exempt_added(cf);
    let sections = sections_holding(&classification, cf.added());
    assert_eq!(sections.len(), 1, "{}: {sections:#?}", cf.fixture.name);
    let section = sections[0];
    assert_eq!(
        section.box_verdict,
        OccluderVerdict::Occludes,
        "{section:#?}"
    );
    assert!(
        section.coverage <= OCCLUDER_COVERAGE_FRACTION,
        "{section:#?}"
    );
    assert!(!section.occludes);
    assert_adds_no_empty_cell_or_portal(cf);
}

// A14 (pin R4): slab membership.

#[test]
fn pillar_on_touching_tiles_adds_no_portal_while_the_tiled_floor_bounds_cells() {
    // The pillar touches the tiles, so it joins their cluster, but its broad
    // faces stand across the floor's and it stays out of the floor's section.
    let cf = fixtures::tiled_floor_with_pillar();
    let classification = classify_exempt_added(&cf);
    let pillar = cf.base;
    assert!(
        classification
            .clusters
            .iter()
            .any(|cluster| cluster.members.contains(&pillar))
    );
    assert_added_stay_exempt(&cf, &classification);
    assert_adds_no_empty_cell_or_portal(&cf);
    assert_apart(&cf.fixture);
}

#[test]
fn tiled_floor_bounds_cells_as_one_solid_slab_does() {
    // The tiles' joints lie inside the section's box, so they split only the
    // tile layer, never the room above or below it.
    let tiled = fixtures::tiled_floor_with_pillar();
    let solid = TierBuild::classified(&fixtures::solid_mezzanine()).interior_cells_and_portals();
    let tiles = TierBuild::classified(&tiled.without()).interior_cells_and_portals();
    assert_eq!(tiles, solid, "(interior cells, portals) tiled vs solid");
}

#[test]
fn lamp_touching_a_strip_facade_adds_no_portal_while_the_facade_bounds_cells() {
    // The lamp's broad face agrees with the facade's, but it stands proud of
    // the facade's slab.
    let cf = fixtures::facade_with_lamp();
    let classification = classify_exempt_added(&cf);
    assert_added_stay_exempt(&cf, &classification);
    let facade = sections_holding(&classification, 6..cf.base);
    assert!(facade.iter().any(|section| section.occludes), "{facade:#?}");
    assert_adds_no_empty_cell_or_portal(&cf);
    assert_apart(&cf.fixture);
}

// A15 (pin R19) and the stepped-solid check.

#[test]
fn touching_step_run_adds_no_empty_cell() {
    let cf = fixtures::step_run(STEP_RUN_STEPS);
    let classification = classify_exempt_added(&cf);
    assert_added_stay_exempt(&cf, &classification);
    assert_adds_no_empty_cell_or_portal(&cf);
}

#[test]
fn step_runs_put_no_riser_or_tread_plane_in_the_occluder_tier() {
    // Stepped-solid check (research §Stacked and stepped solids). Each step's
    // broad face is its riser, and no two risers share a slab, so the run
    // forms no section along its length and no side-profile section; only
    // the two lowest steps, whose broad face is their tread, pair up. A run
    // past the 20 m absolute length stays exempt the same way.
    for steps in [STEP_RUN_STEPS, 30] {
        let cf = fixtures::step_run(steps);
        let classification = classify_exempt_added(&cf);
        assert_added_stay_exempt(&cf, &classification);
        report(&cf.fixture.name, &cf.fixture.brushes, &classification);
    }
}

// The street map, classified without a bake.

#[test]
#[ignore = "report: classifies the generated street map"]
fn stress_street_classification_reports_occluders_and_sections() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/maps/stress-street.map");
    let brushes = crate::parse::parse_map_file(&path, crate::map_format::MapFormat::IdTech2)
        .expect("stress-street.map parses")
        .brush_volumes;
    let start = std::time::Instant::now();
    let first_pass = super::classification_test_fixtures::FirstPass::of(&brushes);
    let first_pass_time = start.elapsed();
    let start = std::time::Instant::now();
    let classification = first_pass.classification(&brushes);
    let classify_time = start.elapsed();
    println!("first pass {first_pass_time:?}, classification {classify_time:?}");
    report("stress-street", &brushes, &classification);
}
