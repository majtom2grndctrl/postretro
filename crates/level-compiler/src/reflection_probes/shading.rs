// Reflection-probe hit shading: what the forward pass draws at a static-world point, static terms only.
// See: context/lib/resource_management.md §5.2 · rendering_pipeline.md §4

use glam::Vec3;

use super::hit::SceneTracer;
use super::lightmap_sample::LightmapSampler;
use super::materials::SurfaceMaterials;
use super::sh_sample::ShVolumeSampler;

/// The renderer's production SH indirect weight, `DEFAULT_INDIRECT_SCALE`
/// (`renderer_types.rs`), which the forward and fluid passes multiply into SH
/// indirect. A capture at full weight would reflect bounce about 3× brighter
/// than the room it mirrors; the diagnostics slider cannot reach a bake.
pub(crate) const PRODUCTION_INDIRECT_WEIGHT: f32 = 0.33;

/// Everything a capture ray reads: the static world, its materials, the
/// encoded lightmap and the packed base SH volume.
pub(crate) struct CaptureScene<'a> {
    pub tracer: SceneTracer<'a>,
    pub materials: SurfaceMaterials<'a>,
    pub lightmap: LightmapSampler<'a>,
    pub sh: ShVolumeSampler<'a>,
    /// Longest ray worth tracing: past the world bounds nothing is hit.
    pub max_distance: f32,
}

impl CaptureScene<'_> {
    /// Radiance arriving at `origin` from unit `direction`; black on a miss.
    /// `sample_angle` is the angular width one sample stands for, which picks
    /// the texture mip a pixel of that footprint would read.
    ///
    /// Shading is `albedo × (lightmap direct + SH indirect × production
    /// weight) + emissive × prefix multiplier`: the forward pass's static
    /// terms with the shading normal at the face normal (no normal map, so the
    /// bumped-Lambert correction is 1). Its ambient floor is 0 in production
    /// and its promoted-light subtraction needs a runtime entity, so neither
    /// enters. Animated weight maps, delta SH and SDF-shadowed lights join in
    /// capture completeness.
    pub(crate) fn radiance(
        &self,
        origin: Vec3,
        direction: Vec3,
        sample_angle: f32,
    ) -> anyhow::Result<Vec3> {
        let Some(hit) = self
            .tracer
            .closest_hit(origin, direction, self.max_distance)
        else {
            return Ok(Vec3::ZERO);
        };
        let surface = self.tracer.surface(&hit, origin, direction);
        let footprint_texels = hit.distance * sample_angle * surface.texel_density;
        let material = self
            .materials
            .sample(surface.texture_index, surface.uv, footprint_texels);
        let direct = self
            .lightmap
            .irradiance(surface.lightmap_block, surface.lightmap_uv)?;
        let indirect = self.sh.indirect(surface.position, surface.normal);
        Ok(shade(material.albedo, direct, indirect, material.emissive))
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
