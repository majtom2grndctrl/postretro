// Scripting ↔ renderer bridge for map lights: entity registry → GPU light buffer.
// See: context/lib/scripting.md

use std::collections::HashMap;

use crate::lighting::{GPU_LIGHT_SIZE, pack_light};
use crate::prl::{FalloffModel, LightType, MapLight};
use crate::render::sh_volume::{
    ANIMATION_DESCRIPTOR_SIZE, SCRIPTED_BRIGHTNESS_SLOT, SCRIPTED_COLOR_SLOT_F32,
    SCRIPTED_FLOATS_PER_LIGHT,
};

#[cfg(test)]
use crate::scripting::components::light::LightAnimation;
use crate::scripting::components::light::{FalloffKind, LightComponent, LightKind};
#[cfg(test)]
use crate::scripting::conv::Vec3Lit;
use crate::scripting::registry::{EntityId, EntityRegistry};

/// Snapshot of a map light's component state as last observed by the bridge.
/// Dirty detection compares the live registry component against this value.
///
/// `animation_start_time` is `Some(t)` while a `play_count`-bounded animation
/// is running, where `t` is the engine time when the animation was last written.
/// When `current_time − t` reaches `play_count × period_ms / 1000.0`, the bridge
/// samples the final keyframe, writes a static `LightComponent` back to the registry,
/// and clears this field. Any `setAnimation` call resets `animation_start_time` to the
/// current frame time — "last call wins" always restarts the count from zero.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LightSnapshot {
    pub(crate) component: LightComponent,
    pub(crate) animation_start_time: Option<f32>,
}

/// Payload handed back to the renderer after `update`.
///
/// GPU buffer fields (`lights_bytes`, `descriptor_bytes`, `samples_bytes`) are
/// only populated when `has_dirty_data` is true; callers skip `write_buffer`
/// otherwise. `effective_brightness` is always populated — it is time-varying
/// and must be re-evaluated every frame for correct shadow-slot ranking.
///
/// - `lights_bytes` — packed `GpuLight` records; always sized to the
///   authored-light count at level load.
/// - `descriptor_bytes` — one `AnimationDescriptor` per map light, same order
///   as `lights_bytes`. Lights without an animation get the sentinel descriptor
///   (all counts zero) so `forward.wgsl` falls back to the static path.
/// - `samples_bytes` — packed f32 samples for the scripted-animation region
///   of `anim_samples`. One `SCRIPTED_FLOATS_PER_LIGHT`-wide slot per map
///   light; written at `scripted_sample_byte_offset` by
///   `Renderer::upload_bridge_samples`.
#[derive(Debug)]
pub(crate) struct LightBridgeUpdate {
    pub(crate) has_dirty_data: bool,
    pub(crate) lights_bytes: Vec<u8>,
    pub(crate) descriptor_bytes: Vec<u8>,
    pub(crate) samples_bytes: Vec<u8>,
    /// One f32 per map light (map-light-index order). Always evaluated at the
    /// current frame time regardless of dirty state — shadow-slot suppression
    /// must track the live animation curve every frame. Static lights and
    /// color-only animations report `1.0`; `start_active: Some(false)` reports `0.0`.
    pub(crate) effective_brightness: Vec<f32>,
}

/// State carried across frames. Owned by the game layer so the renderer never
/// holds component data.
pub(crate) struct LightBridge {
    /// Map-light index → `EntityId`. Fixed at level load; never grows or shrinks.
    entity_ids: Vec<EntityId>,
    /// Dirty-tracking snapshots. `None` for an entry means the slot has never
    /// been snapshotted — treated as unconditionally dirty on first visit so
    /// the initial upload lands.
    snapshots: HashMap<EntityId, LightSnapshot>,
    /// Shape metadata needed to re-pack. Parallels `entity_ids`.
    shape: Vec<MapLightShape>,
    dirty: bool,
    /// f64 origins from level load. Preserved so round-tripping through the f32
    /// `LightComponent` doesn't drop precision on non-moving lights.
    cached_origins_f64: Vec<[f64; 3]>,
    /// Float index into `anim_samples` where the scripted region starts
    /// (= FGD sample float count). Used to compute per-light absolute offsets.
    fgd_sample_float_count: u32,
    /// CPU mirror of the scripted-animation region in `anim_samples`.
    /// Sized to `entity_ids.len() * SCRIPTED_FLOATS_PER_LIGHT`.
    scripted_sample_buf: Vec<f32>,
}

/// Per-light fields not carried by `LightComponent` (runtime-only). Kept so
/// the bridge can rebuild a `MapLight` without the renderer re-supplying the
/// original list each frame.
#[derive(Debug, Clone)]
struct MapLightShape {
    is_dynamic: bool,
    leaf_index: u32,
}

impl LightBridge {
    pub(crate) fn new() -> Self {
        Self {
            entity_ids: Vec::new(),
            snapshots: HashMap::new(),
            shape: Vec::new(),
            dirty: false,
            cached_origins_f64: Vec::new(),
            fgd_sample_float_count: 0,
            scripted_sample_buf: Vec::new(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn light_count(&self) -> usize {
        self.entity_ids.len()
    }

    #[allow(dead_code)]
    pub(crate) fn entity_for_map_index(&self, map_index: usize) -> Option<EntityId> {
        self.entity_ids.get(map_index).copied()
    }

    /// Populate the entity registry with one entity per map light. Called once at level load.
    ///
    /// f64 → f32 origin conversion happens here — the only seam that touches
    /// both precisions. The f64 source is cached; script-facing
    /// `LightComponent.origin` is f32.
    pub(crate) fn populate_from_level(
        &mut self,
        lights: &[MapLight],
        registry: &mut EntityRegistry,
        fgd_sample_float_count: u32,
    ) {
        self.entity_ids.clear();
        self.snapshots.clear();
        self.shape.clear();
        self.cached_origins_f64.clear();
        self.entity_ids.reserve(lights.len());
        self.shape.reserve(lights.len());
        self.cached_origins_f64.reserve(lights.len());
        self.fgd_sample_float_count = fgd_sample_float_count;
        self.scripted_sample_buf = vec![0.0f32; lights.len() * SCRIPTED_FLOATS_PER_LIGHT];

        for light in lights {
            let component = map_light_to_component(light);
            let Some(id) = registry.try_spawn(Default::default()) else {
                log::warn!(
                    "[LightBridge] entity registry exhausted; dropping map light (index {}). \
                     Further map lights in this level will not appear in the scripting surface.",
                    self.entity_ids.len()
                );
                break;
            };
            let _ = registry.set_component(id, component);
            if !light.tags.is_empty() {
                let _ = registry.set_tags(id, light.tags.clone());
            }
            self.entity_ids.push(id);
            self.shape.push(MapLightShape {
                is_dynamic: light.is_dynamic,
                leaf_index: light.leaf_index,
            });
            self.cached_origins_f64.push(light.origin);
        }

        // Ensure the initial pack lands even when no script mutates on frame one.
        self.dirty = true;
    }

    /// Detect mutations, settle completed `play_count`-bounded animations, and
    /// return repacked buffers when anything changed.
    ///
    /// `current_time` is seconds since level load. Only consulted for
    /// `play_count`-bounded animations.
    pub(crate) fn update(
        &mut self,
        registry: &mut EntityRegistry,
        current_time: f32,
    ) -> Option<LightBridgeUpdate> {
        if self.entity_ids.is_empty() {
            return None;
        }

        // Walk `entity_ids` (fixed at level load) rather than the registry's
        // full iterator — stable slots give index correspondence for re-packing.
        // Settled animations are collected and written back after the loop to
        // avoid aliasing the registry borrow.
        let mut settled: Vec<(EntityId, LightComponent)> = Vec::new();
        for &id in &self.entity_ids {
            let Ok(current) = registry.get_component::<LightComponent>(id) else {
                continue;
            };
            let snapshot = self.snapshots.get(&id);

            let changed = match snapshot {
                Some(snap) => snap.component != *current,
                None => true,
            };

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
                    // Record start time so completion can fire on a future frame.
                    // Any mutation resets the clock ("last call wins").
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

        // Commit settled components so a subsequent `world.query` observes
        // post-animation static state.
        for (id, settled_component) in settled {
            // Stale-id error means the entity was despawned between read and write; ignore.
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

        // Compute before the dirty guard — effective_brightness is time-varying.
        // The GPU evaluates animation curves continuously; the CPU suppression
        // check must track the same curve every frame so shadow slots are gained
        // and lost promptly. Previously frozen at the dirty frame, which locked
        // shadow slot assignment to the state at levelLoad animation time.
        let effective_brightness: Vec<f32> = self
            .entity_ids
            .iter()
            .map(|&id| {
                let Ok(component) = registry.get_component::<LightComponent>(id) else {
                    return 0.0;
                };
                eval_effective_brightness(component, current_time)
            })
            .collect();

        if !self.dirty {
            return Some(LightBridgeUpdate {
                has_dirty_data: false,
                lights_bytes: Vec::new(),
                descriptor_bytes: Vec::new(),
                samples_bytes: Vec::new(),
                effective_brightness,
            });
        }
        self.dirty = false;

        let mut lights_bytes: Vec<u8> = Vec::with_capacity(self.entity_ids.len() * GPU_LIGHT_SIZE);
        let mut descriptor_bytes: Vec<u8> =
            Vec::with_capacity(self.entity_ids.len() * ANIMATION_DESCRIPTOR_SIZE);

        self.scripted_sample_buf.fill(0.0);

        for (map_idx, &id) in self.entity_ids.iter().enumerate() {
            let Ok(component) = registry.get_component::<LightComponent>(id) else {
                // Entity was despawned — push a zeroed slot to keep downstream
                // offsets aligned. The renderer's `light_count` bound keeps
                // the slot off the shading path.
                lights_bytes.extend_from_slice(&[0u8; GPU_LIGHT_SIZE]);
                descriptor_bytes.extend_from_slice(&[0u8; ANIMATION_DESCRIPTOR_SIZE]);
                continue;
            };

            let map_light = component_to_map_light(
                component,
                self.cached_origins_f64[map_idx],
                self.shape[map_idx].is_dynamic,
                self.shape[map_idx].leaf_index,
            );
            lights_bytes.extend_from_slice(&pack_light(&map_light));

            let light_base =
                self.fgd_sample_float_count + (map_idx as u32) * (SCRIPTED_FLOATS_PER_LIGHT as u32);
            let brightness_offset = light_base;
            let color_offset = light_base + SCRIPTED_BRIGHTNESS_SLOT as u32;

            let slot_start = map_idx * SCRIPTED_FLOATS_PER_LIGHT;
            if let Some(anim) = &component.animation {
                if let Some(brightness) = &anim.brightness {
                    let count = brightness.len().min(SCRIPTED_BRIGHTNESS_SLOT);
                    self.scripted_sample_buf[slot_start..slot_start + count]
                        .copy_from_slice(&brightness[..count]);
                }
                if let Some(color_samples) = &anim.color {
                    let max_color = SCRIPTED_COLOR_SLOT_F32 / 3;
                    let count = color_samples.len().min(max_color);
                    let color_slot = slot_start + SCRIPTED_BRIGHTNESS_SLOT;
                    for (i, cv) in color_samples.iter().take(count).enumerate() {
                        let rgb = cv.as_f32_3();
                        self.scripted_sample_buf[color_slot + i * 3] = rgb[0];
                        self.scripted_sample_buf[color_slot + i * 3 + 1] = rgb[1];
                        self.scripted_sample_buf[color_slot + i * 3 + 2] = rgb[2];
                    }
                }
            }

            let desc = pack_animation_descriptor(component, brightness_offset, color_offset);
            descriptor_bytes.extend_from_slice(&desc);
        }

        // Native endian matches `f32_slice_to_bytes` in sh_volume.rs.
        let samples_bytes = self
            .scripted_sample_buf
            .iter()
            .flat_map(|&v| v.to_ne_bytes())
            .collect();

        Some(LightBridgeUpdate {
            has_dirty_data: true,
            lights_bytes,
            descriptor_bytes,
            samples_bytes,
            effective_brightness,
        })
    }
}

impl Default for LightBridge {
    fn default() -> Self {
        Self::new()
    }
}

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
    leaf_index: u32,
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
        // Preserve the cached f64 origin — round-tripping through the f32
        // component would drop precision for no reason.
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
        tags: vec![],
        leaf_index,
    }
}

/// CPU mirror of `sample_curve_catmull_rom` from `curve_eval.wgsl`.
/// Closed-loop uniform Catmull-Rom (tension 0.5) at normalized cycle position
/// `cycle_t` ∈ [0, 1).
///
/// Must stay numerically equivalent to the WGSL helper — drift between
/// CPU/GPU evaluation would let a light flicker into a shadow slot it
/// should not own.
fn sample_brightness_at(samples: &[f32], cycle_t: f32) -> f32 {
    let count = samples.len();
    if count == 0 {
        return 1.0;
    }
    if count == 1 {
        return samples[0];
    }
    let scaled = cycle_t * count as f32;
    let i1 = (scaled.floor() as usize) % count;
    let i0 = (i1 + count - 1) % count;
    let i2 = (i1 + 1) % count;
    let i3 = (i1 + 2) % count;
    let f = scaled.fract();
    let (p0, p1, p2, p3) = (samples[i0], samples[i1], samples[i2], samples[i3]);
    let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
    let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let c = -0.5 * p0 + 0.5 * p2;
    let d = p1;
    ((a * f + b) * f + c) * f + d
}

/// Current effective brightness for shadow-slot suppression. Mirrors GPU
/// animation evaluation; called every frame, not just on dirty frames.
fn eval_effective_brightness(component: &LightComponent, current_time: f32) -> f32 {
    match &component.animation {
        None => 1.0,
        Some(anim) => {
            if anim.start_active == Some(false) {
                0.0
            } else if let Some(brightness) = &anim.brightness
                && !brightness.is_empty()
            {
                let period_s = anim.period_ms / 1000.0;
                if period_s > 0.0 {
                    let phase = anim.phase.unwrap_or(0.0);
                    let cycle_t = (current_time / period_s + phase).rem_euclid(1.0);
                    sample_brightness_at(brightness, cycle_t)
                } else {
                    brightness[0]
                }
            } else {
                1.0
            }
        }
    }
}

/// Pack one `LightComponent`'s animation state into a 48-byte
/// `AnimationDescriptor` matching the WGSL layout in `sh_volume.rs`.
///
/// **Sentinel:** `animation == None` produces an all-zero record. `forward.wgsl`
/// reads zero counts as "no animation; use static fields." Every map light
/// owns a descriptor slot regardless of whether it has a live animation, so
/// `setAnimation` always overwrites in place.
///
/// **`play_count` is stripped:** the GPU never sees completion bounds. The
/// CPU-side bridge handles completion by writing the final keyframe back as
/// static `intensity`/`color` and clearing `animation`. The GPU always sees
/// a looping descriptor or a sentinel.
///
/// Sample payloads live in a separate `anim_samples` storage buffer addressed
/// by per-descriptor offsets.
fn pack_animation_descriptor(
    component: &LightComponent,
    brightness_offset: u32,
    color_offset: u32,
) -> [u8; ANIMATION_DESCRIPTOR_SIZE] {
    let mut bytes = [0u8; ANIMATION_DESCRIPTOR_SIZE];
    let Some(anim) = &component.animation else {
        // Sentinel: all zeros. Forward-pass keys on zero counts; zero `active`
        // does not suppress the light on that path.
        return bytes;
    };

    // GPU uses seconds; script-side tracks ms.
    let period_s = anim.period_ms / 1000.0;
    bytes[0..4].copy_from_slice(&period_s.to_ne_bytes());
    let phase = anim.phase.unwrap_or(0.0).rem_euclid(1.0);
    bytes[4..8].copy_from_slice(&phase.to_ne_bytes());

    let brightness_count: u32 = anim
        .brightness
        .as_ref()
        .map_or(0, |v| v.len().min(SCRIPTED_BRIGHTNESS_SLOT) as u32);
    bytes[8..12].copy_from_slice(&brightness_offset.to_ne_bytes());
    bytes[12..16].copy_from_slice(&brightness_count.to_ne_bytes());

    bytes[16..20].copy_from_slice(&component.color[0].to_ne_bytes());
    bytes[20..24].copy_from_slice(&component.color[1].to_ne_bytes());
    bytes[24..28].copy_from_slice(&component.color[2].to_ne_bytes());

    let color_count: u32 = anim
        .color
        .as_ref()
        .map_or(0, |v| v.len().min(SCRIPTED_COLOR_SLOT_F32 / 3) as u32);
    bytes[28..32].copy_from_slice(&color_offset.to_ne_bytes());
    bytes[32..36].copy_from_slice(&color_count.to_ne_bytes());

    // `None` defaults to active; `Some(false)` opts the light out at spawn.
    let active: u32 = u32::from(anim.start_active.unwrap_or(true));
    bytes[36..40].copy_from_slice(&active.to_ne_bytes());

    // bytes[40..48] reserved for the direction channel.
    bytes
}

/// If `current` carries a `play_count`-bounded animation that has elapsed,
/// sample the final keyframe and return the settled static `LightComponent`.
/// Decoupled from mutation so `update`'s diff pass can hold a shared borrow.
fn check_play_count_completion(
    current: &LightComponent,
    snapshot: Option<&LightSnapshot>,
    current_time: f32,
) -> Option<LightComponent> {
    let anim = current.animation.as_ref()?;
    let play_count = anim.play_count?;
    // play_count == 0 is nonsensical; treat as "never completes".
    if play_count == 0 || anim.period_ms <= 0.0 {
        return None;
    }
    let start = snapshot.and_then(|s| s.animation_start_time)?;
    let total_duration_s = (play_count as f32) * anim.period_ms / 1000.0;
    if current_time - start < total_duration_s {
        return None;
    }

    let mut settled = current.clone();
    if let Some(brightness) = &anim.brightness
        && let Some(&final_brightness) = brightness.last()
    {
        settled.intensity = final_brightness;
    }
    if let Some(color) = &anim.color
        && let Some(final_color) = color.last()
    {
        settled.color = final_color.as_f32_3();
    }
    if let Some(direction) = &anim.direction
        && let Some(final_direction) = direction.last()
    {
        settled.cone_direction = Some(final_direction.as_f32_3());
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
            tags: vec![],
            leaf_index: 0,
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
            tags: vec![],
            leaf_index: 0,
        }
    }

    #[test]
    fn populate_from_level_sets_tag_on_registry_entity() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        let mut tagged = sample_point_light();
        tagged.tags = vec!["hallway_wave".to_string()];
        let untagged = sample_spot_light();
        bridge.populate_from_level(&[tagged, untagged], &mut registry, 0);

        let tagged_id = bridge.entity_for_map_index(0).unwrap();
        let untagged_id = bridge.entity_for_map_index(1).unwrap();
        assert_eq!(registry.get_tags(tagged_id).unwrap(), &["hallway_wave"]);
        assert!(registry.get_tags(untagged_id).unwrap().is_empty());
    }

    #[test]
    fn populate_from_level_spawns_one_entity_per_map_light_and_copies_fields() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        let lights = vec![sample_point_light(), sample_spot_light()];

        bridge.populate_from_level(&lights, &mut registry, 0);

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
        bridge.populate_from_level(&lights, &mut registry, 0);

        let update = bridge.update(&mut registry, 0.0).expect("initial dirty");
        assert!(
            update.has_dirty_data,
            "first update must have dirty GPU data"
        );
        assert_eq!(update.lights_bytes.len(), GPU_LIGHT_SIZE);
        assert_eq!(update.descriptor_bytes.len(), ANIMATION_DESCRIPTOR_SIZE);
    }

    #[test]
    fn update_skips_buffer_reupload_when_no_component_changed_since_last_call() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
        // Flush initial upload.
        let _ = bridge.update(&mut registry, 0.0);

        let update = bridge
            .update(&mut registry, 0.016)
            .expect("update always returns Some when lights are present");
        assert!(
            !update.has_dirty_data,
            "idle frame must not re-upload GPU buffers"
        );
        assert_eq!(
            update.lights_bytes.len(),
            0,
            "lights_bytes empty when not dirty"
        );
    }

    #[test]
    fn mutating_intensity_in_registry_produces_repacked_upload_within_one_frame() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0); // flush initial

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
        assert!(
            update.has_dirty_data,
            "mutation must trigger GPU buffer repack"
        );
        // Intensity × color pre-multiplies into bytes 16..28 of the GpuLight record.
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
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
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
            start_active: None,
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
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0);

        let id = bridge.entity_for_map_index(0).unwrap();
        let mut component = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        component.animation = Some(LightAnimation {
            period_ms: 500.0,
            phase: None,
            play_count: Some(2),
            start_active: None,
            brightness: Some(vec![1.0, 0.5, 0.25]),
            color: Some(vec![Vec3Lit([1.0, 0.0, 0.0]), Vec3Lit([0.0, 0.0, 1.0])]),
            direction: None,
        });
        registry.set_component(id, component).unwrap();

        // Animate starts at t=1.0; completion bound = 2 × 0.5s, fires at t=2.0.
        let _ = bridge.update(&mut registry, 1.0);

        let _ = bridge.update(&mut registry, 1.5);
        let mid = registry.get_component::<LightComponent>(id).unwrap();
        assert!(
            mid.animation.is_some(),
            "animation still live before completion bound"
        );

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
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0);
        let id = bridge.entity_for_map_index(0).unwrap();

        let make_anim = || LightAnimation {
            period_ms: 500.0,
            phase: None,
            play_count: Some(2),
            start_active: None,
            brightness: Some(vec![1.0, 0.25]),
            color: None,
            direction: None,
        };

        let mut comp = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        comp.animation = Some(make_anim());
        registry.set_component(id, comp).unwrap();
        let _ = bridge.update(&mut registry, 0.0);

        // Re-write at t=0.6 resets the clock; completion now at t=1.6.
        // Phase change makes this a distinct animation value so the bridge detects a mutation.
        let mut comp = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        let mut anim = make_anim();
        anim.phase = Some(0.5);
        comp.animation = Some(anim);
        registry.set_component(id, comp).unwrap();
        let _ = bridge.update(&mut registry, 0.6);

        // t=1.1 would fire with the original clock (started at 0.0) but not
        // with the restarted clock (started at 0.6, completion at 1.6).
        let _ = bridge.update(&mut registry, 1.1);
        assert!(
            registry
                .get_component::<LightComponent>(id)
                .unwrap()
                .animation
                .is_some(),
            "restart must reset completion clock; animation should still be live at t=1.1"
        );

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
    fn pack_animation_descriptor_honors_start_active_false() {
        // `active` lives at bytes 36..40. `None`/`Some(true)` → 1; `Some(false)` → 0.
        let component = LightComponent {
            origin: [0.0, 0.0, 0.0],
            light_type: LightKind::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffKind::InverseSquared,
            falloff_range: 10.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            cast_shadows: false,
            is_dynamic: true,
            animation: Some(LightAnimation {
                period_ms: 500.0,
                phase: None,
                play_count: None,
                start_active: Some(false),
                brightness: Some(vec![0.1, 1.0]),
                color: None,
                direction: None,
            }),
        };
        let bytes = pack_animation_descriptor(&component, 0, SCRIPTED_BRIGHTNESS_SLOT as u32);
        let active = u32::from_ne_bytes(bytes[36..40].try_into().unwrap());
        assert_eq!(active, 0, "start_active: Some(false) must pack as inactive");
    }

    #[test]
    fn phase_outside_unit_interval_is_wrapped_via_rem_euclid_in_descriptor() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
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
            start_active: None,
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
        // Regression guard: after completion writes back, the snapshot carries
        // `animation_start_time: None` and `animation: None`, so subsequent
        // ticks must not re-enter the completion branch.
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
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
            start_active: None,
            brightness: Some(vec![1.0, 0.0]),
            color: None,
            direction: None,
        });
        registry.set_component(id, comp).unwrap();
        let _ = bridge.update(&mut registry, 0.0);
        let _ = bridge.update(&mut registry, 0.2); // past completion
        let idle1 = bridge.update(&mut registry, 0.3).unwrap();
        assert!(
            !idle1.has_dirty_data,
            "settled idle frame must not re-upload"
        );
        let idle2 = bridge.update(&mut registry, 10.0).unwrap();
        assert!(
            !idle2.has_dirty_data,
            "subsequent idle frame must not re-upload"
        );
    }

    #[test]
    fn effective_brightness_tracks_animation_curve_on_idle_frames() {
        // Regression: effective_brightness was frozen at the levelLoad dirty
        // frame. Lights dark at that instant were permanently suppressed; bright
        // lights held shadow slots regardless of their actual state.
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_spot_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0);

        let id = bridge.entity_for_map_index(0).unwrap();
        let mut comp = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        comp.animation = Some(LightAnimation {
            period_ms: 1000.0,
            phase: Some(0.0),
            play_count: None,
            start_active: None,
            brightness: Some(vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            color: None,
            direction: None,
        });
        registry.set_component(id, comp).unwrap();
        let _ = bridge.update(&mut registry, 0.0); // flush dirty frame

        let dark = bridge.update(&mut registry, 0.5).unwrap();
        assert!(
            !dark.has_dirty_data,
            "no mutation, GPU buffers must not re-upload"
        );
        assert!(
            dark.effective_brightness[0] < 0.01,
            "light is dark at T=0.5s; effective_brightness must reflect live curve; got {}",
            dark.effective_brightness[0]
        );

        let bright = bridge.update(&mut registry, 1.0).unwrap();
        assert!(!bright.has_dirty_data);
        assert!(
            bright.effective_brightness[0] > 0.5,
            "light is bright at T=1.0s (cycle wrap); got {}",
            bright.effective_brightness[0]
        );
    }
}
