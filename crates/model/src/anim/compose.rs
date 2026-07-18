// Skeleton hierarchy composition and skinning-palette generation.
// See: context/lib/rendering_pipeline.md §9

use glam::Mat4;

use crate::BonePaletteEntry;
use crate::skeleton::{Joint, Skeleton};

use super::WORLD_POSE_SCRATCH;

/// Run the parent-before-child forward sweep, writing each joint's **world-space**
/// transform (PRE-inverse-bind, one [`Mat4`] per joint, in skeleton/topo order)
/// into `world`.
///
/// `local_of(i, joint)` returns joint `i`'s composed local transform (the
/// single-clip path samples + composes a clip; the blend path composes an
/// already-blended TRS). `world` is cleared then filled to
/// `skeleton.joints.len()`, so a steady-state call with a reused buffer performs
/// no heap allocation.
///
/// This is the shared hierarchy core: the world poses it produces are what the
/// skinning palette multiplies by each joint's inverse-bind matrix
/// ([`compose_palette`]) and what the world-joint samplers
/// ([`sample_clip_looped_world`], [`sample_blended_world`]) expose directly for
/// hit-zone queries, while their modifier-applied counterparts expose them for
/// attachment presentation. Factoring the sweep here keeps every path on exactly
/// the same forward composition.
pub(super) fn compose_world_pose(
    skeleton: &Skeleton,
    world: &mut Vec<Mat4>,
    mut local_of: impl FnMut(usize, &Joint) -> Mat4,
) {
    let joint_count = skeleton.joints.len();
    world.clear();
    world.reserve(joint_count);

    for (i, joint) in skeleton.joints.iter().enumerate() {
        let local = local_of(i, joint);

        // Forward sweep: parent-before-child topo order guarantees the parent's
        // world matrix is already in `world` when we reach a child. Public field
        // construction can violate that; degrade an invalid parent link to a
        // root instead of panicking.
        let world_pose = match joint.parent {
            Some(p) => world.get(p).copied().unwrap_or(Mat4::IDENTITY) * local,
            None => local,
        };
        world.push(world_pose);
    }
}

/// Compose a per-joint local-matrix function into the skinning palette: run the
/// parent-before-child forward sweep ([`compose_world_pose`]) and apply each
/// joint's inverse-bind matrix, writing one [`BonePaletteEntry`] per joint into
/// `out`.
///
/// `local_of(i, joint)` returns joint `i`'s composed local transform (the
/// single-clip path samples + composes a clip; the blend path composes an
/// already-blended TRS). Both `out` and the world-pose scratch are cleared then
/// filled, so a steady-state call with a reused `out` performs no heap
/// allocation. The hierarchy compose happens **once** here (in the shared core),
/// then the inverse-bind multiply runs per joint — the blend path resolves its
/// per-joint blend before this, never inside it.
pub(super) fn compose_palette(
    skeleton: &Skeleton,
    out: &mut Vec<BonePaletteEntry>,
    local_of: impl FnMut(usize, &Joint) -> Mat4,
) {
    let joint_count = skeleton.joints.len();
    out.clear();
    out.reserve(joint_count);

    WORLD_POSE_SCRATCH.with(|cell| {
        let mut world = cell.borrow_mut();
        compose_world_pose(skeleton, &mut world, local_of);

        for (joint, world_pose) in skeleton.joints.iter().zip(world.iter()) {
            let inverse_bind = Mat4::from_cols_array_2d(&joint.inverse_bind);
            let skinning = *world_pose * inverse_bind;
            out.push(BonePaletteEntry {
                matrix: skinning.to_cols_array_2d(),
            });
        }
    });
}
