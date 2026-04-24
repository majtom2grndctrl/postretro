// Light bridge: seam between the scripting entity registry and the renderer's GPU light buffer.
// See: context/lib/scripting.md

use std::collections::HashMap;

use crate::lighting::{GPU_LIGHT_SIZE, pack_light};
use crate::prl::{FalloffModel, LightType, MapLight};
use crate::render::sh_volume::ANIMATION_DESCRIPTOR_SIZE;

#[cfg(test)]
use crate::scripting::components::light::LightAnimation;
use crate::scripting::components::light::{FalloffKind, LightComponent, LightKind};
use crate::scripting::registry::{EntityId, EntityRegistry};

/// Snapshot of a map light's component state as last observed by the bridge.
/// Dirty detection compares the live registry component against this value.
///
/// `animation_start_time` is `Some(t)` while a `play_count`-bounded animation
/// is running: `t` is the engine time (seconds since level load) when the
/// animation was last (re)written. On each `update`, the bridge compares
/// `current_time − t` against `play_count × period_ms / 1000.0`; when the
/// elapsed time reaches the completion bound, the bridge samples the final
/// keyframe value, writes a static `LightComponent` back to the registry,
/// and clears this field. Any `setAnimation` call (including one that stores
/// an identical animation value) must reset `animation_start_time` to the
/// current frame time — "last call wins" always restarts the count from zero.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LightSnapshot {
    pub(crate) component: LightComponent,
    pub(crate) animation_start_time: Option<f32>,
}

/// Payload handed back to the renderer after `update`. All three fields are
/// `Some` together when any `LightComponent` changed since last frame; `None`
/// means nothing to upload. Keeping the trio atomic avoids partial uploads
/// (re-packed lights without the matching descriptor rewrite, etc.).
///
/// - `lights_bytes` — packed `GpuLight` records for `queue.write_buffer` on
///   the renderer's `lights_buffer`. Always sized to the authored-light count
///   at level load.
/// - `descriptor_bytes` — one `AnimationDescriptor` record per map light, in
///   the same order as `lights_bytes`. Lights without an animation get the
///   sentinel descriptor (all three counts zero) so `forward.wgsl` falls back
///   to the static `intensity`/`color`/`cone_direction` path.
#[derive(Debug)]
pub(crate) struct LightBridgeUpdate {
    pub(crate) lights_bytes: Vec<u8>,
    pub(crate) descriptor_bytes: Vec<u8>,
}

/// State carried across frames. Owned by the game layer (not the renderer) so
/// the "renderer owns GPU" invariant holds.
pub(crate) struct LightBridge {
    /// Map-light index (in `LevelWorld.lights` order) → `EntityId` assigned at
    /// `populate_from_level`. Fixed at level load; never grows or shrinks.
    entity_ids: Vec<EntityId>,
    /// Dirty-tracking snapshots. Populated lazily on the first frame an entity
    /// is observed with a `LightComponent`. `None` for a `map_index` means the
    /// slot has never been snapshotted — treated as "unconditionally dirty" on
    /// first visit so the initial upload lands.
    snapshots: HashMap<EntityId, LightSnapshot>,
    /// Shape metadata needed to re-pack. Parallels `entity_ids`.
    shape: Vec<MapLightShape>,
    /// `true` if at least one entity changed since the last `update` call.
    /// Sticky across a frame until `update` returns the packed bytes.
    dirty: bool,
    /// Scratch: captured at level load so `populate_from_level` can be called
    /// without handing the raw `MapLight` list to every subsequent `update`.
    cached_origins_f64: Vec<[f64; 3]>,
}

/// Per-light fields that the script-facing `LightComponent` does not carry
/// (because they are runtime-only concerns). Kept so the bridge can rebuild a
/// `MapLight` from a mutated `LightComponent` without the renderer having to
/// hand the original list back every frame.
#[derive(Debug, Clone)]
struct MapLightShape {
    is_dynamic: bool,
}

impl LightBridge {
    pub(crate) fn new() -> Self {
        Self {
            entity_ids: Vec::new(),
            snapshots: HashMap::new(),
            shape: Vec::new(),
            dirty: false,
            cached_origins_f64: Vec::new(),
        }
    }

    /// Number of map lights the bridge owns. Equals `LevelWorld.lights.len()`
    /// captured at `populate_from_level` time.
    // Not yet called from production paths; retained for future `world.query`
    // expansion that will expose per-light handles indexed by map position.
    #[allow(dead_code)]
    pub(crate) fn light_count(&self) -> usize {
        self.entity_ids.len()
    }

    /// Lookup the `EntityId` for a given map-light index. Used by the
    /// `world.query` primitive to build `LightEntity` handles.
    // Not yet called from production paths; retained for future `world.query`
    // expansion that will expose per-light handles indexed by map position.
    #[allow(dead_code)]
    pub(crate) fn entity_for_map_index(&self, map_index: usize) -> Option<EntityId> {
        self.entity_ids.get(map_index).copied()
    }

    /// Populate the entity registry with one entity per map light, and seed
    /// the bridge's internal state. Called once at level load.
    ///
    /// f64 → f32 origin conversion happens here at the bridge boundary — the
    /// only seam that touches both precisions. The f64 source stays on the
    /// baker side; script-facing `LightComponent.origin` is f32.
    pub(crate) fn populate_from_level(
        &mut self,
        lights: &[MapLight],
        registry: &mut EntityRegistry,
    ) {
        self.entity_ids.clear();
        self.snapshots.clear();
        self.shape.clear();
        self.cached_origins_f64.clear();
        self.entity_ids.reserve(lights.len());
        self.shape.reserve(lights.len());
        self.cached_origins_f64.reserve(lights.len());

        for light in lights {
            let component = map_light_to_component(light);
            // We spawn with a default Transform; the scripting registry does
            // not model a separate "position" channel for lights — `origin`
            // lives on `LightComponent`.
            let Some(id) = registry.try_spawn(Default::default()) else {
                log::warn!(
                    "[LightBridge] entity registry exhausted; dropping map light (index {}). \
                     Further map lights in this level will not appear in the scripting surface.",
                    self.entity_ids.len()
                );
                break;
            };
            // `set_component` can only fail on a stale ID; the ID was just
            // returned by `try_spawn` on this same borrow so it must be live.
            let _ = registry.set_component(id, component);
            self.entity_ids.push(id);
            self.shape.push(MapLightShape {
                is_dynamic: light.is_dynamic,
            });
            self.cached_origins_f64.push(light.origin);
        }

        // First-frame upload is always dirty so the initial pack lands even
        // when no script mutates anything. Renderer-side init already uploads
        // a packed buffer; re-running on frame one is harmless and ensures the
        // animation descriptor set is in sync.
        self.dirty = true;
    }

    /// Walk every `LightComponent` entity; detect mutations, settle any
    /// completed `play_count`-bounded animations, and — when anything
    /// changed — return the re-packed light + descriptor buffers for the
    /// renderer to upload.
    ///
    /// `current_time` is seconds since level load (matches the engine frame
    /// clock passed to `update_per_frame_uniforms`). Only consulted when a
    /// `LightComponent.animation` carries `play_count: Some(n)`.
    pub(crate) fn update(
        &mut self,
        registry: &mut EntityRegistry,
        current_time: f32,
    ) -> Option<LightBridgeUpdate> {
        if self.entity_ids.is_empty() {
            return None;
        }

        // Pass 1: diff + play_count completion detection. We walk the bridge's
        // `entity_ids` (fixed at level load) rather than the registry's full
        // iterator — map lights have stable slots and we want index
        // correspondence for re-packing.
        //
        // Settled animations are collected and written back after the loop to
        // avoid aliasing the registry borrow.
        let mut settled: Vec<(EntityId, LightComponent)> = Vec::new();
        for &id in &self.entity_ids {
            let Ok(current) = registry.get_component::<LightComponent>(id) else {
                continue;
            };
            let snapshot = self.snapshots.get(&id);

            // Compare against last-observed state.
            let changed = match snapshot {
                Some(snap) => snap.component != *current,
                None => true,
            };

            // Detect play_count completion.
            if let Some(settled_component) =
                check_play_count_completion(current, snapshot, current_time)
            {
                settled.push((id, settled_component));
                continue;
            }

            if changed {
                self.dirty = true;
                let mut new_start = None;
                if let Some(anim) = &current.animation
                    && anim.play_count.is_some()
                {
                    // A play_count-bounded anim: record the start time so
                    // completion can fire on a future frame. "Last call wins"
                    // means any mutation resets the clock.
                    new_start = Some(current_time);
                }
                self.snapshots.insert(
                    id,
                    LightSnapshot {
                        component: current.clone(),
                        animation_start_time: new_start,
                    },
                );
            }
        }

        // Pass 2: commit settled components. `set_component` writes the new
        // static `LightComponent` back through the registry so a subsequent
        // `world.query` observes the post-animation state rather than an
        // animation that has already finished (Sub-plan 4 spec).
        for (id, settled_component) in settled {
            // Ignore a stale-id error on the unlikely path where the entity
            // was despawned between our read and this write.
            let _ = registry.set_component(id, settled_component.clone());
            self.snapshots.insert(
                id,
                LightSnapshot {
                    component: settled_component,
                    animation_start_time: None,
                },
            );
            self.dirty = true;
        }

        if !self.dirty {
            return None;
        }
        self.dirty = false;

        // Re-pack. Walk `entity_ids` in order; for each, pull the live
        // component and build both buffers.
        let mut lights_bytes: Vec<u8> = Vec::with_capacity(self.entity_ids.len() * GPU_LIGHT_SIZE);
        let mut descriptor_bytes: Vec<u8> =
            Vec::with_capacity(self.entity_ids.len() * ANIMATION_DESCRIPTOR_SIZE);

        for (map_idx, &id) in self.entity_ids.iter().enumerate() {
            let Ok(component) = registry.get_component::<LightComponent>(id) else {
                // Entity was despawned — push a zeroed slot so downstream
                // offsets stay aligned. The renderer's `light_count` bound
                // keeps the slot off the shading path; Plan 2 forbids script
                // despawn but a future plan may reach this case.
                lights_bytes.extend_from_slice(&[0u8; GPU_LIGHT_SIZE]);
                descriptor_bytes.extend_from_slice(&[0u8; ANIMATION_DESCRIPTOR_SIZE]);
                continue;
            };

            let map_light = component_to_map_light(
                component,
                self.cached_origins_f64[map_idx],
                self.shape[map_idx].is_dynamic,
            );
            lights_bytes.extend_from_slice(&pack_light(&map_light));

            let desc = pack_animation_descriptor(component);
            descriptor_bytes.extend_from_slice(&desc);
        }

        Some(LightBridgeUpdate {
            lights_bytes,
            descriptor_bytes,
        })
    }
}

impl Default for LightBridge {
    fn default() -> Self {
        Self::new()
    }
}

// --- Conversions ---

fn map_light_to_component(light: &MapLight) -> LightComponent {
    let light_type = match light.light_type {
        LightType::Point => LightKind::Point,
        LightType::Spot => LightKind::Spot,
        LightType::Directional => LightKind::Directional,
    };
    let falloff_model = match light.falloff_model {
        FalloffModel::Linear => FalloffKind::Linear,
        FalloffModel::InverseDistance => FalloffKind::InverseDistance,
        FalloffModel::InverseSquared => FalloffKind::InverseSquared,
    };
    let is_spot = matches!(light_type, LightKind::Spot);
    let is_directional = matches!(light_type, LightKind::Directional);
    LightComponent {
        origin: [
            light.origin[0] as f32,
            light.origin[1] as f32,
            light.origin[2] as f32,
        ],
        light_type,
        intensity: light.intensity,
        color: light.color,
        falloff_model,
        falloff_range: light.falloff_range,
        cone_angle_inner: if is_spot {
            Some(light.cone_angle_inner)
        } else {
            None
        },
        cone_angle_outer: if is_spot {
            Some(light.cone_angle_outer)
        } else {
            None
        },
        cone_direction: if is_spot || is_directional {
            Some(light.cone_direction)
        } else {
            None
        },
        cast_shadows: light.cast_shadows,
        is_dynamic: light.is_dynamic,
        animation: None,
    }
}

fn component_to_map_light(
    component: &LightComponent,
    origin_f64: [f64; 3],
    is_dynamic: bool,
) -> MapLight {
    let light_type = match component.light_type {
        LightKind::Point => LightType::Point,
        LightKind::Spot => LightType::Spot,
        LightKind::Directional => LightType::Directional,
    };
    let falloff_model = match component.falloff_model {
        FalloffKind::Linear => FalloffModel::Linear,
        FalloffKind::InverseDistance => FalloffModel::InverseDistance,
        FalloffKind::InverseSquared => FalloffModel::InverseSquared,
    };
    MapLight {
        // Preserve the original f64 origin from level load — script mutations
        // do not touch position (Plan 2 non-goal), so round-tripping through
        // the f32 component would needlessly drop precision.
        origin: origin_f64,
        light_type,
        intensity: component.intensity,
        color: component.color,
        falloff_model,
        falloff_range: component.falloff_range,
        cone_angle_inner: component.cone_angle_inner.unwrap_or(0.0),
        cone_angle_outer: component.cone_angle_outer.unwrap_or(0.0),
        cone_direction: component.cone_direction.unwrap_or([0.0, 0.0, 0.0]),
        cast_shadows: component.cast_shadows,
        is_dynamic,
    }
}

/// Pack one `LightComponent`'s animation state into a 48-byte
/// `AnimationDescriptor` record matching the WGSL layout (see
/// `postretro/src/render/sh_volume.rs` for the authoritative byte offsets).
///
/// **Sentinel descriptor:** lights with `animation == None` produce a
/// 48-byte record where `brightness_count`, `color_count`, and the reserved
/// direction count are all zero. `forward.wgsl` already reads this as "no
/// animation; fall back to static `intensity`/`color`/`cone_direction`."
/// This is the pre-reservation invariant — every map light owns an
/// `AnimationDescriptor` slot whether or not it has a live animation, so
/// `setAnimation` always overwrites in place and never allocates.
///
/// **`play_count` is stripped:** the GPU descriptor never learns about
/// completion bounds. The CPU-side `LightBridge::update` handles completion
/// by writing the final keyframe back as static `intensity`/`color` and
/// clearing `animation`. The GPU always sees a looping descriptor or a
/// sentinel.
///
/// Sample payloads (brightness/color samples themselves) are *not* packed
/// here — they live in a separate `anim_samples` storage buffer addressed
/// by per-descriptor offsets. Until the renderer-side plumbing for
/// per-map-light sample buffers lands, the descriptor's sample-offset
/// fields reference slot zero; brightness_count/color_count > 0 is
/// sufficient to test the bridge end-to-end.
fn pack_animation_descriptor(component: &LightComponent) -> [u8; ANIMATION_DESCRIPTOR_SIZE] {
    let mut bytes = [0u8; ANIMATION_DESCRIPTOR_SIZE];
    let Some(anim) = &component.animation else {
        // Sentinel: all zeros. `active` stays 0 which reads as inactive on
        // the compose-pass side; the forward-pass spot/point loop keys on
        // `brightness_count == 0 && color_count == 0` and uses the static
        // fields, so a zero `active` flag does not suppress the light.
        return bytes;
    };

    // period is in seconds on the GPU; the script/animation side tracks ms.
    let period_s = anim.period_ms / 1000.0;
    bytes[0..4].copy_from_slice(&period_s.to_le_bytes());
    let phase = anim.phase.unwrap_or(0.0).rem_euclid(1.0);
    bytes[4..8].copy_from_slice(&phase.to_le_bytes());

    let brightness_offset: u32 = 0;
    let brightness_count: u32 = anim.brightness.as_ref().map_or(0, |v| v.len() as u32);
    bytes[8..12].copy_from_slice(&brightness_offset.to_le_bytes());
    bytes[12..16].copy_from_slice(&brightness_count.to_le_bytes());

    bytes[16..20].copy_from_slice(&component.color[0].to_le_bytes());
    bytes[20..24].copy_from_slice(&component.color[1].to_le_bytes());
    bytes[24..28].copy_from_slice(&component.color[2].to_le_bytes());

    let color_offset: u32 = 0;
    let color_count: u32 = anim.color.as_ref().map_or(0, |v| v.len() as u32);
    bytes[28..32].copy_from_slice(&color_offset.to_le_bytes());
    bytes[32..36].copy_from_slice(&color_count.to_le_bytes());

    // `active` — 1 while an animation is live. Sentinel descriptor above
    // keeps this 0.
    let active: u32 = 1;
    bytes[36..40].copy_from_slice(&active.to_le_bytes());

    // bytes[40..48] reserved for the direction channel (Sub-plan 1).
    bytes
}

/// If `current` carries a `play_count`-bounded animation whose elapsed time
/// meets or exceeds the completion bound, sample the final keyframe values
/// and return the settled static `LightComponent`. Otherwise `None`.
///
/// Decoupled from mutation so `update`'s diff pass can walk the registry
/// behind a shared borrow.
fn check_play_count_completion(
    current: &LightComponent,
    snapshot: Option<&LightSnapshot>,
    current_time: f32,
) -> Option<LightComponent> {
    let anim = current.animation.as_ref()?;
    let play_count = anim.play_count?;
    // play_count == 0 is nonsensical; treat as "never completes" so scripts
    // that accidentally build one don't lock up the bridge.
    if play_count == 0 || anim.period_ms <= 0.0 {
        return None;
    }
    let start = snapshot.and_then(|s| s.animation_start_time)?;
    let total_duration_s = (play_count as f32) * anim.period_ms / 1000.0;
    if current_time - start < total_duration_s {
        return None;
    }

    // Completed: sample the last keyframe of each channel; those are the
    // static values the light settles to.
    let mut settled = current.clone();
    if let Some(brightness) = &anim.brightness
        && let Some(&final_brightness) = brightness.last()
    {
        settled.intensity = final_brightness;
    }
    if let Some(color) = &anim.color
        && let Some(&final_color) = color.last()
    {
        settled.color = final_color;
    }
    if let Some(direction) = &anim.direction
        && let Some(&final_direction) = direction.last()
    {
        settled.cone_direction = Some(final_direction);
    }
    settled.animation = None;
    Some(settled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prl::{FalloffModel, LightType};

    fn sample_point_light() -> MapLight {
        MapLight {
            origin: [1.0, 2.0, 3.0],
            light_type: LightType::Point,
            intensity: 1.5,
            color: [1.0, 0.8, 0.6],
            falloff_model: FalloffModel::InverseSquared,
            falloff_range: 10.0,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0, 0.0, 0.0],
            cast_shadows: false,
            is_dynamic: false,
        }
    }

    fn sample_spot_light() -> MapLight {
        MapLight {
            origin: [-5.0, 4.0, 2.0],
            light_type: LightType::Spot,
            intensity: 2.0,
            color: [0.5, 0.5, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 25.0,
            cone_angle_inner: 0.4,
            cone_angle_outer: 0.8,
            cone_direction: [0.0, -1.0, 0.0],
            cast_shadows: true,
            is_dynamic: true,
        }
    }

    #[test]
    fn populate_from_level_spawns_one_entity_per_map_light_and_copies_fields() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        let lights = vec![sample_point_light(), sample_spot_light()];

        bridge.populate_from_level(&lights, &mut registry);

        assert_eq!(bridge.light_count(), 2);
        let spot_id = bridge.entity_for_map_index(1).unwrap();
        let spot_component = registry.get_component::<LightComponent>(spot_id).unwrap();
        assert_eq!(spot_component.light_type, LightKind::Spot);
        assert_eq!(spot_component.intensity, 2.0);
        assert_eq!(spot_component.cone_angle_inner, Some(0.4));
        assert_eq!(spot_component.cone_direction, Some([0.0, -1.0, 0.0]));
        // f64 origin was cast to f32 at the bridge boundary.
        assert_eq!(spot_component.origin, [-5.0, 4.0, 2.0]);
    }

    #[test]
    fn first_update_after_populate_returns_initial_upload_bytes() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        let lights = vec![sample_point_light()];
        bridge.populate_from_level(&lights, &mut registry);

        let update = bridge.update(&mut registry, 0.0).expect("initial dirty");
        assert_eq!(update.lights_bytes.len(), GPU_LIGHT_SIZE);
        assert_eq!(update.descriptor_bytes.len(), ANIMATION_DESCRIPTOR_SIZE);
    }

    #[test]
    fn update_returns_none_when_no_component_changed_since_last_call() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry);
        // Flush initial upload.
        let _ = bridge.update(&mut registry, 0.0);

        let update = bridge.update(&mut registry, 0.016);
        assert!(update.is_none(), "idle frame must not re-upload");
    }

    #[test]
    fn mutating_intensity_in_registry_produces_repacked_upload_within_one_frame() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry);
        let _ = bridge.update(&mut registry, 0.0); // flush initial

        // Script-side mutation: read current, bump intensity, write back.
        let id = bridge.entity_for_map_index(0).unwrap();
        let mut component = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        component.intensity = 7.5;
        registry.set_component(id, component).unwrap();

        let update = bridge
            .update(&mut registry, 0.016)
            .expect("dirty after mutation");
        // Intensity × color pre-multiplies into bytes 16..28 of the packed
        // GpuLight record. Sampled first channel must reflect the new value.
        let packed_r = f32::from_le_bytes(update.lights_bytes[16..20].try_into().unwrap());
        assert!(
            (packed_r - 7.5 * 1.0).abs() < 1e-5,
            "packed color.r should be intensity × color.r = 7.5; got {packed_r}"
        );
    }

    #[test]
    fn setting_animation_then_clearing_produces_sentinel_descriptor() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry);
        let _ = bridge.update(&mut registry, 0.0);

        let id = bridge.entity_for_map_index(0).unwrap();
        let mut component = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        component.animation = Some(LightAnimation {
            period_ms: 1000.0,
            phase: Some(0.0),
            play_count: None,
            brightness: Some(vec![0.1, 1.0, 0.1]),
            color: None,
            direction: None,
        });
        registry.set_component(id, component).unwrap();

        let update = bridge.update(&mut registry, 0.0).expect("dirty");
        let brightness_count =
            u32::from_le_bytes(update.descriptor_bytes[12..16].try_into().unwrap());
        assert_eq!(brightness_count, 3);
        let active = u32::from_le_bytes(update.descriptor_bytes[36..40].try_into().unwrap());
        assert_eq!(active, 1);

        // Clear animation → sentinel.
        let mut component = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        component.animation = None;
        registry.set_component(id, component).unwrap();

        let update = bridge
            .update(&mut registry, 0.1)
            .expect("dirty after clear");
        let brightness_count =
            u32::from_le_bytes(update.descriptor_bytes[12..16].try_into().unwrap());
        let color_count = u32::from_le_bytes(update.descriptor_bytes[32..36].try_into().unwrap());
        let active = u32::from_le_bytes(update.descriptor_bytes[36..40].try_into().unwrap());
        assert_eq!(brightness_count, 0);
        assert_eq!(color_count, 0);
        assert_eq!(active, 0, "sentinel descriptor must be inactive");
    }

    #[test]
    fn play_count_completion_writes_final_keyframe_back_as_static_state() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry);
        let _ = bridge.update(&mut registry, 0.0);

        let id = bridge.entity_for_map_index(0).unwrap();
        let mut component = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        // 2-period bounded animation; final brightness = 0.25, final color = blue.
        component.animation = Some(LightAnimation {
            period_ms: 500.0,
            phase: None,
            play_count: Some(2),
            brightness: Some(vec![1.0, 0.5, 0.25]),
            color: Some(vec![[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]),
            direction: None,
        });
        registry.set_component(id, component).unwrap();

        // Set the animation at t=1.0. Completion bound = 2 × 0.5s = 1.0s
        // (absolute completion at t=2.0).
        let _ = bridge.update(&mut registry, 1.0);

        // Not yet complete at t=1.5.
        let _ = bridge.update(&mut registry, 1.5);
        let mid = registry.get_component::<LightComponent>(id).unwrap();
        assert!(
            mid.animation.is_some(),
            "animation still live before completion bound"
        );

        // At t=2.01 the 2×500ms bound is crossed. Bridge must settle values.
        let _ = bridge.update(&mut registry, 2.01);
        let settled = registry.get_component::<LightComponent>(id).unwrap();
        assert!(
            settled.animation.is_none(),
            "animation cleared on completion"
        );
        assert!(
            (settled.intensity - 0.25).abs() < 1e-6,
            "intensity settled to final brightness keyframe; got {}",
            settled.intensity
        );
        assert_eq!(settled.color, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn setanimation_restart_resets_play_count_clock() {
        // "Last call wins" — a second setAnimation restarts the completion
        // clock from the current frame time.
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry);
        let _ = bridge.update(&mut registry, 0.0);
        let id = bridge.entity_for_map_index(0).unwrap();

        let make_anim = || LightAnimation {
            period_ms: 500.0,
            phase: None,
            play_count: Some(2),
            brightness: Some(vec![1.0, 0.25]),
            color: None,
            direction: None,
        };

        // Write at t=0.0.
        let mut comp = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        comp.animation = Some(make_anim());
        registry.set_component(id, comp).unwrap();
        let _ = bridge.update(&mut registry, 0.0);

        // At t=0.9 — past completion bound (1.0s) would normally fire — but
        // first re-write at t=0.6 resets the clock. Completion now at t=1.6.
        let mut comp = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        // Mutate to trigger the restart path (different animation instance).
        let mut anim = make_anim();
        anim.phase = Some(0.5);
        comp.animation = Some(anim);
        registry.set_component(id, comp).unwrap();
        let _ = bridge.update(&mut registry, 0.6);

        // At t=1.1 — would have triggered completion with the first clock
        // (started at 0.0) but should NOT with the restarted clock (started
        // at 0.6, completion at 1.6).
        let _ = bridge.update(&mut registry, 1.1);
        assert!(
            registry
                .get_component::<LightComponent>(id)
                .unwrap()
                .animation
                .is_some(),
            "restart must reset completion clock; animation should still be live at t=1.1"
        );

        // At t=1.7 — past the restarted clock's completion bound.
        let _ = bridge.update(&mut registry, 1.7);
        assert!(
            registry
                .get_component::<LightComponent>(id)
                .unwrap()
                .animation
                .is_none(),
            "animation settles once restarted completion bound is crossed"
        );
    }

    #[test]
    fn phase_outside_unit_interval_is_wrapped_via_rem_euclid_in_descriptor() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry);
        let _ = bridge.update(&mut registry, 0.0);
        let id = bridge.entity_for_map_index(0).unwrap();

        let mut comp = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        comp.animation = Some(LightAnimation {
            period_ms: 1000.0,
            phase: Some(2.75),
            play_count: None,
            brightness: Some(vec![0.1, 1.0]),
            color: None,
            direction: None,
        });
        registry.set_component(id, comp).unwrap();
        let update = bridge.update(&mut registry, 0.0).expect("dirty");
        let phase = f32::from_le_bytes(update.descriptor_bytes[4..8].try_into().unwrap());
        assert!(
            (phase - 0.75).abs() < 1e-5,
            "phase 2.75 should wrap to 0.75; got {phase}"
        );
    }

    #[test]
    fn idempotent_update_after_settled_component_does_not_re_trigger_completion() {
        // Regression guard: after play_count completion writes back, a
        // follow-up tick with an unchanged component must not re-enter the
        // completion branch (snapshot now carries `animation_start_time: None`
        // and `component.animation: None`).
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry);
        let _ = bridge.update(&mut registry, 0.0);
        let id = bridge.entity_for_map_index(0).unwrap();

        let mut comp = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        comp.animation = Some(LightAnimation {
            period_ms: 100.0,
            phase: None,
            play_count: Some(1),
            brightness: Some(vec![1.0, 0.0]),
            color: None,
            direction: None,
        });
        registry.set_component(id, comp).unwrap();
        let _ = bridge.update(&mut registry, 0.0);
        let _ = bridge.update(&mut registry, 0.2); // past completion

        // Now idle frames must return None.
        assert!(bridge.update(&mut registry, 0.3).is_none());
        assert!(bridge.update(&mut registry, 10.0).is_none());
    }
}
