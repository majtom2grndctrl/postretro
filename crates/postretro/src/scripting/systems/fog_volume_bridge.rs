// Scripting ↔ renderer bridge for fog volumes: entity registry → GPU fog buffers.
// See: context/lib/scripting.md
//
// Mirrors `light_bridge.rs`: per-frame, walks the entity registry to repack
// GPU-ready bytes, returns them to the renderer through a narrow `&[u8]` API
// so the renderer never sees a scripting type.
//
// Fog volume AABBs are baked at level load (immutable runtime) and cached in
// `aabbs` here. Density / colour / scatter / falloff are runtime-tweakable
// `FogVolumeComponent` fields read from the entity registry on every update.

use std::collections::HashMap;

use anyhow::Result;
use glam::{Quat, Vec3};

use crate::fx::fog_volume::{FogPointLight, FogVolume, MAX_FOG_POINT_LIGHTS};
use crate::prl::{LightType, MapLight};
use crate::scripting::registry::{EntityId, EntityRegistry, FogVolumeComponent, Transform};
use postretro_level_format::fog_volumes::FogVolumeRecord;

/// Authoring-time AABB plus the two compile-time falloff parameters carried
/// alongside it. These are not runtime-settable, so they live in a side-table
/// rather than on `FogVolumeComponent`.
pub struct FogVolumeAabb {
    pub min: Vec3,
    pub max: Vec3,
    pub height_gradient: f32,
    pub radial_falloff: f32,
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
}

impl FogVolumeBridge {
    pub(crate) fn new() -> Self {
        Self {
            aabbs: HashMap::new(),
            entity_ids: Vec::new(),
            volumes_bytes: Vec::new(),
            points_bytes: Vec::new(),
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
    ) -> Result<()> {
        self.aabbs.clear();
        self.entity_ids.clear();
        self.entity_ids.reserve(records.len());

        for entry in records {
            let center = (Vec3::from(entry.min) + Vec3::from(entry.max)) * 0.5;
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
                },
            );
            self.entity_ids.push(id);
        }

        Ok(())
    }

    /// Drop per-level state. Byte-buffer capacity is retained so the next
    /// level reuses the allocation.
    pub(crate) fn clear(&mut self) {
        self.aabbs.clear();
        self.entity_ids.clear();
        // Length cleared, capacity retained.
        self.volumes_bytes.clear();
        self.points_bytes.clear();
    }

    /// Walk the registry and repack one `FogVolume` GPU record per tracked
    /// entity. Returns `None` when there are no fog volumes — the renderer
    /// uses this to skip the whole upload path.
    pub(crate) fn update_volumes(&mut self, registry: &EntityRegistry) -> Option<&[u8]> {
        self.volumes_bytes.clear();
        if self.entity_ids.is_empty() {
            return None;
        }

        let mut packed: Vec<FogVolume> = Vec::with_capacity(self.entity_ids.len());
        for id in &self.entity_ids {
            let Ok(component) = registry.get_component::<FogVolumeComponent>(*id) else {
                // Entity despawned or component removed; skip the slot.
                continue;
            };
            let Some(aabb) = self.aabbs.get(id) else {
                continue;
            };
            packed.push(FogVolume {
                min: aabb.min.to_array(),
                density: component.density,
                max_v: aabb.max.to_array(),
                falloff: component.falloff,
                color: component.color,
                scatter: component.scatter,
                height_gradient: aabb.height_gradient,
                radial_falloff: aabb.radial_falloff,
                _pad0: 0.0,
                _pad1: 0.0,
            });
        }

        if packed.is_empty() {
            return None;
        }
        self.volumes_bytes
            .extend_from_slice(bytemuck::cast_slice(&packed));
        Some(&self.volumes_bytes)
    }

    /// Pre-cull a slice of map lights against the cached fog-volume AABBs and
    /// pack the survivors as `FogPointLight` records. Filters to dynamic point
    /// lights only — static lights bake into the SH volume; spot lights have a
    /// dedicated path. Capped at `MAX_FOG_POINT_LIGHTS`.
    ///
    /// `effective_brightness` parallels `lights` (same indexing — see
    /// `LightBridgeUpdate::effective_brightness`). When present, each light's
    /// authored `intensity` is multiplied by its effective brightness before
    /// pre-multiplying into `color`, so `setComponent`-driven intensity
    /// changes (which mutate the bridge, not the static `MapLight` slice)
    /// reach the fog halo. An empty/short slice means "no scripted override"
    /// and falls back to a multiplier of `1.0`.
    pub(crate) fn update_points(
        &mut self,
        lights: &[MapLight],
        effective_brightness: &[f32],
    ) -> &[u8] {
        self.points_bytes.clear();
        if self.aabbs.is_empty() || lights.is_empty() {
            return &self.points_bytes;
        }

        // Suppress fully-dark lights (matches the renderer's shadow-slot
        // suppression threshold in `update_dynamic_light_slots`).
        const BRIGHTNESS_SUPPRESSION_THRESHOLD: f32 = 0.01;

        let mut packed: Vec<FogPointLight> = Vec::new();
        for (i, light) in lights.iter().enumerate() {
            if !matches!(light.light_type, LightType::Point) || !light.is_dynamic {
                continue;
            }
            let multiplier = effective_brightness.get(i).copied().unwrap_or(1.0);
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
            let intensity = light.intensity * multiplier;
            packed.push(FogPointLight {
                position: center.to_array(),
                range,
                color: [
                    light.color[0] * intensity,
                    light.color[1] * intensity,
                    light.color[2] * intensity,
                ],
                _pad: 0.0,
            });
            if packed.len() >= MAX_FOG_POINT_LIGHTS {
                break;
            }
        }

        if !packed.is_empty() {
            self.points_bytes
                .extend_from_slice(bytemuck::cast_slice(&packed));
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
        bridge
            .populate_from_level(&mut registry, &[sample_record()])
            .unwrap();

        assert_eq!(bridge.entity_ids.len(), 1);
        let id = bridge.entity_ids[0];
        let comp = registry.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(comp.density, 0.5);
        assert_eq!(comp.scatter, 0.4);
        let aabb = bridge.aabbs.get(&id).unwrap();
        assert_eq!(aabb.min, Vec3::new(-2.0, 0.0, -2.0));
        assert_eq!(aabb.height_gradient, 0.25);
    }

    #[test]
    fn update_volumes_returns_none_when_no_records() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge.populate_from_level(&mut registry, &[]).unwrap();
        assert!(bridge.update_volumes(&registry).is_none());
    }

    #[test]
    fn update_volumes_packs_density_and_falloff_from_component() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge
            .populate_from_level(&mut registry, &[sample_record()])
            .unwrap();

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

        let bytes = bridge.update_volumes(&registry).expect("dirty volumes");
        assert_eq!(bytes.len(), std::mem::size_of::<FogVolume>());
        // density at byte offset 12, falloff at byte offset 28.
        let density = f32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let falloff = f32::from_le_bytes(bytes[28..32].try_into().unwrap());
        assert_eq!(density, 1.25);
        assert_eq!(falloff, 0.5);
    }

    #[test]
    fn update_points_pre_culls_lights_outside_every_aabb() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge
            .populate_from_level(&mut registry, &[sample_record()])
            .unwrap();

        let inside = point_light([0.0, 1.0, 0.0], 5.0, true);
        let near_miss = point_light([100.0, 100.0, 100.0], 5.0, true);
        let static_light = point_light([0.0, 1.0, 0.0], 5.0, false);

        let bytes = bridge.update_points(&[inside, near_miss, static_light], &[]);
        // Only the dynamic in-range light passes both filters.
        assert_eq!(bytes.len(), std::mem::size_of::<FogPointLight>());
    }

    #[test]
    fn update_points_premultiplies_color_by_intensity() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge
            .populate_from_level(&mut registry, &[sample_record()])
            .unwrap();

        let light = point_light([0.0, 1.0, 0.0], 5.0, true);
        let bytes = bridge.update_points(&[light], &[]).to_vec();
        // FogPointLight layout: position (12) | range (4) | color (12) | _pad (4).
        let r = f32::from_le_bytes(bytes[16..20].try_into().unwrap());
        // intensity 2.0 × color.r 1.0 = 2.0
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
        bridge
            .populate_from_level(&mut registry, &[sample_record()])
            .unwrap();

        let light = point_light([0.0, 1.0, 0.0], 5.0, true);
        // light.intensity = 2.0, color.r = 1.0, multiplier = 0.25 → 0.5
        let bytes = bridge.update_points(&[light], &[0.25]).to_vec();
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
        bridge
            .populate_from_level(&mut registry, &[sample_record()])
            .unwrap();

        let light = point_light([0.0, 1.0, 0.0], 5.0, true);
        let bytes = bridge.update_points(&[light], &[0.0]);
        assert!(bytes.is_empty(), "dark light must be suppressed");
    }

    #[test]
    fn clear_drops_state_but_preserves_buffer_capacity() {
        let mut registry = EntityRegistry::new();
        let mut bridge = FogVolumeBridge::new();
        bridge
            .populate_from_level(&mut registry, &[sample_record()])
            .unwrap();
        let _ = bridge.update_volumes(&registry);
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
