// glTF animation parsing: keyframe validation, clip parsing, and root-motion derivation.
// See: context/lib/rendering_pipeline.md §9

use std::collections::HashMap;

use glam::{Quat, Vec3};
use gltf::accessor::{DataType, Dimensions};

use super::gltf_loader::format_accessor_shape;
use super::skeleton::{AnimationClip, Interp, JointTracks, Track};

/// Minimum net horizontal (XZ) root-motion displacement, in ground units, for a
/// clip to report an authored travel speed. Below this a clip counts as
/// in-place (an idle, turn, or strafe with no forward travel): deriving a speed
/// from sub-millimeter drift would make a later locomotion-calibration step
/// divide the stride rate by ≈0, so such clips yield `None` instead and fall
/// back to a degenerate reference. One millimeter is far below any authored
/// stride yet safely above float round-off in a static root track.
const MIN_TRAVEL_XZ_DISPLACEMENT: f32 = 1.0e-3;

/// Resolve a channel's raw output elements + sampler interpolation into the
/// keyframe `values` and engine [`Interp`] mode the track stores. Returns `None`
/// when the channel must be skipped (its joint then holds its rest pose, the
/// same as an absent channel).
///
/// - LINEAR / STEP map directly to [`Interp::Linear`] / [`Interp::Step`]; the raw
///   outputs are the per-keyframe values (one element per input time). If that
///   one-to-one shape does not hold, the channel is malformed, so we warn and
///   skip it to preserve the parallel-length invariant `Track` requires.
/// - CUBICSPLINE stores three elements per keyframe — `[in-tangent, value,
///   out-tangent]` — so the value of keyframe `k` is `raw[3k + 1]`. We extract
///   those, discard the tangents, store the track as `Linear` (degrading cubic to
///   linear; tangent storage and hermite blending are not implemented), and warn.
///   If the triple shape does not hold (`raw.len() != 3 * key_count`) the channel
///   is malformed for cubic, so we warn and skip it — this also guards the
///   parallel-length invariant `Track` requires (otherwise 3N values would be
///   paired with N times).
fn resolve_keyframes<T: Copy>(
    raw: Vec<T>,
    key_count: usize,
    interpolation: gltf::animation::Interpolation,
    clip_name: &str,
    channel_kind: &str,
) -> Option<(Vec<T>, Interp)> {
    use gltf::animation::Interpolation;
    match interpolation {
        Interpolation::Linear => {
            if raw.len() != key_count {
                log::warn!(
                    "clip '{clip_name}' {channel_kind} channel: LINEAR output count {} \
                     does not match its {key_count} keyframes; skipping channel (joint holds rest pose)",
                    raw.len()
                );
                return None;
            }
            Some((raw, Interp::Linear))
        }
        Interpolation::Step => {
            if raw.len() != key_count {
                log::warn!(
                    "clip '{clip_name}' {channel_kind} channel: STEP output count {} \
                     does not match its {key_count} keyframes; skipping channel (joint holds rest pose)",
                    raw.len()
                );
                return None;
            }
            Some((raw, Interp::Step))
        }
        Interpolation::CubicSpline => {
            if raw.len() != key_count * 3 {
                log::warn!(
                    "clip '{clip_name}' {channel_kind} channel: CUBICSPLINE output count {} \
                     is not 3x its {key_count} keyframes; skipping channel (joint holds rest pose)",
                    raw.len()
                );
                return None;
            }
            log::warn!(
                "clip '{clip_name}' {channel_kind} channel: CUBICSPLINE not supported; \
                 extracting keyframe values and degrading to LINEAR (tangents discarded)"
            );
            // Each triple is [in-tangent, value, out-tangent]; keep the value.
            let values: Vec<T> = (0..key_count).map(|k| raw[3 * k + 1]).collect();
            Some((values, Interp::Linear))
        }
    }
}

fn validate_key_times(times: &[f32], clip_name: &str) -> bool {
    for (i, &time) in times.iter().enumerate() {
        if !time.is_finite() {
            log::warn!(
                "clip '{clip_name}' animation channel: input time at key {i} is not finite; \
                 skipping channel (joint holds rest pose)"
            );
            return false;
        }
    }
    for (i, pair) in times.windows(2).enumerate() {
        if pair[0] >= pair[1] {
            log::warn!(
                "clip '{clip_name}' animation channel: input times are not strictly \
                 ascending at keys {i} and {}; skipping channel (joint holds rest pose)",
                i + 1
            );
            return false;
        }
    }
    true
}

fn vec3_is_finite(v: Vec3) -> bool {
    v.x.is_finite() && v.y.is_finite() && v.z.is_finite()
}

fn validate_vec3_keyframes(values: &[Vec3], clip_name: &str, channel_kind: &str) -> bool {
    for (i, &value) in values.iter().enumerate() {
        if !vec3_is_finite(value) {
            log::warn!(
                "clip '{clip_name}' {channel_kind} channel: output value at key {i} \
                 is not finite; skipping channel (joint holds rest pose)"
            );
            return false;
        }
    }
    true
}

fn quat_length_squared(q: Quat) -> f32 {
    q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w
}

fn validate_rotation_keyframes(values: &[Quat], clip_name: &str) -> bool {
    for (i, &value) in values.iter().enumerate() {
        let len_sq = quat_length_squared(value);
        if !len_sq.is_finite() {
            log::warn!(
                "clip '{clip_name}' rotation channel: output quaternion at key {i} \
                 is not finite; skipping channel (joint holds rest pose)"
            );
            return false;
        }
        if len_sq <= 0.0 {
            log::warn!(
                "clip '{clip_name}' rotation channel: output quaternion at key {i} \
                 has zero length; skipping channel (joint holds rest pose)"
            );
            return false;
        }
    }
    true
}

fn build_track<T>(
    times: Vec<f32>,
    values: Vec<T>,
    mode: Interp,
    clip_name: &str,
    channel_kind: &str,
) -> Option<Track<T>> {
    match Track::new(times, values, mode) {
        Ok(track) => Some(track),
        Err(error) => {
            log::warn!(
                "clip '{clip_name}' {channel_kind} channel: invalid keyframe track \
                 ({error:?}); skipping channel (joint holds rest pose)"
            );
            None
        }
    }
}

/// Human label for an animation channel's target property, for warn messages.
fn animation_channel_kind(property: gltf::animation::Property) -> &'static str {
    use gltf::animation::Property;
    match property {
        Property::Translation => "translation",
        Property::Rotation => "rotation",
        Property::Scale => "scale",
        Property::MorphTargetWeights => "morph weights",
    }
}

/// True when the sampler OUTPUT accessor's `(component type, dimension)` is one
/// the target property (and gltf's `read_outputs`) can decode. glTF permits
/// normalized integer component types for rotation and morph-weight outputs
/// (the same widening the mesh path already allows for `WEIGHTS_0`); translation
/// and scale are `F32 VEC3` only. An accessor outside this set would panic inside
/// gltf's `read_outputs` (an `unreachable!` for an unsupported component type —
/// in release too — or an internal size `debug_assert!`), so the caller
/// validates first and degrades-and-skips the channel.
fn animation_output_shape_ok(
    accessor: &gltf::Accessor,
    property: gltf::animation::Property,
) -> bool {
    use gltf::animation::Property;
    let shape = (accessor.data_type(), accessor.dimensions());
    match property {
        Property::Translation | Property::Scale => shape == (DataType::F32, Dimensions::Vec3),
        Property::Rotation => matches!(
            shape,
            (DataType::F32, Dimensions::Vec4)
                | (DataType::U8, Dimensions::Vec4)
                | (DataType::U16, Dimensions::Vec4)
                | (DataType::I8, Dimensions::Vec4)
                | (DataType::I16, Dimensions::Vec4)
        ),
        Property::MorphTargetWeights => matches!(
            shape,
            (DataType::F32, Dimensions::Scalar)
                | (DataType::U8, Dimensions::Scalar)
                | (DataType::U16, Dimensions::Scalar)
                | (DataType::I8, Dimensions::Scalar)
                | (DataType::I16, Dimensions::Scalar)
        ),
    }
}

/// Why gltf cannot safely read `accessor`, or `None` when it can. Call once the
/// accessor's shape is known good, so `size()` is the element size gltf reads.
///
/// `gltf::accessor::Iter` trusts what document validation leaves unchecked. A
/// stride below the element size panics on the first read, in release too. Its
/// range arithmetic overflows on a zero count or a huge one. A sparse accessor
/// with no buffer view allocates `count` elements whatever the file's size, so
/// it is refused; one with a buffer view reads, its own ranges checked.
fn accessor_read_refusal(
    accessor: &gltf::Accessor,
    buffers: &[gltf::buffer::Data],
) -> Option<&'static str> {
    if accessor.count() == 0 {
        return Some("has no elements");
    }
    let Some(view) = accessor.view() else {
        return Some("has no buffer view");
    };
    let element_size = accessor.size();
    let base = view_read_refusal(
        &view,
        accessor.offset(),
        accessor.count(),
        element_size,
        buffers,
    );
    if base.is_some() {
        return base;
    }
    let sparse = accessor.sparse()?;
    if sparse.count() == 0 || sparse.count() > accessor.count() {
        return Some("has a sparse count outside its elements");
    }
    let indices = sparse.indices();
    let values = sparse.values();
    view_read_refusal(
        &indices.view(),
        indices.offset(),
        sparse.count(),
        indices.index_type().size(),
        buffers,
    )
    .or_else(|| {
        view_read_refusal(
            &values.view(),
            values.offset(),
            sparse.count(),
            element_size,
            buffers,
        )
    })
}

/// Why `count` elements at `offset` in `view` cannot be read, if they cannot.
fn view_read_refusal(
    view: &gltf::buffer::View,
    offset: usize,
    count: usize,
    element_size: usize,
    buffers: &[gltf::buffer::Data],
) -> Option<&'static str> {
    let stride = view.stride().unwrap_or(element_size);
    if stride < element_size {
        return Some("has a byte stride below its element size");
    }
    let read_end = (count - 1)
        .checked_mul(stride)
        .and_then(|span| span.checked_add(element_size))
        .and_then(|span| span.checked_add(offset));
    let view_end = view.offset().checked_add(view.length());
    let buffer_length = buffers.get(view.buffer().index()).map(|data| data.0.len());
    match (read_end, view_end, buffer_length) {
        (Some(read_end), Some(view_end), Some(buffer_length))
            if read_end <= view.length() && view_end <= buffer_length =>
        {
            None
        }
        _ => Some("overruns its buffer"),
    }
}

/// A clip's tracks as authored, before root-motion derivation. A distinct type
/// so each clip derives exactly once, after anything that rewrites its tracks.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedClip {
    pub(crate) name: String,
    pub(crate) duration: f32,
    pub(crate) joints: Vec<JointTracks>,
}

impl ParsedClip {
    /// Derive travel speed and neutralize root drift, finishing the clip.
    pub(crate) fn derive_root_motion(
        mut self,
        locomotion_root: Option<usize>,
        path_str: &str,
    ) -> AnimationClip {
        let travel_speed = derive_and_neutralize_root_motion(
            &mut self.joints,
            locomotion_root,
            self.duration,
            &self.name,
            path_str,
        );
        AnimationClip {
            name: self.name,
            duration: self.duration,
            joints: self.joints,
            travel_speed,
        }
    }
}

/// Parse one animation clip into per-joint TRS tracks, one entry per slot of
/// `joint_count`. `node_to_topo` maps channel target nodes to those slots;
/// channels targeting any other node are skipped.
pub(crate) fn parse_clip(
    anim: &gltf::Animation,
    buffers: &[gltf::buffer::Data],
    node_to_topo: &HashMap<usize, usize>,
    joint_count: usize,
    path_str: &str,
) -> ParsedClip {
    let buffer_data = |buffer: gltf::Buffer| buffers.get(buffer.index()).map(|d| &d.0[..]);
    let name = anim.name().unwrap_or("").to_string();
    let mut joints = vec![JointTracks::default(); joint_count];
    let mut duration = 0.0f32;

    for channel in anim.channels() {
        let target_node = channel.target().node().index();
        // Animation channels target joints by node index; remap to topo order.
        let Some(&topo_idx) = node_to_topo.get(&target_node) else {
            continue;
        };
        if topo_idx >= joints.len() {
            continue;
        }

        // The sampler's interpolation algorithm drives how the runtime blends
        // keyframes: LINEAR/STEP map straight to [`Interp`], while CUBICSPLINE is
        // degraded to LINEAR by extracting each key's value element (tangents
        // discarded). True cubic evaluation is out of scope: tangent storage and
        // hermite blending are not implemented.
        let property = channel.target().property();
        let kind = animation_channel_kind(property);
        let sampler = channel.sampler();
        let interpolation = sampler.interpolation();

        // Validate the sampler accessors BEFORE reading, shape first, then
        // readability. `read_inputs`/`read_outputs` check neither: gltf panics on
        // shapes it cannot decode (an `unreachable!` for an unsupported output
        // component type — in release too — or an internal size `debug_assert!`)
        // and on the accessors `accessor_read_refusal` names. A failure
        // degrades-and-skips the channel — the same policy malformed channels
        // follow below (the joint holds its rest pose).
        let input_accessor = sampler.input();
        if (input_accessor.data_type(), input_accessor.dimensions())
            != (DataType::F32, Dimensions::Scalar)
        {
            log::warn!(
                "clip '{name}' {kind} channel: input accessor shape {} is not F32 Scalar; \
                 skipping channel (joint holds rest pose)",
                format_accessor_shape((input_accessor.data_type(), input_accessor.dimensions())),
            );
            continue;
        }
        let output_accessor = sampler.output();
        if !animation_output_shape_ok(&output_accessor, property) {
            log::warn!(
                "clip '{name}' {kind} channel: output accessor shape {} is unsupported for this \
                 property; skipping channel (joint holds rest pose)",
                format_accessor_shape((output_accessor.data_type(), output_accessor.dimensions())),
            );
            continue;
        }
        let refusal = [("input", &input_accessor), ("output", &output_accessor)]
            .into_iter()
            .find_map(|(role, accessor)| {
                accessor_read_refusal(accessor, buffers).map(|reason| (role, reason))
            });
        if let Some((role, reason)) = refusal {
            log::warn!(
                "clip '{name}' {kind} channel: {role} accessor {reason}; \
                 skipping channel (joint holds rest pose)",
            );
            continue;
        }

        let reader = channel.reader(buffer_data);
        let Some(inputs) = reader.read_inputs() else {
            log::warn!(
                "{path_str}: animation {} ('{name}') channel {} {kind}: input accessor could \
                 not be read; skipping channel (joint holds rest pose)",
                anim.index(),
                channel.index(),
            );
            continue;
        };
        let times: Vec<f32> = inputs.collect();
        if !validate_key_times(&times, &name) {
            continue;
        }
        let channel_duration = times.last().copied().unwrap_or(0.0);
        let Some(outputs) = reader.read_outputs() else {
            log::warn!(
                "{path_str}: animation {} ('{name}') channel {} {kind}: output accessor could \
                 not be read; skipping channel (joint holds rest pose)",
                anim.index(),
                channel.index(),
            );
            continue;
        };
        match outputs {
            gltf::animation::util::ReadOutputs::Translations(it) => {
                let raw: Vec<Vec3> = it.map(Vec3::from).collect();
                if let Some((values, mode)) =
                    resolve_keyframes(raw, times.len(), interpolation, &name, "translation")
                        .filter(|(values, _)| validate_vec3_keyframes(values, &name, "translation"))
                    && let Some(track) = build_track(times, values, mode, &name, "translation")
                {
                    joints[topo_idx].translation = track;
                    duration = duration.max(channel_duration);
                }
            }
            gltf::animation::util::ReadOutputs::Rotations(it) => {
                let raw: Vec<Quat> = it
                    .into_f32()
                    .map(|q| Quat::from_xyzw(q[0], q[1], q[2], q[3]))
                    .collect();
                if let Some((values, mode)) =
                    resolve_keyframes(raw, times.len(), interpolation, &name, "rotation")
                        .filter(|(values, _)| validate_rotation_keyframes(values, &name))
                    && let Some(track) = build_track(times, values, mode, &name, "rotation")
                {
                    joints[topo_idx].rotation = track;
                    duration = duration.max(channel_duration);
                }
            }
            gltf::animation::util::ReadOutputs::Scales(it) => {
                let raw: Vec<Vec3> = it.map(Vec3::from).collect();
                if let Some((values, mode)) =
                    resolve_keyframes(raw, times.len(), interpolation, &name, "scale")
                        .filter(|(values, _)| validate_vec3_keyframes(values, &name, "scale"))
                    && let Some(track) = build_track(times, values, mode, &name, "scale")
                {
                    joints[topo_idx].scale = track;
                    duration = duration.max(channel_duration);
                }
            }
            gltf::animation::util::ReadOutputs::MorphTargetWeights(_) => {
                // Morph targets are out of scope for the skinned slice.
            }
        }
    }

    ParsedClip {
        name,
        duration,
        joints,
    }
}

/// Derive a clip's authored ground travel speed, then make its pose loop in-place.
///
/// The selected locomotion root carries the clip's whole-body translation. We
/// measure the horizontal (XZ) displacement between its first and last
/// translation keyframes and divide by the clip `duration` → ground units per
/// animated second. A clip with no root translation track, a non-positive
/// duration, or a net XZ displacement below [`MIN_TRAVEL_XZ_DISPLACEMENT`] (a
/// near-in-place turn/strafe/idle) yields `None` so downstream calibration
/// substitutes a degenerate reference rather than dividing by a speed of ≈0.
/// Vertical (Y) motion is intentionally excluded — only ground travel counts.
///
/// Entity transforms own simulation displacement. After measuring, subtract the
/// root track's linear net XZ drift from every key. This keeps vertical motion
/// and intra-cycle horizontal sway, while making the first and last horizontal
/// samples equal so looping the pose cannot add a second displacement source.
/// Calculations stage in `f64`; if speed or any rewritten key cannot fit finite
/// `f32`, the original track stays untouched and calibration is disabled.
fn derive_and_neutralize_root_motion(
    joints: &mut [JointTracks],
    locomotion_root: Option<usize>,
    duration: f32,
    clip_name: &str,
    path_str: &str,
) -> Option<f32> {
    let root = locomotion_root?;
    let track = &mut joints.get_mut(root)?.translation;
    let values = track.values();
    let first = *values.first()?;
    let last = *values.last()?;
    let dx = f64::from(last.x) - f64::from(first.x);
    let dz = f64::from(last.z) - f64::from(first.z);
    let displacement = dx.hypot(dz);

    let speed = if duration.is_finite()
        && duration > 0.0
        && displacement >= f64::from(MIN_TRAVEL_XZ_DISPLACEMENT)
    {
        finite_f32(displacement / f64::from(duration)).or_else(|| {
            warn_root_motion_overflow(clip_name, path_str);
            None
        })?
    } else {
        0.0
    };

    if let (Some(&first_time), Some(&last_time)) = (track.times.first(), track.times.last()) {
        let first_time = f64::from(first_time);
        let time_span = f64::from(last_time) - first_time;
        if time_span > 0.0 {
            let mut neutralized_xz = Vec::with_capacity(track.values.len());
            for (&time, value) in track.times.iter().zip(&track.values) {
                let alpha = (f64::from(time) - first_time) / time_span;
                let Some(x) = finite_f32(f64::from(value.x) - dx * alpha) else {
                    warn_root_motion_overflow(clip_name, path_str);
                    return None;
                };
                let Some(z) = finite_f32(f64::from(value.z) - dz * alpha) else {
                    warn_root_motion_overflow(clip_name, path_str);
                    return None;
                };
                neutralized_xz.push((x, z));
            }
            for (value, (x, z)) in track.values.iter_mut().zip(neutralized_xz) {
                value.x = x;
                value.z = z;
            }
        }
    }

    if speed == 0.0 {
        return None;
    }
    Some(speed)
}

fn finite_f32(value: f64) -> Option<f32> {
    (value.is_finite() && value >= f64::from(f32::MIN) && value <= f64::from(f32::MAX))
        .then_some(value as f32)
}

fn warn_root_motion_overflow(clip_name: &str, path_str: &str) {
    log::warn!(
        "[Model] root-motion calibration for clip '{clip_name}' in {path_str} exceeds finite f32 range; keeping the original translation track and disabling travel-speed derivation"
    );
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::clip_library_test_fixtures::{Document, clip, node, patch_document, skinned};
    use crate::gltf_loader::load_model;
    use gltf::animation::Interpolation;
    use log::Level;
    use postretro_test_log_capture::LogCapture;
    use serde_json::{Value, json};

    /// A clip with a sound `arm` rotation channel, then a `pelvis` translation
    /// channel for [`break_translation`] to damage.
    fn two_channel_model(times: &[f32], translations: &[Vec3]) -> Document {
        skinned(vec![
            node("pelvis").at(Vec3::Y),
            node("arm").under("pelvis"),
        ])
        .clip(
            clip("sway")
                .rotation(
                    "arm",
                    &[0.0, 1.0],
                    &[Quat::IDENTITY, Quat::from_rotation_y(0.3)],
                )
                .translation("pelvis", times, translations),
        )
    }

    fn merge_fields(target: &mut Value, fields: &Value) {
        for (key, value) in fields.as_object().unwrap() {
            target[key.as_str()] = value.clone();
        }
    }

    /// Overwrite fields of the accessor on `side` of the translation channel, and
    /// of that accessor's own buffer view.
    fn break_translation(path: &Path, side: &str, accessor: &Value, view: &Value) {
        patch_document(path, |root| {
            let accessor_index =
                root["animations"][0]["samplers"][1][side].as_u64().unwrap() as usize;
            let view_index = root["accessors"][accessor_index]["bufferView"]
                .as_u64()
                .unwrap() as usize;
            merge_fields(&mut root["accessors"][accessor_index], accessor);
            merge_fields(&mut root["bufferViews"][view_index], view);
        });
    }

    // Regression: gltf trusted stride, count and sparse shape. A short stride
    // panicked in release, a zero count overflowed in dev, and a sparse-only
    // accessor with a huge count aborted on allocation.
    #[test]
    fn unreadable_sampler_accessor_warns_and_skips_only_its_channel() {
        const BOTH: &[&str] = &["input", "output"];
        // An input element is 4 bytes, the least stride a document may declare.
        const OUTPUT: &[&str] = &["output"];
        let sparse = json!({
            "count": 1,
            "indices": { "bufferView": 0, "componentType": 5123 },
            "values": { "bufferView": 0 },
        });
        let sparse_only = json!({
            "bufferView": null,
            "count": 1_000_000_000_000u64,
            "sparse": sparse,
        });
        let none = json!({});
        let cases = [
            (BOTH, "has no elements", json!({ "count": 0 }), none.clone()),
            (BOTH, "has no buffer view", sparse_only, none.clone()),
            (
                BOTH,
                "has a sparse count outside its elements",
                json!({ "sparse": { "count": 3, "indices": sparse["indices"], "values": sparse["values"] } }),
                none.clone(),
            ),
            (
                OUTPUT,
                "has a byte stride below its element size",
                none.clone(),
                json!({ "byteStride": 4 }),
            ),
            (
                BOTH,
                "overruns its buffer",
                json!({ "count": u64::MAX }),
                none.clone(),
            ),
            (
                BOTH,
                "overruns its buffer",
                json!({ "byteOffset": 4 }),
                none.clone(),
            ),
            (
                BOTH,
                "overruns its buffer",
                none.clone(),
                json!({ "byteLength": 1u64 << 40 }),
            ),
            (
                BOTH,
                "overruns its buffer",
                none.clone(),
                json!({ "byteOffset": u64::MAX }),
            ),
        ];

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.gltf");
        for (sides, reason, accessor, view) in &cases {
            for side in *sides {
                let label = format!("{side} {accessor} {view}");
                two_channel_model(&[0.0, 1.0], &[Vec3::Y, Vec3::Y * 1.1]).write(&path);
                break_translation(&path, side, accessor, view);
                let capture = LogCapture::start();
                let model = load_model(&path).unwrap_or_else(|error| panic!("{label}: {error}"));

                capture.assert_logged_once(Level::Warn, &format!("{side} accessor {reason}"));
                let joints = &model.clips[0].joints;
                assert!(joints[0].translation.values().is_empty(), "{label}");
                assert_eq!(joints[1].rotation.values().len(), 2, "{label}");
            }
        }
    }

    #[test]
    fn sparse_accessor_with_a_buffer_view_still_reads() {
        let dir = tempfile::tempdir().unwrap();
        let path = two_channel_model(&[0.0, 1.0], &[Vec3::Y, Vec3::Y * 1.1])
            .write(&dir.path().join("model.gltf"));
        let sparse = json!({
            "count": 1,
            "indices": { "bufferView": 0, "componentType": 5123 },
            "values": { "bufferView": 0 },
        });
        break_translation(&path, "output", &json!({ "sparse": sparse }), &json!({}));
        let capture = LogCapture::start();
        let model = load_model(&path).unwrap();

        assert_eq!(model.clips[0].joints[0].translation.values().len(), 2);
        capture.assert_nothing_above(Level::Info);
    }

    #[test]
    fn stride_wider_than_its_element_still_reads() {
        let dir = tempfile::tempdir().unwrap();
        let path = two_channel_model(&[0.0, 1.0, 2.0], &[Vec3::Y, Vec3::Y * 5.0, Vec3::Y * 3.0])
            .write(&dir.path().join("model.gltf"));
        break_translation(&path, "input", &json!({ "count": 2 }), &json!({}));
        break_translation(
            &path,
            "output",
            &json!({ "count": 2 }),
            &json!({ "byteStride": 24 }),
        );
        let capture = LogCapture::start();
        let model = load_model(&path).unwrap();

        assert_eq!(
            model.clips[0].joints[0].translation.values(),
            [Vec3::Y, Vec3::Y * 3.0],
            "the stride steps over the middle key"
        );
        capture.assert_nothing_above(Level::Info);
    }

    #[test]
    fn derive_travel_speed_measures_xz_root_motion_over_duration() {
        // Root travels 3 units on X and 4 on Z (5-unit XZ displacement) across a
        // 2s clip → 2.5 ground units per animated second. The Y component is
        // deliberately non-zero to prove vertical motion is excluded.
        let translation = Track::new(
            vec![0.0, 2.0],
            vec![Vec3::ZERO, Vec3::new(3.0, 1.0, 4.0)],
            Interp::Linear,
        )
        .expect("valid translation track");
        let mut joints = vec![JointTracks {
            translation,
            ..JointTracks::default()
        }];
        let speed = derive_and_neutralize_root_motion(&mut joints, Some(0), 2.0, "walk", "fixture")
            .expect("root motion yields a speed");
        assert!((speed - 2.5).abs() < 1.0e-6, "expected 2.5, got {speed}");
        let root_values = joints[0].translation.values();
        assert_eq!(
            (
                root_values.first().unwrap().x,
                root_values.first().unwrap().z
            ),
            (root_values.last().unwrap().x, root_values.last().unwrap().z)
        );
        assert_eq!(
            root_values.last().unwrap().y,
            1.0,
            "vertical motion is preserved"
        );
    }

    #[test]
    fn derive_travel_speed_is_none_without_root_translation_track() {
        // An in-place clip's root joint has an empty translation track.
        let mut joints = vec![JointTracks::default()];
        assert_eq!(
            derive_and_neutralize_root_motion(&mut joints, Some(0), 1.0, "idle", "fixture",),
            None
        );
    }

    #[test]
    fn derive_travel_speed_is_none_for_near_in_place_root_motion() {
        // Sub-millimeter XZ drift (a near-in-place turn) sits below the epsilon,
        // so it yields None rather than a divide-by-≈0 stride rate downstream.
        let translation = Track::new(
            vec![0.0, 1.0],
            vec![Vec3::ZERO, Vec3::new(0.0001, 5.0, 0.0001)],
            Interp::Linear,
        )
        .expect("valid translation track");
        let mut joints = vec![JointTracks {
            translation,
            ..JointTracks::default()
        }];
        assert_eq!(
            derive_and_neutralize_root_motion(&mut joints, Some(0), 1.0, "turn", "fixture",),
            None
        );
        assert_eq!(
            (
                joints[0].translation.values().first().unwrap().x,
                joints[0].translation.values().first().unwrap().z,
            ),
            (
                joints[0].translation.values().last().unwrap().x,
                joints[0].translation.values().last().unwrap().z,
            ),
            "sub-epsilon drift is still removed from the looping pose"
        );
    }

    #[test]
    fn root_motion_derivation_uses_selected_nonzero_root() {
        let translation = Track::new(
            vec![0.0, 2.0],
            vec![Vec3::new(1.0, 0.0, 2.0), Vec3::new(7.0, 0.5, 10.0)],
            Interp::Linear,
        )
        .unwrap();
        let mut joints = vec![
            JointTracks::default(),
            JointTracks::default(),
            JointTracks {
                translation,
                ..JointTracks::default()
            },
        ];

        let speed = derive_and_neutralize_root_motion(&mut joints, Some(2), 2.0, "walk", "fixture")
            .unwrap();
        assert!((speed - 5.0).abs() < 1.0e-6);
        assert!(joints[0].translation.values().is_empty());
        let selected = joints[2].translation.values();
        assert_eq!(
            (selected.first().unwrap().x, selected.first().unwrap().z),
            (selected.last().unwrap().x, selected.last().unwrap().z)
        );
    }

    #[test]
    fn root_motion_speed_overflow_keeps_finite_track_unchanged() {
        // Regression: subtracting opposite finite f32 extremes and squaring in
        // f32 produced infinities, then neutralization wrote non-finite keys.
        let translation = Track::new(
            vec![0.0, 1.0],
            vec![
                Vec3::new(-f32::MAX, 1.0, -f32::MAX),
                Vec3::new(f32::MAX, 2.0, f32::MAX),
            ],
            Interp::Linear,
        )
        .unwrap();
        let original = translation.clone();
        let mut joints = vec![JointTracks {
            translation,
            ..JointTracks::default()
        }];

        assert_eq!(
            derive_and_neutralize_root_motion(&mut joints, Some(0), 1.0, "extreme", "fixture",),
            None,
        );
        assert_eq!(joints[0].translation, original);
        assert!(
            joints[0]
                .translation
                .values()
                .iter()
                .all(|value| value.is_finite())
        );
    }

    #[test]
    fn root_motion_neutralization_overflow_keeps_finite_track_unchanged() {
        // Net speed is representable, but removing its linear drift would push
        // the opposing middle residual below finite f32 range. Reject the whole
        // calibration before mutating any key.
        let translation = Track::new(
            vec![0.0, 0.5, 1.0],
            vec![
                Vec3::ZERO,
                Vec3::new(-f32::MAX, 1.0, 0.0),
                Vec3::new(f32::MAX, 2.0, 0.0),
            ],
            Interp::Linear,
        )
        .unwrap();
        let original = translation.clone();
        let mut joints = vec![JointTracks {
            translation,
            ..JointTracks::default()
        }];

        assert_eq!(
            derive_and_neutralize_root_motion(
                &mut joints,
                Some(0),
                2.0,
                "extreme-residual",
                "fixture",
            ),
            None,
        );
        assert_eq!(joints[0].translation, original);
        assert!(
            joints[0]
                .translation
                .values()
                .iter()
                .all(|value| value.is_finite())
        );
    }

    #[test]
    fn resolve_keyframes_maps_linear_and_step_directly() {
        let raw = vec![Vec3::X, Vec3::Y, Vec3::Z];
        let (vals, mode) =
            resolve_keyframes(raw.clone(), 3, Interpolation::Linear, "clip", "translation")
                .expect("linear keeps all values");
        assert_eq!(vals, raw, "LINEAR passes raw values through");
        assert_eq!(mode, Interp::Linear);

        let (vals, mode) =
            resolve_keyframes(raw.clone(), 3, Interpolation::Step, "clip", "translation")
                .expect("step keeps all values");
        assert_eq!(vals, raw, "STEP passes raw values through");
        assert_eq!(mode, Interp::Step);
    }

    #[test]
    fn resolve_keyframes_linear_and_step_wrong_shape_skips_channel() {
        // Regression: malformed LINEAR/STEP output counts could create tracks
        // where values were shorter/longer than times, then panic during sampling.
        let short_linear = vec![Vec3::ZERO; 2];
        let result = resolve_keyframes(
            short_linear,
            3,
            Interpolation::Linear,
            "clip",
            "translation",
        );
        assert!(
            result.is_none(),
            "malformed linear (outputs != keys) skips the channel"
        );

        let long_step = vec![Vec3::ZERO; 4];
        let result = resolve_keyframes(long_step, 3, Interpolation::Step, "clip", "scale");
        assert!(
            result.is_none(),
            "malformed step (outputs != keys) skips the channel"
        );
    }

    #[test]
    fn resolve_keyframes_cubicspline_extracts_value_element_as_linear() {
        // Two keyframes, each a [in-tangent, value, out-tangent] triple. The
        // extracted track must be the VALUE element of each triple (index 3k+1),
        // which here differs from the surrounding tangent elements — pinning that
        // we keep the value, not a tangent.
        let in_t0 = Vec3::new(-1.0, -1.0, -1.0);
        let val0 = Vec3::new(5.0, 5.0, 5.0);
        let out_t0 = Vec3::new(9.0, 9.0, 9.0);
        let in_t1 = Vec3::new(-2.0, -2.0, -2.0);
        let val1 = Vec3::new(7.0, 7.0, 7.0);
        let out_t1 = Vec3::new(8.0, 8.0, 8.0);
        let raw = vec![in_t0, val0, out_t0, in_t1, val1, out_t1];

        let (vals, mode) =
            resolve_keyframes(raw, 2, Interpolation::CubicSpline, "clip", "rotation")
                .expect("well-formed cubic triple extracts values");
        assert_eq!(
            vals,
            vec![val0, val1],
            "extracts the value element of each triple"
        );
        assert_ne!(vals[0], in_t0, "extracted value is not the in-tangent");
        assert_ne!(vals[0], out_t0, "extracted value is not the out-tangent");
        assert_eq!(mode, Interp::Linear, "cubic degrades to LINEAR");
    }

    #[test]
    fn resolve_keyframes_cubicspline_wrong_shape_skips_channel() {
        // 5 outputs against 2 keyframes is not a valid 3x cubic triple shape:
        // skip the channel (None) rather than storing a length-mismatched track.
        let raw = vec![Vec3::ZERO; 5];
        let result = resolve_keyframes(raw, 2, Interpolation::CubicSpline, "clip", "scale");
        assert!(
            result.is_none(),
            "malformed cubic (outputs != 3x keys) skips the channel"
        );
    }

    #[test]
    fn validate_key_times_rejects_nan_and_duplicate_keys() {
        // Regression: NaN input times reached locate_span and could underflow
        // during sampling. Loader validation must skip the channel instead.
        assert!(
            !validate_key_times(&[f32::NAN, 1.0], "clip"),
            "NaN input time skips the channel"
        );
        assert!(
            !validate_key_times(&[0.0, 0.0], "clip"),
            "duplicate input times skip the channel"
        );
        assert!(
            !validate_key_times(&[1.0, 0.5], "clip"),
            "descending input times skip the channel"
        );
        assert!(
            validate_key_times(&[0.0, 0.5, 1.0], "clip"),
            "finite strictly ascending times are accepted"
        );
    }

    #[test]
    fn validate_vec3_keyframes_rejects_non_finite_outputs() {
        assert!(
            !validate_vec3_keyframes(&[Vec3::new(0.0, f32::INFINITY, 0.0)], "clip", "translation",),
            "non-finite translation skips the channel"
        );
        assert!(
            !validate_vec3_keyframes(&[Vec3::new(1.0, 2.0, f32::NAN)], "clip", "scale"),
            "non-finite scale skips the channel"
        );
        assert!(
            validate_vec3_keyframes(&[Vec3::ZERO, Vec3::ONE], "clip", "translation"),
            "finite vec3 outputs are accepted"
        );
    }

    #[test]
    fn validate_rotation_keyframes_rejects_non_finite_and_zero_quats() {
        assert!(
            !validate_rotation_keyframes(&[Quat::from_xyzw(0.0, 0.0, 0.0, 0.0)], "clip"),
            "zero quaternion skips the channel"
        );
        assert!(
            !validate_rotation_keyframes(&[Quat::from_xyzw(0.0, f32::NAN, 0.0, 1.0)], "clip"),
            "non-finite quaternion skips the channel"
        );
        assert!(
            validate_rotation_keyframes(&[Quat::IDENTITY], "clip"),
            "finite non-zero quaternion outputs are accepted"
        );
    }
}
