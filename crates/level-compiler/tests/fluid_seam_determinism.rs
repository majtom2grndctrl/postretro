//! Fluid seam closure is deterministic: id-52 bytes, and the reflection probes
//! keyed on them, are identical across cold builds, warm against cold, and
//! worker counts, so a warm build after the first hits the id-54 cache.
//! See: context/lib/build_pipeline.md §Compiler pipeline, §Build Cache

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::{SectionId, read_container, read_section_data};

const TEXTURE: &str = "50-free-textures/concrete_stone_021";
const FLUID_TEXTURE: &str = "test/concrete_grimey";

struct TempBuildDir(PathBuf);

impl TempBuildDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "postretro-fluid-seam-determinism-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(path.join("maps")).expect("create isolated compiler directory");
        Self(path)
    }
}

impl Drop for TempBuildDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn box_brush(min: [i32; 3], max: [i32; 3], texture: &str) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {texture} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {texture} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {texture} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {texture} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {texture} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {texture} 0 0 0 1 1\n\
         }}"
    )
}

/// A sealed room holding a pool of unequal sibling brushes, a second fluid
/// beside it with a taller, narrower brush, and a current over one corner:
/// burial, sibling, cross-volume and push seams to close.
fn pool_room() -> String {
    let walls = [
        box_brush([-272, -272, -16], [272, 272, 0], TEXTURE),
        box_brush([-272, -272, 192], [272, 272, 208], TEXTURE),
        box_brush([-272, -272, 0], [-256, 272, 192], TEXTURE),
        box_brush([256, -272, 0], [272, 272, 192], TEXTURE),
        box_brush([-256, -272, 0], [256, -256, 192], TEXTURE),
        box_brush([-256, 256, 0], [256, 272, 192], TEXTURE),
    ];
    let pool = [
        box_brush([-128, -64, 0], [0, 0, 48], FLUID_TEXTURE),
        box_brush([0, -64, 0], [64, 64, 48], FLUID_TEXTURE),
    ];
    format!(
        "{{\n\"classname\" \"worldspawn\"\n{}\n}}\n\
         {{\n\"classname\" \"player_spawn\"\n\"origin\" \"-192 -192 32\"\n}}\n\
         {{\n\"classname\" \"light\"\n\"origin\" \"0 0 160\"\n\"light\" \"300\"\n\
         \"_falloff_range\" \"400\"\n\"_light_size\" \"0\"\n}}\n\
         {{\n\"classname\" \"fluid_volume\"\n\"fluid\" \"water\"\n{}\n}}\n\
         {{\n\"classname\" \"fluid_volume\"\n\"fluid\" \"slime\"\n{}\n}}\n\
         {{\n\"classname\" \"push_volume\"\n\"push\" \"0 8 0\"\n{}\n}}\n",
        walls.join("\n"),
        pool.join("\n"),
        box_brush([64, -64, 0], [128, -16, 96], FLUID_TEXTURE),
        box_brush([32, 32, 0], [96, 96, 48], TEXTURE),
    )
}

/// Build `map` to its own output with `extra` arguments; returns the output.
fn build(temp: &TempBuildDir, map: &Path, name: &str, extra: &[&str]) -> PathBuf {
    let output_path = temp.0.join(format!("{name}.prl"));
    let output = Command::new(env!("CARGO_BIN_EXE_prl-build"))
        .env("RUST_LOG", "warn")
        .arg(map)
        .arg("-o")
        .arg(&output_path)
        .args([
            "--no-tui",
            "--sh-probe-spacing",
            "4",
            "--lightmap-density",
            "0.25",
        ])
        .args(extra)
        .output()
        .expect("spawn prl-build");
    assert!(
        output.status.success(),
        "prl-build failed for {name}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output_path
}

fn section(prl: &Path, id: SectionId) -> Vec<u8> {
    let bytes = std::fs::read(prl).expect("read compiled PRL");
    let mut cursor = Cursor::new(bytes);
    let metadata = read_container(&mut cursor).expect("read PRL container");
    read_section_data(&mut cursor, &metadata, id as u32)
        .expect("read section")
        .unwrap_or_else(|| panic!("{id:?} is written"))
}

#[test]
fn fluid_seams_close_identically_cold_warm_and_across_worker_counts() {
    let temp = TempBuildDir::new();
    let map = temp.0.join("maps").join("pool-room.map");
    std::fs::write(&map, pool_room()).expect("write fixture map");
    let cache = temp.0.join("cache");
    let cache_dir = cache.to_str().expect("UTF-8 temp path");

    let builds = [
        build(&temp, &map, "cold-a", &["--no-cache"]),
        build(&temp, &map, "cold-b", &["--no-cache"]),
        build(
            &temp,
            &map,
            "cold-one-worker",
            &["--no-cache", "--jobs", "1"],
        ),
        build(&temp, &map, "warm-fill", &["--cache-dir", cache_dir]),
        build(
            &temp,
            &map,
            "warm-hit",
            &["--cache-dir", cache_dir, "--jobs", "1"],
        ),
    ];
    let environment = section(&builds[0], SectionId::EnvironmentVolumes);
    let decoded = EnvironmentVolumesSection::from_bytes(&environment).expect("id 52 decodes");
    assert!(
        decoded.volumes.iter().any(|volume| volume
            .surface_faces
            .iter()
            .any(|face| face.vertices.len() > 4)),
        "the fixture closes no seam"
    );
    let probes = section(&builds[0], SectionId::ReflectionProbes);
    for build in &builds[1..] {
        assert_eq!(
            section(build, SectionId::EnvironmentVolumes),
            environment,
            "{build:?}"
        );
        assert_eq!(
            section(build, SectionId::ReflectionProbes),
            probes,
            "{build:?}"
        );
    }
}
