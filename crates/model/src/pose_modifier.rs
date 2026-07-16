// CPU-only ordered skeletal pose modifiers over sampled local TRS.
// See: context/lib/rendering_pipeline.md §9

use glam::Quat;
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

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    use crate::skeleton::{Joint, RestLocal};

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
