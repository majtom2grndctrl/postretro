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
/// go unseen; [`EMBEDDED_EMITTER_MIN_DIRECTIONS`] sets the share that flags.
///
/// Changing this, or the segment's `RAY_EPSILON` lead, changes the verdict,
/// which the lightmap layer and weight-map keys do not hold: bump
/// `LAYER_FORMAT_VERSION` and the weight-map `STAGE_VERSION`. The cached
/// probe-point stages (direct SH, both billboard scatters) key the flagged
/// set ([`EmbeddedEmitters::hash_keys`]), so they re-key on their own.
pub(crate) const EMBEDDED_EMITTER_DIRECTIONS: u32 = 512;

/// Embedded directions a light needs before it is flagged: 8 of
/// [`EMBEDDED_EMITTER_DIRECTIONS`], a 1/64 (about 1.6%) share of the sphere,
/// half of one shadow sample at the default 32 (`DEFAULT_AREA_SAMPLE_COUNT`).
/// A thinner sliver leaves the light unflagged and unwarned: its texels keep
/// the probe early-out, which stores open texels about half a sample bright
/// at most, under the bake's own sampling step. The share is fixed rather
/// than derived from the bake's sample count, so the flag stays a function of
/// the light and its keyed geometry alone, and the lightmap layer and
/// weight-map keys need not hold it.
///
/// Changing this changes which lights are flagged, which those keys do not
/// hold: bump `LAYER_FORMAT_VERSION` and the weight-map `STAGE_VERSION`. The
/// cached probe-point stages key the flagged set and re-key on their own.
pub(crate) const EMBEDDED_EMITTER_MIN_DIRECTIONS: u32 = 8;

/// [`EMBEDDED_EMITTER_MIN_DIRECTIONS`] as a fraction: a light is flagged
/// exactly when [`embedded_emitter_fraction`] reaches it. Both are counts
/// over a power of two, so the comparison is exact.
pub(crate) const EMBEDDED_EMITTER_MIN_FRACTION: f32 =
    EMBEDDED_EMITTER_MIN_DIRECTIONS as f32 / EMBEDDED_EMITTER_DIRECTIONS as f32;

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
/// escapes them all and the probe early-out would store 1.0. A light this
/// returns at least [`EMBEDDED_EMITTER_MIN_FRACTION`] for is flagged: its
/// texels skip the all-clear early-out ([`super::TexelSampler::new`]), and
/// the probe-point bakes trace its full set at every probe
/// ([`super::ProbeEarlyOut::for_probe_point`]).
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
#[cfg(test)]
pub(crate) fn embedded_emitter_fraction(
    light: &MapLight,
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
) -> f32 {
    if !has_emitter_sphere(light) {
        return 0.0;
    }
    sphere_fraction(light, bvh, primitives, geometry)
}

/// [`embedded_emitter_fraction`] for a light already known to have a
/// sphere to trace.
fn sphere_fraction(
    light: &MapLight,
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
) -> f32 {
    let embedded = (0..EMBEDDED_EMITTER_DIRECTIONS)
        .filter(|&index| direction_embedded(light, index, bvh, primitives, geometry))
        .count();
    embedded as f32 / EMBEDDED_EMITTER_DIRECTIONS as f32
}

/// Whether [`embedded_emitter_fraction`] reaches
/// [`EMBEDDED_EMITTER_MIN_FRACTION`], stopping at the
/// [`EMBEDDED_EMITTER_MIN_DIRECTIONS`]th embedded direction: for callers that
/// need only the light's flag, not the warning's fraction. The stage flags
/// lights through [`EmbeddedEmitters`]; only test bakes call this directly.
#[cfg(test)]
pub(crate) fn has_embedded_emitter(
    light: &MapLight,
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
) -> bool {
    has_emitter_sphere(light)
        && (0..EMBEDDED_EMITTER_DIRECTIONS)
            .filter(|&index| direction_embedded(light, index, bvh, primitives, geometry))
            .take(EMBEDDED_EMITTER_MIN_DIRECTIONS as usize)
            .count()
            == EMBEDDED_EMITTER_MIN_DIRECTIONS as usize
}

/// Whether a light with this [`embedded_emitter_fraction`] is flagged.
fn flags(fraction: f32) -> bool {
    fraction >= EMBEDDED_EMITTER_MIN_FRACTION
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
    has_bounce_emitter_sphere(light) && light.shadow_type != ShadowType::Sdf
}

/// Whether the bounce family's emitter test has a sphere to trace: a point
/// or spot light of finite, non-zero size, `sdf`-shadowed included, since
/// every static light bounces.
fn has_bounce_emitter_sphere(light: &MapLight) -> bool {
    matches!(light.light_type, LightType::Point | LightType::Spot)
        && light.light_size > 0.0
        && light.light_size.is_finite()
}

/// The lights of one bake whose emitter sphere reaches into geometry by at
/// least [`EMBEDDED_EMITTER_MIN_FRACTION`], with the fraction embedded, keyed
/// by the caller's light index (a source index or a position in the stage's
/// light list).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct EmbeddedEmitters {
    /// `(key, fraction)` for every flagged light, ascending by key.
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
        Self::classify_where(
            lights,
            has_emitter_sphere,
            bvh,
            primitives,
            geometry,
            control,
        )
    }

    /// [`Self::classify`] for the bounce family (the surface radiance
    /// cache's patch direct and its parity reference): every point or spot
    /// light with an emitter sphere is tested, `sdf`-shadowed included.
    pub(crate) fn classify_bounce<'l>(
        lights: impl IntoIterator<Item = (usize, &'l MapLight)>,
        bvh: &Bvh<f32, 3>,
        primitives: &[BvhPrimitive],
        geometry: &GeometryResult,
        control: &BakeControl,
    ) -> Self {
        Self::classify_where(
            lights,
            has_bounce_emitter_sphere,
            bvh,
            primitives,
            geometry,
            control,
        )
    }

    fn classify_where<'l>(
        lights: impl IntoIterator<Item = (usize, &'l MapLight)>,
        has_sphere: fn(&MapLight) -> bool,
        bvh: &Bvh<f32, 3>,
        primitives: &[BvhPrimitive],
        geometry: &GeometryResult,
        control: &BakeControl,
    ) -> Self {
        let lights: Vec<(usize, &MapLight)> = lights
            .into_iter()
            .filter(|&(_, light)| has_sphere(light))
            .collect();
        // Strictly: a duplicate key would leave `fraction`'s binary search
        // free to find either entry.
        debug_assert!(lights.windows(2).all(|pair| pair[0].0 < pair[1].0));
        // Indexed collect keeps key order whatever worker ran which light.
        let fractions: Vec<f32> = lights
            .par_iter()
            .map(|&(_, light)| {
                let _permit = control.governor().enter();
                sphere_fraction(light, bvh, primitives, geometry)
            })
            .collect();
        Self {
            embedded: lights
                .iter()
                .zip(fractions)
                .filter(|&(_, fraction)| flags(fraction))
                .map(|(&(key, _), fraction)| (key, fraction))
                .collect(),
        }
    }

    pub(crate) fn is_embedded(&self, key: usize) -> bool {
        flags(self.fraction(key))
    }

    /// Fold the flagged keys, ascending, into a stage cache key: for a stage
    /// whose flags are tested only with the penumbra floor on, so the keyed
    /// lights and geometry alone do not fix them.
    pub(crate) fn hash_keys(&self, hasher: &mut blake3::Hasher) {
        hasher.update(&(self.embedded.len() as u64).to_le_bytes());
        for &(key, _) in &self.embedded {
            hasher.update(&(key as u64).to_le_bytes());
        }
    }

    /// The light's embedded fraction; zero for an unflagged or unknown key,
    /// including a light embedded by less than the threshold.
    pub(crate) fn fraction(&self, key: usize) -> f32 {
        self.embedded
            .binary_search_by_key(&key, |&(key, _)| key)
            .map_or(0.0, |at| self.embedded[at].1)
    }

    /// The same flags under new keys: `key` maps each old key to its new
    /// one, e.g. a position in a stage's light list to the light's source
    /// index. Distinct old keys must map to distinct new ones.
    pub(crate) fn rekeyed(&self, key: impl Fn(usize) -> usize) -> Self {
        let mut embedded: Vec<(usize, f32)> = self
            .embedded
            .iter()
            .map(|&(old, fraction)| (key(old), fraction))
            .collect();
        embedded.sort_by_key(|&(key, _)| key);
        debug_assert!(embedded.windows(2).all(|pair| pair[0].0 < pair[1].0));
        Self { embedded }
    }

    /// A set of the given `(key, fraction)` flags, for tests that need a
    /// verdict no fixture geometry produces.
    #[cfg(test)]
    pub(crate) fn from_flags(flags: impl IntoIterator<Item = (usize, f32)>) -> Self {
        let mut embedded: Vec<(usize, f32)> = flags.into_iter().collect();
        embedded.sort_by_key(|&(key, _)| key);
        Self { embedded }
    }

    /// Warn once per light either set flags, in key order, naming it by
    /// `label` (`light_source_labels`, which give entity index, classname and
    /// authored origin) and `kind`, as the texel-step warnings do. `texels` is
    /// a texel stage's own test, on the geometry it bakes (cut faces
    /// included); `probes` is the probe-point bakes' test, on the pre-cut
    /// geometry; both are keyed alike. The two agree except where a face cut
    /// passes through a light's sphere, so a light either flags warns once,
    /// and the text names the bakes that trace its full set. The fraction is
    /// the texel test's when it flags the light, else the probe test's.
    ///
    /// Called on the caller thread every compile, cold or warm, by the stage
    /// that owns the light's texels, or by the pipeline when that stage has
    /// nothing to bake: both tests run whether or not a cache serves a bake,
    /// so the warning never depends on a memo.
    pub(crate) fn warn<'a, 's>(
        kind: &str,
        texels: &Self,
        probes: &Self,
        light: impl Fn(usize) -> Option<&'a MapLight>,
        label: impl Fn(usize) -> Option<&'s str>,
    ) {
        let (texels, probes) = (&texels.embedded, &probes.embedded);
        let (mut t, mut p) = (0, 0);
        while t < texels.len() || p < probes.len() {
            let next_texel = texels.get(t).map(|&(key, _)| key);
            let next_probe = probes.get(p).map(|&(key, _)| key);
            let key = next_texel
                .into_iter()
                .chain(next_probe)
                .min()
                .expect("a set has a flag left");
            let (in_texels, in_probes) = (next_texel == Some(key), next_probe == Some(key));
            let fraction = if in_texels { texels[t].1 } else { probes[p].1 };
            t += usize::from(in_texels);
            p += usize::from(in_probes);
            // The animated stage's texels are weight-map texels.
            let texel_kind = if kind == "animated" {
                "weight-map"
            } else {
                "lightmap"
            };
            let reach = match (in_texels, in_probes) {
                (true, true) => {
                    format!("its lit {texel_kind} texels and the SH and billboard probes it lights")
                }
                (true, false) => format!("its lit {texel_kind} texels"),
                (false, _) => "the SH and billboard probes it lights".to_owned(),
            };
            warn_one(kind, key, fraction, &reach, &light, &label);
        }
    }

    /// Warn once per light `bounce` flags and `probes` does not, in key
    /// order, naming the SH bounce bake: the bounce family tests
    /// `sdf`-shadowed lights, which no texel or probe-point stage tests, so
    /// no other warning names them. A light both flag keeps the probe-point
    /// line [`Self::warn`] gives it. Both sets are keyed alike; `light` and
    /// `label` resolve those keys, as for [`Self::warn`]. Called on the
    /// caller thread every compile, cold or warm.
    pub(crate) fn warn_bounce_only<'a, 's>(
        kind: &str,
        bounce: &Self,
        probes: &Self,
        light: impl Fn(usize) -> Option<&'a MapLight>,
        label: impl Fn(usize) -> Option<&'s str>,
    ) {
        for &(key, fraction) in &bounce.embedded {
            if probes.is_embedded(key) {
                continue;
            }
            warn_one(
                kind,
                key,
                fraction,
                "the SH bounce patches it lights",
                &light,
                &label,
            );
        }
    }
}

/// One emitter-inside-geometry warning, naming the bakes (`reach`) that
/// trace the light's full set.
fn warn_one<'a, 's>(
    kind: &str,
    key: usize,
    fraction: f32,
    reach: &str,
    light: &impl Fn(usize) -> Option<&'a MapLight>,
    label: &impl Fn(usize) -> Option<&'s str>,
) {
    let name = label(key).map_or_else(|| format!("#{key}"), str::to_owned);
    let size = light(key).map_or(0.0, |light| light.light_size);
    log::warn!(
        "[Lightmap] emitter inside geometry: {kind} light {name}: {:.1}% of its \
         {size:.3} m emitter sphere lies inside or behind a surface, which dims the \
         light by about that much and makes {reach} trace every shadow sample. Move the \
         light off the surface or shrink `_light_size`.",
        fraction * 100.0,
    );
}

#[cfg(test)]
mod tests;
