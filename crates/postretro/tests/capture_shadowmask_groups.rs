// GPU-backed proof that side-by-side BC5 shadowmask groups route every
// selected light to its own mask slot in the world-specular decode path.
// See: context/lib/rendering_pipeline.md §4 (World specular shadowmask)
//
// The fixture's four coloured static lights overlap, so the bake gives them
// all four slots: two per BC5 group. Each variant rewrites id 42 of the
// compiled PRL with constant per-slot planes (exact under BC5) and a chosen
// slot table, so the only thing that differs between captures is which slot
// gates which light. Captures are compared on one adapter; no golden images.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use image::RgbaImage;
use postretro_level_format as prl_format;
use postretro_level_format::SectionId;
use postretro_level_format::shadowmask_atlas::{
    SHADOWMASK_CHANNEL_DROPPED, SHADOWMASK_GROUP_COUNT, ShadowmaskAtlasSection,
};

const CAPTURE_WIDTH: u32 = 160;
const CAPTURE_HEIGHT: u32 = 120;
const SLOT_COUNT: u8 = 4;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("postretro crate must be two levels below the workspace root")
        .to_path_buf()
}

/// Compile the fixture beside its source so capture derives the dev material
/// tree from the standard `content/dev/maps` layout.
fn compile_fixture(workspace: &Path, map_name: &str) -> tempfile::TempPath {
    let source_map = workspace.join(format!("content/dev/maps/{map_name}.map"));
    assert!(
        source_map.is_file(),
        "fixture missing: {}",
        source_map.display()
    );
    let map = tempfile::Builder::new()
        .prefix(&format!(".{map_name}-"))
        .suffix(".prl")
        .tempfile_in(workspace.join("content/dev/maps"))
        .expect("reserve fixture PRL path in content/dev/maps")
        .into_temp_path();
    let compile = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "run",
            "--quiet",
            "-p",
            "postretro-level-compiler",
            "--bin",
            "prl-build",
            "--",
        ])
        .arg(&source_map)
        .arg("-o")
        .arg(&map)
        .arg("--no-tui")
        .current_dir(workspace)
        .output()
        .expect("launch prl-build");
    assert!(
        compile.status.success(),
        "fixture compile failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr),
    );
    map
}

fn read_sections(path: &Path) -> Vec<prl_format::SectionBlob> {
    let mut file = fs::File::open(path).expect("open compiled fixture");
    let meta = prl_format::read_container(&mut file).expect("read fixture container");
    meta.sections
        .iter()
        .map(|entry| prl_format::SectionBlob {
            section_id: entry.section_id,
            version: entry.version,
            data: prl_format::read_section_data(&mut file, &meta, entry.section_id)
                .expect("read fixture section")
                .expect("listed section must be present"),
        })
        .collect()
}

fn shadowmask_of(sections: &[prl_format::SectionBlob]) -> ShadowmaskAtlasSection {
    let blob = sections
        .iter()
        .find(|blob| blob.section_id == SectionId::ShadowmaskAtlas as u32)
        .expect("fixture must ship a ShadowmaskAtlas section");
    ShadowmaskAtlasSection::from_bytes(&blob.data).expect("fixture ShadowmaskAtlas must parse")
}

/// A payload whose every texel holds `slots[s]` in slot `s`. A constant BC4
/// block is `[v, v, 0, 0, 0, 0, 0, 0]`, which decodes to exactly `v`.
fn constant_payload(section: &ShadowmaskAtlasSection, slots: [u8; 4]) -> Vec<u8> {
    let block_columns_per_group = (section.width / 4) as usize;
    let block_rows = (section.height / 4) as usize;
    let bc4 = |value: u8| [value, value, 0, 0, 0, 0, 0, 0];
    let mut payload = Vec::with_capacity(section.data.len());
    for _layer in 0..section.layer_count {
        for _row in 0..block_rows {
            for group in 0..SHADOWMASK_GROUP_COUNT as usize {
                for _column in 0..block_columns_per_group {
                    payload.extend_from_slice(&bc4(slots[group * 2]));
                    payload.extend_from_slice(&bc4(slots[group * 2 + 1]));
                }
            }
        }
    }
    assert_eq!(payload.len(), section.data.len());
    payload
}

/// Write a copy of the fixture whose id 42 carries `channels` and `data`.
/// Both keep their original lengths, so no other section moves.
fn write_variant(
    sections: &[prl_format::SectionBlob],
    base: &ShadowmaskAtlasSection,
    channels: Vec<u8>,
    data: Vec<u8>,
    path: &Path,
) {
    let variant = ShadowmaskAtlasSection {
        channels,
        data,
        ..base.clone()
    };
    let blobs: Vec<_> = sections
        .iter()
        .map(|blob| prl_format::SectionBlob {
            section_id: blob.section_id,
            version: blob.version,
            data: if blob.section_id == SectionId::ShadowmaskAtlas as u32 {
                variant.to_bytes()
            } else {
                blob.data.clone()
            },
        })
        .collect();
    let mut file = fs::File::create(path).expect("create variant PRL");
    prl_format::write_prl(&mut file, &blobs).expect("write variant PRL");
}

/// Capture `map` looking at the north wall. Fails rather than skips without
/// an adapter: this proof does not count unless it ran.
fn capture(workspace: &Path, scratch: &Path, map: &Path, label: &str) -> RgbaImage {
    let camera = serde_json::json!({
        // Quake (500, 40, 192), facing the north wall.
        "position": [-1.016, 4.877, -12.7],
        "yaw_deg": 90.0,
        "pitch_deg": 0.0,
        "fov_deg": 90.0
    });
    capture_scene(workspace, scratch, map, label, camera, None).0
}

/// Run one capture and return its image and stderr. `force_active` is the
/// scene's optional forced-animation list.
fn capture_scene(
    workspace: &Path,
    scratch: &Path,
    map: &Path,
    label: &str,
    camera: serde_json::Value,
    force_active: Option<serde_json::Value>,
) -> (RgbaImage, String) {
    let scene_path = scratch.join(format!("{label}.scene.json"));
    let output_path = scratch.join(format!("{label}.png"));
    let mut scene = serde_json::json!({
        "map": map.display().to_string(),
        "camera": camera,
        "resolution": [CAPTURE_WIDTH, CAPTURE_HEIGHT],
        "output": output_path.display().to_string()
    });
    if let Some(force_active) = force_active {
        scene["force_active"] = force_active;
    }
    fs::write(&scene_path, serde_json::to_vec_pretty(&scene).unwrap()).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_postretro"))
        .arg("--capture")
        .arg(&scene_path)
        .env("POSTRETRO_SH_STREAMING", "sync-proof")
        .env("RUST_LOG", "warn")
        .current_dir(workspace)
        .output()
        .expect("launch postretro capture");
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(
        result.status.success(),
        "capture `{label}` failed\nstdout:\n{}\nstderr:\n{stderr}",
        String::from_utf8_lossy(&result.stdout),
    );
    let image = image::ImageReader::open(&output_path)
        .expect("open capture PNG")
        .with_guessed_format()
        .expect("detect capture format")
        .decode()
        .expect("decode capture PNG")
        .to_rgba8();
    (image, stderr)
}

/// Per-channel light added over `base`; negative where `image` is darker.
fn added_light(image: &RgbaImage, base: &RgbaImage) -> Vec<[i32; 3]> {
    image
        .pixels()
        .zip(base.pixels())
        .map(|(a, b)| std::array::from_fn(|c| i32::from(a[c]) - i32::from(b[c])))
        .collect()
}

fn total(light: &[[i32; 3]]) -> i64 {
    light.iter().flatten().map(|&v| i64::from(v)).sum()
}

/// The slot table for "light `light` in slot `slot`": the other lights take
/// the remaining slots in ascending order.
fn table_with(light: usize, slot: u8, light_count: usize) -> Vec<u8> {
    let mut others = (0..SLOT_COUNT).filter(|&s| s != slot);
    (0..light_count)
        .map(|index| {
            if index == light {
                slot
            } else {
                others.next().expect("four slots seat four lights")
            }
        })
        .collect()
}

fn open_only(slot: u8) -> [u8; 4] {
    std::array::from_fn(|s| if s as u8 == slot { 255 } else { 0 })
}

#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_shadowmask_groups -- --ignored`"]
fn every_selected_light_reads_its_own_slot_in_either_group() {
    let workspace = workspace_root();
    let scratch = tempfile::tempdir().expect("scratch dir");
    let compiled = compile_fixture(&workspace, "shadowmask-groups-capture");
    let sections = read_sections(&compiled);
    let baked = shadowmask_of(&sections);

    let light_count = baked.channels.len();
    assert_eq!(light_count, 4, "fixture must select four static lights");
    let mut baked_slots = baked.channels.clone();
    baked_slots.sort_unstable();
    assert_eq!(
        baked_slots,
        [0, 1, 2, 3],
        "four overlapping lights must fill all four slots, none dropped"
    );
    assert!(!baked.channels.contains(&SHADOWMASK_CHANNEL_DROPPED));

    // Variants sit beside the compiled fixture for the material-tree layout;
    // their temp paths delete themselves even when an assertion fails.
    let maps_dir = compiled
        .parent()
        .expect("fixture has a parent")
        .to_path_buf();
    let mut written = Vec::new();
    let mut capture_variant = |label: &str, channels: Vec<u8>, data: Vec<u8>| {
        let path = tempfile::Builder::new()
            .prefix(&format!(".shadowmask-groups-{label}-"))
            .suffix(".prl")
            .tempfile_in(&maps_dir)
            .expect("reserve variant PRL path")
            .into_temp_path();
        write_variant(&sections, &baked, channels, data, &path);
        let image = capture(&workspace, scratch.path(), &path, label);
        written.push(path);
        image
    };

    let closed = capture_variant(
        "closed",
        baked.channels.clone(),
        constant_payload(&baked, [0; 4]),
    );
    let open = capture_variant(
        "open",
        baked.channels.clone(),
        constant_payload(&baked, [255; 4]),
    );
    let baked_masks = capture(&workspace, scratch.path(), &compiled, "baked");

    // Stay under the soft-knee tonemap, so added specular stays per-channel.
    let peak = open.pixels().flat_map(|p| p.0[..3].to_vec()).max().unwrap();
    assert!(
        peak < 250,
        "fixture must stay below the tonemap knee; peak {peak}"
    );

    let all_specular = added_light(&open, &closed);
    assert!(
        total(&all_specular) > 0,
        "opening every slot must add specular"
    );
    let baked_specular = added_light(&baked_masks, &closed);
    assert!(
        baked_specular
            .iter()
            .zip(&all_specular)
            .all(|(baked, all)| (0..3).all(|c| baked[c] <= all[c] + 1)),
        "baked masks may only remove specular relative to fully open masks"
    );
    assert!(
        total(&baked_specular) < total(&all_specular),
        "baked masks must shadow some visible specular"
    );

    let mut light_images = Vec::new();
    for light in 0..light_count {
        let per_slot: Vec<_> = (0..SLOT_COUNT)
            .map(|slot| {
                capture_variant(
                    &format!("light{light}-slot{slot}"),
                    table_with(light, slot, light_count),
                    constant_payload(&baked, open_only(slot)),
                )
            })
            .collect();
        let own = added_light(&per_slot[0], &closed);
        assert!(
            total(&own) > 0,
            "light {light} must add specular through slot 0"
        );
        for (slot, image) in per_slot.iter().enumerate().skip(1) {
            assert_eq!(
                image.as_raw(),
                per_slot[0].as_raw(),
                "light {light} must read the same mask through slot {slot} as through slot 0"
            );
        }
        light_images.push(per_slot.into_iter().next().unwrap());
    }
    for (a, first) in light_images.iter().enumerate() {
        for (b, second) in light_images.iter().enumerate().skip(a + 1) {
            assert_ne!(
                first.as_raw(),
                second.as_raw(),
                "lights {a} and {b} must add different specular"
            );
        }
    }
}

// Pin: animated-after-take. The animated contribution atlas is sized from the
// lightmap header install keeps; the payloads have already moved into the
// upload. A mismatch would fall back to the dummy atlas with a renderer error.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_shadowmask_groups -- --ignored`"]
fn animated_lights_render_after_lightmap_payloads_move_into_the_upload() {
    let workspace = workspace_root();
    let scratch = tempfile::tempdir().expect("scratch dir");
    let compiled = compile_fixture(&workspace, "spawner-test");
    // Frames the static wall in alarm_light's authored cone.
    let camera = serde_json::json!({
        "position": [6.1, 2.2, -2.5],
        "yaw_deg": 77.0,
        "pitch_deg": -12.0,
        "fov_deg": 90.0
    });
    let (rest, rest_stderr) = capture_scene(
        &workspace,
        scratch.path(),
        &compiled,
        "alarm-rest",
        camera.clone(),
        None,
    );
    let (alarm, alarm_stderr) = capture_scene(
        &workspace,
        scratch.path(),
        &compiled,
        "alarm-forced",
        camera,
        Some(serde_json::json!([{ "tag": "alarm_light", "radiance": [4.0, 0.0, 0.0] }])),
    );
    for stderr in [&rest_stderr, &alarm_stderr] {
        assert!(
            !stderr.contains("[Renderer]"),
            "the install must not degrade any lighting resource:\n{stderr}"
        );
    }
    let reddened = alarm
        .pixels()
        .zip(rest.pixels())
        .filter(|(alarm, rest)| i32::from(alarm[0]) - i32::from(rest[0]) > 12)
        .count();
    assert!(
        reddened >= 64,
        "the forced animated alarm must redden the world through the animated atlas; {reddened} pixels"
    );
}
