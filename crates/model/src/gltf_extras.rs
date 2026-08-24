// glTF `extras` parsing for model metadata.
// See: context/lib/resource_management.md §7

//! Raw JSON shapes and non-fatal degradation rules for model metadata.
//!
//! This module owns the raw JSON shapes and their non-fatal degradation rules.
//! The loader owns topology-dependent interpretation and skeleton assembly.

use glam::Vec3;
use serde::Deserialize;

use crate::mount::MountAxes;

/// A skeletal hit zone authored on a joint node's per-node `extras`. Read at
/// load time and carried parallel to the loaded skeleton. A radius is carried
/// only when authored as a positive finite meter value; absent or invalid radii
/// degrade to `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct JointZone {
    /// Author-supplied zone tag (e.g. "head", "torso").
    pub tag: String,
    /// Optional positive finite zone radius in meters. `None` when the joint
    /// node omits `hitZoneRadius` or authors an invalid radius; the consumer
    /// applies its own default.
    pub radius: Option<f32>,
}

/// The per-node `extras.mount` shape. The core axes deserialize separately
/// from the optional Euler metadata so a malformed Euler never hides valid
/// author intent.
#[derive(Debug, Deserialize)]
struct NodeMountExtras {
    mount: Option<MountExtras>,
}

#[derive(Debug, Deserialize)]
struct MountExtras {
    barrel: Option<[f32; 3]>,
    up: Option<[f32; 3]>,
    #[serde(default)]
    euler: Option<serde_json::Value>,
}

/// Read raw-source weapon axes from a selected mesh node's `extras.mount`.
///
/// Barrel and up are one core pair: a missing, malformed, non-finite,
/// degenerate, or non-orthogonal member degrades the whole declaration to
/// `None`. Euler metadata is independently optional, so malformed Euler data
/// does not hide a valid barrel/up declaration. Metadata never rejects a model.
pub(crate) fn read_mount_axes(extras: &gltf::json::Extras) -> Option<MountAxes> {
    let raw = extras.as_ref()?;
    let parsed = serde_json::from_str::<NodeMountExtras>(raw.get()).ok()?;
    let mount = parsed.mount?;
    let barrel = normalized_mount_axis(mount.barrel?)?;
    let up = normalized_mount_axis(mount.up?)?;
    (barrel.dot(up).abs() <= 1.0e-3).then_some(MountAxes {
        barrel,
        up,
        euler: valid_mount_euler(mount.euler.as_ref()),
    })
}

fn normalized_mount_axis(axis: [f32; 3]) -> Option<Vec3> {
    let axis = Vec3::from_array(axis);
    let length_squared = axis.length_squared();
    (axis.is_finite() && length_squared.is_finite() && length_squared > 1.0e-12)
        .then(|| axis.normalize())
}

fn valid_mount_euler(value: Option<&serde_json::Value>) -> Option<[f32; 3]> {
    let values = value?.as_array()?;
    let values: [f32; 3] = values
        .iter()
        .map(|value| value.as_f64().map(|value| value as f32))
        .collect::<Option<Vec<_>>>()?
        .try_into()
        .ok()?;
    values
        .iter()
        .all(|value| value.is_finite())
        .then_some(values)
}

/// The shape of the document's top-level `extras` this loader cares about.
/// Unknown keys are ignored (no `deny_unknown_fields`) so authors can stash
/// arbitrary metadata alongside `tags`; `tags` defaults to empty when absent.
#[derive(Debug, Deserialize)]
struct ModelExtras {
    #[serde(default)]
    tags: Vec<String>,
}

/// Read the entity tags off the document's top-level `extras`.
///
/// The `extras` feature surfaces the raw JSON as `&Option<Box<RawValue>>`. Absent
/// `extras` → no tags. Present `extras` deserializes into [`ModelExtras`]; any
/// deserialize failure (wrong shape, non-array `tags`, etc.) also yields no tags.
/// Tags are author metadata, not load-critical data — a garbled `extras` must not
/// fail the load, so every error arm collapses to an empty list.
pub(crate) fn read_model_tags(extras: &gltf::json::Extras) -> Vec<String> {
    let Some(raw) = extras.as_ref() else {
        return Vec::new();
    };
    match serde_json::from_str::<ModelExtras>(raw.get()) {
        Ok(parsed) => parsed.tags,
        Err(_) => Vec::new(),
    }
}

/// The shape of a joint node's per-node `extras` this loader cares about.
/// Unknown keys are ignored so authors can stash arbitrary metadata; the zone
/// is meaningful only when `hitZone` is present (see [`read_joint_zone`]).
#[derive(Debug, Deserialize)]
struct JointZoneExtras {
    #[serde(rename = "hitZone")]
    hit_zone: Option<String>,
    /// Radius in meters. Invalid optional values degrade to no authored radius.
    #[serde(rename = "hitZoneRadius")]
    hit_zone_radius: Option<serde_json::Value>,
}

/// Read a single joint node's hit zone off its per-node `extras`
/// (`gltf::Node::extras()` — NOT the document-level extras).
///
/// Absent `extras`, a deserialize failure (wrong shape), or a missing `hitZone`
/// tag all yield `None` — a zone is author metadata, not load-critical data, so
/// a garbled value degrades to no zone for that joint rather than failing the
/// load. The radius is carried only when positive and finite; otherwise the
/// zone keeps its tag and degrades to no authored radius.
pub(crate) fn read_joint_zone(extras: &gltf::json::Extras) -> Option<JointZone> {
    let raw = extras.as_ref()?;
    let parsed = serde_json::from_str::<JointZoneExtras>(raw.get()).ok()?;
    let tag = parsed.hit_zone?;
    Some(JointZone {
        tag,
        radius: valid_hit_zone_radius(parsed.hit_zone_radius.as_ref()),
    })
}

/// Read a named socket off a node's per-node `extras`.
///
/// Sockets are optional author metadata, so an absent tag yields `None` and a
/// malformed value warns before degrading to `None`. The loader owns whether a
/// valid socket name is legal for the selected model topology.
pub(crate) fn read_socket_name(
    extras: &gltf::json::Extras,
    node_index: usize,
    path_str: &str,
) -> Option<String> {
    let raw = extras.as_ref()?;
    let value = match serde_json::from_str::<serde_json::Value>(raw.get()) {
        Ok(value) => value,
        Err(_) => {
            log::warn!(
                "[Model] malformed socket metadata on node {node_index} in {path_str}; ignoring"
            );
            return None;
        }
    };
    let Some(object) = value.as_object() else {
        log::warn!(
            "[Model] malformed socket metadata on node {node_index} in {path_str}; expected an object; ignoring"
        );
        return None;
    };
    let socket = object.get("socket")?;
    let Some(socket) = socket.as_str() else {
        log::warn!(
            "[Model] malformed socket on node {node_index} in {path_str}; expected a string; ignoring"
        );
        return None;
    };
    if socket.is_empty() {
        log::warn!(
            "[Model] malformed socket on node {node_index} in {path_str}; expected a non-empty string; ignoring"
        );
        return None;
    }
    Some(socket.to_string())
}

fn valid_hit_zone_radius(value: Option<&serde_json::Value>) -> Option<f32> {
    let radius = value?.as_f64()? as f32;
    (radius.is_finite() && radius > 0.0).then_some(radius)
}

/// Which spelling family a leg/foot tag was authored with. A model uses ONE
/// family for its leg set; mixing side and indexed names warns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LegNameFamily {
    /// Biped side names `legL`/`legR`/`footL`/`footR`.
    Side,
    /// General N-leg names `leg{i}`/`foot{i}`.
    Indexed,
}

/// The leg-set slot a leg/foot tag references: the leg index (side names map
/// L=0, R=1; indexed names map `{i}` → `i`) and the spelling family it came
/// from (for the mixed-family warning).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LegRef {
    pub(crate) index: usize,
    pub(crate) family: LegNameFamily,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct JointPoseMetadata {
    pub(crate) aim_spine: bool,
    pub(crate) upper_body: bool,
    pub(crate) lower_body: bool,
    pub(crate) aim_bend_weight: f32,
    /// This joint belongs to the named leg's hip→knee→ankle chain (`legL`/
    /// `legR`/`leg{i}`).
    pub(crate) leg_chain: Option<LegRef>,
    /// Conflicting `leg*` names were declared on this node. The membership is
    /// invalid rather than whichever array entry happened to appear last.
    pub(crate) leg_chain_conflict: bool,
    /// This joint is the named leg's foot/ankle target — the IK end effector
    /// (`footL`/`footR`/`foot{i}`).
    pub(crate) foot_target: Option<LegRef>,
    /// Conflicting `foot*` names were declared on this node.
    pub(crate) foot_target_conflict: bool,
    /// Family sightings survive invalid same-node declarations so the loader
    /// still diagnoses a model that mixes side and indexed names.
    pub(crate) saw_side_leg_name: bool,
    pub(crate) saw_indexed_leg_name: bool,
}

/// Read convention-named pose metadata from one joint node's `extras`.
///
/// `poseMask` accepts either one string or an array of strings. Invalid array
/// members and unknown names are diagnosed independently, allowing valid
/// memberships in the same array to survive. Optional metadata never fails the
/// model load.
pub(crate) fn read_pose_masks(
    extras: &gltf::json::Extras,
    node_index: usize,
    path_str: &str,
) -> JointPoseMetadata {
    let mut metadata = JointPoseMetadata {
        aim_bend_weight: 1.0,
        ..JointPoseMetadata::default()
    };
    let Some(raw) = extras.as_ref() else {
        return metadata;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw.get()) else {
        log::warn!(
            "[Model] malformed pose metadata on joint node {node_index} in {path_str}; ignoring"
        );
        return metadata;
    };
    let Some(object) = value.as_object() else {
        log::warn!(
            "[Model] malformed pose metadata on joint node {node_index} in {path_str}; expected an object; ignoring"
        );
        return metadata;
    };
    let Some(pose_mask) = object.get("poseMask") else {
        return metadata;
    };

    {
        let mut read_name = |name: &str| match name {
            "aimSpine" => metadata.aim_spine = true,
            "upperBody" => metadata.upper_body = true,
            "lowerBody" => metadata.lower_body = true,
            "legL" => record_leg_chain(
                &mut metadata,
                LegRef {
                    index: 0,
                    family: LegNameFamily::Side,
                },
                node_index,
                path_str,
            ),
            "legR" => record_leg_chain(
                &mut metadata,
                LegRef {
                    index: 1,
                    family: LegNameFamily::Side,
                },
                node_index,
                path_str,
            ),
            "footL" => record_foot_target(
                &mut metadata,
                LegRef {
                    index: 0,
                    family: LegNameFamily::Side,
                },
                node_index,
                path_str,
            ),
            "footR" => record_foot_target(
                &mut metadata,
                LegRef {
                    index: 1,
                    family: LegNameFamily::Side,
                },
                node_index,
                path_str,
            ),
            unknown => {
                if let Some(index) = indexed_leg_suffix(unknown, "leg") {
                    record_leg_chain(
                        &mut metadata,
                        LegRef {
                            index,
                            family: LegNameFamily::Indexed,
                        },
                        node_index,
                        path_str,
                    );
                } else if let Some(index) = indexed_leg_suffix(unknown, "foot") {
                    record_foot_target(
                        &mut metadata,
                        LegRef {
                            index,
                            family: LegNameFamily::Indexed,
                        },
                        node_index,
                        path_str,
                    );
                } else {
                    log::warn!(
                        "[Model] unknown poseMask '{unknown}' on joint node {node_index} in {path_str}; ignoring"
                    );
                }
            }
        };

        match pose_mask {
            serde_json::Value::String(name) => read_name(name),
            serde_json::Value::Array(names) => {
                if names.is_empty() {
                    log::warn!(
                        "[Model] empty poseMask array on joint node {node_index} in {path_str}; ignoring"
                    );
                }
                for name in names {
                    if let Some(name) = name.as_str() {
                        read_name(name);
                    } else {
                        log::warn!(
                            "[Model] non-string poseMask array value on joint node {node_index} in {path_str}; ignoring"
                        );
                    }
                }
            }
            _ => log::warn!(
                "[Model] malformed poseMask on joint node {node_index} in {path_str}; expected string or string array; ignoring"
            ),
        }
    }

    if metadata.aim_spine {
        if let Some(weight_value) = object.get("aimBendWeight") {
            let valid_weight = weight_value.as_f64().and_then(|weight| {
                let weight = weight as f32;
                (weight.is_finite() && weight > 0.0).then_some(weight)
            });
            if let Some(weight) = valid_weight {
                metadata.aim_bend_weight = weight;
            } else {
                log::warn!(
                    "[Model] invalid aimBendWeight on joint node {node_index} in {path_str}; using 1.0"
                );
            }
        }
    }

    metadata
}

fn record_leg_chain(
    metadata: &mut JointPoseMetadata,
    leg: LegRef,
    node_index: usize,
    path_str: &str,
) {
    note_metadata_family(metadata, leg.family);
    record_leg_ref(
        &mut metadata.leg_chain,
        &mut metadata.leg_chain_conflict,
        leg,
        "leg chain",
        node_index,
        path_str,
    );
}

fn record_foot_target(
    metadata: &mut JointPoseMetadata,
    foot: LegRef,
    node_index: usize,
    path_str: &str,
) {
    note_metadata_family(metadata, foot.family);
    record_leg_ref(
        &mut metadata.foot_target,
        &mut metadata.foot_target_conflict,
        foot,
        "foot target",
        node_index,
        path_str,
    );
}

fn note_metadata_family(metadata: &mut JointPoseMetadata, family: LegNameFamily) {
    match family {
        LegNameFamily::Side => metadata.saw_side_leg_name = true,
        LegNameFamily::Indexed => metadata.saw_indexed_leg_name = true,
    }
}

fn record_leg_ref(
    slot: &mut Option<LegRef>,
    conflict: &mut bool,
    authored: LegRef,
    kind: &str,
    node_index: usize,
    path_str: &str,
) {
    if *conflict {
        return;
    }
    match *slot {
        None => *slot = Some(authored),
        Some(existing) if existing == authored => {}
        Some(existing) => {
            log::warn!(
                "[Model] conflicting {kind} poseMask tags {existing:?} and {authored:?} on joint node {node_index} in {path_str}; dropping this joint's {kind} membership"
            );
            *conflict = true;
            *slot = None;
        }
    }
}

/// Parse an indexed leg/foot tag: `prefix` immediately followed by a base-10
/// index, e.g. `leg0`, `foot12`. Returns the index, or `None` when the name is
/// not `prefix` followed purely by ASCII digits — so the biped side names
/// `legL`/`legR`/`footL`/`footR` fall through to their own match arms rather
/// than being misread as indexed names.
fn indexed_leg_suffix(name: &str, prefix: &str) -> Option<usize> {
    let rest = name.strip_prefix(prefix)?;
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse::<usize>().ok()
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::{read_mount_axes, read_socket_name};

    #[test]
    fn read_socket_name_rejects_empty_socket_tag() {
        // Regression: empty socket names cannot be referenced by descriptors.
        let raw: Box<serde_json::value::RawValue> =
            serde_json::from_str(r#"{"socket":""}"#).expect("test raw JSON parses");
        let extras: gltf::json::Extras = Some(raw);

        assert_eq!(read_socket_name(&extras, 7, "model.gltf"), None);
    }

    #[test]
    fn read_mount_axes_normalizes_core_pair_and_keeps_valid_euler() {
        let raw: Box<serde_json::value::RawValue> =
            serde_json::from_str(r#"{"mount":{"barrel":[0,3,0],"up":[0,0,4],"euler":[10,20,30]}}"#)
                .expect("test raw JSON parses");
        let extras: gltf::json::Extras = Some(raw);

        let mount = read_mount_axes(&extras).expect("valid core pair loads");
        assert_eq!(mount.barrel, Vec3::Y);
        assert_eq!(mount.up, Vec3::Z);
        assert_eq!(mount.euler, Some([10.0, 20.0, 30.0]));
    }

    #[test]
    fn read_mount_axes_keeps_core_pair_when_euler_is_malformed() {
        let raw: Box<serde_json::value::RawValue> = serde_json::from_str(
            r#"{"mount":{"barrel":[1,0,0],"up":[0,1,0],"euler":[0,"bad",0]}}"#,
        )
        .expect("test raw JSON parses");
        let extras: gltf::json::Extras = Some(raw);

        let mount = read_mount_axes(&extras).expect("valid core pair remains available");
        assert_eq!(mount.euler, None);
    }

    #[test]
    fn read_mount_axes_requires_both_valid_core_axes() {
        for raw in [
            r#"{"mount":{"barrel":[1,0,0]}}"#,
            r#"{"mount":{"barrel":[1,0,0],"up":[0,0,0]}}"#,
            r#"{"mount":{"barrel":[1,0,0],"up":[1,0,0]}}"#,
            r#"{"mount":{"barrel":"bad","up":[0,1,0]}}"#,
        ] {
            let raw: Box<serde_json::value::RawValue> =
                serde_json::from_str(raw).expect("test raw JSON parses");
            let extras: gltf::json::Extras = Some(raw);
            assert!(
                read_mount_axes(&extras).is_none(),
                "invalid core axes {extras:?} degrade to no mount declaration",
            );
        }
    }

    #[test]
    fn read_mount_axes_rejects_finite_axes_whose_length_squared_overflows() {
        // Regression: normalizing an overflowed finite axis surfaced Vec3::ZERO
        // as declared mount metadata instead of degrading the declaration.
        let raw: Box<serde_json::value::RawValue> = serde_json::from_str(
            r#"{"mount":{"barrel":[3e38,3e38,0],"up":[0,0,1],"euler":[0,0,0]}}"#,
        )
        .expect("test raw JSON parses");
        let extras: gltf::json::Extras = Some(raw);

        assert_eq!(read_mount_axes(&extras), None);
    }
}
