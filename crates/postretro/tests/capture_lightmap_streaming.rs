// GPU-backed proofs for streamed lightmap cell blocks in headless capture:
// parity with all-resident mode (AC 4) and a forced miss's terms (AC 7).
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency), §7.8
//
// Captures compare on one adapter; no golden images.

use std::fs;
use std::path::{Path, PathBuf};

use image::RgbaImage;
use postretro_level_loader::ShadowType;
use postretro_render_cpu::frame_uniforms::LightTermMask;

mod capture_support;

use capture_support::{capture_command, compile_dev_map, load_capture_rgba, workspace_root};

const PARITY_RESOLUTION: [u32; 2] = [320, 240];
const MISS_RESOLUTION: [u32; 2] = [160, 120];
/// A cap past every evidence map's all-resident layer count (the hallway
/// needs 101): the first pool generation then holds every block.
const CAP_HOLDING_EVERY_BLOCK: u32 = 256;

#[derive(Clone, Copy)]
struct Pose {
    position: [f32; 3],
    yaw_deg: f32,
    pitch_deg: f32,
    fov_deg: f32,
}

struct Capture {
    png: Vec<u8>,
    image: RgbaImage,
    /// The report's `lightmap_streaming` block.
    lightmap: serde_json::Value,
}

/// How one capture runs: its resolution, the lightmap mode, and whether
/// bloom is on.
#[derive(Clone, Copy)]
struct Run<'a> {
    resolution: [u32; 2],
    lightmap_mode: &'a str,
    bloom: bool,
}

/// One measured capture of `map` from `pose` under `run`, with `overrides`
/// merged into the scene. `None` when the machine has no GPU adapter.
fn capture(
    workspace: &Path,
    dir: &Path,
    name: &str,
    map: &Path,
    pose: Pose,
    run: Run<'_>,
    overrides: serde_json::Value,
) -> Option<Capture> {
    let output = dir.join(format!("{name}.png"));
    let report = dir.join(format!("{name}.report.json"));
    let mut scene = serde_json::json!({
        "map": map.display().to_string(),
        "camera": {
            "position": pose.position,
            "yaw_deg": pose.yaw_deg,
            "pitch_deg": pose.pitch_deg,
            "fov_deg": pose.fov_deg,
        },
        "resolution": run.resolution,
        "output": output.display().to_string(),
        // The report names the residency the capture ran with; one warmup and
        // one sample keep the frame at a fixed stepped instant.
        "measurement": {
            "report": report.display().to_string(),
            "warmup_frames": 1,
            "sample_frames": 1,
        },
    });
    if let serde_json::Value::Object(overrides) = overrides {
        for (key, value) in overrides {
            scene[key] = value;
        }
    }
    let scene_path = dir.join(format!("{name}.scene.json"));
    fs::write(
        &scene_path,
        serde_json::to_vec_pretty(&scene).expect("serialize capture scene"),
    )
    .expect("write capture scene");

    let mut command = capture_command(workspace, &scene_path);
    command.env("POSTRETRO_LIGHTMAP_STREAMING", run.lightmap_mode);
    if !run.bloom {
        command.env("POSTRETRO_BLOOM", "0");
    }
    let result = command.output().expect("launch postretro capture");
    if !result.status.success() {
        let output = format!(
            "stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr),
        );
        if output.contains("frame capture requires a GPU adapter") {
            eprintln!("skipping lightmap capture: no GPU adapter available");
            return None;
        }
        panic!("capture `{name}` failed\n{output}");
    }
    let report_bytes = fs::read(&report).expect("read capture report");
    let report: serde_json::Value =
        serde_json::from_slice(&report_bytes).expect("parse capture report");
    Some(Capture {
        png: fs::read(&output).expect("read capture PNG"),
        image: load_capture_rgba(&output),
        lightmap: report["lightmap_streaming"].clone(),
    })
}

/// The baked evidence map, or `None` with a note when this checkout lacks it.
fn evidence_map(workspace: &Path, name: &str) -> Option<PathBuf> {
    let map = workspace.join(format!("content/dev/maps/{name}.prl"));
    if map.is_file() {
        return Some(map);
    }
    eprintln!(
        "skipping lightmap parity on `{name}`: {} is missing; bake it with \
         `cargo run -p postretro-level-compiler -- content/dev/maps/{name}.map -o {}`",
        map.display(),
        map.display(),
    );
    None
}

/// AC 4 on one map: streaming renders byte-identical PNGs to all-resident
/// mode from every pose, with the default pool cap (a partial pool, placed in
/// drain order) and with a cap whose first generation holds every block.
fn assert_streamed_capture_matches_all_resident(map_name: &str, poses: &[Pose]) {
    let workspace = workspace_root();
    let Some(map) = evidence_map(&workspace, map_name) else {
        return;
    };
    let temp = tempfile::tempdir().expect("create isolated capture directory");
    for (index, &pose) in poses.iter().enumerate() {
        let run = |label: &str, mode: &str, overrides: serde_json::Value| {
            capture(
                &workspace,
                temp.path(),
                &format!("{map_name}-{index}-{label}"),
                &map,
                pose,
                Run {
                    resolution: PARITY_RESOLUTION,
                    lightmap_mode: mode,
                    bloom: true,
                },
                overrides,
            )
        };
        let Some(all_resident) = run("all-resident", "all-resident", serde_json::json!({})) else {
            return;
        };
        let Some(streamed) = run("stream", "stream", serde_json::json!({})) else {
            return;
        };
        let Some(streamed_uncapped) = run(
            "stream-every-block",
            "stream",
            serde_json::json!({ "lightmap_pool_cap_layers": CAP_HOLDING_EVERY_BLOCK }),
        ) else {
            return;
        };

        assert_eq!(all_resident.lightmap["mode"], "all-resident");
        for (label, capture) in [
            ("default cap", &streamed),
            ("every block", &streamed_uncapped),
        ] {
            assert_eq!(
                capture.lightmap["mode"], "stream",
                "{map_name} pose {index} ({label}) must stream its lightmap: {}",
                capture.lightmap
            );
            eprintln!(
                "{map_name} pose {index} ({label}): {} of {} block(s) resident, {} pool layer(s)",
                capture.lightmap["resident_blocks"],
                capture.lightmap["block_count"],
                capture.lightmap["pool_layers"],
            );
        }
        assert!(
            lit_pixels(&all_resident.image) > 0,
            "{map_name} pose {index} must see lit geometry"
        );
        assert!(
            streamed.png == all_resident.png,
            "{map_name} pose {index}: streamed capture (default cap) differs from all-resident \
             in {} pixel(s)",
            differing_pixels(&streamed.image, &all_resident.image),
        );
        assert!(
            streamed_uncapped.png == all_resident.png,
            "{map_name} pose {index}: streamed capture (every block fits) differs from \
             all-resident in {} pixel(s)",
            differing_pixels(&streamed_uncapped.image, &all_resident.image),
        );
    }
}

#[test]
#[ignore = "requires a GPU adapter and the baked campaign-test PRL; run with `cargo test -p postretro --features capture --test capture_lightmap_streaming -- --ignored`"]
fn streamed_lightmap_capture_matches_all_resident_on_campaign_test() {
    // The spawn eye (yaw 270 is lit), and the reverse view.
    let spawn = Pose {
        position: [-65.84, 2.6, -45.92],
        yaw_deg: 270.0,
        pitch_deg: 0.0,
        fov_deg: 90.0,
    };
    assert_streamed_capture_matches_all_resident(
        "campaign-test",
        &[
            spawn,
            Pose {
                yaw_deg: 90.0,
                pitch_deg: -10.0,
                ..spawn
            },
        ],
    );
}

#[test]
#[ignore = "requires a GPU adapter and the baked stress-warren-hallway-inspection PRL; run with `cargo test -p postretro --features capture --test capture_lightmap_streaming -- --ignored`"]
fn streamed_lightmap_capture_matches_all_resident_on_hallway_inspection() {
    let spawn = Pose {
        position: [42.27, 3.2, 63.40],
        yaw_deg: 0.0,
        pitch_deg: 0.0,
        fov_deg: 90.0,
    };
    assert_streamed_capture_matches_all_resident(
        "stress-warren-hallway-inspection",
        &[
            spawn,
            Pose {
                yaw_deg: 180.0,
                pitch_deg: -10.0,
                ..spawn
            },
        ],
    );
}

/// AC 7 on a fixture with static lights and no SDF-shadowed ones, bloom off.
/// For each
/// block `b`, its region `R_b` is the pixels a forced miss of `b` changes
/// from the all-resident capture. The proof:
///
/// - the chosen block (the one covering most of the view) renders lit when
///   resident, so its region is not empty;
/// - on every region, the forced-miss capture equals the capture with the
///   static-direct and static-specular terms masked off, byte for byte;
/// - the regions are disjoint and together are exactly the pixels the mask
///   changes, so no block's miss left any of its own static light behind;
/// - on the chosen region, SH indirect is still present: masking it off too
///   changes pixels there.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_lightmap_streaming -- --ignored`"]
fn forced_missing_lightmap_block_drops_exactly_its_static_direct_and_specular() {
    let workspace = workspace_root();
    // Beside its source, so capture resolves the receiver's specular
    // material (see `compile_dev_map`).
    let map_guard = compile_dev_map(&workspace, "specular-shadowmask-capture");
    let map = map_guard.to_path_buf();
    let world = postretro_level_loader::load_prl(&map.to_string_lossy())
        .expect("load compiled fixture PRL");
    assert!(
        world
            .lights
            .iter()
            .all(|light| light.shadow_type != ShadowType::Sdf),
        "the fixture must have no SDF-shadowed static lights"
    );
    assert!(
        world
            .lights
            .iter()
            .any(|light| !light.is_dynamic && light.shadow_type == ShadowType::StaticLightMap),
        "the fixture must bake a static light into its lightmap"
    );
    assert!(
        world.cell_residency_set.is_some(),
        "the fixture must carry id 51 so its lightmap streams"
    );
    let block_count = world
        .lightmap
        .as_ref()
        .expect("fixture has cell blocks")
        .records
        .len() as u32;
    assert!(block_count > 1, "the fixture needs more than one block");

    // The specular receiver's grazing view around the blocker.
    let pose = Pose {
        position: [-4.064, 4.064, -1.626],
        yaw_deg: 45.0,
        pitch_deg: 5.0,
        fov_deg: 80.0,
    };
    let temp = tempfile::tempdir().expect("create isolated capture directory");
    let run = |name: &str, overrides: serde_json::Value| {
        capture(
            &workspace,
            temp.path(),
            name,
            &map,
            pose,
            // Bloom is a screen-space halo: it carries a neighbouring block's
            // static light across the block boundary, which no per-block term
            // comparison can account for. Off, each pixel is its own terms.
            Run {
                resolution: MISS_RESOLUTION,
                lightmap_mode: "stream",
                bloom: false,
            },
            overrides,
        )
    };
    let static_terms = LightTermMask::BAKED_DIRECT_STATIC.bits() | LightTermMask::SPECULAR.bits();
    let indirect_terms =
        LightTermMask::INDIRECT_STATIC.bits() | LightTermMask::INDIRECT_ANIMATED.bits();
    let Some(resident) = run("resident", serde_json::json!({})) else {
        return;
    };
    let Some(masked) = run(
        "masked",
        serde_json::json!({ "light_term_mask": LightTermMask::ALL.bits() & !static_terms }),
    ) else {
        return;
    };
    let Some(masked_indirect) = run(
        "masked-indirect",
        serde_json::json!({
            "light_term_mask": LightTermMask::ALL.bits() & !static_terms & !indirect_terms
        }),
    ) else {
        return;
    };
    assert_eq!(resident.lightmap["mode"], "stream");
    assert_eq!(resident.lightmap["resident_blocks"], block_count);

    let (width, height) = resident.image.dimensions();
    let mut owner: Vec<Option<u32>> = vec![None; (width * height) as usize];
    let mut chosen: Option<(u32, usize)> = None;
    for block in 0..block_count {
        let Some(missing) = run(
            &format!("missing-{block}"),
            serde_json::json!({ "force_missing_lightmap_blocks": [block] }),
        ) else {
            return;
        };
        assert_eq!(missing.lightmap["forced_missing_blocks"], 1);
        let mut region = 0;
        for (x, y, pixel) in missing.image.enumerate_pixels() {
            if *pixel == *resident.image.get_pixel(x, y) {
                continue;
            }
            region += 1;
            assert_eq!(
                pixel,
                masked.image.get_pixel(x, y),
                "block {block} missing: pixel ({x}, {y}) must equal the capture with static \
                 direct and specular masked off",
            );
            let slot = &mut owner[(y * width + x) as usize];
            assert_eq!(
                *slot, None,
                "pixel ({x}, {y}) changed under two blocks' misses"
            );
            *slot = Some(block);
        }
        eprintln!("block {block}: forced miss changes {region} pixel(s)");
        if chosen.is_none_or(|(_, best)| region > best) {
            chosen = Some((block, region));
        }
    }

    let (chosen, region) = chosen.expect("at least one block");
    assert!(
        region > 0,
        "the block covering most of the view must render lit when resident"
    );
    for (x, y, pixel) in resident.image.enumerate_pixels() {
        let static_lit = pixel != masked.image.get_pixel(x, y);
        assert_eq!(
            static_lit,
            owner[(y * width + x) as usize].is_some(),
            "pixel ({x}, {y}): the blocks' miss regions must be exactly the pixels static direct \
             and specular light",
        );
    }
    let indirect_in_region = owner
        .iter()
        .enumerate()
        .filter(|(_, block)| **block == Some(chosen))
        .filter(|(index, _)| {
            let (x, y) = (*index as u32 % width, *index as u32 / width);
            masked.image.get_pixel(x, y) != masked_indirect.image.get_pixel(x, y)
        })
        .count();
    eprintln!(
        "chosen block {chosen}: {region} pixel(s), {indirect_in_region} carry SH indirect while \
         missing"
    );
    assert!(
        indirect_in_region > 0,
        "SH indirect must remain on the missing block's pixels"
    );
}

fn lit_pixels(image: &RgbaImage) -> usize {
    image
        .pixels()
        .filter(|pixel| pixel.0[..3].iter().any(|&channel| channel > 8))
        .count()
}

fn differing_pixels(a: &RgbaImage, b: &RgbaImage) -> usize {
    a.pixels().zip(b.pixels()).filter(|(a, b)| a != b).count()
}
