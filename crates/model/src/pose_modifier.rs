// CPU-only ordered skeletal pose modifiers over sampled local TRS.
// See: context/lib/rendering_pipeline.md §9

use glam::{Mat4, Quat, Vec3};
use postretro_foundation::PoseInputs;

use crate::anim::LocalTrs;
use crate::mesh::MAX_JOINTS;
use crate::skeleton::Skeleton;

const MASK_WORD_BITS: usize = u64::BITS as usize;
const MASK_WORDS: usize = MAX_JOINTS.div_ceil(MASK_WORD_BITS);

/// A set of skeleton/topological joint indices.
///
/// The fixed four-word representation covers the model crate's complete
/// [`MAX_JOINTS`] range without allocation. Iteration is ascending joint-index
/// order, which is parent-before-child for a valid [`crate::skeleton::Skeleton`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct JointMask {
    words: [u64; MASK_WORDS],
}

impl JointMask {
    /// An empty joint set.
    pub const fn new() -> Self {
        Self {
            words: [0; MASK_WORDS],
        }
    }

    /// Add `joint_index` to the set.
    ///
    /// Returns `false` when the index lies beyond the supported skeleton limit;
    /// callers can then diagnose malformed authored data without a panic.
    pub fn insert(&mut self, joint_index: usize) -> bool {
        if joint_index >= MAX_JOINTS {
            return false;
        }
        let word = joint_index / MASK_WORD_BITS;
        let bit = joint_index % MASK_WORD_BITS;
        self.words[word] |= 1_u64 << bit;
        true
    }

    /// Whether this set includes `joint_index`.
    pub fn contains(&self, joint_index: usize) -> bool {
        if joint_index >= MAX_JOINTS {
            return false;
        }
        let word = joint_index / MASK_WORD_BITS;
        let bit = joint_index % MASK_WORD_BITS;
        self.words[word] & (1_u64 << bit) != 0
    }

    /// Whether the set contains no joints.
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|word| *word == 0)
    }

    /// Included joint indices in ascending skeleton/topological order.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        (0..MAX_JOINTS).filter(|&joint_index| self.contains(joint_index))
    }
}

/// One CPU-side operation over a sampled local-TRS pose.
///
/// The enum is deliberately concrete: future modifier families extend this
/// dispatch rather than introduce dynamic dispatch into the palette hot path.
#[derive(Debug, Clone, PartialEq)]
pub enum PoseModifier {
    /// Distribute `PoseInputs::aim_pitch` across `ModifierEntry::mask`.
    ///
    /// Weights are parallel to `ModifierEntry::mask.iter()`. Missing, zero,
    /// negative, or non-finite values default to `1.0`; surplus values are
    /// ignored. This keeps the public representation flexible for unauthored or
    /// partially authored chains. Application normalizes only weights aligned
    /// with joints present in the sampled local pose; stored weights are never
    /// pre-normalized.
    AimPitchBend { bend_weights: Vec<f32> },
    /// Twist the upper body relative to the body heading, using this second mask
    /// to identify lower-only and seam joints. Application may compensate local
    /// rotations in either mask to reach the requested composed hierarchy yaw;
    /// joints in neither mask retain their sampled local transform.
    UpperLowerSplit { lower_body_mask: JointMask },
    /// Plant each leg's foot on its per-foot ground probe with an analytic
    /// two-bone (hip → knee → ankle) solve.
    ///
    /// The `legs` set is an ordered, N-leg-ready list: leg `i` consumes
    /// `PoseInputs::feet[i]`, so a biped runs two solves and a hexapod six from
    /// one loop. Each leg is independent — a probe miss, an out-of-reach target,
    /// or a clip-lifted (swing) foot ramps that leg to its clip pose without
    /// touching the others. The variant's own leg masks drive it; the enclosing
    /// [`ModifierEntry::mask`] is unused for this arm.
    FootIk { legs: Vec<LegChain> },
}

/// One leg's joint set and foot joint for the [`PoseModifier::FootIk`] solver.
///
/// `chain_mask` names the joints of this leg's two-bone chain (hip, knee, and
/// the ankle/foot). It is a mask, not a fixed pair, so the same solver drives
/// any number of legs. `foot_joint` is the ankle/end-effector: its model-space
/// position is what the solve drives onto the ground probe, and its orientation
/// is aligned to the probed ground normal. The hip and knee are recovered by
/// walking the skeleton parent links up from `foot_joint`, keeping to joints
/// present in `chain_mask`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegChain {
    /// Joints belonging to this leg's chain (hip, knee, ankle/foot).
    pub chain_mask: JointMask,
    /// The ankle/foot joint: the solve's end effector and the joint oriented to
    /// the ground normal.
    pub foot_joint: usize,
}

/// A modifier and its primary mask.
///
/// Most modifiers mutate only this mask. [`PoseModifier::UpperLowerSplit`] also
/// uses and may compensate its `lower_body_mask` to preserve composed yaw.
#[derive(Debug, Clone, PartialEq)]
pub struct ModifierEntry {
    pub mask: JointMask,
    pub modifier: PoseModifier,
}

/// A pose-modifier list applied in insertion order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PoseModifierStack {
    entries: Vec<ModifierEntry>,
}

impl PoseModifierStack {
    pub fn new(entries: Vec<ModifierEntry>) -> Self {
        Self { entries }
    }

    pub fn push(&mut self, entry: ModifierEntry) {
        self.entries.push(entry);
    }

    pub fn entries(&self) -> &[ModifierEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Apply every entry in list order to one materialized local pose.
pub(crate) fn apply_pose_modifier_stack(
    stack: &PoseModifierStack,
    inputs: &PoseInputs,
    skeleton: &Skeleton,
    locals: &mut [LocalTrs],
) {
    for entry in stack.entries() {
        match &entry.modifier {
            PoseModifier::AimPitchBend { bend_weights } => {
                apply_aim_pitch_bend(entry.mask, bend_weights, inputs.aim_pitch, locals);
            }
            PoseModifier::UpperLowerSplit { lower_body_mask } => {
                apply_upper_lower_split(
                    entry.mask,
                    *lower_body_mask,
                    inputs.aim_yaw,
                    inputs.heading_yaw,
                    skeleton,
                    locals,
                );
            }
            PoseModifier::FootIk { legs } => {
                apply_foot_ik(legs, inputs, skeleton, locals);
            }
        }
    }
}

fn apply_aim_pitch_bend(
    mask: JointMask,
    bend_weights: &[f32],
    aim_pitch: f32,
    locals: &mut [LocalTrs],
) {
    if mask.is_empty() || !aim_pitch.is_finite() {
        return;
    }

    // Accumulate in f64 so every finite f32 weight and a chain at MAX_JOINTS
    // still produce a finite normalization denominator. Invalid public values
    // degrade to the same 1.0 default as an absent authored weight.
    let local_count = locals.len();
    let total_weight = mask
        .iter()
        .enumerate()
        .filter(|(_, joint_index)| *joint_index < local_count)
        .map(|(weight_index, _)| normalized_bend_weight(bend_weights.get(weight_index)) as f64)
        .sum::<f64>();

    for (weight_index, joint_index) in mask
        .iter()
        .enumerate()
        .filter(|(_, joint_index)| *joint_index < local_count)
    {
        let local = &mut locals[joint_index];
        let weight = normalized_bend_weight(bend_weights.get(weight_index)) as f64;
        let joint_pitch = (f64::from(aim_pitch) * weight / total_weight) as f32;
        // Model forward is +Z. In the engine's right-handed model space a
        // negative X rotation raises +Z, matching positive simulation pitch.
        local.rotation *= Quat::from_rotation_x(-joint_pitch);
    }
}

fn normalized_bend_weight(weight: Option<&f32>) -> f32 {
    match weight.copied() {
        Some(weight) if weight.is_finite() && weight > 0.0 => weight,
        _ => 1.0,
    }
}

fn apply_upper_lower_split(
    upper_body_mask: JointMask,
    lower_body_mask: JointMask,
    aim_yaw: f32,
    heading_yaw: f32,
    skeleton: &Skeleton,
    locals: &mut [LocalTrs],
) {
    if upper_body_mask.is_empty() || !aim_yaw.is_finite() || !heading_yaw.is_finite() {
        return;
    }

    let yaw_delta = wrapped_angle_delta(aim_yaw, heading_yaw);
    let mut effective_factors = [0.0_f32; MAX_JOINTS];
    for joint_index in 0..locals.len().min(skeleton.joints.len()).min(MAX_JOINTS) {
        let parent_factor = skeleton.joints[joint_index]
            .parent
            .filter(|&parent| parent < joint_index && parent < locals.len())
            .map(|parent| effective_factors[parent])
            .unwrap_or(0.0);
        let Some(target_factor) = split_world_factor(upper_body_mask, lower_body_mask, joint_index)
        else {
            // Neither-mask joints retain their sampled local transform. They
            // naturally continue to follow any modified parent.
            effective_factors[joint_index] = parent_factor;
            continue;
        };
        let local_delta = yaw_delta * (target_factor - parent_factor);
        locals[joint_index].rotation *= Quat::from_rotation_y(local_delta);
        effective_factors[joint_index] = target_factor;
    }
}

fn split_world_factor(
    upper_body_mask: JointMask,
    lower_body_mask: JointMask,
    joint_index: usize,
) -> Option<f32> {
    match (
        upper_body_mask.contains(joint_index),
        lower_body_mask.contains(joint_index),
    ) {
        (true, true) => Some(0.5),
        (true, false) => Some(1.0),
        (false, true) => Some(0.0),
        (false, false) => None,
    }
}

fn wrapped_angle_delta(aim_yaw: f32, heading_yaw: f32) -> f32 {
    // Subtract in f64 so opposite finite f32 extremes cannot overflow before
    // wrapping. The result is the shortest signed turn in [-pi, pi).
    let delta = f64::from(aim_yaw) - f64::from(heading_yaw);
    let pi = std::f64::consts::PI;
    ((delta + pi).rem_euclid(std::f64::consts::TAU) - pi) as f32
}

/// Model-space clip lift (in model units) over which a foot fades from fully
/// planted to fully clip-driven. A foot at or below its probed surface plants
/// fully; once the clip lifts it this far above the surface it keeps its clip
/// lift entirely. Sized for the roughly unit-tall models the loader ships.
const PLANT_BLEND_BAND: f32 = 0.15;

/// Angular cap (radians, ~34°) on how far the foot may tilt to meet the probed
/// ground normal, so a steep normal never wrenches the ankle past a plausible
/// pose.
const MAX_FOOT_ALIGN: f32 = 0.6;

/// Numerical floor for bone lengths, reach clamping, and degenerate-axis guards.
const IK_EPS: f32 = 1e-4;

/// Solve and plant every leg in `legs`, each against its own foot probe.
///
/// Leg `i` reads `inputs.feet[i]` (only the first `foot_count` are live). A
/// missing or non-hit probe leaves that leg on its clip pose; every leg is
/// solved independently over the full local buffer so a leg's ancestors compose
/// to model space (the `UpperLowerSplit` parent walk is the precedent).
fn apply_foot_ik(
    legs: &[LegChain],
    inputs: &PoseInputs,
    skeleton: &Skeleton,
    locals: &mut [LocalTrs],
) {
    let live_feet = (inputs.foot_count as usize).min(inputs.feet.len());
    for (leg_index, leg) in legs.iter().enumerate() {
        if leg_index >= live_feet {
            continue;
        }
        let probe = inputs.feet[leg_index];
        // Miss: this leg keeps its clip pose, untouched.
        if !probe.hit {
            continue;
        }
        solve_and_plant_leg(leg, &probe, skeleton, locals);
    }
}

/// Resolve one leg's hip/knee/ankle, run the analytic two-bone solve onto its
/// probe, orient the foot to the ground normal, and blend the result against the
/// clip pose by the plant weight.
fn solve_and_plant_leg(
    leg: &LegChain,
    probe: &postretro_foundation::pose::FootProbe,
    skeleton: &Skeleton,
    locals: &mut [LocalTrs],
) {
    let joint_count = locals.len().min(skeleton.joints.len());
    let ankle = leg.foot_joint;
    if ankle >= joint_count || !leg.chain_mask.contains(ankle) {
        return;
    }
    let Some(knee) = chain_parent(skeleton, leg.chain_mask, ankle, joint_count) else {
        return;
    };
    let Some(hip) = chain_parent(skeleton, leg.chain_mask, knee, joint_count) else {
        return;
    };
    if !probe.contact_height.is_finite() {
        return;
    }

    // Forward-compute the pre-solve model-space transforms of the chain.
    let (hip_pos, hip_rot) = joint_model_transform(skeleton, locals, hip);
    let (knee_pos, knee_rot) = joint_model_transform(skeleton, locals, knee);
    let (ankle_pos, _) = joint_model_transform(skeleton, locals, ankle);

    // Plant weight from where the clip foot sits relative to the probed surface:
    // at or below → full plant; lifted past the blend band → fully clip (swing).
    let weight = plant_weight(ankle_pos.y - probe.contact_height);
    if weight <= 0.0 {
        return;
    }

    // Target keeps the clip foot's model-space XZ and drops to the ground height.
    let target = Vec3::new(ankle_pos.x, probe.contact_height, ankle_pos.z);

    let hip_clip = locals[hip].rotation;
    let knee_clip = locals[knee].rotation;
    let foot_clip = locals[ankle].rotation;

    let (hip_solved, knee_solved) = solve_two_bone(
        hip_pos, knee_pos, ankle_pos, target, hip_rot, knee_rot, hip_clip, knee_clip,
    );
    locals[hip].rotation = hip_solved;
    locals[knee].rotation = knee_solved;

    // Orient the foot toward the ground normal using its post-solve model frame.
    let foot_model_rot = joint_model_transform(skeleton, locals, ankle).1;
    let foot_parent_rot = match skeleton.joints[ankle].parent {
        Some(parent) if parent < joint_count => joint_model_transform(skeleton, locals, parent).1,
        _ => Quat::IDENTITY,
    };
    locals[ankle].rotation =
        orient_foot_local(foot_model_rot, foot_parent_rot, foot_clip, probe.normal);

    // Ramp the whole solved leg toward the clip by the plant weight, so a
    // partially lifted foot keeps a proportional share of its clip lift.
    locals[hip].rotation = hip_clip.slerp(locals[hip].rotation, weight).normalize();
    locals[knee].rotation = knee_clip.slerp(locals[knee].rotation, weight).normalize();
    locals[ankle].rotation = foot_clip.slerp(locals[ankle].rotation, weight).normalize();
}

/// Fraction of the solved plant applied, from the clip foot's lift above the
/// probed surface: `1.0` at or below the surface, ramping to `0.0` once lifted
/// past [`PLANT_BLEND_BAND`].
///
/// Height-only trade-off: `lift` is the signed height `ankle_model_y -
/// contact_height` alone, so a clip-authored swing lift and a stance foot over
/// ground that has dropped away downslope both present as positive lift and ramp
/// out identically. A stance foot over ground more than [`PLANT_BLEND_BAND`]
/// below the clip pose therefore keeps its clip pose and does not plant down —
/// an accepted envelope. Separating the two cases would need gait-phase input
/// this modifier does not receive.
fn plant_weight(lift: f32) -> f32 {
    if !lift.is_finite() {
        return 0.0;
    }
    if lift <= 0.0 {
        1.0
    } else if lift >= PLANT_BLEND_BAND {
        0.0
    } else {
        1.0 - lift / PLANT_BLEND_BAND
    }
}

/// Nearest ancestor of `joint` (walking skeleton parent links) that is present
/// in `mask`. Used to recover the knee from the ankle and the hip from the knee.
fn chain_parent(
    skeleton: &Skeleton,
    mask: JointMask,
    joint: usize,
    joint_count: usize,
) -> Option<usize> {
    let mut parent = skeleton.joints[joint].parent;
    while let Some(candidate) = parent {
        if candidate >= joint_count {
            return None;
        }
        if mask.contains(candidate) {
            return Some(candidate);
        }
        parent = skeleton.joints[candidate].parent;
    }
    None
}

/// Compose `joint`'s model-space position and rotation by walking its local
/// transform up through its ancestors' locals. No allocation — bones are short
/// chains, and the walk left-multiplies each parent's local in turn.
fn joint_model_transform(skeleton: &Skeleton, locals: &[LocalTrs], joint: usize) -> (Vec3, Quat) {
    let mut mat = local_matrix(locals[joint]);
    let mut rot = locals[joint].rotation;
    let mut parent = skeleton.joints[joint].parent;
    while let Some(p) = parent {
        if p >= locals.len() {
            break;
        }
        mat = local_matrix(locals[p]) * mat;
        rot = locals[p].rotation * rot;
        parent = skeleton.joints[p].parent;
    }
    (mat.w_axis.truncate(), rot.normalize())
}

fn local_matrix(local: LocalTrs) -> Mat4 {
    Mat4::from_scale_rotation_translation(local.scale, local.rotation, local.translation)
}

/// Analytic two-bone IK (hip → knee → ankle), after Ryan Juckett's / Daniel
/// Holden's closed-form "two joint" solve.
///
/// Given the chain's current model-space joint positions, the desired `target`
/// ankle position, and the model-space and local rotations of the hip and knee,
/// returns the hip and knee **local** rotations that place the ankle on the
/// target. The reach `|target - hip|` is clamped into the leg's reachable
/// annulus — no farther than the segment sum (an out-of-reach target straightens
/// the leg without hyperextending), no closer than the segment difference (a
/// too-close target cannot fold the ankle below the surface). Pure and
/// side-effect free for direct unit testing.
#[allow(clippy::too_many_arguments)]
fn solve_two_bone(
    hip: Vec3,
    knee: Vec3,
    ankle: Vec3,
    target: Vec3,
    hip_model_rot: Quat,
    knee_model_rot: Quat,
    hip_local: Quat,
    knee_local: Quat,
) -> (Quat, Quat) {
    let lab = (knee - hip).length();
    let lcb = (ankle - knee).length();
    if lab < IK_EPS || lcb < IK_EPS {
        // Degenerate bone lengths — nothing sound to solve; keep the clip pose.
        return (hip_local, knee_local);
    }
    // Reachable reach forms an annulus: the leg can neither extend past its
    // segment sum (far limit — hyperextension) nor fold closer than the
    // difference of its segments (near limit — steep upslope, ground high under
    // the hip). Clamp both ends. Left unclamped, a too-close target folds the
    // leg to a reach *farther* than the target (the law-of-cosines args only get
    // saved from NaN by their own clamp), dropping the ankle below the intended
    // contact height and driving it through the surface at full plant weight.
    let reach_min = (lab - lcb).abs() + IK_EPS;
    let reach_max = lab + lcb - IK_EPS;
    if reach_min >= reach_max {
        // Segment lengths leave no solvable annulus — keep the clip pose.
        return (hip_local, knee_local);
    }
    let lat = (target - hip).length().clamp(reach_min, reach_max);

    let ca = (ankle - hip).normalize_or_zero();
    let ba = (knee - hip).normalize_or_zero();
    let ab = (hip - knee).normalize_or_zero();
    let cb = (ankle - knee).normalize_or_zero();
    let ta = (target - hip).normalize_or_zero();

    // Current interior angles at the hip and knee, plus the swing needed to aim
    // the current ankle direction at the target.
    let ac_ab_0 = ca.dot(ba).clamp(-1.0, 1.0).acos();
    let ba_bc_0 = ab.dot(cb).clamp(-1.0, 1.0).acos();
    let ac_at_0 = ca.dot(ta).clamp(-1.0, 1.0).acos();

    // Desired interior angles from the law of cosines at the clamped reach.
    let ac_ab_1 = ((lab * lab + lat * lat - lcb * lcb) / (2.0 * lab * lat))
        .clamp(-1.0, 1.0)
        .acos();
    let ba_bc_1 = ((lab * lab + lcb * lcb - lat * lat) / (2.0 * lab * lcb))
        .clamp(-1.0, 1.0)
        .acos();

    // Bend-plane normal (axis0) and swing axis (axis1), in model space.
    let axis0 = (ankle - hip).cross(knee - hip).normalize_or_zero();
    let axis1 = (ankle - hip).cross(target - hip).normalize_or_zero();

    // Swing that aims the current ankle direction at the target, applied to the
    // hip. Well defined even for a straight leg.
    let hip_swing = if axis1 == Vec3::ZERO {
        Quat::IDENTITY
    } else {
        Quat::from_axis_angle(hip_model_rot.inverse() * axis1, ac_at_0)
    };

    if axis0 == Vec3::ZERO {
        // Colinear hip-knee-ankle: a perfectly straight leg has no defined bend
        // plane, so the solver can only swing the whole leg toward the target —
        // it cannot flex the knee to reach a closer one. Rigs should author a
        // slight knee bend in planted poses so the bend plane stays defined.
        let hip_out = (hip_local * hip_swing).normalize();
        return (hip_out, knee_local);
    }

    let hip_bend = Quat::from_axis_angle(hip_model_rot.inverse() * axis0, ac_ab_1 - ac_ab_0);
    let knee_bend = Quat::from_axis_angle(knee_model_rot.inverse() * axis0, ba_bc_1 - ba_bc_0);

    let hip_out = (hip_local * (hip_bend * hip_swing)).normalize();
    let knee_out = (knee_local * knee_bend).normalize();
    (hip_out, knee_out)
}

/// Foot local rotation that tilts the sole (the foot's model-space +Y) toward
/// `normal`, capped at [`MAX_FOOT_ALIGN`]. Returns the input `foot_local`
/// unchanged when the normal or the foot frame is degenerate.
fn orient_foot_local(
    foot_model_rot: Quat,
    parent_model_rot: Quat,
    foot_local: Quat,
    normal: Vec3,
) -> Quat {
    let n = normal.normalize_or_zero();
    if n == Vec3::ZERO {
        return foot_local;
    }
    let foot_up = (foot_model_rot * Vec3::Y).normalize_or_zero();
    if foot_up == Vec3::ZERO {
        return foot_local;
    }
    let (axis, angle) = Quat::from_rotation_arc(foot_up, n).to_axis_angle();
    let align = Quat::from_axis_angle(axis, angle.min(MAX_FOOT_ALIGN));
    let new_model = align * foot_model_rot;
    (parent_model_rot.inverse() * new_model).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use postretro_foundation::pose::FootProbe;

    use crate::skeleton::{Joint, RestLocal};

    // ---- Foot-IK fixtures --------------------------------------------------

    fn skeleton_from_parents(parents: &[Option<usize>]) -> Skeleton {
        Skeleton {
            joints: parents
                .iter()
                .map(|&parent| Joint {
                    parent,
                    inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
                    rest_local: RestLocal::default(),
                })
                .collect(),
        }
    }

    fn local_at(translation: Vec3) -> LocalTrs {
        LocalTrs {
            translation,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }

    fn leg_mask(joints: [usize; 3]) -> JointMask {
        let mut mask = JointMask::new();
        for j in joints {
            assert!(mask.insert(j));
        }
        mask
    }

    /// A single hip→knee→ankle chain (joints 0,1,2) with a slight forward knee
    /// bend so the bend plane is well defined. Model-space ankle rests at
    /// `(0, -2, 0)`.
    fn single_leg() -> (Skeleton, Vec<LocalTrs>) {
        let skeleton = skeleton_from_parents(&[None, Some(0), Some(1)]);
        let locals = vec![
            local_at(Vec3::ZERO),
            local_at(Vec3::new(0.0, -1.0, 0.2)),
            local_at(Vec3::new(0.0, -1.0, -0.2)),
        ];
        (skeleton, locals)
    }

    fn foot_ik_stack(legs: Vec<LegChain>) -> PoseModifierStack {
        PoseModifierStack::new(vec![ModifierEntry {
            mask: JointMask::new(),
            modifier: PoseModifier::FootIk { legs },
        }])
    }

    fn inputs_with_feet(feet: &[FootProbe]) -> PoseInputs {
        let mut inputs = PoseInputs::default();
        for (i, probe) in feet.iter().enumerate() {
            inputs.feet[i] = *probe;
        }
        inputs.foot_count = feet.len() as u8;
        inputs
    }

    fn model_pos(skeleton: &Skeleton, locals: &[LocalTrs], joint: usize) -> Vec3 {
        joint_model_transform(skeleton, locals, joint).0
    }

    #[test]
    fn foot_ik_plants_ankle_at_flat_contact_height() {
        let (skeleton, mut locals) = single_leg();
        // Clip ankle rests at y = -2; ground sits at -1.8, so the foot is planted
        // and must lift to the surface.
        let stack = foot_ik_stack(vec![LegChain {
            chain_mask: leg_mask([0, 1, 2]),
            foot_joint: 2,
        }]);
        let inputs = inputs_with_feet(&[FootProbe {
            contact_height: -1.8,
            normal: Vec3::Y,
            hit: true,
        }]);

        apply_pose_modifier_stack(&stack, &inputs, &skeleton, &mut locals);

        let ankle = model_pos(&skeleton, &locals, 2);
        assert!((ankle.y - -1.8).abs() < 1e-3, "ankle y = {}", ankle.y);
        assert!(
            ankle.x.abs() < 1e-3 && ankle.z.abs() < 1e-3,
            "ankle xz = {ankle:?}"
        );
    }

    #[test]
    fn foot_ik_orients_foot_toward_sloped_ground_normal() {
        let (skeleton, mut locals) = single_leg();
        // A ~16° slope normal, comfortably inside the foot-align cap.
        let normal = Vec3::new(0.3, 1.0, 0.0).normalize();
        let stack = foot_ik_stack(vec![LegChain {
            chain_mask: leg_mask([0, 1, 2]),
            foot_joint: 2,
        }]);
        let inputs = inputs_with_feet(&[FootProbe {
            contact_height: -1.8,
            normal,
            hit: true,
        }]);

        apply_pose_modifier_stack(&stack, &inputs, &skeleton, &mut locals);

        // Planted at the sloped contact height...
        let ankle = model_pos(&skeleton, &locals, 2);
        assert!((ankle.y - -1.8).abs() < 1e-3, "ankle y = {}", ankle.y);
        // ...with the sole (foot model +Y) aligned to the ground normal.
        let foot_rot = joint_model_transform(&skeleton, &locals, 2).1;
        let foot_up = foot_rot * Vec3::Y;
        assert!(
            foot_up.dot(normal) > 0.999,
            "foot_up = {foot_up:?}, normal = {normal:?}"
        );
    }

    #[test]
    fn two_bone_solve_clamps_out_of_reach_without_hyperextension() {
        // Reach the ankle at a target far below the leg's segment sum. The solve
        // must straighten the leg to its natural length, never past it.
        let hip = Vec3::ZERO;
        let knee_offset = Vec3::new(0.0, -1.0, 0.2);
        let ankle_offset = Vec3::new(0.0, -1.0, -0.2);
        let knee = hip + knee_offset;
        let ankle = knee + ankle_offset;
        let segment_sum = knee_offset.length() + ankle_offset.length();
        let target = Vec3::new(0.0, -5.0, 0.0); // |target - hip| = 5 >> segment_sum

        let (hip_rot, knee_rot) = solve_two_bone(
            hip,
            knee,
            ankle,
            target,
            Quat::IDENTITY,
            Quat::IDENTITY,
            Quat::IDENTITY,
            Quat::IDENTITY,
        );

        // Forward-compose the solved local rotations back to model positions.
        let knee_solved = hip + hip_rot * knee_offset;
        let ankle_solved = knee_solved + (hip_rot * knee_rot) * ankle_offset;

        let reach = (ankle_solved - hip).length();
        // Reached its full length, but not hyperextended past the segment sum.
        assert!(
            reach <= segment_sum + 1e-3,
            "reach {reach} > segment sum {segment_sum}"
        );
        assert!(
            (reach - segment_sum).abs() < 1e-2,
            "leg did not extend: reach {reach}"
        );
        // And nowhere near the unreachable target height.
        assert!(
            ankle_solved.y > -3.0,
            "ankle overshot toward target: {ankle_solved:?}"
        );
        // Knee interior angle is straight (hip and ankle directions opposed).
        let to_hip = (hip - knee_solved).normalize();
        let to_ankle = (ankle_solved - knee_solved).normalize();
        assert!(
            to_hip.dot(to_ankle) < -0.999,
            "knee not straightened: {}",
            to_hip.dot(to_ankle)
        );
    }

    #[test]
    fn foot_ik_miss_keeps_leg_on_clip_pose() {
        let (skeleton, mut locals) = single_leg();
        let before = locals.clone();
        let stack = foot_ik_stack(vec![LegChain {
            chain_mask: leg_mask([0, 1, 2]),
            foot_joint: 2,
        }]);
        // Probe reports no ground: the leg must stay exactly on its clip pose.
        let inputs = inputs_with_feet(&[FootProbe {
            contact_height: -1.8,
            normal: Vec3::Y,
            hit: false,
        }]);

        apply_pose_modifier_stack(&stack, &inputs, &skeleton, &mut locals);

        assert_eq!(locals, before);
    }

    #[test]
    fn foot_ik_swing_foot_keeps_clip_lift() {
        let (skeleton, mut locals) = single_leg();
        let before = locals.clone();
        let stack = foot_ik_stack(vec![LegChain {
            chain_mask: leg_mask([0, 1, 2]),
            foot_joint: 2,
        }]);
        // Ground sits at -2.5, well below the clip ankle at -2: the clip is
        // lifting the foot in swing, so the plant weight ramps fully to clip.
        let inputs = inputs_with_feet(&[FootProbe {
            contact_height: -2.5,
            normal: Vec3::Y,
            hit: true,
        }]);

        apply_pose_modifier_stack(&stack, &inputs, &skeleton, &mut locals);

        // Clip lift preserved: nothing pulled the swinging foot down to ground.
        assert_eq!(locals, before);
        assert!((model_pos(&skeleton, &locals, 2).y - -2.0).abs() < 1e-6);
    }

    #[test]
    fn foot_ik_solves_each_of_more_than_two_legs_independently() {
        // A root with three legs (not biped-hardcoded): one two-bone solve per
        // leg, each driven by its own probe.
        let skeleton = skeleton_from_parents(&[
            None,    // 0 root
            Some(0), // 1 hip A
            Some(1), // 2 knee A
            Some(2), // 3 ankle A
            Some(0), // 4 hip B
            Some(4), // 5 knee B
            Some(5), // 6 ankle B
            Some(0), // 7 hip C
            Some(7), // 8 knee C
            Some(8), // 9 ankle C
        ]);
        let knee_offset = Vec3::new(0.0, -1.0, 0.2);
        let ankle_offset = Vec3::new(0.0, -1.0, -0.2);
        let mut locals = vec![local_at(Vec3::ZERO); 10];
        for (hip, x) in [(1usize, -1.0), (4, 0.0), (7, 1.0)] {
            locals[hip] = local_at(Vec3::new(x, 0.0, 0.0));
            locals[hip + 1] = local_at(knee_offset);
            locals[hip + 2] = local_at(ankle_offset);
        }

        let stack = foot_ik_stack(vec![
            LegChain {
                chain_mask: leg_mask([1, 2, 3]),
                foot_joint: 3,
            },
            LegChain {
                chain_mask: leg_mask([4, 5, 6]),
                foot_joint: 6,
            },
            LegChain {
                chain_mask: leg_mask([7, 8, 9]),
                foot_joint: 9,
            },
        ]);
        // Distinct ground heights per leg prove independence.
        let inputs = inputs_with_feet(&[
            FootProbe {
                contact_height: -1.9,
                normal: Vec3::Y,
                hit: true,
            },
            FootProbe {
                contact_height: -1.7,
                normal: Vec3::Y,
                hit: true,
            },
            FootProbe {
                contact_height: -1.5,
                normal: Vec3::Y,
                hit: true,
            },
        ]);

        apply_pose_modifier_stack(&stack, &inputs, &skeleton, &mut locals);

        for (ankle, expected_y, expected_x) in
            [(3usize, -1.9, -1.0), (6, -1.7, 0.0), (9, -1.5, 1.0)]
        {
            let pos = model_pos(&skeleton, &locals, ankle);
            assert!(
                (pos.y - expected_y).abs() < 1e-3,
                "ankle {ankle} y = {}",
                pos.y
            );
            assert!(
                (pos.x - expected_x).abs() < 1e-3,
                "ankle {ankle} x = {}",
                pos.x
            );
            assert!(pos.z.abs() < 1e-3, "ankle {ankle} z = {}", pos.z);
        }
    }

    #[test]
    fn joint_mask_covers_full_joint_limit_in_topological_order() {
        let mut mask = JointMask::new();
        assert!(mask.insert(255));
        assert!(mask.insert(0));
        assert!(mask.insert(127));
        assert!(!mask.insert(256));

        assert_eq!(mask.iter().collect::<Vec<_>>(), vec![0, 127, 255]);
        assert!(mask.contains(255));
        assert!(!mask.contains(256));
    }

    #[test]
    fn stack_preserves_insertion_order() {
        let first = ModifierEntry {
            mask: JointMask::new(),
            modifier: PoseModifier::UpperLowerSplit {
                lower_body_mask: JointMask::new(),
            },
        };
        let second = ModifierEntry {
            mask: JointMask::new(),
            modifier: PoseModifier::AimPitchBend {
                bend_weights: vec![2.0],
            },
        };
        let stack = PoseModifierStack::new(vec![first.clone(), second.clone()]);
        assert_eq!(stack.entries(), &[first, second]);
    }

    #[test]
    fn finite_extreme_inputs_and_weights_keep_local_rotations_finite() {
        let mut all_joints = JointMask::new();
        assert!(all_joints.insert(0));
        assert!(all_joints.insert(1));
        let stack = PoseModifierStack::new(vec![
            ModifierEntry {
                mask: all_joints,
                modifier: PoseModifier::UpperLowerSplit {
                    lower_body_mask: JointMask::new(),
                },
            },
            ModifierEntry {
                mask: all_joints,
                modifier: PoseModifier::AimPitchBend {
                    bend_weights: vec![f32::MAX, f32::MAX],
                },
            },
        ]);
        let mut locals = vec![
            LocalTrs {
                translation: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            };
            2
        ];
        let skeleton = Skeleton {
            joints: vec![
                Joint {
                    parent: None,
                    inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
                    rest_local: RestLocal::default(),
                },
                Joint {
                    parent: Some(0),
                    inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
                    rest_local: RestLocal::default(),
                },
            ],
        };

        apply_pose_modifier_stack(
            &stack,
            &PoseInputs {
                aim_pitch: f32::MAX,
                aim_yaw: f32::MAX,
                heading_yaw: -f32::MAX,
                ..Default::default()
            },
            &skeleton,
            &mut locals,
        );

        assert!(locals.iter().all(|local| local.rotation.is_finite()));
    }

    #[test]
    fn malformed_and_misaligned_bend_weights_keep_valid_locals_finite() {
        let mut joints = JointMask::new();
        assert!(joints.insert(0));
        assert!(joints.insert(1));
        assert!(joints.insert(255));
        let stack = PoseModifierStack::new(vec![ModifierEntry {
            mask: joints,
            modifier: PoseModifier::AimPitchBend {
                // Zero and negative values default; the non-finite third value
                // is aligned to an absent local and excluded from normalization.
                bend_weights: vec![0.0, -1.0, f32::NAN, f32::MAX],
            },
        }]);
        let skeleton = Skeleton {
            joints: vec![
                Joint {
                    parent: None,
                    inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
                    rest_local: RestLocal::default(),
                },
                Joint {
                    parent: Some(0),
                    inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
                    rest_local: RestLocal::default(),
                },
            ],
        };
        let mut locals = vec![
            LocalTrs {
                translation: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            };
            2
        ];

        apply_pose_modifier_stack(
            &stack,
            &PoseInputs {
                aim_pitch: f32::MAX,
                ..Default::default()
            },
            &skeleton,
            &mut locals,
        );

        assert!(locals.iter().all(|local| local.rotation.is_finite()));
    }
}
