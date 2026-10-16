// SPIKE (claude/blend-cost-spike, never merged): the reflection blend-cost
// candidates' CPU reference — synthetic neighbours, region-distance weights,
// the per-probe precomputed lobe ratio and the stochastic per-cell pick that
// `fluid_surface.wgsl`'s debug blend lever mirrors.
// See: context/plans/drafts/reflection-probe-seams--blend-cost-spike/index.md

use glam::{IVec2, Vec2, Vec3, Vec4};
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_LIGHTS_PER_PROBE, REFLECTION_PROBE_LOBE_COUNT, ReflectionProbeRecord,
    reflection_probe_lobe_weights,
};

use super::fog::fog_proximity;
use super::motion::{hash_unit, hash3, texel_cell};
use super::reflection::{
    FLUID_REFLECTION_NO_PROBE, FluidProbeGpu, FluidProbeLightGpu, FluidReflectionFragment,
    FluidReflectionLights, reflection_above_horizon, reflection_composite, reflection_fresnel,
    reflection_gate, reflection_light_change, reflection_light_present, reflection_light_value,
    reflection_mirror, reflection_ratio, reflection_tile_uv, reflection_unpack_unorm10x3,
};
use super::reflection_search::reflection_search;
use super::reflection_style::reflection_point;

/// The lever's mode field (`blend_mode` bits 0..7).
pub const FLUID_BLEND_MODE_SINGLE: u32 = 0;
pub const FLUID_BLEND_MODE_BLEND: u32 = 1;
pub const FLUID_BLEND_MODE_PRECOMPUTED: u32 = 2;
pub const FLUID_BLEND_MODE_STOCHASTIC: u32 = 3;
pub const FLUID_BLEND_MODE_MASK: u32 = 0xFF;
/// `blend_mode` bit 8: every valid neighbour weighs at least
/// [`FLUID_BLEND_COVERAGE_FLOOR`], so every fragment takes all three taps.
pub const FLUID_BLEND_COVERAGE_BIT: u32 = 0x100;
pub const FLUID_BLEND_COVERAGE_FLOOR: f32 = 0.25;
/// Provisional band width W, metres (the seams brief pins none yet).
pub const FLUID_BLEND_DEFAULT_BAND: f32 = 1.0;
/// Same-plane test for neighbour selection: capture-point heights within this.
pub const FLUID_BLEND_PLANE_EPSILON: f32 = 0.01;
/// Salt of the stochastic pick's cell hash.
pub const FLUID_SALT_BLEND_PICK: u32 = 0xB529_7A4D;
/// Bytes per probe of the precomputed lobe ratios: five `vec4<f32>`.
pub const FLUID_RATIO_LOBES_STRIDE: usize = REFLECTION_PROBE_LOBE_COUNT * 16;
/// The precomputed ratio uniform at the probe cap: 256 × 5 × vec4 = 20 KiB.
pub const FLUID_RATIO_LOBES_BYTES: usize = 256 * FLUID_RATIO_LOBES_STRIDE;

/// The debug lever's four candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FluidReflectionBlendMode {
    #[default]
    Single,
    Blend,
    Precomputed,
    Stochastic,
}

impl FluidReflectionBlendMode {
    pub const ALL: [Self; 4] = [
        Self::Single,
        Self::Blend,
        Self::Precomputed,
        Self::Stochastic,
    ];

    pub const fn uniform_value(self) -> u32 {
        match self {
            Self::Single => FLUID_BLEND_MODE_SINGLE,
            Self::Blend => FLUID_BLEND_MODE_BLEND,
            Self::Precomputed => FLUID_BLEND_MODE_PRECOMPUTED,
            Self::Stochastic => FLUID_BLEND_MODE_STOCHASTIC,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Blend => "blend",
            Self::Precomputed => "precomputed",
            Self::Stochastic => "stochastic",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.name() == name)
    }
}

/// The scene uniform's `blend_mode` word.
pub const fn pack_blend_word(mode: FluidReflectionBlendMode, coverage: bool) -> u32 {
    mode.uniform_value()
        | if coverage {
            FLUID_BLEND_COVERAGE_BIT
        } else {
            0
        }
}

/// Horizontal (X/Z) distance between two boxes' rectangles, 0 when they touch
/// or overlap.
fn rect_rect_distance_xz(a_min: Vec3, a_max: Vec3, b_min: Vec3, b_max: Vec3) -> f32 {
    let dx = (b_min.x - a_max.x).max(a_min.x - b_max.x).max(0.0);
    let dz = (b_min.z - a_max.z).max(a_min.z - b_max.z).max(0.0);
    (dx * dx + dz * dz).sqrt()
}

/// Horizontal (X/Z) distance from `point` to a box's rectangle, 0 inside.
pub fn point_rect_distance_xz(point: Vec3, box_min: Vec3, box_max: Vec3) -> f32 {
    let dx = (box_min.x - point.x).max(point.x - box_max.x).max(0.0);
    let dz = (box_min.z - point.z).max(point.z - box_max.z).max(0.0);
    (dx * dx + dz * dz).sqrt()
}

/// Synthetic neighbours, two per probe: the other probes whose capture point
/// shares this one's plane height and whose box rectangles lie nearest its
/// own (ties to the lower index); short of two with three or more probes,
/// the next indices fill in; else [`FLUID_REFLECTION_NO_PROBE`].
pub fn probe_neighbours(probes: &[ReflectionProbeRecord]) -> Vec<[u32; 2]> {
    let n = probes.len();
    (0..n)
        .map(|p| {
            let own = &probes[p];
            let mut candidates: Vec<(f32, usize)> = (0..n)
                .filter(|&q| {
                    q != p
                        && (probes[q].capture_point[1] - own.capture_point[1]).abs()
                            <= FLUID_BLEND_PLANE_EPSILON
                })
                .map(|q| {
                    let distance = rect_rect_distance_xz(
                        Vec3::from(own.box_min),
                        Vec3::from(own.box_max),
                        Vec3::from(probes[q].box_min),
                        Vec3::from(probes[q].box_max),
                    );
                    (distance, q)
                })
                .collect();
            // Stable sort on distance keeps ascending index among ties.
            candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut chosen: Vec<u32> = candidates.iter().take(2).map(|&(_, q)| q as u32).collect();
            if chosen.len() < 2 && n >= 3 {
                for step in 1..n {
                    if chosen.len() == 2 {
                        break;
                    }
                    let q = ((p + step) % n) as u32;
                    if !chosen.contains(&q) {
                        chosen.push(q);
                    }
                }
            }
            [
                chosen.first().copied().unwrap_or(FLUID_REFLECTION_NO_PROBE),
                chosen.get(1).copied().unwrap_or(FLUID_REFLECTION_NO_PROBE),
            ]
        })
        .collect()
}

/// One fragment's taps: the own probe then its two neighbours, each with its
/// normalised weight (0 for a skipped tap).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidBlendTaps {
    pub probes: [u32; 3],
    pub weights: [f32; 3],
}

/// One neighbour's unnormalised falloff `max(0, 1 − d/W)` for `d` the
/// position's horizontal distance to the neighbour's box rectangle; with
/// coverage, at least the floor. A missing neighbour weighs 0.
pub fn neighbour_falloff(
    position: Vec3,
    neighbour: u32,
    probes: &[ReflectionProbeRecord],
    band: f32,
    coverage: bool,
) -> f32 {
    if neighbour == FLUID_REFLECTION_NO_PROBE {
        return 0.0;
    }
    let probe = &probes[neighbour as usize];
    let distance = point_rect_distance_xz(
        position,
        Vec3::from(probe.box_min),
        Vec3::from(probe.box_max),
    );
    let falloff = if band > 0.0 {
        (1.0 - distance / band).max(0.0)
    } else {
        0.0
    };
    if coverage {
        falloff.max(FLUID_BLEND_COVERAGE_FLOOR)
    } else {
        falloff
    }
}

/// The shader's weight path: f_P = 1, each neighbour's falloff, normalised.
pub fn blend_taps(
    own: u32,
    neighbours: [u32; 2],
    position: Vec3,
    probes: &[ReflectionProbeRecord],
    band: f32,
    coverage: bool,
) -> FluidBlendTaps {
    let f1 = neighbour_falloff(position, neighbours[0], probes, band, coverage);
    let f2 = neighbour_falloff(position, neighbours[1], probes, band, coverage);
    let inv = 1.0 / (1.0 + f1 + f2);
    FluidBlendTaps {
        probes: [own, neighbours[0], neighbours[1]],
        weights: [inv, f1 * inv, f2 * inv],
    }
}

/// The stochastic pick's uniform draw for one water texel cell of one draw
/// slot: stable under camera motion and across frames.
pub fn stochastic_draw(cell: IVec2, slot: u32) -> f32 {
    hash_unit(hash3(
        cell.x as u32,
        cell.y as u32,
        slot ^ FLUID_SALT_BLEND_PICK,
    ))
}

/// Which tap the cell takes: tap i with probability `weights[i]`; a draw
/// past the cumulative sum by rounding stays on the last tap with weight.
pub fn stochastic_pick(weights: [f32; 3], u: f32) -> usize {
    let mut pick = 0;
    let mut acc = weights[0];
    if u >= acc && weights[1] > 0.0 {
        pick = 1;
    }
    acc += weights[1];
    if u >= acc && weights[2] > 0.0 {
        pick = 2;
    }
    pick
}

/// Per lobe, the probe's unclamped live ratio `1 + Σ share_lobe × change`
/// over its stored lights (shares as the shader unpacks them). Because the
/// lobe weights sum to 1, `max(Σ w_lobe · R_lobe, 0)` is
/// [`reflection_ratio`]: its clamp sits outside the lobe sum, so these stay
/// unclamped and the clamp follows the blend.
pub fn ratio_lobes(
    probe_lights: &[FluidProbeLightGpu; REFLECTION_PROBE_LIGHTS_PER_PROBE],
    lights: &FluidReflectionLights<'_>,
) -> [Vec3; REFLECTION_PROBE_LOBE_COUNT] {
    let mut lobes = [Vec3::ONE; REFLECTION_PROBE_LOBE_COUNT];
    for light in probe_lights {
        if !reflection_light_present(lights, light.roster_index) {
            continue;
        }
        let change = reflection_light_change(
            reflection_light_value(lights, light.roster_index),
            Vec3::from(light.reference),
        );
        for (lobe, word) in lobes.iter_mut().zip(&light.shares) {
            *lobe += reflection_unpack_unorm10x3(*word) * change;
        }
    }
    lobes
}

/// The precomputed ratio toward `direction`: the lobe blend, then the clamp.
pub fn precomputed_ratio(lobes: &[Vec3; REFLECTION_PROBE_LOBE_COUNT], direction: Vec3) -> Vec3 {
    let weights = reflection_probe_lobe_weights(direction.to_array());
    let mut ratio = Vec3::ZERO;
    for (lobe, weight) in lobes.iter().zip(weights) {
        ratio += *lobe * weight;
    }
    ratio.max(Vec3::ZERO)
}

/// Append one probe's lobe ratios as the shader's binding reads them.
pub fn pack_ratio_lobes(lobes: &[Vec3; REFLECTION_PROBE_LOBE_COUNT], out: &mut Vec<f32>) {
    for lobe in lobes {
        out.extend_from_slice(&[lobe.x, lobe.y, lobe.z, 0.0]);
    }
}

/// The blend lever's inputs for one fragment beside the single path's.
#[derive(Debug, Clone, Copy)]
pub struct FluidBlendLever<'a> {
    pub mode: FluidReflectionBlendMode,
    pub coverage: bool,
    pub band: f32,
    /// Per probe, its two synthetic neighbours.
    pub neighbours: &'a [[u32; 2]],
    /// The fragment's draw slot (`in.slot`), the stochastic hash's face key.
    pub slot: u32,
}

/// `fs_main`'s tail under the blend lever: single is
/// [`super::reflection::reflection_fragment`]; blend and precomputed mix the
/// taps' captures and fade weights by the normalised weights before Fresnel;
/// stochastic takes the cell's one picked tap. Every tap searches from the
/// own probe's style point.
#[allow(clippy::too_many_arguments)]
pub fn reflection_blend_fragment(
    surface: Vec4,
    fragment: &FluidReflectionFragment,
    probes: &[ReflectionProbeRecord],
    tile: [u32; 2],
    lights: &FluidReflectionLights<'_>,
    lever: &FluidBlendLever<'_>,
    sample: impl Fn(u32, Vec2) -> Vec3,
    distance: impl Fn(u32, Vec2) -> f32,
) -> Vec4 {
    let view = (fragment.camera - fragment.world_position).normalize_or_zero();
    let outward = fragment.outward.normalize();
    if !reflection_gate(
        fragment.probe,
        outward,
        view,
        fragment.light_term_mask,
        fragment.reflectivity,
    ) {
        return surface;
    }
    let own = &probes[fragment.probe as usize];
    let point = reflection_point(
        fragment.style,
        fragment.texel_uv,
        fragment.world_position,
        outward,
        fragment.axes,
        fragment.time,
        fragment.texels_per_pixel,
        Vec3::from(own.box_min),
        Vec3::from(own.box_max),
        Vec3::from(own.capture_point),
    );
    let view = (fragment.camera - point.position).normalize_or_zero();
    let ray = reflection_above_horizon(reflection_mirror(view, point.normal));
    let tap = |index: u32, precomputed: bool| -> (Vec3, f32) {
        let probe = &probes[index as usize];
        let search = reflection_search(
            point.position,
            ray,
            Vec3::from(probe.box_min),
            Vec3::from(probe.box_max),
            Vec3::from(probe.capture_point),
            tile,
            |uv| distance(probe.layer, uv),
        );
        let packed = FluidProbeGpu::pack(probe).lights;
        let ratio = if precomputed {
            precomputed_ratio(&ratio_lobes(&packed, lights), search.lookup)
        } else {
            reflection_ratio(&packed, search.lookup, lights)
        };
        let capture = sample(
            probe.layer,
            reflection_tile_uv(search.lookup, tile[0], tile[1]),
        ) * ratio;
        (capture, search.weight)
    };
    let (capture, weight) = match lever.mode {
        FluidReflectionBlendMode::Single => tap(fragment.probe, false),
        mode => {
            let taps = blend_taps(
                fragment.probe,
                lever.neighbours[fragment.probe as usize],
                point.position,
                probes,
                lever.band,
                lever.coverage,
            );
            if mode == FluidReflectionBlendMode::Stochastic {
                let pick = stochastic_pick(
                    taps.weights,
                    stochastic_draw(texel_cell(fragment.texel_uv), lever.slot),
                );
                tap(taps.probes[pick], false)
            } else {
                let precomputed = mode == FluidReflectionBlendMode::Precomputed;
                let mut capture = Vec3::ZERO;
                let mut weight = 0.0;
                for (probe, w) in taps.probes.into_iter().zip(taps.weights) {
                    if w > 0.0 {
                        let (c, fade) = tap(probe, precomputed);
                        capture += c * w;
                        weight += fade * w;
                    }
                }
                (capture, weight)
            }
        }
    };
    let fresnel = reflection_fresnel(view.dot(outward), fragment.reflectivity) * weight;
    let band = fog_proximity(fragment.camera, fragment.world_position, outward);
    reflection_composite(surface, capture, fresnel * band)
}

#[cfg(test)]
#[path = "reflection_blend_tests.rs"]
mod tests;
