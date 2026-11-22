// Direct-at-probe SH of an area light: analytic L0–L2 over the polygons plus a control variate.
// See: context/lib/build_pipeline.md §Compiler pipeline (Area lights)

use glam::{DMat3, DVec3, Vec3};

use super::probe::probe_direct_joint_visibility;
use super::{RaytracingCtx, segment_clear};
use crate::area_light::{AreaEmitter, reach_window};
use crate::lightmap_bake::{AreaProbeVisibility, DEFAULT_AREA_SAMPLE_COUNT, area_probe_visibility};
use crate::map_data::MapLight;
use crate::mover_probe_channel::PosedOccluder;
use crate::texel_footprint::ProbeEarlyOut;

/// `sh_math::sh_basis_l2` in f64: the repo's real L2 basis (Ramamoorthi–Hanrahan,
/// no Condon–Shortley phase).
fn sh_basis_l2_f64(d: DVec3) -> [f64; 9] {
    let (x, y, z) = (d.x, d.y, d.z);
    [
        0.282_094_791_773_878_1,
        -0.488_602_511_902_919_9 * y,
        0.488_602_511_902_919_9 * z,
        -0.488_602_511_902_919_9 * x,
        1.092_548_430_592_079_2 * x * y,
        -1.092_548_430_592_079_2 * y * z,
        0.315_391_565_252_520_05 * (3.0 * z * z - 1.0),
        -1.092_548_430_592_079_2 * x * z,
        0.546_274_215_296_039_6 * (x * x - y * y),
    ]
}

/// ∫ Y_lm(ω) dω over the solid angle of the polygons whose front half-space
/// holds `p`, per unit radiance, before the cosine lobe: L0 from the solid
/// angle Ω, L1 from the irradiance vector Φ = ∫ω dω, L2 from the second
/// moment M = ∫ωωᵀ dω (research.md §3). Exact; no ray, no quadrature.
pub(crate) fn area_sh_projection(emitter: &AreaEmitter, p: DVec3) -> [f64; 9] {
    let omega = emitter.solid_angle(p);
    let phi = emitter.irradiance_vector(p);
    let m: DMat3 = emitter.second_moment(p);
    // M is symmetric; column-major `m.col(j)[i]` is M_ij.
    let (xx, yy, zz) = (m.x_axis.x, m.y_axis.y, m.z_axis.z);
    let (xy, yz, xz) = (m.y_axis.x, m.z_axis.y, m.z_axis.x);
    let c1 = 0.488_602_511_902_919_9;
    let c2 = 1.092_548_430_592_079_2;
    [
        0.282_094_791_773_878_1 * omega,
        -c1 * phi.y,
        c1 * phi.z,
        -c1 * phi.x,
        c2 * xy,
        -c2 * yz,
        0.315_391_565_252_520_05 * (3.0 * zz - omega),
        -c2 * xz,
        0.546_274_215_296_039_6 * (xx - yy),
    ]
}

/// Adds `light`'s direct term at `probe` to `acc` (raw projection, before the
/// cosine lobe): the analytic projection over the polygons, windowed at the
/// distance to the nearest polygon, scaled by colour × radiance, and shadowed
/// by a control variate on the probe's solid-angle sample set (research.md
/// §3): c = c_analytic + (Ω/N)·Σₖ (Vₖ − 1)·Y(ωₖ). Every sample clear writes
/// the analytic value, every sample blocked writes nothing; no coefficient is
/// ever scaled by its own ratio. With `posed`, the term then scales by the
/// share q of its static-clear samples the pose leaves clear, as every light's
/// does ([`probe_direct_joint_visibility`]). Samples leave the probe lifted
/// along Φ̂, the light's net direction there.
#[allow(clippy::too_many_arguments)]
pub(super) fn accumulate_area_direct(
    acc: &mut [f32; 27],
    ctx: &RaytracingCtx<'_>,
    probe: Vec3,
    light: &MapLight,
    seed: u64,
    early_out: ProbeEarlyOut,
    posed: Option<&PosedOccluder>,
) {
    let Some(emitter) = light.area.as_ref() else {
        return;
    };
    let p = probe.as_dvec3();
    let window = reach_window(emitter.nearest_distance(p), f64::from(light.falloff_range));
    if window <= 0.0 {
        return;
    }
    let Some(lift) = emitter.irradiance_vector(p).try_normalize() else {
        return;
    };
    let lift = lift.as_vec3();
    let Some(visibility) = area_probe_visibility(
        probe,
        lift,
        light,
        seed,
        DEFAULT_AREA_SAMPLE_COUNT,
        early_out,
        |from, to| segment_clear(ctx, from, to),
    ) else {
        return;
    };
    let mut coefficients = area_sh_projection(emitter, p);
    match visibility {
        AreaProbeVisibility::Blocked => return,
        AreaProbeVisibility::Clear => {}
        AreaProbeVisibility::Partial(samples) => {
            let omega = emitter.solid_angle(p);
            let per_sample = omega / samples.len() as f64;
            for sample in samples.iter().filter(|sample| !sample.clear) {
                for (c, y) in coefficients
                    .iter_mut()
                    .zip(sh_basis_l2_f64(sample.direction))
                {
                    *c -= per_sample * y;
                }
            }
        }
    }
    let mut scale = window * f64::from(light.intensity);
    if let Some(occluder) = posed
        && occluder.may_meet(probe, light)
    {
        let joint =
            probe_direct_joint_visibility(ctx, probe, lift, light, seed, early_out, occluder);
        if joint.meets_mover() {
            scale *= f64::from(joint.surviving_byte()) / 255.0;
        }
    }
    if scale <= 0.0 {
        return;
    }
    for (band, c) in coefficients.iter().enumerate() {
        let value = c * scale;
        for (channel, colour) in light.color.iter().enumerate() {
            acc[band * 3 + channel] += (value * f64::from(*colour)) as f32;
        }
    }
}
