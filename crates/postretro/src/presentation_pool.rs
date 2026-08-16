//! App-side lifetime, projection, and temporary draw assembly for presentation spawns.

use std::collections::HashMap;

use glam::{Mat4, Vec2};
use postretro_entities::{
    EntityId, EntityRegistry, PresentationEasing, PresentationFacts, PresentationSpawn,
    PresentationTemplateHandle,
};
use postretro_renderer::PresentationDrawInput;

use crate::presentation_projection::project_world_to_screen;

/// Production budget for transient spawn presentation. Keyed overlays receive
/// their own independently bounded map configured by their descriptor.
pub(crate) const DEFAULT_PRESENTATION_SPAWN_CAPACITY: usize = 32;

/// Fixed-capacity, app-side lifetime owner for transient world-anchored
/// presentation. It accepts registry intake only; producers never access this
/// pool or renderer state directly.
pub(crate) struct PresentationPool {
    capacity: usize,
    frame_time_seconds: f64,
    /// Global renderer-layout identity. This is the only sequence shared by
    /// spawn and overlay presentation; it never participates in either cap or
    /// eviction policy.
    next_instance_id: u64,
    next_spawn_sequence: u64,
    next_overlay_sequence: u64,
    spawns: Vec<LivePresentationSpawn>,
    overlays: HashMap<EntityId, LivePresentationOverlay>,
}

#[derive(Debug, Clone)]
struct LivePresentationSpawn {
    spawn: PresentationSpawn,
    spawn_time_seconds: f64,
    intake_sequence: u64,
    instance_id: u64,
    scatter: Vec2,
}

#[derive(Debug, Clone)]
struct LivePresentationOverlay {
    template: PresentationTemplateHandle,
    facts: PresentationFacts,
    world_anchor: glam::Vec3,
    last_damaged_time_seconds: f64,
    creation_sequence: u64,
    instance_id: u64,
    linger_seconds: f64,
    suppressed: bool,
}

impl PresentationPool {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            frame_time_seconds: 0.0,
            next_instance_id: 0,
            next_spawn_sequence: 0,
            next_overlay_sequence: 0,
            spawns: Vec::with_capacity(capacity),
            overlays: HashMap::new(),
        }
    }

    /// Drain registry intake, advance the frame-time clock, and expose this
    /// frame's passive renderer inputs. Intake occurs before the clock advances
    /// so a new spawn starts at age zero, then advances by this same render
    /// frame's delta alongside all older instances. The pool deliberately does
    /// not lay out templates: FontSystem and UI assets remain renderer-owned.
    pub(crate) fn advance_and_collect_inputs(
        &mut self,
        registry: &mut EntityRegistry,
        frame_dt_seconds: f32,
        view_projection: Mat4,
        viewport_size: [u32; 2],
    ) -> Vec<PresentationDrawInput> {
        for spawn in registry.take_presentation_spawns() {
            self.intake(spawn);
        }

        self.frame_time_seconds += valid_frame_delta(frame_dt_seconds);
        let frame_time_seconds = self.frame_time_seconds;
        self.spawns.retain(|live| {
            age_seconds_at(frame_time_seconds, live) < lifetime_seconds(&live.spawn)
        });
        self.overlays.retain(|_, overlay| {
            frame_time_seconds - overlay.last_damaged_time_seconds < overlay.linger_seconds
        });

        self.collect_draw_inputs(view_projection, viewport_size)
    }

    fn intake(&mut self, spawn: PresentationSpawn) {
        if self.capacity == 0 {
            return;
        }
        if self.spawns.len() == self.capacity {
            let oldest = self.oldest_spawn_index();
            self.spawns.remove(oldest);
        }

        let scatter = scatter_offset(spawn.scatter_radius, self.next_spawn_sequence);
        self.spawns.push(LivePresentationSpawn {
            spawn,
            spawn_time_seconds: self.frame_time_seconds,
            intake_sequence: self.next_spawn_sequence,
            instance_id: self.next_instance_id,
            scatter,
        });
        self.next_spawn_sequence = self.next_spawn_sequence.wrapping_add(1);
        self.next_instance_id = self.next_instance_id.wrapping_add(1);
    }

    fn oldest_spawn_index(&self) -> usize {
        self.spawns
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                left.spawn_time_seconds
                    .partial_cmp(&right.spawn_time_seconds)
                    .expect("presentation frame clock stays finite")
                    .then_with(|| left.intake_sequence.cmp(&right.intake_sequence))
            })
            .map(|(index, _)| index)
            .expect("a full presentation pool has at least one live spawn")
    }

    fn collect_draw_inputs(
        &self,
        view_projection: Mat4,
        viewport_size: [u32; 2],
    ) -> Vec<PresentationDrawInput> {
        let mut inputs = Vec::with_capacity(self.spawns.len());
        for live in &self.spawns {
            let Some(anchor) =
                project_world_to_screen(live.spawn.world_anchor, view_projection, viewport_size)
            else {
                continue;
            };

            let lifetime = lifetime_seconds(&live.spawn);
            let age = self.age_seconds(live);
            let progress = eased_progress(
                (age / lifetime).clamp(0.0, 1.0) as f32,
                live.spawn.motion.easing,
            );
            let rise = finite_or_zero(live.spawn.motion.rise_pixels) * progress;
            let alpha = fade_alpha(&live.spawn, age, lifetime);

            inputs.push(PresentationDrawInput {
                instance_id: live.instance_id,
                template: live.spawn.template.clone(),
                facts: live.spawn.facts.clone(),
                anchor: [anchor.x + live.scatter.x, anchor.y + live.scatter.y - rise],
                opacity: alpha,
            });
        }
        for overlay in self.overlays.values() {
            if overlay.suppressed {
                continue;
            }
            let Some(anchor) =
                project_world_to_screen(overlay.world_anchor, view_projection, viewport_size)
            else {
                continue;
            };
            inputs.push(PresentationDrawInput {
                instance_id: overlay.instance_id,
                template: overlay.template.clone(),
                facts: overlay.facts.clone(),
                anchor: [anchor.x, anchor.y],
                opacity: 1.0,
            });
        }
        inputs
    }

    fn age_seconds(&self, live: &LivePresentationSpawn) -> f64 {
        age_seconds_at(self.frame_time_seconds, live)
    }

    #[cfg(test)]
    fn live_template_names(&self) -> Vec<&str> {
        self.spawns
            .iter()
            .map(|live| live.spawn.template.0.as_str())
            .collect()
    }

    #[cfg(test)]
    fn live_ages_seconds(&self) -> Vec<f64> {
        self.spawns
            .iter()
            .map(|live| self.age_seconds(live))
            .collect()
    }

    /// Create or refresh one target-keyed overlay. Its cap is separate from
    /// the spawn ring's `capacity`; neither archetype can evict the other.
    pub(crate) fn refresh_overlay(
        &mut self,
        entity: EntityId,
        template: PresentationTemplateHandle,
        linger_seconds: f64,
        max_visible: usize,
    ) {
        if max_visible == 0 {
            return;
        }
        if let Some(overlay) = self.overlays.get_mut(&entity) {
            overlay.template = template;
            overlay.linger_seconds = linger_seconds.max(0.0);
            overlay.last_damaged_time_seconds = self.frame_time_seconds;
            return;
        }
        while self.overlays.len() >= max_visible {
            let Some(entity) = self.least_recent_overlay() else {
                break;
            };
            self.overlays.remove(&entity);
        }
        let creation_sequence = self.next_overlay_sequence;
        self.next_overlay_sequence = self.next_overlay_sequence.wrapping_add(1);
        let instance_id = self.next_instance_id;
        self.next_instance_id = self.next_instance_id.wrapping_add(1);
        self.overlays.insert(
            entity,
            LivePresentationOverlay {
                template,
                facts: PresentationFacts::new(),
                world_anchor: glam::Vec3::ZERO,
                last_damaged_time_seconds: self.frame_time_seconds,
                creation_sequence,
                instance_id,
                linger_seconds: linger_seconds.max(0.0),
                suppressed: true,
            },
        );
    }

    /// Stamp a tracked instance. Tracking creation is dispatch-driven, so this
    /// never creates an overlay during the frame scan.
    pub(crate) fn stamp_overlay(
        &mut self,
        entity: EntityId,
        facts: PresentationFacts,
        world_anchor: glam::Vec3,
        suppressed: bool,
    ) {
        let Some(overlay) = self.overlays.get_mut(&entity) else {
            return;
        };
        overlay.facts = facts;
        overlay.world_anchor = world_anchor;
        overlay.suppressed = suppressed;
    }

    pub(crate) fn evict_overlay(&mut self, entity: EntityId) {
        self.overlays.remove(&entity);
    }

    /// Whether this entity still owns a live keyed overlay after the pool's
    /// frame-time linger eviction. Client network-id bookkeeping uses this to
    /// discard its otherwise separate identity entry at the same boundary.
    pub(crate) fn has_overlay(&self, entity: EntityId) -> bool {
        self.overlays.contains_key(&entity)
    }

    /// Drop all keyed overlays after their authoring snapshot is replaced.
    /// Spawn presentation intentionally survives this separately-owned reset.
    pub(crate) fn clear_overlays(&mut self) {
        self.overlays.clear();
    }

    pub(crate) fn tracked_overlay_ids(&self) -> Vec<EntityId> {
        self.overlays.keys().copied().collect()
    }

    fn least_recent_overlay(&self) -> Option<EntityId> {
        self.overlays
            .iter()
            .min_by(|(_, left), (_, right)| {
                left.last_damaged_time_seconds
                    .partial_cmp(&right.last_damaged_time_seconds)
                    .expect("presentation frame clock stays finite")
                    .then_with(|| left.creation_sequence.cmp(&right.creation_sequence))
            })
            .map(|(entity, _)| *entity)
    }

    #[cfg(test)]
    fn overlay_ids(&self) -> Vec<EntityId> {
        let mut ids: Vec<_> = self.overlays.keys().copied().collect();
        ids.sort_by_key(|id| id.to_raw());
        ids
    }

    #[cfg(test)]
    pub(crate) fn overlay_facts(&self, entity: EntityId) -> Option<&PresentationFacts> {
        self.overlays.get(&entity).map(|overlay| &overlay.facts)
    }

    #[cfg(test)]
    pub(crate) fn overlay_is_suppressed(&self, entity: EntityId) -> Option<bool> {
        self.overlays.get(&entity).map(|overlay| overlay.suppressed)
    }

    #[cfg(test)]
    pub(crate) fn overlay_anchor(&self, entity: EntityId) -> Option<glam::Vec3> {
        self.overlays
            .get(&entity)
            .map(|overlay| overlay.world_anchor)
    }
}

impl Default for PresentationPool {
    fn default() -> Self {
        Self::new(DEFAULT_PRESENTATION_SPAWN_CAPACITY)
    }
}

fn valid_frame_delta(frame_dt_seconds: f32) -> f64 {
    if frame_dt_seconds.is_finite() && frame_dt_seconds > 0.0 {
        f64::from(frame_dt_seconds)
    } else {
        0.0
    }
}

fn age_seconds_at(frame_time_seconds: f64, live: &LivePresentationSpawn) -> f64 {
    (frame_time_seconds - live.spawn_time_seconds).max(0.0)
}

fn lifetime_seconds(spawn: &PresentationSpawn) -> f64 {
    if spawn.lifetime_seconds.is_finite() {
        f64::from(spawn.lifetime_seconds.max(0.0))
    } else {
        0.0
    }
}

fn fade_alpha(spawn: &PresentationSpawn, age_seconds: f64, lifetime_seconds: f64) -> f32 {
    let fade_seconds =
        (finite_or_zero(spawn.fade.duration_seconds).max(0.0) as f64).min(lifetime_seconds);
    if fade_seconds == 0.0 {
        return 1.0;
    }

    ((lifetime_seconds - age_seconds) / fade_seconds).clamp(0.0, 1.0) as f32
}

fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

fn eased_progress(progress: f32, easing: PresentationEasing) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    match easing {
        PresentationEasing::Linear => progress,
        PresentationEasing::EaseIn => progress * progress * progress,
        PresentationEasing::EaseOut => {
            let inverse = 1.0 - progress;
            1.0 - inverse * inverse * inverse
        }
        PresentationEasing::EaseInOut => {
            if progress < 0.5 {
                4.0 * progress * progress * progress
            } else {
                let inverse = -2.0 * progress + 2.0;
                1.0 - inverse * inverse * inverse / 2.0
            }
        }
    }
}

fn scatter_offset(radius: f32, sequence: u64) -> Vec2 {
    let radius = finite_or_zero(radius).max(0.0);
    if radius == 0.0 {
        return Vec2::ZERO;
    }

    // A stateless integer mix gives intake-order-stable scatter without adding
    // producer RNG or a mutable random stream to the renderer bridge.
    let mut bits = sequence.wrapping_add(0x9E37_79B9_7F4A_7C15);
    bits = (bits ^ (bits >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    bits = (bits ^ (bits >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    bits ^= bits >> 31;
    let angle = (bits as f32 / u64::MAX as f32) * std::f32::consts::TAU;
    let distance_bits = bits.rotate_left(29);
    let distance = (distance_bits as f32 / u64::MAX as f32).sqrt() * radius;
    Vec2::new(angle.cos() * distance, angle.sin() * distance)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use glam::{Mat4, Vec3};
    use postretro_entities::{
        PresentationFade, PresentationMotion, PresentationSpawn, PresentationTemplateHandle,
        Transform,
    };

    use super::*;

    const EPSILON: f32 = 1.0e-5;

    fn spawn(template: &str, anchor: Vec3, lifetime_seconds: f32) -> PresentationSpawn {
        PresentationSpawn {
            world_anchor: anchor,
            template: PresentationTemplateHandle::from(template),
            facts: BTreeMap::new(),
            presenter: None,
            lifetime_seconds,
            motion: PresentationMotion {
                rise_pixels: 12.0,
                easing: PresentationEasing::Linear,
            },
            fade: PresentationFade {
                duration_seconds: 0.5,
            },
            scatter_radius: 0.0,
        }
    }

    fn camera_view_projection() -> Mat4 {
        Mat4::perspective_rh(90.0_f32.to_radians(), 4.0 / 3.0, 0.1, 100.0)
            * Mat4::look_at_rh(Vec3::ZERO, -Vec3::Z, Vec3::Y)
    }

    #[test]
    fn pool_evicts_oldest_by_intake_fifo_when_spawn_times_tie() {
        let mut registry = EntityRegistry::new();
        let mut pool = PresentationPool::new(2);
        registry.push_presentation_spawn(spawn("first", Vec3::ZERO, 1.0));
        registry.push_presentation_spawn(spawn("second", Vec3::ZERO, 1.0));
        let _ = pool.advance_and_collect_inputs(&mut registry, 0.0, Mat4::IDENTITY, [800, 600]);

        registry.push_presentation_spawn(spawn("third", Vec3::ZERO, 1.0));
        let _ = pool.advance_and_collect_inputs(&mut registry, 0.0, Mat4::IDENTITY, [800, 600]);

        assert_eq!(pool.live_template_names(), ["second", "third"]);
    }

    #[test]
    fn pool_uses_frame_time_for_new_spawn_age_and_expiry() {
        let mut registry = EntityRegistry::new();
        let mut pool = PresentationPool::new(1);
        registry.push_presentation_spawn(spawn("impact", Vec3::ZERO, 0.2));

        let first = pool.advance_and_collect_inputs(&mut registry, 0.1, Mat4::IDENTITY, [800, 600]);
        assert_eq!(first.len(), 1);
        assert!((pool.live_ages_seconds()[0] - 0.1).abs() < f64::from(EPSILON));
        assert!((first[0].opacity - 0.5).abs() < EPSILON);

        let expired =
            pool.advance_and_collect_inputs(&mut registry, 0.11, Mat4::IDENTITY, [800, 600]);
        assert!(expired.is_empty());
        assert!(pool.live_ages_seconds().is_empty());
    }

    #[test]
    fn pool_skips_behind_and_offscreen_spawns() {
        let mut registry = EntityRegistry::new();
        let mut pool = PresentationPool::new(3);
        registry.push_presentation_spawn(spawn("visible", Vec3::new(0.0, 0.0, -2.0), 1.0));
        registry.push_presentation_spawn(spawn("behind", Vec3::new(0.0, 0.0, 2.0), 1.0));
        registry.push_presentation_spawn(spawn("offscreen", Vec3::new(100.0, 0.0, -2.0), 1.0));

        let inputs = pool.advance_and_collect_inputs(
            &mut registry,
            0.0,
            camera_view_projection(),
            [800, 600],
        );

        assert_eq!(inputs.len(), 1);
        assert!((inputs[0].anchor[0] - 400.0).abs() < EPSILON);
        assert!((inputs[0].anchor[1] - 300.0).abs() < EPSILON);
    }

    #[test]
    fn keyed_overlays_have_a_disjoint_budget_and_evict_fifo_when_damage_times_tie() {
        let mut registry = EntityRegistry::new();
        let first = registry.spawn(Transform::default());
        let second = registry.spawn(Transform::default());
        let third = registry.spawn(Transform::default());
        let mut pool = PresentationPool::new(1);

        pool.refresh_overlay(first, PresentationTemplateHandle::from("status"), 1.0, 2);
        pool.refresh_overlay(second, PresentationTemplateHandle::from("status"), 1.0, 2);
        pool.refresh_overlay(third, PresentationTemplateHandle::from("status"), 1.0, 2);

        assert_eq!(pool.overlay_ids(), [second, third]);
        registry.push_presentation_spawn(spawn("damage-number", Vec3::ZERO, 1.0));
        let _ = pool.advance_and_collect_inputs(&mut registry, 0.0, Mat4::IDENTITY, [800, 600]);

        assert_eq!(pool.live_template_names(), ["damage-number"]);
        assert_eq!(pool.overlay_ids(), [second, third]);
    }

    #[test]
    fn keyed_overlay_linger_expires_without_affecting_spawn_ring() {
        let mut registry = EntityRegistry::new();
        let target = registry.spawn(Transform::default());
        let mut pool = PresentationPool::new(1);
        pool.refresh_overlay(target, PresentationTemplateHandle::from("status"), 0.1, 1);
        registry.push_presentation_spawn(spawn("damage-number", Vec3::ZERO, 1.0));

        let _ = pool.advance_and_collect_inputs(&mut registry, 0.11, Mat4::IDENTITY, [800, 600]);

        assert!(pool.overlay_ids().is_empty());
        assert_eq!(pool.live_template_names(), ["damage-number"]);
    }
}
