// Embedded emitters: point and spot lights whose emitter sphere reaches into geometry.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use bvh::bvh::Bvh;
use glam::Vec3;
use rayon::prelude::*;

use super::clip_reach::front_faces_clear;
use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;
use crate::lightmap_bake::{RAY_EPSILON, probe_sample_direction};
use crate::map_data::{LightType, MapLight, ShadowType};

/// Directions the emitter test traces from a light's centre: the unrotated
/// sphere lattice at this count, fixed so every bake asks the same question.
/// A cap of the sphere smaller than about one direction's share (0.2%) can
/// go unseen, and the early-out then misses no more of the light than that.
///
/// Changing this, or the segment's `RAY_EPSILON` lead, changes the verdict,
/// which no cache key holds: bump both `LAYER_FORMAT_VERSION` and the
/// weight-map `STAGE_VERSION`.
pub(crate) const EMBEDDED_EMITTER_DIRECTIONS: u32 = 512;

/// The fraction of a point or spot light's emitter sphere that lies inside or
/// behind geometry, seen from the light's centre: the share of
/// [`EMBEDDED_EMITTER_DIRECTIONS`] lattice directions whose segment from the
/// centre out to the sphere enters a triangle through its front. Zero for a
/// sun or a hard (`light_size` 0) light, which have no sphere, and for an
/// `sdf` light, which bakes no texel.
///
/// An emitter sample in such a cap is hidden from every open texel, so a
/// texel's true visibility of the light is below 1 even with nothing between
/// them. The nine footprint probes aim at the emitter's centre and its rim as
/// seen from the texel, so a cap facing the texel along the light direction
/// escapes them all and the probe early-out would store 1.0; a light this
/// returns a non-zero fraction for traces every texel's full set
/// ([`super::TexelSampler::new`]).
///
/// Each segment starts `RAY_EPSILON` behind the centre, so a face through
/// the centre itself, where an author snapped the light onto a wall, is
/// reached by every direction. Only fronts count ([`front_faces_clear`]): a
/// direction heading into the wall starts on its open side and enters the face
/// through its front, while one heading out starts behind it and crosses only
/// a back face, so it never counts. A centre up to `RAY_EPSILON` inside a wall, which the
/// buried-light test keeps (`buried_lights.rs`), is still flagged, but counts
/// only the directions whose lead starts in front of the face, so its
/// fraction reads low.
pub(crate) fn embedded_emitter_fraction(
    light: &MapLight,
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
) -> f32 {
    if !has_emitter_sphere(light) {
        return 0.0;
    }
    let embedded = (0..EMBEDDED_EMITTER_DIRECTIONS)
        .filter(|&index| direction_embedded(light, index, bvh, primitives, geometry))
        .count();
    embedded as f32 / EMBEDDED_EMITTER_DIRECTIONS as f32
}

/// Whether [`embedded_emitter_fraction`] is non-zero, stopping at the first
/// embedded direction: for callers that need only the light's flag, not the
/// warning's fraction.
pub(crate) fn has_embedded_emitter(
    light: &MapLight,
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
) -> bool {
    has_emitter_sphere(light)
        && (0..EMBEDDED_EMITTER_DIRECTIONS)
            .any(|index| direction_embedded(light, index, bvh, primitives, geometry))
}

/// Whether lattice direction `index`'s segment, from `RAY_EPSILON` behind the
/// light's centre out to its sphere, enters a triangle through its front.
fn direction_embedded(
    light: &MapLight,
    index: u32,
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
) -> bool {
    // The emitter's centre as the bake's shadow rays see it
    // (`hard_ray_target`, `area_sample_target`).
    let centre = light.origin.as_vec3();
    let direction: Vec3 = probe_sample_direction(light, index, EMBEDDED_EMITTER_DIRECTIONS);
    !front_faces_clear(
        bvh,
        primitives,
        geometry,
        centre - direction * RAY_EPSILON,
        centre + direction * light.light_size,
        |_| true,
    )
}

/// Whether the emitter test has a sphere to trace: a point or spot light of
/// finite, non-zero size that bakes texels.
fn has_emitter_sphere(light: &MapLight) -> bool {
    matches!(light.light_type, LightType::Point | LightType::Spot)
        && light.light_size > 0.0
        && light.light_size.is_finite()
        && light.shadow_type != ShadowType::Sdf
}

/// The lights of one bake whose emitter sphere reaches into geometry, with
/// the fraction embedded, keyed by the caller's light index (a source index
/// or a position in the stage's light list).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct EmbeddedEmitters {
    /// `(key, fraction)` for every embedded light, ascending by key.
    embedded: Vec<(usize, f32)>,
}

/// No light embedded, for tests whose lights stand clear.
#[cfg(test)]
pub(crate) static NO_EMBEDDED_EMITTERS: EmbeddedEmitters = EmbeddedEmitters {
    embedded: Vec::new(),
};

impl EmbeddedEmitters {
    /// Test every `(key, light)` once, one governed item per light with a
    /// sphere; hard lights and suns trace nothing and take no permit. Keys
    /// must ascend.
    pub(crate) fn classify<'l>(
        lights: impl IntoIterator<Item = (usize, &'l MapLight)>,
        bvh: &Bvh<f32, 3>,
        primitives: &[BvhPrimitive],
        geometry: &GeometryResult,
        control: &BakeControl,
    ) -> Self {
        let lights: Vec<(usize, &MapLight)> = lights
            .into_iter()
            .filter(|&(_, light)| has_emitter_sphere(light))
            .collect();
        // Strictly: a duplicate key would leave `fraction`'s binary search
        // free to find either entry.
        debug_assert!(lights.windows(2).all(|pair| pair[0].0 < pair[1].0));
        // Indexed collect keeps key order whatever worker ran which light.
        let fractions: Vec<f32> = lights
            .par_iter()
            .map(|&(_, light)| {
                let _permit = control.governor().enter();
                embedded_emitter_fraction(light, bvh, primitives, geometry)
            })
            .collect();
        Self {
            embedded: lights
                .iter()
                .zip(fractions)
                .filter(|&(_, fraction)| fraction > 0.0)
                .map(|(&(key, _), fraction)| (key, fraction))
                .collect(),
        }
    }

    pub(crate) fn is_embedded(&self, key: usize) -> bool {
        self.fraction(key) > 0.0
    }

    /// The light's embedded fraction; zero for a clear or unknown key.
    pub(crate) fn fraction(&self, key: usize) -> f32 {
        self.embedded
            .binary_search_by_key(&key, |&(key, _)| key)
            .map_or(0.0, |at| self.embedded[at].1)
    }

    /// Warn once per embedded light, in key order, naming it by `label`
    /// (`light_source_labels`, which give entity index, classname and
    /// authored origin) and `kind`, as the texel-step warnings do. The stage
    /// that owns the light calls this on its caller thread every compile,
    /// cold or warm: the test runs whether or not a cache serves the bake,
    /// so the warning never depends on a memo.
    pub(crate) fn warn<'l>(
        &self,
        kind: &str,
        light: impl Fn(usize) -> Option<&'l MapLight>,
        label: impl Fn(usize) -> Option<&'l str>,
    ) {
        for &(key, fraction) in &self.embedded {
            let name = label(key).map_or_else(|| format!("#{key}"), str::to_owned);
            let size = light(key).map_or(0.0, |light| light.light_size);
            log::warn!(
                "[Lightmap] emitter inside geometry: {kind} light {name}: {:.1}% of its \
                 {size:.3} m emitter sphere lies inside or behind a surface, which dims the \
                 light by about that much and makes its lightmap texels trace every shadow \
                 sample. Move the light off the surface or shrink `_light_size`.",
                fraction * 100.0,
            );
        }
    }
}

#[cfg(test)]
mod tests;
