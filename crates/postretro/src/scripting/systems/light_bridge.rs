// Scripting ↔ renderer bridge for map lights: entity registry → GPU light buffer.
// See: context/lib/scripting.md

use std::collections::HashMap;

use postretro_level_loader::{FalloffModel, LightType, MapLight, ShadowType};
use postretro_lighting::{GPU_LIGHT_SIZE, pack_light};
use postretro_render_cpu::sh_volume::{
    ANIMATION_DESCRIPTOR_SIZE, SCRIPTED_BRIGHTNESS_SLOT, SCRIPTED_COLOR_SLOT_F32,
    SCRIPTED_FLOATS_PER_LIGHT,
};
use postretro_render_data::influence::LightInfluence;
use postretro_renderer::RUNTIME_DYNAMIC_LIGHT_RESERVE;

#[cfg(test)]
use postretro_entities::components::light::LightAnimation;
use postretro_entities::components::light::{FalloffKind, LightComponent, LightKind};
use postretro_entities::registry::{ComponentKind, EntityId, EntityRegistry};
#[cfg(test)]
use postretro_scripting_core::conv::Vec3Lit;

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
/// - `lights_bytes` — packed `GpuLight` records for dynamic lights only, in
///   their stable filtered-authored order. Baked lights never enter the direct
///   forward buffer.
/// - `descriptor_bytes` — one `AnimationDescriptor` per dynamic light, same
///   order as `lights_bytes`. Lights without an animation get the sentinel
///   descriptor (all counts zero) so `forward.wgsl` falls back to the static path.
/// - `samples_bytes` — packed f32 samples for the scripted-animation region
///   of `anim_samples`. The map-authored prefix uses full authored order, one
///   `SCRIPTED_FLOATS_PER_LIGHT`-wide slot per light; written at
///   `scripted_sample_byte_offset` by `Renderer::upload_bridge_samples`.
/// - `influence_bytes` — one packed influence sphere per dynamic light, same
///   compact order as `lights_bytes`.
#[derive(Debug)]
pub(crate) struct LightBridgeUpdate {
    pub(crate) has_dirty_data: bool,
    pub(crate) lights_bytes: Vec<u8>,
    pub(crate) descriptor_bytes: Vec<u8>,
    pub(crate) influence_bytes: Vec<u8>,
    pub(crate) samples_bytes: Vec<u8>,
    /// One f32 per dynamic light (stable filtered-authored order). Always
    /// evaluated at the current frame time regardless of dirty state —
    /// shadow-slot suppression must track the live animation curve every frame.
    /// Color-only animations report `1.0`; `start_active: Some(false)` reports `0.0`.
    pub(crate) effective_brightness: Vec<f32>,
    /// Compose-side descriptor writes for `_animated` (and other slot-bearing)
    /// lights. Each entry is `(animated_slot, 48-byte ANIMATION_DESCRIPTOR
    /// bytes)` — the renderer overwrites the compose descriptor buffer at the
    /// slot. Populated only when the bridge is dirty AND the affected light
    /// has a cached `animated_slot`. Empty otherwise.
    pub(crate) compose_descriptor_writes: Vec<(u32, [u8; ANIMATION_DESCRIPTOR_SIZE])>,
}

/// State carried across frames. Owned by the game layer so the renderer never
/// holds component data.
pub(crate) struct LightBridge {
    /// Authored map-light prefix followed by runtime-spawned dynamic lights.
    /// Authored indices stay stable; runtime entries append until the renderer's
    /// reserved capacity is full. Despawned entries remain as tombstone slots.
    entity_ids: Vec<EntityId>,
    authored_light_count: usize,
    /// Dirty-tracking snapshots. `None` for an entry means the slot has never
    /// been snapshotted — treated as unconditionally dirty on first visit so
    /// the initial upload lands.
    snapshots: HashMap<EntityId, LightSnapshot>,
    /// Shape metadata needed to re-pack. Parallels `entity_ids`.
    shape: Vec<MapLightShape>,
    /// Static, slotless map lights for which the animation diagnostic has
    /// already been emitted. Map-light indices are stable for the level, so
    /// this avoids tying author-facing diagnostics to runtime `EntityId`s.
    warned_slotless_animation_indices: std::collections::HashSet<usize>,
    dirty: bool,
    /// f64 origins from level load. Preserved so round-tripping through the f32
    /// `LightComponent` doesn't drop precision on non-moving lights.
    cached_origins_f64: Vec<[f64; 3]>,
    /// Influence records parallel to `entity_ids`. Authored records preserve
    /// PRL data; runtime records derive from their spawn-time component.
    cached_influences: Vec<LightInfluence>,
    /// Float index into `anim_samples` where the scripted region starts
    /// (= FGD sample float count). Used to compute per-light absolute offsets.
    fgd_sample_float_count: u32,
    /// CPU mirror of the scripted-animation region in `anim_samples`. The
    /// map-authored prefix preserves full authored order.
    scripted_sample_buf: Vec<f32>,
    runtime_capacity_warned: bool,
}

/// Per-light fields not carried by `LightComponent` (runtime-only). Kept so
/// the bridge can rebuild a `MapLight` without the renderer re-supplying the
/// original list each frame.
#[derive(Debug, Clone)]
struct MapLightShape {
    is_dynamic: bool,
    cell_index: u32,
    /// Cached `MapLight.animated_slot` so the bridge can route
    /// `setLightAnimation` writes to the animated-compose descriptor buffer
    /// without re-querying the source.
    animated_slot: Option<u32>,
}

impl LightBridge {
    pub(crate) fn new() -> Self {
        Self {
            entity_ids: Vec::new(),
            authored_light_count: 0,
            snapshots: HashMap::new(),
            shape: Vec::new(),
            warned_slotless_animation_indices: std::collections::HashSet::new(),
            dirty: false,
            cached_origins_f64: Vec::new(),
            cached_influences: Vec::new(),
            fgd_sample_float_count: 0,
            scripted_sample_buf: Vec::new(),
            runtime_capacity_warned: false,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.entity_ids.clear();
        self.authored_light_count = 0;
        self.snapshots.clear();
        self.shape.clear();
        self.warned_slotless_animation_indices.clear();
        self.dirty = false;
        self.cached_origins_f64.clear();
        self.cached_influences.clear();
        self.fgd_sample_float_count = 0;
        self.scripted_sample_buf.clear();
        self.runtime_capacity_warned = false;
    }

    #[allow(dead_code)]
    pub(crate) fn light_count(&self) -> usize {
        self.entity_ids.len()
    }

    #[allow(dead_code)]
    pub(crate) fn entity_for_map_index(&self, map_index: usize) -> Option<EntityId> {
        self.entity_ids.get(map_index).copied()
    }

    /// Collect all tracked lights (FGD map-authored + descriptor-spawned dynamic)
    /// as `(MapLight, brightness_multiplier)` pairs, reading live component
    /// state from the registry.
    ///
    /// Used by `FogVolumeBridge::update_points` so script-spawned dynamic lights
    /// contribute alongside map-authored lights. The brightness multiplier is
    /// paired with each `MapLight` here so the two cannot drift out of alignment
    /// when component lookups fail (the previous parallel-`Vec<f32>` API silently
    /// shifted the multipliers by one slot per missing component).
    ///
    /// `current_time` matches `LightBridge::update`'s clock — seconds since
    /// level load — so animated brightness is sampled the same way the GPU
    /// path samples it.
    pub(crate) fn collect_all_as_map_lights(
        &self,
        registry: &EntityRegistry,
        current_time: f32,
    ) -> Vec<(MapLight, f32)> {
        self.entity_ids
            .iter()
            .enumerate()
            .filter_map(|(map_idx, &id)| {
                let component = registry
                    .get_component::<postretro_entities::components::light::LightComponent>(id)
                    .ok()?;
                let brightness = eval_effective_brightness(component, current_time);
                let map_light = component_to_map_light(
                    component,
                    self.cached_origins_f64[map_idx],
                    self.shape[map_idx].is_dynamic,
                    self.shape[map_idx].cell_index,
                );
                Some((map_light, brightness))
            })
            .collect()
    }

    /// Populate the entity registry with one entity per map light. Called once at level load.
    ///
    /// f64 → f32 origin conversion happens here — the only seam that touches
    /// both precisions. The f64 source is cached; script-facing
    /// `LightComponent.origin` is f32.
    #[cfg(test)]
    pub(crate) fn populate_from_level(
        &mut self,
        lights: &[MapLight],
        registry: &mut EntityRegistry,
        fgd_sample_float_count: u32,
    ) {
        self.populate_from_level_with_influences(lights, &[], registry, fgd_sample_float_count);
    }

    pub(crate) fn populate_from_level_with_influences(
        &mut self,
        lights: &[MapLight],
        light_influences: &[LightInfluence],
        registry: &mut EntityRegistry,
        fgd_sample_float_count: u32,
    ) {
        self.entity_ids.clear();
        self.authored_light_count = 0;
        self.snapshots.clear();
        self.shape.clear();
        self.warned_slotless_animation_indices.clear();
        self.cached_origins_f64.clear();
        self.cached_influences.clear();
        self.entity_ids.reserve(lights.len());
        self.shape.reserve(lights.len());
        self.cached_origins_f64.reserve(lights.len());
        self.cached_influences.reserve(lights.len());
        self.fgd_sample_float_count = fgd_sample_float_count;
        self.scripted_sample_buf = vec![0.0f32; lights.len() * SCRIPTED_FLOATS_PER_LIGHT];

        self.runtime_capacity_warned = false;

        for (map_idx, light) in lights.iter().enumerate() {
            let component = map_light_to_component(light);
            let Some(id) = registry.try_spawn(Default::default(), &[]) else {
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
                cell_index: light.cell_index,
                animated_slot: light.animated_slot,
            });
            self.cached_origins_f64.push(light.origin);
            self.cached_influences.push(
                light_influences
                    .get(map_idx)
                    .cloned()
                    .unwrap_or_else(uncullable_light_influence),
            );
        }
        self.authored_light_count = self.entity_ids.len();

        // Ensure the initial pack lands even when no script mutates on frame one.
        self.dirty = true;
    }

    /// Pick up `LightComponent` entities that were spawned outside of
    /// `populate_from_level` — typically by the data-archetype sweep, which
    /// runs after `App::resumed()` (where `populate_from_level` is called)
    /// during the first `RedrawRequested` once the data script has populated
    /// the entity-type registry. Also called every host fixed tick (`main.rs`)
    /// to enroll descriptor lights carried by runtime spawner-spawned enemies;
    /// that call is cheap on a no-op tick, since the scan below early-returns
    /// without allocating once nothing new appears.
    ///
    /// Any `LightComponent` entity not already tracked in `self.entity_ids` is
    /// appended to the bridge's parallel arrays so its component participates
    /// in the per-frame dirty/pack loop. Enrollment reads spawn-time origin and
    /// influence data but does not mutate the component. The next `update`
    /// produces the initial GPU upload for these new entries.
    ///
    /// Descriptor-spawned lights are always dynamic
    /// (`data_archetype.rs` forces `is_dynamic = true` regardless of source);
    /// they have no cell assignment yet, so `cell_index` is recorded as
    /// `u32::MAX` — the unassigned sentinel. Replace with a real cell index
    /// when runtime-spawned light cell assignment is implemented.
    /// The cached f64 origin mirrors the f32 component origin
    /// (descriptor-spawn is f32 from the start; there is no f64 source).
    pub(crate) fn absorb_dynamic_lights(&mut self, registry: &EntityRegistry) {
        // Called every fixed tick, but a runtime spawn that carries a descriptor
        // light is rare — most ticks find nothing new. Scan the Light column and
        // only touch the heap once an untracked id actually appears, so the
        // common no-op tick allocates nothing. Membership is a linear check
        // against the capacity-bounded `entity_ids`; materializing
        // a `HashSet`/`Vec` of the whole column up front would churn the heap
        // every tick just to discover there was nothing to absorb.
        let mut absorbed_any = false;
        for (id, _) in registry.iter_with_kind(ComponentKind::Light) {
            if self.entity_ids.contains(&id) {
                continue;
            }

            let runtime_count = self
                .entity_ids
                .len()
                .saturating_sub(self.authored_light_count);
            if runtime_count >= RUNTIME_DYNAMIC_LIGHT_RESERVE {
                if !self.runtime_capacity_warned {
                    self.runtime_capacity_warned = true;
                    log::warn!(
                        "[LightBridge] runtime dynamic-light reserve ({}) exhausted; \
                         additional spawned lights will not render. Further warnings suppressed.",
                        RUNTIME_DYNAMIC_LIGHT_RESERVE,
                    );
                }
                continue;
            }

            // Read the component to capture the spawn-time f32 origin so the
            // f64 cache matches what the bridge will hand back to the renderer
            // when it round-trips through `component_to_map_light`.
            let (origin_f64, influence) = match registry.get_component::<LightComponent>(id) {
                Ok(component) => (
                    [
                        component.origin[0] as f64,
                        component.origin[1] as f64,
                        component.origin[2] as f64,
                    ],
                    component_to_influence(component),
                ),
                Err(_) => continue,
            };

            self.entity_ids.push(id);
            self.shape.push(MapLightShape {
                is_dynamic: true,
                cell_index: u32::MAX,
                // Script-spawned dynamic lights have no baked slot; the
                // bridge routes them via the legacy forward path.
                animated_slot: None,
            });
            self.cached_origins_f64.push(origin_f64);
            self.cached_influences.push(influence);
            absorbed_any = true;
        }

        if !absorbed_any {
            return;
        }

        // Resize scripted-sample mirror to match the new entity count and
        // mark dirty so the next `update` rebuilds the GPU buffers including
        // the new entries.
        self.scripted_sample_buf
            .resize(self.entity_ids.len() * SCRIPTED_FLOATS_PER_LIGHT, 0.0);
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

        // Walk stable tracked slots rather than the registry's full iterator.
        // The authored prefix never moves; runtime lights only append.
        // Settled animations are collected and written back after the loop to
        // avoid aliasing the registry borrow.
        let mut settled: Vec<(EntityId, LightComponent)> = Vec::new();
        for (map_idx, &id) in self.entity_ids.iter().enumerate() {
            let Ok(current) = registry.get_component::<LightComponent>(id) else {
                // A tracked dynamic light that disappears must force one
                // tombstone upload; otherwise its last GPU record stays live.
                if self.snapshots.remove(&id).is_some() {
                    self.dirty = true;
                }
                continue;
            };

            let shape = &self.shape[map_idx];
            if !shape.is_dynamic
                && shape.animated_slot.is_none()
                && current.animation.is_some()
                && self.warned_slotless_animation_indices.insert(map_idx)
            {
                let tags = registry.get_tags(id).unwrap_or(&[]);
                log::warn!(
                    "[LightBridge] static map light {map_idx} (tags: {tags:?}) received an animation but has no animated compose slot; its baked contribution will not animate. Use script-derived membership or `_animated 1`."
                );
            }
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
            .zip(&self.shape)
            .filter(|(_, shape)| shape.is_dynamic)
            .map(|(&id, _)| {
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
                influence_bytes: Vec::new(),
                samples_bytes: Vec::new(),
                effective_brightness,
                compose_descriptor_writes: Vec::new(),
            });
        }
        self.dirty = false;

        let dynamic_light_count = self.shape.iter().filter(|shape| shape.is_dynamic).count();
        let mut lights_bytes: Vec<u8> = Vec::with_capacity(dynamic_light_count * GPU_LIGHT_SIZE);
        let mut descriptor_bytes: Vec<u8> =
            Vec::with_capacity(dynamic_light_count * ANIMATION_DESCRIPTOR_SIZE);
        let mut influences = Vec::with_capacity(dynamic_light_count);
        let mut compose_descriptor_writes: Vec<(u32, [u8; ANIMATION_DESCRIPTOR_SIZE])> = Vec::new();

        self.scripted_sample_buf.fill(0.0);

        for (map_idx, &id) in self.entity_ids.iter().enumerate() {
            let Ok(component) = registry.get_component::<LightComponent>(id) else {
                // A missing dynamic entity still occupies its compact forward
                // slot. Static entries have no direct-light slot to preserve.
                if self.shape[map_idx].is_dynamic {
                    lights_bytes.extend_from_slice(&[0u8; GPU_LIGHT_SIZE]);
                    descriptor_bytes.extend_from_slice(&[0u8; ANIMATION_DESCRIPTOR_SIZE]);
                    influences.push(self.cached_influences[map_idx].clone());
                }
                continue;
            };

            let map_light = component_to_map_light(
                component,
                self.cached_origins_f64[map_idx],
                self.shape[map_idx].is_dynamic,
                self.shape[map_idx].cell_index,
            );
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

            let forward_desc =
                pack_forward_animation_descriptor(component, brightness_offset, color_offset);
            if self.shape[map_idx].is_dynamic {
                lights_bytes.extend_from_slice(&pack_light(&map_light));
                descriptor_bytes.extend_from_slice(&forward_desc);
                influences.push(self.cached_influences[map_idx].clone());
            }

            // For `_animated` (and other slot-bearing) lights, also queue a
            // write into the animated-compose descriptor buffer at the cached
            // section slot. The compose pass reads the same 48-byte stride
            // from its own descriptor buffer (group 1 binding 4) — the offsets
            // we just baked point into the shared `anim_samples` scripted
            // region, which both the forward and compose paths sample.
            if let Some(slot) = self.shape[map_idx].animated_slot {
                compose_descriptor_writes.push((
                    slot,
                    pack_compose_animation_descriptor(component, brightness_offset, color_offset),
                ));
            }
        }

        // Matches `postretro_render_cpu::sh_volume` sample packing: native-endian f32 bytes.
        let samples_bytes = self
            .scripted_sample_buf
            .iter()
            .flat_map(|&v| v.to_ne_bytes())
            .collect();
        let influence_bytes = postretro_lighting::influence::pack_influence(&influences);

        Some(LightBridgeUpdate {
            has_dirty_data: true,
            lights_bytes,
            descriptor_bytes,
            influence_bytes,
            samples_bytes,
            effective_brightness,
            compose_descriptor_writes,
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
        is_dynamic: light.is_dynamic,
        animated_slot: light.animated_slot,
        animation: None,
    }
}

fn component_to_map_light(
    component: &LightComponent,
    origin_f64: [f64; 3],
    is_dynamic: bool,
    cell_index: u32,
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
        is_dynamic,
        // Script-spawned lights have no authoring surface for the
        // shadow-pool opt-in (Task 1b); default `false`. Wired later if a
        // script-side API for entity-shadow opt-in lands.
        casts_entity_shadows: false,
        animated_slot: None,
        tags: vec![],
        cell_index,
        // Script-spawned lights have no authoring surface for `_shadow_type`
        // and `LightComponent` does not carry it; default `StaticLightMap`
        // (same gap as `casts_entity_shadows` above). Preserving the tag
        // through the component is part of the broader entity-model migration.
        shadow_type: ShadowType::StaticLightMap,
    }
}

fn uncullable_light_influence() -> LightInfluence {
    LightInfluence {
        center: glam::Vec3::ZERO,
        radius: f32::MAX,
    }
}

fn component_to_influence(component: &LightComponent) -> LightInfluence {
    LightInfluence {
        center: glam::Vec3::from_array(component.origin),
        radius: if matches!(component.light_type, LightKind::Directional) {
            f32::MAX
        } else {
            component.falloff_range.max(0.0)
        },
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

/// Pack one `LightComponent`'s animation state into the 48-byte descriptor
/// layout owned by `postretro_render_cpu::sh_volume` and consumed by WGSL.
///
/// Forward descriptors use an all-zero sentinel for `animation == None` so
/// direct-light shaders fall back to packed `GpuLight` fields. Compose
/// descriptors instead stay active with authored radiance because their baked
/// weight-map path has no direct-light fallback.
///
/// **`play_count` is stripped:** the GPU never sees completion bounds. The
/// CPU-side bridge handles completion by writing the final keyframe back as
/// static `intensity`/`color` and clearing `animation`. The GPU always sees
/// a looping descriptor or a sentinel.
///
/// Sample payloads live in a separate `anim_samples` storage buffer addressed
/// by per-descriptor offsets.
fn pack_forward_animation_descriptor(
    component: &LightComponent,
    brightness_offset: u32,
    color_offset: u32,
) -> [u8; ANIMATION_DESCRIPTOR_SIZE] {
    pack_animation_descriptor(
        component,
        brightness_offset,
        color_offset,
        component.color,
        false,
    )
}

fn pack_compose_animation_descriptor(
    component: &LightComponent,
    brightness_offset: u32,
    color_offset: u32,
) -> [u8; ANIMATION_DESCRIPTOR_SIZE] {
    let base_color = if component
        .animation
        .as_ref()
        .and_then(|animation| animation.color.as_ref())
        .is_some_and(|samples| !samples.is_empty())
    {
        [component.intensity; 3]
    } else {
        [
            component.color[0] * component.intensity,
            component.color[1] * component.intensity,
            component.color[2] * component.intensity,
        ]
    };
    pack_animation_descriptor(component, brightness_offset, color_offset, base_color, true)
}

fn pack_animation_descriptor(
    component: &LightComponent,
    brightness_offset: u32,
    color_offset: u32,
    base_color: [f32; 3],
    active_without_animation: bool,
) -> [u8; ANIMATION_DESCRIPTOR_SIZE] {
    let mut bytes = [0u8; ANIMATION_DESCRIPTOR_SIZE];
    let Some(anim) = &component.animation else {
        // Forward uses the zero sentinel to fall back to `GpuLight`. Compose
        // has no such fallback: a reserved static slot stays active and reads
        // authored radiance from `base_color` when no curve is installed.
        if active_without_animation {
            bytes[0..4].copy_from_slice(&1.0f32.to_ne_bytes());
            bytes[16..20].copy_from_slice(&base_color[0].to_ne_bytes());
            bytes[20..24].copy_from_slice(&base_color[1].to_ne_bytes());
            bytes[24..28].copy_from_slice(&base_color[2].to_ne_bytes());
            bytes[36..40].copy_from_slice(&1u32.to_ne_bytes());
        }
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

    bytes[16..20].copy_from_slice(&base_color[0].to_ne_bytes());
    bytes[20..24].copy_from_slice(&base_color[1].to_ne_bytes());
    bytes[24..28].copy_from_slice(&base_color[2].to_ne_bytes());

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
    use postretro_level_loader::{FalloffModel, LightType};

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
            is_dynamic: false,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![],
            cell_index: 0,
            shadow_type: postretro_level_loader::ShadowType::StaticLightMap,
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
            is_dynamic: true,
            casts_entity_shadows: true,
            animated_slot: None,
            tags: vec![],
            cell_index: 0,
            shadow_type: postretro_level_loader::ShadowType::StaticLightMap,
        }
    }

    fn sample_dynamic_point_light() -> MapLight {
        let mut light = sample_point_light();
        light.is_dynamic = true;
        light
    }

    fn sample_animation() -> LightAnimation {
        LightAnimation {
            period_ms: 1000.0,
            phase: None,
            play_count: None,
            start_active: None,
            brightness: Some(vec![0.0, 1.0, 0.0]),
            color: None,
            direction: None,
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
        let lights = vec![sample_dynamic_point_light()];
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
        bridge.populate_from_level(&[sample_dynamic_point_light()], &mut registry, 0);
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
        bridge.populate_from_level(&[sample_dynamic_point_light()], &mut registry, 0);
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
        bridge.populate_from_level(&[sample_dynamic_point_light()], &mut registry, 0);
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
        bridge.populate_from_level(&[sample_dynamic_point_light()], &mut registry, 0);
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
        bridge.populate_from_level(&[sample_dynamic_point_light()], &mut registry, 0);
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
            is_dynamic: true,
            animated_slot: None,
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
        let bytes =
            pack_forward_animation_descriptor(&component, 0, SCRIPTED_BRIGHTNESS_SLOT as u32);
        let active = u32::from_ne_bytes(bytes[36..40].try_into().unwrap());
        assert_eq!(active, 0, "start_active: Some(false) must pack as inactive");
    }

    #[test]
    fn phase_outside_unit_interval_is_wrapped_via_rem_euclid_in_descriptor() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_dynamic_point_light()], &mut registry, 0);
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
    fn absorb_dynamic_lights_picks_up_components_spawned_after_populate() {
        // Mirrors the data-archetype sweep: a `LightComponent` lands in the
        // registry after `populate_from_level`. The bridge must enroll the
        // new entity so it ends up in the GPU upload on the next `update`.
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_dynamic_point_light()], &mut registry, 0);
        assert_eq!(bridge.light_count(), 1);

        // Simulate descriptor-spawn: a fresh entity with a `LightComponent`.
        let new_id = registry.try_spawn(Default::default(), &[]).unwrap();
        let component = LightComponent {
            origin: [9.0, -2.0, 4.5],
            light_type: LightKind::Point,
            intensity: 3.0,
            color: [1.0, 0.5, 0.25],
            falloff_model: FalloffKind::InverseSquared,
            falloff_range: 12.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            is_dynamic: true,
            animated_slot: None,
            animation: None,
        };
        registry.set_component(new_id, component).unwrap();

        bridge.absorb_dynamic_lights(&registry);
        assert_eq!(
            bridge.light_count(),
            2,
            "absorb must enroll the new dynamic light",
        );
        assert_eq!(bridge.entity_for_map_index(1), Some(new_id));

        // Idempotent: a second pass with no new lights must not double-enroll.
        bridge.absorb_dynamic_lights(&registry);
        assert_eq!(bridge.light_count(), 2);

        // Next update produces a GPU upload that includes both lights.
        let update = bridge.update(&mut registry, 0.0).expect("dirty");
        assert!(update.has_dirty_data);
        assert_eq!(update.lights_bytes.len(), 2 * GPU_LIGHT_SIZE);
        assert_eq!(update.descriptor_bytes.len(), 2 * ANIMATION_DESCRIPTOR_SIZE,);
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

    /// `setLightAnimation` on a static `_animated` light (one with a cached
    /// `animated_slot`) feeds a compose-side descriptor write. The bridge
    /// produces `compose_descriptor_writes` keyed on the cached slot, not on
    /// map-light index; the descriptor bytes carry the live brightness count.
    /// Asserted: brightness-only animation reaches the compose descriptor
    /// without going through the `is_dynamic` forward path.
    #[test]
    fn animated_light_routes_set_animation_through_compose_buffer() {
        let mut light = sample_point_light();
        // Static geometry; intensity arrives from script. Compiler assigned
        // slot 3 (arbitrary; the bridge keys on this value).
        light.is_dynamic = false;
        light.animated_slot = Some(3);

        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[light], &mut registry, 0);
        // Flush the initial dirty upload (no script animation yet — bytes
        // present but they're the sentinel descriptor for the slot).
        let initial = bridge.update(&mut registry, 0.0).expect("initial dirty");
        assert!(initial.has_dirty_data);
        assert_eq!(
            initial.compose_descriptor_writes.len(),
            1,
            "every populated `_animated` light writes its initial descriptor"
        );
        assert_eq!(initial.compose_descriptor_writes[0].0, 3);

        // Now run a setLightAnimation on the light.
        let id = bridge.entity_for_map_index(0).unwrap();
        let mut component = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        component.animation = Some(LightAnimation {
            period_ms: 1000.0,
            phase: None,
            play_count: None,
            start_active: None,
            brightness: Some(vec![0.0, 1.0, 0.0]),
            color: None,
            direction: None,
        });
        registry.set_component(id, component).unwrap();

        let update = bridge
            .update(&mut registry, 0.0)
            .expect("dirty after setLightAnimation");
        assert!(update.has_dirty_data);
        assert_eq!(
            update.compose_descriptor_writes.len(),
            1,
            "dirty frame emits one compose-side write for the slot-bearing light"
        );
        let (slot, bytes) = &update.compose_descriptor_writes[0];
        assert_eq!(*slot, 3, "write targets the cached `animated_slot`");
        // Bytes[12..16] = brightness_count (matches `pack_animation_descriptor`).
        let brightness_count = u32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        assert_eq!(
            brightness_count, 3,
            "compose descriptor carries the scripted brightness curve length"
        );
        // active (bytes[36..40]) = 1: animation is live, not sentinel.
        let active = u32::from_ne_bytes(bytes[36..40].try_into().unwrap());
        assert_eq!(active, 1);
    }

    // Regression: the scripting bridge was populated from the renderer's
    // dynamic-only list, so a script-reserved baked light was absent from
    // world.query and could never install its compose descriptor.
    #[test]
    fn full_authored_order_exposes_static_light_without_entering_direct_buffer() {
        let mut scripted_static = sample_point_light();
        scripted_static.tags = vec!["script_wave".into()];
        scripted_static.animated_slot = Some(4);
        let dynamic = sample_spot_light();

        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(
            &[scripted_static, dynamic],
            &mut registry,
            ANIMATION_DESCRIPTOR_SIZE as u32 / 4,
        );

        let static_id = bridge.entity_for_map_index(0).expect("authored slot 0");
        assert_eq!(
            registry
                .query_by_component_and_tag(ComponentKind::Light, Some("script_wave"))
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            vec![static_id],
        );

        let mut component = registry
            .get_component::<LightComponent>(static_id)
            .unwrap()
            .clone();
        component.animation = Some(sample_animation());
        registry.set_component(static_id, component).unwrap();

        let update = bridge.update(&mut registry, 0.0).expect("initial dirty");

        assert_eq!(
            bridge.light_count(),
            2,
            "both authored lights stay queryable"
        );
        assert_eq!(
            update.lights_bytes.len(),
            GPU_LIGHT_SIZE,
            "only the dynamic light enters the direct forward buffer",
        );
        assert_eq!(
            update.descriptor_bytes.len(),
            ANIMATION_DESCRIPTOR_SIZE,
            "forward descriptors stay index-parallel to dynamic lights",
        );
        assert_eq!(update.effective_brightness.len(), 1);
        assert_eq!(
            update.samples_bytes.len(),
            2 * SCRIPTED_FLOATS_PER_LIGHT * 4,
            "script samples retain stable full-authored map-light slots",
        );
        assert_eq!(update.compose_descriptor_writes.len(), 1);
        let (slot, descriptor) = &update.compose_descriptor_writes[0];
        assert_eq!(*slot, 4);
        assert_eq!(
            u32::from_ne_bytes(descriptor[8..12].try_into().unwrap()),
            ANIMATION_DESCRIPTOR_SIZE as u32 / 4,
            "authored map-light index 0 starts at the scripted sample region",
        );
        assert_eq!(
            u32::from_ne_bytes(descriptor[12..16].try_into().unwrap()),
            3,
        );
    }

    /// A light without a baked slot (legacy / non-`_animated`) produces no
    /// compose-side descriptor writes — the bridge falls back to the legacy
    /// forward path entirely.
    #[test]
    fn non_animated_light_produces_no_compose_descriptor_writes() {
        let light = sample_dynamic_point_light(); // animated_slot = None
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[light], &mut registry, 0);
        let update = bridge.update(&mut registry, 0.0).unwrap();
        assert!(update.has_dirty_data);
        assert!(
            update.compose_descriptor_writes.is_empty(),
            "lights without `animated_slot` must not feed the compose buffer"
        );
    }

    #[test]
    fn slotless_static_animation_warns_once_with_stable_map_index_and_tags() {
        let mut light = sample_point_light();
        light.tags = vec!["hallway_wave".into(), "alarm".into()];
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[light], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0);

        let id = bridge.entity_for_map_index(0).unwrap();
        let mut component = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        component.animation = Some(sample_animation());
        registry.set_component(id, component).unwrap();

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            let _ = bridge.update(&mut registry, 0.1);

            // A subsequent dirty update while the same animation remains live
            // must not repeat the author-facing warning.
            let mut component = registry
                .get_component::<LightComponent>(id)
                .unwrap()
                .clone();
            component.intensity = 2.0;
            registry.set_component(id, component).unwrap();
            let _ = bridge.update(&mut registry, 0.2);
        });

        let warnings: Vec<_> = captured
            .iter()
            .filter(|(level, message)| {
                *level == log::Level::Warn && message.contains("static map light")
            })
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "expected one slotless-light warning: {captured:?}"
        );
        let message = &warnings[0].1;
        assert!(message.contains("map light 0"));
        assert!(message.contains("hallway_wave"));
        assert!(message.contains("alarm"));
        assert!(message.contains("baked contribution will not animate"));
        assert!(message.contains("script-derived membership or `_animated 1`"));
    }

    #[test]
    fn only_slotless_static_light_animations_warn() {
        let mut slot_backed_static = sample_point_light();
        slot_backed_static.animated_slot = Some(7);
        let lights = vec![
            sample_spot_light(),
            slot_backed_static,
            sample_point_light(),
        ];
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&lights, &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0);

        // Dynamic and slot-backed static lights receive animations; the final
        // static slotless light remains unanimated. None meet the diagnostic gate.
        for map_idx in [0, 1] {
            let id = bridge.entity_for_map_index(map_idx).unwrap();
            let mut component = registry
                .get_component::<LightComponent>(id)
                .unwrap()
                .clone();
            component.animation = Some(sample_animation());
            registry.set_component(id, component).unwrap();
        }

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            let _ = bridge.update(&mut registry, 0.1);
        });
        assert!(
            !captured.iter().any(|(level, message)| {
                *level == log::Level::Warn && message.contains("static map light")
            }),
            "dynamic, slot-backed, and unanimated lights must not warn: {captured:?}"
        );
    }

    // Regression: maps with no authored dynamic lights allocated a zero-length
    // forward contract, so the first runtime-spawned light could not render.
    #[test]
    fn runtime_light_on_static_only_map_emits_complete_dynamic_record_set() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0);

        let id = registry.try_spawn(Default::default(), &[]).unwrap();
        let mut component = map_light_to_component(&sample_dynamic_point_light());
        component.animation = Some(sample_animation());
        registry.set_component(id, component).unwrap();
        bridge.absorb_dynamic_lights(&registry);

        let update = bridge
            .update(&mut registry, 0.0)
            .expect("runtime light dirty");
        assert_eq!(update.lights_bytes.len(), GPU_LIGHT_SIZE);
        assert_eq!(update.influence_bytes.len(), 16);
        assert_eq!(update.descriptor_bytes.len(), ANIMATION_DESCRIPTOR_SIZE);
        assert_eq!(
            u32::from_ne_bytes(update.descriptor_bytes[12..16].try_into().unwrap()),
            3,
            "runtime light animation descriptor reaches the compact forward slot",
        );
        assert_eq!(update.effective_brightness.len(), 1);
    }

    // Regression: disappearance did not dirty the bridge, leaving the last
    // live GPU record illuminating the scene after entity despawn.
    #[test]
    fn despawned_dynamic_light_emits_one_zero_tombstone() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_dynamic_point_light()], &mut registry, 0);
        let id = bridge.entity_for_map_index(0).unwrap();
        let _ = bridge.update(&mut registry, 0.0);

        registry.despawn(id).unwrap();
        let update = bridge.update(&mut registry, 0.1).expect("despawn dirty");
        assert!(update.has_dirty_data);
        assert_eq!(update.lights_bytes, vec![0; GPU_LIGHT_SIZE]);
        assert_eq!(update.descriptor_bytes, vec![0; ANIMATION_DESCRIPTOR_SIZE]);
        assert_eq!(update.effective_brightness, vec![0.0]);

        assert!(!bridge.update(&mut registry, 0.2).unwrap().has_dirty_data);
    }

    // Regression: the compose path has no `GpuLight` fallback. A zero sentinel
    // on install or clear turned a script-reserved static light black.
    #[test]
    fn static_compose_descriptor_stays_active_with_authored_radiance() {
        let mut light = sample_point_light();
        light.intensity = 2.5;
        light.color = [0.4, 0.2, 0.1];
        light.animated_slot = Some(0);
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[light], &mut registry, 0);

        let initial = bridge.update(&mut registry, 0.0).unwrap();
        let initial_desc = &initial.compose_descriptor_writes[0].1;
        assert_eq!(
            u32::from_ne_bytes(initial_desc[36..40].try_into().unwrap()),
            1
        );
        assert!((f32::from_ne_bytes(initial_desc[16..20].try_into().unwrap()) - 1.0).abs() < 1e-6);

        let id = bridge.entity_for_map_index(0).unwrap();
        let mut component = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        component.animation = Some(sample_animation());
        registry.set_component(id, component).unwrap();
        let _ = bridge.update(&mut registry, 0.1);

        let mut component = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        component.animation = None;
        registry.set_component(id, component).unwrap();
        let cleared = bridge.update(&mut registry, 0.2).unwrap();
        let cleared_desc = &cleared.compose_descriptor_writes[0].1;
        assert_eq!(
            u32::from_ne_bytes(cleared_desc[36..40].try_into().unwrap()),
            1
        );
        assert!((f32::from_ne_bytes(cleared_desc[16..20].try_into().unwrap()) - 1.0).abs() < 1e-6);
    }
}
