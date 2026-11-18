// Chart dump tests: the writer and comparator on synthetic records, the
// capture sites through a real bake, and the opt-in A/B comparison.
// See: context/lib/build_pipeline.md §Build Cache (CLI flags)

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use postretro_level_format::shadowmask_atlas::NO_FACE_OWNERS;

use super::compare::{RingPolicy, compare_dumps};
use super::format::{ChartKey, ChartMeta, DumpWriter, Placement, PlaneKind};
use super::reader::DumpReader;
use super::{Category, ChartDumpConfig, DumpKind, prepare_dir};
use crate::coloured_bounce_test_fixtures::{Appearance, room_map, without_fluid, write_textures};

// ---- synthetic records ----

/// A 6×5 padded chart (padding 2: a 2×1 grid) with direction scale 2.
fn lightmap_meta(face: u32, block_x: u32) -> ChartMeta {
    ChartMeta {
        key: ChartKey { face, sub: 0 },
        chart_fingerprint: 0xC0FFEE + u64::from(face),
        width: 6,
        height: 5,
        padding: 2,
        origin: [0, 0],
        placement: Placement {
            block_x,
            ..Placement::default()
        },
        direction_texel_scale: 2,
        direction_offset: [0, 0],
        direction_dims: [3, 2],
        owners: NO_FACE_OWNERS,
    }
}

fn irradiance(seed: f32) -> Vec<u8> {
    (0..6 * 5 * 4)
        .flat_map(|i| (seed + i as f32).to_le_bytes())
        .collect()
}

fn write_dump(dir: &Path, texels: bool, records: &[(ChartMeta, Vec<u8>)]) {
    let config = ChartDumpConfig {
        dir: dir.to_path_buf(),
        texels,
    };
    prepare_dir(&config).unwrap();
    let mut writer = DumpWriter::create(&config, DumpKind::Lightmap).unwrap();
    for (meta, irradiance) in records {
        let direction = vec![7u8; 3 * 2 * 2];
        writer.push(
            meta.clone(),
            &[
                (PlaneKind::Irradiance, irradiance),
                (PlaneKind::Direction, &direction),
            ],
        );
    }
    writer.finish().unwrap();
}

/// Byte offset of irradiance texel `(x, y)` inside the 6-wide record.
fn irradiance_texel(x: usize, y: usize) -> usize {
    (y * 6 + x) * 16
}

#[test]
fn comparator_names_the_first_differing_grid_texel_and_passes_equal_dumps() {
    let root = tempfile::tempdir().unwrap();
    let records = vec![
        (lightmap_meta(4, 0), irradiance(1.0)),
        (lightmap_meta(1, 8), irradiance(2.0)),
    ];
    let (base, same, moved) = (
        root.path().join("base"),
        root.path().join("same"),
        root.path().join("moved"),
    );
    write_dump(&base, true, &records);
    write_dump(&same, true, &records);
    let equal = compare_dumps(&base, &same).unwrap();
    assert_eq!(equal.failures(RingPolicy::Fail), Vec::<String>::new());
    assert_eq!(equal.categories[&Category::IrradianceGrid].identical, 2);
    assert_eq!(equal.categories[&Category::Direction].identical, 2);

    // Face 1's grid texel (3, 2) drifts by one ulp; its ring is untouched.
    let mut drifted = records.clone();
    let at = irradiance_texel(3, 2);
    drifted[1].1[at] ^= 1;
    write_dump(&moved, true, &drifted);
    let comparison = compare_dumps(&base, &moved).unwrap();
    let grid = &comparison.categories[&Category::IrradianceGrid];
    assert_eq!((grid.compared, grid.identical, grid.differing), (2, 1, 1));
    let (key, detail) = &grid.examples[0];
    assert_eq!(*key, ChartKey { face: 1, sub: 0 });
    assert!(detail.contains("first at (3, 2)"), "{detail}");
    assert_eq!(
        comparison.categories[&Category::IrradianceRing].differing,
        0
    );
    assert_eq!(comparison.failures(RingPolicy::Report).len(), 1);
}

#[test]
fn comparator_separates_the_ring_skips_odd_direction_origins_and_reports_missing_charts() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("base");
    let cand = root.path().join("cand");
    // Face 2 sits at an odd block origin: its direction has no baseline.
    write_dump(
        &base,
        true,
        &[
            (lightmap_meta(2, 3), irradiance(1.0)),
            (lightmap_meta(5, 0), irradiance(1.0)),
        ],
    );
    let mut ring_only = irradiance(1.0);
    ring_only[irradiance_texel(0, 0)] ^= 1;
    let mut odd = lightmap_meta(2, 3);
    odd.direction_dims = [3, 2];
    write_dump(
        &cand,
        true,
        &[(odd, ring_only), (lightmap_meta(9, 0), irradiance(1.0))],
    );
    let comparison = compare_dumps(&base, &cand).unwrap();
    assert_eq!(
        comparison.categories[&Category::IrradianceRing].differing,
        1
    );
    assert_eq!(
        comparison.categories[&Category::IrradianceGrid].differing,
        0
    );
    assert_eq!(
        comparison.categories[&Category::Direction].skipped_unaligned,
        1
    );
    let tally = &comparison.kinds[&DumpKind::Lightmap];
    assert_eq!(
        tally.missing_in_candidate,
        vec![ChartKey { face: 5, sub: 0 }]
    );
    assert_eq!(
        tally.missing_in_baseline,
        vec![ChartKey { face: 9, sub: 0 }]
    );
    let report_only = comparison.failures(RingPolicy::Report);
    assert_eq!(report_only.len(), 2, "{report_only:?}");
    assert_eq!(comparison.failures(RingPolicy::Fail).len(), 3);
}

#[test]
fn hashes_only_dumps_compare_without_texels() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("base");
    let cand = root.path().join("cand");
    write_dump(&base, false, &[(lightmap_meta(0, 0), irradiance(1.0))]);
    write_dump(&cand, false, &[(lightmap_meta(0, 0), irradiance(1.5))]);
    assert!(!DumpKind::Lightmap.data_path(&base).exists());
    let comparison = compare_dumps(&base, &cand).unwrap();
    let grid = &comparison.categories[&Category::IrradianceGrid];
    assert_eq!(grid.differing, 1);
    assert!(grid.examples[0].1.contains("hashes-only"));
}

#[test]
fn comparing_nothing_fails() {
    let root = tempfile::tempdir().unwrap();
    let (base, cand) = (root.path().join("a"), root.path().join("b"));
    write_dump(&base, true, &[]);
    write_dump(&cand, true, &[]);
    let failures = compare_dumps(&base, &cand)
        .unwrap()
        .failures(RingPolicy::Fail);
    assert_eq!(failures, vec!["no chart was compared".to_owned()]);
    assert!(compare_dumps(&base, root.path()).is_err(), "not a dump");
}

#[test]
fn dump_flags_require_an_uncached_bake_and_name_one_directory() {
    let parse = |flags: &[&str]| {
        crate::parse_args_from(
            ["input.map"]
                .iter()
                .chain(flags)
                .map(|flag| (*flag).to_owned()),
        )
    };
    let error = parse(&["--dump-chart-texels", "out"])
        .unwrap_err()
        .to_string();
    assert!(error.contains("--no-cache or --release"), "{error}");
    for mode in ["--no-cache", "--release"] {
        let args = parse(&["--dump-chart-hashes", "out", mode]).unwrap();
        assert_eq!(
            args.chart_dump,
            Some(ChartDumpConfig {
                dir: PathBuf::from("out"),
                texels: false,
            })
        );
    }
    assert!(
        parse(&[
            "--dump-chart-texels",
            "a",
            "--dump-chart-hashes",
            "b",
            "--no-cache"
        ])
        .is_err()
    );
    assert!(parse(&["--no-cache"]).unwrap().chart_dump.is_none());
}

// ---- through a real bake ----

/// Compile the coloured-bounce room (one static light that owns masks, one
/// animated light) on `workers` workers, dumping to `dump` when given.
fn bake_room(root: &Path, name: &str, workers: usize, dump: Option<&Path>) -> Vec<u8> {
    let map = root.join("maps/room.map");
    if !map.exists() {
        write_textures(root);
        std::fs::create_dir_all(map.parent().unwrap()).unwrap();
        std::fs::write(
            &map,
            without_fluid(room_map(Appearance::RedWall, true, true)),
        )
        .unwrap();
    }
    let output = root.join(format!("{name}.prl"));
    let path = |path: &Path| path.to_str().expect("UTF-8 path").to_owned();
    let mut argv = vec![
        path(&map),
        "-o".to_owned(),
        path(&output),
        "--baked-root".to_owned(),
        path(&root.join("baked")),
        "--no-cache".to_owned(),
        "--sh-probe-spacing".to_owned(),
        "4".to_owned(),
        "--lightmap-density".to_owned(),
        "0.125".to_owned(),
    ];
    if let Some(dir) = dump {
        argv.extend(["--dump-chart-texels".to_owned(), path(dir)]);
    }
    let args = crate::parse_args_from(argv.into_iter()).expect("arguments parse");
    rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .expect("worker pool")
        .install(|| {
            let started = Instant::now();
            let reporter: Arc<dyn crate::reporter::Reporter> = Arc::new(
                crate::reporter::PlainReporter::new(started, crate::logger::LogSink::default()),
            );
            crate::pipeline::run(
                &args,
                None,
                started,
                reporter,
                Arc::new(crate::governor::Governor::new(workers, false)),
            )
            .expect("room compiles");
        });
    std::fs::read(&output).unwrap()
}

fn dump_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&path).unwrap(),
            )
        })
        .collect();
    files.sort();
    files
}

#[test]
fn bake_dump_is_deterministic_leaves_the_prl_unchanged_and_catches_a_perturbed_texel() {
    let root = tempfile::tempdir().unwrap();
    let (one, three) = (root.path().join("dump-1"), root.path().join("dump-3"));
    let dumped = bake_room(root.path(), "dumped", 1, Some(&one));
    let dumped_wide = bake_room(root.path(), "dumped-wide", 3, Some(&three));
    let plain = bake_room(root.path(), "plain", 1, None);
    assert!(dumped == plain, "the dump changed the PRL");
    assert!(dumped_wide == plain, "worker count changed the PRL");

    // Same files, byte for byte, whatever the worker count.
    let files = dump_files(&one);
    let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "animated.data",
            "animated.index",
            "chart-dump.json",
            "lightmap.data",
            "lightmap.index",
            "mask.data",
            "mask.index",
        ]
    );
    assert!(files == dump_files(&three), "dump bytes depend on workers");

    let comparison = compare_dumps(&one, &three).unwrap();
    assert_eq!(
        comparison.failures(RingPolicy::Fail),
        Vec::<String>::new(),
        "{}",
        comparison.render()
    );
    for category in [
        Category::IrradianceGrid,
        Category::Direction,
        Category::MaskAGrid,
        Category::AnimatedWeights,
    ] {
        assert!(
            comparison.categories[&category].identical > 0,
            "nothing compared for {}:\n{}",
            category.label(),
            comparison.render()
        );
    }

    // Re-record the dump with one bit of one chart's first grid texel
    // flipped, as a bake that produced that value would have written it.
    let perturbed = root.path().join("dump-perturbed");
    let config = ChartDumpConfig {
        dir: perturbed.clone(),
        texels: true,
    };
    prepare_dir(&config).unwrap();
    for (name, bytes) in &files {
        if !name.starts_with("lightmap.") {
            std::fs::write(perturbed.join(name), bytes).unwrap();
        }
    }
    let reader = DumpReader::open(&one, DumpKind::Lightmap).unwrap();
    let target = reader
        .entries
        .values()
        .find(|entry| entry.meta.width > 4 && entry.meta.height > 4)
        .expect("a chart with a grid")
        .meta
        .key;
    let mut writer = DumpWriter::create(&config, DumpKind::Lightmap).unwrap();
    for entry in reader.entries.values() {
        let planes: Vec<(PlaneKind, Vec<u8>)> = entry
            .planes
            .iter()
            .map(|span| {
                let mut bytes = reader.read_plane(entry, span.kind).unwrap().unwrap();
                if entry.meta.key == target && span.kind == PlaneKind::Irradiance {
                    bytes[(2 * entry.meta.width as usize + 2) * 16] ^= 1;
                }
                (span.kind, bytes)
            })
            .collect();
        let planes: Vec<(PlaneKind, &[u8])> = planes
            .iter()
            .map(|(kind, bytes)| (*kind, bytes.as_slice()))
            .collect();
        writer.push(entry.meta.clone(), &planes);
    }
    writer.finish().unwrap();
    let comparison = compare_dumps(&one, &perturbed).unwrap();
    let grid = &comparison.categories[&Category::IrradianceGrid];
    assert_eq!(grid.differing, 1, "{}", comparison.render());
    assert_eq!(grid.examples[0].0, target);
    assert!(
        grid.examples[0].1.contains("first at (2, 2)"),
        "{}",
        grid.examples[0].1
    );
    assert!(!comparison.failures(RingPolicy::Fail).is_empty());
}

// ---- the A/B comparison ----

/// The bit-identity instrument for irradiance, direction and masks: compare a candidate bake's chart dump against
/// a baseline's. Fails, never passes empty, when either input is unset.
#[test]
#[ignore = "manual A/B: set POSTRETRO_CHART_DUMP_BASELINE and POSTRETRO_CHART_DUMP_CANDIDATE \
            to two `prl-build --dump-chart-texels` (or --dump-chart-hashes) directories; \
            POSTRETRO_CHART_DUMP_RING=report reports padding-ring differences without failing"]
fn chart_dump_candidate_matches_baseline() {
    let dir = |name: &str| {
        PathBuf::from(std::env::var_os(name).unwrap_or_else(|| {
            panic!("{name} is unset: this comparison needs two chart dump directories")
        }))
    };
    let baseline = dir("POSTRETRO_CHART_DUMP_BASELINE");
    let candidate = dir("POSTRETRO_CHART_DUMP_CANDIDATE");
    let ring = match std::env::var("POSTRETRO_CHART_DUMP_RING").as_deref() {
        Err(_) | Ok("fail") => RingPolicy::Fail,
        Ok("report") => RingPolicy::Report,
        Ok(other) => panic!("POSTRETRO_CHART_DUMP_RING must be `fail` or `report`, got {other:?}"),
    };
    let comparison = compare_dumps(&baseline, &candidate).expect("both dumps read");
    println!("{}", comparison.render());
    let failures = comparison.failures(ring);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
