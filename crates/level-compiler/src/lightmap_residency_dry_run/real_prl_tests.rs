use std::path::PathBuf;

use super::{read_dry_run_input, run_dry_run};

/// Measurement helper: attribution, per-cell mandatory bytes under distance
/// bounds, the sampled visible-set bounds, the cell-block pool walks, and the
/// brief set's lead map, band and walks for a real compiled PRL. Run from the
/// workspace root, in release for large maps:
///
/// ```text
/// POSTRETRO_LIGHTMAP_RESIDENCY_DRY_RUN_PRL=/abs/path/to/map.prl \
/// POSTRETRO_LIGHTMAP_RESIDENCY_DRY_RUN_CSV=/abs/path/to/cells.csv \
///   cargo test --release -p postretro-level-compiler --bin prl-build \
///   lightmap_residency_dry_run_from_prl -- --ignored --nocapture
/// ```
///
/// The PRL path must be absolute: the test runs from the crate directory.
/// The CSV variable is optional and names a per-cell output file.
///
/// Lightless maps (placeholder id 22) and maps whose id 42 the bake dropped
/// for an empty placement set are not meaningful inputs.
///
/// The report prints first; the self-checks then assert, so a failure still
/// shows the numbers.
#[test]
#[ignore = "measurement helper; set POSTRETRO_LIGHTMAP_RESIDENCY_DRY_RUN_PRL"]
fn lightmap_residency_dry_run_from_prl() {
    let path = std::env::var("POSTRETRO_LIGHTMAP_RESIDENCY_DRY_RUN_PRL")
        .expect("POSTRETRO_LIGHTMAP_RESIDENCY_DRY_RUN_PRL must name a compiler-produced PRL");
    let started = std::time::Instant::now();
    let input = read_dry_run_input(&PathBuf::from(&path)).unwrap();
    let report = run_dry_run(&input);
    println!("PRL: {path}");
    println!("{}", report.render());
    println!("elapsed: {:.1}s", started.elapsed().as_secs_f64());
    if let Ok(csv_path) = std::env::var("POSTRETRO_LIGHTMAP_RESIDENCY_DRY_RUN_CSV") {
        std::fs::write(&csv_path, report.csv()).unwrap();
        println!("per-cell CSV: {csv_path}");
    }

    let stats = &input.reconstruction;
    assert_eq!(
        (
            stats.mixed_layer_faces,
            stats.out_of_bounds_faces,
            stats.unmatched_bvh_faces
        ),
        (0, 0, 0),
        "real charts were not recovered (mixed-layer, out-of-bounds, unmatched BVH faces)"
    );
    let attribution = &report.attribution;
    let mut sections = vec![
        ("id 22 irradiance", &attribution.irradiance),
        ("id 22 direction", &attribution.direction),
    ];
    if let Some(shadowmask) = &attribution.shadowmask {
        sections.push(("id 42 shadowmask", shadowmask));
    }
    for (name, section) in sections {
        assert_eq!(
            section.attributed() + section.unattributed,
            section.payload,
            "{name}: attributed + unattributed must equal the payload"
        );
    }
    assert_eq!(
        attribution.overlap_texels, 0,
        "recovered charts overlap: the reconstruction disagrees with the stored packing"
    );
    assert!(
        report.repack.reproduces_stored(),
        "stored-order repack does not reproduce the PRL layout: {:?}",
        report.repack
    );
    if let Some(validation) = report.validation {
        assert!(
            validation.all_matched(),
            "hub-metric recompute disagrees with stored id-46 records: {validation:?}"
        );
    }
    if let Some(visible) = &report.visible_set {
        for variant in &visible.brief_set.variants {
            assert_eq!(
                variant.consistency.1,
                0,
                "{} lead map disagrees with direct evaluation of M(c, L)",
                variant.dilation.label()
            );
        }
        assert!(
            super::tiles_render::cluster_units_agree_across_granularity(
                visible,
                &report.tile_layouts
            ),
            "cluster-unit tile bytes differ between cell-granular and cluster-closure sets"
        );
    }
}
