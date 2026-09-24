//! `solve-weapon-mount`: solve a rigid weapon bake against the engine's own
//! sampled socket frame, or check a baked one.
//!
//! Two modes, one rule: a result is VERIFIED only when the weapon declares its
//! raw-source barrel/up axes. Geometric detection is an assist — it measures the
//! already-baked mesh, so it can describe what is there but never certify what
//! was intended, and its derived axes are deliberately never persisted.
//!
//! See: context/lib/resource_management.md §7

use std::ffi::OsString;
use std::path::Path;

use glam::{Mat4, Vec3};
use postretro_model::gltf_loader::{LoadedModel, load_model};
use postretro_model::mount::{
    MountAxes, MountConfidence, MountDetection, MountVerification, corrective_delta,
    corrective_delta_for_axes, detect_weapon_mount, read_muzzle_offset_in_model,
    resolve_socket_frame_in_model, verify_mount,
};

mod args;
mod blender;

use args::{
    MountCheckThresholds, SolveWeaponMountArgs, parse_read_muzzle_offset_args,
    parse_solve_weapon_mount_args, usage,
};
use blender::{
    blender_to_gltf_rotation, blender_xyz_euler_degrees, blender_xyz_rotation,
    compose_declared_axes_into_baked_frame, emitted_blender_command, gltf_to_blender_rotation,
};

pub(crate) fn run(args: Vec<OsString>) -> Result<i32, String> {
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
                    usage(
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
    println!("Run this command (emit-only; the tool does not invoke Blender):");
    println!("{}", emitted_blender_command(&args, euler, emitted_axes));
    Ok(0)
}

/// Print the author-time, model-local muzzle point from a rigid viewmodel socket.
///
/// This deliberately stays separate from the skinned holder-joint solver: a
/// viewmodel muzzle is a composed rest translation in mesh-node-local space, not
/// an animated skinned socket frame.
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
        usage("declared check is missing the applied euler; add extras.mount.euler or --current-euler X Y Z")
    })
}

fn current_bake_euler(
    persisted_mount: Option<MountAxes>,
    cli_euler: Option<[f32; 3]>,
) -> Option<[f32; 3]> {
    cli_euler.or_else(|| persisted_mount.and_then(|mount| mount.euler))
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

fn print_mount_metrics(verification: MountVerification) {
    println!(
        "barrel·+Z: {}",
        format_number(verification.barrel_dot_forward),
    );
    println!("barrel·+Y: {}", format_number(verification.barrel_dot_up));
    println!("up·+Y: {}", format_number(verification.up_dot_up));
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
    if detection.confidence == MountConfidence::Low {
        println!(
            "WARNING: geometric assist detection is LOW confidence; its result remains UNVERIFIED."
        );
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    fn os_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    /// The crate root, read at run time. `env!` would bake a build-machine path
    /// into the shipped binary — the exact thing `postretro-tool` exists to
    /// avoid — so even a test reaches for the runtime read cargo already sets.
    fn crate_root() -> PathBuf {
        PathBuf::from(
            std::env::var("CARGO_MANIFEST_DIR")
                .expect("cargo sets CARGO_MANIFEST_DIR for its own test runs"),
        )
    }

    fn write_weapon_fixture() -> PathBuf {
        let source =
            crate_root().join("../model/tests/fixtures/multi_primitive/multi_primitive.gltf");
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time follows Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "postretro_tool_mount_{}_{}.gltf",
            std::process::id(),
            unique,
        ));
        std::fs::copy(source, &path).expect("model fixture copies");
        path
    }

    /// The prop writer is Python. When no interpreter is installed the failure
    /// must say so: Windows ships a `python3` alias stub on PATH whose "Python
    /// was not found" output reads like a broken PATH rather than a missing
    /// install, and chasing that costs an afternoon.
    fn python_interpreter() -> Command {
        let probe = Command::new("python3").arg("--version").output();
        let version = match probe {
            Ok(output) if output.status.success() => output,
            Ok(output) => panic!(
                "`python3` is on PATH but does not run (exit {}). On Windows this is usually the \
                 Microsoft Store alias stub shadowing a real install. Install Python — for example \
                 `uv python install --default` — so a bare `python3` resolves.\nstdout:\n{}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            ),
            Err(error) => panic!(
                "no `python3` interpreter found on PATH ({error}). This test shells the real \
                 Python prop writer on purpose — install Python, for example with \
                 `uv python install --default`."
            ),
        };
        let _ = version;
        Command::new("python3")
    }

    fn postprocess_weapon_fixture_with_prop_writer(path: &Path) {
        let converter = crate_root().join("../../tools/prop_to_gltf.py");
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
        let output = python_interpreter()
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
        // Regression: the solver treated a reflected socket basis as a trusted
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
}
