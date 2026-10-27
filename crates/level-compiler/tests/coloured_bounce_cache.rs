//! PNG edits, sidecar reuse and material-stage ordering through the real compiler CLI.
//! See: context/plans/in-progress/coloured-sh-bounce/index.md

#[path = "../src/coloured_bounce_test_fixtures.rs"]
#[allow(dead_code)] // Other suites use this shared room's unresolved appearances.
mod fixtures;

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::{read_container, read_section_data};

const SH_START: &str = "SH volume bake...";
const DELTA_START: &str = "Delta SH volume bake...";
const TAIL_START: &str = "Texture mip bake...";

struct Fixture {
    dir: tempfile::TempDir,
    map: PathBuf,
}

struct Bake {
    log: String,
    sections: BTreeMap<u32, Vec<u8>>,
    sidecars: BTreeMap<String, blake3::Hash>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("maps")).unwrap();
        fixtures::write_textures(dir.path());
        let map = dir.path().join("maps/coloured-bounce.map");
        std::fs::write(
            &map,
            fixtures::room_map(fixtures::Appearance::RedWall, true, true),
        )
        .unwrap();
        Self { dir, map }
    }

    fn texture(&self, name: &str) -> PathBuf {
        self.dir
            .path()
            .join("textures/bounce")
            .join(format!("{name}.png"))
    }

    fn materials(&self) -> PathBuf {
        self.dir.path().join("baked/materials")
    }

    fn run(&self, cache: bool, workers: u32, extra: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_prl-build"));
        command
            .env("RUST_LOG", "info")
            .arg(&self.map)
            .arg("-o")
            .arg(self.dir.path().join("room.prl"))
            .arg("--baked-root")
            .arg(self.dir.path().join("baked"))
            .arg("--jobs")
            .arg(workers.to_string())
            .args([
                "--no-tui",
                "--lightmap-density",
                "0.25",
                "--sh-probe-spacing",
                "1",
                "--soft-shadow-samples",
                "4",
                "--uncompressed-irradiance",
            ]);
        if cache {
            command
                .arg("--cache-dir")
                .arg(self.dir.path().join("cache"));
        } else {
            command.arg("--no-cache");
        }
        command
            .args(extra)
            .output()
            .expect("run tiny material fixture")
    }

    fn bake(&self, cache: bool, workers: u32) -> Bake {
        let output = self.run(cache, workers, &[]);
        let log = diagnostic(&output);
        assert!(output.status.success(), "fixture compile failed:\n{log}");
        let mut cursor = Cursor::new(std::fs::read(self.dir.path().join("room.prl")).unwrap());
        let metadata = read_container(&mut cursor).unwrap();
        let sections = [34, 27]
            .into_iter()
            .map(|id| {
                let bytes = read_section_data(&mut cursor, &metadata, id)
                    .unwrap()
                    .unwrap_or_else(|| panic!("fixture must exercise id {id}"));
                (id, bytes)
            })
            .collect();
        Bake {
            log,
            sections,
            sidecars: sidecars(&self.materials()),
        }
    }
}

fn diagnostic(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn sidecars(root: &Path) -> BTreeMap<String, blake3::Hash> {
    if !root.exists() {
        return BTreeMap::new();
    }
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "prm"))
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                blake3::hash(&std::fs::read(path).unwrap()),
            )
        })
        .collect()
}

fn write_png(path: &Path, texel: impl Fn(usize) -> [u8; 4]) {
    let pixels: Vec<u8> = (0..256).flat_map(texel).collect();
    let mut encoder = png::Encoder::new(std::fs::File::create(path).unwrap(), 16, 16);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&pixels)
        .unwrap();
}

fn tally(log: &str, stage: &str) -> (usize, usize) {
    (
        log.matches(&format!("[cache] {stage} hit")).count(),
        log.matches(&format!("[cache] {stage} miss")).count(),
    )
}

fn all_miss(bake: &Bake, totals: Option<&Bake>) {
    for stage in ["sh_group", "indirect_delta_sh_subblock"] {
        let (hits, misses) = tally(&bake.log, stage);
        assert!(
            hits == 0 && misses > 0,
            "{stage} must rebake every entry:\n{}",
            bake.log
        );
        if let Some(previous) = totals {
            let (old_hits, old_misses) = tally(&previous.log, stage);
            assert_eq!(
                misses,
                old_hits + old_misses,
                "{stage} entry count stays fixed"
            );
        }
    }
}

fn all_hit(bake: &Bake, previous: &Bake) {
    for stage in ["sh_group", "indirect_delta_sh_subblock"] {
        let (hits, misses) = tally(&bake.log, stage);
        let (old_hits, old_misses) = tally(&previous.log, stage);
        assert!(
            hits > 0 && misses == 0,
            "{stage} must reuse every entry:\n{}",
            bake.log
        );
        assert_eq!(
            hits,
            old_hits + old_misses,
            "{stage} entry count stays fixed"
        );
    }
}

#[test]
#[ignore = "focused cold-bake/material-cache integration gate"]
fn mean_edits_rekey_every_group_and_entry_but_auxiliary_and_fluid_edits_hit() {
    let fixture = Fixture::new();
    let first = fixture.bake(true, 1);
    all_miss(&first, None);
    let warm = fixture.bake(true, 1);
    all_hit(&warm, &first);
    assert_eq!(first.sections, warm.sections);
    assert_eq!(first.sidecars, warm.sidecars);

    // One 255→254 red texel out of 256 leaves the 8-bit mean at 255.
    write_png(&fixture.texture("red"), |i| {
        [if i == 0 { 254 } else { 255 }, 0, 0, 255]
    });
    let tiny = fixture.bake(true, 1);
    all_hit(&tiny, &warm);
    assert_eq!(tiny.sections, warm.sections);
    assert_ne!(
        tiny.sidecars, warm.sidecars,
        "changed source still produces a new sidecar"
    );

    for (name, colour) in [
        ("red_n", [128, 128, 255, 255]),
        ("red_s", [64; 4]),
        ("water", [127, 80, 20, 255]),
    ] {
        write_png(&fixture.texture(name), |_| colour);
        let edited = fixture.bake(true, 2);
        all_hit(&edited, &tiny);
        assert_eq!(
            edited.sections, tiny.sections,
            "{name} does not affect static-world bounce"
        );
    }
    // A regional repaint moves the mean, so every group and indirect entry misses.
    write_png(&fixture.texture("red"), |i| {
        if i < 128 {
            [0, 0, 255, 255]
        } else {
            [255, 0, 0, 255]
        }
    });
    let repainted = fixture.bake(true, 2);
    all_miss(&repainted, Some(&tiny));
    for id in [34, 27] {
        assert_ne!(
            repainted.sections[&id], tiny.sections[&id],
            "mean repaint must change id {id}"
        );
    }
    let repeated = fixture.bake(true, 1);
    all_hit(&repeated, &repainted);
    assert_eq!(repainted.sections, repeated.sections);
}

#[test]
#[ignore = "focused cold-bake/glow-cache integration gate"]
fn glow_mask_changes_rekey_but_nonzero_emissive_intensity_changes_hit() {
    let fixture = Fixture::new();
    let map = std::fs::read_to_string(&fixture.map)
        .unwrap()
        .replace("bounce/red", "bounce/neon_panel");
    std::fs::write(&fixture.map, map).unwrap();
    write_png(&fixture.texture("neon_panel"), |i| {
        if i < 128 { [255; 4] } else { [64, 16, 16, 255] }
    });
    write_png(&fixture.texture("neon_panel_e"), |i| {
        if i < 128 {
            [1, 0, 0, 255]
        } else {
            [0, 0, 0, 255]
        }
    });
    let first = fixture.bake(true, 1);
    all_miss(&first, None);
    write_png(&fixture.texture("neon_panel_e"), |i| {
        if i < 128 {
            [255, 120, 20, 255]
        } else {
            [0, 0, 0, 255]
        }
    });
    let brighter = fixture.bake(true, 2);
    all_hit(&brighter, &first);
    assert_eq!(first.sections, brighter.sections);
    assert_ne!(first.sidecars, brighter.sidecars);
    // One extra marked dark texel leaves the retained constant colour unchanged.
    write_png(&fixture.texture("neon_panel_e"), |i| {
        if i <= 128 {
            [255, 120, 20, 255]
        } else {
            [0, 0, 0, 255]
        }
    });
    let unchanged_mean = fixture.bake(true, 1);
    all_hit(&unchanged_mean, &brighter);
    assert_eq!(brighter.sections, unchanged_mean.sections);
    write_png(&fixture.texture("neon_panel_e"), |_| [0, 0, 0, 255]);
    let unmasked = fixture.bake(true, 1);
    all_miss(&unmasked, Some(&first));
    assert_ne!(unmasked.sections[&27], first.sections[&27]);
}

fn warm_is_no_brighter_than_cold(warm: &[u8], cold: &[u8]) {
    let warm = OctahedralShVolumeSection::from_bytes(warm).unwrap();
    let cold = OctahedralShVolumeSection::from_bytes(cold).unwrap();
    assert_eq!(warm.irradiance_format, IRRADIANCE_FORMAT_RGBA16F);
    assert_eq!(cold.irradiance_format, IRRADIANCE_FORMAT_RGBA16F);
    assert_eq!(warm.grid_dimensions, cold.grid_dimensions);
    assert_eq!(warm.compact_atlas.len(), cold.compact_atlas.len());
    assert!(!warm.compact_atlas.is_empty());
    for (warm, cold) in warm
        .compact_atlas
        .chunks_exact(8)
        .zip(cold.compact_atlas.chunks_exact(8))
    {
        for channel in 0..3 {
            let warm = u16::from_le_bytes([warm[channel * 2], warm[channel * 2 + 1]]);
            let cold = u16::from_le_bytes([cold[channel * 2], cold[channel * 2 + 1]]);
            // Positive finite IEEE binary16 values have the same order as their bits.
            assert!(
                warm < 0x7c00 && cold < 0x7c00,
                "nonnegative finite irradiance"
            );
            assert!(
                warm <= cold,
                "warm channel {warm:#06x} exceeds cold {cold:#06x}"
            );
        }
    }
}

#[test]
#[ignore = "focused sidecar/cache/worker determinism gate"]
fn sidecar_reuse_and_rebake_preserve_bytes_with_and_without_stage_cache() {
    let mut cold_reference = None;
    for cache in [false, true] {
        let fixture = Fixture::new();
        let first = fixture.bake(cache, 1);
        assert!(!first.sidecars.is_empty());
        let reused = fixture.bake(cache, 2);
        assert_eq!(
            first.sections, reused.sections,
            "sidecars reused with stage cache {cache}"
        );
        assert_eq!(first.sidecars, reused.sidecars);
        if cache {
            all_miss(&first, None);
            all_hit(&reused, &first);
        } else {
            for stage in ["sh_group", "indirect_delta_sh_subblock"] {
                assert_eq!(
                    tally(&first.log, stage),
                    (0, 0),
                    "no-cache builds construct no stage cache key"
                );
            }
        }
        std::fs::remove_dir_all(fixture.materials()).unwrap();
        let rebuilt = fixture.bake(cache, 2);
        assert_eq!(
            first.sections, rebuilt.sections,
            "rebuilt sidecars retain lighting bytes"
        );
        assert_eq!(
            first.sidecars, rebuilt.sidecars,
            "sidecar format and bytes stay fixed"
        );
        if cache {
            all_hit(&rebuilt, &reused);
            let cold: &Bake = cold_reference.as_ref().unwrap();
            assert_eq!(
                rebuilt.sections[&27], cold.sections[&27],
                "warm indirect deltas are exact"
            );
            warm_is_no_brighter_than_cold(&rebuilt.sections[&34], &cold.sections[&34]);
            // Warm sidecars must also work when the stage cache is empty.
            std::fs::remove_dir_all(fixture.dir.path().join("cache")).unwrap();
            let empty_cache = fixture.bake(true, 2);
            all_miss(&empty_cache, Some(&first));
            assert_eq!(empty_cache.sections, first.sections);
            assert_eq!(empty_cache.sidecars, first.sidecars);
            // A rejected sidecar is regenerated before a populated SH cache hits.
            let name = empty_cache.sidecars.keys().next().unwrap();
            std::fs::write(fixture.materials().join(name), b"truncated prm").unwrap();
            let repaired = fixture.bake(true, 1);
            all_hit(&repaired, &empty_cache);
            assert_eq!(repaired.sections, first.sections);
            assert_eq!(repaired.sidecars, first.sidecars);
        } else {
            cold_reference = Some(first);
        }
    }
}

/// Keep the IHDR and chunk layout valid, but break compressed IDAT bytes.
/// Header-only validation succeeds; the production texel decoder must fail.
fn corrupt_pixels(path: &Path) {
    let mut bytes = std::fs::read(path).unwrap();
    let mut offset = 8;
    loop {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        if &bytes[offset + 4..offset + 8] == b"IDAT" {
            assert!(length > 0);
            bytes[offset + 8] ^= 0xff;
            break;
        }
        offset += length + 12;
        assert!(offset < bytes.len(), "fixture PNG has IDAT");
    }
    std::fs::write(path, bytes).unwrap();
}

#[test]
#[ignore = "focused compiler refusal-before-materials gate"]
fn budget_and_invalid_animation_fail_before_corrupt_world_texture_is_read() {
    let fixture = Fixture::new();
    corrupt_pixels(&fixture.texture("red"));
    let refused = fixture.run(false, 1, &["--working-set-max-size", "0"]);
    let log = diagnostic(&refused);
    assert!(!refused.status.success());
    assert!(
        log.contains("SH delta working-set gate refused before dense baking"),
        "{log}"
    );
    assert!(
        !log.contains("decoding PNG"),
        "gate must beat bad source: {log}"
    );
    assert!(!log.contains(SH_START), "{log}");
    assert!(sidecars(&fixture.materials()).is_empty());

    let map = std::fs::read_to_string(&fixture.map).unwrap().replacen(
        "\"_color\" \"255 255 255\"", "\"_color\" \"255 255 255\"\n\"color_curve\" \"[0, 1, 0, 0] [500, 0, 1, 0]\"\n\"period_ms\" \"500\"", 1,
    );
    std::fs::write(&fixture.map, map).unwrap();
    let invalid = fixture.run(false, 1, &[]);
    let log = diagnostic(&invalid);
    assert!(!invalid.status.success());
    assert!(log.contains("color animation is only valid on"), "{log}");
    assert!(
        !log.contains("Texture color-space validation..."),
        "invalid animation must precede texture-root reads: {log}"
    );
    assert!(!log.contains(SH_START), "{log}");
    assert!(sidecars(&fixture.materials()).is_empty());
}

#[test]
#[ignore = "focused world-versus-fluid decode failure ordering gate"]
fn world_decode_failure_precedes_sh_while_fluid_only_failure_follows_both_sh_bakes() {
    for (name, late) in [("red", false), ("water", true)] {
        let fixture = Fixture::new();
        corrupt_pixels(&fixture.texture(name));
        let output = fixture.run(false, 1, &[]);
        let log = diagnostic(&output);
        assert!(!output.status.success());
        assert!(
            log.contains("decoding PNG") && log.contains(&format!("{name}.png")),
            "pixel decode must report offending source: {log}"
        );
        assert_eq!(
            log.contains(SH_START),
            late,
            "world material decode must precede SH: {log}"
        );
        assert_eq!(
            log.contains(DELTA_START),
            late,
            "fluid-only decode must follow delta SH: {log}"
        );
        assert_eq!(
            log.contains(TAIL_START),
            late,
            "fluid-only source belongs to texture tail: {log}"
        );
        if late {
            assert!(
                !sidecars(&fixture.materials()).is_empty(),
                "world materials already baked"
            );
        }
    }
}
