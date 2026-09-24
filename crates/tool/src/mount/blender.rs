//! The glTF↔Blender basis change and the bake command the solver emits.
//!
//! The tool never invokes Blender. It solves the corrective rotation against the
//! engine's sampled socket frame and prints the command that performs the bake,
//! so the author keeps the bake in their own hands and the exact arguments stay
//! reviewable.
//!
//! See: context/lib/resource_management.md §7

use glam::{Mat3, Vec3};
use postretro_model::mount::MountAxes;

use super::args::SolveWeaponMountArgs;
use super::format_number;

/// glTF-to-Blender basis change for rotation operators. The columns map
/// `(x, y, z)` to `(x, -z, y)`, so a corrective rotation needs a two-sided
/// similarity transform rather than a one-sided vector conversion.
pub(super) fn gltf_to_blender_rotation(rotation: Mat3) -> Mat3 {
    let map = gltf_to_blender_basis();
    map * rotation * map.transpose()
}

pub(super) fn blender_to_gltf_rotation(rotation: Mat3) -> Mat3 {
    let map = gltf_to_blender_basis();
    map.transpose() * rotation * map
}

fn gltf_to_blender_basis() -> Mat3 {
    Mat3::from_cols(Vec3::X, Vec3::Z, -Vec3::Y)
}

/// Blender's `XYZ` Euler mode applies `Rz * Ry * Rx`.
pub(super) fn blender_xyz_rotation(euler_degrees: [f32; 3]) -> Mat3 {
    Mat3::from_rotation_z(euler_degrees[2].to_radians())
        * Mat3::from_rotation_y(euler_degrees[1].to_radians())
        * Mat3::from_rotation_x(euler_degrees[0].to_radians())
}

pub(super) fn blender_xyz_euler_degrees(rotation: Mat3) -> [f32; 3] {
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

/// Compose raw-source declared axes through the rotation baked into the weapon.
pub(super) fn compose_declared_axes_into_baked_frame(
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

pub(super) fn emitted_blender_command(
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

#[cfg(test)]
mod tests {
    use super::super::args::parse_solve_weapon_mount_args;
    use super::*;
    use std::ffi::OsString;

    fn os_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
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
