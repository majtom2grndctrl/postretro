// GPU-backed integration coverage for the shipped static frame-capture path.
// See: context/lib/rendering_pipeline.md §7.8

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use image::GenericImageView as _;

const CAPTURE_WIDTH: u32 = 64;
const CAPTURE_HEIGHT: u32 = 48;

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
