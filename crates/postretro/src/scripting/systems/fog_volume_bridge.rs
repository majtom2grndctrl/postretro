// Scripting ↔ renderer bridge for fog volumes: entity registry → GPU fog buffers.
// See: context/lib/scripting.md
//
// Mirrors `light_bridge.rs`: per-frame, walks the entity registry to repack
// GPU-ready bytes. `update_volumes` returns `Option<&[u8]>` — `None` means no active volumes.
// The caller uploads an empty slice on `None`, which zeroes `volume_count` and
// causes `FogPass::active()` to return false, skipping the compute dispatch.
//
// Fog volume AABBs are baked into the PRL at compile time (immutable at runtime)
// and cached in `aabbs` here. Density / colour / scatter / falloff are runtime-tweakable
// `FogVolumeComponent` fields read from the entity registry on every update.

use std::collections::HashMap;

use glam::{Quat, Vec3};

use crate::fx::fog_volume::{FogPointLight, FogVolume, MAX_FOG_POINT_LIGHTS};
use crate::prl::{LightType, MapLight};
use crate::scripting::registry::{EntityId, EntityRegistry, FogVolumeComponent, Transform};
use postretro_level_format::fog_volumes::FogVolumeRecord;

/// Authoring-time AABB plus the two compile-time falloff parameters carried
/// alongside it. These are not runtime-settable — surfacing them at runtime
/// would require adding them to `FogVolumeComponent` and the scripting API —
/// so they live in a side-table rather than on `FogVolumeComponent`.
///
/// `center`, `inv_half_ext`, `half_diag`, and `inv_height_extent` are baked
/// into the PRL by the level compiler; they're cached here so per-frame fog
/// uploads can copy precomputed values without recomputing them from min/max.
pub struct FogVolumeAabb {
    pub min: Vec3,
    pub max: Vec3,
    pub height_gradient: f32,
    pub radial_falloff: f32,
    pub center: Vec3,
    pub inv_half_ext: Vec3,
    pub half_diag: f32,
    pub inv_height_extent: f32,
}

/// State carried across frames. Owned by the game layer so the renderer never
/// holds component data.
pub(crate) struct FogVolumeBridge {
    /// Compile-time AABB + height/radial falloff per entity, keyed by `EntityId`.
    aabbs: HashMap<EntityId, FogVolumeAabb>,
    /// Map-volume index → `EntityId`. Fixed at level load.
    entity_ids: Vec<EntityId>,
    /// Reusable byte buffer for `FogVolume` records. Capacity retained between
    /// frames; cleared at the start of each `update_volumes` call.
    volumes_bytes: Vec<u8>,
    /// Reusable byte buffer for `FogPointLight` records.
    points_bytes: Vec<u8>,
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
            points_bytes: Vec::new(),
            warned_overflow: false,
        }
    }

    /// Populate the entity registry with one entity per fog-volume record.
    /// Called once at level load. Stores the AABB + height/radial falloff in
    /// the side-table; the four runtime-settable parameters become a
    /// `FogVolumeComponent` on the spawned entity.
    pub(crate) fn populate_from_level(
        &mut self,
        registry: &mut EntityRegistry,
        records: &[FogVolumeRecord],
    ) {
        self.aabbs.clear();
        self.entity_ids.clear();
        self.entity_ids.reserve(records.len());

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
                color: entry.color,
                scatter: entry.scatter,
                falloff: entry.falloff,
            };
            // `set_component` only fails on stale id — the id was just returned.
            let _ = registry.set_component(id, component);

            self.aabbs.insert(
                id,
                FogVolumeAabb {
                    min: Vec3::from(entry.min),
                    max: Vec3::from(entry.max),
                    height_gradient: entry.height_gradient,
                    radial_falloff: entry.radial_falloff,
                    center: Vec3::from(entry.center),
                    inv_half_ext: Vec3::from(entry.inv_half_ext),
                    half_diag: entry.half_diag,
                    inv_height_extent: entry.inv_height_extent,
                },
            );
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
        self.points_bytes.clear();
        self.warned_overflow = false;
    }

    /// Walk the registry and repack one `FogVolume` GPU record per tracked
    /// entity, in original PRL record order. Returns `None` when there are no
    /// fog volumes (empty `entity_ids`) — the renderer uses this to skip the
    /// whole upload path.
    ///
    /// The returned byte slice has length `entity_ids.len() * FOG_VOLUME_SIZE`
    /// — every slot is emitted, in source order, even when the underlying
    /// entity has no `FogVolumeComponent` or has density ≤ 0. This preserves
    /// the canonical index that `FogCellMasks` bits refer to.
    ///
    /// The returned `live_mask` has bit `i` set iff slot `i` has a present
    /// component with density > 0. The renderer ANDs this into the
    /// portal-cull mask so density-zero slots never reach the GPU.
    pub(crate) fn update_volumes(&mut self, registry: &EntityRegistry) -> Option<(&[u8], u32)> {
        self.volumes_bytes.clear();
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
            }

            let fv = match (component, aabb) {
                (Some(component), Some(aabb)) => FogVolume {
                    min: aabb.min.to_array(),
                    density: component.density,
                    max_v: aabb.max.to_array(),
                    falloff: component.falloff,
                    color: component.color,
                    scatter: component.scatter,
                    center: aabb.center.to_array(),
                    half_diag: aabb.half_diag,
                    inv_half_ext: aabb.inv_half_ext.to_array(),
                    inv_height_extent: aabb.inv_height_extent,
                    height_gradient: aabb.height_gradient,
                    radial_falloff: aabb.radial_falloff,
                    _pad: [0.0, 0.0],
                },
                _ => FogVolume {
                    min: [0.0; 3],
                    density: 0.0,
                    max_v: [0.0; 3],
                    falloff: 0.0,
                    color: [0.0; 3],
                    scatter: 0.0,
                    center: [0.0; 3],
                    half_diag: 0.0,
                    inv_half_ext: [0.0; 3],
                    inv_height_extent: 0.0,
                    height_gradient: 0.0,
                    radial_falloff: 0.0,
                    _pad: [0.0, 0.0],
                },
            };
            self.volumes_bytes
                .extend_from_slice(bytemuck::bytes_of(&fv));
        }

        Some((&self.volumes_bytes, live_mask))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prl::{FalloffModel, LightType, MapLight};

    fn sample_record() -> FogVolumeRecord {
        FogVolumeRecord {
            min: [-2.0, 0.0, -2.0],
            density: 0.5,
            max: [2.0, 3.0, 2.0],
            falloff: 1.0,
            color: [0.6, 0.7, 0.8],
            scatter: 0.4,
            height_gradient: 0.25,
            radial_falloff: 0.0,
            center: [0.0, 1.5, 0.0],
            inv_half_ext: [0.5, 1.0 / 1.5, 0.5],
            half_diag: 2.5,
            inv_height_extent: 1.0 / 3.0,
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
            tags: vec![],
            leaf_index: 0,
        }
    }

    #[test]
    fn populate_from_level_spawns_one_entity_per_record_with_component() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record()]);

        assert_eq!(bridge.entity_ids.len(), 1);
        let id = bridge.entity_ids[0];
        let comp = registry.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(comp.density, 0.5);
        assert_eq!(comp.scatter, 0.4);
        let aabb = bridge.aabbs.get(&id).unwrap();
        assert_eq!(aabb.min, Vec3::new(-2.0, 0.0, -2.0));
        assert_eq!(aabb.height_gradient, 0.25);
        assert_eq!(aabb.center, Vec3::new(0.0, 1.5, 0.0));
        assert_eq!(aabb.inv_half_ext, Vec3::new(0.5, 1.0 / 1.5, 0.5));
        assert_eq!(aabb.half_diag, 2.5);
        assert_eq!(aabb.inv_height_extent, 1.0 / 3.0);
    }

    #[test]
    fn update_volumes_returns_none_when_no_records() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[]);
        assert!(bridge.update_volumes(&registry).is_none());
    }

    #[test]
    fn update_volumes_packs_density_and_falloff_from_component() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[sample_record()]);

        // Mutate via the registry the way a script would.
        let id = bridge.entity_ids[0];
        registry
            .set_component(
                id,
                FogVolumeComponent {
                    density: 1.25,
                    color: [0.1, 0.2, 0.3],
                    scatter: 0.9,
                    falloff: 0.5,
                },
            )
            .unwrap();

        let (bytes, live_mask) = bridge.update_volumes(&registry).expect("dirty volumes");
        assert_eq!(bytes.len(), std::mem::size_of::<FogVolume>());
        // density at byte offset 12, falloff at byte offset 28.
        let density = f32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let falloff = f32::from_le_bytes(bytes[28..32].try_into().unwrap());
        assert_eq!(density, 1.25);
        assert_eq!(falloff, 0.5);
        assert_eq!(
            live_mask, 0b1,
            "single non-zero-density slot should be live"
        );
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
                    color: [0.0; 3],
                    scatter: 0.0,
                    falloff: 0.0,
                },
            )
            .unwrap();

        let (bytes, live_mask) = bridge.update_volumes(&registry).expect("two slots");
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
}
