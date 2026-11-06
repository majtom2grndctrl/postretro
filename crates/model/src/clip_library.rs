// Shared clip library reads, and their load-time binding into a mesh's joint order.
// See: context/lib/resource_management.md §7

use std::collections::{HashMap, HashSet};
use std::path::Path;

use glam::{Quat, Vec3, Vec4};
use postretro_level_format::gltf_resolve::parse_document;

use super::gltf_clips::{ParsedClip, parse_clip};
use super::gltf_loader::ModelLoadError;
use super::skeleton::{AnimationClip, JointTracks, Skeleton, Track};

/// Everything a clip library contributes to a load, read once beside the model
/// so its content key and its parse cover the same bytes.
pub struct ClipLibrarySources {
    document: gltf::Document,
    buffers: Vec<gltf::buffer::Data>,
    document_digest: [u8; 32],
    /// Names the library in errors and warnings. Never part of a content key.
    path: String,
}

impl ClipLibrarySources {
    /// blake3 of the library document's bytes as read.
    pub fn document_digest(&self) -> [u8; 32] {
        self.document_digest
    }

    /// Each referenced buffer's bytes, in buffer-index order.
    pub fn buffers(&self) -> impl Iterator<Item = &[u8]> {
        self.buffers.iter().map(|data| data.0.as_slice())
    }
}

/// Read a clip library. Not `read_model_sources`: a library has no mesh.
pub(crate) fn read_clip_library(
    library_path: &Path,
    model_path: &str,
) -> Result<ClipLibrarySources, ModelLoadError> {
    let path = library_path.display().to_string();
    let unreadable = |source| ModelLoadError::ClipLibraryUnreadable {
        path: model_path.to_string(),
        library: path.clone(),
        source,
    };
    let parent_dir = library_path.parent().unwrap_or_else(|| Path::new(""));
    let bytes = std::fs::read(library_path).map_err(|error| unreadable(gltf::Error::Io(error)))?;
    let document = parse_document(&bytes).map_err(unreadable)?;
    let buffers = gltf::import_buffers(&document, Some(parent_dir), None).map_err(unreadable)?;
    Ok(ClipLibrarySources {
        document,
        buffers,
        document_digest: *blake3::hash(&bytes).as_bytes(),
        path,
    })
}

/// The mesh side of a bind: its skeleton, each joint's name in the same topo
/// order, and the locomotion root the loader selected.
pub(crate) struct MeshRig<'a> {
    pub(crate) skeleton: &'a Skeleton,
    pub(crate) joint_names: &'a [Option<String>],
    pub(crate) locomotion_root: Option<usize>,
}

/// Bind `library`'s clips and append them after the embedded `clips`, in library
/// order. A library clip an embedded clip already names is skipped: that is how
/// a character overrides a shared clip.
pub(crate) fn append_library_clips(
    clips: &mut Vec<AnimationClip>,
    library: &ClipLibrarySources,
    rig: MeshRig,
    path_str: &str,
) -> Result<(), ModelLoadError> {
    let binder = Binder::new(library, &rig, path_str)?;
    if clips.is_empty() && library.document.animations().next().is_none() {
        return Err(ModelLoadError::ClipLibraryEmpty {
            path: path_str.to_string(),
            library: library.path.clone(),
        });
    }

    // Every node is a parse slot, not only those this mesh binds: a clip's
    // duration then counts the tracks a bind drops, so it is the same on every
    // mesh, and no skin is needed to find a joint.
    let node_count = library.document.nodes().len();
    let node_slots: HashMap<usize, usize> = (0..node_count).map(|node| (node, node)).collect();
    let embedded: HashSet<String> = clips.iter().map(|clip| clip.name.clone()).collect();
    for animation in library.document.animations() {
        if embedded.contains(animation.name().unwrap_or("")) {
            continue;
        }
        let parsed = parse_clip(
            &animation,
            &library.buffers,
            &node_slots,
            node_count,
            &library.path,
        );
        clips.push(
            binder
                .bind(parsed, path_str)
                .derive_root_motion(Some(binder.root.joint), path_str),
        );
    }
    Ok(())
}

/// One mesh joint's library counterpart.
struct JointBind {
    library_node: usize,
    /// `rest_mesh * rest_library⁻¹`.
    rest_correction: Quat,
}

struct RootBind {
    joint: usize,
    mesh_rest: Vec3,
    library_rest: Vec3,
    /// Mesh root rest height over library root rest height.
    height_ratio: f32,
}

/// The per-model half of a bind, computed once for all of a library's clips.
struct Binder {
    /// Parallel to the mesh skeleton. `None` holds rest.
    joints: Vec<Option<JointBind>>,
    root: RootBind,
}

impl Binder {
    fn new(
        library: &ClipLibrarySources,
        rig: &MeshRig,
        path_str: &str,
    ) -> Result<Self, ModelLoadError> {
        let root = rig
            .locomotion_root
            .ok_or_else(|| ModelLoadError::ClipLibraryNoRoot {
                path: path_str.to_string(),
                library: library.path.clone(),
            })?;

        // Duplicate names keep the first document node, as sockets do.
        let mut nodes_by_name: HashMap<&str, gltf::Node> = HashMap::new();
        let mut repeated_names: HashSet<&str> = HashSet::new();
        for node in library.document.nodes() {
            let Some(name) = node.name() else {
                continue;
            };
            if nodes_by_name.contains_key(name) {
                repeated_names.insert(name);
            } else {
                nodes_by_name.insert(name, node);
            }
        }
        // The first node may carry no tracks while a later one does, which
        // leaves the joint in rest with nothing else to say why.
        let mut repeated: Vec<&str> = rig
            .joint_names
            .iter()
            .flatten()
            .map(String::as_str)
            .filter(|name| repeated_names.contains(name))
            .collect();
        repeated.sort_unstable();
        repeated.dedup();
        if !repeated.is_empty() {
            log::warn!(
                "[Model] joints in {path_str} match several nodes in clip library {} and bind the first of each: {}",
                library.path,
                repeated.join(", ")
            );
        }

        let library_node = |joint: usize| {
            rig.joint_names[joint]
                .as_deref()
                .and_then(|name| nodes_by_name.get(name))
        };

        let library_root =
            library_node(root).ok_or_else(|| ModelLoadError::ClipLibraryRootMissing {
                path: path_str.to_string(),
                library: library.path.clone(),
                joint: joint_label(rig, root),
            })?;
        let mesh_rest = rig.skeleton.joints[root].rest_local.translation;
        let library_rest = Vec3::from(library_root.transform().decomposed().0);
        let height_ratio = mesh_rest.y / library_rest.y;
        let usable = |height: f32| height.is_finite() && height > 0.0;
        if !usable(mesh_rest.y) || !usable(library_rest.y) || !usable(height_ratio) {
            return Err(ModelLoadError::ClipLibraryRootHeight {
                path: path_str.to_string(),
                library: library.path.clone(),
                mesh_height: mesh_rest.y,
                library_height: library_rest.y,
            });
        }

        let mut has_child = vec![false; rig.skeleton.joints.len()];
        for joint in &rig.skeleton.joints {
            if let Some(parent) = joint.parent {
                has_child[parent] = true;
            }
        }

        let mut joints = Vec::with_capacity(rig.skeleton.joints.len());
        let mut unbound = Vec::new();
        let mut degenerate = Vec::new();
        for (index, joint) in rig.skeleton.joints.iter().enumerate() {
            let Some(node) = library_node(index) else {
                // A leaf with no library joint is routine, such as an end
                // marker. A non-leaf is more likely a renamed joint, whose
                // subtree would otherwise stop following clips with no sign.
                if has_child[index] {
                    unbound.push(joint_label(rig, index));
                }
                joints.push(None);
                continue;
            };
            let library_rotation = Quat::from_array(node.transform().decomposed().1);
            let rest_correction = unit_rotation(joint.rest_local.rotation)
                .zip(unit_rotation(library_rotation))
                .map(|(mesh, library)| mesh * library.inverse());
            if rest_correction.is_none() {
                degenerate.push(joint_label(rig, index));
            }
            joints.push(rest_correction.map(|rest_correction| JointBind {
                library_node: node.index(),
                rest_correction,
            }));
        }

        if !unbound.is_empty() {
            log::warn!(
                "[Model] non-leaf joints in {path_str} have no joint in clip library {} and hold rest: {}",
                library.path,
                unbound.join(", ")
            );
        }
        if !degenerate.is_empty() {
            log::warn!(
                "[Model] joints in {path_str} or clip library {} have a degenerate rest rotation and hold rest: {}",
                library.path,
                degenerate.join(", ")
            );
        }
        warn_parent_mismatches(library, rig, &joints, path_str);

        Ok(Self {
            joints,
            root: RootBind {
                joint: root,
                mesh_rest,
                library_rest,
                height_ratio,
            },
        })
    }

    /// Recast a library clip, parsed one slot per library node, onto the mesh's
    /// joints. Key times, interpolation and duration stay the library's.
    /// Non-root translation and all scale drop, so proportions stay the mesh's.
    fn bind(&self, clip: ParsedClip, path_str: &str) -> ParsedClip {
        let mut joints = vec![JointTracks::default(); self.joints.len()];
        let mut dropped_rotations = 0;
        for (index, bind) in self.joints.iter().enumerate() {
            let Some(bind) = bind else {
                continue;
            };
            let source = &clip.joints[bind.library_node];
            let rotation = map_track(&source.rotation, |rotation| {
                (bind.rest_correction * rotation).normalize()
            });
            // A key of near-zero length passes parse validation, but its squared
            // length underflows here, normalizing to NaN or to a non-unit value.
            if rotation.values.iter().all(|value| value.is_normalized()) {
                joints[index].rotation = rotation;
            } else {
                dropped_rotations += 1;
            }
            if index != self.root.joint {
                continue;
            }
            let translation = map_track(&source.translation, |translation| {
                let delta = (translation - self.root.library_rest) * self.root.height_ratio;
                self.root.mesh_rest + bind.rest_correction * delta
            });
            if translation.values.iter().all(|value| value.is_finite()) {
                joints[index].translation = translation;
            } else {
                log::warn!(
                    "[Model] library clip '{}' has non-finite root translation in {path_str}; the root holds its rest translation",
                    clip.name
                );
            }
        }
        if dropped_rotations > 0 {
            log::warn!(
                "[Model] library clip '{}' has {dropped_rotations} rotation tracks with a near-zero key in {path_str}; those joints hold rest",
                clip.name
            );
        }
        ParsedClip {
            name: clip.name,
            duration: clip.duration,
            joints,
        }
    }
}

fn map_track<T: Copy>(track: &Track<T>, map: impl Fn(T) -> T) -> Track<T> {
    Track {
        times: track.times.clone(),
        values: track.values.iter().copied().map(map).collect(),
        mode: track.mode,
    }
}

/// A rest rotation is raw authored data, and may not be unit or even non-zero.
fn unit_rotation(rotation: Quat) -> Option<Quat> {
    Vec4::from(rotation).try_normalize().map(Quat::from_vec4)
}

fn joint_label(rig: &MeshRig, index: usize) -> String {
    rig.joint_names[index]
        .clone()
        .unwrap_or_else(|| format!("<unnamed joint {index}>"))
}

/// Binding is per joint and local, so a differing parent still binds, but the
/// pose it gives is unlikely to be the one authored. A library parent the mesh
/// has no joint for reads as no parent: an armature node above the root is not
/// a difference.
fn warn_parent_mismatches(
    library: &ClipLibrarySources,
    rig: &MeshRig,
    joints: &[Option<JointBind>],
    path_str: &str,
) {
    let mut library_parents: HashMap<usize, &str> = HashMap::new();
    for node in library.document.nodes() {
        if let Some(name) = node.name() {
            for child in node.children() {
                library_parents.insert(child.index(), name);
            }
        }
    }
    let mesh_names: HashSet<&str> = rig
        .joint_names
        .iter()
        .flatten()
        .map(String::as_str)
        .collect();

    let mismatched: Vec<String> = joints
        .iter()
        .enumerate()
        .filter_map(|(index, bind)| Some((index, bind.as_ref()?)))
        .filter(|(index, bind)| {
            let mesh_parent = rig.skeleton.joints[*index]
                .parent
                .and_then(|parent| rig.joint_names[parent].as_deref());
            let library_parent = library_parents
                .get(&bind.library_node)
                .copied()
                .filter(|name| mesh_names.contains(name));
            mesh_parent != library_parent
        })
        .map(|(index, _)| joint_label(rig, index))
        .collect();
    if !mismatched.is_empty() {
        log::warn!(
            "[Model] joints in {path_str} bind with a different parent than in clip library {}: {}",
            library.path,
            mismatched.join(", ")
        );
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, PI};
    use std::path::{Path, PathBuf};

    use glam::{Quat, Vec3};
    use log::Level;
    use postretro_test_log_capture::LogCapture;
    use serde_json::json;
    use tempfile::TempDir;

    use crate::anim::{Loop, sample_clip_looped_world};
    use crate::clip_library_test_fixtures::{
        Document, clip, library, node, patch_document, rigid, skinned,
    };
    use crate::gltf_loader::{
        LoadedModel, ModelLoadError, load_model, load_model_from_sources, read_model_sources,
    };
    use crate::skeleton::{AnimationClip, Interp};

    const LIBRARY: &str = "library.gltf";

    /// Write `model` bound to `lib` in a fresh directory.
    fn write_pair(model: Document, lib: Document) -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        lib.write(&dir.path().join(LIBRARY));
        let path = model
            .clip_library(LIBRARY)
            .write(&dir.path().join("model.gltf"));
        (dir, path)
    }

    fn load_pair(model: Document, lib: Document) -> LoadedModel {
        let (_dir, path) = write_pair(model, lib);
        load_model(&path).expect("the model binds")
    }

    fn load_err(model: Document, lib: Document) -> ModelLoadError {
        let (_dir, path) = write_pair(model, lib);
        load_model(&path).expect_err("the load must fail")
    }

    fn named<'a>(model: &'a LoadedModel, name: &str) -> &'a AnimationClip {
        model
            .clips
            .iter()
            .find(|clip| clip.name == name)
            .unwrap_or_else(|| panic!("clip '{name}' resolves"))
    }

    fn clip_names(model: &LoadedModel) -> Vec<&str> {
        model.clips.iter().map(|clip| clip.name.as_str()).collect()
    }

    fn assert_rotation(got: Quat, want: Quat, label: &str) {
        assert!(
            got.dot(want).abs() > 1.0 - 1.0e-5,
            "{label}: got {got:?}, want {want:?}"
        );
        assert!((got.length() - 1.0).abs() < 1.0e-5, "{label}: normalized");
    }

    fn assert_vec3(got: Vec3, want: Vec3, label: &str) {
        assert!(
            got.abs_diff_eq(want, 1.0e-5),
            "{label}: got {got:?}, want {want:?}"
        );
    }

    /// A rig with its root at height 1, for tests the root does not concern.
    fn root_at(height: f32) -> crate::clip_library_test_fixtures::Node {
        node("pelvis").at(Vec3::new(0.0, height, 0.0))
    }

    // --- Binding -----------------------------------------------------------

    #[test]
    fn joints_bind_by_name_across_order_and_count() {
        // Distinct turns per library joint; the mesh lists its joints in another
        // order, lacks `tail`, and adds a leaf of its own.
        let turn = |angle: f32| Quat::from_rotation_y(angle);
        let lib = library(vec![
            root_at(1.0),
            node("spine").under("pelvis"),
            node("tail").under("pelvis"),
            node("arm_left").under("spine"),
            node("arm_right").under("spine"),
        ])
        .clip(
            clip("wave")
                .rotation("spine", &[0.0, 1.0], &[turn(0.1), turn(0.2)])
                .rotation("tail", &[0.0, 1.0], &[turn(0.3), turn(0.4)])
                .rotation("arm_left", &[0.0, 1.0], &[turn(0.5), turn(0.6)])
                .rotation("arm_right", &[0.0, 1.0], &[turn(0.7), turn(0.8)]),
        );
        let model = load_pair(
            skinned(vec![
                root_at(1.0),
                node("spine").under("pelvis"),
                node("arm_right").under("spine"),
                node("arm_left").under("spine"),
                node("hand_right").under("arm_right"),
            ]),
            lib,
        );

        let clip = named(&model, "wave");
        assert_eq!(clip.joints.len(), model.skeleton.joints.len());
        assert_eq!(clip.joints.len(), 5);
        for (joint, want) in [(1, 0.1), (2, 0.7), (3, 0.5)] {
            assert_rotation(
                clip.joints[joint].rotation.values()[0],
                turn(want),
                "each joint takes its own track",
            );
        }
        assert!(clip.joints[0].rotation.values().is_empty());
        assert!(clip.joints[4].rotation.values().is_empty());
    }

    #[test]
    fn rotation_composes_rest_correction() {
        let rest_lib = Quat::from_rotation_z(0.5);
        let rest_mesh = Quat::from_rotation_y(FRAC_PI_2);
        let delta = Quat::from_rotation_x(0.7);
        let key = rest_lib * delta;
        let lib = library(vec![
            root_at(1.0),
            node("arm").under("pelvis").turned(rest_lib),
        ])
        .clip(clip("raise").rotation("arm", &[0.0, 1.0], &[rest_lib, key]));
        let model = load_pair(
            skinned(vec![
                root_at(1.0),
                node("arm").under("pelvis").turned(rest_mesh),
            ]),
            lib,
        );

        let values = named(&model, "raise").joints[1].rotation.values().to_vec();
        assert_rotation(values[0], rest_mesh, "library rest lands on mesh rest");
        assert_rotation(
            values[1],
            rest_mesh * rest_lib.inverse() * key,
            "the delta rides the mesh rest",
        );
        assert!(
            values[1].angle_between(key) > 0.1,
            "uncorrected name-matching would give a different pose"
        );
    }

    #[test]
    fn composed_rotations_are_normalized() {
        let key = Quat::from_xyzw(0.0, 0.0, 0.0, 2.0);
        let lib = library(vec![root_at(1.0), node("arm").under("pelvis")])
            .clip(clip("hold").rotation("arm", &[0.0, 1.0], &[key, key]));
        let model = load_pair(
            skinned(vec![root_at(1.0), node("arm").under("pelvis")]),
            lib,
        );

        assert_rotation(
            named(&model, "hold").joints[1].rotation.values()[1],
            Quat::IDENTITY,
            "a non-unit key composes to a unit rotation",
        );
    }

    // Regression: this key passed validation, then normalized to NaN through a
    // rest correction, or to a non-unit rotation through none.
    #[test]
    fn near_zero_rotation_key_holds_rest_and_warns_once() {
        let tiny = Quat::from_xyzw(0.0, 0.0, 0.0, 3.0e-23);
        let hostile = [Quat::IDENTITY, tiny];
        let sound = [Quat::IDENTITY, Quat::from_rotation_y(0.2)];
        let lib = library(vec![
            root_at(1.0),
            node("arm").under("pelvis"),
            node("leg").under("pelvis"),
            node("head").under("pelvis"),
        ])
        .clip(
            clip("seize")
                .rotation("arm", &[0.0, 1.0], &hostile)
                .rotation("leg", &[0.0, 4.0], &hostile)
                .rotation("head", &[0.0, 1.0], &sound),
        );
        let capture = LogCapture::start();
        let model = load_pair(
            skinned(vec![
                root_at(1.0),
                node("arm")
                    .under("pelvis")
                    .turned(Quat::from_rotation_y(FRAC_PI_2)),
                node("leg").under("pelvis"),
                node("head").under("pelvis"),
            ]),
            lib,
        );

        let clip = named(&model, "seize");
        assert!(clip.joints[1].rotation.values().is_empty(), "NaN key");
        assert!(clip.joints[2].rotation.values().is_empty(), "non-unit key");
        assert_rotation(
            clip.joints[3].rotation.values()[1],
            sound[1],
            "a sound track on the same clip binds",
        );
        assert_eq!(clip.duration, 4.0, "a dropped track keeps the duration");
        capture.assert_logged_once(Level::Warn, "near-zero key");
    }

    // Regression: the root bound a first node with no tracks, and the model
    // stood in rest with no warning.
    #[test]
    fn repeated_library_node_for_a_mesh_joint_warns_once_naming_it() {
        let lib = library(vec![
            root_at(1.0),
            node("pelvis_twin").at(Vec3::Y),
            node("arm").under("pelvis"),
            node("arm_twin").under("pelvis"),
            node("crate_lid"),
            node("crate_lid_twin"),
        ])
        .clip(clip("sway").rotation(
            "pelvis_twin",
            &[0.0, 1.0],
            &[Quat::IDENTITY, Quat::from_rotation_y(0.3)],
        ));
        let (dir, path) = write_pair(bound_mesh(), lib);
        patch_document(&dir.path().join(LIBRARY), |root| {
            root["nodes"][1]["name"] = json!("pelvis");
            root["nodes"][3]["name"] = json!("arm");
            root["nodes"][5]["name"] = json!("crate_lid");
        });
        let capture = LogCapture::start();
        let model = load_model(&path).unwrap();

        assert!(
            named(&model, "sway").joints[0].rotation.values().is_empty(),
            "the first node wins"
        );
        capture.assert_logged_once(Level::Warn, "match several nodes");
        capture.assert_logged(Level::Warn, "bind the first of each: arm, pelvis");
        capture.assert_not_logged(Level::Warn, "crate_lid");
    }

    #[test]
    fn unmatched_joints_hold_rest_or_are_ignored() {
        let lib = library(vec![
            root_at(1.0),
            node("spine").under("pelvis"),
            node("library_only").under("spine"),
        ])
        .clip(
            clip("sway")
                .rotation(
                    "spine",
                    &[0.0, 1.0],
                    &[Quat::IDENTITY, Quat::from_rotation_y(0.3)],
                )
                .rotation(
                    "library_only",
                    &[0.0, 1.0],
                    &[Quat::IDENTITY, Quat::from_rotation_y(0.6)],
                ),
        );
        let capture = LogCapture::start();
        let model = load_pair(
            skinned(vec![
                root_at(1.0),
                node("spine").under("pelvis"),
                node("mesh_only").under("spine"),
            ]),
            lib,
        );

        let tracks = &named(&model, "sway").joints[2];
        assert!(tracks.rotation.values().is_empty(), "mesh joint holds rest");
        assert!(tracks.translation.values().is_empty());
        assert!(tracks.scale.values().is_empty());
        capture.assert_nothing_above(Level::Info);
    }

    #[test]
    fn non_leaf_joint_without_library_joint_warns_once_naming_it() {
        let lib = library(vec![root_at(1.0), node("forearm").under("pelvis")]).clip(clip("idle"));
        let capture = LogCapture::start();
        load_pair(
            skinned(vec![
                root_at(1.0),
                node("renamed_a").under("pelvis"),
                node("renamed_b").under("renamed_a"),
                node("leaf_marker").under("renamed_b"),
            ]),
            lib,
        );

        capture.assert_logged_once(Level::Warn, "non-leaf joints");
        capture.assert_logged(Level::Warn, "hold rest: renamed_a, renamed_b");
        capture.assert_not_logged(Level::Warn, "leaf_marker");
        capture.assert_not_logged(Level::Warn, "forearm");
    }

    #[test]
    fn joint_with_a_different_parent_binds_and_warns_once_naming_it() {
        let turn = Quat::from_rotation_y(0.4);
        let lib = library(vec![
            root_at(1.0),
            node("spine").under("pelvis"),
            node("head").under("spine"),
            node("hand").under("spine"),
        ])
        .clip(clip("nod").rotation("hand", &[0.0, 1.0], &[turn, turn]));
        let capture = LogCapture::start();
        let model = load_pair(
            skinned(vec![
                root_at(1.0),
                node("spine").under("pelvis"),
                node("head").under("spine"),
                node("hand").under("head"),
            ]),
            lib,
        );

        assert_rotation(
            named(&model, "nod").joints[3].rotation.values()[0],
            turn,
            "the joint still binds",
        );
        capture.assert_logged_once(Level::Warn, "different parent");
        capture.assert_logged(Level::Warn, "clip library");
        let warning = capture
            .records()
            .into_iter()
            .find(|record| record.message.contains("different parent"))
            .unwrap();
        assert!(warning.message.ends_with(": hand"), "{}", warning.message);
    }

    #[test]
    fn degenerate_rest_rotation_holds_rest_and_warns() {
        let lib = library(vec![
            root_at(1.0),
            node("arm").under("pelvis").zero_rotation(),
        ])
        .clip(clip("lift").rotation(
            "arm",
            &[0.0, 1.0],
            &[Quat::IDENTITY, Quat::from_rotation_y(0.2)],
        ));
        let capture = LogCapture::start();
        let model = load_pair(
            skinned(vec![root_at(1.0), node("arm").under("pelvis")]),
            lib,
        );

        assert!(named(&model, "lift").joints[1].rotation.values().is_empty());
        capture.assert_logged_once(Level::Warn, "degenerate rest rotation");
    }

    #[test]
    fn non_root_translation_and_all_scale_drop() {
        let arm_rest = Vec3::new(0.3, 0.2, 0.0);
        let lib = library(vec![root_at(1.0), node("arm").under("pelvis")]).clip(
            clip("stretch")
                .translation("arm", &[0.0, 1.0], &[Vec3::ZERO, Vec3::splat(5.0)])
                .scale("arm", &[0.0, 1.0], &[Vec3::ONE, Vec3::splat(3.0)])
                .scale("pelvis", &[0.0, 1.0], &[Vec3::ONE, Vec3::splat(4.0)]),
        );
        let rest_scale = Vec3::new(1.0, 2.0, 1.0);
        let model = load_pair(
            skinned(vec![
                root_at(1.0).scaled(rest_scale),
                node("arm").under("pelvis").at(arm_rest),
            ]),
            lib,
        );

        let clip = named(&model, "stretch");
        assert!(clip.joints[1].translation.values().is_empty());
        assert!(clip.joints[1].scale.values().is_empty());
        assert!(clip.joints[0].scale.values().is_empty());
        let mut world = Vec::new();
        sample_clip_looped_world(clip, &model.skeleton, 1.0, Loop::Clamp, &mut world);
        let (root_scale, _, _) = world[0].to_scale_rotation_translation();
        assert_vec3(root_scale, rest_scale, "the root keeps its rest scale");
        assert_vec3(
            world[1].w_axis.truncate(),
            Vec3::new(0.0, 1.0, 0.0) + rest_scale * arm_rest,
            "the bone keeps the mesh's length",
        );
    }

    #[test]
    fn root_translation_scales_by_height_and_turns_into_mesh_facing() {
        let library_rest = Vec3::new(0.1, 1.0, -0.2);
        let mesh_rest = Vec3::new(0.0, 2.0, 0.3);
        let offset = Vec3::new(0.5, 0.5, 0.25);
        let lib = library(vec![node("pelvis").at(library_rest)]).clip(clip("hop").translation(
            "pelvis",
            &[0.0, 1.0, 2.0],
            &[library_rest, library_rest + offset, library_rest],
        ));
        let model = load_pair(
            skinned(vec![
                node("pelvis")
                    .at(mesh_rest)
                    .turned(Quat::from_rotation_y(PI)),
            ]),
            lib,
        );

        let values = named(&model, "hop").joints[0].translation.values().to_vec();
        assert_vec3(values[0], mesh_rest, "library rest lands on mesh rest");
        assert_vec3(
            values[1],
            mesh_rest + Vec3::new(-1.0, 1.0, -0.5),
            "doubled, and turned half way round",
        );
    }

    #[test]
    fn library_root_at_rest_leaves_mesh_root_at_its_rest() {
        let library_rest = Vec3::new(0.1, 1.0, -0.2);
        let mesh_rest = Vec3::new(0.0, 2.0, 0.3);
        let lib = library(vec![node("pelvis").at(library_rest)]).clip(clip("stand").translation(
            "pelvis",
            &[0.0, 1.0],
            &[library_rest, library_rest],
        ));
        let model = load_pair(skinned(vec![node("pelvis").at(mesh_rest)]), lib);

        let clip = named(&model, "stand");
        for value in clip.joints[0].translation.values() {
            assert_vec3(*value, mesh_rest, "the root stays at the mesh's rest");
        }
        assert_eq!(clip.travel_speed, None);
    }

    #[test]
    fn root_translation_past_finite_range_holds_rest_and_warns() {
        let lib = library(vec![node("pelvis").at(Vec3::new(0.0, 1.0e-5, 0.0))]).clip(
            clip("launch").translation(
                "pelvis",
                &[0.0, 1.0],
                &[Vec3::ZERO, Vec3::new(0.0, 1.0e10, 0.0)],
            ),
        );
        let capture = LogCapture::start();
        let model = load_pair(
            skinned(vec![node("pelvis").at(Vec3::new(0.0, 1.0e30, 0.0))]),
            lib,
        );

        assert!(
            named(&model, "launch").joints[0]
                .translation
                .values()
                .is_empty()
        );
        capture.assert_logged_once(Level::Warn, "has non-finite root translation");
    }

    #[test]
    fn travel_speed_scales_by_height_and_neutralizes_like_an_embedded_clip() {
        let rest = Vec3::new(0.0, 1.0, 0.0);
        let lib = library(vec![node("pelvis").at(rest)])
            .clip(clip("walk").translation(
                "pelvis",
                &[0.0, 1.0, 2.0],
                &[rest, rest + Vec3::new(0.2, 0.1, 1.5), rest + Vec3::Z * 3.0],
            ))
            .clip(clip("idle").translation("pelvis", &[0.0, 1.0], &[rest, rest + Vec3::Y * 0.1]));
        let mesh_rest = Vec3::new(0.0, 2.0, 0.0);
        let bound = load_pair(skinned(vec![node("pelvis").at(mesh_rest)]), lib);

        let walk = named(&bound, "walk");
        let speed = walk.travel_speed.expect("the clip travels");
        assert!((speed - 3.0).abs() < 1.0e-5, "6 m over 2 s, got {speed}");
        assert_eq!(named(&bound, "idle").travel_speed, None);

        // The same clip authored into the mesh, already at its bound values.
        let dir = tempfile::tempdir().unwrap();
        let embedded = skinned(vec![node("pelvis").at(mesh_rest)])
            .clip(clip("walk").translation(
                "pelvis",
                &[0.0, 1.0, 2.0],
                &[
                    mesh_rest,
                    mesh_rest + Vec3::new(0.4, 0.2, 3.0),
                    mesh_rest + Vec3::Z * 6.0,
                ],
            ))
            .write(&dir.path().join("embedded.gltf"));
        let embedded = load_model(&embedded).unwrap();
        assert_eq!(walk, &embedded.clips[0]);
        let values = walk.joints[0].translation.values();
        assert_eq!(values.first().unwrap().z, values.last().unwrap().z);
    }

    #[test]
    fn bounds_cover_a_pose_only_a_library_clip_reaches_at_the_scaled_height() {
        let rest = Vec3::new(0.0, 1.0, 0.0);
        let lib = library(vec![node("pelvis").at(rest)]).clip(clip("leap").translation(
            "pelvis",
            &[0.0, 1.0],
            &[rest, rest + Vec3::Y * 3.0],
        ));
        let mesh = || skinned(vec![node("pelvis").at(Vec3::new(0.0, 2.0, 0.0))]);
        let bound = load_pair(mesh(), lib);

        let dir = tempfile::tempdir().unwrap();
        let unbound = load_model(&mesh().write(&dir.path().join("plain.gltf"))).unwrap();

        let mut world = Vec::new();
        let clip = named(&bound, "leap");
        sample_clip_looped_world(clip, &bound.skeleton, 1.0, Loop::Clamp, &mut world);
        let posed = world[0].transform_point3(Vec3::Y);
        assert_vec3(posed, Vec3::new(0.0, 9.0, 0.0), "2 m rest, 6 m scaled leap");
        assert!(
            bound.mesh.bounds().max.y >= posed.y,
            "{:?} must hold {posed:?}",
            bound.mesh.bounds()
        );
        assert!(
            unbound.mesh.bounds().max.y < posed.y,
            "only the clip reaches it"
        );
    }

    #[test]
    fn skinless_library_under_a_non_joint_parent_binds_its_named_root() {
        // The top node is taller than the root and turned: picking it would
        // change both the ratio and the correction.
        let rest = Vec3::new(0.0, 1.0, 0.0);
        let lib = library(vec![
            node("armature")
                .at(Vec3::new(0.0, 8.0, 0.0))
                .turned(Quat::from_rotation_y(FRAC_PI_2)),
            node("pelvis").under("armature").at(rest),
            node("spine").under("pelvis"),
        ])
        .clip(clip("bob").translation(
            "pelvis",
            &[0.0, 1.0, 2.0],
            &[rest, rest + Vec3::Y, rest],
        ));
        let capture = LogCapture::start();
        let model = load_pair(
            skinned(vec![
                node("pelvis").at(Vec3::new(0.0, 2.0, 0.0)),
                node("spine").under("pelvis"),
            ]),
            lib,
        );

        assert_vec3(
            named(&model, "bob").joints[0].translation.values()[1],
            Vec3::new(0.0, 4.0, 0.0),
            "height ratio comes from the node named as the root",
        );
        capture.assert_nothing_above(Level::Info);
    }

    #[test]
    fn bound_tracks_keep_library_key_times_and_interpolation() {
        let times = [0.0, 0.25, 1.5];
        let rest = Vec3::new(0.0, 1.0, 0.0);
        let turns = [
            Quat::IDENTITY,
            Quat::from_rotation_y(0.4),
            Quat::from_rotation_y(0.8),
        ];
        let lib = library(vec![node("pelvis").at(rest), node("arm").under("pelvis")]).clip(
            clip("chop")
                .stepped_rotation("arm", &times, &turns)
                .stepped_translation("pelvis", &times, &[rest, rest + Vec3::Y, rest])
                .rotation("pelvis", &times, &turns),
        );
        let model = load_pair(
            skinned(vec![
                node("pelvis").at(Vec3::new(0.0, 2.0, 0.0)),
                node("arm").under("pelvis"),
            ]),
            lib,
        );

        let clip = named(&model, "chop");
        assert_eq!(clip.joints[1].rotation.times(), times);
        assert_eq!(clip.joints[1].rotation.mode(), Interp::Step);
        assert_eq!(clip.joints[0].translation.times(), times);
        assert_eq!(clip.joints[0].translation.mode(), Interp::Step);
        assert_eq!(clip.joints[0].rotation.mode(), Interp::Linear);
    }

    #[test]
    fn clip_duration_is_the_librarys_whatever_a_mesh_drops() {
        let library_doc = || {
            library(vec![
                root_at(1.0),
                node("arm").under("pelvis"),
                node("tail").under("pelvis"),
            ])
            .clip(
                clip("long_tail")
                    .rotation("arm", &[0.0, 1.0], &[Quat::IDENTITY, Quat::IDENTITY])
                    .rotation("tail", &[0.0, 7.0], &[Quat::IDENTITY, Quat::IDENTITY]),
            )
            .clip(
                clip("long_scale")
                    .rotation("arm", &[0.0, 1.0], &[Quat::IDENTITY, Quat::IDENTITY])
                    .scale("arm", &[0.0, 5.0], &[Vec3::ONE, Vec3::ONE]),
            )
            .clip(
                clip("long_translation")
                    .rotation("arm", &[0.0, 1.0], &[Quat::IDENTITY, Quat::IDENTITY])
                    .translation("arm", &[0.0, 3.0], &[Vec3::ZERO, Vec3::ZERO]),
            )
        };
        let with_tail = load_pair(
            skinned(vec![
                root_at(1.0),
                node("arm").under("pelvis"),
                node("tail").under("pelvis"),
            ]),
            library_doc(),
        );
        let without_tail = load_pair(
            skinned(vec![root_at(1.0), node("arm").under("pelvis")]),
            library_doc(),
        );

        for (name, duration) in [
            ("long_tail", 7.0),
            ("long_scale", 5.0),
            ("long_translation", 3.0),
        ] {
            assert_eq!(named(&with_tail, name).duration, duration, "{name}");
            assert_eq!(named(&without_tail, name).duration, duration, "{name}");
        }
    }

    // --- Clip list ---------------------------------------------------------

    fn turning_library() -> Document {
        library(vec![root_at(1.0), node("arm").under("pelvis")])
            .clip(clip("shared").rotation(
                "arm",
                &[0.0, 1.0],
                &[Quat::IDENTITY, Quat::from_rotation_y(0.2)],
            ))
            .clip(clip("library_only").rotation(
                "arm",
                &[0.0, 2.0],
                &[Quat::IDENTITY, Quat::from_rotation_y(0.4)],
            ))
    }

    fn embedding_model() -> Document {
        let rest = Vec3::new(0.0, 1.0, 0.0);
        skinned(vec![root_at(1.0), node("arm").under("pelvis")])
            .clip(clip("embedded_only").translation(
                "pelvis",
                &[0.0, 1.0],
                &[rest, rest + Vec3::Z * 2.0],
            ))
            .clip(clip("shared").rotation(
                "arm",
                &[0.0, 9.0],
                &[Quat::IDENTITY, Quat::from_rotation_y(1.0)],
            ))
    }

    #[test]
    fn embedded_clips_list_first_and_override_silently() {
        let capture = LogCapture::start();
        let model = load_pair(embedding_model(), turning_library());

        assert_eq!(
            clip_names(&model),
            ["embedded_only", "shared", "library_only"]
        );
        assert_eq!(
            named(&model, "shared").duration,
            9.0,
            "the embedded clip wins"
        );
        capture.assert_nothing_above(Level::Info);
    }

    #[test]
    fn embedded_clips_equal_a_load_with_no_library() {
        let dir = tempfile::tempdir().unwrap();
        let plain = load_model(&embedding_model().write(&dir.path().join("plain.gltf"))).unwrap();
        let bound = load_pair(embedding_model(), turning_library());

        assert!(plain.clips[0].travel_speed.is_some());
        assert_eq!(plain.clips.len(), 2);
        assert_eq!(bound.clips[..2], plain.clips[..]);
    }

    #[test]
    fn clip_zero_is_the_librarys_first_without_embedded_clips() {
        let model = load_pair(
            skinned(vec![root_at(1.0), node("arm").under("pelvis")]),
            turning_library(),
        );

        assert_eq!(clip_names(&model), ["shared", "library_only"]);
    }

    #[test]
    fn model_with_no_reference_reads_and_loads_no_library() {
        let dir = tempfile::tempdir().unwrap();
        let path = embedding_model().write(&dir.path().join("model.gltf"));
        let capture = LogCapture::start();

        assert!(read_model_sources(&path).unwrap().library().is_none());
        assert_eq!(
            clip_names(&load_model(&path).unwrap()),
            ["embedded_only", "shared"]
        );
        capture.assert_nothing_above(Level::Info);
    }

    // --- Refusal and permission --------------------------------------------

    fn bound_mesh() -> Document {
        skinned(vec![root_at(1.0), node("arm").under("pelvis")])
    }

    #[test]
    fn unreadable_library_fails_the_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = bound_mesh()
            .clip_library("absent.gltf")
            .write(&dir.path().join("model.gltf"));
        let error = load_model(&path).unwrap_err();
        assert!(
            matches!(error, ModelLoadError::ClipLibraryUnreadable { .. }),
            "absent file: {error:?}"
        );

        std::fs::write(dir.path().join("absent.gltf"), b"not glTF {{{").unwrap();
        let error = load_model(&path).unwrap_err();
        assert!(
            matches!(error, ModelLoadError::ClipLibraryUnreadable { .. }),
            "not glTF: {error:?}"
        );

        let library_path = turning_library().write(&dir.path().join("absent.gltf"));
        std::fs::remove_file(library_path.with_extension("bin")).unwrap();
        let error = load_model(&path).unwrap_err();
        assert!(
            matches!(error, ModelLoadError::ClipLibraryUnreadable { .. }),
            "missing buffer: {error:?}"
        );
    }

    // Regression: both inputs panicked inside gltf, before any library check ran.
    #[test]
    fn library_gltf_panics_on_fails_the_load() {
        let patches: [(&str, fn(&mut serde_json::Value)); 2] = [
            ("POSITION index past the accessors", |root| {
                root["meshes"] = serde_json::json!([
                    { "primitives": [{ "attributes": { "POSITION": 99 } }] }
                ]);
            }),
            ("buffer URI escaping invalid UTF-8", |root| {
                root["buffers"][0]["uri"] = serde_json::json!("%FF.bin");
            }),
        ];
        for (label, patch) in patches {
            let dir = tempfile::tempdir().unwrap();
            let path = bound_mesh()
                .clip_library(LIBRARY)
                .write(&dir.path().join("model.gltf"));
            turning_library().write(&dir.path().join(LIBRARY));
            patch_document(&dir.path().join(LIBRARY), patch);

            let result = std::panic::catch_unwind(|| load_model(&path));
            let load_result = result.unwrap_or_else(|_| panic!("{label}: load panicked"));

            assert!(
                matches!(
                    load_result,
                    Err(ModelLoadError::ClipLibraryUnreadable { .. })
                ),
                "{label}: got {load_result:?}"
            );
        }
    }

    #[test]
    fn library_lacking_the_mesh_root_fails_the_load() {
        let error = load_err(
            bound_mesh(),
            library(vec![node("hips").at(Vec3::Y), node("arm").under("hips")]).clip(clip("idle")),
        );

        assert!(
            matches!(&error, ModelLoadError::ClipLibraryRootMissing { joint, .. } if joint == "pelvis"),
            "{error:?}"
        );
    }

    fn forest_library() -> Document {
        library(vec![root_at(1.0), node("prop_root").at(Vec3::Y)]).clip(clip("idle"))
    }

    #[test]
    fn skinned_mesh_with_no_selected_root_fails_the_load() {
        let forest = || skinned(vec![root_at(1.0), node("prop_root").at(Vec3::Y)]);
        let error = load_err(forest(), forest_library());
        assert!(
            matches!(error, ModelLoadError::ClipLibraryNoRoot { .. }),
            "undeclared forest: {error:?}"
        );

        let error = load_err(bound_mesh().declared_root("arm"), forest_library());
        assert!(
            matches!(error, ModelLoadError::ClipLibraryNoRoot { .. }),
            "declared non-root: {error:?}"
        );
    }

    #[test]
    fn forest_with_a_declared_root_binds() {
        let model = load_pair(
            skinned(vec![root_at(1.0), node("prop_root").at(Vec3::Y)]).declared_root("pelvis"),
            forest_library(),
        );

        assert_eq!(clip_names(&model), ["idle"]);
    }

    #[test]
    fn non_positive_root_height_fails_the_load() {
        for (mesh_height, library_height) in [(0.0, 1.0), (-1.0, 1.0), (1.0, 0.0), (1.0, -1.0)] {
            let error = load_err(
                skinned(vec![root_at(mesh_height)]),
                library(vec![root_at(library_height)]).clip(clip("idle")),
            );

            assert!(
                matches!(error, ModelLoadError::ClipLibraryRootHeight { .. }),
                "mesh {mesh_height}, library {library_height}: {error:?}"
            );
        }
    }

    // Regression: a ratio of zero passed the height check and flattened root
    // translation, whether from underflow or a library height parsed to infinity.
    #[test]
    fn root_heights_with_no_usable_ratio_fail_the_load() {
        for (mesh_height, library_height) in [(1.0e-30, 1.0e30), (1.0e30, 1.0e-30)] {
            let error = load_err(
                skinned(vec![root_at(mesh_height)]),
                library(vec![root_at(library_height)]).clip(clip("idle")),
            );

            assert!(
                matches!(error, ModelLoadError::ClipLibraryRootHeight { .. }),
                "mesh {mesh_height}, library {library_height}: {error:?}"
            );
        }

        let (dir, path) = write_pair(
            skinned(vec![root_at(1.0)]),
            library(vec![root_at(1.0)]).clip(clip("idle")),
        );
        patch_document(&dir.path().join(LIBRARY), |root| {
            root["nodes"][0]["translation"][1] = json!(1.0e39);
        });
        let error = load_model(&path).unwrap_err();
        assert!(
            matches!(
                error,
                ModelLoadError::ClipLibraryRootHeight { library_height, .. }
                    if library_height == f32::INFINITY
            ),
            "{error:?}"
        );
    }

    #[test]
    fn unreadable_library_accessor_skips_its_channel_and_keeps_the_clip() {
        let lib = library(vec![root_at(1.0), node("arm").under("pelvis")]).clip(
            clip("sway")
                .rotation(
                    "arm",
                    &[0.0, 1.0],
                    &[Quat::IDENTITY, Quat::from_rotation_y(0.3)],
                )
                .translation("pelvis", &[0.0, 1.0], &[Vec3::Y, Vec3::Y * 1.1]),
        );
        let (dir, path) = write_pair(bound_mesh(), lib);
        patch_document(&dir.path().join(LIBRARY), |root| {
            let output = root["animations"][0]["samplers"][1]["output"]
                .as_u64()
                .unwrap() as usize;
            root["accessors"][output]["count"] = json!(0);
        });
        let capture = LogCapture::start();
        let model = load_model(&path).unwrap();

        let clip = named(&model, "sway");
        assert!(clip.joints[0].translation.values().is_empty());
        assert_eq!(clip.joints[1].rotation.values().len(), 2);
        capture.assert_logged_once(Level::Warn, "output accessor has no elements");
    }

    #[test]
    fn library_with_no_clips_fails_only_a_model_with_none() {
        let empty = || library(vec![root_at(1.0), node("arm").under("pelvis")]);
        let error = load_err(bound_mesh(), empty());
        assert!(
            matches!(error, ModelLoadError::ClipLibraryEmpty { .. }),
            "{error:?}"
        );

        let model = load_pair(embedding_model(), empty());
        assert_eq!(clip_names(&model), ["embedded_only", "shared"]);
    }

    #[test]
    fn malformed_reference_warns_and_keeps_tags_and_embedded_clips() {
        let dir = tempfile::tempdir().unwrap();
        turning_library().write(&dir.path().join(LIBRARY));
        for value in [
            json!(7),
            json!(null),
            json!(["library.gltf"]),
            json!(""),
            json!("/library.gltf"),
            json!("file:library.gltf"),
            json!("http://x"),
            json!("C:lib.gltf"),
            json!("file%3Alibrary.gltf"),
            json!("clips\\library.gltf"),
            json!("clips%5Clibrary.gltf"),
            json!("library%FF.gltf"),
        ] {
            let path = embedding_model()
                .extras(json!({ "tags": ["armored"], "clipLibrary": value }))
                .write(&dir.path().join("model.gltf"));
            let capture = LogCapture::start();
            let model = load_model(&path).unwrap_or_else(|error| panic!("{value}: {error}"));

            capture.assert_logged_once(Level::Warn, "malformed clipLibrary");
            assert_eq!(clip_names(&model), ["embedded_only", "shared"], "{value}");
            assert_eq!(model.tags, ["armored"], "{value}");
        }
    }

    #[test]
    fn relative_reference_may_climb_out_of_the_model_directory() {
        let dir = tempfile::tempdir().unwrap();
        turning_library().write(&dir.path().join("clips/shared lib.gltf"));
        let path = bound_mesh()
            .clip_library("../clips/shared%20lib.gltf")
            .write(&dir.path().join("trooper/model.gltf"));
        let capture = LogCapture::start();

        assert_eq!(
            clip_names(&load_model(&path).unwrap()),
            ["shared", "library_only"]
        );
        capture.assert_nothing_above(Level::Info);
    }

    #[test]
    fn static_model_warns_and_ignores_even_an_unreadable_library() {
        let dir = tempfile::tempdir().unwrap();
        let path = rigid()
            .clip_library("absent.gltf")
            .write(&dir.path().join("model.gltf"));
        let capture = LogCapture::start();

        assert!(read_model_sources(&path).unwrap().library().is_none());
        capture.assert_logged_once(Level::Warn, "names clip library");
        assert!(load_model(&path).unwrap().clips.is_empty());
    }

    // --- Cache ---------------------------------------------------------------

    #[test]
    fn parse_binds_the_bytes_read_even_if_the_library_changes_after() {
        let (dir, path) = write_pair(bound_mesh(), turning_library());
        let fresh = load_model(&path).unwrap();
        let library_path: &Path = &dir.path().join(LIBRARY);

        let edited_read = read_model_sources(&path).unwrap();
        library(vec![root_at(1.0), node("arm").under("pelvis")])
            .clip(clip("edited"))
            .write(library_path);
        let edited = load_model_from_sources(&path, edited_read).unwrap();
        assert_eq!(edited.clips, fresh.clips, "an edit after the read");

        turning_library().write(library_path);
        let deleted_read = read_model_sources(&path).unwrap();
        std::fs::remove_file(library_path).unwrap();
        std::fs::remove_file(library_path.with_extension("bin")).unwrap();
        let deleted = load_model_from_sources(&path, deleted_read).unwrap();
        assert_eq!(deleted.clips, fresh.clips, "a delete after the read");
    }
}
