//! The dev mod's emissive panels after the `neon` prefix retired: combat-demo
//! and reflection-probes-test compile with no `emissive_volume`, their panels
//! take strength 4.0 from `content/dev/materials.toml`, and combat-demo's bloom
//! A/B holds — the bright panel's peak glow clears the bloom threshold and the
//! dim panel's stays under it.
//! See: context/lib/resource_management.md §4.5 · content/dev/maps/combat-demo.README.md

mod prl_build_support;

use std::path::{Path, PathBuf};

use postretro_level_format::emissive::EmissiveSection;
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_level_format::{SectionId, read_container, read_section_data};

use prl_build_support::prl_build;

const BRIGHT_PANEL: &str = "neon/glow_panel";
const DIM_PANEL: &str = "neon/dim_panel";
/// The strength `content/dev/materials.toml` gives both panels.
const PANEL_STRENGTH: f32 = 4.0;

/// The renderer's bright-pass threshold, read from its source: the compiler
/// cannot depend on the renderer, and a copied literal would drift.
fn bloom_threshold() -> f32 {
    const SOURCE: &str = include_str!("../../renderer/src/render/bloom.rs");
    const DECLARATION: &str = "pub const BLOOM_THRESHOLD: f32 =";
    let at = SOURCE
        .find(DECLARATION)
        .expect("the renderer declares BLOOM_THRESHOLD");
    let value = &SOURCE[at + DECLARATION.len()..];
    value[..value.find(';').expect("the declaration ends")]
        .trim()
        .parse()
        .expect("BLOOM_THRESHOLD is a float literal")
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Compile `content/dev/maps/<map>.map` and return its texture names and
/// emissive section.
fn compile(map: &str) -> (Vec<String>, EmissiveSection) {
    let source = workspace().join(format!("content/dev/maps/{map}.map"));
    let text = std::fs::read_to_string(&source).expect("read the map");
    assert!(
        !text.contains("emissive_volume"),
        "`{map}` must keep its look with no emissive volume"
    );
    let out = tempfile::tempdir().expect("temp output directory");
    let prl = out.path().join(format!("{map}.prl"));
    let output = prl_build()
        .arg(&source)
        .arg("-o")
        .arg(&prl)
        .arg("--no-tui")
        .arg("--no-cache")
        .arg("--baked-root")
        .arg(out.path().join("baked"))
        .output()
        .expect("spawn prl-build");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "`{map}` failed to compile\nstdout:\n{}\nstderr:\n{stderr}",
        String::from_utf8_lossy(&output.stdout),
    );
    for quiet in ["materials.toml", "Unknown prefix"] {
        assert!(
            !stderr.contains(quiet),
            "`{map}` must compile without a `{quiet}` warning:\n{stderr}"
        );
    }

    let mut cursor = std::io::Cursor::new(std::fs::read(&prl).expect("read PRL"));
    let metadata = read_container(&mut cursor).expect("container");
    let mut section = |id: SectionId| {
        read_section_data(&mut cursor, &metadata, id as u32)
            .expect("read section")
            .unwrap_or_else(|| panic!("`{map}` has no {id:?} section"))
    };
    let names = TextureNamesSection::from_bytes(&section(SectionId::TextureNames))
        .expect("texture names decode")
        .names;
    let emissive =
        EmissiveSection::from_bytes(&section(SectionId::Emissive)).expect("emissive decodes");
    assert!(emissive.volumes.is_empty(), "`{map}` bakes no volume");
    (names, emissive)
}

/// The baked strength of the texture named `name`.
fn baked_strength(names: &[String], emissive: &EmissiveSection, name: &str) -> f32 {
    let index = names
        .iter()
        .position(|candidate| candidate.eq_ignore_ascii_case(name))
        .unwrap_or_else(|| panic!("no texture `{name}` in {names:?}"));
    emissive.texture_strength(index)
}

fn srgb_to_linear(byte: u8) -> f32 {
    let c = f32::from(byte) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The highest Rec.709 luminance of any texel of `<name>_e.png`, in linear
/// light: the measure `bloom_extract.wgsl` thresholds.
fn peak_glow_luminance(name: &str) -> f32 {
    let path = workspace().join(format!("content/dev/textures/{name}_e.png"));
    let image = image::open(&path)
        .unwrap_or_else(|error| panic!("decode {}: {error}", path.display()))
        .to_rgba8();
    image
        .pixels()
        .map(|pixel| {
            let [r, g, b, _] = pixel.0;
            0.2126 * srgb_to_linear(r) + 0.7152 * srgb_to_linear(g) + 0.0722 * srgb_to_linear(b)
        })
        .fold(0.0, f32::max)
}

#[test]
fn combat_demo_panels_keep_their_bloom_split_with_no_volume() {
    let (names, emissive) = compile("combat-demo");
    let threshold = bloom_threshold();
    for panel in [BRIGHT_PANEL, DIM_PANEL] {
        assert_eq!(
            baked_strength(&names, &emissive, panel),
            PANEL_STRENGTH,
            "{panel}"
        );
    }

    let bright = PANEL_STRENGTH * peak_glow_luminance(BRIGHT_PANEL);
    let dim = PANEL_STRENGTH * peak_glow_luminance(DIM_PANEL);
    assert!(
        bright > threshold,
        "the bright panel peaks at {bright}, under the bloom threshold {threshold}"
    );
    assert!(
        dim < threshold,
        "the dim panel peaks at {dim}, over the bloom threshold {threshold}"
    );
}

#[test]
#[ignore = "compiles reflection-probes-test (~11 s wall, ~1 CPU-minute); run with --ignored"]
fn reflection_probes_test_compiles_its_panel_at_strength_four_with_no_volume() {
    let (names, emissive) = compile("reflection-probes-test");
    assert_eq!(
        baked_strength(&names, &emissive, BRIGHT_PANEL),
        PANEL_STRENGTH
    );
}
