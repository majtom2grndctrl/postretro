// Later bounces: patch-to-patch gather over every static face, weighted by `_bounce_fill`.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::f32::consts::{PI, TAU};

use glam::Vec3;
use rayon::prelude::*;

use super::patches::{Patch, PatchSet, Side};
use super::sum::PatchRadiance;
use crate::bake_control::BakeControl;
use crate::sh_bake::{RaytracingCtx, bounce_hit, reflected_radiance};

/// Bounces gathered after bounce one. Provisional: the convergence tests
/// report each bounce's energy, and the owner's A/B settles the count.
/// Folded into the id-34 volume key, so changing it re-keys the volume; a
/// change to `cosine_directions` or `patch_frame` needs a
/// `SH_BASE_VOLUME_STAGE_VERSION` bump.
pub(crate) const LATER_BOUNCE_COUNT: u32 = 4;

/// Cosine-distributed rays a patch's front side gathers per later bounce.
pub(crate) const BOUNCE_RAYS_PER_PATCH: u32 = 64;

/// The fill a map without a valid `_bounce_fill` bakes at. Provisional: the
/// owner's A/B picks the shipped default.
pub(crate) const DEFAULT_BOUNCE_FILL: f32 = 1.0;

/// Patches one governed work item gathers. Chunks split the patch set in a
/// fixed way, so no bounce depends on which worker ran which chunk.
const PATCHES_PER_CHUNK: usize = 256;

/// Whether `fill` is a `_bounce_fill` the bake takes: finite, in [0, 1].
pub(crate) fn is_valid_bounce_fill(fill: f32) -> bool {
    fill.is_finite() && (0.0..=1.0).contains(&fill)
}

/// Progress units the later bounces advance at `fill`: one per patch chunk
/// per bounce, none at fill 0.
pub(crate) fn later_bounce_progress_units(patch_count: usize, fill: f32) -> usize {
    if fill == 0.0 {
        return 0;
    }
    LATER_BOUNCE_COUNT as usize * patch_count.div_ceil(PATCHES_PER_CHUNK)
}

/// Host bytes the later bounces hold beside the summed bounce one, or
/// `None` on overflow: the previous bounce, the next and the running sum of
/// later bounces, one front side per patch each, and the shared direction
/// set. Fill 0 allocates none of them.
pub(crate) fn later_bounce_buffer_bytes(patch_count: u64, fill: f32) -> Option<u64> {
    if fill == 0.0 {
        return Some(0);
    }
    patch_count
        .checked_mul(3 * size_of::<[f32; 3]>() as u64)?
        .checked_add(u64::from(BOUNCE_RAYS_PER_PATCH) * size_of::<Vec3>() as u64)
}

/// The radiance a later-bounce gather ray reads where it hits.
#[derive(Clone, Copy)]
pub(super) enum BounceSource<'a> {
    /// Bounce one, both sides of every patch.
    First(&'a PatchRadiance),
    /// A later bounce: front sides only. Back sides keep bounce one alone,
    /// so a hit on a back side reads black here.
    Later(&'a [[f32; 3]]),
}

impl BounceSource<'_> {
    fn read(&self, patch: u32, side: Side) -> Vec3 {
        match (self, side) {
            (Self::First(radiance), _) => radiance.get(patch, side),
            (Self::Later(front), Side::Front) => Vec3::from(front[patch as usize]),
            (Self::Later(_), Side::Back) => Vec3::ZERO,
        }
    }

    fn is_zero(&self) -> bool {
        match self {
            Self::First(radiance) => radiance.is_zero(),
            Self::Later(front) => front.iter().flatten().all(|&v| v == 0.0),
        }
    }
}

/// Add `fill` × the later bounces to `radiance`'s front sides, where
/// `radiance` holds bounce one. Bounce k + 1 reads only bounce k, through
/// the probe gather's bilinear patch lookup; their sum, scaled by `fill`
/// once at the end, joins bounce one. Back sides keep bounce one only: they
/// face away from the room a face bounds, so no later bounce gathers there.
/// At fill 0 nothing is traced or allocated, so the result is bounce one bit
/// for bit. Each bounce gathers every patch, lit or not, so a room beyond
/// every light's reach takes fill from its neighbours.
pub(crate) fn add_later_bounces(
    ctx: &RaytracingCtx<'_>,
    patches: &PatchSet,
    radiance: &mut PatchRadiance,
    fill: f32,
    control: &BakeControl,
) {
    if fill == 0.0 {
        return;
    }
    let count = patches.len();
    let chunks = count.div_ceil(PATCHES_PER_CHUNK);
    let mut running = vec![[0.0f32; 3]; count];
    let mut previous = vec![[0.0f32; 3]; count];
    let mut next = vec![[0.0f32; 3]; count];
    #[cfg(test)]
    let _held = [&running, &previous, &next]
        .map(|buffer| crate::live_bytes::hold_vec("later-bounce buffers", buffer));
    for bounce in 0..LATER_BOUNCE_COUNT {
        let source = if bounce == 0 {
            BounceSource::First(radiance)
        } else {
            BounceSource::Later(&previous)
        };
        if source.is_zero() {
            // Zero in, zero out: every bounce left is zero too.
            control.advance(chunks * (LATER_BOUNCE_COUNT - bounce) as usize);
            break;
        }
        gather_bounce(ctx, patches, source, &mut next, control);
        for (sum, value) in running.iter_mut().zip(&next) {
            for (s, v) in sum.iter_mut().zip(value) {
                *s += v;
            }
        }
        std::mem::swap(&mut previous, &mut next);
    }
    radiance.add_front_scaled(&running, fill);
}

/// Each later bounce in order, from bounce one in `first`: what
/// [`add_later_bounces`] sums, kept apart for the convergence tests.
#[cfg(test)]
pub(super) fn later_bounces(
    ctx: &RaytracingCtx<'_>,
    patches: &PatchSet,
    first: &PatchRadiance,
    control: &BakeControl,
) -> Vec<Vec<[f32; 3]>> {
    let mut bounces: Vec<Vec<[f32; 3]>> = Vec::new();
    for _ in 0..LATER_BOUNCE_COUNT {
        let source = match bounces.last() {
            None => BounceSource::First(first),
            Some(previous) => BounceSource::Later(previous),
        };
        let mut next = vec![[0.0f32; 3]; patches.len()];
        gather_bounce(ctx, patches, source, &mut next, control);
        bounces.push(next);
    }
    bounces
}

/// One later bounce into `out` (one front side per patch): each patch's
/// outgoing radiance under the `source` radiance its fixed rays reach. Each
/// chunk takes a governor permit and advances progress by one.
fn gather_bounce(
    ctx: &RaytracingCtx<'_>,
    patches: &PatchSet,
    source: BounceSource<'_>,
    out: &mut [[f32; 3]],
    control: &BakeControl,
) {
    debug_assert_eq!(out.len(), patches.len());
    let directions = cosine_directions(BOUNCE_RAYS_PER_PATCH);
    #[cfg(test)]
    let _directions = crate::live_bytes::hold_vec("later-bounce buffers", &directions);
    out.par_chunks_mut(PATCHES_PER_CHUNK)
        .enumerate()
        .for_each(|(chunk, out)| {
            let _permit = control.governor().enter();
            let first = chunk * PATCHES_PER_CHUNK;
            for (patch, slot) in patches.patches()[first..].iter().zip(out.iter_mut()) {
                *slot = gather_patch(ctx, patches, source, &directions, patch).to_array();
            }
            control.advance(1);
        });
}

/// A patch's front-side outgoing radiance under `source`: with
/// cosine-distributed rays the irradiance is π/N × Σ L, and the patch
/// reflects its albedo over π of it, as bounce one does
/// ([`reflected_radiance`]). A ray that misses reads black, and a hit reads
/// the side it arrives at, through the probe gather's bilinear lookup.
fn gather_patch(
    ctx: &RaytracingCtx<'_>,
    patches: &PatchSet,
    source: BounceSource<'_>,
    directions: &[Vec3],
    patch: &Patch,
) -> Vec3 {
    let [tangent, bitangent, normal] = patch_frame(patch.normal, patch.seed_id());
    let mut incoming = Vec3::ZERO;
    // The ray index feeds only the test-only direction record below.
    for (_ray, local) in (0u32..).zip(directions) {
        let dir = tangent * local.x + bitangent * local.y + normal * local.z;
        #[cfg(test)]
        crate::bake_counters::record_direction(
            crate::bake_counters::Gather::Patch,
            patch.seed_id(),
            _ray,
            dir,
        );
        let Some(hit) = bounce_hit(ctx, patch.centroid, dir) else {
            continue;
        };
        if let Some((blend, side)) = patches.lookup_bilinear(hit.triangle, hit.point, dir) {
            for &(hit_patch, weight) in blend.entries() {
                incoming += source.read(hit_patch, side) * weight;
            }
        }
    }
    reflected_radiance(
        ctx.bounce_materials,
        patch.texture_index,
        incoming * (PI / directions.len() as f32),
    )
}

/// The fixed gather set in a +Z-up frame: a golden-angle spiral of equal
/// area cells on the unit disk, lifted onto the hemisphere (Malley), so the
/// directions are cosine-distributed. No RNG and no input but the count.
fn cosine_directions(count: u32) -> Vec<Vec3> {
    let golden = PI * (3.0 - 5.0f32.sqrt());
    (0..count)
        .map(|i| {
            let r2 = (i as f32 + 0.5) / count as f32;
            let r = r2.sqrt();
            let (sin, cos) = (golden * i as f32).sin_cos();
            Vec3::new(r * cos, r * sin, (1.0 - r2).max(0.0).sqrt())
        })
        .collect()
}

/// The patch's gather frame: its front normal as +Z, and the tangent pair
/// turned about it by an angle from the patch's geometric seed id alone, so
/// neighbouring patches do not share ray directions. No radiance enters it.
fn patch_frame(normal: Vec3, seed_id: u64) -> [Vec3; 3] {
    let (tangent, bitangent) = normal.any_orthonormal_pair();
    let angle = (seed_id >> 40) as f32 * (TAU / (1u32 << 24) as f32);
    let (sin, cos) = angle.sin_cos();
    [
        tangent * cos + bitangent * sin,
        bitangent * cos - tangent * sin,
        normal,
    ]
}
