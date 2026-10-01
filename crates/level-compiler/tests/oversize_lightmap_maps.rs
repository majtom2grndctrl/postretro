//! On-demand compile checks for lightmap cells larger than one pool layer.
//!
//! Compiles real content maps through the `prl-build` binary and reads the
//! emitted lightmap blocks and vertex placements. Every test here is
//! `#[ignore]`: each is a multi-minute bake.
//!
//! See: context/lib/build_pipeline.md §Compiler pipeline (atlas preparation)

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use postretro_level_format::SectionId;
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::lightmap::{LIGHTMAP_POOL_LAYER_EDGE, LightmapSection};
use postretro_level_format::{read_container, read_section_data};

fn workspace_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

/// Compile `content/dev/maps/<map>.map` to a temp `.prl` with `extra` flags.
fn compile(map: &str, extra: &[&str]) -> PathBuf {
    let ws = workspace_root();
    let input = ws.join(format!("content/dev/maps/{map}.map"));
    assert!(input.exists(), "map missing: {}", input.display());
    let out_dir = std::env::temp_dir().join("postretro_oversize_lightmap_maps");
    std::fs::create_dir_all(&out_dir).expect("mkdir temp out");
    let output = out_dir.join(format!("{map}.prl"));
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args([
            "run",
            "--quiet",
            "--release",
            "-p",
            "postretro-level-compiler",
            "--bin",
            "prl-build",
            "--",
        ])
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .args(extra)
        .current_dir(&ws)
        .status()
        .expect("spawn prl-build");
    assert!(status.success(), "prl-build {map} failed: {status}");
    output
}

fn section(prl: &Path, id: SectionId) -> Vec<u8> {
    let bytes = std::fs::read(prl).expect("read compiled .prl");
    let mut cursor = Cursor::new(&bytes);
    let meta = read_container(&mut cursor).expect("read_container");
    read_section_data(&mut cursor, &meta, id as u32)
        .expect("read_section_data")
        .unwrap_or_else(|| panic!("{}: section {id:?} absent", prl.display()))
}

fn lightmap(prl: &Path) -> LightmapSection {
    LightmapSection::from_bytes(&section(prl, SectionId::Lightmap)).expect("id 22 decodes")
}

fn geometry(prl: &Path) -> GeometrySection {
    GeometrySection::from_bytes(&section(prl, SectionId::Geometry)).expect("id 17 decodes")
}

/// Each block's `(cell, width, height)`, in block-id order.
fn block_records(section: &LightmapSection) -> Vec<(u32, u16, u16)> {
    section
        .blocks
        .iter()
        .map(|b| (b.cell_id, b.width, b.height))
        .collect()
}

/// Neither map opts out of the default density: no `_lightmap_density`
/// worldspawn key and no scale region.
fn assert_default_density(map: &str) {
    let source =
        std::fs::read_to_string(workspace_root().join(format!("content/dev/maps/{map}.map")))
            .expect("read map source");
    assert!(
        !source.contains("_lightmap_density"),
        "{map} overrides the density"
    );
    assert!(
        !source.contains("lightmap_scale_region"),
        "{map} has a scale region"
    );
}

// Regression: both maps failed atlas preparation at the default density once
// the atlas ceiling became the 2048 pool layer, because the BSP leaves each
// map a cell whose charts exceed one layer.
#[test]
#[ignore = "multi-minute prl-build bake; run on demand with -- --ignored"]
fn movement_feel_and_kinematic_platform_compile_at_the_default_density() {
    for map in ["movement-feel", "kinematic-platform"] {
        assert_default_density(map);
        let prl = compile(map, &[]);
        let section = lightmap(&prl);
        let records = block_records(&section);
        assert!(
            records
                .iter()
                .all(|&(_, w, h)| u32::from(w) <= LIGHTMAP_POOL_LAYER_EDGE
                    && u32::from(h) <= LIGHTMAP_POOL_LAYER_EDGE),
            "{map}: a block exceeds the pool edge"
        );
        // A cell's blocks are one contiguous run, and at least one cell
        // needed several.
        let mut runs: Vec<u32> = records.iter().map(|r| r.0).collect();
        let block_count = runs.len();
        runs.dedup();
        let mut distinct = runs.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(
            runs.len(),
            distinct.len(),
            "{map}: a cell's blocks interleave"
        );
        assert!(
            block_count > runs.len(),
            "{map}: expected a multi-block cell"
        );
        // Every vertex names a real block.
        let geometry = geometry(&prl);
        assert!(
            geometry
                .vertices
                .iter()
                .all(|v| usize::from(v.lightmap_block) <= block_count),
            "{map}: a vertex names a block past id 22"
        );
    }
}

/// Maps whose cells all fit one pool layer.
const FITTING_MAPS: [&str; 2] = ["campaign-test", "stress-warren-hallway-inspection"];

/// Directory holding `<map>.prl` for each of [`FITTING_MAPS`], compiled with
/// `--release` at `19fb3fc40` (before multi-block cells).
fn baseline_dir() -> PathBuf {
    PathBuf::from(std::env::var("POSTRETRO_OVERSIZE_BASELINE_DIR").expect(
        "set POSTRETRO_OVERSIZE_BASELINE_DIR to the `--release` PRLs compiled at 19fb3fc40",
    ))
}

// Multi-block packing leaves a map whose cells fit one layer exactly as it
// was: the same blocks, and every vertex at the same block and lightmap UV.
#[test]
#[ignore = "long --release prl-build bakes against a baseline; run on demand with -- --ignored"]
fn fitting_maps_keep_their_block_extents_and_chart_placements() {
    let baseline = baseline_dir();
    let mut compared = 0;
    for map in FITTING_MAPS {
        let before = baseline.join(format!("{map}.prl"));
        // The hallway's baseline is an hours-long bake; compare what exists.
        if !before.exists() {
            eprintln!("{map}: no baseline at {}, skipped", before.display());
            continue;
        }
        compared += 1;
        let after = compile(map, &["--release"]);
        assert_eq!(
            block_records(&lightmap(&before)),
            block_records(&lightmap(&after)),
            "{map}: block extents changed"
        );
        let placements = |prl: &Path| -> Vec<(u16, [u16; 2])> {
            geometry(prl)
                .vertices
                .iter()
                .map(|v| (v.lightmap_block, v.lightmap_uv))
                .collect()
        };
        assert_eq!(
            placements(&before),
            placements(&after),
            "{map}: vertex block or lightmap UV changed"
        );
    }
    assert!(
        compared > 0,
        "no baseline PRL found in {}",
        baseline.display()
    );
}
