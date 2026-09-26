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
use std::path::Path;

use image::RgbaImage;
use postretro_level_format as prl_format;
use postretro_level_format::SectionId;
use postretro_level_format::shadowmask_atlas::{
    SHADOWMASK_CHANNEL_DROPPED, SHADOWMASK_GROUP_COUNT, ShadowmaskAtlasSection,
};

mod capture_support;

use capture_support::{capture_command, compile_dev_map, load_capture_rgba, workspace_root};

const CAPTURE_WIDTH: u32 = 160;
const CAPTURE_HEIGHT: u32 = 120;
const SLOT_COUNT: u8 = 4;

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

fn north_wall_camera() -> serde_json::Value {
    serde_json::json!({
        // Quake (500, 40, 192), facing the north wall.
        "position": [-1.016, 4.877, -12.7],
        "yaw_deg": 90.0,
        "pitch_deg": 0.0,
        "fov_deg": 90.0
    })
}

/// Capture `map` looking at the north wall. Fails rather than skips without
/// an adapter: this proof does not count unless it ran.
fn capture(workspace: &Path, scratch: &Path, map: &Path, label: &str) -> RgbaImage {
    capture_scene(workspace, scratch, map, label, north_wall_camera(), None).0
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
    // `warn` keeps every `[Renderer]` error on stderr for callers to check.
    let result = capture_command(workspace, &scene_path)
        .env("RUST_LOG", "warn")
        .output()
        .expect("launch postretro capture");
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(
        result.status.success(),
        "capture `{label}` failed\nstdout:\n{}\nstderr:\n{stderr}",
        String::from_utf8_lossy(&result.stdout),
    );
    (load_capture_rgba(&output_path), stderr)
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
    let compiled = compile_dev_map(&workspace, "shadowmask-groups-capture");
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

/// Swap the BC4 sub-blocks of slots `a` and `b` everywhere in a side-by-side
/// BC5 payload: slot `s` is group `s / 2`'s R (`s % 2 == 0`) or G block.
fn swap_slot_blocks(section: &ShadowmaskAtlasSection, a: u8, b: u8) -> Vec<u8> {
    let columns_per_group = (section.width / 4) as usize;
    let block_rows = (section.height * section.layer_count / 4) as usize;
    let sub_block = |row: usize, column: usize, slot: u8| {
        let block = row * columns_per_group * 2 + (slot as usize / 2) * columns_per_group + column;
        block * 16 + (slot as usize % 2) * 8
    };
    let mut data = section.data.clone();
    for row in 0..block_rows {
        for column in 0..columns_per_group {
            let (x, y) = (sub_block(row, column, a), sub_block(row, column, b));
            for byte in 0..8 {
                data.swap(x + byte, y + byte);
            }
        }
    }
    data
}

// Pins: seam-bleed and M4. Group placement must not change what any light
// reads: swapping which half holds the open and closed groups, or moving a
// light's baked mask from group 0 to group 1, renders byte-identically.
// Baked chart gutters keep UVs off each half's outer columns, so this proves
// routing and placement in a real frame. Driven edge UVs are proven by the
// renderer's `shadowmask_sample_test` readback.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_shadowmask_groups -- --ignored`"]
fn group_placement_never_changes_what_a_light_reads() {
    let workspace = workspace_root();
    let scratch = tempfile::tempdir().expect("scratch dir");
    let compiled = compile_dev_map(&workspace, "shadowmask-groups-capture");
    let sections = read_sections(&compiled);
    let baked = shadowmask_of(&sections);
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

    // Seam: the two halves differ everywhere (255 against 0). Lights 0 and 1
    // stay open whether their group sits left or right of the seam.
    let open_left = capture_variant(
        "open-left",
        vec![0, 1, 2, 3],
        constant_payload(&baked, [255, 255, 0, 0]),
    );
    let open_right = capture_variant(
        "open-right",
        vec![2, 3, 0, 1],
        constant_payload(&baked, [0, 0, 255, 255]),
    );
    let closed = capture_variant(
        "all-closed",
        vec![0, 1, 2, 3],
        constant_payload(&baked, [0; 4]),
    );
    assert!(
        total(&added_light(&open_left, &closed)) > 0,
        "open lights must add specular"
    );
    assert_eq!(
        open_left.as_raw(),
        open_right.as_raw(),
        "which half holds the open group must not change the frame"
    );

    // Group move: every group-0 light's baked mask moves into group 1 and the
    // group-1 masks move into group 0, with the slot table following.
    let baked_frame = capture(&workspace, scratch.path(), &compiled, "baked-placement");
    let moved_table: Vec<u8> = baked
        .channels
        .iter()
        .map(|&slot| (slot + 2) % SLOT_COUNT)
        .collect();
    let moved_masks = ShadowmaskAtlasSection {
        data: swap_slot_blocks(&baked, 0, 2),
        ..baked.clone()
    };
    let moved_masks = swap_slot_blocks(&moved_masks, 1, 3);
    assert!(
        moved_masks != baked.data,
        "the group move must change the payload, or the frame comparison proves nothing"
    );
    let moved = capture_variant("groups-moved", moved_table, moved_masks);
    assert_eq!(
        moved.as_raw(),
        baked_frame.as_raw(),
        "moving lights between groups must leave world specular unchanged"
    );
}

// Pins: animated-after-take and capture-install. The animated contribution
// atlas is sized from the lightmap header install keeps; the payloads have
// already moved into the upload. A mismatch would fall back to the dummy atlas
// with a renderer error. A capture install that left a header without its
// payload would log `[Renderer] ... header arrived without its payload`; the
// no-`[Renderer]` check below covers both.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_shadowmask_groups -- --ignored`"]
fn animated_lights_render_after_lightmap_payloads_move_into_the_upload() {
    let workspace = workspace_root();
    let scratch = tempfile::tempdir().expect("scratch dir");
    let compiled = compile_dev_map(&workspace, "spawner-test");
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
        assert_no_renderer_errors(stderr);
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

/// No lighting resource degraded at install. This includes the capture-install
/// failure, `[Renderer] ... header arrived without its payload`, which a
/// capture that borrowed or lost the moved payloads would log.
fn assert_no_renderer_errors(stderr: &str) {
    assert!(
        !stderr.contains("[Renderer]"),
        "the install must not degrade any lighting resource:\n{stderr}"
    );
}

/// A copy of the compiled fixture without id 42. Every other section keeps its
/// bytes, so the level still ships its lightmap and its selected lights.
fn write_without_shadowmask(sections: &[prl_format::SectionBlob], path: &Path) {
    let blobs: Vec<_> = sections
        .iter()
        .filter(|blob| blob.section_id != SectionId::ShadowmaskAtlas as u32)
        .map(|blob| prl_format::SectionBlob {
            section_id: blob.section_id,
            version: blob.version,
            data: blob.data.clone(),
        })
        .collect();
    let mut file = fs::File::create(path).expect("create no-shadowmask PRL");
    prl_format::write_prl(&mut file, &blobs).expect("write no-shadowmask PRL");
}

// Pins: partial-lighting-install and capture-install. A level with a lightmap
// but no shadowmask installs through capture: its lightmap payloads move into
// the upload with no mask to pair, and the specular path takes its neutral
// placeholder. A panic fails the capture; a header left without its payload
// logs a `[Renderer]` error. The level with neither is covered by the loader's
// take-seam unit tests.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_shadowmask_groups -- --ignored`"]
fn level_with_a_lightmap_but_no_shadowmask_captures_cleanly() {
    let workspace = workspace_root();
    let scratch = tempfile::tempdir().expect("scratch dir");
    let compiled = compile_dev_map(&workspace, "shadowmask-groups-capture");
    let sections = read_sections(&compiled);
    let has = |sections: &[prl_format::SectionBlob], id: SectionId| {
        sections.iter().any(|blob| blob.section_id == id as u32)
    };
    assert!(
        has(&sections, SectionId::ShadowmaskAtlas),
        "fixture must bake a ShadowmaskAtlas for the variant to drop"
    );

    // Beside the compiled fixture for the material-tree layout.
    let stripped = tempfile::Builder::new()
        .prefix(".shadowmask-groups-no-mask-")
        .suffix(".prl")
        .tempfile_in(compiled.parent().expect("fixture has a parent"))
        .expect("reserve no-shadowmask PRL path")
        .into_temp_path();
    write_without_shadowmask(&sections, &stripped);
    let stripped_sections = read_sections(&stripped);
    assert!(
        has(&stripped_sections, SectionId::Lightmap),
        "variant must keep the lightmap"
    );
    assert!(
        !has(&stripped_sections, SectionId::ShadowmaskAtlas),
        "variant must ship no ShadowmaskAtlas"
    );

    let (image, stderr) = capture_scene(
        &workspace,
        scratch.path(),
        &stripped,
        "no-shadowmask",
        north_wall_camera(),
        None,
    );
    assert_no_renderer_errors(&stderr);

    // Without id 42 every mask reads the all-visible placeholder, so the frame
    // must equal the same level with every baked mask fully open: the lightmap
    // and everything else installed intact, and nothing reads the absent atlas.
    let baked = shadowmask_of(&sections);
    let open_masks = tempfile::Builder::new()
        .prefix(".shadowmask-groups-open-masks-")
        .suffix(".prl")
        .tempfile_in(compiled.parent().expect("fixture has a parent"))
        .expect("reserve open-mask PRL path")
        .into_temp_path();
    write_variant(
        &sections,
        &baked,
        baked.channels.clone(),
        constant_payload(&baked, [255; 4]),
        &open_masks,
    );
    let (open_image, open_stderr) = capture_scene(
        &workspace,
        scratch.path(),
        &open_masks,
        "open-masks",
        north_wall_camera(),
        None,
    );
    assert_no_renderer_errors(&open_stderr);
    assert!(
        image
            .pixels()
            .any(|pixel| pixel.0[..3].iter().any(|&channel| channel > 0)),
        "the lightmap must light the frame"
    );
    assert_eq!(
        image.as_raw(),
        open_image.as_raw(),
        "a level without a shadowmask must render as if every mask were fully visible"
    );
}
