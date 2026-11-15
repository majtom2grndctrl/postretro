// A mid pose's posed later bounce over its near-field domain: the static
// patches inside the domain gather posed, and every other static patch takes
// the mover-absent bake's bounce, so a gather ray that leaves the domain
// reads it.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)

use glam::Vec3;
use rayon::prelude::*;

use super::bounce::{PATCHES_PER_CHUNK, Source, gather_patch};
use super::posed_scene::PosedScene;
use crate::bake_control::BakeControl;
use crate::sh_bake::RaytracingCtx;
use crate::surface_radiance::PatchSet;

/// One near-field later bounce's inputs.
pub(super) struct NearFieldGather<'a> {
    pub(super) ctx: &'a RaytracingCtx<'a>,
    pub(super) patches: &'a PatchSet,
    pub(super) scene: &'a PosedScene,
    /// The previous bounce over both patch sets: posed inside the domain
    /// and on the posed sides, the mover-absent bake's past the domain.
    pub(super) source: Source<'a>,
    pub(super) directions: &'a [Vec3],
    /// The static patches inside the domain, ascending.
    pub(super) domain: &'a [u32],
    /// The mover-absent bake's same bounce, one front side per static
    /// patch; `None` past the last bounce id 34 gathered, which is black.
    pub(super) absent: Option<&'a [[f32; 3]]>,
    /// Whether the source holds any light: a black source gathers black, so
    /// the domain's patches trace nothing.
    pub(super) traced: bool,
}

/// One later bounce of the static patches into `out` (one front side per
/// static patch): each domain patch gathers it from `gather.source` with
/// the pose's hits, as a terminus gathers every patch; every other patch
/// takes the mover-absent bounce. Each chunk of domain patches takes a
/// governor permit and advances progress by one.
pub(super) fn gather_near_field(
    gather: NearFieldGather<'_>,
    out: &mut [[f32; 3]],
    control: &BakeControl,
) {
    let NearFieldGather {
        ctx,
        patches,
        scene,
        source,
        directions,
        domain,
        absent,
        traced,
    } = gather;
    match absent {
        Some(layer) => out.copy_from_slice(layer),
        None => out.fill([0.0; 3]),
    }
    let gathered: Vec<[f32; 3]> = domain
        .par_chunks(PATCHES_PER_CHUNK)
        .flat_map_iter(|chunk| {
            let _permit = control.governor().enter();
            let values: Vec<[f32; 3]> = chunk
                .iter()
                .map(|&patch| {
                    if traced {
                        let patch = &patches.patches()[patch as usize];
                        gather_patch(ctx, patches, scene, source, directions, patch).to_array()
                    } else {
                        [0.0; 3]
                    }
                })
                .collect();
            control.advance(1);
            values
        })
        .collect();
    #[cfg(test)]
    let _gathered = crate::live_bytes::hold_vec("mover near-field later bounce", &gathered);
    for (&patch, value) in domain.iter().zip(gathered) {
        out[patch as usize] = value;
    }
}
