// Scripting ↔ renderer bridge for fog volumes: entity registry → GPU fog buffers.
// See: context/lib/rendering_pipeline.md §7.5

use std::collections::HashMap;

use glam::{Quat, Vec3};

use crate::fx::fog_volume::{FogPointLight, FogVolume, MAX_FOG_POINT_LIGHTS};
use crate::prl::{LightType, MapLight};
use crate::scripting::components::fog_volume::FogAnimation;
use crate::scripting::registry::{
    ComponentKind, EntityId, EntityRegistry, FogVolumeComponent, Transform,
};
use postretro_level_format::fog_volumes::FogVolumeRecord;

/// Authoring-time AABB and shape parameters cached alongside the entity. Not
/// runtime-settable — these are baked at compile time and live in a side-table
/// rather than the `FogVolumeComponent`.
///
/// `center`, `inv_half_ext`, `half_diag`, `shape_mode`, `anisotropy`, and
/// `ambient_scatter` are baked into the PRL by the level compiler; they're
/// cached here so per-frame fog uploads can copy precomputed values without
/// recomputing them from min/max. `shape_mode` is a discriminant flag (0.0 =
/// legacy radial sphere/capsule fade against `half_diag`, 1.0 = ellipsoid
/// using `inv_half_ext`); the shader compares with `> 0.5` to avoid float
/// precision issues.
///
/// Note: `radial_falloff` lives on `FogVolumeComponent.falloff` (runtime-
/// settable) and is read from the component at GPU pack time.
///
/// `anisotropy` and `ambient_scatter` differ from `radial_falloff`: the
/// author-facing `scatter_bias` KVP has no runtime component equivalent — the
/// compiler's HG translation is the only way to change it.
pub struct FogVolumeAabb {
    pub min: Vec3,
    pub max: Vec3,
    pub center: Vec3,
    pub inv_half_ext: Vec3,
    pub half_diag: f32,
    pub shape_mode: f32,
    pub anisotropy: f32,
    pub ambient_scatter: f32,
}

/// Per-entity bookkeeping for the per-frame fog animation evaluator. Stored
/// in a side-table because animation start time is engine state — it must not
/// round-trip through the script-visible `FogVolumeComponent` where each
/// `setFogAnimation` would clobber the moment a curve was installed.
struct FogAnimSlot {
    animation_start_time_ms: f64,
    cached_animation: FogAnimation,
}

/// State carried across frames. Owned by the game layer so the renderer never
/// holds component data.
pub(crate) struct FogVolumeBridge {
    /// Compile-time AABB and shape parameters per entity, keyed by `EntityId`.
    aabbs: HashMap<EntityId, FogVolumeAabb>,
    /// Map-volume index → `EntityId`. Fixed at level load.
    entity_ids: Vec<EntityId>,
    /// Reusable byte buffer for `FogVolume` records. Capacity retained between
    /// frames; cleared at the start of each `update_volumes` call.
    volumes_bytes: Vec<u8>,
    /// Per-canonical-slot bounding planes. Indexed parallel to `entity_ids` so
    /// the renderer can look up planes when dense-packing the active set. Cloned
    /// from the side-table at level load — keeps the per-frame `update_volumes`
    /// path allocation-free since the renderer reads by reference.
    canonical_planes: Vec<Vec<[f32; 4]>>,
    /// Reusable byte buffer for `FogPointLight` records.
    points_bytes: Vec<u8>,
    /// Per-frame cache of (min, max) AABBs for volumes that are currently active
    /// (component present, density > 0). Populated by `update_volumes`; cleared
    /// by both `update_volumes` (frame reset) and `clear` (level reset).
    /// Consumed by the renderer's spot-light pre-cull.
    active_aabbs: Vec<(Vec3, Vec3)>,
    /// Per-entity start times for `FogVolumeComponent.animation`. Populated
    /// lazily by `tick` the first frame an animation is observed; cleared
    /// when an animation goes away (script clears it, or `play_count`
    /// settles). Start time resets whenever any field of `FogAnimation`
    /// changes (detected by full-struct equality in `tick`) — including
    /// `period_ms` changes with an otherwise-identical curve.
    anim_slots: HashMap<EntityId, FogAnimSlot>,
    /// Set to `true` the first time the point-light cap warning fires; suppresses
    /// per-frame log spam when the scene consistently exceeds `MAX_FOG_POINT_LIGHTS`.
    warned_overflow: bool,
}

impl FogVolumeBridge {
    pub(crate) fn new() -> Self {
        Self {
            aabbs: HashMap::new(),
            entity_ids: Vec::new(),
            volumes_bytes: Vec::new(),
            canonical_planes: Vec::new(),
            points_bytes: Vec::new(),
            active_aabbs: Vec::new(),
            anim_slots: HashMap::new(),
            warned_overflow: false,
        }
    }

    /// Populate the entity registry with one entity per fog-volume record.
    /// Called once at level load. Stores the AABB and shape parameters in the
    /// side-table; the four runtime-settable parameters (`density`, `glow`,
    /// `edge_softness`, `falloff`) become a `FogVolumeComponent` on the spawned
    /// entity.
    pub(crate) fn populate_from_level(
        &mut self,
        registry: &mut EntityRegistry,
        records: &[FogVolumeRecord],
    ) {
        self.aabbs.clear();
        self.entity_ids.clear();
        self.canonical_planes.clear();
        self.entity_ids.reserve(records.len());
        self.canonical_planes.reserve(records.len());

        for entry in records {
            let center = Vec3::from(entry.center);
            let transform = Transform {
                position: center,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            };
            let Some(id) = registry.try_spawn(transform, &entry.tags) else {
                log::warn!(
                    "[FogVolumeBridge] entity registry exhausted; dropping fog volume \
                     (index {}). Further fog volumes in this level will not appear.",
                    self.entity_ids.len()
                );
                break;
            };
            let component = FogVolumeComponent {
                density: entry.density,
                glow: entry.glow,
                edge_softness: entry.edge_softness,
                falloff: entry.radial_falloff,
                tint: entry.tint,
                saturation: entry.saturation,
                min_brightness: entry.min_brightness,
                light_range: entry.light_range,
                animation: None,
            };
            // `set_component` only fails on stale id — the id was just returned.
            let _ = registry.set_component(id, component);

            self.aabbs.insert(
                id,
                FogVolumeAabb {
                    min: Vec3::from(entry.min),
                    max: Vec3::from(entry.max),
                    center: Vec3::from(entry.center),
                    inv_half_ext: Vec3::from(entry.inv_half_ext),
                    half_diag: entry.half_diag,
                    shape_mode: entry.shape_mode,
                    anisotropy: entry.anisotropy,
                    ambient_scatter: entry.ambient_scatter,
                },
            );
            self.canonical_planes.push(entry.planes.clone());
            self.entity_ids.push(id);
        }
    }

    /// Drop per-level state. Byte-buffer capacity is retained so the next
    /// level reuses the allocation.
    pub(crate) fn clear(&mut self) {
        self.aabbs.clear();
        self.entity_ids.clear();
        // Length cleared, capacity retained.
        self.volumes_bytes.clear();
        self.canonical_planes.clear();
        self.points_bytes.clear();
        self.active_aabbs.clear();
        self.anim_slots.clear();
        self.warned_overflow = false;
    }

    /// Evaluate every fog volume's `animation` curve for the current frame
    /// and write the sampled value back into `FogVolumeComponent.density`.
    /// Called once per frame, immediately before `update_volumes`, so the GPU
    /// pack picks up the freshly evaluated density unchanged.
    ///
    /// `time_seconds` is engine wall-clock time; sampling is done in
    /// milliseconds because `FogAnimation.period_ms` is millisecond-keyed.
    ///
    /// Taken as `f64` so `(now_ms - start_ms)` retains sub-second precision
    /// after long uptimes — at ~30 minutes an `f32` ms count loses enough
    /// mantissa bits that density steps become visible. Narrowing to `f32`
    /// happens at the curve-sample leaf, after the difference is computed.
    pub(crate) fn tick(&mut self, registry: &mut EntityRegistry, time_seconds: f64) {
        let now_ms = time_seconds * 1000.0;
        let mut updates: Vec<(EntityId, FogVolumeComponent)> = Vec::new();
        let mut clear_slots: Vec<EntityId> = Vec::new();
        for (id, value) in registry.iter_with_kind(ComponentKind::FogVolume) {
            let crate::scripting::registry::ComponentValue::FogVolume(component) = value else {
                continue;
            };
            let Some(animation) = component.animation.as_ref() else {
                if self.anim_slots.contains_key(&id) {
                    clear_slots.push(id);
                }
                continue;
            };
            // Defensive: validate() rejects period_ms <= 0 at install time;
            // this guard keeps the evaluator safe if that invariant breaks.
            if animation.period_ms <= 0.0 {
                continue;
            }

            let slot = self.anim_slots.entry(id).or_insert_with(|| FogAnimSlot {
                animation_start_time_ms: now_ms,
                cached_animation: animation.clone(),
            });
            if &slot.cached_animation != animation {
                slot.animation_start_time_ms = now_ms;
                slot.cached_animation = animation.clone();
            }
            let start_ms = slot.animation_start_time_ms;

            if let Some(settled) = settle_play_count(animation, start_ms, now_ms) {
                let mut next = component.clone();
                if let Some(d) = settled.density {
                    next.density = d;
                }
                if let Some(s) = settled.saturation {
                    next.saturation = s;
                }
                if let Some(m) = settled.min_brightness {
                    next.min_brightness = m;
                }
                if let Some(l) = settled.light_range {
                    next.light_range = l;
                }
                next.animation = None;
                updates.push((id, next));
                clear_slots.push(id);
                continue;
            }

            let sampled_density = sample_density_curve_at(animation, start_ms, now_ms);
            let sampled_saturation = sample_saturation_curve_at(animation, start_ms, now_ms);
            let sampled_min_brightness =
                sample_min_brightness_curve_at(animation, start_ms, now_ms);
            let sampled_light_range = sample_light_range_curve_at(animation, start_ms, now_ms);
            if sampled_density.is_none()
                && sampled_saturation.is_none()
                && sampled_min_brightness.is_none()
                && sampled_light_range.is_none()
            {
                continue;
            }
            let mut next = component.clone();
            if let Some(d) = sampled_density {
                next.density = d;
            }
            if let Some(s) = sampled_saturation {
                next.saturation = s;
            }
            if let Some(m) = sampled_min_brightness {
                next.min_brightness = m;
            }
            if let Some(l) = sampled_light_range {
                next.light_range = l;
            }
            updates.push((id, next));
        }

        for id in clear_slots {
            self.anim_slots.remove(&id);
        }
        for (id, component) in updates {
            // `set_component` only fails on a stale id — the id was just
            // yielded by `iter_with_kind`, so any failure here is unreachable.
            let _ = registry.set_component(id, component);
        }
    }

    /// Walk the registry and repack one `FogVolume` GPU record per tracked
    /// entity, in original PRL record order. Returns `None` when `entity_ids`
    /// is empty — i.e. no fog volumes were registered for this level. Returns
    /// `Some` whenever at least one volume was registered, even if all volumes
    /// currently have density ≤ 0; the `Some` case preserves the canonical
    /// index layout that `FogCellMasks` bit indices rely on. The renderer uses
    /// `None` to skip the entire fog-volume upload path.
    ///
    /// The returned byte slice has length `entity_ids.len() * FOG_VOLUME_SIZE`
    /// — every slot is emitted, in source order, even when the underlying
    /// entity has no `FogVolumeComponent` or has density ≤ 0. This preserves
    /// the canonical index that `FogCellMasks` bits refer to.
    ///
    /// The returned `live_mask` has bit `i` set iff slot `i` has a present
    /// component with density > 0. The renderer ANDs this into the
    /// portal-cull mask so density-zero slots never reach the GPU.
    #[allow(clippy::type_complexity)]
    pub(crate) fn update_volumes(
        &mut self,
        registry: &EntityRegistry,
    ) -> Option<(&[u8], &[Vec<[f32; 4]>], u32)> {
        self.volumes_bytes.clear();
        self.active_aabbs.clear();
        if self.entity_ids.is_empty() {
            return None;
        }

        let mut live_mask = 0u32;
        for (i, id) in self.entity_ids.iter().enumerate() {
            let component = registry.get_component::<FogVolumeComponent>(*id).ok();
            let aabb = self.aabbs.get(id);

            // Slot `i` is live iff the component is present with density > 0.
            // Zero-density and missing-component slots emit a zero-density
            // placeholder so the buffer's index layout matches the PRL record
            // order — the renderer drops these via the live_mask AND.
            let live = component.is_some_and(|c| c.density > 0.0) && aabb.is_some();
            if live {
                live_mask |= 1u32 << i;
                if let Some(a) = aabb {
                    self.active_aabbs.push((a.min, a.max));
                }
            }

            // `plane_count` is the canonical-slot's plane count. `plane_offset`
            // is left at zero here — the renderer's dense repack patches it
            // when the active set is packed into the GPU buffer (offsets are
            // dense-order, not canonical-order).
            let plane_count = self
                .canonical_planes
                .get(i)
                .map(|p| p.len() as u32)
                .unwrap_or(0);
            let fv = match (component, aabb) {
                (Some(component), Some(aabb)) => FogVolume {
                    min: aabb.min.to_array(),
                    density: component.density,
                    max_v: aabb.max.to_array(),
                    edge_softness: component.edge_softness,
                    center: aabb.center.to_array(),
                    half_diag: aabb.half_diag,
                    inv_half_ext: aabb.inv_half_ext.to_array(),
                    shape_mode: aabb.shape_mode,
                    tint: component.tint,
                    saturation: component.saturation,
                    radial_falloff: component.falloff,
                    glow: component.glow,
                    plane_offset: 0,
                    plane_count,
                    min_brightness: component.min_brightness,
                    light_range: component.light_range,
                    anisotropy: aabb.anisotropy,
                    ambient_scatter: aabb.ambient_scatter,
                },
                _ => FogVolume {
                    min: [0.0; 3],
                    density: 0.0,
                    max_v: [0.0; 3],
                    edge_softness: 0.0,
                    center: [0.0; 3],
                    half_diag: 0.0,
                    inv_half_ext: [0.0; 3],
                    shape_mode: 0.0,
                    tint: [1.0, 1.0, 1.0],
                    saturation: 1.0,
                    radial_falloff: 0.0,
                    glow: 0.0,
                    plane_offset: 0,
                    plane_count: 0,
                    min_brightness: 0.0,
                    light_range: 1.0,
                    anisotropy: 0.0,
                    ambient_scatter: 1.0,
                },
            };
            self.volumes_bytes
                .extend_from_slice(bytemuck::bytes_of(&fv));
        }

        Some((&self.volumes_bytes, &self.canonical_planes, live_mask))
    }

    /// Pre-cull a slice of map lights against the cached fog-volume AABBs and
    /// pack the survivors as `FogPointLight` records. Filters to dynamic point
    /// lights only — static lights bake into the SH volume; spot lights have a
    /// dedicated path. Capped at `MAX_FOG_POINT_LIGHTS`.
    ///
    /// Each entry pairs a `MapLight` with its effective brightness multiplier
    /// at the current frame. Pairing (rather than parallel slices) prevents
    /// index drift when a `LightComponent` lookup fails — see
    /// `LightBridge::collect_all_as_map_lights`.
    pub(crate) fn update_points(&mut self, lights: &[(MapLight, f32)]) -> &[u8] {
        self.points_bytes.clear();
        if self.aabbs.is_empty() || lights.is_empty() {
            return &self.points_bytes;
        }

        // Suppress fully-dark lights (matches the renderer's shadow-slot
        // suppression threshold in `update_dynamic_light_slots`).
        const BRIGHTNESS_SUPPRESSION_THRESHOLD: f32 = 0.01;

        let mut total_candidates = 0usize;
        for (light, multiplier) in lights.iter() {
            if !matches!(light.light_type, LightType::Point) || !light.is_dynamic {
                continue;
            }
            let multiplier = *multiplier;
            if multiplier < BRIGHTNESS_SUPPRESSION_THRESHOLD {
                continue;
            }
            let center = Vec3::new(
                light.origin[0] as f32,
                light.origin[1] as f32,
                light.origin[2] as f32,
            );
            let range = light.falloff_range;
            if !sphere_intersects_any_aabb(center, range, self.aabbs.values()) {
                continue;
            }
            total_candidates += 1;
            let packed_count = self.points_bytes.len() / std::mem::size_of::<FogPointLight>();
            if packed_count >= MAX_FOG_POINT_LIGHTS {
                // Keep counting so we can log the total below.
                continue;
            }
            let intensity = light.intensity * multiplier;
            let record = FogPointLight {
                position: center.to_array(),
                range,
                color: [
                    light.color[0] * intensity,
                    light.color[1] * intensity,
                    light.color[2] * intensity,
                ],
                _pad: 0.0,
            };
            self.points_bytes
                .extend_from_slice(bytemuck::bytes_of(&record));
        }

        let uploaded = self.points_bytes.len() / std::mem::size_of::<FogPointLight>();
        if total_candidates > MAX_FOG_POINT_LIGHTS && !self.warned_overflow {
            self.warned_overflow = true;
            log::warn!(
                "[FogVolumeBridge] fog point lights: {} uploaded, {} total (capped at {})",
                uploaded,
                total_candidates,
                MAX_FOG_POINT_LIGHTS,
            );
        }
        &self.points_bytes
    }

    /// (min, max) AABBs of fog volumes that are currently active (component
    /// present, density > 0). Refreshed each call to `update_volumes`. Empty
    /// when no volumes are active. Consumed by the renderer to pre-cull
    /// dynamic spot lights against the active fog set before they reach the
    /// raymarch's per-step inner loop.
    pub(crate) fn active_aabbs(&self) -> &[(Vec3, Vec3)] {
        &self.active_aabbs
    }
}

impl Default for FogVolumeBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Sphere ↔ AABB intersection: the closest point on the box to the sphere
/// center is within `radius`. Cheap pre-cull so the fog raymarch never iterates
/// lights whose influence sphere does not overlap a fog volume.
fn sphere_intersects_any_aabb<'a>(
    center: Vec3,
    radius: f32,
    aabbs: impl IntoIterator<Item = &'a FogVolumeAabb>,
) -> bool {
    let r2 = radius * radius;
    for aabb in aabbs {
        let clamped = center.clamp(aabb.min, aabb.max);
        let d = center - clamped;
        if d.length_squared() <= r2 {
            return true;
        }
    }
    false
}

/// Sample `animation.density` at the current wall-clock time. Returns `None`
/// when the animation carries no density curve — the component's static
/// density is left untouched in that case.
fn sample_density_curve_at(animation: &FogAnimation, start_ms: f64, now_ms: f64) -> Option<f32> {
    let curve = animation.density.as_ref()?;
    let t = normalized_phase(animation, start_ms, now_ms);
    Some(sample_density_curve(curve, t))
}

/// Sample `animation.saturation` at the current wall-clock time. Returns `None`
/// when the animation carries no saturation curve — the component's static
/// saturation is left untouched in that case. Shares the same phase math as
/// the density channel so both channels move in lockstep on the same timeline.
fn sample_saturation_curve_at(animation: &FogAnimation, start_ms: f64, now_ms: f64) -> Option<f32> {
    let curve = animation.saturation.as_ref()?;
    let t = normalized_phase(animation, start_ms, now_ms);
    Some(sample_density_curve(curve, t))
}

/// Sample `animation.min_brightness` at the current wall-clock time. Returns `None`
/// when the animation carries no min_brightness curve — the component's static
/// min_brightness is left untouched in that case. Shares the same phase math as
/// the density channel so both channels move in lockstep on the same timeline.
fn sample_min_brightness_curve_at(
    animation: &FogAnimation,
    start_ms: f64,
    now_ms: f64,
) -> Option<f32> {
    let curve = animation.min_brightness.as_ref()?;
    let t = normalized_phase(animation, start_ms, now_ms);
    Some(sample_density_curve(curve, t))
}

/// Sample `animation.light_range` at the current wall-clock time. Returns `None`
/// when the animation carries no light_range curve — the component's static
/// light_range is left untouched in that case. Shares the same phase math as
/// the density channel so both channels move in lockstep on the same timeline.
fn sample_light_range_curve_at(
    animation: &FogAnimation,
    start_ms: f64,
    now_ms: f64,
) -> Option<f32> {
    let curve = animation.light_range.as_ref()?;
    let t = normalized_phase(animation, start_ms, now_ms);
    Some(sample_density_curve(curve, t))
}

/// Compute the normalised `[0.0, 1.0)` phase for the current frame. Difference
/// and modulo run in `f64` so a `now_ms` past ~30 minutes still resolves
/// sub-second timing; narrowing to `f32` happens once, here, after the
/// difference is reduced into `[0.0, 1.0)`.
fn normalized_phase(animation: &FogAnimation, start_ms: f64, now_ms: f64) -> f32 {
    let period_ms = animation.period_ms as f64;
    let phase_offset = animation.phase.unwrap_or(0.0) as f64;
    let t = ((now_ms - start_ms) / period_ms + phase_offset).rem_euclid(1.0);
    t as f32
}

/// Linear interpolation across an N-sample density curve, where samples are
/// uniformly spaced over `[0.0, 1.0]`. Deliberately simpler than the shader's
/// Catmull-Rom path: fog density is a single scalar per frame and the visual
/// difference at curve boundaries is imperceptible at the cadence reactions
/// drive fog at.
fn sample_density_curve(curve: &[f32], t: f32) -> f32 {
    match curve.len() {
        // Defensive: validation rejects empty curves at install time, so this
        // arm is unreachable in practice. Return the no-fog identity rather
        // than panicking from a hot path.
        0 => 0.0,
        1 => curve[0],
        n => {
            let pos = t * (n - 1) as f32;
            let lo_idx = (pos as usize).min(n - 1);
            let hi_idx = (lo_idx + 1).min(n - 1);
            let frac = pos - lo_idx as f32;
            let lo = curve[lo_idx];
            let hi = curve[hi_idx];
            lo + (hi - lo) * frac
        }
    }
}

/// Final values written back when a `play_count`-bounded animation settles.
/// Fields are `None` when the animation has no curve for that channel —
/// the component's static value is left untouched for that channel.
struct SettledValues {
    density: Option<f32>,
    saturation: Option<f32>,
    min_brightness: Option<f32>,
    light_range: Option<f32>,
}

/// Returns `Some(SettledValues)` when `animation` is `play_count`-bounded and
/// has elapsed past its end. The caller writes the settled values back as
/// static scalars and clears `animation`. See also
/// `light_bridge::check_play_count_completion`. Unlike the light bridge,
/// `play_count == 0` is coerced to `1` at install time by
/// `set_fog_animation::validate` and never reaches here.
fn settle_play_count(
    animation: &FogAnimation,
    start_ms: f64,
    now_ms: f64,
) -> Option<SettledValues> {
    let play_count = animation.play_count?;
    debug_assert!(play_count > 0, "play_count coerced to >= 1 at install time");
    if animation.period_ms <= 0.0 {
        return None;
    }
    // Comparison in f64: at long uptimes an f32 `now_ms - start_ms` loses
    // mantissa bits faster than the period denominator, so the quotient could
    // round past `play_count` a frame early. f64 keeps the boundary precise.
    let elapsed_periods = (now_ms - start_ms) / animation.period_ms as f64;
    if elapsed_periods < play_count as f64 {
        return None;
    }
    Some(SettledValues {
        density: animation.density.as_ref().and_then(|c| c.last().copied()),
        saturation: animation
            .saturation
            .as_ref()
            .and_then(|c| c.last().copied()),
        min_brightness: animation
            .min_brightness
            .as_ref()
            .and_then(|c| c.last().copied()),
        light_range: animation
            .light_range
            .as_ref()
            .and_then(|c| c.last().copied()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prl::{FalloffModel, LightType, MapLight};

    fn sample_record() -> FogVolumeRecord {
        FogVolumeRecord {
            min: [-2.0, 0.0, -2.0],
            density: 0.5,
            max: [2.0, 3.0, 2.0],
            edge_softness: 1.0,
            glow: 0.4,
            radial_falloff: 2.0,
            tint: [1.0, 1.0, 1.0],
            saturation: 1.0,
            min_brightness: 0.0,
            light_range: 1.0,
            anisotropy: 0.0,
            ambient_scatter: 1.0,
            center: [0.0, 1.5, 0.0],
            inv_half_ext: [0.5, 1.0 / 1.5, 0.5],
            half_diag: 2.5,
            shape_mode: 0.0,
            plane_count: 0,
            planes: vec![],
            tags: vec![],
        }
    }

    fn point_light(origin: [f64; 3], range: f32, dynamic: bool) -> MapLight {
        MapLight {
            origin,
            light_type: LightType::Point,
            intensity: 2.0,
            color: [1.0, 0.5, 0.25],
            falloff_model: FalloffModel::InverseSquared,
            falloff_range: range,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0, 0.0, 0.0],
            cast_shadows: false,
            is_dynamic: dynamic,
            casts_entity_shadows: false,
            tags: vec![],
            leaf_index: 0,
        }
    }

    #[test]
    fn populate_from_level_spawns_one_entity_per_record_with_component() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        let mut record = sample_record();
        record.anisotropy = 0.35;
        record.ambient_scatter = 0.6;
        bridge.populate_from_level(&mut registry, &[record]);

        assert_eq!(bridge.entity_ids.len(), 1);
        let id = bridge.entity_ids[0];
        let comp = registry.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(comp.density, 0.5);
        assert_eq!(comp.glow, 0.4);
        assert_eq!(comp.edge_softness, 1.0);
        assert_eq!(comp.falloff, 2.0);
        let aabb = bridge.aabbs.get(&id).unwrap();
        assert_eq!(aabb.min, Vec3::new(-2.0, 0.0, -2.0));
        assert_eq!(aabb.center, Vec3::new(0.0, 1.5, 0.0));
        assert_eq!(aabb.inv_half_ext, Vec3::new(0.5, 1.0 / 1.5, 0.5));
        assert_eq!(aabb.half_diag, 2.5);
        assert_eq!(aabb.shape_mode, 0.0);
        assert_eq!(aabb.anisotropy, 0.35);
        assert_eq!(aabb.ambient_scatter, 0.6);
    }

    #[test]
    fn update_volumes_returns_none_when_no_records() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[]);
        assert!(bridge.update_volumes(&registry).is_none());
    }

    #[test]
    fn update_volumes_packs_runtime_fields_from_component() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        let mut record = sample_record();
        record.anisotropy = 0.35;
        record.ambient_scatter = 0.6;
        bridge.populate_from_level(&mut registry, &[record]);

        // Mutate via the registry the way a script would.
        let id = bridge.entity_ids[0];
        registry
            .set_component(
                id,
                FogVolumeComponent {
                    density: 1.25,
                    glow: 0.9,
                    edge_softness: 0.5,
                    falloff: 3.5,
                    tint: [1.0, 1.0, 1.0],
                    saturation: 1.0,
                    min_brightness: 0.0,
                    light_range: 1.0,
                    animation: None,
                },
            )
            .unwrap();

        let (bytes, _planes, live_mask) = bridge.update_volumes(&registry).expect("dirty volumes");
        assert_eq!(bytes.len(), std::mem::size_of::<FogVolume>());
        // FogVolume: Pod (see fx/fog_volume.rs) — read fields by name rather
        // than chasing byte offsets, which silently drift if the struct grows.
        let volume: &FogVolume = bytemuck::from_bytes(bytes);
        assert_eq!(volume.density, 1.25);
        assert_eq!(volume.edge_softness, 0.5);
        assert_eq!(volume.radial_falloff, 3.5);
        assert_eq!(volume.anisotropy, 0.35);
        assert_eq!(volume.ambient_scatter, 0.6);
        assert_eq!(
            live_mask, 0b1,
            "single non-zero-density slot should be live"
        );
    }

    #[test]
    fn update_volumes_fallback_uses_identity_directional_fog_defaults() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record()]);

        let id = bridge.entity_ids[0];
        registry
            .remove_component::<FogVolumeComponent>(id)
            .expect("component should exist");

        let (bytes, _planes, live_mask) = bridge.update_volumes(&registry).expect("one slot");
        let volume: &FogVolume = bytemuck::from_bytes(bytes);
        assert_eq!(volume.density, 0.0);
        assert_eq!(volume.anisotropy, 0.0);
        assert_eq!(volume.ambient_scatter, 1.0);
        assert_eq!(live_mask, 0);
    }

    #[test]
    fn update_volumes_emits_canonical_length_with_zero_density_placeholder() {
        // Two records — zero out the second one's density. The bridge must
        // still emit two GPU records (canonical layout) but mark only slot 0
        // as live so the renderer drops slot 1 from the dense repack.
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record(), sample_record()]);

        let id1 = bridge.entity_ids[1];
        registry
            .set_component(
                id1,
                FogVolumeComponent {
                    density: 0.0,
                    glow: 0.0,
                    edge_softness: 0.0,
                    falloff: 1.0,
                    tint: [1.0, 1.0, 1.0],
                    saturation: 1.0,
                    min_brightness: 0.0,
                    light_range: 1.0,
                    animation: None,
                },
            )
            .unwrap();

        let (bytes, _planes, live_mask) = bridge.update_volumes(&registry).expect("two slots");
        assert_eq!(bytes.len(), 2 * std::mem::size_of::<FogVolume>());
        assert_eq!(live_mask, 0b01);
    }

    #[test]
    fn update_points_pre_culls_lights_outside_every_aabb() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record()]);

        let inside = point_light([0.0, 1.0, 0.0], 5.0, true);
        let near_miss = point_light([100.0, 100.0, 100.0], 5.0, true);
        let static_light = point_light([0.0, 1.0, 0.0], 5.0, false);

        let bytes = bridge.update_points(&[(inside, 1.0), (near_miss, 1.0), (static_light, 1.0)]);
        // Only the dynamic in-range light passes both filters.
        assert_eq!(bytes.len(), std::mem::size_of::<FogPointLight>());
    }

    #[test]
    fn update_points_premultiplies_color_by_intensity() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record()]);

        let light = point_light([0.0, 1.0, 0.0], 5.0, true);
        let bytes = bridge.update_points(&[(light, 1.0)]).to_vec();
        // FogPointLight layout: position (12) | range (4) | color (12) | _pad (4).
        let r = f32::from_le_bytes(bytes[16..20].try_into().unwrap());
        // intensity 2.0 × color.r 1.0 × multiplier 1.0 = 2.0
        assert!((r - 2.0).abs() < 1e-5);
    }

    #[test]
    fn update_points_applies_effective_brightness_multiplier() {
        // Regression: scripts mutating `LightComponent.intensity` update the
        // light bridge's `effective_brightness`, not the static `MapLight`
        // slice. The fog packer must apply that multiplier so a `setComponent`
        // intensity change reaches the halo.
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record()]);

        let light = point_light([0.0, 1.0, 0.0], 5.0, true);
        // light.intensity = 2.0, color.r = 1.0, multiplier = 0.25 → 0.5
        let bytes = bridge.update_points(&[(light, 0.25)]).to_vec();
        let r = f32::from_le_bytes(bytes[16..20].try_into().unwrap());
        assert!(
            (r - 0.5).abs() < 1e-5,
            "color.r should be intensity × multiplier × color.r = 0.5; got {r}"
        );
    }

    #[test]
    fn update_points_suppresses_dark_lights() {
        // Effective brightness < 0.01 → light dropped (matches forward-pass
        // shadow-slot suppression threshold).
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record()]);

        let light = point_light([0.0, 1.0, 0.0], 5.0, true);
        let bytes = bridge.update_points(&[(light, 0.0)]);
        assert!(bytes.is_empty(), "dark light must be suppressed");
    }

    #[test]
    fn active_aabbs_reflects_only_density_positive_volumes() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record(), sample_record()]);

        let id1 = bridge.entity_ids[1];
        registry
            .set_component(
                id1,
                FogVolumeComponent {
                    density: 0.0,
                    glow: 0.0,
                    edge_softness: 0.0,
                    falloff: 1.0,
                    tint: [1.0, 1.0, 1.0],
                    saturation: 1.0,
                    min_brightness: 0.0,
                    light_range: 1.0,
                    animation: None,
                },
            )
            .unwrap();

        let _ = bridge.update_volumes(&registry).expect("two slots");
        let active = bridge.active_aabbs();
        assert_eq!(active.len(), 1, "only the live volume contributes an AABB");
        assert_eq!(active[0].0, Vec3::new(-2.0, 0.0, -2.0));
        assert_eq!(active[0].1, Vec3::new(2.0, 3.0, 2.0));
    }

    #[test]
    fn active_aabbs_clears_when_all_volumes_go_dark() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record()]);

        let _ = bridge.update_volumes(&registry).expect("one slot");
        assert_eq!(bridge.active_aabbs().len(), 1);

        let id0 = bridge.entity_ids[0];
        registry
            .set_component(
                id0,
                FogVolumeComponent {
                    density: 0.0,
                    glow: 0.0,
                    edge_softness: 0.0,
                    falloff: 1.0,
                    tint: [1.0, 1.0, 1.0],
                    saturation: 1.0,
                    min_brightness: 0.0,
                    light_range: 1.0,
                    animation: None,
                },
            )
            .unwrap();
        let _ = bridge.update_volumes(&registry).expect("still one slot");
        assert!(
            bridge.active_aabbs().is_empty(),
            "stale AABB must not survive into a frame where every volume is off"
        );
    }

    #[test]
    fn update_volumes_propagates_plane_count_from_record() {
        // A `FogVolumeRecord` carrying non-empty planes must surface its plane
        // count on the packed `FogVolume` (plane_count is at byte offset 92).
        // The renderer's dense repack patches plane_offset at upload time —
        // here we only assert plane_count, which the bridge writes directly.
        let mut record = sample_record();
        record.planes = vec![
            [1.0, 0.0, 0.0, 0.5],
            [-1.0, 0.0, 0.0, 0.5],
            [0.0, 1.0, 0.0, 0.5],
            [0.0, -1.0, 0.0, 0.5],
            [0.0, 0.0, 1.0, 0.5],
            [0.0, 0.0, -1.0, 0.5],
        ];
        record.plane_count = record.planes.len() as u32;

        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[record.clone()]);

        let (bytes, planes, _live_mask) = bridge.update_volumes(&registry).expect("one slot");
        assert_eq!(bytes.len(), std::mem::size_of::<FogVolume>());
        // FogVolume.plane_count sits at byte offset 92 (see fx/fog_volume.rs).
        let plane_count = u32::from_le_bytes(bytes[92..96].try_into().unwrap());
        assert_eq!(plane_count, record.plane_count);
        // Side-table planes mirror the record exactly.
        assert_eq!(planes.len(), 1);
        assert_eq!(planes[0], record.planes);
    }

    #[test]
    fn fog_volume_bridge_round_trips_ellipsoid_shape_mode() {
        // Ellipsoid volumes (`shape_mode == 1.0`) must round-trip from PRL
        // record → side-table → packed `FogVolume`. The shader compares
        // `> 0.5` to pick the ellipsoid path, so an inadvertent rename or
        // reorder on the bridge boundary would silently regress every
        // ellipsoid volume to the legacy radial fade.
        let mut record = sample_record();
        record.shape_mode = 1.0;

        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[record]);

        let (bytes, _planes, live_mask) = bridge.update_volumes(&registry).expect("one slot");
        assert_eq!(bytes.len(), std::mem::size_of::<FogVolume>());
        assert_eq!(live_mask, 0b1);
        // FogVolume.shape_mode sits at byte offset 60 — see fx/fog_volume.rs.
        let shape_mode = f32::from_le_bytes(bytes[60..64].try_into().unwrap());
        assert!(
            (shape_mode - 1.0).abs() < 1e-6,
            "ellipsoid shape_mode must survive the bridge; got {shape_mode}"
        );
    }

    #[test]
    fn clear_drops_state_but_preserves_buffer_capacity() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record()]);
        let _ = bridge.update_volumes(&registry).expect("one slot");
        let cap_before = bridge.volumes_bytes.capacity();

        bridge.clear();
        assert!(bridge.entity_ids.is_empty());
        assert!(bridge.aabbs.is_empty());
        assert!(bridge.volumes_bytes.is_empty());
        // Capacity invariant — best-effort, only meaningful when something was
        // actually packed.
        assert!(bridge.volumes_bytes.capacity() >= cap_before);
    }

    /// 16-sample sine curve over [0.0, 1.0) for animation evaluator tests.
    fn sine_curve(samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| {
                let phase = (i as f32 / samples as f32) * std::f32::consts::TAU;
                0.5 + 0.5 * phase.sin()
            })
            .collect()
    }

    fn install_fog_animation(registry: &mut EntityRegistry, id: EntityId, animation: FogAnimation) {
        let mut comp = registry
            .get_component::<FogVolumeComponent>(id)
            .unwrap()
            .clone();
        comp.animation = Some(animation);
        registry.set_component(id, comp).unwrap();
    }

    #[test]
    fn evaluator_writes_curve_sample_into_component_density() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record()]);
        let id = bridge.entity_ids[0];

        let curve = sine_curve(16);
        // Capture the expected value at t = 0.5 from the linear-interp sampler
        // — the same path the evaluator uses, so this is a self-consistency
        // check rather than a transcendental approximation.
        let expected_at_half = sample_density_curve(&curve, 0.5);
        install_fog_animation(
            &mut registry,
            id,
            FogAnimation {
                period_ms: 1000.0,
                phase: None,
                play_count: None,
                density: Some(curve),
                saturation: None,
                min_brightness: None,
                light_range: None,
            },
        );

        // First tick at t=0 anchors the start time. Second tick at half-period
        // samples t=0.5 (no phase offset).
        bridge.tick(&mut registry, 0.0);
        bridge.tick(&mut registry, 0.5);

        let density = registry
            .get_component::<FogVolumeComponent>(id)
            .unwrap()
            .density;
        assert!(
            (density - expected_at_half).abs() < 0.01,
            "density at t=0.5 expected ~{expected_at_half}, got {density}"
        );

        // Hand-computed check: 2-sample curve [0.0, 1.0] at t=0.5.
        // pos = 0.5 * 1 = 0.5, lo = 0.0, hi = 1.0, result = 0.0 + 0.5 = 0.5.
        // Pins the linear interpolation formula independently of the evaluator path.
        let two_sample_curve = [0.0_f32, 1.0_f32];
        let hand_computed = sample_density_curve(&two_sample_curve, 0.5);
        assert!(
            (hand_computed - 0.5_f32).abs() < 0.01,
            "linear interp of [0.0, 1.0] at t=0.5 must equal 0.5; got {hand_computed}"
        );
    }

    #[test]
    fn evaluator_settles_play_count_bounded_animation_and_clears_field() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record()]);
        let id = bridge.entity_ids[0];

        let curve = vec![0.1, 0.5, 0.9];
        install_fog_animation(
            &mut registry,
            id,
            FogAnimation {
                period_ms: 1000.0,
                phase: None,
                play_count: Some(1),
                density: Some(curve.clone()),
                saturation: None,
                min_brightness: None,
                light_range: None,
            },
        );

        bridge.tick(&mut registry, 0.0);
        // Past one period — must settle to the final keyframe and clear `animation`.
        bridge.tick(&mut registry, 1.5);

        let comp = registry.get_component::<FogVolumeComponent>(id).unwrap();
        assert!(
            comp.animation.is_none(),
            "animation field must be cleared after settle"
        );
        assert!(
            (comp.density - *curve.last().unwrap()).abs() < 1e-6,
            "settled density must equal the final keyframe; got {}",
            comp.density
        );
        assert!(
            !bridge.anim_slots.contains_key(&id),
            "side-table entry must be removed once settled"
        );
    }

    #[test]
    fn evaluator_skips_components_without_animation() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record()]);
        let id = bridge.entity_ids[0];

        let original = registry
            .get_component::<FogVolumeComponent>(id)
            .unwrap()
            .clone();

        bridge.tick(&mut registry, 0.0);
        bridge.tick(&mut registry, 0.5);

        let after = registry.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(after, &original);
        assert!(bridge.anim_slots.is_empty());
    }

    #[test]
    fn set_fog_animation_resets_start_time() {
        // Installing a second animation while one is in flight must phase the
        // new curve from t=0 — otherwise the first animation's elapsed time
        // bleeds into the second's sampling, which would manifest as an
        // unexpected "jump" the moment a script swaps curves.
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record()]);
        let id = bridge.entity_ids[0];

        let original_curve = vec![0.0, 1.0];
        install_fog_animation(
            &mut registry,
            id,
            FogAnimation {
                period_ms: 1000.0,
                phase: None,
                play_count: None,
                density: Some(original_curve),
                saturation: None,
                min_brightness: None,
                light_range: None,
            },
        );

        bridge.tick(&mut registry, 0.0);
        bridge.tick(&mut registry, 0.3);

        let new_curve = vec![0.7, 0.2];
        install_fog_animation(
            &mut registry,
            id,
            FogAnimation {
                period_ms: 1000.0,
                phase: None,
                play_count: None,
                density: Some(new_curve.clone()),
                saturation: None,
                min_brightness: None,
                light_range: None,
            },
        );
        // Second tick at the same wall-clock as the install — the new
        // animation should sample at t≈0, i.e. `new_curve[0]`. The bridge
        // detects the density-curve swap and resets the start time without
        // an explicit clear from the reaction.
        bridge.tick(&mut registry, 0.3);

        let density = registry
            .get_component::<FogVolumeComponent>(id)
            .unwrap()
            .density;
        let expected_first = new_curve[0];
        let expected_old = sample_density_curve(&[0.0, 1.0], 0.3);
        assert!(
            (density - expected_first).abs() < 1e-4,
            "density should sample new curve at t=0 (~{expected_first}), \
             not the old curve at t=0.3 (~{expected_old}); got {density}"
        );
    }

    #[test]
    fn set_fog_animation_resets_start_time_when_only_period_changes() {
        // Reinstalling with the same density curve but a different `period_ms`
        // must still reset the start anchor — otherwise a script tweaking only
        // the period would keep the prior animation's accumulated phase.
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record()]);
        let id = bridge.entity_ids[0];

        let curve = vec![0.0, 1.0];
        install_fog_animation(
            &mut registry,
            id,
            FogAnimation {
                period_ms: 1000.0,
                phase: None,
                play_count: None,
                density: Some(curve.clone()),
                saturation: None,
                min_brightness: None,
                light_range: None,
            },
        );

        bridge.tick(&mut registry, 0.0);
        bridge.tick(&mut registry, 0.4);

        // Same density curve, different period.
        install_fog_animation(
            &mut registry,
            id,
            FogAnimation {
                period_ms: 500.0,
                phase: None,
                play_count: None,
                density: Some(curve.clone()),
                saturation: None,
                min_brightness: None,
                light_range: None,
            },
        );
        bridge.tick(&mut registry, 0.4);

        let density = registry
            .get_component::<FogVolumeComponent>(id)
            .unwrap()
            .density;
        let expected_first = curve[0];
        assert!(
            (density - expected_first).abs() < 1e-4,
            "period-only change must reset start anchor and sample at t=0 (~{expected_first}); got {density}"
        );
    }

    #[test]
    fn evaluator_writes_curve_sample_into_component_min_brightness() {
        // A FogAnimation with only a min_brightness curve (no density/saturation)
        // must write the sampled value to FogVolumeComponent.min_brightness each
        // frame without touching other fields.
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record()]);
        let id = bridge.entity_ids[0];

        // Two-sample ramp: [0.0, 1.0]. At t=0.5 the linear interpolation
        // returns exactly 0.5 — hand-checkable without transcendental math.
        let curve = vec![0.0_f32, 1.0_f32];
        let expected_at_half = sample_density_curve(&curve, 0.5);
        install_fog_animation(
            &mut registry,
            id,
            FogAnimation {
                period_ms: 1000.0,
                phase: None,
                play_count: None,
                density: None,
                saturation: None,
                min_brightness: Some(curve),
                light_range: None,
            },
        );

        // First tick at t=0 anchors the start time; second tick at half-period
        // samples the curve at t=0.5.
        bridge.tick(&mut registry, 0.0);
        bridge.tick(&mut registry, 0.5);

        let comp = registry.get_component::<FogVolumeComponent>(id).unwrap();
        assert!(
            (comp.min_brightness - expected_at_half).abs() < 1e-4,
            "min_brightness at t=0.5 expected ~{expected_at_half}, got {}",
            comp.min_brightness
        );
        // Static density must be untouched (sample_record initialises it to 0.5).
        assert!(
            (comp.density - 0.5).abs() < 1e-6,
            "density must not be modified by a min_brightness-only animation; got {}",
            comp.density
        );
    }

    #[test]
    fn evaluator_settles_play_count_bounded_min_brightness_and_clears_field() {
        // After play_count periods expire, FogVolumeComponent.min_brightness
        // must hold the final keyframe value and `animation` must be None.
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record()]);
        let id = bridge.entity_ids[0];

        let curve = vec![0.1_f32, 0.5, 0.8];
        install_fog_animation(
            &mut registry,
            id,
            FogAnimation {
                period_ms: 1000.0,
                phase: None,
                play_count: Some(1),
                density: None,
                saturation: None,
                min_brightness: Some(curve.clone()),
                light_range: None,
            },
        );

        bridge.tick(&mut registry, 0.0);
        // 1.5 seconds elapsed — past one 1000 ms period; must settle.
        bridge.tick(&mut registry, 1.5);

        let comp = registry.get_component::<FogVolumeComponent>(id).unwrap();
        assert!(
            comp.animation.is_none(),
            "animation field must be cleared after settle"
        );
        assert!(
            (comp.min_brightness - *curve.last().unwrap()).abs() < 1e-6,
            "settled min_brightness must equal the final keyframe; got {}",
            comp.min_brightness
        );
        assert!(
            !bridge.anim_slots.contains_key(&id),
            "side-table entry must be removed once settled"
        );
    }

    #[test]
    fn evaluator_writes_curve_sample_into_component_light_range() {
        // A FogAnimation with only a light_range curve (no other
        // channels) must write the sampled value to
        // FogVolumeComponent.light_range each frame without touching
        // other fields.
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record()]);
        let id = bridge.entity_ids[0];

        // Two-sample ramp: [0.5, 1.5]. At t=0.25 → pos = 0.25, lo = 0.5,
        // hi = 1.5, result = 0.5 + 0.25 * 1.0 = 0.75.
        let curve = vec![0.5_f32, 1.5_f32];
        let expected_at_quarter = sample_density_curve(&curve, 0.25);
        install_fog_animation(
            &mut registry,
            id,
            FogAnimation {
                period_ms: 1000.0,
                phase: None,
                play_count: None,
                density: None,
                saturation: None,
                min_brightness: None,
                light_range: Some(curve),
            },
        );

        // First tick at t=0 anchors the start time; second tick at t=0.25 s
        // (a quarter period) samples the curve at normalised t=0.25.
        bridge.tick(&mut registry, 0.0);
        bridge.tick(&mut registry, 0.25);

        let comp = registry.get_component::<FogVolumeComponent>(id).unwrap();
        assert!(
            (comp.light_range - expected_at_quarter).abs() < 1e-4,
            "light_range at t=0.25 expected ~{expected_at_quarter}, got {}",
            comp.light_range
        );
        // Static density must be untouched (sample_record initialises it to 0.5).
        assert!(
            (comp.density - 0.5).abs() < 1e-6,
            "density must not be modified by a light_range-only animation; got {}",
            comp.density
        );
    }
}
