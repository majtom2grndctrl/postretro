// Engine-frame weapon mount solving and verification.
// See: context/lib/resource_management.md §7

//! Design-time mount math in the raw glTF frame.
//!
//! Blender conversion deliberately stays outside this module. The socket resolver
//! uses the regular model loader and neutral pose sampler so its reference-pose
//! frame is the same frame used by runtime attachments.

use std::path::Path;

use glam::{Mat3, Mat4, Vec3};
use thiserror::Error;

use crate::anim::{Loop, sample_clip_looped_world_modified};
use crate::gltf_loader::{LoadedModel, ModelLoadError, SocketBinding, load_model};

const MIN_DIRECTION_LENGTH_SQUARED: f32 = 1.0e-12;
const SOCKET_ORTHONORMAL_EPSILON: f32 = 1.0e-4;
const LOW_CONFIDENCE_RADIUS_RATIO: f32 = 1.5;
const LOW_CONFIDENCE_ELONGATION: f32 = 2.0;

/// Author-declared weapon axes in raw-source weapon-local glTF space.
///
/// `euler` records a Blender-tool value for the caller's benefit. The model
/// crate neither interprets nor converts it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MountAxes {
    pub barrel: Vec3,
    pub up: Vec3,
    pub euler: Option<[f32; 3]>,
}

impl MountAxes {
    /// Build the right-handed local mount frame `[side, up, barrel]`.
    pub fn frame(self) -> Result<MountFrame, MountSolveError> {
        MountFrame::from_axes(self.barrel, self.up)
    }
}

/// A right-handed weapon-local orientation with columns `[side, up, barrel]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MountFrame {
    pub side: Vec3,
    pub up: Vec3,
    pub barrel: Vec3,
}

impl MountFrame {
    /// Validate author-declared axes and construct their orthonormal frame.
    pub fn from_axes(barrel: Vec3, up: Vec3) -> Result<Self, MountSolveError> {
        let barrel = normalized_direction(barrel, "barrel axis")?;
        let up = normalized_direction(up, "up axis")?;
        if barrel.dot(up).abs() > 1.0e-3 {
            return Err(MountSolveError::NonOrthogonalAxes);
        }

        let side = normalized_direction(up.cross(barrel), "side axis")?;
        Ok(Self { side, up, barrel })
    }
}

/// The reproducible confidence signal for geometric weapon-axis detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountConfidence {
    High,
    Low,
}

/// One refined end of the detected long axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MountEnd {
    pub centroid: Vec3,
    pub max_cross_radius: f32,
}

/// Geometric weapon-axis detection over a loaded mesh's local vertices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MountDetection {
    pub frame: MountFrame,
    pub bbox_min: Vec3,
    pub bbox_max: Vec3,
    /// End A is the `t max` end of the initial long-axis orientation.
    pub end_a: MountEnd,
    /// End B is the `t min` end of the initial long-axis orientation.
    pub end_b: MountEnd,
    pub muzzle: MountEnd,
    pub stock: MountEnd,
    pub length: f32,
    pub confidence: MountConfidence,
}

/// A named skinned socket resolved to the engine's world-joint frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedSocketFrame {
    pub joint_index: usize,
    pub matrix: Mat4,
}

/// Engine-frame mount verification values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MountVerification {
    pub barrel_world: Vec3,
    pub up_world: Vec3,
    pub barrel_dot_forward: f32,
    pub barrel_dot_up: f32,
    pub up_dot_up: f32,
}

/// Failures that prevent a design-time mount solve.
#[derive(Debug, Error)]
pub enum MountSolveError {
    #[error("model load failed: {0}")]
    ModelLoad(#[from] ModelLoadError),
    #[error("socket {socket:?} was not found")]
    MissingSocket { socket: String },
    #[error("socket {socket:?} is not a skinned joint")]
    NonSkinnedSocket { socket: String },
    #[error("weapon viewmodel has no rigid \"muzzle\" socket")]
    MissingMuzzleSocket,
    #[error(
        "weapon viewmodel socket \"muzzle\" is a skinned joint; muzzleOffset must use a rigid socket on the viewmodel"
    )]
    SkinnedMuzzleSocket,
    #[error("clip {clip:?} was not found")]
    MissingClip { clip: String },
    #[error("socket joint {joint} is outside the sampled world pose")]
    MissingJointWorldPose { joint: usize },
    #[error("weapon geometry needs at least two distinct finite vertices")]
    InvalidWeaponGeometry,
    #[error("{axis} must be a finite non-zero direction")]
    InvalidDirection { axis: &'static str },
    #[error("barrel and up axes must be orthogonal")]
    NonOrthogonalAxes,
    #[error("socket frame rotation columns must be finite and non-zero")]
    DegenerateSocketFrame,
    #[error("socket frame is not a proper rigid rotation: {reason}")]
    NonRigidSocketFrame { reason: &'static str },
    #[error("socket frame transforms the {axis} to a non-finite or zero direction")]
    InvalidMountedDirection { axis: &'static str },
}

/// Detect a weapon's local barrel/up/side frame from its loaded mesh vertices.
///
/// The thin end of the refined long axis is treated as the muzzle. The opposite
/// of the mean lateral mass offset from its bore supplies up, yielding the
/// right-handed frame `[side, up, barrel]`.
pub fn detect_weapon_mount(model: &LoadedModel) -> Result<MountDetection, MountSolveError> {
    let vertices: Vec<Vec3> = model
        .mesh
        .vertices
        .iter()
        .map(|vertex| Vec3::from_array(vertex.position))
        .collect();
    detect_weapon_mount_vertices(&vertices)
}

/// Load a holder and resolve the named skinned socket at the requested clip time.
///
/// This deliberately goes through [`load_model`] and the neutral modified-pose
/// sampler used by runtime attachment presentation. The fixed `Loop::Clamp` and
/// `None` pose inputs define the design-time reference-pose contract.
pub fn resolve_socket_frame(
    holder_path: &Path,
    clip_name: &str,
    socket_name: &str,
    time: f32,
) -> Result<ResolvedSocketFrame, MountSolveError> {
    let model = load_model(holder_path)?;
    resolve_socket_frame_in_model(&model, clip_name, socket_name, time)
}

/// Resolve a named skinned socket from an already-loaded holder model.
pub fn resolve_socket_frame_in_model(
    model: &LoadedModel,
    clip_name: &str,
    socket_name: &str,
    time: f32,
) -> Result<ResolvedSocketFrame, MountSolveError> {
    let joint_index = match model.sockets.get(socket_name) {
        Some(SocketBinding::SkinnedJoint(joint)) => *joint,
        Some(SocketBinding::RigidRest(_)) => {
            return Err(MountSolveError::NonSkinnedSocket {
                socket: socket_name.to_string(),
            });
        }
        None => {
            return Err(MountSolveError::MissingSocket {
                socket: socket_name.to_string(),
            });
        }
    };
    let clip = model
        .clips
        .iter()
        .find(|clip| clip.name == clip_name)
        .ok_or_else(|| MountSolveError::MissingClip {
            clip: clip_name.to_string(),
        })?;

    let mut world_pose = Vec::new();
    sample_clip_looped_world_modified(
        clip,
        &model.skeleton,
        time,
        Loop::Clamp,
        &model.pose_stack,
        None,
        &mut world_pose,
    );
    let matrix = *world_pose
        .get(joint_index)
        .ok_or(MountSolveError::MissingJointWorldPose { joint: joint_index })?;
    Ok(ResolvedSocketFrame {
        joint_index,
        matrix,
    })
}

/// Read the authoring-only model-local muzzle point from a rigid viewmodel socket.
///
/// Unlike [`resolve_socket_frame_in_model`], this never samples a skeleton or
/// converts between coordinate systems. The rigid rest translation and mesh
/// vertices are both expressed in the viewmodel mesh node's raw glTF frame.
pub fn read_muzzle_offset_in_model(model: &LoadedModel) -> Result<Vec3, MountSolveError> {
    match model.sockets.get("muzzle") {
        Some(SocketBinding::RigidRest(transform)) => Ok(transform.w_axis.truncate()),
        Some(SocketBinding::SkinnedJoint(_)) => Err(MountSolveError::SkinnedMuzzleSocket),
        None => Err(MountSolveError::MissingMuzzleSocket),
    }
}

/// Compute the glTF-space corrective rotation `D = S^T * G^T`.
///
/// `S` is the proper rotation from the socket matrix's direction axes and `G`
/// is the weapon frame `[side, up, barrel]`. This module intentionally has no
/// Euler conversion because that belongs to the authoring-tool adapter.
pub fn corrective_delta(
    socket_frame: Mat4,
    weapon_frame: MountFrame,
) -> Result<Mat3, MountSolveError> {
    let socket_rotation = normalized_socket_rotation(socket_frame)?;
    let weapon_rotation = Mat3::from_cols(weapon_frame.side, weapon_frame.up, weapon_frame.barrel);
    Ok(socket_rotation.transpose() * weapon_rotation.transpose())
}

/// Compute a corrective delta directly from declared barrel/up axes.
pub fn corrective_delta_for_axes(
    socket_frame: Mat4,
    axes: MountAxes,
) -> Result<Mat3, MountSolveError> {
    corrective_delta(socket_frame, axes.frame()?)
}

/// Verify a weapon frame mounted at a socket frame.
///
/// After validating the socket's direction axes as a proper rotation,
/// verification intentionally uses the raw `Mat3::from_mat4(socket_frame)`, not
/// the normalized rotation used by [`corrective_delta`], preserving the legacy
/// diagnostic's reported values and any positive scale they carry.
pub fn verify_mount(
    socket_frame: Mat4,
    weapon_frame: MountFrame,
) -> Result<MountVerification, MountSolveError> {
    normalized_socket_rotation(socket_frame)?;
    let socket_rotation = Mat3::from_mat4(socket_frame);
    let barrel_world =
        normalized_direction(socket_rotation * weapon_frame.barrel, "mounted barrel axis")
            .map_err(|_| MountSolveError::InvalidMountedDirection { axis: "barrel" })?;
    let up_world = normalized_direction(socket_rotation * weapon_frame.up, "mounted up axis")
        .map_err(|_| MountSolveError::InvalidMountedDirection { axis: "up" })?;
    Ok(MountVerification {
        barrel_world,
        up_world,
        barrel_dot_forward: barrel_world.dot(Vec3::Z),
        barrel_dot_up: barrel_world.dot(Vec3::Y),
        up_dot_up: up_world.dot(Vec3::Y),
    })
}

/// Verify declared barrel/up axes without geometric detection.
pub fn verify_mount_axes(
    socket_frame: Mat4,
    axes: MountAxes,
) -> Result<MountVerification, MountSolveError> {
    verify_mount(socket_frame, axes.frame()?)
}

fn detect_weapon_mount_vertices(vertices: &[Vec3]) -> Result<MountDetection, MountSolveError> {
    if vertices.len() < 2 || vertices.iter().any(|vertex| !vertex.is_finite()) {
        return Err(MountSolveError::InvalidWeaponGeometry);
    }

    let mut bbox_min = Vec3::splat(f32::INFINITY);
    let mut bbox_max = Vec3::splat(f32::NEG_INFINITY);
    for vertex in vertices {
        bbox_min = bbox_min.min(*vertex);
        bbox_max = bbox_max.max(*vertex);
    }

    // The initial extreme pair is deliberately subsampled, matching the prior
    // diagnostic's work bound before the full-vertex centroid refinements.
    let stride = (vertices.len() / 3000).max(1);
    let sample: Vec<Vec3> = vertices.iter().step_by(stride).copied().collect();
    let (mut point_a, mut point_b, mut best_distance_squared) = (Vec3::ZERO, Vec3::ZERO, -1.0f32);
    for (i, a) in sample.iter().enumerate() {
        for b in sample.iter().skip(i + 1) {
            let distance_squared = a.distance_squared(*b);
            if distance_squared > best_distance_squared {
                best_distance_squared = distance_squared;
                point_a = *a;
                point_b = *b;
            }
        }
    }
    let mut axis = normalized_direction(point_a - point_b, "long axis")?;
    let (mut centroid_a, mut centroid_b) = (Vec3::ZERO, Vec3::ZERO);
    for _ in 0..3 {
        let projections: Vec<f32> = vertices.iter().map(|vertex| vertex.dot(axis)).collect();
        let projection_min = projections.iter().copied().fold(f32::INFINITY, f32::min);
        let projection_max = projections
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        let length = projection_max - projection_min;
        if !length.is_finite() || length <= 0.0 {
            return Err(MountSolveError::InvalidWeaponGeometry);
        }

        let (mut sum_a, mut count_a, mut sum_b, mut count_b) =
            (Vec3::ZERO, 0.0f32, Vec3::ZERO, 0.0f32);
        for (vertex, projection) in vertices.iter().zip(&projections) {
            if *projection > projection_max - 0.10 * length {
                sum_a += *vertex;
                count_a += 1.0;
            }
            if *projection < projection_min + 0.10 * length {
                sum_b += *vertex;
                count_b += 1.0;
            }
        }
        if count_a == 0.0 || count_b == 0.0 {
            return Err(MountSolveError::InvalidWeaponGeometry);
        }
        centroid_a = sum_a / count_a;
        centroid_b = sum_b / count_b;
        axis = normalized_direction(centroid_a - centroid_b, "long axis")?;
    }

    let center = vertices.iter().copied().sum::<Vec3>() / vertices.len() as f32;
    let projections: Vec<f32> = vertices.iter().map(|vertex| vertex.dot(axis)).collect();
    let projection_min = projections.iter().copied().fold(f32::INFINITY, f32::min);
    let projection_max = projections
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    let length = projection_max - projection_min;
    if !length.is_finite() || length <= 0.0 {
        return Err(MountSolveError::InvalidWeaponGeometry);
    }
    let radial_distance = |vertex: Vec3| {
        let from_center = vertex - center;
        (from_center - from_center.dot(axis) * axis).length()
    };
    let (mut radius_a, mut radius_b) = (0.0f32, 0.0f32);
    for (vertex, projection) in vertices.iter().zip(&projections) {
        if *projection > projection_max - 0.15 * length {
            radius_a = radius_a.max(radial_distance(*vertex));
        }
        if *projection < projection_min + 0.15 * length {
            radius_b = radius_b.max(radial_distance(*vertex));
        }
    }

    let end_a = MountEnd {
        centroid: centroid_a,
        max_cross_radius: radius_a,
    };
    let end_b = MountEnd {
        centroid: centroid_b,
        max_cross_radius: radius_b,
    };
    let (barrel, muzzle, stock) = if radius_a < radius_b {
        (axis, end_a, end_b)
    } else {
        (-axis, end_b, end_a)
    };
    let mut mean_lateral_offset = Vec3::ZERO;
    for vertex in vertices {
        let from_muzzle = *vertex - muzzle.centroid;
        mean_lateral_offset += from_muzzle - from_muzzle.dot(barrel) * barrel;
    }
    mean_lateral_offset /= vertices.len() as f32;
    let up = normalized_direction(
        -mean_lateral_offset - (-mean_lateral_offset).dot(barrel) * barrel,
        "geometric up axis",
    )?;
    let side = up.cross(barrel);

    let radius_ratio = radius_a.max(radius_b) / radius_a.min(radius_b);
    let elongation = length / (2.0 * radius_a.max(radius_b));
    let confidence =
        if radius_ratio < LOW_CONFIDENCE_RADIUS_RATIO || elongation < LOW_CONFIDENCE_ELONGATION {
            MountConfidence::Low
        } else {
            MountConfidence::High
        };

    Ok(MountDetection {
        frame: MountFrame { side, up, barrel },
        bbox_min,
        bbox_max,
        end_a,
        end_b,
        muzzle,
        stock,
        length,
        confidence,
    })
}

fn normalized_direction(value: Vec3, axis: &'static str) -> Result<Vec3, MountSolveError> {
    let length_squared = value.length_squared();
    if !value.is_finite()
        || !length_squared.is_finite()
        || length_squared <= MIN_DIRECTION_LENGTH_SQUARED
    {
        return Err(MountSolveError::InvalidDirection { axis });
    }
    Ok(value.normalize())
}

fn normalized_socket_rotation(socket_frame: Mat4) -> Result<Mat3, MountSolveError> {
    let raw = Mat3::from_mat4(socket_frame);
    let x_axis = normalized_direction(raw.x_axis, "socket x axis")
        .map_err(|_| MountSolveError::DegenerateSocketFrame)?;
    let y_axis = normalized_direction(raw.y_axis, "socket y axis")
        .map_err(|_| MountSolveError::DegenerateSocketFrame)?;
    let z_axis = normalized_direction(raw.z_axis, "socket z axis")
        .map_err(|_| MountSolveError::DegenerateSocketFrame)?;
    if x_axis.dot(y_axis).abs() > SOCKET_ORTHONORMAL_EPSILON
        || x_axis.dot(z_axis).abs() > SOCKET_ORTHONORMAL_EPSILON
        || y_axis.dot(z_axis).abs() > SOCKET_ORTHONORMAL_EPSILON
    {
        return Err(MountSolveError::NonRigidSocketFrame {
            reason: "direction axes are not orthogonal",
        });
    }

    let rotation = Mat3::from_cols(x_axis, y_axis, z_axis);
    let determinant = rotation.determinant();
    if !determinant.is_finite() || determinant <= 0.0 {
        return Err(MountSolveError::NonRigidSocketFrame {
            reason: "direction axes are reflected (determinant must be positive)",
        });
    }
    Ok(rotation)
}

#[cfg(test)]
mod tests {
    use glam::{Mat3, Mat4, Vec3};

    use crate::gltf_loader::{LoadedModel, SocketBinding};

    use super::{
        MountAxes, MountConfidence, MountFrame, corrective_delta, detect_weapon_mount_vertices,
        read_muzzle_offset_in_model, verify_mount,
    };

    fn assert_close(actual: f32, expected: f32) {
        const EPSILON: f32 = 1.0e-5;
        assert!(
            (actual - expected).abs() <= EPSILON,
            "expected {expected}, got {actual}",
        );
    }

    #[test]
    fn geometric_detection_marks_near_symmetric_ends_low_confidence() {
        // A long rectangular prism has matching end cross-sections, so it
        // cannot identify which end is the muzzle even though it is elongated.
        let vertices = [
            Vec3::new(-1.0, -1.0, -3.0),
            Vec3::new(1.0, -1.0, -3.0),
            Vec3::new(-1.0, 1.0, -3.0),
            Vec3::new(1.0, 1.0, -3.0),
            Vec3::new(-1.0, -1.0, 3.0),
            Vec3::new(1.0, -1.0, 3.0),
            Vec3::new(-1.0, 1.0, 3.0),
            Vec3::new(1.0, 1.0, 3.0),
            // Interior mass below the bore makes the geometric up direction
            // determinate without changing either end's cross-section.
            Vec3::new(0.0, -2.0, 0.0),
        ];

        let detection = detect_weapon_mount_vertices(&vertices).expect("prism is valid geometry");
        assert_eq!(detection.confidence, MountConfidence::Low);
    }

    #[test]
    fn corrective_delta_maps_declared_axes_to_engine_targets() {
        let socket = Mat4::from_mat3(Mat3::from_rotation_y(0.6))
            * Mat4::from_scale(Vec3::new(2.0, 3.0, 4.0));
        let axes = MountAxes {
            barrel: Vec3::X,
            up: Vec3::Y,
            euler: None,
        };
        let frame = axes.frame().expect("cardinal axes form a frame");
        let delta = corrective_delta(socket, frame).expect("scaled rotation is usable");
        let socket_rotation = Mat3::from_cols(
            Mat3::from_mat4(socket).x_axis.normalize(),
            Mat3::from_mat4(socket).y_axis.normalize(),
            Mat3::from_mat4(socket).z_axis.normalize(),
        );

        assert_close((socket_rotation * delta * axes.barrel).dot(Vec3::Z), 1.0);
        assert_close((socket_rotation * delta * axes.up).dot(Vec3::Y), 1.0);
    }

    #[test]
    fn verification_uses_raw_socket_rotation_for_reported_directions() {
        let frame = MountFrame::from_axes(Vec3::Z, Vec3::Y).expect("cardinal axes form a frame");
        let socket = Mat4::from_scale(Vec3::new(2.0, 3.0, 4.0));
        let verification = verify_mount(socket, frame).expect("scaled rotation is verifiable");

        assert_eq!(verification.barrel_world, Vec3::Z);
        assert_eq!(verification.up_world, Vec3::Y);
        assert_close(verification.barrel_dot_forward, 1.0);
        assert_close(verification.barrel_dot_up, 0.0);
        assert_close(verification.up_dot_up, 1.0);
    }

    #[test]
    fn verification_rejects_degenerate_socket_before_producing_metrics() {
        // Regression: zero socket columns normalized to NaN, and NaN threshold
        // comparisons let a mount check report a false pass.
        let frame = MountFrame::from_axes(Vec3::Z, Vec3::Y).expect("cardinal axes form a frame");
        let error = verify_mount(Mat4::ZERO, frame)
            .expect_err("a degenerate socket cannot produce verification metrics");

        assert!(
            error.to_string().contains("finite and non-zero"),
            "the error identifies the degenerate socket basis: {error}",
        );
    }

    #[test]
    fn corrective_delta_rejects_sheared_socket_direction_axes() {
        // Regression: independently normalizing columns accepted shear caused
        // by a rotated child under hierarchical non-uniform scale.
        let frame = MountFrame::from_axes(Vec3::Z, Vec3::Y).expect("cardinal axes form a frame");
        let sheared_socket =
            Mat4::from_scale(Vec3::new(2.0, 3.0, 4.0)) * Mat4::from_rotation_z(0.4);

        let error = corrective_delta(sheared_socket, frame)
            .expect_err("a sheared socket cannot produce a trusted corrective");
        assert!(
            error.to_string().contains("not orthogonal"),
            "the error identifies the sheared socket basis: {error}",
        );
    }

    #[test]
    fn verification_rejects_reflected_socket_direction_axes() {
        // Regression: negative-scale reflections were accepted as rotations
        // and could make a declared mount report trusted metrics.
        let frame = MountFrame::from_axes(Vec3::Z, Vec3::Y).expect("cardinal axes form a frame");
        let reflected_socket = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));

        let error = verify_mount(reflected_socket, frame)
            .expect_err("a reflected socket cannot produce trusted verification metrics");
        assert!(
            error.to_string().contains("determinant must be positive"),
            "the error identifies the reflected socket basis: {error}",
        );
    }

    #[test]
    fn muzzle_offset_reads_raw_rigid_rest_translation() {
        let mut model = LoadedModel::default();
        model.sockets.insert(
            "muzzle".to_string(),
            SocketBinding::RigidRest(Mat4::from_translation(Vec3::new(0.1, -0.2, 0.3))),
        );

        assert_eq!(
            read_muzzle_offset_in_model(&model).expect("rigid muzzle socket resolves"),
            Vec3::new(0.1, -0.2, 0.3),
        );
    }

    #[test]
    fn muzzle_offset_rejects_missing_or_skinned_socket() {
        let missing = read_muzzle_offset_in_model(&LoadedModel::default())
            .expect_err("a viewmodel muzzle socket is required");
        assert!(missing.to_string().contains("no rigid \"muzzle\" socket"));

        let mut skinned = LoadedModel::default();
        skinned
            .sockets
            .insert("muzzle".to_string(), SocketBinding::SkinnedJoint(4));
        let error = read_muzzle_offset_in_model(&skinned)
            .expect_err("a skinned muzzle socket cannot supply a viewmodel offset");
        assert!(error.to_string().contains("is a skinned joint"));
    }
}
