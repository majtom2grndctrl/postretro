// Posed patch radiance: the static bounce one with one mover posed, then the
// later bounces over the static patches and the mover's own.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::f32::consts::PI;

use glam::Vec3;
use rayon::prelude::*;

use super::posed_scene::PosedScene;
use crate::bake_control::BakeControl;
use crate::map_data::MapLight;
use crate::sh_bake::{RaytracingCtx, bounce_hit, reflected_radiance, segment_clear};
use crate::surface_radiance::{
    BOUNCE_RAYS_PER_PATCH, LATER_BOUNCE_COUNT, LayerPlan, Patch, PatchRadiance, PatchReachIndex,
    PatchSet, Side, cosine_directions, patch_frame, shade_patch_side,
};
use crate::texel_footprint::ProbeEarlyOut;

/// Work items split patch lists this finely, as the SH base's bounces do.
const PATCHES_PER_CHUNK: usize = 256;

/// One planned layer's light as the shading reads it: its content's seed
/// axis, its early-out, and how many static lights share it.
pub(crate) struct LightTerm<'a> {
    light: &'a MapLight,
    axis: u64,
    early_out: ProbeEarlyOut,
    occurrences: u32,
}

/// The SH base plan's layers, in its content order: the order the static
/// bounce one sums them.
pub(crate) fn light_terms<'a>(lights: &[&'a MapLight], plan: &LayerPlan) -> Vec<LightTerm<'a>> {
    plan.layers
        .iter()
        .map(|planned| LightTerm {
            light: lights[planned.light],
            axis: planned.digest.seed_axis(),
            early_out: ProbeEarlyOut::for_probe_point(planned.embedded),
            occurrences: planned.occurrences,
        })
        .collect()
}

/// Patch radiance with one mover posed: every static patch, and the posed
/// sides' own patches.
pub(crate) struct PosedRadiance {
    pub(crate) world: PatchRadiance,
    pub(crate) mover: PatchRadiance,
}

/// Bounce one with `scene` posed, from the static bounce one. A static
/// patch's light term changes only where one of its shadow segments can
/// meet the pose ([`PosedScene::may_shadow`]); there the term is re-shaded
/// with the posed brushes added to the trace, and the difference added, so
/// every other side keeps its static value bit for bit. A static patch
/// buried in a posed brush traces from inside it and goes dark. The posed
/// sides shade every light over the static world and the other posed
/// brushes, front sides only: their backs face into the brush.
pub(crate) fn posed_bounce_one(
    ctx: &RaytracingCtx<'_>,
    patches: &PatchSet,
    reach: &PatchReachIndex<'_>,
    bounce_one: &PatchRadiance,
    terms: &[LightTerm<'_>],
    scene: &PosedScene,
    control: &BakeControl,
) -> PosedRadiance {
    let static_trace = |from, to| segment_clear(ctx, from, to);
    let posed_trace = |from, to| !scene.segment_blocked(from, to) && segment_clear(ctx, from, to);
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    for (term_index, term) in terms.iter().enumerate() {
        if !scene.light_may_reach(term.light) {
            continue;
        }
        for patch in reach.in_reach(term.light) {
            if scene.may_shadow(patches.patches()[patch as usize].centroid, term.light) {
                pairs.push((patch, term_index as u32));
            }
        }
    }
    pairs.sort_unstable();
    #[cfg(test)]
    let _pairs = crate::live_bytes::hold_vec("mover posed bounce one", &pairs);
    let deltas: Vec<[Vec3; 2]> = pairs
        .par_chunks(PATCHES_PER_CHUNK)
        .flat_map_iter(|chunk| {
            let _permit = control.governor().enter();
            chunk
                .iter()
                .map(|&(patch, term)| {
                    let patch = &patches.patches()[patch as usize];
                    let term = &terms[term as usize];
                    Side::BOTH.map(|side| {
                        let shade = |trace: &dyn Fn(Vec3, Vec3) -> bool| {
                            shade_patch_side(
                                ctx,
                                patch,
                                side,
                                term.light,
                                term.axis,
                                term.early_out,
                                trace,
                            )
                            .unwrap_or(Vec3::ZERO)
                        };
                        let before = shade(&static_trace);
                        let after = shade(&posed_trace);
                        (after - before) * term.occurrences as f32
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect();
    #[cfg(test)]
    let _deltas = crate::live_bytes::hold_vec("mover posed bounce one", &deltas);
    let mut world = bounce_one.clone();
    for (&(patch, _), delta) in pairs.iter().zip(&deltas) {
        for side in Side::BOTH {
            if delta[side as usize] != Vec3::ZERO {
                world.add_side(patch, side, delta[side as usize]);
            }
        }
    }
    let mut mover = PatchRadiance::zeroed(scene.patches.len());
    let lit: Vec<Vec3> = scene
        .patches
        .patches()
        .par_chunks(PATCHES_PER_CHUNK)
        .flat_map_iter(|chunk| {
            let _permit = control.governor().enter();
            chunk
                .iter()
                .map(|patch| {
                    // The SH base's sum order: each layer in content
                    // order, once per light sharing it.
                    let mut sum = Vec3::ZERO;
                    for term in terms {
                        let radiance = shade_patch_side(
                            ctx,
                            patch,
                            Side::Front,
                            term.light,
                            term.axis,
                            term.early_out,
                            posed_trace,
                        )
                        .unwrap_or(Vec3::ZERO);
                        for _ in 0..term.occurrences {
                            sum += radiance;
                        }
                    }
                    sum
                })
                .collect::<Vec<_>>()
        })
        .collect();
    for (patch, radiance) in lit.into_iter().enumerate() {
        mover.add_side(patch as u32, Side::Front, radiance);
    }
    PosedRadiance { world, mover }
}

/// What a later-bounce gather ray reads where it hits, over both patch
/// sets: [`crate::surface_radiance`]'s bounce source with the mover's
/// patches beside the static ones.
#[derive(Clone, Copy)]
enum Source<'a> {
    First(&'a PosedRadiance),
    Later(&'a [[f32; 3]], &'a [[f32; 3]]),
}

impl Source<'_> {
    fn world(&self, patch: u32, side: Side) -> Vec3 {
        match (self, side) {
            (Self::First(radiance), _) => radiance.world.get(patch, side),
            (Self::Later(front, _), Side::Front) => Vec3::from(front[patch as usize]),
            (Self::Later(..), Side::Back) => Vec3::ZERO,
        }
    }

    fn mover(&self, patch: u32, side: Side) -> Vec3 {
        match (self, side) {
            (Self::First(radiance), _) => radiance.mover.get(patch, side),
            (Self::Later(_, front), Side::Front) => Vec3::from(front[patch as usize]),
            (Self::Later(..), Side::Back) => Vec3::ZERO,
        }
    }

    fn is_zero(&self) -> bool {
        match self {
            Self::First(radiance) => radiance.world.is_zero() && radiance.mover.is_zero(),
            Self::Later(world, mover) => world
                .iter()
                .chain(mover.iter())
                .flatten()
                .all(|&v| v == 0.0),
        }
    }
}

/// Which later bounces one pose re-bakes, and what it keeps of id 34's.
#[derive(Clone, Copy)]
pub(crate) struct PosedLaterBounces<'t> {
    /// Later bounces run posed: the mover's `light_bounces` less bounce one.
    pub(crate) posed: u32,
    /// The SH base's own later bounces past the posed ones, summed per
    /// static patch. The mover's own patches take no tail: the static
    /// bounces never saw them.
    pub(crate) static_tail: Option<&'t [[f32; 3]]>,
}

/// [`crate::surface_radiance::add_later_bounces`] over both patch sets,
/// with every gather ray's nearest hit the nearer of the static tree's and
/// the pose's ([`PosedScene::trace`]). A ray from inside a posed brush (a
/// buried static patch) reads black. Where no ray of a patch meets the pose
/// and every patch it reads holds its static value, its bounce is the
/// static one bit for bit.
///
/// `later.posed` later bounces run posed; `later.static_tail`, when given,
/// joins them before the fill scales the sum.
pub(crate) fn add_posed_later_bounces(
    ctx: &RaytracingCtx<'_>,
    patches: &PatchSet,
    scene: &PosedScene,
    radiance: &mut PosedRadiance,
    fill: f32,
    later: PosedLaterBounces<'_>,
    control: &BakeControl,
) {
    let PosedLaterBounces {
        posed: posed_later,
        static_tail,
    } = later;
    if fill == 0.0 {
        return;
    }
    let (world_count, mover_count) = (patches.len(), scene.patches.len());
    let mut running = (
        vec![[0.0f32; 3]; world_count],
        vec![[0.0f32; 3]; mover_count],
    );
    let mut previous = running.clone();
    let mut next = running.clone();
    #[cfg(test)]
    let _held = [&running, &previous, &next].map(|(world, mover)| {
        [world, mover]
            .map(|buffer| crate::live_bytes::hold_vec("mover posed later bounces", buffer))
    });
    let directions = cosine_directions(BOUNCE_RAYS_PER_PATCH);
    for bounce in 0..posed_later.min(LATER_BOUNCE_COUNT) {
        let source = if bounce == 0 {
            Source::First(radiance)
        } else {
            Source::Later(&previous.0, &previous.1)
        };
        if source.is_zero() {
            break;
        }
        for (set, out) in [
            (patches.patches(), &mut next.0),
            (scene.patches.patches(), &mut next.1),
        ] {
            out.par_chunks_mut(PATCHES_PER_CHUNK)
                .enumerate()
                .for_each(|(chunk, out)| {
                    let _permit = control.governor().enter();
                    let first = chunk * PATCHES_PER_CHUNK;
                    for (patch, slot) in set[first..].iter().zip(out.iter_mut()) {
                        *slot = gather_patch(ctx, patches, scene, source, &directions, patch)
                            .to_array();
                    }
                });
        }
        for (running, next) in [(&mut running.0, &next.0), (&mut running.1, &next.1)] {
            for (sum, value) in running.iter_mut().zip(next) {
                for (s, v) in sum.iter_mut().zip(value) {
                    *s += v;
                }
            }
        }
        std::mem::swap(&mut previous, &mut next);
    }
    if let Some(tail) = static_tail {
        for (sum, value) in running.0.iter_mut().zip(tail) {
            for (s, v) in sum.iter_mut().zip(value) {
                *s += v;
            }
        }
    }
    radiance.world.add_front_scaled(&running.0, fill);
    radiance.mover.add_front_scaled(&running.1, fill);
}

/// One patch's front-side later bounce, as the SH base gathers it, with the
/// pose's hit tested against each static hit.
fn gather_patch(
    ctx: &RaytracingCtx<'_>,
    patches: &PatchSet,
    scene: &PosedScene,
    source: Source<'_>,
    directions: &[Vec3],
    patch: &Patch,
) -> Vec3 {
    let [tangent, bitangent, normal] = patch_frame(patch.normal, patch.seed_id());
    let mut incoming = Vec3::ZERO;
    for local in directions {
        let dir = tangent * local.x + bitangent * local.y + normal * local.z;
        let hit = bounce_hit(ctx, patch.centroid, dir);
        let nearest = hit.as_ref().map_or(f32::INFINITY, |hit| hit.distance);
        if let Some(posed) = scene.trace(patch.centroid, dir, nearest) {
            if let Some(triangle) = posed.triangle
                && let Some((blend, side)) =
                    scene.patches.lookup_bilinear(triangle, posed.point, dir)
            {
                for &(hit_patch, weight) in blend.entries() {
                    incoming += source.mover(hit_patch, side) * weight;
                }
            }
            continue;
        }
        let Some(hit) = hit else {
            continue;
        };
        if let Some((blend, side)) = patches.lookup_bilinear(hit.triangle, hit.point, dir) {
            for &(hit_patch, weight) in blend.entries() {
                incoming += source.world(hit_patch, side) * weight;
            }
        }
    }
    reflected_radiance(
        ctx.bounce_materials,
        patch.texture_index,
        incoming * (PI / directions.len() as f32),
    )
}
