//! `light_area` glow through the real compiler CLI: id 62's entries by
//! `_glow`, `_color` and compose slot, whatever the texture strength; `_glow`
//! and the `_e` map keying no lighting bake; `light` scaling the light but
//! not the glow; the loader's per-vertex ids; and maps without area lights.
//! See: context/lib/build_pipeline.md §PRL section IDs · resource_management.md §4.5

mod coloured_bounce_support;

use std::collections::BTreeMap;
use std::path::Path;

use coloured_bounce_support::fixtures::{box_brush, write_texture};
use coloured_bounce_support::{Bake, FOUR_SHADOW_SAMPLES, Fixture, cache_tally, current_compiler};
use postretro_level_format::SectionId;
use postretro_level_format::area_glow::{AREA_GLOW_FIRST_ENTRY, AreaGlowSection};
use postretro_level_format::emissive::EmissiveSection;
use postretro_level_format::lightmap::LightmapSection;

const TALLY_LOG: &str = "info,prl_build::delta_sh_cache=debug";
const GLASS: &str = "bounce/glass";

/// Write a solid 16 × 16 `textures/bounce/<name>_e.png`.
fn write_glow_map(root: &Path, name: &str, rgb: [u8; 3]) {
    write_texture(root, &format!("{name}_e"), rgb);
}

/// The sealed white room with a pool (so reflection capture runs), a
/// worldspawn panel painted `glass` on the east wall, and one `light_area`
/// whose `glass` slab hangs from the ceiling beside a white frame. `keys`
/// are the area light's extra `"key" "value"` lines.
fn area_map(glass: &str, keys: &str) -> String {
    let white = "bounce/white";
    let world = [
        box_brush([-144, -144, -16], [144, 144, 0], white),
        box_brush([-144, -144, 128], [144, 144, 144], white),
        box_brush([-144, -144, 0], [-128, 144, 128], white),
        box_brush([128, -144, 0], [144, 144, 128], white),
        box_brush([-128, -144, 0], [128, -128, 128], white),
        box_brush([-128, 128, 0], [128, 144, 128], white),
        box_brush([120, -16, 32], [128, 16, 64], glass),
    ]
    .join("\n");
    let slab = box_brush([-32, -32, 120], [32, 32, 128], glass);
    let frame = box_brush([32, -32, 120], [40, 32, 128], white);
    let water = box_brush([-96, -96, 0], [-48, -48, 16], "bounce/water");
    format!(
        "{{\n\"classname\" \"worldspawn\"\n{world}\n}}\n\
         {{\n\"classname\" \"player_spawn\"\n\"origin\" \"64 64 32\"\n}}\n\
         {{\n\"classname\" \"light_area\"\n\"light\" \"300\"\n\"_color\" \"255 0 0\"\n\
         \"_falloff_range\" \"256\"\n{keys}{slab}\n{frame}\n}}\n\
         {{\n\"classname\" \"fluid_volume\"\n\"fluid\" \"water\"\n{water}\n}}\n"
    )
}

/// The same room with no area light: a static point light instead.
fn plain_map() -> String {
    let text = area_map(GLASS, "");
    let start = text.find("{\n\"classname\" \"light_area\"").unwrap();
    let end = start + text[start..].find("}\n}\n").unwrap() + 4;
    let light = "{\n\"classname\" \"light\"\n\"origin\" \"0 0 96\"\n\"light\" \"300\"\n\
                 \"_falloff_range\" \"256\"\n}\n";
    format!("{}{light}{}", &text[..start], &text[end..])
}

fn fixture(map: &str) -> Fixture {
    let fixture = Fixture::new(map, true);
    for name in ["glass", "pane_white", "pane_red"] {
        write_texture(fixture.root(), name, [255, 255, 255]);
    }
    write_glow_map(fixture.root(), "glass", [255, 255, 255]);
    write_glow_map(fixture.root(), "pane_white", [255, 255, 255]);
    write_glow_map(fixture.root(), "pane_red", [255, 0, 0]);
    fixture
}

fn materials(fixture: &Fixture, toml: &str) {
    std::fs::write(fixture.root().join("materials.toml"), toml).unwrap();
}

fn bake(fixture: &Fixture, label: &str, cache: bool) -> Bake {
    let cache_dir = fixture.root().join("cache").to_string_lossy().into_owned();
    let mut flags = vec!["--jobs", "2"];
    flags.extend(FOUR_SHADOW_SAMPLES);
    if cache {
        flags.extend(["--cache-dir", cache_dir.as_str()]);
    } else {
        flags.push("--no-cache");
    }
    fixture.bake(&current_compiler(), label, TALLY_LOG, &flags)
}

fn area_glow(bake: &Bake) -> AreaGlowSection {
    AreaGlowSection::from_bytes(bake.section(SectionId::AreaGlow)).expect("id 62 decodes")
}

fn strength(bake: &Bake, name: &str) -> f32 {
    let emissive = EmissiveSection::from_bytes(bake.section(SectionId::Emissive)).unwrap();
    let index = bake.names().iter().position(|n| n == name).unwrap();
    emissive.texture_strength(index)
}

/// Every section but `except`, which must match between the two bakes.
fn same_sections_but(a: &Bake, b: &Bake, except: &[SectionId], case: &str) {
    let skip: Vec<u32> = except.iter().map(|id| *id as u32).collect();
    let keys = |bake: &Bake| -> Vec<u32> { bake.sections.keys().copied().collect() };
    assert_eq!(keys(a), keys(b), "{case}: the same sections are written");
    for (id, payload) in &a.sections {
        if skip.contains(id) {
            continue;
        }
        assert!(
            b.sections[id] == *payload,
            "{case}: section {id} ({:?}) differs",
            SectionId::from_u32(*id)
        );
    }
}

/// Every cached stage but reflection capture reuses every entry, and
/// reflection capture misses.
fn only_capture_misses(bake: &Bake, case: &str) {
    let tally = cache_tally(&bake.log);
    assert_eq!(
        tally.get("reflection_probes").copied(),
        Some((0, 1)),
        "{case}: reflection capture must miss:\n{}",
        bake.log
    );
    // The lightmap reads its whole-section memo or its layers; SH its
    // gathered volume or its patch layers; each hits the cache.
    for stages in [
        &["lightmap_section", "lightmap_layer"][..],
        &["sh_base_volume", "sh_patch_layer"],
        &["direct_sh_volume"],
        &["billboard_direct_scatter"],
    ] {
        assert!(
            stages
                .iter()
                .any(|stage| tally.get(*stage).is_some_and(|entries| entries.0 > 0)),
            "{case}: {stages:?} hit the cache:\n{}",
            bake.log
        );
    }
    for (stage, (_, misses)) in tally {
        if stage != "reflection_probes" {
            assert_eq!(misses, 0, "{case}: {stage} must stay warm:\n{}", bake.log);
        }
    }
}

// Acceptance rows 45 and 52. A static area light's `_e` faces carry
// `_glow × _color` whatever their texture's strength (0, 1 or 4), while the
// same texture on a worldspawn panel keeps that strength. `_glow` 0, 0.5 and
// 8 scale the entry and leave every lightmap, SH, scatter, mover-channel and
// bounce-delta section byte-identical; a warm rebuild after each `_glow`
// edit reads every lighting entry from the cache and misses only reflection
// capture. `_glow` 100 reads 64 with one warning naming the light and key;
// 64 compiles silently; a negative `_glow` fails the build.
#[test]
fn glow_follows_glow_and_colour_and_keys_only_reflection_capture() {
    let mut fixture = fixture(&area_map(GLASS, ""));
    fixture.reach_verify = false;
    materials(&fixture, "[\"bounce/glass\"]\nemissive = 0\n");
    let base = bake(&fixture, "base", true);
    let glow = area_glow(&base);
    let [light] = glow.lights.as_slice() else {
        panic!("one area light: {:?}", glow.lights);
    };
    assert_eq!(light.glow, 4.0, "the default `_glow`");
    assert_eq!(
        light.color,
        [1.0, 0.0, 0.0],
        "a white pane on a red light glows red"
    );
    assert_eq!(light.compose_slot, None, "static");
    // The underside and three sides; the top is flush with the ceiling and
    // the fourth side meets the frame.
    assert_eq!(
        light.faces.len(),
        4,
        "the slab's exposed glass: {:?}",
        light.faces
    );
    assert_eq!(
        strength(&base, GLASS),
        0.0,
        "the worldspawn panel stays dark"
    );

    // The loader stamps the glass faces with area ids from 257; the panel,
    // in no volume, stays 0.
    let world = postretro_level_loader::load_prl(base.prl.to_str().unwrap()).expect("loads");
    assert_eq!(world.area_glow, glow);
    let ids: std::collections::BTreeSet<u32> =
        world.vertex_emissive_volumes.iter().copied().collect();
    let expected: std::collections::BTreeSet<u32> = (0..light.faces.len())
        .map(|k| (AREA_GLOW_FIRST_ENTRY + k) as u32)
        .chain([0])
        .collect();
    assert_eq!(ids, expected);

    for strength_value in [1.0f32, 4.0] {
        materials(
            &fixture,
            &format!("[\"bounce/glass\"]\nemissive = {strength_value}\n"),
        );
        let edited = bake(&fixture, "strength", true);
        assert_eq!(
            strength(&edited, GLASS),
            strength_value,
            "the panel's strength"
        );
        assert_eq!(area_glow(&edited), glow, "the glass ignores its strength");
        same_sections_but(
            &base,
            &edited,
            &[SectionId::Emissive, SectionId::ReflectionProbes],
            "strength",
        );
    }
    materials(&fixture, "[\"bounce/glass\"]\nemissive = 0\n");

    for (raw, read) in [
        ("0", 0.0f32),
        ("0.5", 0.5),
        ("8", 8.0),
        ("100", 64.0),
        ("64", 64.0),
    ] {
        fixture.edit_map(|_| area_map(GLASS, &format!("\"_glow\" \"{raw}\"\n")));
        let edited = bake(&fixture, "glow", true);
        let edited_glow = area_glow(&edited);
        assert_eq!(edited_glow.lights[0].glow, read, "`_glow` {raw}");
        assert_eq!(edited_glow.lights[0].faces, light.faces);
        same_sections_but(
            &base,
            &edited,
            &[SectionId::AreaGlow, SectionId::ReflectionProbes],
            &format!("`_glow` {raw}"),
        );
        if raw == "64" {
            // The same id 62 as `_glow` 100 just built: every entry is warm.
            let tally = cache_tally(&edited.log);
            assert!(
                tally.values().all(|(_, misses)| *misses == 0),
                "`_glow` 64 after 100 rebuilds nothing: {tally:?}"
            );
        } else {
            only_capture_misses(&edited, &format!("`_glow` {raw}"));
        }
        // The plain reporter echoes each warning indented in its summary;
        // count the logged records.
        let warnings = edited
            .log
            .lines()
            .filter(|line| {
                line.starts_with("[WARN")
                    && line.contains("`_glow` 100 is past the emissive ceiling; it reads 64")
            })
            .count();
        if raw == "100" {
            assert_eq!(warnings, 1, "one warning:\n{}", edited.log);
            assert!(
                edited.log.contains("'light_area': `_glow` 100"),
                "{}",
                edited.log
            );
        } else {
            assert_eq!(warnings, 0, "`_glow` {raw} compiles silently");
            assert!(!edited.log.contains("emissive ceiling"), "{}", edited.log);
        }
    }

    fixture.edit_map(|_| area_map(GLASS, "\"_glow\" \"-1\"\n"));
    let failed = fixture.run(&current_compiler(), "negative", TALLY_LOG, &["--no-cache"]);
    assert!(!failed.succeeded(), "a negative `_glow` fails");
    assert!(
        failed.log.contains("'light_area'") && failed.log.contains("_glow"),
        "the failure names the light and key:\n{}",
        failed.log
    );
}

// Acceptance row 51: two otherwise equal area lights on the same diffuse,
// one on a white all-glowing `_e` at strength 4, one on a red all-glowing
// `_e` at default strength, bake byte-identical lighting: neither the `_e`
// map nor its strength reaches a lighting bake.
#[test]
fn white_and_red_glass_bake_byte_identical_lighting() {
    let white = fixture(&area_map("bounce/pane_white", ""));
    materials(&white, "[\"bounce/pane_white\"]\nemissive = 4\n");
    let red = fixture(&area_map("bounce/pane_red", ""));
    let a = bake(&white, "white", false);
    let b = bake(&red, "red", false);
    assert_eq!(strength(&a, "bounce/pane_white"), 4.0);
    assert_eq!(strength(&b, "bounce/pane_red"), 1.0);
    same_sections_but(
        &a,
        &b,
        &[
            SectionId::TextureNames,
            SectionId::TextureCacheKeys,
            SectionId::Emissive,
            SectionId::ReflectionProbes,
        ],
        "white vs red `_e`",
    );
    assert_eq!(area_glow(&a), area_glow(&b));
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

/// Every lightmap irradiance channel the bake stored, in block order.
fn irradiance(bake: &Bake) -> Vec<f32> {
    let lightmap = LightmapSection::from_bytes(bake.section(SectionId::Lightmap)).unwrap();
    let mut values = Vec::new();
    for block in 0..lightmap.blocks.len() {
        let (irradiance, _) = lightmap.decode_block(block).unwrap();
        values.extend(
            irradiance
                .as_chunks::<2>()
                .0
                .iter()
                .enumerate()
                .filter(|(i, _)| i % 4 != 3)
                .map(|(_, bytes)| f16_to_f32(u16::from_le_bytes(*bytes))),
        );
    }
    values
}

// Acceptance row 50 (research.md O2): with `_falloff_range` set, doubling an
// area light's `light` doubles its lightmap and leaves its faces' glow
// entry, the emissive term, unchanged.
#[test]
fn doubling_light_doubles_the_lightmap_and_leaves_the_glow() {
    let one = fixture(&area_map(GLASS, ""));
    let two = fixture(&area_map(GLASS, "").replace("\"light\" \"300\"", "\"light\" \"600\""));
    let a = bake(&one, "one", false);
    let b = bake(&two, "two", false);
    let (glow_a, glow_b) = (area_glow(&a), area_glow(&b));
    assert_eq!(glow_b.lights[0].intensity, 2.0 * glow_a.lights[0].intensity);
    assert_eq!(glow_a.lights[0].faces, glow_b.lights[0].faces);
    assert_eq!(
        postretro_render_cpu::emissive_volume::area_glow_entry(&glow_a.lights[0]),
        postretro_render_cpu::emissive_volume::area_glow_entry(&glow_b.lights[0]),
        "the faces' emissive term ignores `light`"
    );
    let (lit_a, lit_b) = (irradiance(&a), irradiance(&b));
    assert_eq!(lit_a.len(), lit_b.len());
    let mut lit = 0;
    for (x, y) in lit_a.iter().zip(&lit_b) {
        assert!(
            (y - 2.0 * x).abs() <= 2e-3 * (2.0 * x).abs() + 1e-6,
            "{x} doubles to {y}"
        );
        lit += usize::from(*x > 1e-3);
    }
    assert!(lit > 100, "the room is lit: {lit} channels");
}

// Acceptance row 60 (research.md O5): a map with no area light writes no
// id 62, and the loader builds no area ids.
#[test]
fn a_map_without_area_lights_writes_no_area_glow() {
    let plain = fixture(&plain_map());
    let bake = bake(&plain, "plain", false);
    assert!(!bake.sections.contains_key(&(SectionId::AreaGlow as u32)));
    let world = postretro_level_loader::load_prl(bake.prl.to_str().unwrap()).expect("loads");
    assert!(world.area_glow.lights.is_empty());
    assert!(
        world.vertex_emissive_volumes.is_empty(),
        "no volume, no area: no stream"
    );
    let _: BTreeMap<u32, Vec<u8>> = bake.sections;
}
