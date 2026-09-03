// GPU-backed integration coverage for the shipped static frame-capture path.
// See: context/lib/rendering_pipeline.md §7.8

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use image::{GenericImageView as _, RgbaImage};

mod capture_receiver_controls;

const CAPTURE_WIDTH: u32 = 64;
const CAPTURE_HEIGHT: u32 = 48;
const RECEIVER_CAPTURE_WIDTH: u32 = 640;
const RECEIVER_CAPTURE_HEIGHT: u32 = 480;
const RECEIVER_EYE: [f32; 3] = [6.1, 2.2, -2.5];
const RECEIVER_YAW_DEG: f32 = 77.0;
const RECEIVER_PITCH_DEG: f32 = -12.0;
const RECEIVER_FOV_DEG: f32 = 90.0;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("postretro crate must be two levels below the workspace root")
        .to_path_buf()
}

// Regression: HDR scene color must be tonemapped to RGBA8 before E20 readback.
#[test]
#[ignore = "requires a GPU adapter; run with `cargo test -p postretro --features capture --test capture_frame -- --ignored`"]
fn hdr_capture_writes_png_at_requested_dimensions() {
    let workspace = workspace_root();
    let map = workspace.join(
        "crates/level-compiler/tests/fixtures/golden/\
         test_animated_weight_maps_mixed.pre-script-light-membership.prl",
    );
    assert!(map.is_file(), "capture fixture missing: {}", map.display());

    let temp = tempfile::tempdir().expect("create isolated capture directory");
    let scene_path = temp.path().join("scene.json");
    let output_path = temp.path().join("capture.png");
    let scene = serde_json::json!({
        "map": map.display().to_string(),
        "camera": {
            "position": [4.0, 0.75, 4.0],
            "yaw_deg": 0.0,
            "pitch_deg": 0.0,
            "fov_deg": 100.0
        },
        "resolution": [CAPTURE_WIDTH, CAPTURE_HEIGHT],
        "output": output_path.display().to_string()
    });
    fs::write(
        &scene_path,
        serde_json::to_vec_pretty(&scene).expect("serialize capture scene"),
    )
    .expect("write capture scene");

    let result = Command::new(env!("CARGO_BIN_EXE_postretro"))
        .arg("--capture")
        .arg(&scene_path)
        .current_dir(&workspace)
        .output()
        .expect("launch postretro capture");
    assert!(
        result.status.success(),
        "capture failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );

    let image = image::ImageReader::open(&output_path)
        .expect("open capture PNG")
        .with_guessed_format()
        .expect("detect capture image format")
        .decode()
        .expect("decode capture PNG");
    assert_eq!(
        image.dimensions(),
        (CAPTURE_WIDTH, CAPTURE_HEIGHT),
        "capture output dimensions must match the scene request",
    );
}

// Manual A/B gate for specular-shadowmask occlusion. The source fixture exists
// only on this branch, so bake it once and hand the exact PRL plus its material
// cache to both binaries through a shared standard dev-layout artifact tree:
//
// ```sh
// BRANCH_WT=/absolute/path/to/specular-branch
// MAIN_WT=/absolute/path/to/clean-main-worktree
// BASELINE_REV=main
// HANDOFF_DIR="$(mktemp -d)"
// git -C "$BRANCH_WT" worktree add --detach "$MAIN_WT" "$BASELINE_REV"
// mkdir -p "$HANDOFF_DIR/content/dev/maps" "$HANDOFF_DIR/baked/materials"
// (
//   cd "$BRANCH_WT"
//   cargo run --quiet -p postretro-level-compiler --bin prl-build -- \
//     content/dev/maps/specular-shadowmask-capture.map \
//     -o "$HANDOFF_DIR/content/dev/maps/specular-shadowmask-capture.prl" \
//     --no-tui
// )
// cp -R "$BRANCH_WT/baked/materials/." "$HANDOFF_DIR/baked/materials/"
// cat > "$HANDOFF_DIR/scene.json" <<EOF
// {
//   "map": "$HANDOFF_DIR/content/dev/maps/specular-shadowmask-capture.prl",
//   "camera": {
//     "position": [-4.064, 4.064, -1.626],
//     "yaw_deg": 45.0,
//     "pitch_deg": 5.0,
//     "fov_deg": 80.0
//   },
//   "resolution": [64, 48],
//   "output": "$HANDOFF_DIR/capture.png"
// }
// EOF
// (cd "$MAIN_WT" && cargo build -p postretro --features capture)
// (cd "$BRANCH_WT" && cargo build -p postretro --features capture)
// "$MAIN_WT/target/debug/postretro" --capture "$HANDOFF_DIR/scene.json"
// mv "$HANDOFF_DIR/capture.png" "$HANDOFF_DIR/baseline.png"
// POSTRETRO_SPEC_SHADOWMASK_FORCE_ONE=1 \
//   "$BRANCH_WT/target/debug/postretro" --capture "$HANDOFF_DIR/scene.json"
// mv "$HANDOFF_DIR/capture.png" "$HANDOFF_DIR/forced.png"
// cmp "$HANDOFF_DIR/baseline.png" "$HANDOFF_DIR/forced.png"
// "$BRANCH_WT/target/debug/postretro" --capture "$HANDOFF_DIR/scene.json"
// mv "$HANDOFF_DIR/capture.png" "$HANDOFF_DIR/occluded.png"
// ```
//
// Run both binaries on the same GPU adapter. The PRL path deliberately remains
// under `<handoff>/content/dev/maps`: capture derives `<handoff>/baked/materials`
// from that layout, so both binaries load the copied specular material instead
// of the black placeholder. A successful `cmp` proves byte identity without
// requiring pre-change main to contain this test or source map. In
// `occluded.png`, the blocker shadow on the north-wall grazing highlight must
// visibly darken relative to `forced.png`. No golden is committed because
// adapter rounding makes rendered output unsuitable for default CI.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_frame -- --ignored`"]
fn specular_shadowmask_capture_scene_compiles_loads_and_writes_png() {
    let workspace = workspace_root();
    let source_map = workspace.join("content/dev/maps/specular-shadowmask-capture.map");
    assert!(
        source_map.is_file(),
        "capture source map missing: {}",
        source_map.display()
    );

    // Keep the compiled PRL directly under content/dev/maps. Capture derives
    // content/dev, then <workspace>/baked/materials, from this standard layout.
    // Compiling into the generic temp directory makes that derivation point at
    // the wrong tree and silently replaces the material's specular slot with
    // the black placeholder.
    let map_guard = tempfile::Builder::new()
        .prefix(".specular-shadowmask-capture-")
        .suffix(".prl")
        .tempfile_in(workspace.join("content/dev/maps"))
        .expect("reserve capture PRL path in content/dev/maps")
        .into_temp_path();
    let map = map_guard.to_path_buf();
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
        .current_dir(&workspace)
        .output()
        .expect("launch prl-build");
    assert!(
        compile.status.success(),
        "specular-shadowmask capture map compile failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr),
    );

    let loaded = postretro_level_loader::load_prl(&map.to_string_lossy())
        .expect("load compiled specular-shadowmask capture PRL");
    assert_eq!(
        loaded.lights.len(),
        2,
        "capture fixture must have two lights"
    );
    assert!(
        loaded.lights[0].is_dynamic,
        "the dynamic prefix must remain at global world.lights index zero"
    );
    assert!(
        !loaded.lights[1].is_dynamic,
        "the selected point light must follow the dynamic prefix"
    );
    assert_eq!(
        loaded.entity_shadow_lights,
        vec![1],
        "selection indexes must target the selected static light in global world.lights space"
    );
    assert!(
        loaded.shadowmask_atlas.is_some(),
        "capture map must load a ShadowmaskAtlas section"
    );

    let material_index = loaded
        .texture_names
        .iter()
        .position(|name| name == "50-free-textures/concrete_stone_021")
        .expect("capture receiver material must remain in the compiled texture table");
    let material_key = loaded.texture_cache_keys.keys[material_index];
    assert_ne!(
        material_key, [0; 32],
        "capture receiver material must resolve to a compiled .prm bundle"
    );
    let prm_path = workspace.join("baked/materials").join(format!(
        "{}.prm",
        postretro_level_format::prm::cache_filename_for_key(&material_key)
    ));
    let prm_bytes = fs::read(&prm_path)
        .unwrap_or_else(|err| panic!("read capture receiver bundle {}: {err}", prm_path.display()));
    let (header, slots) = postretro_level_format::prm::PrmFile::from_bytes_partial(&prm_bytes);
    header.expect("capture receiver .prm header must be valid");
    let specular = slots
        .into_iter()
        .nth(1)
        .expect("capture receiver .prm slot table must include specular index")
        .expect("capture receiver .prm must carry a specular slot");
    assert_eq!(
        specular.format,
        postretro_level_format::prm::PrmFormat::R8Unorm,
        "capture receiver specular slot must use the runtime R8 format"
    );
    assert!(
        specular.payload.iter().any(|&value| value != 0),
        "capture receiver specular payload must contain non-black texels"
    );

    let temp = tempfile::tempdir().expect("create isolated capture directory");
    let scene_path = temp.path().join("scene.json");
    let output_path = temp.path().join("capture.png");
    let scene = serde_json::json!({
        "map": map.display().to_string(),
        "camera": {
            // Quake (64, 160, 160) translated to engine axes, looking across
            // the north-wall receiver at a grazing angle around the blocker.
            "position": [-4.064, 4.064, -1.626],
            "yaw_deg": 45.0,
            "pitch_deg": 5.0,
            "fov_deg": 80.0
        },
        "resolution": [CAPTURE_WIDTH, CAPTURE_HEIGHT],
        "output": output_path.display().to_string()
    });
    fs::write(
        &scene_path,
        serde_json::to_vec_pretty(&scene).expect("serialize capture scene"),
    )
    .expect("write capture scene");

    let result = Command::new(env!("CARGO_BIN_EXE_postretro"))
        .arg("--capture")
        .arg(&scene_path)
        .current_dir(&workspace)
        .output()
        .expect("launch postretro capture");
    assert!(
        result.status.success(),
        "capture failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );

    let image = image::ImageReader::open(&output_path)
        .expect("open capture PNG")
        .with_guessed_format()
        .expect("detect capture image format")
        .decode()
        .expect("decode capture PNG");
    assert_eq!(image.dimensions(), (CAPTURE_WIDTH, CAPTURE_HEIGHT));
}

// Regression: a static capture used to draw only world geometry, so it could
// not prove that the alarm's animated direct term reached the map-authored
// closet mover and prop_mesh. Keep these as same-adapter comparisons instead
// of committed PNGs: adapter rounding makes cross-adapter pixels unsuitable
// for the default test matrix.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_frame -- --ignored`"]
fn spawner_capture_forced_alarm_reds_dynamic_receivers_and_keeps_baked_rest() {
    let workspace = workspace_root();
    let source_map = workspace.join("content/dev/maps/spawner-test.map");
    assert!(
        source_map.is_file(),
        "capture source map missing: {}",
        source_map.display()
    );

    // The map must live below content/dev/maps so capture derives the normal
    // content root and its baked material cache. A generic temp path would
    // silently resolve placeholder resources instead.
    let map_guard = tempfile::Builder::new()
        .prefix(".capture-animated-direct-")
        .suffix(".prl")
        .tempfile_in(workspace.join("content/dev/maps"))
        .expect("reserve spawner capture PRL path in content/dev/maps")
        .into_temp_path();
    let map = map_guard.to_path_buf();
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
        .current_dir(&workspace)
        .output()
        .expect("launch prl-build");
    assert!(
        compile.status.success(),
        "spawner capture map compile failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr),
    );

    let loaded = postretro_level_loader::load_prl(&map.to_string_lossy())
        .expect("load spawner capture fixture");
    let alarm = loaded
        .lights
        .iter()
        .find(|light| light.tags.iter().any(|tag| tag == "alarm_light"))
        .expect("fixture contains alarm light");
    let slot = alarm
        .animated_slot
        .expect("alarm reserves an animated descriptor") as usize;
    let descriptor = &loaded
        .sh_volume
        .as_ref()
        .expect("fixture contains SH volume")
        .animation_descriptors[slot];
    assert_eq!(
        descriptor.start_active, 1,
        "fixture must exercise baked start-active rest"
    );

    let temp = tempfile::tempdir().expect("create isolated capture directory");
    let rest_scene = temp.path().join("rest.json");
    let rest_output = temp.path().join("rest.png");
    write_spawner_capture_scene(&rest_scene, &map, &rest_output, None);

    // This first process has no authored state. Its descriptor is precisely
    // the baked start_active rest state installed with the level geometry.
    if !run_capture_or_skip_without_adapter(&workspace, &rest_scene) {
        return;
    }
    let rest_bytes = fs::read(&rest_output).expect("read first rest capture bytes");

    let red_scene = temp.path().join("red.json");
    let red_output = temp.path().join("red.png");
    write_spawner_capture_scene(&red_scene, &map, &red_output, Some([4.0, 0.0, 0.0]));

    // Do not reuse the rest process: capture-process isolation prevents the
    // authored red descriptor seed from leaking into the rest baseline.
    if !run_capture_or_skip_without_adapter(&workspace, &red_scene) {
        return;
    }

    // Run the exact same scene in another process after the authored-red scene.
    // This pins byte determinism and the fresh process's baked-rest baseline.
    if !run_capture_or_skip_without_adapter(&workspace, &rest_scene) {
        return;
    }
    assert_eq!(
        fs::read(&rest_output).expect("read repeated rest capture bytes"),
        rest_bytes,
        "identical rest captures on one adapter must produce identical PNG bytes",
    );

    let rest = load_capture_rgba(&rest_output);
    let red = load_capture_rgba(&red_output);
    assert_eq!(
        rest.dimensions(),
        (RECEIVER_CAPTURE_WIDTH, RECEIVER_CAPTURE_HEIGHT),
        "rest capture must use the requested dimensions",
    );
    assert_eq!(red.dimensions(), rest.dimensions());
    let dark_scene = temp.path().join("dark.json");
    let dark_output = temp.path().join("dark.png");
    write_spawner_capture_scene(&dark_scene, &map, &dark_output, Some([0.0; 3]));
    if !run_capture_or_skip_without_adapter(&workspace, &dark_scene) {
        return;
    }
    let dark = load_capture_rgba(&dark_output);
    assert_region_has_baked_rest_radiance("cone-lit wall", &rest, &dark, (432, 152, 160, 136));
    assert_ne!(
        red, rest,
        "the forced-red scene must remain distinct from the unmodified baked rest frame",
    );

    let no_direct_map = capture_receiver_controls::without_animated_direct(&map, slot as u32);
    let no_direct_scene = temp.path().join("rest-without-alarm-direct.json");
    let no_direct_output = temp.path().join("rest-without-alarm-direct.png");
    write_spawner_capture_scene(&no_direct_scene, &no_direct_map, &no_direct_output, None);
    if !run_capture_or_skip_without_adapter(&workspace, &no_direct_scene) {
        return;
    }
    let no_direct = load_capture_rgba(&no_direct_output);

    // Bake once. Each control changes only one receiver record, preserving
    // every lighting payload and the other receiver. Triangle masks bound the
    // receiver footprint; removal controls distinguish its visible surfaces
    // from world geometry that occludes those projected triangles.
    for receiver in [
        capture_receiver_controls::Receiver::Mover,
        capture_receiver_controls::Receiver::Prop,
    ] {
        let mask = capture_receiver_controls::receiver_mask(&map, &workspace, receiver);
        let absent_map = capture_receiver_controls::without_receiver(&map, receiver);
        let absent_scene = temp
            .path()
            .join(format!("absent-{}.json", receiver.label()));
        let absent_output = temp.path().join(format!("absent-{}.png", receiver.label()));
        write_spawner_capture_scene(
            &absent_scene,
            &absent_map,
            &absent_output,
            Some([4.0, 0.0, 0.0]),
        );
        if !run_capture_or_skip_without_adapter(&workspace, &absent_scene) {
            return;
        }
        let proven_receiver_pixels = assert_receiver_reddens(
            receiver.label(),
            &rest,
            &red,
            &load_capture_rgba(&absent_output),
            &mask,
        );
        assert_receiver_has_baked_rest_radiance(
            receiver.label(),
            &rest,
            &dark,
            &no_direct,
            &proven_receiver_pixels,
        );
    }
    assert_region_reddens("cone-lit wall", &rest, &red, (432, 152, 160, 136));
}

fn write_spawner_capture_scene(
    scene_path: &std::path::Path,
    map: &std::path::Path,
    output: &std::path::Path,
    alarm_radiance: Option<[f32; 3]>,
) {
    let mut scene = serde_json::json!({
        "map": map.display().to_string(),
        "camera": {
            // Quake (roughly 104, -240, 87) translated to engine axes. This
            // frames the closet-door mover, prop_mesh, and the static wall in
            // alarm_light's authored cone at their no-tick rest poses.
            "position": RECEIVER_EYE,
            "yaw_deg": RECEIVER_YAW_DEG,
            "pitch_deg": RECEIVER_PITCH_DEG,
            "fov_deg": RECEIVER_FOV_DEG
        },
        "resolution": [RECEIVER_CAPTURE_WIDTH, RECEIVER_CAPTURE_HEIGHT],
        "output": output.display().to_string()
    });
    if let Some(radiance) = alarm_radiance {
        scene["force_active"] = serde_json::json!([
            { "tag": "alarm_light", "radiance": radiance }
        ]);
    }
    fs::write(
        scene_path,
        serde_json::to_vec_pretty(&scene).expect("serialize spawner capture scene"),
    )
    .expect("write spawner capture scene");
}

fn run_capture_or_skip_without_adapter(
    workspace: &std::path::Path,
    scene_path: &std::path::Path,
) -> bool {
    let result = Command::new(env!("CARGO_BIN_EXE_postretro"))
        .arg("--capture")
        .arg(scene_path)
        .current_dir(workspace)
        .output()
        .expect("launch postretro capture");
    if result.status.success() {
        return true;
    }

    let output = format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
    if output.contains("frame capture requires a GPU adapter") {
        eprintln!("skipping capture golden: no GPU adapter available");
        return false;
    }

    panic!("capture failed\n{output}");
}

fn load_capture_rgba(path: &std::path::Path) -> RgbaImage {
    image::ImageReader::open(path)
        .unwrap_or_else(|err| panic!("open capture PNG {}: {err}", path.display()))
        .with_guessed_format()
        .expect("detect capture image format")
        .decode()
        .expect("decode capture PNG")
        .to_rgba8()
}

fn assert_region_has_baked_rest_radiance(
    label: &str,
    rest: &RgbaImage,
    dark: &RgbaImage,
    (left, top, width, height): (u32, u32, u32, u32),
) {
    // Only alarm radiance differs. Other baked lighting and ambient floor
    // cannot satisfy this comparison if the rest descriptor is dropped.
    const MIN_LIT_PIXELS: usize = 64;

    let lit_pixels = (top..top + height)
        .flat_map(|y| (left..left + width).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let rest = rest.get_pixel(x, y).0;
            let dark = dark.get_pixel(x, y).0;
            (0..3)
                .map(|c| i32::from(rest[c]) - i32::from(dark[c]))
                .sum::<i32>()
                > 12
        })
        .count();
    assert!(
        lit_pixels >= MIN_LIT_PIXELS,
        "{label} must retain visible baked rest radiance without force_active; \
         found {lit_pixels} pixels brighter than the zero-radiance alarm control",
    );
}

fn assert_receiver_reddens(
    label: &str,
    rest: &RgbaImage,
    red: &RgbaImage,
    absent: &RgbaImage,
    mask: &[(u32, u32)],
) -> Vec<(u32, u32)> {
    // The projected triangle union is only a search region: it has no depth
    // test and includes surfaces hidden behind world geometry or outside the
    // alarm cone. Its area cannot be a denominator for visible, lit coverage.
    // Keep an absolute floor of independently proven receiver pixels; both
    // removal and light-state comparisons must pass at every counted pixel.
    const MIN_REDDENED_RECEIVER_PIXELS: usize = 64;

    let reddened_receiver_pixels: Vec<_> = mask
        .iter()
        .filter(|&&(x, y)| {
            let rest = rest.get_pixel(x, y).0;
            let red = red.get_pixel(x, y).0;
            let absent = absent.get_pixel(x, y).0;
            let receiver_difference: i32 = (0..3)
                .map(|c| (i32::from(red[c]) - i32::from(absent[c])).abs())
                .sum();
            let chroma_gain =
                (i32::from(red[0]) - i32::from(red[1])) - (i32::from(rest[0]) - i32::from(rest[1]));
            // A missing color draw that still casts a shadow can only darken
            // the background relative to removal. Require added red light on
            // the receiver footprint, not merely a shadow color difference.
            receiver_difference > 12
                && i32::from(red[0]) - i32::from(absent[0]) > 4
                && chroma_gain > 8
        })
        .copied()
        .collect();
    assert!(
        reddened_receiver_pixels.len() >= MIN_REDDENED_RECEIVER_PIXELS,
        "{label} must be present and redden on its projected triangles: \
         {} proven receiver pixels in a {}-pixel search region, \
         need {MIN_REDDENED_RECEIVER_PIXELS}",
        reddened_receiver_pixels.len(),
        mask.len(),
    );
    reddened_receiver_pixels
}

fn assert_receiver_has_baked_rest_radiance(
    label: &str,
    rest: &RgbaImage,
    dark: &RgbaImage,
    no_direct: &RgbaImage,
    proven_receiver_pixels: &[(u32, u32)],
) {
    // The red/removal comparison proves these pixels belong to the receiver.
    // Requiring both deltas excludes unrelated world light and alarm indirect
    // SH: only the rest descriptor's direct-SH contribution can satisfy both.
    const MIN_LIT_RECEIVER_PIXELS: usize = 64;
    let lit_pixels = proven_receiver_pixels
        .iter()
        .filter(|&&(x, y)| {
            let rest = rest.get_pixel(x, y).0;
            [dark, no_direct].iter().all(|control| {
                let control = control.get_pixel(x, y).0;
                (0..3)
                    .map(|channel| i32::from(rest[channel]) - i32::from(control[channel]))
                    .sum::<i32>()
                    > 12
            })
        })
        .count();
    assert!(
        lit_pixels >= MIN_LIT_RECEIVER_PIXELS,
        "{label} must retain baked rest direct-SH radiance without force_active: \
         {lit_pixels} proven receiver pixels exceed both zero-alarm and zero-direct controls, \
         need {MIN_LIT_RECEIVER_PIXELS}",
    );
}

// Regression: the prop's projected silhouette includes occluded and unlit
// surfaces, so demanding red pixels across 10% of that union rejected its draw.
#[test]
fn receiver_predicate_accepts_visible_lit_patch_with_occluded_projected_triangles() {
    let rest = RgbaImage::from_pixel(64, 64, image::Rgba([40, 40, 40, 255]));
    let mut red = RgbaImage::from_pixel(64, 64, image::Rgba([80, 30, 30, 255]));
    let absent = red.clone();
    let mask: Vec<_> = (0..64).flat_map(|y| (0..64).map(move |x| (x, y))).collect();

    // Most projected pixels show an occluding wall that reddens too. Only this
    // visible patch is brighter than the same-lit wall in the removal control.
    for y in 24..32 {
        for x in 24..32 {
            red.put_pixel(x, y, image::Rgba([100, 30, 30, 255]));
        }
    }
    assert_receiver_reddens("partly occluded receiver", &rest, &red, &absent, &mask);
    red.put_pixel(24, 24, *absent.get_pixel(24, 24));
    assert!(
        std::panic::catch_unwind(|| {
            assert_receiver_reddens(
                "insufficient receiver coverage",
                &rest,
                &red,
                &absent,
                &mask,
            );
        })
        .is_err()
    );
    assert!(
        std::panic::catch_unwind(|| {
            assert_receiver_reddens("occluding wall only", &rest, &absent, &absent, &mask);
        })
        .is_err()
    );
}

#[test]
fn receiver_predicate_rejects_shadow_only_and_unchanged_receiver_color() {
    let rest = RgbaImage::from_pixel(16, 16, image::Rgba([40, 40, 40, 255]));
    let red = RgbaImage::from_pixel(16, 16, image::Rgba([80, 30, 30, 255]));
    let absent = RgbaImage::from_pixel(16, 16, image::Rgba([100, 60, 60, 255]));
    let mask: Vec<_> = (0..16).flat_map(|y| (0..16).map(move |x| (x, y))).collect();

    // A surviving shadow draw can change the background and its red chroma,
    // but cannot supply the added red channel required for receiver presence.
    assert!(
        std::panic::catch_unwind(|| {
            assert_receiver_reddens("shadow only", &rest, &red, &absent, &mask);
        })
        .is_err()
    );
    assert!(
        std::panic::catch_unwind(|| {
            assert_receiver_reddens("unchanged receiver", &red, &red, &rest, &mask);
        })
        .is_err()
    );
}

// Regression: broad crops accepted red walls when the receiver draw vanished.
#[test]
fn receiver_predicate_rejects_background_only_and_requires_substantial_coverage() {
    let rest = RgbaImage::from_pixel(16, 16, image::Rgba([40, 40, 40, 255]));
    let red = RgbaImage::from_pixel(16, 16, image::Rgba([80, 30, 30, 255]));
    let mask: Vec<_> = (0..16).flat_map(|y| (0..16).map(move |x| (x, y))).collect();
    assert!(
        std::panic::catch_unwind(|| assert_receiver_reddens("missing", &rest, &red, &red, &mask))
            .is_err()
    );
    let mut only_one_receiver_pixel = red.clone();
    only_one_receiver_pixel.put_pixel(0, 0, image::Rgba([10, 10, 10, 255]));
    assert!(
        std::panic::catch_unwind(|| assert_receiver_reddens(
            "one pixel",
            &rest,
            &red,
            &only_one_receiver_pixel,
            &mask
        ))
        .is_err()
    );
    assert_receiver_reddens("visible receiver", &rest, &red, &rest, &mask);
}

#[test]
fn receiver_rest_predicate_rejects_forward_or_indirect_light_without_rest_direct_sh() {
    // Regression: a bright static wall passed AC3 even when the receivers'
    // separate direct-SH paths lost the baked start-active descriptor.
    let mut rest = RgbaImage::from_pixel(16, 16, image::Rgba([200, 200, 200, 255]));
    let dark = RgbaImage::from_pixel(16, 16, image::Rgba([40, 40, 40, 255]));
    let absent = RgbaImage::from_pixel(16, 16, image::Rgba([80, 40, 40, 255]));
    let mut red = absent.clone();
    for y in 0..8 {
        for x in 0..8 {
            rest.put_pixel(x, y, image::Rgba([60, 60, 60, 255]));
            red.put_pixel(x, y, image::Rgba([120, 40, 40, 255]));
        }
    }
    let mask: Vec<_> = (0..16).flat_map(|y| (0..16).map(move |x| (x, y))).collect();
    let proven = assert_receiver_reddens("receiver patch", &rest, &red, &absent, &mask);
    assert_eq!(proven.len(), 64);

    // World/forward and indirect light remain bright, and red mode works,
    // but the direct-only control is identical at rest: this must fail.
    let mut no_direct = rest.clone();
    assert!(
        std::panic::catch_unwind(|| {
            assert_receiver_has_baked_rest_radiance(
                "rest direct missing",
                &rest,
                &dark,
                &no_direct,
                &proven,
            );
        })
        .is_err()
    );
    for &(x, y) in &proven {
        no_direct.put_pixel(x, y, *dark.get_pixel(x, y));
    }
    assert_receiver_has_baked_rest_radiance(
        "rest direct present",
        &rest,
        &dark,
        &no_direct,
        &proven,
    );
    assert!(
        std::panic::catch_unwind(|| {
            assert_receiver_has_baked_rest_radiance(
                "rest descriptor missing",
                &rest,
                &rest,
                &no_direct,
                &proven,
            );
        })
        .is_err()
    );
    no_direct.put_pixel(0, 0, *rest.get_pixel(0, 0));
    assert!(
        std::panic::catch_unwind(|| {
            assert_receiver_has_baked_rest_radiance(
                "insufficient rest coverage",
                &rest,
                &dark,
                &no_direct,
                &proven,
            );
        })
        .is_err()
    );
}

#[test]
fn rest_predicate_rejects_brightness_unrelated_to_alarm_descriptor() {
    let unrelated_light = RgbaImage::from_pixel(16, 16, image::Rgba([200, 200, 200, 255]));
    assert!(
        std::panic::catch_unwind(|| assert_region_has_baked_rest_radiance(
            "unrelated light",
            &unrelated_light,
            &unrelated_light,
            (0, 0, 16, 16)
        ))
        .is_err()
    );
}

fn assert_region_reddens(
    label: &str,
    rest: &RgbaImage,
    red: &RgbaImage,
    (left, top, width, height): (u32, u32, u32, u32),
) {
    let mut red_chroma_delta = 0_i64;
    for y in top..top + height {
        for x in left..left + width {
            let rest_pixel = rest.get_pixel(x, y).0;
            let red_pixel = red.get_pixel(x, y).0;
            let rest_chroma = i64::from(rest_pixel[0]) - i64::from(rest_pixel[1]);
            let red_chroma = i64::from(red_pixel[0]) - i64::from(red_pixel[1]);
            red_chroma_delta += red_chroma - rest_chroma;
        }
    }

    assert!(
        red_chroma_delta > 256,
        "{label} must gain visible red chroma under the authored alarm descriptor; \
         same-adapter crop delta was {red_chroma_delta}",
    );
}
