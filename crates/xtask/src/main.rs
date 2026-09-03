// Development workflow entry points for Postretro.
// See: context/lib/development_guide.md

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use glam::{Mat3, Mat4, Vec3};
use postretro_level_format::prm::cache_filename_for_key;
use postretro_model::gltf_loader::{LoadedModel, load_model};
use postretro_model::mount::{
    MountAxes, MountConfidence, MountDetection, MountVerification, corrective_delta,
    corrective_delta_for_axes, detect_weapon_mount, read_muzzle_offset_in_model,
    resolve_socket_frame_in_model, verify_mount,
};

mod crate_graph;
mod dist;

fn main() {
    let code = match try_main() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("xtask: {err}");
            1
        }
    };
    std::process::exit(code);
}

fn try_main() -> Result<i32, String> {
    let mut args = std::env::args_os();
    let _program = args.next();
    let Some(command) = args.next() else {
        print_help();
        return Ok(1);
    };

    if command == "help" || command == "--help" || command == "-h" {
        print_help();
        return Ok(0);
    }

    if command == "run" {
        return run_postretro(args.collect());
    }

    if command == "observe" {
        return observe_headless(args.collect());
    }

    if command == "capture" {
        return capture_frame(args.collect());
    }

    if command == "mint-identity" {
        return mint_identity(args.collect());
    }

    if command == "bake-model-textures" {
        return bake_model_textures_command(args.collect());
    }

    if command == "solve-weapon-mount" {
        return solve_weapon_mount_command(args.collect());
    }

    if command == "crate-graph" {
        return crate_graph::run(args.collect());
    }

    if command == "dist" {
        return dist::run(args.collect());
    }

    Err(format!(
        "unknown command `{}`\n\nRun `cargo run -p xtask -- --help` for usage.",
        command.to_string_lossy()
    ))
}

fn bake_model_textures_command(args: Vec<OsString>) -> Result<i32, String> {
    let gltf_path = parse_bake_model_textures_args(args)?;
    let workspace_root = workspace_root()?;
    let gltf_path = workspace_relative_path(&gltf_path, &workspace_root);
    let prm_root = workspace_root.join("baked").join("materials");
    let baked = bake_model_textures_for_gltf(&gltf_path, &prm_root)?;

    if baked.is_empty() {
        println!(
            "No filesystem base-color textures found in {}",
            gltf_path.display()
        );
        return Ok(0);
    }

    for texture in baked {
        println!(
            "Baked {} -> {} (key {})",
            texture.source_path.display(),
            texture.prm_path.display(),
            texture.key_hex
        );
    }

    Ok(0)
}

fn parse_bake_model_textures_args(args: Vec<OsString>) -> Result<PathBuf, String> {
    match args.as_slice() {
        [gltf_path] => Ok(PathBuf::from(gltf_path)),
        [] => Err("bake-model-textures requires a glTF path\n\n\
             Usage: cargo run -p xtask -- bake-model-textures <scene.gltf>"
            .to_string()),
        _ => Err("bake-model-textures accepts exactly one glTF path\n\n\
             Usage: cargo run -p xtask -- bake-model-textures <scene.gltf>"
            .to_string()),
    }
}

/// Solve a rigid weapon bake against the engine's neutral sampled socket frame,
/// then print the Blender command that performs the actual vertex bake.
fn solve_weapon_mount_command(args: Vec<OsString>) -> Result<i32, String> {
    if let Some(viewmodel_path) = parse_read_muzzle_offset_args(&args)? {
        return read_muzzle_offset_command(&viewmodel_path);
    }

    let args = parse_solve_weapon_mount_args(args)?;
    let holder = load_model(&args.holder_path)
        .map_err(|error| format!("load skeleton {}: {error}", args.holder_path.display()))?;
    let weapon = load_model(&args.weapon_path)
        .map_err(|error| format!("load weapon {}: {error}", args.weapon_path.display()))?;
    let socket = resolve_socket_frame_in_model(&holder, &args.clip, &args.mount_joint, args.time)
        .map_err(|error| {
        format!(
            "resolve mount joint {:?} on {}: {error}",
            args.mount_joint,
            args.holder_path.display()
        )
    })?;

    println!(
        "Socket {:?} -> joint {}; clip {:?} @ t={}",
        args.mount_joint,
        socket.joint_index,
        args.clip,
        format_number(args.time),
    );
    print_non_reference_mount_pose_note(&args.clip, args.time);

    if args.check {
        return check_weapon_mount(&args, &weapon, socket.matrix);
    }

    let cli_axes = args.cli_axes()?;
    let declared_axes = cli_axes.or(weapon.mount);
    let (euler, emitted_axes, unverified) = match declared_axes {
        Some(axes) => {
            let delta = corrective_delta_for_axes(socket.matrix, axes)
                .map_err(|error| format!("solve declared weapon axes: {error}"))?;
            println!(
                "Declared raw-source axes: barrel {}  up {}",
                format_vec3(axes.barrel),
                format_vec3(axes.up),
            );
            (
                blender_xyz_euler_degrees(gltf_to_blender_rotation(delta)),
                Some(axes),
                false,
            )
        }
        None => {
            let detection = detect_weapon_mount(&weapon)
                .map_err(|error| format!("detect weapon mount geometry: {error}"))?;
            let current_euler =
                current_bake_euler(weapon.mount, args.current_euler).ok_or_else(|| {
                    solve_weapon_mount_usage(
                        "geometric assist requires the current bake euler from extras.mount.euler or --current-euler X Y Z",
                    )
                })?;
            let current_blender = blender_xyz_rotation(current_euler);
            let residual = corrective_delta(socket.matrix, detection.frame)
                .map_err(|error| format!("solve geometric residual: {error}"))?;
            let total_blender = gltf_to_blender_rotation(residual) * current_blender;

            // Detection measures the already-baked mesh. Show an author a
            // source-frame candidate, but never persist it as declared intent.
            let current_gltf = blender_to_gltf_rotation(current_blender);
            let candidate_axes = MountAxes {
                barrel: current_gltf.transpose() * detection.frame.barrel,
                up: current_gltf.transpose() * detection.frame.up,
                euler: None,
            };

            println!("UNVERIFIED geometric assist — no declared barrel/up axes were found.");
            print_detected_baked_frame(detection);
            println!(
                "UNVERIFIED raw-source candidate axes: barrel {}  up {}",
                format_vec3(candidate_axes.barrel),
                format_vec3(candidate_axes.up),
            );
            println!(
                "Current baked Blender XYZ euler: {} {} {}",
                format_number(current_euler[0]),
                format_number(current_euler[1]),
                format_number(current_euler[2]),
            );
            println!(
                "UNVERIFIED assist rebake will not persist mount axes; declare barrel/up before a VERIFIED check."
            );
            (blender_xyz_euler_degrees(total_blender), None, true)
        }
    };

    let prefix = if unverified { "UNVERIFIED " } else { "" };
    println!(
        "{prefix}Blender XYZ rotate-euler (degrees): {} {} {}",
        format_number(euler[0]),
        format_number(euler[1]),
        format_number(euler[2]),
    );
    println!("Run this command (emit-only; xtask does not invoke Blender):");
    println!("{}", emitted_blender_command(&args, euler, emitted_axes));
    Ok(0)
}

/// Print the author-time, model-local muzzle point from a rigid viewmodel socket.
///
/// This deliberately stays separate from the skinned holder-joint solver below:
/// a viewmodel muzzle is a composed rest translation in mesh-node-local space,
/// not an animated skinned socket frame.
fn read_muzzle_offset_command(viewmodel_path: &Path) -> Result<i32, String> {
    let viewmodel = load_model(viewmodel_path).map_err(|error| {
        format!(
            "load weapon viewmodel {}: {error}",
            viewmodel_path.display()
        )
    })?;
    let offset = read_muzzle_offset_in_model(&viewmodel).map_err(|error| {
        format!(
            "read muzzleOffset from {}: {error}",
            viewmodel_path.display()
        )
    })?;

    println!("muzzleOffset: {}", format_vec3(offset));
    println!("Raw model-local metres from the rigid viewmodel \"muzzle\" socket.");
    Ok(0)
}

/// Recognize the distinct viewmodel-only read before parsing the regular mount solver.
fn parse_read_muzzle_offset_args(args: &[OsString]) -> Result<Option<PathBuf>, String> {
    if args
        .first()
        .map(|argument| argument != "--read-muzzle-offset")
        .unwrap_or(true)
    {
        return Ok(None);
    }

    match args {
        [_, viewmodel_path] => Ok(Some(PathBuf::from(argument_string(
            viewmodel_path,
            "--read-muzzle-offset",
        )?))),
        _ => Err(solve_weapon_mount_usage(
            "--read-muzzle-offset requires exactly one weapon viewmodel glTF path",
        )),
    }
}

/// Check a baked weapon against its holder socket without invoking Blender.
fn check_weapon_mount(
    args: &SolveWeaponMountArgs,
    weapon: &LoadedModel,
    socket_matrix: Mat4,
) -> Result<i32, String> {
    let declared_axes = args.cli_axes()?.or(weapon.mount);
    let (verification, status) = match declared_axes {
        Some(declared_axes) => {
            let applied_euler = applied_check_euler(weapon.mount, args.current_euler)?;
            let baked_axes = compose_declared_axes_into_baked_frame(declared_axes, applied_euler);
            let baked_frame = baked_axes
                .frame()
                .map_err(|error| format!("compose declared weapon axes: {error}"))?;

            println!(
                "Declared raw-source axes: barrel {}  up {}",
                format_vec3(declared_axes.barrel),
                format_vec3(declared_axes.up),
            );
            println!(
                "Composed baked-frame axes: barrel {}  up {}",
                format_vec3(baked_axes.barrel),
                format_vec3(baked_axes.up),
            );
            (
                verify_mount(socket_matrix, baked_frame)
                    .map_err(|error| format!("verify declared weapon mount: {error}"))?,
                "VERIFIED",
            )
        }
        None => {
            let detection = detect_weapon_mount(weapon)
                .map_err(|error| format!("detect weapon mount geometry: {error}"))?;
            println!("UNVERIFIED geometric assist — no declared barrel/up axes were found.");
            print_detected_baked_frame(detection);
            (
                verify_mount(socket_matrix, detection.frame)
                    .map_err(|error| format!("verify geometric weapon mount: {error}"))?,
                "UNVERIFIED",
            )
        }
    };

    print_mount_metrics(verification);
    let failures = failed_mount_metrics(verification, args.thresholds);
    if failures.is_empty() {
        println!("{status}: mount check passed.");
        Ok(0)
    } else {
        println!("{status}: mount check failed: {}", failures.join(", "));
        Ok(1)
    }
}

fn applied_check_euler(
    persisted_mount: Option<MountAxes>,
    current_euler: Option<[f32; 3]>,
) -> Result<[f32; 3], String> {
    current_bake_euler(persisted_mount, current_euler).ok_or_else(|| {
        solve_weapon_mount_usage(
            "declared check is missing the applied euler; add extras.mount.euler or --current-euler X Y Z",
        )
    })
}

fn current_bake_euler(
    persisted_mount: Option<MountAxes>,
    cli_euler: Option<[f32; 3]>,
) -> Option<[f32; 3]> {
    cli_euler.or_else(|| persisted_mount.and_then(|mount| mount.euler))
}

/// Compose raw-source declared axes through the rotation baked into the weapon.
fn compose_declared_axes_into_baked_frame(
    declared_axes: MountAxes,
    applied_blender_euler: [f32; 3],
) -> MountAxes {
    let applied_gltf_rotation =
        blender_to_gltf_rotation(blender_xyz_rotation(applied_blender_euler));
    MountAxes {
        barrel: applied_gltf_rotation * declared_axes.barrel,
        up: applied_gltf_rotation * declared_axes.up,
        euler: None,
    }
}

fn print_mount_metrics(verification: MountVerification) {
    println!(
        "barrel·+Z: {}",
        format_number(verification.barrel_dot_forward),
    );
    println!("barrel·+Y: {}", format_number(verification.barrel_dot_up));
    println!("up·+Y: {}", format_number(verification.up_dot_up));
}

fn print_low_confidence_detection_warning(confidence: MountConfidence) {
    if confidence == MountConfidence::Low {
        println!(
            "WARNING: geometric assist detection is LOW confidence; its result remains UNVERIFIED."
        );
    }
}

fn print_detected_baked_frame(detection: MountDetection) {
    println!(
        "Detected baked-frame axes: barrel {}  up {}  side {}",
        format_vec3(detection.frame.barrel),
        format_vec3(detection.frame.up),
        format_vec3(detection.frame.side),
    );
    println!(
        "Detection: confidence {}; length {}; muzzle radius {}; stock radius {}",
        format_confidence(detection.confidence),
        format_number(detection.length),
        format_number(detection.muzzle.max_cross_radius),
        format_number(detection.stock.max_cross_radius),
    );
    print_low_confidence_detection_warning(detection.confidence);
}

fn print_non_reference_mount_pose_note(clip: &str, time: f32) {
    if !is_reference_mount_pose(clip, time) {
        println!(
            "NOTE: pose (clip {clip:?}, time {}) is not the reference (clip \"idle_aiming\", time 0). A rigid bake is exact only at this pose or the reference, not both; wrist-reorienting poses such as limitator \"reloading\" need a skinned weapon.",
            format_number(time),
        );
    }
}

fn is_reference_mount_pose(clip: &str, time: f32) -> bool {
    clip == "idle_aiming" && time == 0.0
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MountCheckThresholds {
    min_barrel_dot: f32,
    max_barrel_y: f32,
    min_up_dot: f32,
}

impl Default for MountCheckThresholds {
    fn default() -> Self {
        Self {
            min_barrel_dot: 0.999,
            max_barrel_y: 0.02,
            min_up_dot: 0.999,
        }
    }
}

fn failed_mount_metrics(
    verification: MountVerification,
    thresholds: MountCheckThresholds,
) -> Vec<&'static str> {
    let mut failures = Vec::new();
    if !verification.barrel_dot_forward.is_finite()
        || verification.barrel_dot_forward < thresholds.min_barrel_dot
    {
        failures.push("barrel·+Z");
    }
    if !verification.barrel_dot_up.is_finite()
        || verification.barrel_dot_up.abs() > thresholds.max_barrel_y
    {
        failures.push("|barrel·+Y|");
    }
    if !verification.up_dot_up.is_finite() || verification.up_dot_up < thresholds.min_up_dot {
        failures.push("up·+Y");
    }
    failures
}

#[derive(Debug, PartialEq)]
struct SolveWeaponMountArgs {
    holder_path: PathBuf,
    mount_joint: String,
    clip: String,
    time: f32,
    weapon_path: PathBuf,
    barrel: Option<Vec3>,
    up: Option<Vec3>,
    check: bool,
    raw_source: Option<String>,
    out: Option<String>,
    grip: Option<[String; 3]>,
    scale: Option<String>,
    sockets: Vec<String>,
    current_euler: Option<[f32; 3]>,
    thresholds: MountCheckThresholds,
}

impl SolveWeaponMountArgs {
    fn cli_axes(&self) -> Result<Option<MountAxes>, String> {
        match (self.barrel, self.up) {
            (Some(barrel), Some(up)) => Ok(Some(MountAxes {
                barrel,
                up,
                euler: None,
            })),
            (None, None) => Ok(None),
            _ => Err(solve_weapon_mount_usage(
                "--barrel and --up must be supplied together",
            )),
        }
    }
}

fn parse_solve_weapon_mount_args(args: Vec<OsString>) -> Result<SolveWeaponMountArgs, String> {
    let mut holder_path = None;
    let mut mount_joint = None;
    let mut clip = None;
    let mut time = None;
    let mut weapon_path = None;
    let mut barrel = None;
    let mut up = None;
    let mut check = false;
    let mut raw_source = None;
    let mut out = None;
    let mut grip = None;
    let mut scale = None;
    let mut sockets = Vec::new();
    let mut current_euler = None;
    let mut min_barrel_dot = None;
    let mut max_barrel_y = None;
    let mut min_up_dot = None;
    let mut index = 0;

    while index < args.len() {
        let argument = argument_string(&args[index], "solve-weapon-mount argument")?;
        match argument.as_str() {
            "--mount-joint" => {
                set_once(
                    &mut mount_joint,
                    next_argument(&args, &mut index, "--mount-joint")?,
                    "--mount-joint",
                )?;
            }
            "--clip" => {
                set_once(
                    &mut clip,
                    next_argument(&args, &mut index, "--clip")?,
                    "--clip",
                )?;
            }
            "--time" => {
                let value = next_argument(&args, &mut index, "--time")?;
                set_once(&mut time, parse_finite_number(&value, "--time")?, "--time")?;
            }
            "--weapon" => {
                let value = next_argument(&args, &mut index, "--weapon")?;
                set_once(&mut weapon_path, PathBuf::from(value), "--weapon")?;
            }
            "--barrel" => {
                set_once(
                    &mut barrel,
                    parse_vec3(&args, &mut index, "--barrel")?,
                    "--barrel",
                )?;
            }
            "--up" => {
                set_once(&mut up, parse_vec3(&args, &mut index, "--up")?, "--up")?;
            }
            "--check" => {
                if check {
                    return Err(solve_weapon_mount_usage(
                        "solve-weapon-mount accepts only one --check",
                    ));
                }
                check = true;
                index += 1;
            }
            "--raw-source" => {
                set_once(
                    &mut raw_source,
                    next_argument(&args, &mut index, "--raw-source")?,
                    "--raw-source",
                )?;
            }
            "--out" => {
                set_once(
                    &mut out,
                    next_argument(&args, &mut index, "--out")?,
                    "--out",
                )?;
            }
            "--grip" => {
                set_once(
                    &mut grip,
                    parse_raw_vec3(&args, &mut index, "--grip")?,
                    "--grip",
                )?;
            }
            "--scale" => {
                let value = next_argument(&args, &mut index, "--scale")?;
                let parsed = parse_finite_number(&value, "--scale")?;
                if parsed <= 0.0 {
                    return Err(solve_weapon_mount_usage(&format!(
                        "--scale must be greater than zero, got {value:?}"
                    )));
                }
                set_once(&mut scale, value, "--scale")?;
            }
            "--socket" => {
                let socket = next_argument(&args, &mut index, "--socket")?;
                let Some((name, node)) = socket.split_once('=') else {
                    return Err(solve_weapon_mount_usage(
                        "--socket requires NAME=NODE (this is a prop metadata tag, not --mount-joint)",
                    ));
                };
                if name.is_empty() || node.is_empty() {
                    return Err(solve_weapon_mount_usage(
                        "--socket requires non-empty NAME and NODE in NAME=NODE",
                    ));
                }
                sockets.push(socket);
            }
            "--current-euler" => {
                set_once(
                    &mut current_euler,
                    parse_array3(&args, &mut index, "--current-euler")?,
                    "--current-euler",
                )?;
            }
            "--min-barrel-dot" => {
                set_once(
                    &mut min_barrel_dot,
                    parse_threshold(&args, &mut index, "--min-barrel-dot", -1.0, 1.0)?,
                    "--min-barrel-dot",
                )?;
            }
            "--max-barrel-y" => {
                set_once(
                    &mut max_barrel_y,
                    parse_threshold(&args, &mut index, "--max-barrel-y", 0.0, 1.0)?,
                    "--max-barrel-y",
                )?;
            }
            "--min-up-dot" => {
                set_once(
                    &mut min_up_dot,
                    parse_threshold(&args, &mut index, "--min-up-dot", -1.0, 1.0)?,
                    "--min-up-dot",
                )?;
            }
            option if option.starts_with('-') => {
                return Err(solve_weapon_mount_usage(&format!(
                    "unknown solve-weapon-mount option {option:?}"
                )));
            }
            path => {
                if holder_path.replace(PathBuf::from(path)).is_some() {
                    return Err(solve_weapon_mount_usage(
                        "solve-weapon-mount accepts exactly one skeleton model path",
                    ));
                }
                index += 1;
            }
        }
    }

    let holder_path = holder_path.ok_or_else(|| {
        solve_weapon_mount_usage("solve-weapon-mount requires a skeleton model path")
    })?;
    let weapon_path = weapon_path
        .ok_or_else(|| solve_weapon_mount_usage("solve-weapon-mount requires --weapon <path>"))?;
    if !check && raw_source.is_none() {
        return Err(solve_weapon_mount_usage(
            "solve-weapon-mount requires --raw-source <path> in solve mode",
        ));
    }
    if !check && out.is_none() {
        return Err(solve_weapon_mount_usage(
            "solve-weapon-mount requires --out <path> in solve mode",
        ));
    }

    if barrel.is_some() != up.is_some() {
        return Err(solve_weapon_mount_usage(
            "--barrel and --up must be supplied together",
        ));
    }

    Ok(SolveWeaponMountArgs {
        holder_path,
        mount_joint: mount_joint.unwrap_or_else(|| "hand_r".to_string()),
        clip: clip.unwrap_or_else(|| "idle_aiming".to_string()),
        time: time.unwrap_or(0.0),
        weapon_path,
        barrel,
        up,
        check,
        raw_source,
        out,
        grip,
        scale,
        sockets,
        current_euler,
        thresholds: MountCheckThresholds {
            min_barrel_dot: min_barrel_dot.unwrap_or(0.999),
            max_barrel_y: max_barrel_y.unwrap_or(0.02),
            min_up_dot: min_up_dot.unwrap_or(0.999),
        },
    })
}

fn next_argument(args: &[OsString], index: &mut usize, option: &str) -> Result<String, String> {
    let Some(value) = args.get(*index + 1) else {
        return Err(solve_weapon_mount_usage(&format!(
            "{option} requires a value"
        )));
    };
    *index += 2;
    argument_string(value, option)
}

fn parse_vec3(args: &[OsString], index: &mut usize, option: &str) -> Result<Vec3, String> {
    Ok(Vec3::from_array(parse_array3(args, index, option)?))
}

fn parse_array3(args: &[OsString], index: &mut usize, option: &str) -> Result<[f32; 3], String> {
    let values = parse_raw_vec3(args, index, option)?;
    Ok([
        parse_finite_number(&values[0], option)?,
        parse_finite_number(&values[1], option)?,
        parse_finite_number(&values[2], option)?,
    ])
}

fn parse_raw_vec3(
    args: &[OsString],
    index: &mut usize,
    option: &str,
) -> Result<[String; 3], String> {
    let first = next_argument(args, index, option)?;
    let second = next_vector_component(args, index, option)?;
    let third = next_vector_component(args, index, option)?;
    for value in [&first, &second, &third] {
        let _ = parse_finite_number(value, option)?;
    }
    Ok([first, second, third])
}

fn next_vector_component(
    args: &[OsString],
    index: &mut usize,
    option: &str,
) -> Result<String, String> {
    let Some(value) = args.get(*index) else {
        return Err(solve_weapon_mount_usage(&format!(
            "{option} requires exactly three finite numbers"
        )));
    };
    *index += 1;
    argument_string(value, option)
}

fn argument_string(value: &OsString, label: &str) -> Result<String, String> {
    value
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| solve_weapon_mount_usage(&format!("{label} must be valid UTF-8")))
}

fn parse_finite_number(value: &str, option: &str) -> Result<f32, String> {
    let parsed = value.parse::<f32>().map_err(|_| {
        solve_weapon_mount_usage(&format!("{option} expects a finite number, got {value:?}"))
    })?;
    if !parsed.is_finite() {
        return Err(solve_weapon_mount_usage(&format!(
            "{option} expects a finite number, got {value:?}"
        )));
    }
    Ok(parsed)
}

fn parse_threshold(
    args: &[OsString],
    index: &mut usize,
    option: &str,
    minimum: f32,
    maximum: f32,
) -> Result<f32, String> {
    let value = next_argument(args, index, option)?;
    let parsed = parse_finite_number(&value, option)?;
    if !(minimum..=maximum).contains(&parsed) {
        return Err(solve_weapon_mount_usage(&format!(
            "{option} must be between {minimum} and {maximum}, got {value:?}"
        )));
    }
    Ok(parsed)
}

fn set_once<T>(slot: &mut Option<T>, value: T, option: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(solve_weapon_mount_usage(&format!(
            "solve-weapon-mount accepts only one {option}"
        )));
    }
    Ok(())
}

fn solve_weapon_mount_usage(message: &str) -> String {
    format!(
        "{message}\n\nUsage: cargo run -p xtask -- solve-weapon-mount <skeleton.gltf> \\
         --weapon <baked-weapon.gltf> [--check] [--raw-source <raw-source> --out <output.gltf>] \\
         [--mount-joint NAME] [--clip NAME] [--time SECONDS] \\
         [--barrel X Y Z --up X Y Z] [--current-euler X Y Z] \\
         [--min-barrel-dot VALUE] [--max-barrel-y VALUE] [--min-up-dot VALUE] \\
         [--grip X Y Z] [--scale FACTOR] [--socket NAME=NODE]..."
    )
}

/// glTF-to-Blender basis change for rotation operators. The columns map
/// `(x, y, z)` to `(x, -z, y)`, so a corrective rotation needs a two-sided
/// similarity transform rather than a one-sided vector conversion.
fn gltf_to_blender_rotation(rotation: Mat3) -> Mat3 {
    let map = gltf_to_blender_basis();
    map * rotation * map.transpose()
}

fn blender_to_gltf_rotation(rotation: Mat3) -> Mat3 {
    let map = gltf_to_blender_basis();
    map.transpose() * rotation * map
}

fn gltf_to_blender_basis() -> Mat3 {
    Mat3::from_cols(Vec3::X, Vec3::Z, -Vec3::Y)
}

/// Blender's `XYZ` Euler mode applies `Rz * Ry * Rx`.
fn blender_xyz_rotation(euler_degrees: [f32; 3]) -> Mat3 {
    Mat3::from_rotation_z(euler_degrees[2].to_radians())
        * Mat3::from_rotation_y(euler_degrees[1].to_radians())
        * Mat3::from_rotation_x(euler_degrees[0].to_radians())
}

fn blender_xyz_euler_degrees(rotation: Mat3) -> [f32; 3] {
    let sine_y = (-rotation.x_axis.z).clamp(-1.0, 1.0);
    let y = sine_y.asin();
    let cosine_y = (1.0 - sine_y * sine_y).sqrt();
    let (x, z) = if cosine_y > 1.0e-6 {
        (
            rotation.y_axis.z.atan2(rotation.z_axis.z),
            rotation.x_axis.y.atan2(rotation.x_axis.x),
        )
    } else if sine_y.is_sign_positive() {
        // At +90° pitch, Blender's XYZ representation has one free degree of
        // freedom. Choosing Z = 0 leaves a stable, equivalent X rotation.
        (rotation.y_axis.x.atan2(rotation.y_axis.y), 0.0)
    } else {
        // At -90° pitch the observable combination is X + Z.
        ((-rotation.y_axis.x).atan2(rotation.y_axis.y), 0.0)
    };
    [x.to_degrees(), y.to_degrees(), z.to_degrees()]
}

fn emitted_blender_command(
    args: &SolveWeaponMountArgs,
    euler: [f32; 3],
    axes: Option<MountAxes>,
) -> String {
    let raw_source = args
        .raw_source
        .as_deref()
        .expect("solve parser requires --raw-source before emitting a Blender command");
    let out = args
        .out
        .as_deref()
        .expect("solve parser requires --out before emitting a Blender command");
    let mut command = vec![
        "blender".to_string(),
        "--background".to_string(),
        "--python".to_string(),
        "tools/prop_to_gltf.py".to_string(),
        "--".to_string(),
        "--input".to_string(),
        shell_quote(raw_source),
        "--output".to_string(),
        shell_quote(out),
    ];
    if let Some(grip) = &args.grip {
        command.push("--grip".to_string());
        command.extend(grip.iter().cloned());
    }
    if let Some(scale) = &args.scale {
        command.push("--scale".to_string());
        command.push(scale.clone());
    }
    for socket in &args.sockets {
        command.push("--socket".to_string());
        command.push(shell_quote(socket));
    }
    command.push("--rotate-euler".to_string());
    command.extend(euler.into_iter().map(format_number));
    if let Some(axes) = axes {
        command.push("--mount-axes".to_string());
        command.extend(
            [axes.barrel, axes.up]
                .into_iter()
                .flat_map(|axis| axis.to_array().into_iter().map(format_number)),
        );
    }
    command.join(" ")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn format_number(value: f32) -> String {
    format!("{value:.6}")
}

fn format_vec3(value: Vec3) -> String {
    let [x, y, z] = value.to_array();
    format!(
        "[{}, {}, {}]",
        format_number(x),
        format_number(y),
        format_number(z),
    )
}

fn format_confidence(confidence: MountConfidence) -> &'static str {
    match confidence {
        MountConfidence::High => "high",
        MountConfidence::Low => "LOW",
    }
}

fn workspace_relative_path(path: &Path, workspace_root: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct BakedModelTexture {
    source_path: PathBuf,
    key_hex: String,
    prm_path: PathBuf,
}

fn bake_model_textures_for_gltf(
    gltf_path: &Path,
    prm_root: &Path,
) -> Result<Vec<BakedModelTexture>, String> {
    bake_model_textures_for_gltf_with(
        gltf_path,
        prm_root,
        postretro_level_format::gltf_resolve::resolve_document_base_color_paths,
        postretro_level_compiler::texture_mips::bake_diffuse_texture,
    )
}

fn bake_model_textures_for_gltf_with<Resolve, ResolveError, Bake, BakeError>(
    gltf_path: &Path,
    prm_root: &Path,
    mut resolve_base_color_paths: Resolve,
    mut bake_diffuse: Bake,
) -> Result<Vec<BakedModelTexture>, String>
where
    Resolve: FnMut(&Path) -> Result<Vec<PathBuf>, ResolveError>,
    ResolveError: Display,
    Bake: FnMut(&Path, &Path) -> Result<[u8; 32], BakeError>,
    BakeError: Display,
{
    let texture_paths = resolve_base_color_paths(gltf_path).map_err(|error| {
        format!(
            "resolve model textures for {}: {error}",
            gltf_path.display()
        )
    })?;

    let mut seen = HashSet::new();
    let mut baked = Vec::new();
    for texture_path in texture_paths {
        if !seen.insert(texture_path.clone()) {
            continue;
        }

        let key = bake_diffuse(&texture_path, prm_root)
            .map_err(|error| format!("bake model texture {}: {error}", texture_path.display()))?;
        let key_hex = cache_filename_for_key(&key);
        baked.push(BakedModelTexture {
            source_path: texture_path,
            prm_path: prm_root.join(format!("{key_hex}.prm")),
            key_hex,
        });
    }

    Ok(baked)
}

fn run_postretro(engine_args: Vec<OsString>) -> Result<i32, String> {
    let run_args = split_run_args(engine_args);
    let sidecar_cargo_args = sidecar_cargo_args(&run_args.cargo_run_args);
    let workspace_root = workspace_root()?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));

    build_scripts_sidecar(&cargo, &workspace_root, &sidecar_cargo_args)?;

    let mut command = Command::new(&cargo);
    command
        .current_dir(&workspace_root)
        .arg("run")
        .arg("-p")
        .arg("postretro")
        .arg("--bin")
        .arg("postretro")
        .args(run_args.cargo_run_args)
        .arg("--")
        .args(run_args.engine_args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    status_code(
        command
            .status()
            .map_err(|e| format!("launch postretro: {e}")),
    )
}

/// Build the `scripts-build` sidecar shared by every launch path (`run`,
/// `observe`). `sidecar_cargo_args` mirrors the subset of cargo flags that must
/// reach the sidecar build (empty for `observe`, which takes no passthrough).
fn build_scripts_sidecar(
    cargo: &OsStr,
    workspace_root: &Path,
    sidecar_cargo_args: &[OsString],
) -> Result<(), String> {
    let mut sidecar_build = Command::new(cargo);
    sidecar_build
        .current_dir(workspace_root)
        .arg("build")
        .arg("-p")
        .arg("postretro-script-compiler")
        .arg("--bin")
        .arg("scripts-build")
        .args(sidecar_cargo_args);

    run_checked(&mut sidecar_build, "build scripts-build")
}

/// `observe <runspec.json>`: build the scripts sidecar, then run the engine
/// headless under the `observability` feature. xtask is a transparent pipe — it
/// forwards the child's stdout, stderr, and exit code untouched and never parses
/// the runspec or the JSON document the engine emits. Unlike `run`, `observe`
/// takes no general cargo/engine passthrough: it always builds with `--features
/// observability` and always passes `--headless`. `--pool-seed` is the one
/// supported engine option because headless pool rolls must be pinnable.
fn observe_headless(args: Vec<OsString>) -> Result<i32, String> {
    let observe_args = parse_observe_args(args)?;
    let workspace_root = workspace_root()?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));

    build_scripts_sidecar(&cargo, &workspace_root, &[])?;

    // Cargo's own build/status output goes to stderr; the engine writes the JSON
    // document to stdout. Inheriting all three streams keeps stdout pristine JSON
    // and propagates the child's exit code. Path resolution is left to cargo's
    // working directory (the workspace root), mirroring the `run` plumbing.
    let mut command = Command::new(&cargo);
    command
        .current_dir(&workspace_root)
        .arg("run")
        .arg("-p")
        .arg("postretro")
        .arg("--bin")
        .arg("postretro")
        .arg("--features")
        .arg("observability")
        .arg("--")
        .args(observe_postretro_args(&observe_args))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    status_code(
        command
            .status()
            .map_err(|e| format!("launch postretro: {e}")),
    )
}

#[derive(Debug, PartialEq, Eq)]
struct ObserveArgs {
    runspec: PathBuf,
    pool_seed: Option<OsString>,
}

fn parse_observe_args(args: Vec<OsString>) -> Result<ObserveArgs, String> {
    let mut runspec = None;
    let mut pool_seed = None;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if arg == "--pool-seed" {
            if pool_seed.is_some() {
                return Err(observe_usage("observe accepts only one --pool-seed"));
            }
            let Some(value) = args.get(index + 1) else {
                return Err(observe_usage("observe --pool-seed requires a value"));
            };
            pool_seed = Some(value.clone());
            index += 2;
            continue;
        }

        if let Some(value) = arg
            .to_str()
            .and_then(|arg| arg.strip_prefix("--pool-seed="))
        {
            if pool_seed.is_some() {
                return Err(observe_usage("observe accepts only one --pool-seed"));
            }
            pool_seed = Some(OsString::from(value));
            index += 1;
            continue;
        }

        if runspec.replace(PathBuf::from(arg)).is_some() {
            return Err(observe_usage("observe accepts exactly one runspec path"));
        }
        index += 1;
    }

    let runspec = runspec.ok_or_else(|| observe_usage("observe requires a runspec path"))?;
    Ok(ObserveArgs { runspec, pool_seed })
}

fn observe_postretro_args(args: &ObserveArgs) -> Vec<OsString> {
    let mut forwarded = vec![
        OsString::from("--headless"),
        args.runspec.clone().into_os_string(),
    ];
    if let Some(seed) = &args.pool_seed {
        forwarded.push(OsString::from("--pool-seed"));
        forwarded.push(seed.clone());
    }
    forwarded
}

fn observe_usage(message: &str) -> String {
    format!(
        "{message}\n\nUsage: cargo run -p xtask -- observe <runspec.json> \
         [--pool-seed <u64> | --pool-seed=<u64>]"
    )
}

/// `capture <scene.json>`: run the engine's world-only frame capture mode.
/// Unlike `observe`, capture executes no scripts and therefore needs no
/// scripts-build sidecar.
fn capture_frame(args: Vec<OsString>) -> Result<i32, String> {
    let scene = parse_capture_args(args)?;
    let workspace_root = workspace_root()?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));

    let mut command = Command::new(&cargo);
    command
        .current_dir(&workspace_root)
        .arg("run")
        .arg("-p")
        .arg("postretro")
        .arg("--bin")
        .arg("postretro")
        .arg("--features")
        .arg("capture")
        .arg("--")
        .arg("--capture")
        .arg(&scene)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    status_code(
        command
            .status()
            .map_err(|e| format!("launch postretro capture: {e}")),
    )
}

fn parse_capture_args(args: Vec<OsString>) -> Result<PathBuf, String> {
    match args.as_slice() {
        [scene] => Ok(PathBuf::from(scene)),
        [] => Err("capture requires a scene path\n\n\
             Usage: cargo run -p xtask -- capture <scene.json>"
            .to_string()),
        _ => Err("capture accepts exactly one scene path\n\n\
             Usage: cargo run -p xtask -- capture <scene.json>"
            .to_string()),
    }
}

/// `mint-identity <mod-root>` builds the TypeScript sidecar only when the mod's
/// entry point needs it, then runs the authoring-only ledger mint binary. The
/// mint bin owns diagnostics; xtask only forwards stdio and status.
fn mint_identity(args: Vec<OsString>) -> Result<i32, String> {
    let mod_root = parse_mint_identity_args(args)?;
    let invocation_dir = std::env::current_dir()
        .map_err(|error| format!("resolve mint-identity invocation directory: {error}"))?;
    let plan = plan_mint_identity(&mod_root, &invocation_dir, Path::is_file);
    let workspace_root = workspace_root()?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));

    if plan.build_scripts_sidecar {
        build_scripts_sidecar(&cargo, &workspace_root, &[])?;
    }

    let mut command = Command::new(&cargo);
    command
        .current_dir(&workspace_root)
        .arg("run")
        .arg("-p")
        .arg("postretro")
        .arg("--bin")
        .arg("mint-identity")
        .arg("--")
        .arg(plan.mod_root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    status_code(
        command
            .status()
            .map_err(|error| format!("launch mint-identity: {error}")),
    )
}

#[derive(Debug, PartialEq, Eq)]
struct MintIdentityPlan {
    mod_root: PathBuf,
    build_scripts_sidecar: bool,
}

fn plan_mint_identity(
    supplied_mod_root: &Path,
    invocation_dir: &Path,
    is_file: impl Fn(&Path) -> bool,
) -> MintIdentityPlan {
    let mod_root = if supplied_mod_root.is_absolute() {
        supplied_mod_root.to_path_buf()
    } else {
        invocation_dir.join(supplied_mod_root)
    };
    let build_scripts_sidecar = is_file(&mod_root.join("start-script.ts"));
    MintIdentityPlan {
        mod_root,
        build_scripts_sidecar,
    }
}

fn parse_mint_identity_args(args: Vec<OsString>) -> Result<PathBuf, String> {
    match args.as_slice() {
        [mod_root] => Ok(PathBuf::from(mod_root)),
        [] => Err("mint-identity requires a mod root\n\n\
             Usage: cargo run -p xtask -- mint-identity <mod-root>"
            .to_string()),
        _ => Err("mint-identity accepts exactly one mod root\n\n\
             Usage: cargo run -p xtask -- mint-identity <mod-root>"
            .to_string()),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct RunArgs {
    cargo_run_args: Vec<OsString>,
    engine_args: Vec<OsString>,
}

fn split_run_args(args: Vec<OsString>) -> RunArgs {
    let Some(separator) = args.iter().position(|arg| arg == "--") else {
        return RunArgs {
            cargo_run_args: Vec::new(),
            engine_args: args,
        };
    };

    RunArgs {
        cargo_run_args: args[..separator].to_vec(),
        engine_args: args[separator + 1..].to_vec(),
    }
}

fn sidecar_cargo_args(cargo_run_args: &[OsString]) -> Vec<OsString> {
    let mut sidecar_args = Vec::new();
    let mut index = 0;
    while index < cargo_run_args.len() {
        let arg = &cargo_run_args[index];
        if arg == "--release" || arg == "-r" {
            sidecar_args.push(arg.clone());
            index += 1;
            continue;
        }

        if arg == "--profile" || arg == "--target-dir" {
            sidecar_args.push(arg.clone());
            if let Some(value) = cargo_run_args.get(index + 1) {
                sidecar_args.push(value.clone());
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }

        if arg == "--target" {
            sidecar_args.push(arg.clone());
            if let Some(value) = cargo_run_args.get(index + 1)
                && !value.as_encoded_bytes().starts_with(b"-")
            {
                sidecar_args.push(value.clone());
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }

        if let Some(arg) = arg.to_str() {
            if arg.starts_with("--profile=")
                || arg.starts_with("--target=")
                || arg.starts_with("--target-dir=")
            {
                sidecar_args.push(cargo_run_args[index].clone());
            }
        }
        index += 1;
    }

    sidecar_args
}

fn run_checked(command: &mut Command, label: &str) -> Result<(), String> {
    let status = command.status().map_err(|e| format!("{label}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{label}: exited with {status}"))
    }
}

fn status_code(status: Result<std::process::ExitStatus, String>) -> Result<i32, String> {
    let status = status?;
    Ok(status.code().unwrap_or(1))
}

fn workspace_root() -> Result<PathBuf, String> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|crates_dir| crates_dir.parent())
        .map(PathBuf::from)
        .ok_or_else(|| {
            format!(
                "could not derive workspace root from {}",
                manifest_dir.display()
            )
        })
}

fn print_help() {
    eprintln!(
        "Postretro development tasks\n\n\
         USAGE:\n\
           cargo run -p xtask -- run [cargo-run flags...] -- [postretro args...]\n\
           cargo run -p xtask -- run [postretro args...]\n\
           cargo run -p xtask -- observe <runspec.json> [--pool-seed=<u64>]\n\
           cargo run -p xtask -- capture <scene.json>\n\
           cargo run -p xtask -- mint-identity <mod-root>\n\
           cargo run -p xtask -- bake-model-textures <scene.gltf>\n\
           cargo run -p xtask -- solve-weapon-mount --read-muzzle-offset <viewmodel.gltf>\n\
           cargo run -p xtask -- solve-weapon-mount <skeleton.gltf> --weapon <weapon.gltf> [--check] [--raw-source <raw> --out <output.gltf>] [options]\n\
           cargo run -p xtask -- crate-graph [--write | --check | --mermaid | --rdeps <crate> | --deps <crate>]\n\
           cargo run -p xtask -- dist [--manifest <path>] [--out <dir>]\n\n\
         COMMANDS:\n\
           run                  Build scripts-build, then run the postretro engine\n\
           observe              Build scripts-build, then run the engine headless\n\
                                (--features observability --headless), forwarding\n\
                                the JSON document on stdout untouched\n\
           capture              Run the engine's world-only frame capture\n\
                                (--features capture --capture <scene.json>)\n\
           mint-identity        Mint a mod's durable state-slot identity ledger;\n\
                                builds scripts-build only for TypeScript mods\n\
           bake-model-textures  Bake glTF base-color sidecars into baked/materials\n\
           solve-weapon-mount   Read a viewmodel muzzle offset, solve a rigid weapon\n\
                                mount and print the Blender bake command, or --check\n\
                                a baked mount in-engine\n\
           crate-graph          Analyze the internal crate dependency graph: print it,\n\
                                --write the committed snapshot, --check its freshness,\n\
                                --mermaid the diagram, or query --rdeps / --deps of a crate\n\
           dist                 Build a host-native standalone distribution payload\n\n\
         EXAMPLES:\n\
           cargo run -p xtask -- run content/dev/maps/campaign-test.prl\n\
           cargo run -p xtask -- run --features dev-tools -- content/dev/maps/campaign-test.prl\n\
           cargo run -p xtask -- run --release -- content/dev/maps/campaign-test.prl\n\
           cargo run -p xtask -- observe runspec.json --pool-seed=17\n\
           cargo run -p xtask -- mint-identity content/dev\n\
           cargo run -p xtask -- bake-model-textures content/dev/models/reference_enemy_kaykit_knight/scene.gltf\n\
           cargo run -p xtask -- solve-weapon-mount --read-muzzle-offset content/dev/models/ar_4/model.gltf\n\
           cargo run -p xtask -- solve-weapon-mount content/dev/models/limitator/model.gltf --weapon content/dev/models/ar_4/model.gltf --barrel 0 1 0 --up 0 0 1 --raw-source raw/ar_4.glb --out content/dev/models/ar_4/model.gltf\n\n\
         NOTES:\n\
           Cargo flags before `--` are passed to the engine cargo run. Only\n\
           --release/-r, --profile, --target, and --target-dir are also mirrored\n\
           to the scripts-build sidecar build."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn write_weapon_fixture() -> PathBuf {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../model/tests/fixtures/multi_primitive/multi_primitive.gltf");
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time follows Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "postretro_xtask_mount_{}_{}.gltf",
            std::process::id(),
            unique,
        ));
        std::fs::copy(source, &path).expect("model fixture copies");
        path
    }

    fn postprocess_weapon_fixture_with_prop_writer(path: &Path) {
        let converter =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/prop_to_gltf.py");
        let python = r#"
import importlib.util
import sys
import types

sys.modules["bpy"] = types.ModuleType("bpy")
mathutils = types.ModuleType("mathutils")
mathutils.Vector = tuple
sys.modules["mathutils"] = mathutils

spec = importlib.util.spec_from_file_location("prop_to_gltf_test", sys.argv[1])
converter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(converter)
converter.postprocess_gltf(
    sys.argv[2],
    rotate_euler=[0.0, 0.0, 0.0],
    mount_axes=[0.0, 0.0, 2.0, 0.0, 3.0, 0.0],
)
"#;
        let output = Command::new("python3")
            .arg("-c")
            .arg(python)
            .arg(converter)
            .arg(path)
            .output()
            .expect("python3 runs the prop postprocessor");
        assert!(
            output.status.success(),
            "prop postprocessor failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn split_run_args_without_separator_keeps_backwards_compatible_engine_args() {
        assert_eq!(
            split_run_args(os_args(&[
                "content/dev/maps/campaign-test.prl",
                "--host",
                "127.0.0.1:3456",
            ])),
            RunArgs {
                cargo_run_args: Vec::new(),
                engine_args: os_args(&[
                    "content/dev/maps/campaign-test.prl",
                    "--host",
                    "127.0.0.1:3456",
                ]),
            }
        );
    }

    #[test]
    fn parse_mint_identity_args_requires_exactly_one_mod_root() {
        assert_eq!(
            parse_mint_identity_args(os_args(&["content/dev"])).expect("one path is valid"),
            PathBuf::from("content/dev"),
        );
        assert!(parse_mint_identity_args(Vec::new()).is_err());
        assert!(parse_mint_identity_args(os_args(&["one", "two"])).is_err());
    }

    #[test]
    fn split_run_args_uses_first_standalone_separator() {
        assert_eq!(
            split_run_args(os_args(&[
                "--features",
                "dev-tools",
                "--",
                "content/dev/maps/campaign-test.prl",
                "--",
                "--host",
            ])),
            RunArgs {
                cargo_run_args: os_args(&["--features", "dev-tools"]),
                engine_args: os_args(&["content/dev/maps/campaign-test.prl", "--", "--host",]),
            }
        );
    }

    #[test]
    fn sidecar_cargo_args_mirrors_profile_target_and_target_dir_flags() {
        assert_eq!(
            sidecar_cargo_args(&os_args(&[
                "--release",
                "-r",
                "--profile",
                "dev",
                "--profile=release-with-debug",
                "--target=x86_64-unknown-linux-gnu",
                "--target-dir",
                "target/custom",
                "--target-dir=target/other",
            ])),
            os_args(&[
                "--release",
                "-r",
                "--profile",
                "dev",
                "--profile=release-with-debug",
                "--target=x86_64-unknown-linux-gnu",
                "--target-dir",
                "target/custom",
                "--target-dir=target/other",
            ])
        );
    }

    #[test]
    fn sidecar_cargo_args_does_not_mirror_engine_package_feature_flags() {
        assert_eq!(
            sidecar_cargo_args(&os_args(&[
                "--features",
                "dev-tools",
                "--no-default-features",
                "--all-features",
                "--release",
            ])),
            os_args(&["--release"])
        );
    }

    #[test]
    fn sidecar_cargo_args_does_not_consume_feature_flag_as_optional_target_value() {
        assert_eq!(
            sidecar_cargo_args(&os_args(&[
                "--target",
                "--features",
                "dev-tools",
                "--target",
                "x86_64-unknown-linux-gnu",
            ])),
            os_args(&["--target", "--target", "x86_64-unknown-linux-gnu"])
        );
    }

    #[test]
    fn parse_observe_args_accepts_runspec_without_seed() {
        assert_eq!(
            parse_observe_args(os_args(&["runspec.json"])),
            Ok(ObserveArgs {
                runspec: PathBuf::from("runspec.json"),
                pool_seed: None,
            })
        );

        assert!(parse_observe_args(Vec::new()).is_err());
        assert!(parse_observe_args(os_args(&["a.json", "b.json"])).is_err());
    }

    #[test]
    fn parse_observe_args_accepts_split_and_equals_pool_seed_and_forwards_it() {
        for input in [
            os_args(&["runspec.json", "--pool-seed", "17"]),
            os_args(&["runspec.json", "--pool-seed=17"]),
        ] {
            let parsed = parse_observe_args(input).expect("observe arguments should parse");
            assert_eq!(
                parsed,
                ObserveArgs {
                    runspec: PathBuf::from("runspec.json"),
                    pool_seed: Some(OsString::from("17")),
                }
            );
            assert_eq!(
                observe_postretro_args(&parsed),
                os_args(&["--headless", "runspec.json", "--pool-seed", "17"]),
            );
        }
    }

    #[test]
    fn parse_observe_args_rejects_missing_or_duplicate_pool_seed() {
        assert!(parse_observe_args(os_args(&["runspec.json", "--pool-seed"])).is_err());
        assert!(
            parse_observe_args(os_args(&[
                "runspec.json",
                "--pool-seed=17",
                "--pool-seed",
                "18",
            ]))
            .is_err()
        );
    }

    #[test]
    fn parse_capture_args_accepts_exactly_one_scene_path() {
        assert_eq!(
            parse_capture_args(os_args(&["scene.json"])),
            Ok(PathBuf::from("scene.json"))
        );

        assert!(parse_capture_args(Vec::new()).is_err());
        assert!(parse_capture_args(os_args(&["a.json", "b.json"])).is_err());
    }

    #[test]
    fn parse_bake_model_textures_args_accepts_exactly_one_gltf_path() {
        assert_eq!(
            parse_bake_model_textures_args(os_args(&[
                "content/dev/models/reference_enemy_kaykit_knight/scene.gltf"
            ])),
            Ok(PathBuf::from(
                "content/dev/models/reference_enemy_kaykit_knight/scene.gltf"
            ))
        );

        assert!(parse_bake_model_textures_args(Vec::new()).is_err());
        assert!(parse_bake_model_textures_args(os_args(&["one.gltf", "two.gltf"])).is_err());
    }

    #[test]
    fn parse_read_muzzle_offset_args_accepts_only_one_viewmodel_path() {
        assert_eq!(
            parse_read_muzzle_offset_args(&os_args(&[
                "--read-muzzle-offset",
                "content/dev/models/ar_4/model.gltf",
            ])),
            Ok(Some(PathBuf::from("content/dev/models/ar_4/model.gltf"))),
        );
        assert_eq!(
            parse_read_muzzle_offset_args(&os_args(&["holder.gltf"])),
            Ok(None),
            "ordinary solver arguments must continue to select the mount path",
        );
        assert!(parse_read_muzzle_offset_args(&os_args(&["--read-muzzle-offset"])).is_err());
        assert!(
            parse_read_muzzle_offset_args(&os_args(&[
                "--read-muzzle-offset",
                "one.gltf",
                "two.gltf",
            ]))
            .is_err()
        );
    }

    #[test]
    fn workspace_relative_path_roots_relative_paths_at_workspace() {
        let workspace = Path::new("/workspace");
        assert_eq!(
            workspace_relative_path(
                Path::new("content/dev/models/reference_enemy_kaykit_knight/scene.gltf"),
                workspace
            ),
            PathBuf::from("/workspace/content/dev/models/reference_enemy_kaykit_knight/scene.gltf")
        );
        assert_eq!(
            workspace_relative_path(Path::new("/tmp/model/scene.gltf"), workspace),
            PathBuf::from("/tmp/model/scene.gltf")
        );
    }

    #[test]
    fn mint_identity_plans_sidecar_only_for_resolved_typescript_mod_root() {
        let invocation_dir = Path::new("/work/project");
        let typescript_entry = Path::new("/work/project/mods/ts/start-script.ts");

        let typescript = plan_mint_identity(Path::new("mods/ts"), invocation_dir, |path| {
            path == typescript_entry
        });
        assert_eq!(typescript.mod_root, Path::new("/work/project/mods/ts"));
        assert!(typescript.build_scripts_sidecar);

        for root in [Path::new("mods/luau"), Path::new("mods/js")] {
            let plan = plan_mint_identity(root, invocation_dir, |_| false);
            assert_eq!(plan.mod_root, invocation_dir.join(root));
            assert!(!plan.build_scripts_sidecar);
        }
    }

    #[test]
    fn mint_identity_keeps_absolute_mod_root_when_planning_command() {
        let plan = plan_mint_identity(
            Path::new("/installed/mod"),
            Path::new("/unrelated/invocation"),
            |_| false,
        );

        assert_eq!(plan.mod_root, Path::new("/installed/mod"));
        assert!(!plan.build_scripts_sidecar);
    }

    #[test]
    fn bake_model_textures_for_gltf_reports_baked_keys_and_paths() {
        let gltf_path = PathBuf::from("content/dev/models/fixture/scene.gltf");
        let prm_root = PathBuf::from("/workspace/baked/materials");
        let diffuse_a = PathBuf::from("content/dev/models/fixture/a.png");
        let diffuse_b = PathBuf::from("content/dev/models/fixture/b.png");

        let baked = bake_model_textures_for_gltf_with(
            &gltf_path,
            &prm_root,
            |path| {
                assert_eq!(path, gltf_path.as_path());
                Ok::<_, String>(vec![
                    diffuse_a.clone(),
                    diffuse_a.clone(),
                    diffuse_b.clone(),
                ])
            },
            |path, root| {
                assert_eq!(root, prm_root.as_path());
                let mut key = [0u8; 32];
                key[31] = if path == diffuse_a.as_path() { 1 } else { 2 };
                Ok::<_, String>(key)
            },
        )
        .expect("fake bake should succeed");

        assert_eq!(
            baked,
            vec![
                BakedModelTexture {
                    source_path: diffuse_a,
                    key_hex: format!("{}01", "0".repeat(62)),
                    prm_path: prm_root.join(format!("{}01.prm", "0".repeat(62))),
                },
                BakedModelTexture {
                    source_path: diffuse_b,
                    key_hex: format!("{}02", "0".repeat(62)),
                    prm_path: prm_root.join(format!("{}02.prm", "0".repeat(62))),
                },
            ]
        );
    }

    #[test]
    fn parse_solve_weapon_mount_args_defaults_and_preserves_bake_passthrough() {
        let parsed = parse_solve_weapon_mount_args(os_args(&[
            "content/dev/models/limitator/model.gltf",
            "--weapon",
            "content/dev/models/ar_4/model.gltf",
            "--barrel",
            "0",
            "1",
            "0",
            "--up",
            "0",
            "0",
            "1",
            "--raw-source",
            "raw/ar 4.glb",
            "--out",
            "content/dev/models/ar_4/model.gltf",
            "--grip",
            "0.0",
            "-0.05",
            "0.120",
            "--scale",
            "0.68",
            "--socket",
            "muzzle=BarrelTip",
            "--socket",
            "optic_rail=ScopeMount",
        ]))
        .expect("complete solve command should parse");

        assert_eq!(
            parsed.holder_path,
            PathBuf::from("content/dev/models/limitator/model.gltf")
        );
        assert_eq!(parsed.mount_joint, "hand_r");
        assert_eq!(parsed.clip, "idle_aiming");
        assert_eq!(parsed.time, 0.0);
        assert_eq!(
            parsed.weapon_path,
            PathBuf::from("content/dev/models/ar_4/model.gltf")
        );
        assert_eq!(parsed.barrel, Some(Vec3::Y));
        assert_eq!(parsed.up, Some(Vec3::Z));
        assert!(!parsed.check);
        assert_eq!(parsed.raw_source.as_deref(), Some("raw/ar 4.glb"));
        assert_eq!(
            parsed.out.as_deref(),
            Some("content/dev/models/ar_4/model.gltf")
        );
        assert_eq!(parsed.thresholds, MountCheckThresholds::default());
        assert_eq!(
            parsed.grip,
            Some(["0.0".into(), "-0.05".into(), "0.120".into()])
        );
        assert_eq!(parsed.scale.as_deref(), Some("0.68"));
        assert_eq!(
            parsed.sockets,
            ["muzzle=BarrelTip", "optic_rail=ScopeMount"]
        );
    }

    #[test]
    fn parse_solve_weapon_mount_args_rejects_incomplete_axes_and_missing_bake_endpoints() {
        let common = [
            "holder.gltf",
            "--weapon",
            "weapon.gltf",
            "--raw-source",
            "raw.glb",
            "--out",
            "out.gltf",
        ];
        let mut incomplete_axes = common.to_vec();
        incomplete_axes.extend(["--barrel", "0", "0", "1"]);
        assert!(parse_solve_weapon_mount_args(os_args(&incomplete_axes)).is_err());

        assert!(
            parse_solve_weapon_mount_args(os_args(&[
                "holder.gltf",
                "--weapon",
                "weapon.gltf",
                "--raw-source",
                "raw.glb",
            ]))
            .is_err()
        );
    }

    #[test]
    fn parse_solve_weapon_mount_rejects_non_positive_scale() {
        // Regression: zero scale was silently omitted from the emitted bake,
        // while negative scale could bake a reflected weapon frame.
        for scale in ["0", "-0.68"] {
            let error = parse_solve_weapon_mount_args(os_args(&[
                "holder.gltf",
                "--weapon",
                "weapon.gltf",
                "--raw-source",
                "raw.glb",
                "--out",
                "out.gltf",
                "--scale",
                scale,
            ]))
            .expect_err("mount workflow scale must be positive");
            assert!(error.contains("--scale must be greater than zero"));
        }
    }

    #[test]
    fn parse_solve_weapon_mount_check_mode_does_not_require_bake_endpoints() {
        let parsed = parse_solve_weapon_mount_args(os_args(&[
            "holder.gltf",
            "--weapon",
            "baked-weapon.gltf",
            "--check",
            "--min-barrel-dot",
            "0.95",
            "--max-barrel-y",
            "0.1",
            "--min-up-dot",
            "0.9",
        ]))
        .expect("check mode only needs the baked weapon");

        assert!(parsed.check);
        assert_eq!(parsed.raw_source, None);
        assert_eq!(parsed.out, None);
        assert_eq!(
            parsed.thresholds,
            MountCheckThresholds {
                min_barrel_dot: 0.95,
                max_barrel_y: 0.1,
                min_up_dot: 0.9,
            }
        );
    }

    #[test]
    fn parse_solve_weapon_mount_rejects_out_of_range_check_thresholds() {
        for args in [
            [
                "holder.gltf",
                "--weapon",
                "weapon.gltf",
                "--check",
                "--min-barrel-dot",
                "1.01",
            ]
            .as_slice(),
            [
                "holder.gltf",
                "--weapon",
                "weapon.gltf",
                "--check",
                "--max-barrel-y",
                "-0.01",
            ]
            .as_slice(),
            [
                "holder.gltf",
                "--weapon",
                "weapon.gltf",
                "--check",
                "--min-up-dot",
                "nan",
            ]
            .as_slice(),
        ] {
            assert!(parse_solve_weapon_mount_args(os_args(args)).is_err());
        }
    }

    #[test]
    fn declared_check_composes_applied_euler_forward_into_baked_frame() {
        let baked_axes = compose_declared_axes_into_baked_frame(
            MountAxes {
                barrel: Vec3::Y,
                up: Vec3::Z,
                euler: None,
            },
            [90.0, 0.0, 0.0],
        );

        assert_vec3_close(baked_axes.barrel, Vec3::Z);
        assert_vec3_close(baked_axes.up, -Vec3::Y);
    }

    #[test]
    fn current_bake_euler_cli_override_wins_for_assist_and_declared_check() {
        let persisted = MountAxes {
            barrel: Vec3::Z,
            up: Vec3::Y,
            euler: Some([10.0, 20.0, 30.0]),
        };
        assert_eq!(
            current_bake_euler(Some(persisted), Some([40.0, 50.0, 60.0])),
            Some([40.0, 50.0, 60.0])
        );
        assert_eq!(
            current_bake_euler(Some(persisted), None),
            Some([10.0, 20.0, 30.0])
        );
        assert_eq!(
            applied_check_euler(
                Some(MountAxes {
                    euler: None,
                    ..persisted
                }),
                Some([40.0, 50.0, 60.0]),
            ),
            Ok([40.0, 50.0, 60.0])
        );
        assert!(
            applied_check_euler(None, None)
                .expect_err("declared check needs an applied euler")
                .contains("missing the applied euler")
        );
    }

    #[test]
    fn mount_check_names_each_out_of_tolerance_metric() {
        let verification = MountVerification {
            barrel_world: Vec3::Z,
            up_world: Vec3::Y,
            barrel_dot_forward: 0.998,
            barrel_dot_up: -0.03,
            up_dot_up: 0.998,
        };

        assert_eq!(
            failed_mount_metrics(verification, MountCheckThresholds::default()),
            ["barrel·+Z", "|barrel·+Y|", "up·+Y"]
        );
    }

    #[test]
    fn mount_check_rejects_non_finite_metrics() {
        // Regression: NaN comparisons were all false, so invalid metrics could
        // be reported as a passing check.
        let verification = MountVerification {
            barrel_world: Vec3::splat(f32::NAN),
            up_world: Vec3::splat(f32::NAN),
            barrel_dot_forward: f32::NAN,
            barrel_dot_up: f32::NAN,
            up_dot_up: f32::NAN,
        };

        assert_eq!(
            failed_mount_metrics(verification, MountCheckThresholds::default()),
            ["barrel·+Z", "|barrel·+Y|", "up·+Y"]
        );
    }

    #[test]
    fn prop_writer_output_loads_and_checks_without_cli_axes_or_euler() {
        // Regression: the normal prop writer -> model loader -> declared check
        // seam must persist intent without a second hand-authored JSON shape.
        let weapon_path = write_weapon_fixture();
        postprocess_weapon_fixture_with_prop_writer(&weapon_path);
        let weapon = load_model(&weapon_path).expect("prop writer output loads");
        assert_eq!(
            weapon.mount,
            Some(MountAxes {
                barrel: Vec3::Z,
                up: Vec3::Y,
                euler: Some([0.0, 0.0, 0.0]),
            }),
            "the loader surfaces normalized metadata from the real writer",
        );
        let args = parse_solve_weapon_mount_args(vec![
            OsString::from("holder.gltf"),
            OsString::from("--weapon"),
            weapon_path.clone().into_os_string(),
            OsString::from("--check"),
        ])
        .expect("normal persisted check needs no CLI axes or euler");

        let result = check_weapon_mount(&args, &weapon, Mat4::IDENTITY);
        let _ = std::fs::remove_file(weapon_path);

        assert_eq!(result, Ok(0));
    }

    #[test]
    fn declared_check_rejects_degenerate_socket_before_reporting_pass() {
        let weapon = LoadedModel {
            mount: Some(MountAxes {
                barrel: Vec3::Z,
                up: Vec3::Y,
                euler: Some([0.0, 0.0, 0.0]),
            }),
            ..LoadedModel::default()
        };
        let args = parse_solve_weapon_mount_args(os_args(&[
            "holder.gltf",
            "--weapon",
            "weapon.gltf",
            "--check",
        ]))
        .expect("declared check arguments parse");

        let error = check_weapon_mount(&args, &weapon, Mat4::ZERO)
            .expect_err("degenerate sockets cannot produce passing metrics");
        assert!(error.contains("rotation columns must be finite and non-zero"));
    }

    #[test]
    fn declared_check_surfaces_reflected_socket_as_a_model_error() {
        // Regression: xtask treated a reflected socket basis as a trusted
        // declared check instead of propagating the model-layer refusal.
        let weapon = LoadedModel {
            mount: Some(MountAxes {
                barrel: Vec3::Z,
                up: Vec3::Y,
                euler: Some([0.0, 0.0, 0.0]),
            }),
            ..LoadedModel::default()
        };
        let args = parse_solve_weapon_mount_args(os_args(&[
            "holder.gltf",
            "--weapon",
            "weapon.gltf",
            "--check",
        ]))
        .expect("declared check arguments parse");

        let error = check_weapon_mount(&args, &weapon, Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)))
            .expect_err("reflected sockets cannot produce trusted checks");
        assert!(error.contains("determinant must be positive"));
    }

    #[test]
    fn reference_mount_pose_requires_idle_aiming_at_zero() {
        assert!(is_reference_mount_pose("idle_aiming", 0.0));
        assert!(!is_reference_mount_pose("reloading", 0.0));
        assert!(!is_reference_mount_pose("idle_aiming", 0.1));
    }

    #[test]
    fn emitted_blender_command_carries_complete_bake_and_mount_metadata_arguments() {
        let args = parse_solve_weapon_mount_args(os_args(&[
            "holder.gltf",
            "--weapon",
            "weapon.gltf",
            "--raw-source",
            "raw author's asset.glb",
            "--out",
            "out author's asset.gltf",
            "--grip",
            "0.0",
            "-0.05",
            "0.120",
            "--scale",
            "0.68",
            "--socket",
            "muzzle's tip=BarrelTip",
        ]))
        .expect("complete solve command should parse");
        let command = emitted_blender_command(
            &args,
            [10.0, 20.0, 30.0],
            Some(MountAxes {
                barrel: Vec3::Y,
                up: Vec3::Z,
                euler: None,
            }),
        );

        assert_eq!(
            command,
            concat!(
                "blender --background --python tools/prop_to_gltf.py -- --input 'raw author'\"'\"'s asset.glb' ",
                "--output 'out author'\"'\"'s asset.gltf' --grip 0.0 -0.05 0.120 --scale 0.68 ",
                "--socket 'muzzle'\"'\"'s tip=BarrelTip' --rotate-euler 10.000000 20.000000 30.000000 ",
                "--mount-axes 0.000000 1.000000 0.000000 0.000000 0.000000 1.000000"
            )
        );
    }

    #[test]
    fn geometric_assist_command_remains_unverified_after_rebake() {
        let args = parse_solve_weapon_mount_args(os_args(&[
            "holder.gltf",
            "--weapon",
            "weapon.gltf",
            "--raw-source",
            "raw.glb",
            "--out",
            "weapon.gltf",
            "--current-euler",
            "10",
            "20",
            "30",
        ]))
        .expect("geometric assist arguments parse");

        let command = emitted_blender_command(&args, [40.0, 50.0, 60.0], None);

        assert!(command.contains("--rotate-euler 40.000000 50.000000 60.000000"));
        assert!(
            !command.contains("--mount-axes"),
            "assist-derived axes must not become a persisted declaration: {command}",
        );
    }

    #[test]
    fn blender_euler_decomposition_round_trips_similarity_rotation_including_gimbal_lock() {
        for euler in [
            [20.0, -30.0, 45.0],
            [35.0, 90.0, -15.0],
            [-20.0, -90.0, 70.0],
        ] {
            let gltf_rotation = blender_to_gltf_rotation(blender_xyz_rotation(euler));
            let blender_rotation = gltf_to_blender_rotation(gltf_rotation);
            let decomposed = blender_xyz_euler_degrees(blender_rotation);
            assert_mat3_close(blender_xyz_rotation(decomposed), blender_rotation);
        }
    }

    #[test]
    fn gltf_to_blender_similarity_maps_axes_and_rotation_operators_on_both_sides() {
        let basis = gltf_to_blender_basis();
        assert_eq!(basis * Vec3::X, Vec3::X);
        assert_eq!(basis * Vec3::Y, Vec3::Z);
        assert_eq!(basis * Vec3::Z, -Vec3::Y);

        let gltf_rotation = Mat3::from_rotation_y(std::f32::consts::FRAC_PI_2);
        assert_mat3_close(
            gltf_to_blender_rotation(gltf_rotation),
            Mat3::from_rotation_z(std::f32::consts::FRAC_PI_2),
        );
    }

    fn assert_mat3_close(actual: Mat3, expected: Mat3) {
        const EPSILON: f32 = 1.0e-5;
        for (actual, expected) in actual
            .to_cols_array()
            .into_iter()
            .zip(expected.to_cols_array())
        {
            assert!(
                (actual - expected).abs() <= EPSILON,
                "expected {expected}, got {actual}",
            );
        }
    }

    fn assert_vec3_close(actual: Vec3, expected: Vec3) {
        const EPSILON: f32 = 1.0e-5;
        assert!(
            actual.abs_diff_eq(expected, EPSILON),
            "expected {expected:?}, got {actual:?}",
        );
    }
}
