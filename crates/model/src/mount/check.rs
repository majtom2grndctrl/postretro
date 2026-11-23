// The declared-axes mount check: compose, verify, judge.
// See: context/lib/resource_management.md §7

//! The VERIFIED half of `solve-weapon-mount --check`, print-free.
//!
//! A bake persists its raw-source barrel/up axes and the Blender XYZ Euler its
//! bake applied (`extras.mount`). The check rotates those axes through that
//! Euler into the baked frame, mounts the frame at the holder's sampled socket,
//! and judges the result against [`MountThresholds`]. Only the Blender basis
//! change and Euler convention the composition needs live here; Euler
//! decomposition and bake-command emission stay in `postretro-tool`.

use glam::{Mat3, Mat4, Vec3};
use thiserror::Error;

use super::{
    MountAxes, MountSolveError, MountVerification, resolve_socket_frame_in_model, verify_mount,
};
use crate::gltf_loader::LoadedModel;

/// The holder clip whose pose `solve-weapon-mount` solves and checks against
/// unless told otherwise.
pub const REFERENCE_MOUNT_CLIP: &str = "idle_aiming";

/// The clip time of the reference mount pose, in seconds.
pub const REFERENCE_MOUNT_TIME: f32 = 0.0;

/// Pass bounds for a mounted weapon's engine-frame orientation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MountThresholds {
    /// Lowest accepted `barrel·+Z`.
    pub min_barrel_dot: f32,
    /// Highest accepted `|barrel·+Y|`.
    pub max_barrel_y: f32,
    /// Lowest accepted `up·+Y`.
    pub min_up_dot: f32,
}

impl Default for MountThresholds {
    fn default() -> Self {
        Self {
            min_barrel_dot: 0.999,
            max_barrel_y: 0.02,
            min_up_dot: 0.999,
        }
    }
}

/// A declared bake's check result.
#[derive(Debug, Clone, PartialEq)]
pub struct DeclaredMountCheck {
    /// The declared raw-source axes, composed through the applied Euler.
    pub baked_axes: MountAxes,
    pub verification: MountVerification,
    /// The label of each out-of-tolerance metric; empty when the check passes.
    pub failures: Vec<&'static str>,
}

impl DeclaredMountCheck {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Failures that leave a declared check without a verdict.
#[derive(Debug, Error)]
pub enum DeclaredMountCheckError {
    #[error("resolve holder socket: {0}")]
    Socket(Box<MountSolveError>),
    #[error("weapon declares no extras.mount barrel/up axes")]
    UndeclaredAxes,
    #[error("weapon's extras.mount is missing the applied euler")]
    MissingAppliedEuler,
    /// The composed axes do not form a frame.
    #[error("compose declared weapon axes: {0}")]
    Compose(Box<MountSolveError>),
    /// The socket frame refused the composed frame. Carries the composed axes,
    /// which a caller may still report.
    #[error("verify declared weapon mount: {source}")]
    Verify {
        baked_axes: MountAxes,
        source: Box<MountSolveError>,
    },
}

/// Check `weapon`'s persisted `extras.mount` against `holder`'s `socket` at
/// `clip`/`time`: the same declared-axes check `solve-weapon-mount --check`
/// runs on persisted axes.
///
/// Refuses a weapon without declared axes or without its applied Euler rather
/// than falling back to geometric detection, which can never verify.
pub fn check_declared_weapon_mount(
    holder: &LoadedModel,
    weapon: &LoadedModel,
    clip: &str,
    socket: &str,
    time: f32,
    thresholds: MountThresholds,
) -> Result<DeclaredMountCheck, DeclaredMountCheckError> {
    let declared_axes = weapon
        .mount
        .ok_or(DeclaredMountCheckError::UndeclaredAxes)?;
    let applied_euler = declared_axes
        .euler
        .ok_or(DeclaredMountCheckError::MissingAppliedEuler)?;
    let socket = resolve_socket_frame_in_model(holder, clip, socket, time)
        .map_err(|error| DeclaredMountCheckError::Socket(Box::new(error)))?;
    check_declared_mount(socket.matrix, declared_axes, applied_euler, thresholds)
}

/// Compose `declared_axes` through `applied_blender_euler`, verify the baked
/// frame at `socket_matrix`, and judge it against `thresholds`.
///
/// Only [`DeclaredMountCheckError::Compose`] and
/// [`DeclaredMountCheckError::Verify`] arise here.
pub fn check_declared_mount(
    socket_matrix: Mat4,
    declared_axes: MountAxes,
    applied_blender_euler: [f32; 3],
    thresholds: MountThresholds,
) -> Result<DeclaredMountCheck, DeclaredMountCheckError> {
    let baked_axes = compose_declared_axes_into_baked_frame(declared_axes, applied_blender_euler);
    let baked_frame = baked_axes
        .frame()
        .map_err(|error| DeclaredMountCheckError::Compose(Box::new(error)))?;
    let verification = verify_mount(socket_matrix, baked_frame).map_err(|source| {
        DeclaredMountCheckError::Verify {
            baked_axes,
            source: Box::new(source),
        }
    })?;
    Ok(DeclaredMountCheck {
        baked_axes,
        verification,
        failures: failed_mount_metrics(verification, thresholds),
    })
}

/// The label of each metric outside `thresholds`, in report order. A
/// non-finite metric always fails.
pub fn failed_mount_metrics(
    verification: MountVerification,
    thresholds: MountThresholds,
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

/// Compose raw-source declared axes through the rotation baked into the weapon.
pub fn compose_declared_axes_into_baked_frame(
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

/// glTF-to-Blender basis change for rotation operators. The columns map
/// `(x, y, z)` to `(x, -z, y)`, so a corrective rotation needs a two-sided
/// similarity transform rather than a one-sided vector conversion.
pub fn gltf_to_blender_rotation(rotation: Mat3) -> Mat3 {
    let map = gltf_to_blender_basis();
    map * rotation * map.transpose()
}

/// The inverse of [`gltf_to_blender_rotation`].
pub fn blender_to_gltf_rotation(rotation: Mat3) -> Mat3 {
    let map = gltf_to_blender_basis();
    map.transpose() * rotation * map
}

fn gltf_to_blender_basis() -> Mat3 {
    Mat3::from_cols(Vec3::X, Vec3::Z, -Vec3::Y)
}

/// Blender's `XYZ` Euler mode applies `Rz * Ry * Rx`.
pub fn blender_xyz_rotation(euler_degrees: [f32; 3]) -> Mat3 {
    Mat3::from_rotation_z(euler_degrees[2].to_radians())
        * Mat3::from_rotation_y(euler_degrees[1].to_radians())
        * Mat3::from_rotation_x(euler_degrees[0].to_radians())
}

#[cfg(test)]
mod tests {
    use glam::{Mat3, Mat4, Vec3};

    use super::*;

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
        assert_mat3_close(
            blender_to_gltf_rotation(gltf_to_blender_rotation(gltf_rotation)),
            gltf_rotation,
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
            failed_mount_metrics(verification, MountThresholds::default()),
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
            failed_mount_metrics(verification, MountThresholds::default()),
            ["barrel·+Z", "|barrel·+Y|", "up·+Y"]
        );
    }

    #[test]
    fn declared_mount_check_passes_only_when_the_composed_frame_matches_the_socket() {
        let check = |up: Vec3| {
            check_declared_mount(
                Mat4::IDENTITY,
                MountAxes {
                    barrel: Vec3::Y,
                    up,
                    euler: None,
                },
                [90.0, 0.0, 0.0],
                MountThresholds::default(),
            )
            .expect("an identity socket verifies")
        };

        // The applied X turn carries raw +Y to +Z and raw -Z to +Y.
        let seated = check(-Vec3::Z);
        assert_vec3_close(seated.baked_axes.barrel, Vec3::Z);
        assert_vec3_close(seated.baked_axes.up, Vec3::Y);
        assert!(seated.passed(), "failures: {:?}", seated.failures);

        let upside_down = check(Vec3::Z);
        assert_eq!(upside_down.failures, ["up·+Y"]);
        assert!(!upside_down.passed());
    }

    #[test]
    fn declared_mount_check_keeps_the_composed_axes_when_the_socket_refuses_them() {
        let error = check_declared_mount(
            Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)),
            MountAxes {
                barrel: Vec3::Z,
                up: Vec3::Y,
                euler: None,
            },
            [0.0, 0.0, 0.0],
            MountThresholds::default(),
        )
        .expect_err("a reflected socket cannot verify");

        match error {
            DeclaredMountCheckError::Verify { baked_axes, source } => {
                assert_vec3_close(baked_axes.barrel, Vec3::Z);
                assert!(source.to_string().contains("determinant must be positive"));
            }
            other => panic!("expected a verify error, got {other:?}"),
        }
    }

    #[test]
    fn declared_weapon_mount_check_refuses_a_weapon_without_its_applied_euler() {
        let holder = LoadedModel::default();
        let undeclared = LoadedModel::default();
        assert!(matches!(
            check_declared_weapon_mount(
                &holder,
                &undeclared,
                REFERENCE_MOUNT_CLIP,
                "hand_r",
                REFERENCE_MOUNT_TIME,
                MountThresholds::default(),
            ),
            Err(DeclaredMountCheckError::UndeclaredAxes)
        ));

        let without_euler = LoadedModel {
            mount: Some(MountAxes {
                barrel: Vec3::Z,
                up: Vec3::Y,
                euler: None,
            }),
            ..LoadedModel::default()
        };
        assert!(matches!(
            check_declared_weapon_mount(
                &holder,
                &without_euler,
                REFERENCE_MOUNT_CLIP,
                "hand_r",
                REFERENCE_MOUNT_TIME,
                MountThresholds::default(),
            ),
            Err(DeclaredMountCheckError::MissingAppliedEuler)
        ));
    }
}
