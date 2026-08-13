//! Canonical primitive hashing for engine-owned compatibility digests.
//!
//! Recipes keep their membership decisions in their own modules. This module only
//! owns unambiguous primitive framing and the structural walk for the closed IR
//! vocabulary shared by those recipes.

use glam::Vec3;
use postretro_foundation::ir::{IrNode, IrValue};

pub(crate) fn hash_len(hasher: &mut blake3::Hasher, len: usize) {
    hasher.update(&(len as u64).to_le_bytes());
}

pub(crate) fn hash_str(hasher: &mut blake3::Hasher, value: &str) {
    hash_len(hasher, value.len());
    hasher.update(value.as_bytes());
}

pub(crate) fn hash_vec3(hasher: &mut blake3::Hasher, value: Vec3) {
    hash_f32(hasher, value.x);
    hash_f32(hasher, value.y);
    hash_f32(hasher, value.z);
}

pub(crate) fn hash_f32(hasher: &mut blake3::Hasher, value: f32) {
    hasher.update(&value.to_bits().to_le_bytes());
}

pub(crate) fn hash_f64(hasher: &mut blake3::Hasher, value: f64) {
    hasher.update(&value.to_bits().to_le_bytes());
}

pub(crate) fn hash_u32(hasher: &mut blake3::Hasher, value: u32) {
    hasher.update(&value.to_le_bytes());
}

/// Hash the IR structurally instead of serializing it. The exhaustive match is
/// deliberate: a new IR variant must stop compatibility-recipe compilation until
/// its canonical representation is chosen.
pub(crate) fn hash_ir_node(hasher: &mut blake3::Hasher, node: &IrNode) {
    match node {
        IrNode::Const { value } => {
            hasher.update(&[0]);
            hash_ir_value(hasher, value);
        }
        IrNode::Input { name, owner } => {
            hasher.update(&[1]);
            hash_str(hasher, name);
            // Preserve the legacy byte sequence for an unowned input, while
            // keeping owner-addressed reads distinct in compatibility hashes.
            if let Some(owner) = owner {
                hasher.update(&[u8::MAX]);
                hash_str(hasher, owner);
            }
        }
        IrNode::Add { a, b } => {
            hasher.update(&[2]);
            hash_ir_node(hasher, a);
            hash_ir_node(hasher, b);
        }
        IrNode::Sub { a, b } => {
            hasher.update(&[3]);
            hash_ir_node(hasher, a);
            hash_ir_node(hasher, b);
        }
        IrNode::Mul { a, b } => {
            hasher.update(&[4]);
            hash_ir_node(hasher, a);
            hash_ir_node(hasher, b);
        }
        IrNode::Div { a, b } => {
            hasher.update(&[5]);
            hash_ir_node(hasher, a);
            hash_ir_node(hasher, b);
        }
        IrNode::Clamp { x, lo, hi } => {
            hasher.update(&[6]);
            hash_ir_node(hasher, x);
            hash_ir_node(hasher, lo);
            hash_ir_node(hasher, hi);
        }
        IrNode::Lerp { a, b, t } => {
            hasher.update(&[7]);
            hash_ir_node(hasher, a);
            hash_ir_node(hasher, b);
            hash_ir_node(hasher, t);
        }
        IrNode::Lt { a, b } => {
            hasher.update(&[8]);
            hash_ir_node(hasher, a);
            hash_ir_node(hasher, b);
        }
        IrNode::Le { a, b } => {
            hasher.update(&[9]);
            hash_ir_node(hasher, a);
            hash_ir_node(hasher, b);
        }
        IrNode::Gt { a, b } => {
            hasher.update(&[10]);
            hash_ir_node(hasher, a);
            hash_ir_node(hasher, b);
        }
        IrNode::Ge { a, b } => {
            hasher.update(&[11]);
            hash_ir_node(hasher, a);
            hash_ir_node(hasher, b);
        }
        IrNode::Eq { a, b } => {
            hasher.update(&[12]);
            hash_ir_node(hasher, a);
            hash_ir_node(hasher, b);
        }
        IrNode::Ne { a, b } => {
            hasher.update(&[13]);
            hash_ir_node(hasher, a);
            hash_ir_node(hasher, b);
        }
        IrNode::Select { cond, a, b } => {
            hasher.update(&[14]);
            hash_ir_node(hasher, cond);
            hash_ir_node(hasher, a);
            hash_ir_node(hasher, b);
        }
    }
}

/// Hash an IR leaf with a stable explicit variant tag.
pub(crate) fn hash_ir_value(hasher: &mut blake3::Hasher, value: &IrValue) {
    match value {
        IrValue::Bool(value) => {
            hasher.update(&[0, u8::from(*value)]);
        }
        IrValue::Number(value) => {
            hasher.update(&[1]);
            hash_f32(hasher, *value);
        }
    }
}
