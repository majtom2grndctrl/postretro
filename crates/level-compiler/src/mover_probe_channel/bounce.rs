// Posed patch radiance: the static bounce one with one mover posed, then the
// later bounces over the static patches and the mover's own.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::f32::consts::PI;

use glam::Vec3;
use rayon::prelude::*;

use super::posed_scene::{PosedScene, PosedSurfaces};
use crate::bake_control::BakeControl;
use crate::map_data::MapLight;
use crate::sh_bake::{RAY_EPSILON, RaytracingCtx, bounce_hit, reflected_radiance, segment_clear};
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
    /// (static patch, light) pairs whose shadow segments may meet the pose,
    /// so re-shaded.
    pub(crate) reshaded_pairs: u64,
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
    let mut world = bounce_one.clone();
    for (&(patch, _), delta) in pairs.iter().zip(&deltas) {
        for side in Side::BOTH {
            if delta[side as usize] != Vec3::ZERO {
                world.add_side(patch, side, delta[side as usize]);
            }
        }
    }
    let mut mover = PatchRadiance::zeroed(scene.patches.len());
    if scene.surfaces == PosedSurfaces::Patches {
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
    }
    PosedRadiance {
        world,
        mover,
        reshaded_pairs: pairs.len() as u64,
    }
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

/// [`crate::surface_radiance::add_later_bounces`] over both patch sets,
/// with every gather ray's nearest hit the nearer of the static tree's and
/// the pose's. A ray from inside a posed brush (a buried static patch)
/// reads black. Where no ray of a patch meets the pose and every patch it
/// reads holds its static value, its bounce is the static one bit for bit.
pub(crate) fn add_posed_later_bounces(
    ctx: &RaytracingCtx<'_>,
    patches: &PatchSet,
    scene: &PosedScene,
    radiance: &mut PosedRadiance,
    fill: f32,
    control: &BakeControl,
) {
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
    let directions = cosine_directions(BOUNCE_RAYS_PER_PATCH);
    for bounce in 0..LATER_BOUNCE_COUNT {
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
    radiance.world.add_front_scaled(&running.0, fill);
    radiance.mover.add_front_scaled(&running.1, fill);
}

/// One patch's front-side later bounce, as the SH base gathers it, with the
/// pose's analytic entry tested against each static hit.
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
        // A posed face flush with a static one leaves the static hit.
        if let Some(posed) = scene
            .ray_entry(patch.centroid + dir * RAY_EPSILON, dir, nearest, &[])
            .filter(|posed| posed.distance < nearest)
        {
            if scene.surfaces == PosedSurfaces::Patches
                && let Some(face) = posed.face
                && let Some((blend, side)) =
                    scene
                        .patches
                        .lookup_bilinear(scene.face_triangle(face), posed.point, dir)
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
