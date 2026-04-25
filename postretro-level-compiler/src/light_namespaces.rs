// Typed light-namespace envelopes.
//
// Each bake stage operates over a different subset of `MapLight`. Pre-filtering
// once into typed envelopes — and naming the resulting index spaces — replaces
// parallel filter predicates spread across the bake stages. Per-stage code
// takes the typed slice it needs and applies no further filter.
//
// Index-space contracts:
// - `AlphaLightsNs` matches the on-disk `AlphaLightRecord` order (`!bake_only`).
//   `LightInfluence` records and `ChunkLightList` light_indices share this slot
//   space — record `i` here is record `i` everywhere downstream.
// - `AnimatedBakedLights` matches the `AnimationDescriptor` array order
//   (`!is_dynamic && animation.is_some()`).
// - `StaticBakedLights` is internal to lightmap and SH base bakes; no on-disk
//   slot space.

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

/// Lights consumed by the static lightmap bake and the SH base bake.
///
/// Filter: `!is_dynamic && animation.is_none()`. Iteration order matches the
/// original `&[MapLight]`.
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

/// Lights consumed by the animated-light-chunks builder, the animated weight
/// map baker, and the SH bake's animation-descriptor list.
///
/// Filter: `!is_dynamic && animation.is_some()`. Iteration order matches the
/// original `&[MapLight]`. Indices in this namespace are the same indices the
/// runtime `AnimationDescriptor` buffer uses.
///
/// `bake_only` animated lights are retained — they participate in weight-map
/// compose at runtime.
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

/// Lights consumed by the AlphaLights pack, the LightInfluence pack, the
/// LightTags pack, and the chunk light list bake.
///
/// Filter: `!bake_only`. Iteration order matches the original `&[MapLight]`.
/// Indices in this namespace are the same slot indices as the on-disk
/// `AlphaLightRecord` array — record `i` in AlphaLights, LightInfluence, and
/// LightTags all line up.
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
