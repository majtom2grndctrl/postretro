//! Ordered, CPU-only skeletal pose-modifier stack primitives.

use postretro_foundation::PoseInputs;

use crate::anim::LocalTrs;
use crate::mesh::MAX_JOINTS;

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
    /// Weights are parallel to `ModifierEntry::mask.iter()`. An absent weight
    /// (including an empty vector) has weight `1.0`, giving unauthored chains a
    /// uniform bend. The modifier implementation normalizes weights when it is
    /// applied; stored weights are never pre-normalized.
    AimPitchBend { bend_weights: Vec<f32> },
    /// Twist `ModifierEntry::mask` (the upper body) relative to the body heading,
    /// using this second mask to identify lower-body and seam joints.
    UpperLowerSplit { lower_body_mask: JointMask },
}

/// A modifier and the joints it is allowed to mutate.
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
///
/// Task 2 establishes the ordered dispatch seam. The rotation math for both
/// initial variants lands in Task 3; the exhaustive arms below are explicit
/// no-ops until then so enum entries are not bypassed by the sampler itself.
pub(crate) fn apply_pose_modifier_stack(
    stack: &PoseModifierStack,
    inputs: &PoseInputs,
    locals: &mut [LocalTrs],
) {
    for entry in stack.entries() {
        match &entry.modifier {
            PoseModifier::AimPitchBend { bend_weights } => {
                let _ = (entry.mask, bend_weights, inputs, &mut *locals);
                // Task 3: distribute pitch over the masked chain.
            }
            PoseModifier::UpperLowerSplit { lower_body_mask } => {
                let _ = (entry.mask, lower_body_mask, inputs, &mut *locals);
                // Task 3: apply upper/lower yaw split and seam weighting.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
