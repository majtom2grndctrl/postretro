// GPU-backed proofs over the compiled shadowmask fixture: lightmap payloads
// install through capture with and without id 42.
// See: context/lib/rendering_pipeline.md §4 (Static specular ownership)
//
// Variants rewrite every cell block's id-42 group planes in the compiled PRL
// with constant per-channel planes (exact under BC5), keeping its owner
// table, so the only thing that differs between captures is what the masks
// hold. Captures are compared on one adapter; no golden images.

use std::fs;
use std::path::Path;

use image::RgbaImage;
use postretro_level_format as prl_format;
use postretro_level_format::SectionId;
use postretro_level_format::alpha_lights::AlphaLightsSection;
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::lightmap::LightmapBlockIndex;
use postretro_level_format::shadowmask_atlas::{
    FaceOwners, SHADOWMASK_GROUP_COUNT, ShadowmaskAtlasSection, ShadowmaskOwnerBounds,
};

mod capture_support;

use capture_support::{capture_command, compile_dev_map, load_capture_rgba, workspace_root};

const CAPTURE_WIDTH: u32 = 160;
const CAPTURE_HEIGHT: u32 = 120;

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

/// Id 42 parsed against the id-22 block index it pairs with and the owner
/// bounds ids 17 and 18 set.
fn shadowmask_of(sections: &[prl_format::SectionBlob]) -> ShadowmaskAtlasSection {
    let section = |id: SectionId| {
        sections
            .iter()
            .find(|blob| blob.section_id == id as u32)
            .unwrap_or_else(|| panic!("fixture must ship a {id:?} section"))
    };
    let lightmap = section(SectionId::Lightmap);
    let index = LightmapBlockIndex::from_prefix(&lightmap.data, lightmap.data.len() as u64)
        .expect("fixture Lightmap index must parse");
    let faces = GeometrySection::from_bytes(&section(SectionId::Geometry).data)
        .expect("fixture Geometry must parse")
        .faces
        .len();
    let static_lights = AlphaLightsSection::from_bytes(&section(SectionId::AlphaLights).data)
        .expect("fixture AlphaLights must parse")
        .lights
        .iter()
        .filter(|light| !light.is_dynamic)
        .count();
    let bounds = ShadowmaskOwnerBounds {
        face_count: faces as u32,
        static_spec_light_count: static_lights as u32,
    };
    ShadowmaskAtlasSection::from_bytes(&section(SectionId::ShadowmaskAtlas).data, &index, bounds)
        .expect("fixture ShadowmaskAtlas must parse")
}

/// Per cell block, the group A then group B BC5 planes.
type GroupPlanes = Vec<[Vec<u8>; 2]>;

/// Group planes whose every texel holds `channels[c]` in channel `c`, block
/// for block at the baked lengths. A constant BC4 block is
/// `[v, v, 0, 0, 0, 0, 0, 0]`, which decodes to exactly `v`.
fn constant_payload(section: &ShadowmaskAtlasSection, channels: [u8; 4]) -> GroupPlanes {
    let bc4 = |value: u8| [value, value, 0, 0, 0, 0, 0, 0];
    section
        .blocks
        .iter()
        .map(|groups| {
            std::array::from_fn(|group| {
                let bc5: Vec<u8> = bc4(channels[group * 2])
                    .into_iter()
                    .chain(bc4(channels[group * 2 + 1]))
                    .collect();
                let plane = bc5.repeat(groups[group].len() / 16);
                assert_eq!(plane.len(), groups[group].len());
                plane
            })
        })
        .collect()
}

/// Write a copy of the fixture whose id 42 carries `owners` and `blocks`.
/// Both keep their original lengths, so no other section moves.
fn write_variant(
    sections: &[prl_format::SectionBlob],
    owners: Vec<FaceOwners>,
    blocks: GroupPlanes,
    path: &Path,
) {
    const {
        assert!(
            SHADOWMASK_GROUP_COUNT == 2,
            "a block carries group A and group B"
        );
    }
    let variant = ShadowmaskAtlasSection { owners, blocks };
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

/// Run one capture and return its image and stderr. Fails rather than skips
/// without an adapter: this proof does not count unless it ran.
/// `force_active` is the scene's optional forced-animation list.
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

// Pins: animated-after-take and capture-install. The animated contribution
// atlas keys on the cell blocks the pool plan installs from the block index
// and the payloads install moved into the upload. A mismatch would fall back
// to the dummy atlas with a renderer error. A capture install that left the
// index without its payloads would log `[Renderer] Lightmap cell blocks
// rejected`; the no-`[Renderer]` check below covers both.
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
/// failure, `[Renderer] Lightmap cell blocks rejected: ...`, which a capture
/// that borrowed or lost the moved payloads would log.
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
// placeholder. A panic fails the capture; an index left without its payloads
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

    // Without id 42 no owner has a mask to read, so the frame must equal the
    // same level with every baked mask fully closed: the lightmap and
    // everything else installed intact, and no static non-SDF specular.
    let baked = shadowmask_of(&sections);
    let closed_masks = tempfile::Builder::new()
        .prefix(".shadowmask-groups-closed-masks-")
        .suffix(".prl")
        .tempfile_in(compiled.parent().expect("fixture has a parent"))
        .expect("reserve closed-mask PRL path")
        .into_temp_path();
    write_variant(
        &sections,
        baked.owners.clone(),
        constant_payload(&baked, [0; 4]),
        &closed_masks,
    );
    let (closed_image, closed_stderr) = capture_scene(
        &workspace,
        scratch.path(),
        &closed_masks,
        "closed-masks",
        north_wall_camera(),
        None,
    );
    assert_no_renderer_errors(&closed_stderr);
    assert!(
        image
            .pixels()
            .any(|pixel| pixel.0[..3].iter().any(|&channel| channel > 0)),
        "the lightmap must light the frame"
    );
    assert_eq!(
        image.as_raw(),
        closed_image.as_raw(),
        "a level without a shadowmask must render as if every mask were fully occluded"
    );
}

// End to end, each owner position reads its own channel in either mask
// group. The fixture's lights overlap on the north wall, so faces there carry
// owners in both groups. Opening one group alone brightens the frame, the two
// groups brighten it differently, and opening both is at least as bright as
// either, texel for texel. Swapping owners with their planes is proven by
// `world_specular_test`; an owner table out of ascending order fails the load.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_shadowmask_groups -- --ignored`"]
fn owners_in_either_mask_group_reach_the_frame_through_their_own_channels() {
    let workspace = workspace_root();
    let scratch = tempfile::tempdir().expect("scratch dir");
    let compiled = compile_dev_map(&workspace, "shadowmask-groups-capture");
    let sections = read_sections(&compiled);
    let baked = shadowmask_of(&sections);
    let owned_positions = baked
        .owners
        .iter()
        .map(|owners| owners.iter().filter(|&&owner| owner != u16::MAX).count())
        .max()
        .unwrap_or(0);
    assert!(
        owned_positions >= 3,
        "the fixture must put owners in group B; most owners on one face: {owned_positions}"
    );

    let mut images = Vec::new();
    for (label, channels) in [
        ("closed", [0u8, 0, 0, 0]),
        ("group-a", [255, 255, 0, 0]),
        ("group-b", [0, 0, 255, 255]),
        ("open", [255, 255, 255, 255]),
    ] {
        let path = tempfile::Builder::new()
            .prefix(&format!(".shadowmask-groups-{label}-"))
            .suffix(".prl")
            .tempfile_in(compiled.parent().expect("fixture has a parent"))
            .expect("reserve variant PRL path")
            .into_temp_path();
        write_variant(
            &sections,
            baked.owners.clone(),
            constant_payload(&baked, channels),
            &path,
        );
        let (image, stderr) = capture_scene(
            &workspace,
            scratch.path(),
            &path,
            label,
            north_wall_camera(),
            None,
        );
        assert_no_renderer_errors(&stderr);
        images.push(image);
    }
    let [closed, group_a, group_b, open] = [&images[0], &images[1], &images[2], &images[3]];
    assert_ne!(
        closed.as_raw(),
        group_a.as_raw(),
        "group A owners must reach the frame"
    );
    assert_ne!(
        closed.as_raw(),
        group_b.as_raw(),
        "group B owners must reach the frame"
    );
    assert_ne!(
        group_a.as_raw(),
        group_b.as_raw(),
        "the groups hold different owners"
    );
    for ((open, a), b) in open.pixels().zip(group_a.pixels()).zip(group_b.pixels()) {
        for c in 0..3 {
            assert!(
                open.0[c].saturating_add(1) >= a.0[c].max(b.0[c]),
                "opening every channel cannot darken a texel: {open:?} vs {a:?} / {b:?}"
            );
        }
    }
}

/// The specular term alone (`LightTermMask::SPECULAR`) of the
/// specular-shadowmask fixture, looking across the north-wall receiver at a
/// grazing angle around the blocker.
fn capture_specular_only(workspace: &Path, scratch: &Path, map: &Path, label: &str) -> RgbaImage {
    let scene_path = scratch.join(format!("{label}.scene.json"));
    let output_path = scratch.join(format!("{label}.png"));
    let scene = serde_json::json!({
        "map": map.display().to_string(),
        "camera": {
            "position": [-4.064, 4.064, -1.626],
            "yaw_deg": 45.0,
            "pitch_deg": 5.0,
            "fov_deg": 80.0
        },
        "resolution": [CAPTURE_WIDTH, CAPTURE_HEIGHT],
        "light_term_mask": 0x40,
        "output": output_path.display().to_string()
    });
    fs::write(&scene_path, serde_json::to_vec_pretty(&scene).unwrap()).unwrap();
    let result = capture_command(workspace, &scene_path)
        .env("RUST_LOG", "warn")
        .output()
        .expect("launch postretro capture");
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(
        result.status.success(),
        "capture `{label}` failed\nstderr:\n{stderr}"
    );
    assert_no_renderer_errors(&stderr);
    load_capture_rgba(&output_path)
}

fn lit_pixels(image: &RgbaImage) -> usize {
    image
        .pixels()
        .filter(|pixel| pixel.0[..3].iter().any(|&c| c > 0))
        .count()
}

// End to end, a static light adds world specular only where it owns the
// face, through its baked mask. With every owner entry emptied (the light
// walled off from every face) or every mask closed, the specular term is
// black, and the baked mask darkens part of the fully open highlight (the
// blocker's shadow). Keeping id 42 when id 40 is cleared at load is proven
// on the loader: a hand-stripped id 40 here would orphan the cluster
// directory's id-41 resource.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_shadowmask_groups -- --ignored`"]
fn static_specular_comes_only_from_face_owners_through_their_baked_masks() {
    let workspace = workspace_root();
    let scratch = tempfile::tempdir().expect("scratch dir");
    let compiled = compile_dev_map(&workspace, "specular-shadowmask-capture");
    let sections = read_sections(&compiled);
    let baked = shadowmask_of(&sections);
    assert!(
        baked.owners.iter().any(|owners| owners[0] != u16::MAX),
        "the fixture's static light must own a face"
    );
    let variant_path = |label: &str| {
        tempfile::Builder::new()
            .prefix(&format!(".specular-shadowmask-{label}-"))
            .suffix(".prl")
            .tempfile_in(compiled.parent().expect("fixture has a parent"))
            .expect("reserve variant PRL path")
            .into_temp_path()
    };

    let real = capture_specular_only(&workspace, scratch.path(), &compiled, "baked");
    assert!(
        lit_pixels(&real) > 0,
        "the owner's highlight must reach the frame"
    );

    let unowned = variant_path("unowned");
    write_variant(
        &sections,
        vec![[u16::MAX; 4]; baked.owners.len()],
        baked.blocks.clone(),
        &unowned,
    );
    let unowned_image = capture_specular_only(&workspace, scratch.path(), &unowned, "unowned");
    assert_eq!(
        lit_pixels(&unowned_image),
        0,
        "a light owning no face adds no specular"
    );

    let closed = variant_path("closed");
    write_variant(
        &sections,
        baked.owners.clone(),
        constant_payload(&baked, [0; 4]),
        &closed,
    );
    let closed_image = capture_specular_only(&workspace, scratch.path(), &closed, "closed");
    assert_eq!(
        lit_pixels(&closed_image),
        0,
        "an occluded owner adds no specular"
    );

    let open = variant_path("open");
    write_variant(
        &sections,
        baked.owners.clone(),
        constant_payload(&baked, [255; 4]),
        &open,
    );
    let open_image = capture_specular_only(&workspace, scratch.path(), &open, "open");
    let mut darkened = 0;
    for (open, real) in open_image.pixels().zip(real.pixels()) {
        for c in 0..3 {
            assert!(
                open.0[c].saturating_add(1) >= real.0[c],
                "a fully open mask cannot be darker than the baked one"
            );
        }
        if open.0[..3]
            .iter()
            .zip(&real.0[..3])
            .any(|(o, r)| *o > r.saturating_add(4))
        {
            darkened += 1;
        }
    }
    assert!(
        darkened > 0,
        "the baked mask must shadow part of the highlight"
    );
}
