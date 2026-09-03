// Scripting ↔ renderer bridge for map lights: entity registry → GPU light buffer.
// See: context/lib/scripting.md

use std::collections::{HashMap, HashSet};

use postretro_foundation::Vec3Lit;
use postretro_level_format::sh_volume::AnimationDescriptor;
use postretro_level_loader::{FalloffModel, LightType, MapLight, ShadowType};
use postretro_lighting::{GPU_LIGHT_SIZE, pack_light};
use postretro_render_cpu::sh_volume::{
    ANIMATION_DESCRIPTOR_SIZE, SCRIPTED_BRIGHTNESS_SLOT, SCRIPTED_COLOR_SLOT_F32,
    SCRIPTED_FLOATS_PER_LIGHT,
};
use postretro_render_data::influence::LightInfluence;
use postretro_renderer::RUNTIME_DYNAMIC_LIGHT_RESERVE;

use postretro_entities::Transform;
use postretro_entities::components::light::LightAnimation;
use postretro_entities::components::light::{FalloffKind, LightComponent, LightKind};
use postretro_entities::registry::{ComponentKind, EntityId, EntityRegistry};

/// Snapshot of a map light's component state as last observed by the bridge.
/// Dirty detection compares the live registry component against this value.
///
/// `animation_start_time` is `Some(t)` while a `play_count`-bounded animation
/// is running, where `t` is the engine time when the animation was last written.
/// `animation_cycle_index` identifies the finite period currently packed for
/// endpoint-clamped GPU sampling.
/// When `current_time − t` reaches `play_count × period_ms / 1000.0`, the bridge
/// samples the final keyframe, writes a static `LightComponent` back to the registry,
/// and clears the timing state. Any `setAnimation` call resets
/// `animation_start_time` to the current frame time — "last call wins" always
/// restarts the count from zero.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LightSnapshot {
    pub(crate) component: LightComponent,
    pub(crate) animation_start_time: Option<f32>,
    pub(crate) animation_cycle_index: u32,
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
    /// Authored indices stay stable. Despawned runtime entries remain as
    /// tombstones until a later spawn reuses their slot.
    entity_ids: Vec<EntityId>,
    authored_light_count: usize,
    /// Reclaimed runtime tombstone indices available for reuse. Until reuse,
    /// their retained slots preserve forward tombstones on every dirty repack
    /// and the GPU packing high-water mark. Runtime lights have no compose slot.
    free_slots: Vec<usize>,
    /// Dirty-tracking snapshots. `None` for an entry means the slot has never
    /// been snapshotted — treated as unconditionally dirty on first visit so
    /// the initial upload lands.
    snapshots: HashMap<EntityId, LightSnapshot>,
    /// Slot-bearing map lights whose compiler descriptor remains authoritative.
    /// The bridge latches a light out of this set on its first component
    /// mutation, after which script-authored clear/settle writes own the slot.
    preserve_baked_descriptors: HashSet<EntityId>,
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
    /// Body-matched render poses for runtime lights that follow their entity.
    /// Kept separately from the authored origin/influence caches so non-follow
    /// map lights retain their exact existing packing path.
    cached_follow_positions: Vec<Option<glam::Vec3>>,
    /// Float index into `anim_samples` where the scripted region starts
    /// (= FGD sample float count). Used to compute per-light absolute offsets.
    fgd_sample_float_count: u32,
    /// CPU mirror of the scripted-animation region in `anim_samples`. The
    /// map-authored prefix preserves full authored order.
    scripted_sample_buf: Vec<f32>,
    runtime_capacity_warned: bool,
    /// Last Light-column membership stamp examined by enrollment. Existing light
    /// mutations do not change it, so connected-client render frames with no spawn
    /// or removal skip the registry-wide discovery scan.
    observed_light_membership_generation: u64,
    #[cfg(test)]
    enrollment_scan_count: usize,
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
    /// Runtime tombstones are reclaimed exactly once after their entity
    /// disappears. Reuse resets this marker for the new entity.
    reclaimed: bool,
}

impl LightBridge {
    pub(crate) fn new() -> Self {
        Self {
            entity_ids: Vec::new(),
            authored_light_count: 0,
            free_slots: Vec::new(),
            snapshots: HashMap::new(),
            preserve_baked_descriptors: HashSet::new(),
            shape: Vec::new(),
            warned_slotless_animation_indices: std::collections::HashSet::new(),
            dirty: false,
            cached_origins_f64: Vec::new(),
            cached_influences: Vec::new(),
            cached_follow_positions: Vec::new(),
            fgd_sample_float_count: 0,
            scripted_sample_buf: Vec::new(),
            runtime_capacity_warned: false,
            observed_light_membership_generation: u64::MAX,
            #[cfg(test)]
            enrollment_scan_count: 0,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.entity_ids.clear();
        self.authored_light_count = 0;
        self.free_slots.clear();
        self.snapshots.clear();
        self.preserve_baked_descriptors.clear();
        self.shape.clear();
        self.warned_slotless_animation_indices.clear();
        self.dirty = false;
        self.cached_origins_f64.clear();
        self.cached_influences.clear();
        self.cached_follow_positions.clear();
        self.fgd_sample_float_count = 0;
        self.scripted_sample_buf.clear();
        self.runtime_capacity_warned = false;
        self.observed_light_membership_generation = u64::MAX;
        #[cfg(test)]
        {
            self.enrollment_scan_count = 0;
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
                let brightness =
                    eval_effective_brightness(component, self.snapshots.get(&id), current_time);
                let mut map_light = component_to_map_light(
                    component,
                    self.cached_follow_positions[map_idx]
                        .map(position_to_origin_f64)
                        .unwrap_or(self.cached_origins_f64[map_idx]),
                    self.shape[map_idx].is_dynamic,
                    self.shape[map_idx].cell_index,
                );
                if let Some(radius) =
                    eval_animated_radius(component, self.snapshots.get(&id), current_time)
                {
                    map_light.falloff_range = radius;
                }
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
        self.populate_from_level_with_influences(
            lights,
            &[],
            &[],
            registry,
            fgd_sample_float_count,
        );
    }

    pub(crate) fn populate_from_level_with_influences(
        &mut self,
        lights: &[MapLight],
        light_influences: &[LightInfluence],
        baked_descriptors: &[AnimationDescriptor],
        registry: &mut EntityRegistry,
        fgd_sample_float_count: u32,
    ) {
        self.entity_ids.clear();
        self.authored_light_count = 0;
        self.free_slots.clear();
        self.snapshots.clear();
        self.preserve_baked_descriptors.clear();
        self.shape.clear();
        self.warned_slotless_animation_indices.clear();
        self.cached_origins_f64.clear();
        self.cached_influences.clear();
        self.cached_follow_positions.clear();
        self.entity_ids.reserve(lights.len());
        self.shape.reserve(lights.len());
        self.cached_origins_f64.reserve(lights.len());
        self.cached_influences.reserve(lights.len());
        self.cached_follow_positions.reserve(lights.len());
        self.fgd_sample_float_count = fgd_sample_float_count;
        self.scripted_sample_buf = vec![0.0f32; lights.len() * SCRIPTED_FLOATS_PER_LIGHT];

        self.runtime_capacity_warned = false;

        for (map_idx, light) in lights.iter().enumerate() {
            let baked_descriptor = light
                .animated_slot
                .and_then(|slot| baked_descriptors.get(slot as usize));
            let component = map_light_to_component(light, baked_descriptor);
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
                reclaimed: false,
            });
            self.cached_origins_f64.push(light.origin);
            self.cached_influences.push(
                light_influences
                    .get(map_idx)
                    .cloned()
                    .unwrap_or_else(uncullable_light_influence),
            );
            self.cached_follow_positions.push(None);
            if baked_descriptor.is_some() {
                let component = registry
                    .get_component::<LightComponent>(id)
                    .expect("light component was installed before snapshot")
                    .clone();
                self.snapshots.insert(
                    id,
                    LightSnapshot {
                        component,
                        animation_start_time: None,
                        animation_cycle_index: 0,
                    },
                );
                self.preserve_baked_descriptors.insert(id);
            }
        }
        self.authored_light_count = self.entity_ids.len();
        // Force one post-install discovery pass: descriptor/runtime lights may
        // already exist beside the authored prefix when this bridge is populated.
        self.observed_light_membership_generation = u64::MAX;

        // Ensure the initial pack lands even when no script mutates on frame one.
        self.dirty = true;
    }

    /// Pick up `LightComponent` entities that were spawned outside of
    /// `populate_from_level` — typically by the data-archetype sweep, which
    /// runs after `App::resumed()` (where `populate_from_level` is called)
    /// during the first `RedrawRequested` once the data script has populated
    /// the entity-type registry. It may be called every fixed or render frame:
    /// the registry's Light-membership stamp bypasses the discovery scan unless
    /// a light was added or removed.
    ///
    /// Any `LightComponent` entity not already tracked in `self.entity_ids`
    /// reuses a reclaimed runtime slot, or extends the parallel arrays when
    /// needed, so its component participates in the per-frame dirty/pack loop.
    /// Enrollment reads spawn-time origin and influence data but does not mutate
    /// the component. The next `update` produces its initial GPU upload.
    ///
    /// Descriptor-spawned lights are always dynamic
    /// (`data_archetype.rs` forces `is_dynamic = true` regardless of source);
    /// they have no cell assignment yet, so `cell_index` is recorded as
    /// `u32::MAX` — the unassigned sentinel. Replace with a real cell index
    /// when runtime-spawned light cell assignment is implemented.
    /// The cached f64 origin mirrors the f32 component origin
    /// (descriptor-spawn is f32 from the start; there is no f64 source).
    pub(crate) fn absorb_dynamic_lights(&mut self, registry: &EntityRegistry) {
        let membership_generation = registry.light_membership_generation();
        if membership_generation == self.observed_light_membership_generation {
            return;
        }

        // Reclaim missing tracked lights before checking capacity. A contact can
        // remove its travel light and spawn a short impact flash in one frame; the
        // replacement may safely overwrite that slot in the same full repack.
        self.reclaim_missing_runtime_slots(registry);

        // Runtime spawns are rare. Once membership changes, scan without allocating;
        // membership remains a linear check against the reserve-bounded tracked ids.
        #[cfg(test)]
        {
            self.enrollment_scan_count += 1;
        }
        let mut absorbed_any = false;
        for (id, _) in registry.iter_with_kind(ComponentKind::Light) {
            if self.entity_ids.contains(&id) {
                continue;
            }

            let live_runtime_count =
                self.entity_ids.len() - self.authored_light_count - self.free_slots.len();
            if live_runtime_count >= RUNTIME_DYNAMIC_LIGHT_RESERVE {
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

            let runtime_shape = MapLightShape {
                is_dynamic: true,
                cell_index: u32::MAX,
                // Script-spawned dynamic lights have no baked slot; the
                // bridge routes them via the legacy forward path.
                animated_slot: None,
                reclaimed: false,
            };
            if let Some(slot) = self.free_slots.pop() {
                self.entity_ids[slot] = id;
                self.shape[slot] = runtime_shape;
                self.cached_origins_f64[slot] = origin_f64;
                self.cached_influences[slot] = influence;
                self.cached_follow_positions[slot] = None;
            } else {
                self.entity_ids.push(id);
                self.shape.push(runtime_shape);
                self.cached_origins_f64.push(origin_f64);
                self.cached_influences.push(influence);
                self.cached_follow_positions.push(None);
            }
            absorbed_any = true;
        }

        self.observed_light_membership_generation = membership_generation;

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

    fn reclaim_missing_runtime_slots(&mut self, registry: &EntityRegistry) {
        for map_idx in self.authored_light_count..self.entity_ids.len() {
            let id = self.entity_ids[map_idx];
            if registry.get_component::<LightComponent>(id).is_ok() || self.shape[map_idx].reclaimed
            {
                continue;
            }
            self.shape[map_idx].reclaimed = true;
            self.free_slots.push(map_idx);
            self.snapshots.remove(&id);
            self.dirty = true;
        }
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
        alpha: f32,
    ) -> Option<LightBridgeUpdate> {
        if self.entity_ids.is_empty() {
            return None;
        }

        // Walk stable tracked slots rather than the registry's full iterator.
        // The authored prefix never moves; runtime lights may reuse tombstones
        // in place without changing slot indices.
        // Settled animations are collected and written back after the loop to
        // avoid aliasing the registry borrow.
        let mut settled: Vec<(usize, EntityId, LightComponent, bool)> = Vec::new();
        for (map_idx, &id) in self.entity_ids.iter().enumerate() {
            let Ok(current) = registry.get_component::<LightComponent>(id) else {
                // A tracked light that disappears must force one tombstone
                // upload; otherwise its last forward or compose record stays live.
                if map_idx >= self.authored_light_count && !self.shape[map_idx].reclaimed {
                    self.shape[map_idx].reclaimed = true;
                    self.free_slots.push(map_idx);
                    self.dirty = true;
                }
                if self.snapshots.remove(&id).is_some() {
                    self.dirty = true;
                }
                continue;
            };

            let followed_position = follow_transform_position(registry, id, current, alpha);
            if self.cached_follow_positions[map_idx] != followed_position {
                self.cached_follow_positions[map_idx] = followed_position;
                self.dirty = true;
            }

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
                let had_radius_animation = current
                    .animation
                    .as_ref()
                    .is_some_and(|animation| animation.radius.is_some());
                settled.push((map_idx, id, settled_component, had_radius_animation));
                continue;
            }

            // Radius is evaluated CPU-side and changes the packed forward
            // range plus its paired culling sphere, so it cannot share the
            // GPU-only brightness/color dirty behavior. A stationary impact
            // flash has no follow-pose movement to make this frame dirty.
            if current
                .animation
                .as_ref()
                .is_some_and(|animation| animation.radius.is_some())
            {
                self.dirty = true;
            }

            let cycle_index = if changed {
                0
            } else {
                finite_animation_cycle_index(current, snapshot, current_time)
            };
            let cycle_changed =
                snapshot.is_some_and(|snapshot| snapshot.animation_cycle_index != cycle_index);

            if changed || cycle_changed {
                if changed {
                    self.preserve_baked_descriptors.remove(&id);
                }
                self.dirty = true;
                let mut new_start = if changed {
                    None
                } else {
                    snapshot.and_then(|snapshot| snapshot.animation_start_time)
                };
                if let Some(anim) = &current.animation
                    && anim.play_count.is_some()
                    && changed
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
                        animation_cycle_index: cycle_index,
                    },
                );
            }
        }

        // Commit settled components so a subsequent `world.query` observes
        // post-animation static state.
        for (map_idx, id, settled_component, had_radius_animation) in settled {
            // Stale-id error means the entity was despawned between read and write; ignore.
            let _ = registry.set_component(id, settled_component.clone());
            if had_radius_animation {
                // The active-curve pack path above overrides this cache every
                // frame. Keep its final value after clearing `animation` so
                // the completion frame and later static frames retain the
                // radius paired with the settled `GpuLight` range.
                self.cached_influences[map_idx].radius = settled_component.falloff_range;
            }
            self.snapshots.insert(
                id,
                LightSnapshot {
                    component: settled_component,
                    animation_start_time: None,
                    animation_cycle_index: 0,
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
                eval_effective_brightness(component, self.snapshots.get(&id), current_time)
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
                // slot. A missing slot-bearing static entity similarly needs
                // an explicit compose tombstone or its last baked delta stays
                // active indefinitely.
                if self.shape[map_idx].is_dynamic {
                    lights_bytes.extend_from_slice(&[0u8; GPU_LIGHT_SIZE]);
                    descriptor_bytes.extend_from_slice(&[0u8; ANIMATION_DESCRIPTOR_SIZE]);
                    influences.push(self.cached_influences[map_idx].clone());
                }
                if let Some(slot) = self.shape[map_idx].animated_slot {
                    compose_descriptor_writes.push((slot, [0u8; ANIMATION_DESCRIPTOR_SIZE]));
                }
                continue;
            };

            let followed_position = self.cached_follow_positions[map_idx];
            let sampled_radius =
                eval_animated_radius(component, self.snapshots.get(&id), current_time);
            let mut map_light = component_to_map_light(
                component,
                followed_position
                    .map(position_to_origin_f64)
                    .unwrap_or(self.cached_origins_f64[map_idx]),
                self.shape[map_idx].is_dynamic,
                self.shape[map_idx].cell_index,
            );
            if let Some(radius) = sampled_radius {
                map_light.falloff_range = radius;
            }
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

            let snapshot = self.snapshots.get(&id);
            let forward_desc = pack_forward_animation_descriptor(
                component,
                snapshot,
                brightness_offset,
                color_offset,
            );
            if self.shape[map_idx].is_dynamic {
                lights_bytes.extend_from_slice(&pack_light(&map_light));
                descriptor_bytes.extend_from_slice(&forward_desc);
                let mut influence = self.cached_influences[map_idx].clone();
                if let Some(position) = followed_position {
                    influence.center = position;
                }
                if let Some(radius) = sampled_radius {
                    influence.radius = radius;
                }
                influences.push(influence);
            }

            // For `_animated` (and other slot-bearing) lights, also queue a
            // write into the animated-compose descriptor buffer at the cached
            // section slot. The compose pass reads the same 48-byte stride
            // from its own descriptor buffer (group 1 binding 4) — the offsets
            // we just baked point into the shared `anim_samples` scripted
            // region, which both the forward and compose paths sample.
            if let Some(slot) = self.shape[map_idx].animated_slot {
                if !self.preserve_baked_descriptors.contains(&id) {
                    compose_descriptor_writes.push((
                        slot,
                        pack_compose_animation_descriptor(
                            component,
                            snapshot,
                            brightness_offset,
                            color_offset,
                        ),
                    ));
                }
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

fn map_light_to_component(
    light: &MapLight,
    baked_descriptor: Option<&AnimationDescriptor>,
) -> LightComponent {
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
        follow_transform: false,
        carrier: None,
        animation: baked_descriptor.map(|descriptor| LightAnimation {
            period_ms: descriptor.period * 1000.0,
            phase: Some(descriptor.phase),
            play_count: None,
            start_active: Some(descriptor.start_active != 0),
            brightness: (!descriptor.brightness.is_empty()).then(|| descriptor.brightness.clone()),
            color: (!descriptor.color.is_empty())
                .then(|| descriptor.color.iter().copied().map(Vec3Lit).collect()),
            direction: (!descriptor.direction.is_empty())
                .then(|| descriptor.direction.iter().copied().map(Vec3Lit).collect()),
            radius: None,
        }),
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
        // Script-spawned lights have no authoring surface for the shadow-pool
        // opt-in, so they remain non-shadow-casting unless that capability is
        // deliberately added to the entity light contract.
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

/// Resolve a mover-attached light from the exact pose its projectile body uses
/// in the current render frame. Sprite bodies deliberately take the raw tick
/// transform because billboard collection does not interpolate them; rigid
/// model bodies use the same interpolated transform as mesh collection. An
/// unrepresentable carrier composition returns `None`, retaining the authored
/// finite position instead of packing a non-finite GPU position.
fn follow_transform_position(
    registry: &EntityRegistry,
    id: EntityId,
    component: &LightComponent,
    alpha: f32,
) -> Option<glam::Vec3> {
    if let Some(carrier) = component.carrier.as_ref() {
        return registry
            .interpolated_transform(carrier.mover_entity, alpha)
            .ok()
            .and_then(|transform| {
                let rotation = glam::DQuat::from_xyzw(
                    f64::from(transform.rotation.x),
                    f64::from(transform.rotation.y),
                    f64::from(transform.rotation.z),
                    f64::from(transform.rotation.w),
                );
                let local_offset = glam::DVec3::new(
                    f64::from(carrier.local_offset.x),
                    f64::from(carrier.local_offset.y),
                    f64::from(carrier.local_offset.z),
                );
                let position = glam::DVec3::new(
                    f64::from(transform.position.x),
                    f64::from(transform.position.y),
                    f64::from(transform.position.z),
                ) + rotation * local_offset;
                let narrowed =
                    glam::Vec3::new(position.x as f32, position.y as f32, position.z as f32);
                narrowed.is_finite().then_some(narrowed)
            });
    }
    if !component.follow_transform {
        return None;
    }
    if registry
        .has_component_kind(id, ComponentKind::SpriteVisual)
        .ok()
        == Some(true)
    {
        return registry
            .get_component::<Transform>(id)
            .ok()
            .map(|transform| transform.position);
    }
    if registry.has_component_kind(id, ComponentKind::Mesh).ok() == Some(true) {
        return registry
            .interpolated_transform(id, alpha)
            .ok()
            .map(|transform| transform.position);
    }
    // A follow light is expected to accompany one of the two projectile body
    // components above. If presentation is mid-transition, still use the
    // entity's current live pose rather than the spawn-origin cache.
    registry
        .get_component::<Transform>(id)
        .ok()
        .map(|transform| transform.position)
}

fn position_to_origin_f64(position: glam::Vec3) -> [f64; 3] {
    [position.x as f64, position.y as f64, position.z as f64]
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

/// Endpoint-clamped Catmull-Rom sampling for finite animations. Samples are
/// uniformly spaced over inclusive `[0, 1]`, so `t == 1` is the final keyframe
/// rather than the closed curve's first keyframe.
fn sample_brightness_at_open(samples: &[f32], t: f32) -> f32 {
    let count = samples.len();
    if count == 0 {
        return 1.0;
    }
    if count == 1 {
        return samples[0];
    }
    let last = count - 1;
    let scaled = t.clamp(0.0, 1.0) * last as f32;
    let i1 = (scaled.floor() as usize).min(last);
    let i0 = i1.saturating_sub(1);
    let i2 = (i1 + 1).min(last);
    let i3 = (i1 + 2).min(last);
    let f = scaled.fract();
    let (p0, p1, p2, p3) = (samples[i0], samples[i1], samples[i2], samples[i3]);
    let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
    let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let c = -0.5 * p0 + 0.5 * p2;
    let d = p1;
    ((a * f + b) * f + c) * f + d
}

fn finite_animation_cycle_index(
    component: &LightComponent,
    snapshot: Option<&LightSnapshot>,
    current_time: f32,
) -> u32 {
    let Some(anim) = component.animation.as_ref() else {
        return 0;
    };
    let Some(play_count) = anim.play_count.filter(|&count| count > 0) else {
        return 0;
    };
    let Some(start) = snapshot.and_then(|snapshot| snapshot.animation_start_time) else {
        return 0;
    };
    let period_s = anim.period_ms / 1000.0;
    if period_s <= 0.0 {
        return 0;
    }
    ((current_time - start).max(0.0) / period_s)
        .floor()
        .min(play_count.saturating_sub(1) as f32) as u32
}

/// Current effective brightness for shadow-slot suppression. Mirrors GPU
/// animation evaluation; called every frame, not just on dirty frames.
fn eval_effective_brightness(
    component: &LightComponent,
    snapshot: Option<&LightSnapshot>,
    current_time: f32,
) -> f32 {
    match &component.animation {
        None => 1.0,
        Some(anim) => {
            if anim.start_active == Some(false) {
                0.0
            } else if let Some(brightness) = &anim.brightness
                && !brightness.is_empty()
            {
                sample_light_animation_curve(anim, snapshot, current_time, brightness)
            } else {
                1.0
            }
        }
    }
}

/// Current sampled falloff range, when an animation owns the radius channel.
/// Radius follows the brightness timing contract exactly, but stays CPU-side:
/// shaders only receive its result through the packed `GpuLight` range.
fn eval_animated_radius(
    component: &LightComponent,
    snapshot: Option<&LightSnapshot>,
    current_time: f32,
) -> Option<f32> {
    let animation = component.animation.as_ref()?;
    let radius = animation.radius.as_deref()?;
    Some(sample_light_animation_curve(
        animation,
        snapshot,
        current_time,
        radius,
    ))
}

/// Sample a scalar light-animation channel using the same closed-loop and
/// finite endpoint-clamped timing as brightness. `sample_brightness_at` names
/// the existing CPU/WGSL Catmull-Rom mirror; scalar radius curves deliberately
/// use that same evaluator.
fn sample_light_animation_curve(
    animation: &LightAnimation,
    snapshot: Option<&LightSnapshot>,
    current_time: f32,
    samples: &[f32],
) -> f32 {
    let period_s = animation.period_ms / 1000.0;
    if period_s <= 0.0 {
        return sample_brightness_at(samples, 0.0);
    }

    let phase = animation.phase.unwrap_or(0.0).rem_euclid(1.0);
    if animation.play_count.is_some_and(|count| count > 0)
        && let Some(snapshot) = snapshot
        && let Some(start) = snapshot.animation_start_time
    {
        let cycle_start = start + snapshot.animation_cycle_index as f32 * period_s;
        let open_t = phase + ((current_time - cycle_start) / period_s) * (1.0 - phase);
        sample_brightness_at_open(samples, open_t)
    } else {
        let cycle_t = (current_time / period_s + phase).rem_euclid(1.0);
        sample_brightness_at(samples, cycle_t)
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
/// CPU-side bridge resets finite descriptors once per period and handles
/// completion by writing final radiance back as static `intensity`/`color`
/// before clearing `animation`. A negative packed period selects the shared
/// endpoint-clamped curve mode; positive periods remain closed loops.
///
/// Sample payloads live in a separate `anim_samples` storage buffer addressed
/// by per-descriptor offsets.
fn pack_forward_animation_descriptor(
    component: &LightComponent,
    snapshot: Option<&LightSnapshot>,
    brightness_offset: u32,
    color_offset: u32,
) -> [u8; ANIMATION_DESCRIPTOR_SIZE] {
    pack_animation_descriptor(
        component,
        brightness_offset,
        color_offset,
        component.color,
        false,
        snapshot,
    )
}

fn pack_compose_animation_descriptor(
    component: &LightComponent,
    snapshot: Option<&LightSnapshot>,
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
    pack_animation_descriptor(
        component,
        brightness_offset,
        color_offset,
        base_color,
        true,
        snapshot,
    )
}

pub(crate) fn pack_animation_descriptor(
    component: &LightComponent,
    brightness_offset: u32,
    color_offset: u32,
    base_color: [f32; 3],
    active_without_animation: bool,
    snapshot: Option<&LightSnapshot>,
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

    // GPU uses seconds; script-side tracks ms. A negative period is the
    // descriptor-only marker for endpoint-clamped finite sampling. Its
    // magnitude and phase map the current authored period from the authored
    // starting phase through t=1 (the final keyframe).
    let period_s = anim.period_ms / 1000.0;
    let authored_phase = anim.phase.unwrap_or(0.0).rem_euclid(1.0);
    let (packed_period, packed_phase) = if anim.play_count.is_some_and(|count| count > 0)
        && let Some(snapshot) = snapshot
        && let Some(start) = snapshot.animation_start_time
    {
        let cycle_start = start + snapshot.animation_cycle_index as f32 * period_s;
        let open_period = period_s / (1.0 - authored_phase).max(1.0e-6);
        (-open_period, authored_phase - cycle_start / open_period)
    } else {
        (period_s, authored_phase)
    };
    bytes[0..4].copy_from_slice(&packed_period.to_ne_bytes());
    bytes[4..8].copy_from_slice(&packed_phase.to_ne_bytes());

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

    // `None` defaults to active; `Some(false)` keeps this descriptor dark
    // until an explicit mutation replaces or clears it.
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
        settled.intensity *= final_brightness;
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
    if let Some(radius) = &anim.radius
        && let Some(&final_radius) = radius.last()
    {
        settled.falloff_range = final_radius;
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
            radius: None,
        }
    }

    fn runtime_component(
        origin: [f32; 3],
        falloff_range: f32,
        animation: Option<LightAnimation>,
    ) -> LightComponent {
        let mut component = map_light_to_component(&sample_dynamic_point_light(), None);
        component.origin = origin;
        component.falloff_range = falloff_range;
        component.animation = animation;
        component
    }

    fn spawn_runtime_light(registry: &mut EntityRegistry, component: LightComponent) -> EntityId {
        let id = registry
            .try_spawn(Default::default(), &[])
            .expect("test registry has capacity");
        registry
            .set_component(id, component)
            .expect("fresh entity accepts light component");
        id
    }

    fn live_runtime_count(bridge: &LightBridge) -> usize {
        bridge.entity_ids.len() - bridge.authored_light_count - bridge.free_slots.len()
    }

    fn packed_dynamic_range(update: &LightBridgeUpdate) -> f32 {
        f32::from_ne_bytes(update.lights_bytes[44..48].try_into().unwrap())
    }

    fn packed_dynamic_position(bytes: &[u8]) -> glam::Vec3 {
        glam::Vec3::new(
            f32::from_ne_bytes(bytes[0..4].try_into().unwrap()),
            f32::from_ne_bytes(bytes[4..8].try_into().unwrap()),
            f32::from_ne_bytes(bytes[8..12].try_into().unwrap()),
        )
    }

    fn packed_dynamic_direction(bytes: &[u8]) -> glam::Vec3 {
        glam::Vec3::new(
            f32::from_ne_bytes(bytes[32..36].try_into().unwrap()),
            f32::from_ne_bytes(bytes[36..40].try_into().unwrap()),
            f32::from_ne_bytes(bytes[40..44].try_into().unwrap()),
        )
    }

    fn packed_dynamic_influence_center(bytes: &[u8]) -> glam::Vec3 {
        glam::Vec3::new(
            f32::from_ne_bytes(bytes[0..4].try_into().unwrap()),
            f32::from_ne_bytes(bytes[4..8].try_into().unwrap()),
            f32::from_ne_bytes(bytes[8..12].try_into().unwrap()),
        )
    }

    fn packed_dynamic_influence_radius(update: &LightBridgeUpdate) -> f32 {
        f32::from_ne_bytes(update.influence_bytes[12..16].try_into().unwrap())
    }

    fn assert_packed_radius_in_lockstep(update: &LightBridgeUpdate, expected: f32) {
        let packed_range = packed_dynamic_range(update);
        let influence_radius = packed_dynamic_influence_radius(update);
        assert!(
            (packed_range - expected).abs() < 1e-6,
            "packed GpuLight range should be {expected}; got {packed_range}"
        );
        assert!(
            (influence_radius - expected).abs() < 1e-6,
            "packed influence radius should be {expected}; got {influence_radius}"
        );
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

    fn baked_descriptor(start_active: u32) -> AnimationDescriptor {
        AnimationDescriptor {
            period: 0.5,
            phase: 0.25,
            base_color: [1.5, 1.2, 0.9],
            brightness: vec![0.1, 1.0, 0.1],
            color: vec![[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            direction: vec![[0.0, -1.0, 0.0]],
            start_active,
        }
    }

    // Regression: the mandatory first dirty update replaced the compiler
    // descriptor with an active, curve-free descriptor.
    #[test]
    fn initial_install_preserves_baked_animation_descriptor_until_script_mutation() {
        let mut light = sample_point_light();
        light.animated_slot = Some(0);
        let descriptor = baked_descriptor(0);
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level_with_influences(
            &[light],
            &[],
            std::slice::from_ref(&descriptor),
            &mut registry,
            0,
        );

        let id = bridge.entity_for_map_index(0).unwrap();
        let component = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        let animation = component.animation.as_ref().expect("baked animation");
        assert_eq!(animation.period_ms, 500.0);
        assert_eq!(animation.phase, Some(0.25));
        assert_eq!(animation.start_active, Some(false));
        assert_eq!(animation.brightness.as_deref(), Some(&[0.1, 1.0, 0.1][..]));
        assert_eq!(
            animation.color.as_ref().unwrap()[1].as_f32_3(),
            [0.0, 0.0, 1.0]
        );
        assert_eq!(
            animation.direction.as_ref().unwrap()[0].as_f32_3(),
            [0.0, -1.0, 0.0]
        );

        let initial = bridge
            .update(&mut registry, 0.0, 0.0)
            .expect("initial dirty");
        assert!(initial.has_dirty_data);
        assert!(
            initial.compose_descriptor_writes.is_empty(),
            "renderer-owned baked descriptor must survive initial install"
        );

        let mut cleared = component.clone();
        cleared.animation = None;
        registry.set_component(id, cleared).unwrap();
        let update = bridge
            .update(&mut registry, 0.1, 0.0)
            .expect("explicit clear is dirty");
        assert_eq!(update.compose_descriptor_writes.len(), 1);
        let (_, descriptor_bytes) = &update.compose_descriptor_writes[0];
        assert_eq!(
            u32::from_ne_bytes(descriptor_bytes[36..40].try_into().unwrap()),
            1,
            "clear restores active authored radiance"
        );
        assert_eq!(
            u32::from_ne_bytes(descriptor_bytes[12..16].try_into().unwrap()),
            0
        );
        assert_eq!(
            u32::from_ne_bytes(descriptor_bytes[32..36].try_into().unwrap()),
            0
        );
    }

    #[test]
    fn first_update_after_populate_returns_initial_upload_bytes() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        let lights = vec![sample_dynamic_point_light()];
        bridge.populate_from_level(&lights, &mut registry, 0);

        let update = bridge
            .update(&mut registry, 0.0, 0.0)
            .expect("initial dirty");
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
        let _ = bridge.update(&mut registry, 0.0, 0.0);

        let update = bridge
            .update(&mut registry, 0.016, 0.0)
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
    fn follow_transform_uses_raw_sprite_pose_and_interpolated_model_pose() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[], &mut registry, 0);

        let sprite = registry.spawn(Transform {
            position: glam::Vec3::new(1.0, 0.0, 0.0),
            ..Transform::default()
        });
        registry
            .set_component(
                sprite,
                postretro_entities::components::sprite_visual::SpriteVisual {
                    sprite: "sprites/projectiles/bolt.png".to_string(),
                    size: 0.4,
                    opacity: 1.0,
                    rotation: 0.0,
                    tint: [1.0; 3],
                },
            )
            .unwrap();
        registry.snapshot_transform(sprite);
        registry
            .set_component(
                sprite,
                Transform {
                    position: glam::Vec3::new(9.0, 0.0, 0.0),
                    ..Transform::default()
                },
            )
            .unwrap();
        let mut sprite_light = runtime_component([-10.0, 0.0, 0.0], 5.0, None);
        sprite_light.follow_transform = true;
        registry.set_component(sprite, sprite_light).unwrap();

        let model = registry.spawn(Transform {
            position: glam::Vec3::new(2.0, 0.0, 0.0),
            ..Transform::default()
        });
        registry
            .set_component(
                model,
                postretro_entities::components::mesh::MeshComponent::stateless(
                    "models/projectiles/rocket.gltf".to_string(),
                ),
            )
            .unwrap();
        registry.snapshot_transform(model);
        registry
            .set_component(
                model,
                Transform {
                    position: glam::Vec3::new(10.0, 0.0, 0.0),
                    ..Transform::default()
                },
            )
            .unwrap();
        let mut model_light = runtime_component([-20.0, 0.0, 0.0], 5.0, None);
        model_light.follow_transform = true;
        registry.set_component(model, model_light).unwrap();

        bridge.absorb_dynamic_lights(&registry);
        let update = bridge
            .update(&mut registry, 0.0, 0.25)
            .expect("follow lights are enrolled and packed");
        assert_eq!(update.lights_bytes.len(), 2 * GPU_LIGHT_SIZE);
        assert_eq!(update.influence_bytes.len(), 2 * 16);

        assert!(
            packed_dynamic_position(&update.lights_bytes[..GPU_LIGHT_SIZE])
                .distance(glam::Vec3::new(9.0, 0.0, 0.0))
                <= 1.0e-6,
            "sprite lights use the raw billboard Transform rather than their spawn origin"
        );
        assert!(
            packed_dynamic_influence_center(&update.influence_bytes[..16])
                .distance(glam::Vec3::new(9.0, 0.0, 0.0))
                <= 1.0e-6,
            "the sprite influence sphere follows the same raw pose"
        );

        assert!(
            packed_dynamic_position(&update.lights_bytes[GPU_LIGHT_SIZE..])
                .distance(glam::Vec3::new(4.0, 0.0, 0.0))
                <= 1.0e-6,
            "model lights use the mesh's 25%-interpolated render pose"
        );
        assert!(
            packed_dynamic_influence_center(&update.influence_bytes[16..])
                .distance(glam::Vec3::new(4.0, 0.0, 0.0))
                <= 1.0e-6,
            "the model influence sphere follows the interpolated render pose"
        );
    }

    #[test]
    fn carrier_uses_interpolated_mover_pose_for_light_and_influence() {
        use postretro_entities::components::light::LightCarrier;

        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[], &mut registry, 0);

        let mover = registry.spawn(Transform::default());
        registry.snapshot_transform(mover);
        registry
            .set_component(
                mover,
                Transform {
                    position: glam::Vec3::new(4.0, 2.0, 1.0),
                    rotation: glam::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
                    ..Transform::default()
                },
            )
            .unwrap();

        let light = spawn_runtime_light(
            &mut registry,
            LightComponent {
                follow_transform: true,
                carrier: Some(LightCarrier {
                    mover_entity: mover,
                    local_offset: glam::Vec3::X,
                }),
                ..runtime_component([99.0, 0.0, 0.0], 5.0, None)
            },
        );
        registry
            .set_component(
                light,
                Transform {
                    position: glam::Vec3::new(99.0, 0.0, 0.0),
                    ..Transform::default()
                },
            )
            .unwrap();

        bridge.absorb_dynamic_lights(&registry);
        let update = bridge
            .update(&mut registry, 0.0, 0.5)
            .expect("carried light is enrolled and packed");
        let expected = glam::Vec3::new(2.0, 1.0, 0.5)
            + glam::Quat::from_rotation_z(std::f32::consts::FRAC_PI_4) * glam::Vec3::X;

        assert!(
            packed_dynamic_position(&update.lights_bytes).distance(expected) <= 1.0e-6,
            "carrier pose overrides the light's own follow-transform pose"
        );
        assert!(
            packed_dynamic_influence_center(&update.influence_bytes).distance(expected) <= 1.0e-6,
            "carrier pose relocates the matching dynamic-light influence"
        );
    }

    // Regression: malformed finite V6 carrier inputs composed to infinity and
    // were packed into both the GPU light record and its culling influence.
    #[test]
    fn unrepresentable_carrier_composition_keeps_gpu_positions_finite() {
        use postretro_entities::components::light::LightCarrier;

        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        let mut light = sample_dynamic_point_light();
        light.origin = [3.0e38, 0.0, 0.0];
        bridge.populate_from_level(&[light], &mut registry, 0);
        bridge.cached_influences[0].center = glam::Vec3::new(3.0e38, 0.0, 0.0);

        let mover = registry.spawn(Transform {
            position: glam::Vec3::new(3.0e38, 0.0, 0.0),
            ..Transform::default()
        });
        let light_entity = bridge.entity_for_map_index(0).unwrap();
        let mut component = registry
            .get_component::<LightComponent>(light_entity)
            .unwrap()
            .clone();
        component.carrier = Some(LightCarrier {
            mover_entity: mover,
            local_offset: glam::Vec3::new(3.0e38, 0.0, 0.0),
        });
        registry.set_component(light_entity, component).unwrap();

        let update = bridge
            .update(&mut registry, 0.0, 1.0)
            .expect("initial bridge update should pack the light");
        let packed_position = packed_dynamic_position(&update.lights_bytes);
        let packed_influence = packed_dynamic_influence_center(&update.influence_bytes);

        assert!(packed_position.is_finite());
        assert!(packed_influence.is_finite());
        let fallback_x = 3.0e38;
        let epsilon = fallback_x * 1.0e-6;
        assert!((packed_position.x - fallback_x).abs() <= epsilon);
        assert!((packed_influence.x - fallback_x).abs() <= epsilon);
    }

    #[test]
    fn carried_light_matches_mover_interpolation_after_zero_and_two_fixed_ticks() {
        use crate::kinematic_mover::{self, MoverTickStateTable};
        use postretro_entities::components::light::LightCarrier;
        use postretro_entities::{
            KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode,
        };

        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_dynamic_point_light()], &mut registry, 0);

        let mover = registry.spawn(Transform::default());
        registry
            .set_component(
                mover,
                KinematicMoverComponent::new(
                    12,
                    KinematicMoverConfig {
                        waypoints: vec![glam::Vec3::ZERO, glam::Vec3::new(4.0, 0.0, 0.0)],
                        waypoint_names: vec!["start".to_string(), "finish".to_string()],
                        speed_mps: 4.0,
                        wait_ms: 0.0,
                        mode: KinematicMoverMode::Once,
                        started: true,
                        spin_axis: glam::Vec3::ZERO,
                        initial_spin_rate_rad_s: 0.0,
                        spin_accel_rad_s2: 0.0,
                        carry_yaw: false,
                    },
                ),
            )
            .unwrap();
        let light = bridge.entity_for_map_index(0).unwrap();
        let mut component = registry
            .get_component::<LightComponent>(light)
            .unwrap()
            .clone();
        let local_offset = glam::Vec3::new(2.0, 1.0, 0.0);
        component.carrier = Some(LightCarrier {
            mover_entity: mover,
            local_offset,
        });
        registry.set_component(light, component).unwrap();

        // A render-only frame after load has no fixed tick history to advance.
        // The bridge must read the same spawn pose that mover geometry renders.
        let zero_tick_alpha = 0.73;
        let zero_tick_expected = registry
            .interpolated_transform(mover, zero_tick_alpha)
            .unwrap()
            .position
            + local_offset;
        let zero_tick_update = bridge
            .update(&mut registry, 0.0, zero_tick_alpha)
            .expect("first bridge update packs the bound carrier");
        assert!(
            packed_dynamic_position(&zero_tick_update.lights_bytes).distance(zero_tick_expected)
                <= 1.0e-6,
            "zero-tick render frame must compose from the mover's spawn pose"
        );

        // Catch-up can advance two fixed ticks before one render. Interpolation
        // must still use the renderer-visible previous/current pair after tick 2.
        let mut tick_states = MoverTickStateTable::default();
        for _ in 0..2 {
            registry.snapshot_transform(mover);
            kinematic_mover::run_kinematic_mover_tick(&mut registry, &mut tick_states, 0.25);
        }
        let two_tick_alpha = 0.25;
        let two_tick_expected = registry
            .interpolated_transform(mover, two_tick_alpha)
            .unwrap()
            .position
            + local_offset;
        let two_tick_update = bridge
            .update(&mut registry, 0.0, two_tick_alpha)
            .expect("mover movement makes the carrier upload dirty");
        assert!(
            packed_dynamic_position(&two_tick_update.lights_bytes).distance(two_tick_expected)
                <= 1.0e-6,
            "two-tick render frame must match geometry's interpolated mover pose"
        );
        assert!(
            packed_dynamic_influence_center(&two_tick_update.influence_bytes)
                .distance(two_tick_expected)
                <= 1.0e-6,
            "two-tick light influence must share the geometry-matched position"
        );
    }

    #[test]
    fn carried_light_tracks_ping_pong_reversal_and_stop_hold_without_snapping() {
        use crate::kinematic_mover::{self, MoverTickStateTable, apply_mover_command};
        use postretro_entities::components::light::LightCarrier;
        use postretro_entities::{
            KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode, MoverCommand,
        };

        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        let mut authored = sample_dynamic_point_light();
        authored.origin = [50.0, 50.0, 50.0];
        bridge.populate_from_level(&[authored], &mut registry, 0);
        let mover = registry.spawn(Transform::default());
        registry
            .set_component(
                mover,
                KinematicMoverComponent::new(
                    13,
                    KinematicMoverConfig {
                        waypoints: vec![glam::Vec3::ZERO, glam::Vec3::new(2.0, 0.0, 0.0)],
                        waypoint_names: vec!["start".to_string(), "finish".to_string()],
                        speed_mps: 1.0,
                        wait_ms: 0.0,
                        mode: KinematicMoverMode::PingPong,
                        started: true,
                        spin_axis: glam::Vec3::ZERO,
                        initial_spin_rate_rad_s: 0.0,
                        spin_accel_rad_s2: 0.0,
                        carry_yaw: false,
                    },
                ),
            )
            .unwrap();
        let light = bridge.entity_for_map_index(0).unwrap();
        let local_offset = glam::Vec3::new(0.0, 1.0, 0.0);
        let mut component = registry
            .get_component::<LightComponent>(light)
            .unwrap()
            .clone();
        component.carrier = Some(LightCarrier {
            mover_entity: mover,
            local_offset,
        });
        registry.set_component(light, component).unwrap();

        let mut tick_states = MoverTickStateTable::default();
        registry.snapshot_transform(mover);
        kinematic_mover::run_kinematic_mover_tick(&mut registry, &mut tick_states, 2.5);
        assert_eq!(
            registry
                .get_component::<KinematicMoverComponent>(mover)
                .unwrap()
                .direction_sign,
            -1,
            "the fixture must cross the ping-pong endpoint and reverse"
        );
        let reversal_alpha = 0.75;
        let reversal_expected = registry
            .interpolated_transform(mover, reversal_alpha)
            .unwrap()
            .position
            + local_offset;
        let reversal_update = bridge
            .update(&mut registry, 0.0, reversal_alpha)
            .expect("reversal movement repacks the carried light");
        assert!(
            packed_dynamic_position(&reversal_update.lights_bytes).distance(reversal_expected)
                <= 1.0e-6,
            "the light must remain on the interpolation path through reversal"
        );

        let mut mover_component = registry
            .get_component::<KinematicMoverComponent>(mover)
            .unwrap()
            .clone();
        apply_mover_command(&mut mover_component, &MoverCommand::Stop);
        registry.set_component(mover, mover_component).unwrap();
        registry.snapshot_transform(mover);
        kinematic_mover::run_kinematic_mover_tick(&mut registry, &mut tick_states, 0.5);
        let stop_expected = registry
            .interpolated_transform(mover, 0.5)
            .unwrap()
            .position
            + local_offset;
        let stop_update = bridge
            .update(&mut registry, 0.0, 0.5)
            .expect("the stop frame changes the followed pose from the reversal blend");
        assert!(
            packed_dynamic_position(&stop_update.lights_bytes).distance(stop_expected) <= 1.0e-6,
            "a stopped mover keeps its composed carrier position"
        );
        assert!(
            packed_dynamic_position(&stop_update.lights_bytes).distance(glam::Vec3::splat(50.0))
                > 1.0,
            "a stop must never snap the light back to its authored origin"
        );

        registry.snapshot_transform(mover);
        kinematic_mover::run_kinematic_mover_tick(&mut registry, &mut tick_states, 0.5);
        let held_update = bridge.update(&mut registry, 0.0, 0.5).unwrap();
        assert!(
            !held_update.has_dirty_data,
            "a stable stop hold preserves the last composed GPU upload"
        );
        let held_origin = bridge.collect_all_as_map_lights(&registry, 0.0)[0].0.origin;
        assert!(
            glam::Vec3::from_array(held_origin.map(|value| value as f32)).distance(stop_expected)
                <= 1.0e-6,
            "the bridge cache retains the stopped carrier pose rather than the authored origin"
        );
    }

    #[test]
    fn carried_light_holds_once_terminus_without_snapping_to_authored_origin() {
        use crate::kinematic_mover::{self, MoverTickStateTable};
        use postretro_entities::components::light::LightCarrier;
        use postretro_entities::{
            KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode,
        };

        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        let mut authored = sample_dynamic_point_light();
        authored.origin = [-40.0, 0.0, 0.0];
        bridge.populate_from_level(&[authored], &mut registry, 0);
        let mover = registry.spawn(Transform::default());
        registry
            .set_component(
                mover,
                KinematicMoverComponent::new(
                    14,
                    KinematicMoverConfig {
                        waypoints: vec![glam::Vec3::ZERO, glam::Vec3::new(2.0, 0.0, 0.0)],
                        waypoint_names: vec!["start".to_string(), "terminus".to_string()],
                        speed_mps: 1.0,
                        wait_ms: 0.0,
                        mode: KinematicMoverMode::Once,
                        started: true,
                        spin_axis: glam::Vec3::ZERO,
                        initial_spin_rate_rad_s: 0.0,
                        spin_accel_rad_s2: 0.0,
                        carry_yaw: false,
                    },
                ),
            )
            .unwrap();
        let light = bridge.entity_for_map_index(0).unwrap();
        let local_offset = glam::Vec3::new(0.0, 0.0, 1.0);
        let mut component = registry
            .get_component::<LightComponent>(light)
            .unwrap()
            .clone();
        component.carrier = Some(LightCarrier {
            mover_entity: mover,
            local_offset,
        });
        registry.set_component(light, component).unwrap();

        let mut tick_states = MoverTickStateTable::default();
        registry.snapshot_transform(mover);
        kinematic_mover::run_kinematic_mover_tick(&mut registry, &mut tick_states, 2.0);
        assert!(
            registry
                .get_component::<KinematicMoverComponent>(mover)
                .unwrap()
                .completed,
            "fixture must complete at the once terminus"
        );
        let terminus_update = bridge
            .update(&mut registry, 0.0, 1.0)
            .expect("completion frame packs the terminus pose");
        let terminus = glam::Vec3::new(2.0, 0.0, 1.0);
        assert!(
            packed_dynamic_position(&terminus_update.lights_bytes).distance(terminus) <= 1.0e-6,
            "completion frame must publish the composed terminus position"
        );

        registry.snapshot_transform(mover);
        kinematic_mover::run_kinematic_mover_tick(&mut registry, &mut tick_states, 0.5);
        let held_update = bridge.update(&mut registry, 0.0, 0.5).unwrap();
        assert!(
            !held_update.has_dirty_data,
            "a completed once mover holds its last bridge upload"
        );
        let held_origin = bridge.collect_all_as_map_lights(&registry, 0.0)[0].0.origin;
        assert!(
            glam::Vec3::from_array(held_origin.map(|value| value as f32)).distance(terminus)
                <= 1.0e-6,
            "once completion must hold at the terminus rather than snapping to authored origin"
        );
    }

    #[test]
    fn carried_spot_tracks_translating_mover_without_rotating_authored_aim() {
        use postretro_entities::components::light::LightCarrier;

        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_spot_light()], &mut registry, 0);

        let mover = registry.spawn(Transform {
            position: glam::Vec3::new(-6.0, 1.0, 4.0),
            ..Transform::default()
        });
        registry.snapshot_transform(mover);
        registry
            .set_component(
                mover,
                Transform {
                    position: glam::Vec3::new(2.0, 5.0, -4.0),
                    ..Transform::default()
                },
            )
            .unwrap();

        let local_offset = glam::Vec3::new(1.5, -0.5, 2.0);
        let light = bridge.entity_for_map_index(0).unwrap();
        let mut component = registry
            .get_component::<LightComponent>(light)
            .unwrap()
            .clone();
        let authored_aim = glam::Vec3::from_array(component.cone_direction.unwrap());
        component.carrier = Some(LightCarrier {
            mover_entity: mover,
            local_offset,
        });
        registry.set_component(light, component).unwrap();

        let update = bridge
            .update(&mut registry, 0.0, 0.25)
            .expect("carried spot is packed");
        let interpolated_mover_position = glam::Vec3::new(-4.0, 2.0, 2.0);
        let expected_position = interpolated_mover_position + local_offset;

        assert!(
            packed_dynamic_position(&update.lights_bytes).distance(expected_position) <= 1.0e-6,
            "carried spot position must use the translating mover's interpolated pose"
        );
        assert!(
            packed_dynamic_influence_center(&update.influence_bytes).distance(expected_position)
                <= 1.0e-6,
            "the carried spot's culling influence must follow its packed position"
        );
        assert!(
            packed_dynamic_direction(&update.lights_bytes).distance(authored_aim) <= 1.0e-6,
            "a translating mover must not rotate a carried spot's authored world-space cone aim"
        );
    }

    #[test]
    fn carried_omni_orbits_spinning_mover_at_authored_offset() {
        use postretro_entities::components::light::LightCarrier;

        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_dynamic_point_light()], &mut registry, 0);

        let mover = registry.spawn(Transform {
            position: glam::Vec3::new(3.0, 2.0, -1.0),
            ..Transform::default()
        });
        registry.snapshot_transform(mover);
        registry
            .set_component(
                mover,
                Transform {
                    position: glam::Vec3::new(3.0, 2.0, -1.0),
                    rotation: glam::Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
                    ..Transform::default()
                },
            )
            .unwrap();

        let local_offset = glam::Vec3::new(4.0, 0.0, 0.0);
        let light = bridge.entity_for_map_index(0).unwrap();
        let mut component = registry
            .get_component::<LightComponent>(light)
            .unwrap()
            .clone();
        component.carrier = Some(LightCarrier {
            mover_entity: mover,
            local_offset,
        });
        registry.set_component(light, component).unwrap();

        let update = bridge
            .update(&mut registry, 0.0, 0.5)
            .expect("carried omni is packed");
        let expected_position = glam::Vec3::new(3.0, 2.0, -1.0)
            + glam::Quat::from_rotation_y(std::f32::consts::FRAC_PI_4) * local_offset;

        assert!(
            packed_dynamic_position(&update.lights_bytes).distance(expected_position) <= 1.0e-6,
            "a carried omni must orbit with the mover's interpolated rotation"
        );
        assert!(
            packed_dynamic_influence_center(&update.influence_bytes).distance(expected_position)
                <= 1.0e-6,
            "the orbiting omni's culling influence must share its moved center"
        );
        assert!(
            (packed_dynamic_position(&update.lights_bytes)
                .distance(glam::Vec3::new(3.0, 2.0, -1.0))
                - local_offset.length())
            .abs()
                <= 1.0e-6,
            "the spinning mover must preserve the omni's authored orbit radius"
        );
    }

    #[test]
    fn far_moved_carried_light_uses_relocated_influence_for_culling_without_mover_draw() {
        use postretro_entities::components::light::LightCarrier;

        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        let mut light = sample_dynamic_point_light();
        light.origin = [-80.0, 0.0, 0.0];
        light.falloff_range = 6.0;
        light.cell_index = 91; // Deliberately stale after the carrier moves.
        bridge.populate_from_level_with_influences(
            &[light],
            &[LightInfluence {
                center: glam::Vec3::new(-80.0, 0.0, 0.0),
                radius: 6.0,
            }],
            &[],
            &mut registry,
            0,
        );

        // No Mesh/SpriteVisual is attached: this mover has no beauty draw to
        // keep the light alive. The bridge must still read its Transform.
        let mover = registry.spawn(Transform {
            position: glam::Vec3::new(100.0, 0.0, 0.0),
            ..Transform::default()
        });
        let light_entity = bridge.entity_for_map_index(0).unwrap();
        let mut component = registry
            .get_component::<LightComponent>(light_entity)
            .unwrap()
            .clone();
        component.carrier = Some(LightCarrier {
            mover_entity: mover,
            local_offset: glam::Vec3::new(1.0, 0.0, 0.0),
        });
        registry.set_component(light_entity, component).unwrap();

        let update = bridge
            .update(&mut registry, 0.0, 0.0)
            .expect("far-moved carried light is packed");
        let moved_center = packed_dynamic_influence_center(&update.influence_bytes);
        let reachable_receiver = [(
            glam::Vec3::new(99.0, -1.0, -1.0),
            glam::Vec3::new(103.0, 1.0, 1.0),
        )];

        assert!(
            packed_dynamic_position(&update.lights_bytes).distance(moved_center) <= 1.0e-6,
            "the direct-light record and culling influence must agree on the carried position"
        );
        assert!(
            postretro_lighting::light_reaches_visible_cell(
                moved_center,
                packed_dynamic_influence_radius(&update),
                &reachable_receiver,
            ),
            "the moved influence, not the stale authored cell or origin, keeps a reachable receiver lit"
        );
        assert!(
            !postretro_lighting::light_reaches_visible_cell(
                glam::Vec3::new(-80.0, 0.0, 0.0),
                packed_dynamic_influence_radius(&update),
                &reachable_receiver,
            ),
            "this fixture must distinguish relocated influence culling from the stale authored origin"
        );
    }

    #[test]
    fn runtime_light_conversion_disables_entity_shadows() {
        let component = runtime_component([1.0, 2.0, 3.0], 5.0, None);
        let converted = component_to_map_light(&component, [1.0, 2.0, 3.0], true, u32::MAX);

        assert!(converted.is_dynamic);
        assert!(
            !converted.casts_entity_shadows,
            "projectile runtime lights must stay outside the entity-shadow pool"
        );
    }

    #[test]
    fn radius_animation_repacks_growing_and_shrinking_range_with_influence() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_dynamic_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0, 0.0);

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
            brightness: None,
            color: None,
            direction: None,
            radius: Some(vec![4.0, 12.0]),
        });
        registry.set_component(id, component).unwrap();

        let start = bridge.update(&mut registry, 0.0, 0.0).unwrap();
        assert!(start.has_dirty_data);
        assert_packed_radius_in_lockstep(&start, 4.0);

        let grown = bridge.update(&mut registry, 0.5, 0.0).unwrap();
        assert!(
            grown.has_dirty_data,
            "an active radius curve must re-pack even while the light is stationary"
        );
        assert_packed_radius_in_lockstep(&grown, 12.0);
        let grown_map_light = bridge.collect_all_as_map_lights(&registry, 0.5);
        assert!(
            (grown_map_light[0].0.falloff_range - 12.0).abs() < 1e-6,
            "CPU MapLight consumers must receive the same current radius"
        );

        let shrunk = bridge.update(&mut registry, 1.0, 0.0).unwrap();
        assert!(shrunk.has_dirty_data);
        assert_packed_radius_in_lockstep(&shrunk, 4.0);
    }

    #[test]
    fn radius_none_keeps_static_range_and_influence_bytes_identical() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_dynamic_point_light()], &mut registry, 0);
        let static_update = bridge.update(&mut registry, 0.0, 0.0).unwrap();

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
            brightness: Some(vec![0.0, 1.0, 0.0]),
            color: None,
            direction: None,
            radius: None,
        });
        registry.set_component(id, component).unwrap();

        let animated_without_radius = bridge.update(&mut registry, 0.0, 0.0).unwrap();
        assert_eq!(
            animated_without_radius.lights_bytes,
            static_update.lights_bytes
        );
        assert_eq!(
            animated_without_radius.influence_bytes, static_update.influence_bytes,
            "a missing radius curve must retain the existing culling volume byte-for-byte"
        );
    }

    #[test]
    fn finite_radius_animation_settles_final_range_and_influence() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_dynamic_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0, 0.0);

        let id = bridge.entity_for_map_index(0).unwrap();
        let mut component = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        component.falloff_range = 3.0;
        component.animation = Some(LightAnimation {
            period_ms: 1000.0,
            phase: None,
            play_count: Some(1),
            start_active: None,
            brightness: None,
            color: None,
            direction: None,
            radius: Some(vec![3.0, 18.0]),
        });
        registry.set_component(id, component).unwrap();

        let initial = bridge.update(&mut registry, 0.0, 0.0).unwrap();
        assert_packed_radius_in_lockstep(&initial, 3.0);

        let settled_update = bridge.update(&mut registry, 1.01, 0.0).unwrap();
        let settled_component = registry.get_component::<LightComponent>(id).unwrap();
        assert!(settled_component.animation.is_none());
        assert!(
            (settled_component.falloff_range - 18.0).abs() < 1e-6,
            "finite completion must write the final radius back to static component state"
        );
        assert_packed_radius_in_lockstep(&settled_update, 18.0);
    }

    #[test]
    fn mutating_intensity_in_registry_produces_repacked_upload_within_one_frame() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_dynamic_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0, 0.0); // flush initial

        let id = bridge.entity_for_map_index(0).unwrap();
        let mut component = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        component.intensity = 7.5;
        registry.set_component(id, component).unwrap();

        let update = bridge
            .update(&mut registry, 0.016, 0.0)
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
        let _ = bridge.update(&mut registry, 0.0, 0.0);

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
            radius: None,
        });
        registry.set_component(id, component).unwrap();

        let update = bridge.update(&mut registry, 0.0, 0.0).expect("dirty");
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
            .update(&mut registry, 0.1, 0.0)
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
        let _ = bridge.update(&mut registry, 0.0, 0.0);

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
            radius: None,
        });
        registry.set_component(id, component).unwrap();

        // Animate starts at t=1.0; completion bound = 2 × 0.5s, fires at t=2.0.
        let _ = bridge.update(&mut registry, 1.0, 0.0);

        let second_period = bridge.update(&mut registry, 1.5, 0.0).unwrap();
        assert!(
            second_period.has_dirty_data,
            "finite descriptors reset to endpoint-clamped sampling each period"
        );
        let packed_period =
            f32::from_ne_bytes(second_period.descriptor_bytes[0..4].try_into().unwrap());
        assert!(
            packed_period < 0.0,
            "negative period selects endpoint-clamped GPU sampling"
        );
        let mid = registry.get_component::<LightComponent>(id).unwrap();
        assert!(
            mid.animation.is_some(),
            "animation still live before completion bound"
        );

        let near_completion = bridge.update(&mut registry, 1.999, 0.0).unwrap();
        assert!(
            (near_completion.effective_brightness[0] - 0.25).abs() < 0.01,
            "finite sampling must approach the final brightness before settlement; got {}",
            near_completion.effective_brightness[0]
        );

        let completed = bridge.update(&mut registry, 2.01, 0.0).unwrap();
        let settled = registry.get_component::<LightComponent>(id).unwrap();
        assert!(
            settled.animation.is_none(),
            "animation cleared on completion"
        );
        assert!(
            (settled.intensity - 0.375).abs() < 1e-6,
            "settled intensity must preserve authored 1.5 × final brightness 0.25; got {}",
            settled.intensity
        );
        assert_eq!(settled.color, [0.0, 0.0, 1.0]);
        let packed_r = f32::from_le_bytes(completed.lights_bytes[16..20].try_into().unwrap());
        let packed_g = f32::from_le_bytes(completed.lights_bytes[20..24].try_into().unwrap());
        let packed_b = f32::from_le_bytes(completed.lights_bytes[24..28].try_into().unwrap());
        assert_eq!(
            [packed_r, packed_g, packed_b],
            [0.0, 0.0, 0.375],
            "forward record must settle to the same final radiance"
        );
    }

    #[test]
    fn slot_bearing_one_shot_settles_compose_descriptor_to_final_radiance() {
        let mut light = sample_point_light();
        light.intensity = 2.0;
        light.color = [0.5, 0.25, 0.125];
        light.animated_slot = Some(2);
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[light], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0, 0.0);

        let id = bridge.entity_for_map_index(0).unwrap();
        let mut component = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        component.animation = Some(LightAnimation {
            period_ms: 1000.0,
            phase: None,
            play_count: Some(1),
            start_active: None,
            brightness: Some(vec![1.0, 0.25]),
            color: Some(vec![Vec3Lit([1.0, 0.0, 0.0]), Vec3Lit([0.0, 0.0, 1.0])]),
            direction: None,
            radius: None,
        });
        registry.set_component(id, component).unwrap();

        let playing = bridge.update(&mut registry, 1.0, 0.0).unwrap();
        assert!(
            f32::from_ne_bytes(
                playing.compose_descriptor_writes[0].1[0..4]
                    .try_into()
                    .unwrap()
            ) < 0.0,
            "finite compose descriptors must use endpoint-clamped sampling"
        );

        let settled = bridge.update(&mut registry, 2.01, 0.0).unwrap();
        let (slot, descriptor) = &settled.compose_descriptor_writes[0];
        assert_eq!(*slot, 2);
        assert_eq!(
            u32::from_ne_bytes(descriptor[36..40].try_into().unwrap()),
            1,
            "settled slot stays active"
        );
        let final_radiance = [
            f32::from_ne_bytes(descriptor[16..20].try_into().unwrap()),
            f32::from_ne_bytes(descriptor[20..24].try_into().unwrap()),
            f32::from_ne_bytes(descriptor[24..28].try_into().unwrap()),
        ];
        assert_eq!(final_radiance, [0.0, 0.0, 0.5]);
    }

    #[test]
    fn setanimation_restart_resets_play_count_clock() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_dynamic_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0, 0.0);
        let id = bridge.entity_for_map_index(0).unwrap();

        let make_anim = || LightAnimation {
            period_ms: 500.0,
            phase: None,
            play_count: Some(2),
            start_active: None,
            brightness: Some(vec![1.0, 0.25]),
            color: None,
            direction: None,
            radius: None,
        };

        let mut comp = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        comp.animation = Some(make_anim());
        registry.set_component(id, comp).unwrap();
        let _ = bridge.update(&mut registry, 0.0, 0.0);

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
        let _ = bridge.update(&mut registry, 0.6, 0.0);

        // t=1.1 would fire with the original clock (started at 0.0) but not
        // with the restarted clock (started at 0.6, completion at 1.6).
        let _ = bridge.update(&mut registry, 1.1, 0.0);
        assert!(
            registry
                .get_component::<LightComponent>(id)
                .unwrap()
                .animation
                .is_some(),
            "restart must reset completion clock; animation should still be live at t=1.1"
        );

        let _ = bridge.update(&mut registry, 1.7, 0.0);
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
            follow_transform: false,
            carrier: None,
            animation: Some(LightAnimation {
                period_ms: 500.0,
                phase: None,
                play_count: None,
                start_active: Some(false),
                brightness: Some(vec![0.1, 1.0]),
                color: None,
                direction: None,
                radius: None,
            }),
        };
        let bytes =
            pack_forward_animation_descriptor(&component, None, 0, SCRIPTED_BRIGHTNESS_SLOT as u32);
        let active = u32::from_ne_bytes(bytes[36..40].try_into().unwrap());
        assert_eq!(active, 0, "start_active: Some(false) must pack as inactive");
    }

    #[test]
    fn phase_outside_unit_interval_is_wrapped_via_rem_euclid_in_descriptor() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_dynamic_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0, 0.0);
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
            radius: None,
        });
        registry.set_component(id, comp).unwrap();
        let update = bridge.update(&mut registry, 0.0, 0.0).expect("dirty");
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
        let _ = bridge.update(&mut registry, 0.0, 0.0);
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
            radius: None,
        });
        registry.set_component(id, comp).unwrap();
        let _ = bridge.update(&mut registry, 0.0, 0.0);
        let _ = bridge.update(&mut registry, 0.2, 0.0); // past completion
        let idle1 = bridge.update(&mut registry, 0.3, 0.0).unwrap();
        assert!(
            !idle1.has_dirty_data,
            "settled idle frame must not re-upload"
        );
        let idle2 = bridge.update(&mut registry, 10.0, 0.0).unwrap();
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
            follow_transform: false,
            carrier: None,
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
        let scans_after_enrollment = bridge.enrollment_scan_count;
        bridge.absorb_dynamic_lights(&registry);
        assert_eq!(bridge.light_count(), 2);
        assert_eq!(bridge.enrollment_scan_count, scans_after_enrollment);

        // Next update produces a GPU upload that includes both lights.
        let update = bridge.update(&mut registry, 0.0, 0.0).expect("dirty");
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
        let _ = bridge.update(&mut registry, 0.0, 0.0);

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
            radius: None,
        });
        registry.set_component(id, comp).unwrap();
        let _ = bridge.update(&mut registry, 0.0, 0.0); // flush dirty frame

        let dark = bridge.update(&mut registry, 0.5, 0.0).unwrap();
        assert!(
            !dark.has_dirty_data,
            "no mutation, GPU buffers must not re-upload"
        );
        assert!(
            dark.effective_brightness[0] < 0.01,
            "light is dark at T=0.5s; effective_brightness must reflect live curve; got {}",
            dark.effective_brightness[0]
        );

        let bright = bridge.update(&mut registry, 1.0, 0.0).unwrap();
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
        let initial = bridge
            .update(&mut registry, 0.0, 0.0)
            .expect("initial dirty");
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
            radius: None,
        });
        registry.set_component(id, component).unwrap();

        let update = bridge
            .update(&mut registry, 0.0, 0.0)
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

        let update = bridge
            .update(&mut registry, 0.0, 0.0)
            .expect("initial dirty");

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
        let update = bridge.update(&mut registry, 0.0, 0.0).unwrap();
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
        let _ = bridge.update(&mut registry, 0.0, 0.0);

        let id = bridge.entity_for_map_index(0).unwrap();
        let mut component = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        component.animation = Some(sample_animation());
        registry.set_component(id, component).unwrap();

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            let _ = bridge.update(&mut registry, 0.1, 0.0);

            // A subsequent dirty update while the same animation remains live
            // must not repeat the author-facing warning.
            let mut component = registry
                .get_component::<LightComponent>(id)
                .unwrap()
                .clone();
            component.intensity = 2.0;
            registry.set_component(id, component).unwrap();
            let _ = bridge.update(&mut registry, 0.2, 0.0);
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
        let _ = bridge.update(&mut registry, 0.0, 0.0);

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
            let _ = bridge.update(&mut registry, 0.1, 0.0);
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
        let _ = bridge.update(&mut registry, 0.0, 0.0);

        let id = registry.try_spawn(Default::default(), &[]).unwrap();
        let mut component = map_light_to_component(&sample_dynamic_point_light(), None);
        component.animation = Some(sample_animation());
        registry.set_component(id, component).unwrap();
        bridge.absorb_dynamic_lights(&registry);

        let update = bridge
            .update(&mut registry, 0.0, 0.0)
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
        let _ = bridge.update(&mut registry, 0.0, 0.0);

        registry.despawn(id).unwrap();
        let update = bridge
            .update(&mut registry, 0.1, 0.0)
            .expect("despawn dirty");
        assert!(update.has_dirty_data);
        assert_eq!(update.lights_bytes, vec![0; GPU_LIGHT_SIZE]);
        assert_eq!(update.descriptor_bytes, vec![0; ANIMATION_DESCRIPTOR_SIZE]);
        assert_eq!(update.effective_brightness, vec![0.0]);

        assert!(
            !bridge
                .update(&mut registry, 0.2, 0.0)
                .unwrap()
                .has_dirty_data
        );
    }

    // Regression: connected clients skip authoritative simulate_tick, so their
    // predicted and observer impact lights never advanced their deferred despawn.
    #[test]
    fn connected_client_impact_lights_advance_and_reclaim_runtime_slots() {
        let mut registry = EntityRegistry::new();
        let config = postretro_foundation::ProjectileImpactLight {
            color: [0.4, 0.8, 1.0],
            intensity: 4.0,
            radius: 6.0,
            peak_radius: None,
            fade_ms: 10.0,
        };
        crate::weapon::spawn_projectile_impact_light(
            &mut registry,
            glam::Vec3::new(1.0, 0.0, 0.0),
            &config,
        );
        crate::weapon::spawn_projectile_impact_light(
            &mut registry,
            glam::Vec3::new(2.0, 0.0, 0.0),
            &config,
        );

        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[], &mut registry, 0);
        bridge.absorb_dynamic_lights(&registry);
        let _ = bridge.update(&mut registry, 0.0, 0.0);
        assert_eq!(live_runtime_count(&bridge), 2);

        crate::sim::advance_client_presentation_effects(&mut registry, 0.020);
        crate::impact_effects::run_end_of_frame_removal_pass(&mut registry, |_, _| {});
        let update = bridge
            .update(&mut registry, 0.020, 0.0)
            .expect("client-side despawns dirty the bridge");

        assert_eq!(registry.iter_with_kind(ComponentKind::Light).count(), 0);
        assert_eq!(live_runtime_count(&bridge), 0);
        assert_eq!(bridge.free_slots.len(), 2);
        assert_eq!(update.lights_bytes, vec![0; 2 * GPU_LIGHT_SIZE]);
    }

    // Regression: slot-bearing baked lights have no dynamic-forward record.
    // Their compose descriptor must still be tombstoned on despawn or the
    // renderer keeps composing the stale baked delta.
    #[test]
    fn despawned_slot_bearing_static_light_emits_compose_tombstone() {
        let mut light = sample_point_light();
        light.animated_slot = Some(5);
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[light], &mut registry, 0);
        let id = bridge.entity_for_map_index(0).unwrap();
        let _ = bridge.update(&mut registry, 0.0, 0.0);

        registry.despawn(id).unwrap();
        let update = bridge
            .update(&mut registry, 0.1, 0.0)
            .expect("despawn dirty");

        assert!(update.has_dirty_data);
        assert!(update.lights_bytes.is_empty());
        assert!(update.descriptor_bytes.is_empty());
        assert_eq!(
            update.compose_descriptor_writes,
            vec![(5, [0u8; ANIMATION_DESCRIPTOR_SIZE])]
        );
        assert!(
            !bridge
                .update(&mut registry, 0.2, 0.0)
                .unwrap()
                .has_dirty_data
        );
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

        let initial = bridge.update(&mut registry, 0.0, 0.0).unwrap();
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
        let _ = bridge.update(&mut registry, 0.1, 0.0);

        let mut component = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        component.animation = None;
        registry.set_component(id, component).unwrap();
        let cleared = bridge.update(&mut registry, 0.2, 0.0).unwrap();
        let cleared_desc = &cleared.compose_descriptor_writes[0].1;
        assert_eq!(
            u32::from_ne_bytes(cleared_desc[36..40].try_into().unwrap()),
            1
        );
        assert!((f32::from_ne_bytes(cleared_desc[16..20].try_into().unwrap()) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn runtime_light_churn_far_past_reserve_reuses_peak_concurrent_slots() {
        const CONCURRENT_LIGHTS: usize = 3;
        assert!(CONCURRENT_LIGHTS < RUNTIME_DYNAMIC_LIGHT_RESERVE);

        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0, 0.0);

        for cycle in 0..(RUNTIME_DYNAMIC_LIGHT_RESERVE * 2) {
            let ids: Vec<_> = (0..CONCURRENT_LIGHTS)
                .map(|offset| {
                    spawn_runtime_light(
                        &mut registry,
                        runtime_component([cycle as f32, offset as f32, 0.0], 8.0, None),
                    )
                })
                .collect();
            bridge.absorb_dynamic_lights(&registry);
            assert_eq!(live_runtime_count(&bridge), CONCURRENT_LIGHTS);

            let live = bridge
                .update(&mut registry, cycle as f32 + 0.1, 0.0)
                .unwrap();
            assert_eq!(live.lights_bytes.len(), CONCURRENT_LIGHTS * GPU_LIGHT_SIZE);
            assert!(
                live.lights_bytes
                    .chunks_exact(GPU_LIGHT_SIZE)
                    .all(|record| record.iter().any(|&byte| byte != 0)),
                "every live runtime light must produce a non-zero forward record"
            );

            for id in ids {
                registry.despawn(id).unwrap();
            }
            let tombstones = bridge
                .update(&mut registry, cycle as f32 + 0.2, 0.0)
                .unwrap();
            assert_eq!(
                tombstones.lights_bytes.len(),
                CONCURRENT_LIGHTS * GPU_LIGHT_SIZE
            );
            assert!(tombstones.lights_bytes.iter().all(|&byte| byte == 0));
            assert_eq!(bridge.free_slots.len(), CONCURRENT_LIGHTS);
        }

        assert_eq!(
            bridge.entity_ids.len(),
            bridge.authored_light_count + CONCURRENT_LIGHTS
        );
    }

    #[test]
    fn zero_duration_unsnapshotted_runtime_despawn_reclaims_and_tombstones() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0, 0.0);

        let mut zero_duration = sample_animation();
        zero_duration.period_ms = 0.0;
        zero_duration.play_count = Some(1);
        let id = spawn_runtime_light(
            &mut registry,
            runtime_component([7.0, 8.0, 9.0], 10.0, Some(zero_duration)),
        );
        bridge.absorb_dynamic_lights(&registry);
        let runtime_slot = bridge.authored_light_count;

        registry.despawn(id).unwrap();
        let update = bridge.update(&mut registry, 0.1, 0.0).unwrap();

        assert!(update.has_dirty_data);
        assert_eq!(update.lights_bytes, vec![0; GPU_LIGHT_SIZE]);
        assert_eq!(update.descriptor_bytes, vec![0; ANIMATION_DESCRIPTOR_SIZE]);
        assert_eq!(bridge.free_slots, vec![runtime_slot]);
        assert!(bridge.shape[runtime_slot].reclaimed);
    }

    #[test]
    fn disappeared_runtime_light_emits_forward_tombstone_in_its_reclaim_frame() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0, 0.0);

        let id = spawn_runtime_light(&mut registry, runtime_component([1.0, 2.0, 3.0], 9.0, None));
        let survivor =
            spawn_runtime_light(&mut registry, runtime_component([4.0, 5.0, 6.0], 7.0, None));
        bridge.absorb_dynamic_lights(&registry);
        let _ = bridge.update(&mut registry, 0.1, 0.0);

        registry.despawn(id).unwrap();
        let disappeared = bridge.update(&mut registry, 0.2, 0.0).unwrap();
        assert_eq!(
            &disappeared.lights_bytes[..GPU_LIGHT_SIZE],
            &[0; GPU_LIGHT_SIZE]
        );
        assert_eq!(
            &disappeared.descriptor_bytes[..ANIMATION_DESCRIPTOR_SIZE],
            &[0; ANIMATION_DESCRIPTOR_SIZE]
        );
        assert_eq!(disappeared.effective_brightness, vec![0.0, 1.0]);
        assert!(bridge.shape[bridge.authored_light_count].is_dynamic);
        assert!(bridge.shape[bridge.authored_light_count].reclaimed);

        let mut survivor_component = registry
            .get_component::<LightComponent>(survivor)
            .unwrap()
            .clone();
        survivor_component.intensity = 12.0;
        registry
            .set_component(survivor, survivor_component)
            .unwrap();
        let repacked = bridge.update(&mut registry, 0.3, 0.0).unwrap();
        assert!(repacked.has_dirty_data);
        assert_eq!(
            &repacked.lights_bytes[..GPU_LIGHT_SIZE],
            &[0; GPU_LIGHT_SIZE]
        );
    }

    #[test]
    fn disappeared_slot_bearing_dynamic_light_keeps_forward_and_compose_tombstones() {
        let mut slot_bearing = sample_dynamic_point_light();
        slot_bearing.animated_slot = Some(6);
        let survivor = sample_dynamic_point_light();
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[slot_bearing, survivor], &mut registry, 0);
        let missing_id = bridge.entity_for_map_index(0).unwrap();
        let survivor_id = bridge.entity_for_map_index(1).unwrap();
        let _ = bridge.update(&mut registry, 0.0, 0.0);

        registry.despawn(missing_id).unwrap();
        let disappeared = bridge.update(&mut registry, 0.1, 0.0).unwrap();
        assert_eq!(
            &disappeared.lights_bytes[..GPU_LIGHT_SIZE],
            &[0; GPU_LIGHT_SIZE]
        );
        assert_eq!(
            disappeared.compose_descriptor_writes,
            vec![(6, [0u8; ANIMATION_DESCRIPTOR_SIZE])]
        );

        let mut survivor_component = registry
            .get_component::<LightComponent>(survivor_id)
            .unwrap()
            .clone();
        survivor_component.intensity = 3.0;
        registry
            .set_component(survivor_id, survivor_component)
            .unwrap();
        let repacked = bridge.update(&mut registry, 0.2, 0.0).unwrap();
        assert_eq!(
            &repacked.lights_bytes[..GPU_LIGHT_SIZE],
            &[0; GPU_LIGHT_SIZE]
        );
        assert_eq!(
            repacked.compose_descriptor_writes,
            vec![(6, [0u8; ANIMATION_DESCRIPTOR_SIZE])]
        );
    }

    #[test]
    fn runtime_reclamation_leaves_authored_forward_and_compose_output_unchanged() {
        let mut static_slot_light = sample_point_light();
        static_slot_light.animated_slot = Some(4);
        let authored_dynamic = sample_dynamic_point_light();
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[authored_dynamic, static_slot_light], &mut registry, 0);
        let initial = bridge.update(&mut registry, 0.0, 0.0).unwrap();
        let authored_forward = initial.lights_bytes.clone();
        let authored_compose = initial.compose_descriptor_writes.clone();

        let first =
            spawn_runtime_light(&mut registry, runtime_component([4.0, 5.0, 6.0], 7.0, None));
        bridge.absorb_dynamic_lights(&registry);
        let with_runtime = bridge.update(&mut registry, 0.1, 0.0).unwrap();
        assert_eq!(
            &with_runtime.lights_bytes[..GPU_LIGHT_SIZE],
            authored_forward.as_slice()
        );
        assert_eq!(with_runtime.compose_descriptor_writes, authored_compose);

        registry.despawn(first).unwrap();
        let tombstone = bridge.update(&mut registry, 0.2, 0.0).unwrap();
        assert_eq!(
            &tombstone.lights_bytes[..GPU_LIGHT_SIZE],
            authored_forward.as_slice()
        );
        assert_eq!(tombstone.compose_descriptor_writes, authored_compose);

        let second = spawn_runtime_light(
            &mut registry,
            runtime_component([6.0, 5.0, 4.0], 11.0, None),
        );
        bridge.absorb_dynamic_lights(&registry);
        let reused = bridge.update(&mut registry, 0.3, 0.0).unwrap();
        assert_eq!(
            &reused.lights_bytes[..GPU_LIGHT_SIZE],
            authored_forward.as_slice()
        );
        assert_eq!(reused.compose_descriptor_writes, authored_compose);
        assert_eq!(
            bridge.entity_for_map_index(bridge.authored_light_count),
            Some(second)
        );
    }

    #[test]
    fn reused_runtime_slot_overwrites_origin_influence_and_script_samples() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0, 0.0);

        let old = spawn_runtime_light(
            &mut registry,
            runtime_component([1.0, 2.0, 3.0], 19.0, Some(sample_animation())),
        );
        bridge.absorb_dynamic_lights(&registry);
        let populated = bridge.update(&mut registry, 0.1, 0.0).unwrap();
        assert!(populated.samples_bytes.iter().any(|&byte| byte != 0));

        registry.despawn(old).unwrap();
        let _ = bridge.update(&mut registry, 0.2, 0.0);

        let new_component = runtime_component([-4.0, 5.0, -6.0], 3.5, None);
        let new = spawn_runtime_light(&mut registry, new_component.clone());
        bridge.absorb_dynamic_lights(&registry);
        let reused = bridge.update(&mut registry, 0.3, 0.0).unwrap();
        let runtime_slot = bridge.authored_light_count;

        assert_eq!(bridge.entity_ids[runtime_slot], new);
        for (actual, expected) in bridge.cached_origins_f64[runtime_slot]
            .iter()
            .zip([-4.0, 5.0, -6.0])
        {
            assert!((actual - expected).abs() < 1e-6);
        }
        assert!(
            (bridge.cached_influences[runtime_slot].center - glam::Vec3::new(-4.0, 5.0, -6.0))
                .length()
                < 1e-6
        );
        assert!((bridge.cached_influences[runtime_slot].radius - 3.5).abs() < 1e-6);
        let expected = component_to_map_light(&new_component, [-4.0, 5.0, -6.0], true, u32::MAX);
        assert_eq!(reused.lights_bytes, pack_light(&expected).to_vec());
        assert_eq!(
            reused.influence_bytes,
            postretro_lighting::influence::pack_influence(&[component_to_influence(
                &new_component
            )])
        );
        assert!(reused.samples_bytes.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn runtime_reserve_counts_live_slots_and_accepts_reclaimed_boundary_slot() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0, 0.0);

        let ids: Vec<_> = (0..RUNTIME_DYNAMIC_LIGHT_RESERVE)
            .map(|index| {
                spawn_runtime_light(
                    &mut registry,
                    runtime_component([index as f32, 0.0, 0.0], 4.0, None),
                )
            })
            .collect();
        bridge.absorb_dynamic_lights(&registry);
        assert_eq!(live_runtime_count(&bridge), RUNTIME_DYNAMIC_LIGHT_RESERVE);
        let _ = bridge.update(&mut registry, 0.1, 0.0);

        let overflow = spawn_runtime_light(
            &mut registry,
            runtime_component([999.0, 0.0, 0.0], 4.0, None),
        );
        bridge.absorb_dynamic_lights(&registry);
        assert!(!bridge.entity_ids.contains(&overflow));

        registry.despawn(ids[0]).unwrap();
        let _ = bridge.update(&mut registry, 0.2, 0.0);
        assert_eq!(
            live_runtime_count(&bridge),
            RUNTIME_DYNAMIC_LIGHT_RESERVE - 1
        );
        assert_eq!(bridge.free_slots.len(), 1);

        bridge.absorb_dynamic_lights(&registry);
        assert_eq!(live_runtime_count(&bridge), RUNTIME_DYNAMIC_LIGHT_RESERVE);
        assert!(bridge.entity_ids.contains(&overflow));
    }

    #[test]
    fn runtime_reserve_overflow_warns_once_without_disturbing_enrolled_lights() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[], &mut registry, 0);
        let ids: Vec<_> = (0..=RUNTIME_DYNAMIC_LIGHT_RESERVE)
            .map(|index| {
                spawn_runtime_light(
                    &mut registry,
                    runtime_component([index as f32, 0.0, 0.0], 4.0, None),
                )
            })
            .collect();

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            bridge.absorb_dynamic_lights(&registry);
            bridge.absorb_dynamic_lights(&registry);
        });
        let warnings = captured
            .iter()
            .filter(|(level, message)| {
                *level == log::Level::Warn && message.contains("runtime dynamic-light reserve")
            })
            .count();
        assert_eq!(warnings, 1, "reserve exhaustion has one actionable warning");
        assert_eq!(live_runtime_count(&bridge), RUNTIME_DYNAMIC_LIGHT_RESERVE);
        assert_eq!(
            bridge.entity_for_map_index(RUNTIME_DYNAMIC_LIGHT_RESERVE - 1),
            Some(ids[RUNTIME_DYNAMIC_LIGHT_RESERVE - 1]),
            "the last admitted light retains its stable slot"
        );
        assert!(
            !bridge
                .entity_ids
                .contains(&ids[RUNTIME_DYNAMIC_LIGHT_RESERVE]),
            "the surplus light is dropped without corrupting admitted lights"
        );

        let update = bridge
            .update(&mut registry, 0.0, 0.0)
            .expect("enrolled runtime lights pack normally");
        assert_eq!(
            update.lights_bytes.len(),
            RUNTIME_DYNAMIC_LIGHT_RESERVE * GPU_LIGHT_SIZE
        );
    }

    #[test]
    fn reserve_full_same_frame_despawn_and_spawn_reuses_reclaimed_slot_immediately() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0, 0.0);

        let ids: Vec<_> = (0..RUNTIME_DYNAMIC_LIGHT_RESERVE)
            .map(|index| {
                spawn_runtime_light(
                    &mut registry,
                    runtime_component([index as f32, 0.0, 0.0], 4.0, None),
                )
            })
            .collect();
        bridge.absorb_dynamic_lights(&registry);
        let _ = bridge.update(&mut registry, 0.1, 0.0);

        registry.despawn(ids[0]).unwrap();
        let replacement = spawn_runtime_light(
            &mut registry,
            runtime_component([777.0, 0.0, 0.0], 4.0, None),
        );
        bridge.absorb_dynamic_lights(&registry);
        // Regression: absorb used to test reserve capacity before update reclaimed
        // the removed travel light, dropping a same-frame short impact flash.
        assert_eq!(bridge.entity_ids[bridge.authored_light_count], replacement);
        assert_eq!(live_runtime_count(&bridge), RUNTIME_DYNAMIC_LIGHT_RESERVE);
        assert!(bridge.free_slots.is_empty());

        let update = bridge
            .update(&mut registry, 0.2, 0.0)
            .expect("same-frame replacement forces one complete repack");
        assert_eq!(
            update.lights_bytes.len(),
            RUNTIME_DYNAMIC_LIGHT_RESERVE * GPU_LIGHT_SIZE
        );
    }

    #[test]
    fn batched_runtime_absorb_reuses_three_slots_then_appends_two() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0, 0.0);

        let first_batch: Vec<_> = (0..3)
            .map(|index| {
                spawn_runtime_light(
                    &mut registry,
                    runtime_component([index as f32, 0.0, 0.0], 4.0, None),
                )
            })
            .collect();
        bridge.absorb_dynamic_lights(&registry);
        let _ = bridge.update(&mut registry, 0.1, 0.0);
        for id in first_batch {
            registry.despawn(id).unwrap();
        }
        let _ = bridge.update(&mut registry, 0.2, 0.0);
        let prior_len = bridge.entity_ids.len();
        assert_eq!(bridge.free_slots.len(), 3);

        let second_batch: Vec<_> = (0..5)
            .map(|index| {
                spawn_runtime_light(
                    &mut registry,
                    runtime_component([10.0 + index as f32, 0.0, 0.0], 4.0, None),
                )
            })
            .collect();
        bridge.absorb_dynamic_lights(&registry);

        assert!(bridge.free_slots.is_empty());
        assert_eq!(bridge.entity_ids.len(), prior_len + 2);
        assert_eq!(live_runtime_count(&bridge), 5);
        let update = bridge.update(&mut registry, 0.3, 0.0).unwrap();
        assert!(update.has_dirty_data);
        assert_eq!(update.lights_bytes.len(), 5 * GPU_LIGHT_SIZE);
        assert!(
            update
                .lights_bytes
                .chunks_exact(GPU_LIGHT_SIZE)
                .all(|record| record.iter().any(|&byte| byte != 0)),
            "batched runtime enrollment must upload every live forward record"
        );
        for id in second_batch {
            assert!(bridge.entity_ids.contains(&id));
        }
    }

    #[test]
    fn reused_runtime_slot_reclaims_exactly_once_after_second_despawn() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0, 0.0);

        let first =
            spawn_runtime_light(&mut registry, runtime_component([1.0, 0.0, 0.0], 4.0, None));
        bridge.absorb_dynamic_lights(&registry);
        let _ = bridge.update(&mut registry, 0.1, 0.0);
        registry.despawn(first).unwrap();
        let _ = bridge.update(&mut registry, 0.2, 0.0);

        let reused =
            spawn_runtime_light(&mut registry, runtime_component([2.0, 0.0, 0.0], 4.0, None));
        bridge.absorb_dynamic_lights(&registry);
        let _ = bridge.update(&mut registry, 0.3, 0.0);
        registry.despawn(reused).unwrap();
        let _ = bridge.update(&mut registry, 0.4, 0.0);
        assert_eq!(bridge.free_slots.len(), 1);

        let idle = bridge.update(&mut registry, 0.5, 0.0).unwrap();
        assert!(!idle.has_dirty_data);
        assert_eq!(bridge.free_slots.len(), 1);
    }

    #[test]
    fn retained_gpu_packing_stays_at_high_water_while_live_collection_filters_tombstones() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0, 0.0);

        let runtime_ids: Vec<_> = (0..3)
            .map(|index| {
                spawn_runtime_light(
                    &mut registry,
                    runtime_component([index as f32, 0.0, 0.0], 4.0, None),
                )
            })
            .collect();
        bridge.absorb_dynamic_lights(&registry);
        let _ = bridge.update(&mut registry, 0.1, 0.0);

        registry.despawn(runtime_ids[0]).unwrap();
        registry.despawn(runtime_ids[1]).unwrap();
        let update = bridge.update(&mut registry, 0.2, 0.0).unwrap();
        assert_eq!(update.lights_bytes.len(), 3 * GPU_LIGHT_SIZE);
        assert_eq!(update.effective_brightness.len(), 3);
        assert_eq!(bridge.collect_all_as_map_lights(&registry, 0.2).len(), 2);
    }

    #[test]
    fn clear_and_populate_reset_free_slots_before_a_new_authored_prefix() {
        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level(&[sample_point_light()], &mut registry, 0);
        let _ = bridge.update(&mut registry, 0.0, 0.0);
        let runtime =
            spawn_runtime_light(&mut registry, runtime_component([1.0, 0.0, 0.0], 4.0, None));
        bridge.absorb_dynamic_lights(&registry);
        let _ = bridge.update(&mut registry, 0.1, 0.0);
        registry.despawn(runtime).unwrap();
        let _ = bridge.update(&mut registry, 0.2, 0.0);
        assert_eq!(bridge.free_slots.len(), 1);

        bridge.clear();
        assert!(bridge.free_slots.is_empty());

        let mut next_registry = EntityRegistry::new();
        let next_lights = [sample_point_light(), sample_spot_light()];
        bridge.populate_from_level(&next_lights, &mut next_registry, 0);
        assert!(bridge.free_slots.is_empty());
        assert_eq!(bridge.authored_light_count, next_lights.len());
        assert_eq!(bridge.entity_ids.len(), next_lights.len());

        let next_runtime = spawn_runtime_light(
            &mut next_registry,
            runtime_component([2.0, 0.0, 0.0], 4.0, None),
        );
        bridge.absorb_dynamic_lights(&next_registry);
        let _ = bridge.update(&mut next_registry, 0.3, 0.0);
        next_registry.despawn(next_runtime).unwrap();
        let _ = bridge.update(&mut next_registry, 0.4, 0.0);
        assert_eq!(bridge.free_slots.len(), 1);

        let mut replacement_registry = EntityRegistry::new();
        bridge.populate_from_level(
            &[sample_dynamic_point_light()],
            &mut replacement_registry,
            0,
        );
        assert!(bridge.free_slots.is_empty());
        assert_eq!(bridge.authored_light_count, 1);
        assert_eq!(bridge.entity_ids.len(), 1);

        let authored = bridge.update(&mut replacement_registry, 0.5, 0.0).unwrap();
        let authored_forward = authored.lights_bytes.clone();
        let final_runtime = spawn_runtime_light(
            &mut replacement_registry,
            runtime_component([3.0, 0.0, 0.0], 6.0, None),
        );
        bridge.absorb_dynamic_lights(&replacement_registry);
        let with_runtime = bridge.update(&mut replacement_registry, 0.6, 0.0).unwrap();
        let runtime_slot = bridge
            .entity_ids
            .iter()
            .position(|&id| id == final_runtime)
            .expect("final runtime light is enrolled");
        assert!(runtime_slot >= bridge.authored_light_count);
        assert_eq!(
            &with_runtime.lights_bytes[..bridge.authored_light_count * GPU_LIGHT_SIZE],
            authored_forward.as_slice()
        );
    }
}
