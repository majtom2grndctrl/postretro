// Per-hit SH bounce estimator: the animated delta bake (id 27) and the base's parity reference.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::Vec3;

use super::light_eval::bounce_contribution;
use super::trace::{gather_hit, segment_clear};
use super::{BOUNCE_ALBEDO, BounceVisibility, RaytracingCtx, SAMPLING_LATTICE_OFFSET, SKY_COLOR};
#[cfg(test)]
use crate::lightmap_bake::soft_visibility_early_out;
use crate::lightmap_bake::{DEFAULT_AREA_SAMPLE_COUNT, soft_visibility_one_sample};
use crate::map_data::MapLight;
use crate::surface_materials::stats::BounceMaterials;
#[cfg(test)]
use crate::texel_footprint::ProbeEarlyOut;

/// Deterministic per-(probe, ray, light) seed for the soft-visibility area-sample
/// rotation. `sh_bake` has no texel coordinate to hash (unlike the lightmap/animated
/// callers), so the seed is mixed from the probe index, ray index, and the light's
/// light axis — the three indices that uniquely identify a shadow query in this
/// bake. The light axis is the delta bake's unit-light digest in production; the
/// test-only base reference uses the light's global `static_lights` index, which
/// keeps a kept light's lattice rotation identical whether the reference sees the
/// full light set or a subset. SplitMix64-style integer avalanche of those indices plus
/// `SAMPLING_LATTICE_OFFSET`: a pure function of the indices, so it reproduces
/// byte-for-byte across separate processes. NO `RandomState`, NO hash-map iteration
/// order — the byte-identical-`.prl` determinism AC depends on this staying
/// index-derived. The animated delta bake (id 27) reaches this through
/// `sample_radiance_rgb`, seeded on a digest of its keyed unit light rather than
/// an index; the test-only base reference passes global light indices through the
/// same path and inherits the same deterministic seed. The surface radiance cache's patch shading mixes (patch seed id, side, light axis)
/// through the same avalanche.
pub(crate) fn soft_visibility_seed(probe_index: u64, ray_index: u64, light_index: u64) -> u64 {
    // Distinct odd multipliers keep the three index axes from aliasing onto each
    // other (e.g. probe 1/ray 0 vs probe 0/ray 1) before the avalanche.
    let mut z = SAMPLING_LATTICE_OFFSET
        ^ probe_index.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ ray_index.wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ light_index.wrapping_mul(0x94D0_49BB_1331_11EB);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Which sample of a `lattice`-sample emitter lattice the one-sample bounce
/// traces for gather ray `ray_index` of probe `probe_index` against the light
/// on seed axis `light_axis`.
///
/// Stratification: the index walks the lattice in ray order,
/// `(offset + ray_index) mod lattice`. The gather rays are a Fibonacci sphere
/// ordered pole to pole, so each run of `lattice` consecutive rays is one polar
/// band of the probe's sphere, and each band visits every emitter sample
/// exactly once; every sample is assigned exactly `RAYS_PER_PROBE / lattice`
/// ray indices. Hits are not stratified: a ray that misses, or skips the light,
/// leaves its sample untraced. `offset` is a pure function of the probe and the
/// light axis (the high half of the ray-0 visibility seed, whose low half
/// drives that ray's lattice rotation), so neighbouring probes and lights on
/// different axes start their passes at unrelated samples instead of aliasing
/// the same rays onto the same samples. The delta bake's axis is a digest of its
/// keyed unit light, so two animated lights differ unless every keyed field
/// matches; the test-only base reference uses the light's global index. Over the offset
/// each ray's sample is uniform on the lattice, so a hit's expected visibility
/// is the whole lattice's clear fraction under that ray's rotation, with no
/// fixed centre ray to bias a penumbra.
pub(crate) fn bounce_sample_index(
    probe_index: u64,
    ray_index: u64,
    light_axis: u64,
    lattice: u32,
) -> u32 {
    let offset = soft_visibility_seed(probe_index, 0, light_axis) >> 32;
    (offset.wrapping_add(ray_index) % u64::from(lattice)) as u32
}

/// Indirect (bounced) radiance along one ray plus the ray's hit distance —
/// direct term is the lightmap's responsibility. Production consumer is the
/// animated delta bake, which discards the distance; the test-only base parity
/// reference consumes it for depth moments. A ray that misses all geometry returns `SKY_COLOR` and
/// `far_sentinel` as its distance — the depth-moment bake reads "no hit" as
/// "fully open" at the probe-spacing scale.
///
/// `probe_index`/`ray_index` only feed the deterministic soft-visibility seed
/// (`soft_visibility_seed`); they never alter which geometry is traced. Each
/// light's bounce contribution is scaled by the `[0, 1]` area-light visibility
/// fraction so bounced shadows soften in lockstep with the direct static term
/// (an authored hard light — `_light_size`/`_angular_diameter == 0` — collapses to
/// the single hard ray inside either visibility helper, matching the old binary
/// gate).
/// `visibility` picks how a soft light resolves: production traces one
/// stratified emitter sample per (hit, light) (`bounce_sample_index`), whose
/// expectation at a hit is the full 32-sample lattice's clear fraction.
///
/// `light_global_indices` carries each `lights[i]`'s seed axis. The delta bake
/// passes its one light's content-derived digest (`bake_probe_indirect_rgb`). The
/// test-only base reference passes positions in the global `static_lights` slice,
/// so a `lights` subset still gives a kept light the rotation the full set gives
/// it. `None` means identity (slice position == global index).
// Each argument is a distinct, irreducible input to the per-ray bounce sample
// (geometry ctx, ray origin/dir, the light slice + its global-index map, the sky
// sentinel, the two seed axes, and the visibility mode). Bundling them into a
// struct would only move the plumbing, not reduce it.
#[allow(clippy::too_many_arguments)]
pub(super) fn sample_radiance_rgb(
    ctx: &RaytracingCtx<'_>,
    origin: Vec3,
    dir: Vec3,
    lights: &[&MapLight],
    light_global_indices: Option<&[u64]>,
    far_sentinel: f32,
    probe_index: u64,
    ray_index: u64,
    visibility: BounceVisibility,
) -> (Vec3, f32) {
    match gather_hit(ctx, origin, dir) {
        None => (Vec3::from(SKY_COLOR), far_sentinel),
        Some(hit) => {
            let mut radiance = Vec3::ZERO;
            for (light_index, light) in lights.iter().enumerate() {
                // This must precede `global_index` and seed derivation: a skipped
                // light would only add `0 × visibility`. Kept lights retain their
                // original slice/global index and therefore their deterministic
                // visibility seed.
                let Some(contribution) = bounce_contribution(light, hit.point, hit.normal) else {
                    continue;
                };
                let global_index = light_global_indices
                    .map(|g| g[light_index])
                    .unwrap_or(light_index as u64);
                let seed = soft_visibility_seed(probe_index, ray_index, global_index);
                // SH bounce is low-frequency, so it has no author-facing
                // sample-count knob — it always samples the default lattice
                // (the `--soft-shadow-samples` knob reaches only the lightmap
                // and animated stages).
                let trace = |from, to| segment_clear(ctx, from, to);
                let v = match visibility {
                    BounceVisibility::OneSample => soft_visibility_one_sample(
                        hit.point,
                        hit.normal,
                        light,
                        seed,
                        |lattice| {
                            bounce_sample_index(probe_index, ray_index, global_index, lattice)
                        },
                        DEFAULT_AREA_SAMPLE_COUNT,
                        trace,
                    ),
                    #[cfg(test)]
                    BounceVisibility::FullSet(embedded) => soft_visibility_early_out(
                        hit.point,
                        hit.normal,
                        light,
                        seed,
                        DEFAULT_AREA_SAMPLE_COUNT,
                        ProbeEarlyOut::for_probe_point(embedded.is_embedded(global_index as usize)),
                        trace,
                    ),
                    #[cfg(test)]
                    BounceVisibility::FullSetReseeded(salt, embedded) => soft_visibility_early_out(
                        hit.point,
                        hit.normal,
                        light,
                        soft_visibility_seed(seed, salt, 0),
                        DEFAULT_AREA_SAMPLE_COUNT,
                        ProbeEarlyOut::for_probe_point(embedded.is_embedded(global_index as usize)),
                        trace,
                    ),
                };
                if v <= 0.0 {
                    continue;
                }
                radiance += contribution * v;
            }
            (
                reflected_radiance(ctx.bounce_materials, hit.texture_index, radiance),
                hit.distance,
            )
        }
    }
}

/// Outgoing radiance of a Lambertian surface of texture `texture_index` under
/// `irradiance`: its bounce albedo over π, the texture's resolved mean or
/// grey. Shared by the per-hit estimator and the surface radiance cache's
/// patch shading, so both reflect light alike.
pub(crate) fn reflected_radiance(
    materials: Option<&BounceMaterials>,
    texture_index: u32,
    irradiance: Vec3,
) -> Vec3 {
    match materials.and_then(|materials| materials.resolved_albedo(texture_index)) {
        Some(albedo) => irradiance * albedo / std::f32::consts::PI,
        // The scalar multiply, not a splatted Vec3, keeps an unresolved
        // hit's bytes identical to the pre-colour grey bake.
        None => irradiance * BOUNCE_ALBEDO / std::f32::consts::PI,
    }
}
