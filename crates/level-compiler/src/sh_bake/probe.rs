// Per-probe SH bakes: indirect, direct-at-probe, and indirect with depth moments.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::Vec3;
use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::sh_volume::OctahedralShProbe;

use super::bounce::{sample_radiance_rgb, soft_visibility_seed};
use super::light_eval::incident_radiance_at_point;
use super::sh_math::{accumulate_sh_rgb, apply_cosine_lobe_rgb};
use super::trace::{segment_clear, sphere_directions};
use super::{
    BakedProbe, BounceVisibility, RAYS_PER_PROBE, RadianceSource, RaytracingCtx,
    SAMPLING_LATTICE_OFFSET, ShBakeCtx,
};
use crate::lightmap_bake::soft_visibility_early_out;
use crate::map_data::MapLight;
use crate::texel_footprint::ProbeEarlyOut;

/// Indirect-only SH L2 RGB for the per-light delta SH baker (`delta_sh_bake.rs`),
/// which carries no depth moments. Shares the per-ray sampler with the SH-volume
/// path (`bake_probe_rgb_with_moments`); only the ray-accumulation loop is
/// duplicated, so the delta path projects radiance without ever touching the
/// `Σd` / `Σd²` moments. This loop discards the distance the shared sampler
/// returns.
///
/// `light_axis` is the light's soft-visibility seed axis. The delta bake
/// derives it from the keyed light, so animated lights sharing a cell sample
/// their emitters independently and the axis moves only with the cache key.
pub(crate) fn bake_probe_indirect_rgb(
    ctx: &RaytracingCtx<'_>,
    probe_pos: Vec3,
    light: &MapLight,
    light_axis: u64,
    probe_index: u64,
    visibility: BounceVisibility,
) -> [f32; 27] {
    let directions = sphere_directions(RAYS_PER_PROBE, SAMPLING_LATTICE_OFFSET);
    let mc_weight = (4.0 * std::f32::consts::PI) / RAYS_PER_PROBE as f32;
    let mut acc = [0f32; 27];
    for (ray_index, dir) in directions.iter().enumerate() {
        // Pass f32::INFINITY as an unreachable sentinel; the delta path discards
        // the returned distance, so this value is never read. `probe_index`/
        // `ray_index` seed the soft-visibility area-sample rotation only.
        let (radiance, _) = sample_radiance_rgb(
            ctx,
            probe_pos,
            *dir,
            &[light],
            Some(&[light_axis]),
            f32::INFINITY,
            probe_index,
            ray_index as u64,
            visibility,
        );
        accumulate_sh_rgb(&mut acc, *dir, radiance, mc_weight);
    }
    apply_cosine_lobe_rgb(&mut acc);
    acc
}

/// Direct-at-probe SH L2 RGB for the direct SH baker (`direct_sh_bake.rs`). Bakes
/// DIRECT light from STATIC lights along the probe→light incident direction using
/// the validated D3 assembly (Task 0): for each light reaching the probe,
/// `radiance = incident_radiance_at_point(light, probe) × soft visibility(probe,
/// light)` (NORMAL-FREE — the surface cosine is left to the runtime fragment),
/// `accumulate_sh_rgb` along the incident direction, then `apply_cosine_lobe_rgb`
/// once after all lights. The packed coefficients reconstruct irradiance directly
/// (the cosine lobe is already convolved in).
///
/// `lights` is the per-probe reaching set (after the D9 falloff+portal cull);
/// `light_global_indices[i]` is `lights[i]`'s position in the full global
/// `static_lights` slice, threaded into the soft-visibility seed so a kept light
/// gets the same lattice rotation whether the bake sees the full set or a culled
/// subset. A culled
/// light is provably zero-contribution at this probe, so the seed indexing keeps
/// the output byte-identical to an unculled bake.
///
/// The probe is a point in air with no surface normal, so soft visibility
/// (`soft_visibility_early_out`) uses the incident direction `l` as the offset
/// normal (matching the Task 0 D3 harness); the hard-shadow case routes through
/// `segment_clear`, the same shadow-ray routine the indirect bake uses.
///
/// `early_outs[i]` is `lights[i]`'s probe early-out,
/// [`ProbeEarlyOut::for_probe_point`] of its embedded-emitter flag: a light
/// whose emitter sphere reaches into geometry traces its full sample set,
/// and every other light keeps the 4-probe early-out, bit for bit.
pub(crate) fn bake_probe_direct_rgb(
    ctx: &RaytracingCtx<'_>,
    probe_pos: Vec3,
    lights: &[&MapLight],
    light_global_indices: &[u64],
    early_outs: &[ProbeEarlyOut],
    probe_index: u64,
) -> [f32; 27] {
    debug_assert_eq!(lights.len(), early_outs.len());
    let mut acc = [0f32; 27];
    for (light_index, light) in lights.iter().enumerate() {
        let Some((radiance, l)) = incident_radiance_at_point(light, probe_pos) else {
            continue;
        };
        let global_index = light_global_indices[light_index];
        // No per-ray axis here (direct is a single shadow query per light, not a
        // sphere of rays), so the ray-index seed axis is fixed at 0.
        let seed = soft_visibility_seed(probe_index, 0, global_index);
        let v = soft_visibility_early_out(
            probe_pos,
            l,
            light,
            seed,
            crate::lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT,
            early_outs[light_index],
            |from, to| segment_clear(ctx, from, to),
        );
        if v <= 0.0 {
            continue;
        }
        accumulate_sh_rgb(&mut acc, l, radiance * v, 1.0);
    }
    apply_cosine_lobe_rgb(&mut acc);
    acc
}

/// The probe gather's fixed direction set: one Fibonacci sphere of
/// `RAYS_PER_PROBE` rotated by a constant, the same for every probe, light
/// set, fill and radiance source. Nothing but those two constants enters it.
pub(super) fn probe_gather_directions() -> Vec<Vec3> {
    sphere_directions(RAYS_PER_PROBE, SAMPLING_LATTICE_OFFSET)
}

/// [`bake_probe_rgb_along`] over [`probe_gather_directions`], made per call.
#[cfg(test)]
pub(super) fn bake_probe_rgb_with_moments(
    inputs: &ShBakeCtx<'_>,
    probe_pos: Vec3,
    static_lights: &[&MapLight],
    light_global_indices: Option<&[u64]>,
    far_sentinel: f32,
    probe_index: u64,
    source: RadianceSource<'_>,
) -> ([f32; 27], f32, f32) {
    bake_probe_rgb_along(
        &probe_gather_directions(),
        inputs,
        probe_pos,
        static_lights,
        light_global_indices,
        far_sentinel,
        probe_index,
        source,
    )
}

/// SH-volume-path twin of `bake_probe_indirect_rgb` that also accumulates the
/// per-probe depth moments `Σd` / `Σd²` over the same 256-direction loop. The
/// shared per-ray sampler keeps the bounce math identical across both paths;
/// the ray-accumulation loop is duplicated so the delta path stays moment-free.
/// Sky-miss rays contribute `far_sentinel` to both sums (via
/// `sample_radiance_rgb`), so all `RAYS_PER_PROBE` rays accumulate and the
/// caller's division by the constant is exact. Accumulation is a sequential sum
/// over the fixed direction list so the bake stays byte-identical across runs.
/// `source` picks only where a hit's radiance comes from; the directions and
/// the hits, so the moments, are the same either way. `directions` is
/// [`probe_gather_directions`], made once per volume bake.
// The probe's inputs plus its shared direction set; see `sample_radiance_rgb`.
#[allow(clippy::too_many_arguments)]
fn bake_probe_rgb_along(
    directions: &[Vec3],
    inputs: &ShBakeCtx<'_>,
    probe_pos: Vec3,
    static_lights: &[&MapLight],
    light_global_indices: Option<&[u64]>,
    far_sentinel: f32,
    probe_index: u64,
    source: RadianceSource<'_>,
) -> ([f32; 27], f32, f32) {
    debug_assert_eq!(directions.len(), RAYS_PER_PROBE as usize);
    let ctx = inputs.ray_ctx();
    let mc_weight = (4.0 * std::f32::consts::PI) / RAYS_PER_PROBE as f32;
    let mut acc = [0f32; 27];
    let mut sum_d = 0.0f32;
    let mut sum_d2 = 0.0f32;
    for (ray_index, dir) in directions.iter().enumerate() {
        #[cfg(test)]
        crate::bake_counters::record_direction(
            crate::bake_counters::Gather::Probe,
            probe_index,
            ray_index as u32,
            *dir,
        );
        let (radiance, distance) = match source {
            RadianceSource::Lights(visibility) => sample_radiance_rgb(
                &ctx,
                probe_pos,
                *dir,
                static_lights,
                light_global_indices,
                far_sentinel,
                probe_index,
                ray_index as u64,
                visibility,
            ),
            RadianceSource::Patches(patches) => patches.sample(&ctx, probe_pos, *dir, far_sentinel),
        };
        accumulate_sh_rgb(&mut acc, *dir, radiance, mc_weight);
        sum_d += distance;
        sum_d2 += distance * distance;
    }
    apply_cosine_lobe_rgb(&mut acc);
    (acc, sum_d, sum_d2)
}

/// Bake one probe's SH coefficients + depth-moment metadata with the per-hit
/// estimator, as the whole-volume `bake_sh_volume` path does for each probe.
/// `probe_index` is the probe's flat linear index in the global grid
/// (`x + y*nx + z*nx*ny`); it seeds only the deterministic soft-visibility
/// lattice rotation, so passing the same global index keeps the per-probe
/// output byte-identical to the volume's. Invalid probes bake nothing and
/// return the default (zeroed) record, matching the volume path.
///
/// `light_global_indices` maps each `static_lights[i]` to its position in the
/// full global light slice for the soft-visibility seed (see
/// `sample_radiance_rgb`). The whole-volume path passes `None` (its slice IS the
/// global set); a bounded subset passes its global indices so a kept light gets
/// the same lattice rotation it would in the full-set bake.
#[cfg(test)]
pub(crate) fn bake_probe(
    inputs: &ShBakeCtx<'_>,
    probe_pos: Vec3,
    static_lights: &[&MapLight],
    light_global_indices: Option<&[u64]>,
    far_sentinel: f32,
    valid: bool,
    probe_index: u64,
) -> BakedProbe {
    bake_probe_with_source(
        &probe_gather_directions(),
        inputs,
        probe_pos,
        static_lights,
        light_global_indices,
        far_sentinel,
        valid,
        probe_index,
        RadianceSource::Lights(BounceVisibility::OneSample),
    )
}

/// [`bake_probe`] with the gather hits' radiance source chosen, along
/// `directions` ([`probe_gather_directions`]).
// `bake_probe`'s inputs plus the radiance source; see `sample_radiance_rgb`.
#[allow(clippy::too_many_arguments)]
pub(super) fn bake_probe_with_source(
    directions: &[Vec3],
    inputs: &ShBakeCtx<'_>,
    probe_pos: Vec3,
    static_lights: &[&MapLight],
    light_global_indices: Option<&[u64]>,
    far_sentinel: f32,
    valid: bool,
    probe_index: u64,
    source: RadianceSource<'_>,
) -> BakedProbe {
    if !valid {
        return BakedProbe::default();
    }
    let (coefficients, sum_d, sum_d2) = bake_probe_rgb_along(
        directions,
        inputs,
        probe_pos,
        static_lights,
        light_global_indices,
        far_sentinel,
        probe_index,
        source,
    );
    // All RAYS_PER_PROBE rays accumulate (sky misses via the sentinel), so these
    // are exact divisions by the constant.
    let mean_distance = sum_d / RAYS_PER_PROBE as f32;
    let mean_sq_distance = sum_d2 / RAYS_PER_PROBE as f32;
    BakedProbe {
        coefficients,
        metadata: OctahedralShProbe {
            validity: 1,
            mean_distance: f32_to_f16_bits(mean_distance),
            mean_sq_distance: f32_to_f16_bits(mean_sq_distance),
            density_level: 0,
            node_scale: 0,
        },
    }
}
