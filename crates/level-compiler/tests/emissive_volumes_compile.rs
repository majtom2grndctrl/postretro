//! `emissive_volume` compiles into id 60 only: a default volume around glowing
//! faces leaves every other section byte-identical, reflection probes included,
//! and the section reads back each volume's record and member faces.
//! See: context/lib/build_pipeline.md §Compiler pipeline · resource_management.md §4.5

mod prl_build_support;

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Output;

use postretro_level_format::emissive::EmissiveSection;
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_level_format::{SectionId, read_container, read_section_data};

use prl_build_support::{BuildDir, link_dev_textures, prl_build};

const WALL: &str = "50-free-textures/concrete_pavement_036";
/// A dev texture with an `_e` map.
const PANEL: &str = "neon/glow_panel";
/// Quake map units to engine meters.
const UNIT: f32 = 0.0254;

fn box_brush(min: [i32; 3], max: [i32; 3], texture: &str) -> String {
    box_brush_facing(min, max, texture, texture)
}

/// [`box_brush`] whose side at `min.x`, facing −x, shows `low_x_texture`.
fn box_brush_facing(min: [i32; 3], max: [i32; 3], texture: &str, low_x_texture: &str) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {low_x_texture} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {texture} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {texture} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {texture} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {texture} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {texture} 0 0 0 1 1\n\
         }}"
    )
}

/// A sealed room (interior map units −128..128, 0..128) whose +x wall shows
/// a glowing texture on its inner side, a pool on the floor, so the level
/// has a reflection probe that sees that wall, and a light. The glow is on a
/// wall rather than a small panel brush: capture exclusion drops a small
/// brush near a probe, and the probe must see the glow. `volumes` follow
/// every other entity, so no entity index moves.
fn room_map(volumes: &[String]) -> String {
    let brushes = [
        box_brush([-144, -144, -16], [144, 144, 0], WALL),
        box_brush([-144, -144, 128], [144, 144, 144], WALL),
        box_brush([-144, -144, 0], [-128, 144, 128], WALL),
        box_brush_facing([128, -144, 0], [144, 144, 128], WALL, PANEL),
        box_brush([-128, -144, 0], [128, -128, 128], WALL),
        box_brush([-128, 128, 0], [128, 144, 128], WALL),
    ];
    let mut entities = vec![
        format!(
            "{{\n\"classname\" \"worldspawn\"\n{}\n}}",
            brushes.join("\n")
        ),
        "{\n\"classname\" \"player_spawn\"\n\"origin\" \"-64 0 32\"\n}".to_string(),
        "{\n\"classname\" \"light\"\n\"origin\" \"0 0 96\"\n\"light\" \"300\"\n\
         \"_falloff_range\" \"320\"\n\"_light_size\" \"0\"\n}"
            .to_string(),
        format!(
            "{{\n\"classname\" \"fluid_volume\"\n\"fluid\" \"water\"\n{}\n}}",
            box_brush([-96, -96, 0], [0, 96, 16], WALL)
        ),
    ];
    entities.extend(volumes.iter().cloned());
    entities.join("\n")
}

/// An `emissive_volume` around the glowing wall side's centre (map
/// (128, 0, 64)), clear of every other side's, with `properties` (one
/// `"key" "value"` per line).
fn panel_volume(properties: &str) -> String {
    format!(
        "{{\n\"classname\" \"emissive_volume\"\n{properties}{}\n}}",
        box_brush([100, -48, 24], [136, 48, 104], "trigger")
    )
}

fn compile(root: &Path, name: &str, map_text: &str) -> (PathBuf, String) {
    let input = root.join("maps").join(format!("{name}.map"));
    let output_path = root.join(format!("{name}.prl"));
    std::fs::write(&input, format!("{map_text}\n")).expect("write fixture map");
    let output = prl_build()
        .env("RUST_LOG", "warn")
        .arg(&input)
        .arg("-o")
        .arg(&output_path)
        .arg("--no-cache")
        .arg("--no-tui")
        .arg("--baked-root")
        .arg(root.join("baked"))
        .arg("--sh-probe-spacing")
        .arg("4")
        .arg("--lightmap-density")
        .arg("0.25")
        .output()
        .expect("spawn prl-build");
    assert!(
        output.status.success(),
        "prl-build failed for {name}\n{}",
        combined_logs(&output)
    );
    (output_path, combined_logs(&output))
}

fn combined_logs(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Every section payload, keyed by section id.
fn sections(prl: &Path) -> BTreeMap<u32, Vec<u8>> {
    let bytes = std::fs::read(prl).expect("read compiled PRL");
    let mut cursor = Cursor::new(bytes);
    let metadata = read_container(&mut cursor).expect("read PRL container");
    let ids: Vec<u32> = metadata
        .sections
        .iter()
        .map(|entry| entry.section_id)
        .collect();
    ids.into_iter()
        .map(|id| {
            let payload = read_section_data(&mut cursor, &metadata, id)
                .unwrap_or_else(|error| panic!("read section {id}: {error}"))
                .unwrap_or_else(|| panic!("section {id} is listed"));
            (id, payload)
        })
        .collect()
}

fn section(sections: &BTreeMap<u32, Vec<u8>>, id: SectionId) -> &[u8] {
    sections
        .get(&(id as u32))
        .unwrap_or_else(|| panic!("{id:?} must be present"))
}

fn emissive(sections: &BTreeMap<u32, Vec<u8>>) -> EmissiveSection {
    EmissiveSection::from_bytes(section(sections, SectionId::Emissive)).expect("Emissive decodes")
}

/// The Geometry faces whose texture is `texture`, ascending.
fn faces_of(sections: &BTreeMap<u32, Vec<u8>>, texture: &str) -> Vec<u32> {
    let names = TextureNamesSection::from_bytes(section(sections, SectionId::TextureNames))
        .expect("TextureNames decodes")
        .names;
    let index = names
        .iter()
        .position(|name| name == texture)
        .unwrap_or_else(|| panic!("{texture} is a texture name: {names:?}")) as u32;
    let geometry = GeometrySection::from_bytes(section(sections, SectionId::Geometry))
        .expect("Geometry decodes");
    (0..geometry.faces.len() as u32)
        .filter(|&face| geometry.faces[face as usize].texture_index == index)
        .collect()
}

fn setup() -> BuildDir {
    let dir = BuildDir::temp();
    std::fs::create_dir_all(dir.path().join("maps")).expect("create maps directory");
    link_dev_textures(dir.path());
    dir
}

// Acceptance A6: a default volume (min 0, max 1, white) around `_e` faces
// changes only the emissive section; reflection probes, which shade the
// glowing wall at strength × 1 × white, stay byte-identical. (The authored
// volume below proves the probe sees that wall.)
#[test]
fn a_default_volume_changes_only_the_emissive_section() {
    let dir = setup();
    let (base_prl, _) = compile(dir.path(), "base", &room_map(&[]));
    let base = sections(&base_prl);
    section(&base, SectionId::ReflectionProbes);
    assert!(emissive(&base).volumes.is_empty());

    let (with_prl, logs) = compile(dir.path(), "with", &room_map(&[panel_volume("")]));
    let with_volume = sections(&with_prl);
    assert!(
        !logs.contains("emissive_volume"),
        "a volume around glowing faces compiles silently:\n{logs}"
    );
    assert_eq!(
        with_volume.keys().collect::<Vec<_>>(),
        base.keys().collect::<Vec<_>>(),
        "the same sections are emitted"
    );
    for (id, payload) in &base {
        if *id == SectionId::Emissive as u32 {
            continue;
        }
        assert!(
            with_volume[id] == *payload,
            "section {id} ({:?}) differs from the map without the volume",
            SectionId::from_u32(*id)
        );
    }
    assert_eq!(
        section(&with_volume, SectionId::ReflectionProbes),
        section(&base, SectionId::ReflectionProbes),
        "the capture shades a default volume's faces as it shades them without one"
    );

    let base_emissive = emissive(&base);
    let emissive = emissive(&with_volume);
    assert_eq!(emissive.textures, base_emissive.textures);
    let [volume] = emissive.volumes.as_slice() else {
        panic!("one volume: {:?}", emissive.volumes);
    };
    let panel = faces_of(&with_volume, PANEL);
    assert!(!panel.is_empty());
    assert_eq!(
        volume.faces, panel,
        "the glowing side's faces, and no other"
    );
    assert_eq!((volume.min, volume.max, volume.color), (0.0, 1.0, [1.0; 3]));
    assert!(volume.tags.is_empty());
}

// Acceptance A7 (compile half): id 60 reads back a volume's fixed range,
// linear colour, origin, tags and member faces; the runtime loader accepts
// it; and its max and colour reach the reflection capture.
#[test]
fn a_volumes_record_and_faces_read_back_from_the_emissive_section() {
    let dir = setup();
    let (base_prl, _) = compile(dir.path(), "base", &room_map(&[]));
    let base = sections(&base_prl);
    let (prl, logs) = compile(
        dir.path(),
        "authored",
        &room_map(&[panel_volume(
            "\"emissive_min\" \"5\"\n\"emissive_max\" \"0.5\"\n\
             \"emissive_color\" \"255 128 0\"\n\"_tags\" \"bar_sign lobby\"\n",
        )]),
    );
    assert!(
        logs.contains("entity 4 'emissive_volume' `emissive_max` 0.5 is below `emissive_min` 5"),
        "the swap warns:\n{logs}"
    );
    let compiled = sections(&prl);
    let [volume] = emissive(&compiled).volumes.try_into().expect("one volume");
    assert_eq!((volume.min, volume.max), (0.5, 5.0), "swapped");
    assert_eq!(volume.color[0], 1.0);
    assert!((volume.color[1] - 128.0 / 255.0).abs() < 1e-6);
    assert_eq!(volume.color[2], 0.0);
    assert_eq!(volume.tags, ["bar_sign", "lobby"]);
    assert_eq!(volume.faces, faces_of(&compiled, PANEL));
    // The union AABB's centre, map (118, 0, 64): engine (−y, z, −x).
    let origin = [0.0, 64.0 * UNIT, -118.0 * UNIT];
    for (actual, expected) in volume.origin.iter().zip(origin) {
        assert!((actual - expected).abs() < 1e-5, "{:?}", volume.origin);
    }

    let world = postretro_level_loader::load_prl(prl.to_str().expect("UTF-8 path"))
        .expect("the compiled level loads");
    assert_eq!(world.emissive.volumes, emissive(&compiled).volumes);

    assert_ne!(
        section(&compiled, SectionId::ReflectionProbes),
        section(&base, SectionId::ReflectionProbes),
        "the probe reflects the glowing wall at max × colour"
    );
}
