// Reflection-probe hit shading: what the forward pass draws at a static-world point, split into static and per-light terms.
// See: context/lib/resource_management.md §5.2 · rendering_pipeline.md §4

use glam::Vec3;

use super::animated_direct::AnimatedDirectSampler;
use super::delta_sample::DeltaShSampler;
use super::hit::SceneTracer;
use super::lightmap_sample::LightmapSampler;
use super::materials::SurfaceMaterials;
use super::sdf_lights::SdfLights;
use super::sh_sample::ShVolumeSampler;

/// The renderer's production SH indirect weight, `DEFAULT_INDIRECT_SCALE`
/// (`renderer_types.rs`), which the forward and fluid passes multiply into SH
/// indirect, base and delta alike. A capture at full weight would reflect
/// bounce about 3× brighter than the room it mirrors; the diagnostics slider
/// cannot reach a bake.
pub(crate) const PRODUCTION_INDIRECT_WEIGHT: f32 = 0.33;

/// Shadow-ray start above a hit, along its face normal, so the receiving face
/// never occludes its own sdf light; the chunk light list's receiver offset.
const SDF_SHADOW_NORMAL_OFFSET_M: f32 = 2.0e-3;

/// Everything a capture ray reads: the static world, its materials, the
/// encoded lightmap, the packed base SH volume, the animated lights' baked
/// transport and the runtime's sdf lights.
pub(crate) struct CaptureScene<'a> {
    pub tracer: SceneTracer<'a>,
    pub materials: SurfaceMaterials<'a>,
    pub lightmap: LightmapSampler<'a>,
    pub sh: ShVolumeSampler<'a>,
    /// Id 25, when the level has animated direct.
    pub animated_direct: Option<AnimatedDirectSampler<'a>>,
    /// Id 27, when the level has animated indirect.
    pub deltas: Option<DeltaShSampler<'a>>,
    pub sdf: SdfLights,
    /// Longest ray worth tracing: past the world bounds nothing is hit.
    pub max_distance: f32,
}

/// Each animated light's contribution at one hit, by AnimatedBakedLights
/// roster index: unit-radiance transport times albedo, so the light's share
/// of the captured radiance is this times its reference value.
#[derive(Debug, Clone, Default)]
pub(crate) struct AnimatedTerms {
    lights: Vec<(u32, Vec3)>,
}

impl AnimatedTerms {
    pub(crate) fn clear(&mut self) {
        self.lights.clear();
    }

    pub(crate) fn add(&mut self, light: u32, value: Vec3) {
        match self.lights.iter_mut().find(|(l, _)| *l == light) {
            Some((_, sum)) => *sum += value,
            None => self.lights.push((light, value)),
        }
    }

    pub(crate) fn lights(&self) -> &[(u32, Vec3)] {
        &self.lights
    }
}

/// Per-thread buffers one capture ray reuses.
#[derive(Debug, Default)]
pub(crate) struct ShadeScratch {
    /// The animated terms of the last [`CaptureScene::radiance`] call.
    pub animated: AnimatedTerms,
    slots: Vec<(u32, f32)>,
}

impl CaptureScene<'_> {
    /// Radiance arriving at `origin` from unit `direction`, split as the
    /// capture composes it: the returned static radiance, and in
    /// `scratch.animated` each animated light's unit-radiance contribution;
    /// beside it the hit's distance along `direction`. Black with no animated
    /// terms and no distance on a miss. `sample_angle` is the angular
    /// width one sample stands for, which picks the texture mip a pixel of
    /// that footprint would read.
    ///
    /// Static is `albedo × (lightmap + selected sdf direct + base SH ×
    /// production weight) + emissive × prefix multiplier`; a light's unit term
    /// is `albedo × (its weight-map direct + its delta SH × production
    /// weight)`. These are the forward pass's terms with the shading normal at
    /// the face normal (no normal map, so both bumped-Lambert factors are 1).
    /// Its ambient floor is 0 in production and its promoted-light subtraction
    /// needs a runtime entity, so neither enters. An animated sdf light's
    /// direct is static: the forward sdf loop reads the spec buffer's fixed
    /// colour, never its curve, and draws it whether or not the light is
    /// active, so only its delta SH follows its curve.
    pub(crate) fn radiance(
        &self,
        origin: Vec3,
        direction: Vec3,
        sample_angle: f32,
        scratch: &mut ShadeScratch,
    ) -> anyhow::Result<(Vec3, Option<f32>)> {
        scratch.animated.clear();
        let Some(hit) = self
            .tracer
            .closest_hit(origin, direction, self.max_distance)
        else {
            return Ok((Vec3::ZERO, None));
        };
        let surface = self.tracer.surface(&hit, origin, direction);
        let footprint_texels = hit.distance * sample_angle * surface.texel_density;
        let material = self
            .materials
            .sample(surface.texture_index, surface.uv, footprint_texels);
        let mut direct = self
            .lightmap
            .irradiance(surface.lightmap_block, surface.lightmap_uv)?;
        if !self.sdf.is_empty() {
            let selection = self.sdf.select(surface.position);
            let shadow_origin = surface.position + surface.normal * SDF_SHADOW_NORMAL_OFFSET_M;
            direct += self
                .sdf
                .direct(&selection, surface.position, surface.normal, |light| {
                    self.tracer.segment_clear(shadow_origin, light)
                });
        }
        let slots = self.deltas.as_ref().map(|_| &mut scratch.slots);
        let indirect = self
            .sh
            .indirect_and_slots(surface.position, surface.normal, slots);
        if let Some(animated) = &self.animated_direct {
            animated.accumulate(
                surface.animated_block,
                surface.lightmap_block,
                surface.lightmap_uv,
                material.albedo,
                &mut scratch.animated,
            );
        }
        if let Some(deltas) = &self.deltas {
            deltas.accumulate(
                &scratch.slots,
                &self.sh.tile_taps(surface.normal),
                material.albedo * PRODUCTION_INDIRECT_WEIGHT,
                &mut scratch.animated,
            );
        }
        Ok((
            shade(material.albedo, direct, indirect, material.emissive),
            Some(hit.distance),
        ))
    }
}

/// The forward pass's static composition, `forward.wgsl` `fs_main`:
/// `base_color.rgb * total_light + emissive * emissive_strength`, with
/// `total_light = indirect * indirect_scale + static_direct`.
pub(crate) fn shade(albedo: Vec3, direct: Vec3, indirect: Vec3, emissive: Vec3) -> Vec3 {
    albedo * (direct + indirect * PRODUCTION_INDIRECT_WEIGHT) + emissive
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The renderer constant is not reachable from a compiler dev-dependency,
    /// so the parity test reads its definition from the renderer source.
    #[test]
    fn indirect_weight_matches_the_renderer_production_default() {
        let source = include_str!("../../../renderer/src/render/renderer_types.rs");
        let line = source
            .lines()
            .find(|line| line.contains("pub const DEFAULT_INDIRECT_SCALE: f32"))
            .expect("renderer_types.rs defines DEFAULT_INDIRECT_SCALE");
        let value: f32 = line
            .split('=')
            .nth(1)
            .and_then(|rhs| rhs.trim().trim_end_matches(';').trim().parse().ok())
            .expect("DEFAULT_INDIRECT_SCALE is a float literal");
        assert_eq!(PRODUCTION_INDIRECT_WEIGHT, value);
    }

    /// The forward pass adds no ambient term in production: its floor's
    /// default is 0. A non-zero default would add a term the capture lacks.
    #[test]
    fn renderer_ambient_floor_default_is_zero() {
        let source = include_str!("../../../renderer/src/render/renderer_types.rs");
        assert!(
            source.contains("pub const DEFAULT_AMBIENT_FLOOR: f32 = 0.0;"),
            "the capture omits the ambient floor because its production default is 0"
        );
    }

    #[test]
    fn shade_weights_indirect_but_not_direct_or_emissive() {
        let albedo = Vec3::new(0.5, 1.0, 0.0);
        let lit = shade(
            albedo,
            Vec3::splat(2.0),
            Vec3::splat(1.0),
            Vec3::new(0.0, 0.0, 3.0),
        );
        let expected = albedo * (2.0 + PRODUCTION_INDIRECT_WEIGHT) + Vec3::new(0.0, 0.0, 3.0);
        assert!((lit - expected).length() < 1e-6, "{lit}");
    }
}
