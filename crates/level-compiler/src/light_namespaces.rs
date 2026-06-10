// Typed light-namespace envelopes.
//
// Pre-filtering `MapLight` once into named typed envelopes replaces per-stage
// filter predicates. Each stage takes the slice it needs; no further filtering.
//
// Index-space contracts:
// - `AlphaLightsNs`: on-disk `AlphaLightRecord` order (`!bake_only`).
//   `LightInfluence` records share this slot space — record `i` aligns across both.
//   NOTE: `ChunkLightList` light_indices do NOT share this space; emitted in the
//   compacted `!is_dynamic` spec_lights space (`pack_spec_lights`), dynamic lights
//   dropped with no placeholder. See `chunk_light_list_bake.rs`.
// - `AnimatedBakedLights`: `AnimationDescriptor` array order
//   (`!is_dynamic && animation.is_some()`).
// - `StaticBakedLights`: internal to lightmap and SH base bakes; no on-disk slot.

use postretro_level_format::light_influence::InfluenceRecord;

use crate::map_data::{LightType, MapLight};

/// One slot in the static-baked namespace. `source_index` is the position of
/// the light in the original `&[MapLight]` array.
#[derive(Debug, Clone, Copy)]
pub struct StaticBakedEntry<'a> {
    pub source_index: usize,
    pub light: &'a MapLight,
}

/// One slot in the animated-baked namespace. `source_index` is the position of
/// the light in the original `&[MapLight]` array; `influence` is the
/// pre-derived influence record parallel to this slot.
#[derive(Debug, Clone)]
pub struct AnimatedBakedEntry<'a> {
    pub source_index: usize,
    pub light: &'a MapLight,
    pub influence: InfluenceRecord,
}

/// One slot in the AlphaLights namespace. `source_index` is the position of
/// the light in the original `&[MapLight]` array.
#[derive(Debug, Clone, Copy)]
pub struct AlphaLightEntry<'a> {
    pub source_index: usize,
    pub light: &'a MapLight,
}

/// Lights for the static lightmap bake and SH base bake.
///
/// Filter: `!is_dynamic && animation.is_none()` — position axis only, never
/// shadow type. SH needs every baked-tier light (both shadow types) for bounce;
/// filtering on shadow type here would starve it. The `sdf` exclusion lives at
/// the direct lightmap consumer, keeping `lm_irr` disjoint from the runtime SDF
/// set while SH still sees all baked-tier lights. Iteration order: original `&[MapLight]`.
#[derive(Debug, Clone)]
pub struct StaticBakedLights<'a> {
    entries: Vec<StaticBakedEntry<'a>>,
}

impl<'a> StaticBakedLights<'a> {
    pub fn from_lights(lights: &'a [MapLight]) -> Self {
        let entries = lights
            .iter()
            .enumerate()
            .filter(|(_, l)| !l.is_dynamic && l.animation.is_none())
            .map(|(i, l)| StaticBakedEntry {
                source_index: i,
                light: l,
            })
            .collect();
        Self { entries }
    }

    pub fn entries(&self) -> &[StaticBakedEntry<'a>] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Lights for the animated weight-map bake, animation descriptors, and SH delta bake.
///
/// Filter: `!is_dynamic && animation.is_some()` — position axis only, never
/// shadow type. The delta bake needs every animated baked-tier light for bounce;
/// filtering on shadow type here would starve it. The `sdf` exclusion lives at
/// the direct weight-map consumer (`lm_anim` stays disjoint from the runtime SDF
/// set; delta bake still sees all animated baked-tier lights). `bake_only`
/// animated lights are retained — they participate in weight-map compose at
/// runtime. Indices match the runtime `AnimationDescriptor` buffer and
/// `AnimatedLightChunks` light_indices. Iteration order: original `&[MapLight]`.
#[derive(Debug, Clone)]
pub struct AnimatedBakedLights<'a> {
    entries: Vec<AnimatedBakedEntry<'a>>,
}

impl<'a> AnimatedBakedLights<'a> {
    pub fn from_lights(lights: &'a [MapLight]) -> Self {
        let entries = lights
            .iter()
            .enumerate()
            .filter(|(_, l)| !l.is_dynamic && l.animation.is_some())
            .map(|(i, l)| AnimatedBakedEntry {
                source_index: i,
                light: l,
                influence: derive_influence(l),
            })
            .collect();
        Self { entries }
    }

    pub fn entries(&self) -> &[AnimatedBakedEntry<'a>] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Construct from parallel light + influence slices in this namespace's
    /// iteration order. The caller is responsible for the input being already
    /// filtered to `!is_dynamic && animation.is_some()` — used by tests that
    /// inject custom influence records that diverge from `derive_influence`.
    #[cfg(test)]
    pub fn from_parallel_slices(lights: &'a [MapLight], influence: &[InfluenceRecord]) -> Self {
        debug_assert_eq!(lights.len(), influence.len());
        let entries = lights
            .iter()
            .zip(influence.iter())
            .enumerate()
            .map(|(i, (l, infl))| AnimatedBakedEntry {
                source_index: i,
                light: l,
                influence: infl.clone(),
            })
            .collect();
        Self { entries }
    }

    /// Materialize parallel `(Vec<MapLight>, Vec<InfluenceRecord>)` slices in
    /// this namespace's iteration order. Used by stages that need owned light
    /// records (e.g. the animated weight-map baker, which holds its inputs
    /// across multi-stage pipelines).
    pub fn to_parallel_vecs(&self) -> (Vec<MapLight>, Vec<InfluenceRecord>) {
        let lights = self.entries.iter().map(|e| e.light.clone()).collect();
        let influence = self.entries.iter().map(|e| e.influence.clone()).collect();
        (lights, influence)
    }
}

/// Lights for the AlphaLights pack, LightInfluence pack, LightTags pack, and
/// chunk light list bake.
///
/// Filter: `!bake_only`. Indices match on-disk `AlphaLightRecord` slot space —
/// AlphaLights, LightInfluence, and LightTags records all align. Iteration
/// order: original `&[MapLight]`.
#[derive(Debug, Clone)]
pub struct AlphaLightsNs<'a> {
    entries: Vec<AlphaLightEntry<'a>>,
}

impl<'a> AlphaLightsNs<'a> {
    pub fn from_lights(lights: &'a [MapLight]) -> Self {
        let entries = lights
            .iter()
            .enumerate()
            .filter(|(_, l)| !l.bake_only)
            .map(|(i, l)| AlphaLightEntry {
                source_index: i,
                light: l,
            })
            .collect();
        Self { entries }
    }

    pub fn entries(&self) -> &[AlphaLightEntry<'a>] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

fn derive_influence(light: &MapLight) -> InfluenceRecord {
    match light.light_type {
        LightType::Directional => InfluenceRecord {
            center: [0.0, 0.0, 0.0],
            radius: f32::MAX,
        },
        LightType::Point | LightType::Spot => InfluenceRecord {
            center: [
                light.origin.x as f32,
                light.origin.y as f32,
                light.origin.z as f32,
            ],
            radius: light.falloff_range,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_data::{FalloffModel, LightAnimation, ShadowType};
    use glam::DVec3;

    /// A baked-tier point light with the requested shadow type / animation /
    /// tier. `is_dynamic` is the position axis (set by classname in real
    /// translator output); shadow type is orthogonal.
    fn light(shadow_type: ShadowType, animated: bool, is_dynamic: bool) -> MapLight {
        MapLight {
            origin: DVec3::ZERO,
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 10.0,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: animated.then(|| LightAnimation {
                period: 1.0,
                phase: 0.0,
                brightness: Some(vec![1.0, 0.5]),
                color: None,
                direction: None,
                start_active: true,
            }),
            bake_only: false,
            is_dynamic,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![],
            shadow_type,
        }
    }

    fn source_indices<T>(entries: &[T], get: impl Fn(&T) -> usize) -> Vec<usize> {
        entries.iter().map(get).collect()
    }

    /// Indirect-not-starved contract: the namespace keys on the **position**
    /// axis (`!is_dynamic`), never on shadow type — so `sdf`-typed baked-tier
    /// lights REMAIN in the static/SH-base set. Only dynamic-tier lights drop.
    /// (The shadow-type exclusion lives down at the direct lightmap bake, not
    /// here — guarded by the bake-level disjoint-direct test.)
    #[test]
    fn static_baked_set_keys_on_position_not_shadow_type() {
        let lights = vec![
            light(ShadowType::StaticLightMap, false, false), // 0: included
            light(ShadowType::Sdf, false, false),            // 1: included (SH needs its bounce)
            light(ShadowType::StaticLightMap, false, true),  // 2: excluded (dynamic tier)
        ];
        let ns = StaticBakedLights::from_lights(&lights);
        let idx = source_indices(ns.entries(), |e| e.source_index);
        assert_eq!(idx, vec![0, 1]);
    }

    /// The animated namespace likewise keys on position: an `sdf`-typed
    /// animated baked-tier light REMAINS (the delta-SH bake needs its bounce);
    /// only a dynamic-tier animated light drops.
    #[test]
    fn animated_baked_set_keys_on_position_not_shadow_type() {
        let lights = vec![
            light(ShadowType::StaticLightMap, true, false), // 0: included
            light(ShadowType::Sdf, true, false),            // 1: included (SH delta needs bounce)
            light(ShadowType::StaticLightMap, true, true),  // 2: excluded (dynamic tier)
            light(ShadowType::StaticLightMap, false, false), // 3: not animated → not here
        ];
        let ns = AnimatedBakedLights::from_lights(&lights);
        let idx = source_indices(ns.entries(), |e| e.source_index);
        assert_eq!(idx, vec![0, 1]);
    }
}
