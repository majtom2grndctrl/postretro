//! App-side lifetime, projection, and temporary draw assembly for presentation spawns.

use glam::{Mat4, Vec2};
use postretro_entities::{
    EntityRegistry, PresentationEasing, PresentationFacts, PresentationSpawn,
    PresentationTemplateHandle,
};
use postretro_foundation::PresentationDrawInput;

use super::presentation_projection::project_world_to_screen;

#[path = "presentation_pool_overlays.rs"]
mod overlays;

use overlays::KeyedOverlays;
pub use overlays::OverlayKind;

/// Production budget for transient spawn presentation. Keyed overlays receive
/// their own independently bounded map configured by their descriptor.
pub const DEFAULT_PRESENTATION_SPAWN_CAPACITY: usize = 32;

/// Whether presentation instances animate their rise. The App passes the
/// player's resolved reduce-motion switch at its frame-time call site; nothing
/// in simulation reads the preference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MotionPreference {
    #[default]
    Full,
    Reduced,
}

/// Fixed-capacity, app-side lifetime owner for transient world-anchored
/// presentation. It accepts registry intake only; producers never access this
/// pool or renderer state directly.
pub struct PresentationPool {
    capacity: usize,
    frame_time_seconds: f64,
    /// Global renderer-layout identity. This is the only sequence shared by
    /// spawn and overlay presentation; it never participates in either cap or
    /// eviction policy.
    next_instance_id: u64,
    next_spawn_sequence: u64,
    spawns: Vec<LivePresentationSpawn>,
    overlays: KeyedOverlays,
    pending_spawns: Vec<PresentationSpawn>,
    draw_inputs: Vec<PresentationDrawInput>,
}

#[derive(Debug, Clone)]
struct LivePresentationSpawn {
    spawn: PresentationSpawn,
    spawn_time_seconds: f64,
    intake_sequence: u64,
    instance_id: u64,
    scatter: Vec2,
}

impl PresentationPool {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            frame_time_seconds: 0.0,
            next_instance_id: 0,
            next_spawn_sequence: 0,
            spawns: Vec::with_capacity(capacity),
            overlays: KeyedOverlays::default(),
            pending_spawns: Vec::new(),
            draw_inputs: Vec::with_capacity(capacity),
        }
    }

    /// Drain registry intake, advance the frame-time clock, and expose this
    /// frame's passive renderer inputs. Intake occurs before the clock advances
    /// so a new spawn starts at age zero, then advances by this same render
    /// frame's delta alongside all older instances. The pool deliberately does
    /// not lay out templates: FontSystem and UI assets remain renderer-owned.
    pub fn advance_and_collect_inputs(
        &mut self,
        registry: &mut EntityRegistry,
        frame_dt_seconds: f32,
        view_projection: Mat4,
        viewport_size: [u32; 2],
        motion: MotionPreference,
    ) -> Vec<PresentationDrawInput> {
        registry.drain_presentation_spawns_into(&mut self.pending_spawns);
        let mut pending_spawns = std::mem::take(&mut self.pending_spawns);
        for spawn in pending_spawns.drain(..) {
            self.intake(spawn);
        }
        self.pending_spawns = pending_spawns;

        self.frame_time_seconds += valid_frame_delta(frame_dt_seconds);
        let frame_time_seconds = self.frame_time_seconds;
        self.spawns.retain(|live| {
            age_seconds_at(frame_time_seconds, live) < lifetime_seconds(&live.spawn)
        });
        self.overlays.expire_lingering(frame_time_seconds);

        self.collect_draw_inputs(view_projection, viewport_size, motion)
    }

    /// Accept the renderer's previous frame buffer after it swaps in the newly
    /// collected inputs. Two buffers ping-pong across the app/renderer boundary,
    /// retaining Vec, template-string, and fact-map storage at their warm
    /// high-water marks.
    pub fn recycle_draw_inputs(&mut self, inputs: Vec<PresentationDrawInput>) {
        self.draw_inputs = inputs;
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
        &mut self,
        view_projection: Mat4,
        viewport_size: [u32; 2],
        motion: MotionPreference,
    ) -> Vec<PresentationDrawInput> {
        let mut inputs = std::mem::take(&mut self.draw_inputs);
        let required = self.spawns.len().saturating_add(self.overlays.len());
        if required > inputs.capacity() {
            inputs.reserve(required.saturating_sub(inputs.len()));
        }
        let mut input_count = 0;
        for live in &self.spawns {
            let anchor =
                project_world_to_screen(live.spawn.world_anchor, view_projection, viewport_size);

            let lifetime = lifetime_seconds(&live.spawn);
            let age = self.age_seconds(live);
            // Reduced motion: the instance sits at its end-of-life rise from
            // spawn. Scatter (a fixed offset) and fade (opacity) are not motion.
            let progress = match motion {
                MotionPreference::Full => eased_progress(
                    (age / lifetime).clamp(0.0, 1.0) as f32,
                    live.spawn.motion.easing,
                ),
                MotionPreference::Reduced => 1.0,
            };
            let rise = finite_or_zero(live.spawn.motion.rise_pixels) * progress;
            let alpha = fade_alpha(&live.spawn, age, lifetime);

            let projected_anchor = anchor.map_or([0.0, 0.0], |anchor| {
                [anchor.x + live.scatter.x, anchor.y + live.scatter.y - rise]
            });
            write_draw_input(
                &mut inputs,
                &mut input_count,
                live.instance_id,
                &live.spawn.template,
                &live.spawn.facts,
                projected_anchor,
                alpha,
                anchor.is_some(),
            );
        }
        for overlay in self.overlays.values() {
            let anchor = (!overlay.suppressed)
                .then(|| {
                    project_world_to_screen(overlay.world_anchor, view_projection, viewport_size)
                })
                .flatten();
            write_draw_input(
                &mut inputs,
                &mut input_count,
                overlay.instance_id,
                &overlay.template,
                &overlay.facts,
                anchor.map_or([0.0, 0.0], |anchor| [anchor.x, anchor.y]),
                1.0,
                anchor.is_some(),
            );
        }
        inputs.truncate(input_count);
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
}

impl Default for PresentationPool {
    fn default() -> Self {
        Self::new(DEFAULT_PRESENTATION_SPAWN_CAPACITY)
    }
}

#[allow(clippy::too_many_arguments)]
fn write_draw_input(
    inputs: &mut Vec<PresentationDrawInput>,
    input_count: &mut usize,
    instance_id: u64,
    template: &PresentationTemplateHandle,
    facts: &PresentationFacts,
    anchor: [f32; 2],
    opacity: f32,
    visible: bool,
) {
    if let Some(input) = inputs.get_mut(*input_count) {
        input.instance_id = instance_id;
        input.template.clone_from(template);
        sync_facts(&mut input.facts, facts);
        input.anchor = anchor;
        input.opacity = opacity;
        input.visible = visible;
    } else {
        inputs.push(PresentationDrawInput {
            instance_id,
            template: template.clone(),
            facts: facts.clone(),
            anchor,
            opacity,
            visible,
        });
    }
    *input_count += 1;
}

/// Synchronize a warm draw-input fact snapshot without rebuilding its tree or
/// stable key/value Strings. Key-set changes remain a cold structural event.
fn sync_facts(target: &mut PresentationFacts, source: &PresentationFacts) {
    target.retain(|name, _| source.contains_key(name));
    for (name, source_value) in source {
        if let Some(target_value) = target.get_mut(name) {
            match (target_value, source_value) {
                (
                    postretro_entities::PresentationFact::Text(target),
                    postretro_entities::PresentationFact::Text(source),
                ) => target.clone_from(source),
                (target, source) => target.clone_from(source),
            }
        } else {
            target.insert(name.clone(), source_value.clone());
        }
    }
}

fn update_fact(
    facts: &mut PresentationFacts,
    name: &'static str,
    value: postretro_entities::PresentationFact,
) {
    if let Some(current) = facts.get_mut(name) {
        *current = value;
    } else {
        facts.insert(name.to_owned(), value);
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
        glam::camera::rh::proj::directx::perspective(90.0_f32.to_radians(), 4.0 / 3.0, 0.1, 100.0)
            * glam::camera::rh::view::look_at_mat4(Vec3::ZERO, -Vec3::Z, Vec3::Y)
    }

    #[test]
    fn pool_evicts_oldest_by_intake_fifo_when_spawn_times_tie() {
        let mut registry = EntityRegistry::new();
        let mut pool = PresentationPool::new(2);
        registry.push_presentation_spawn(spawn("first", Vec3::ZERO, 1.0));
        registry.push_presentation_spawn(spawn("second", Vec3::ZERO, 1.0));
        let _ = pool.advance_and_collect_inputs(
            &mut registry,
            0.0,
            Mat4::IDENTITY,
            [800, 600],
            MotionPreference::Full,
        );

        registry.push_presentation_spawn(spawn("third", Vec3::ZERO, 1.0));
        let _ = pool.advance_and_collect_inputs(
            &mut registry,
            0.0,
            Mat4::IDENTITY,
            [800, 600],
            MotionPreference::Full,
        );

        assert_eq!(pool.live_template_names(), ["second", "third"]);
    }

    #[test]
    fn pool_uses_frame_time_for_new_spawn_age_and_expiry() {
        let mut registry = EntityRegistry::new();
        let mut pool = PresentationPool::new(1);
        registry.push_presentation_spawn(spawn("impact", Vec3::ZERO, 0.2));

        let first = pool.advance_and_collect_inputs(
            &mut registry,
            0.1,
            Mat4::IDENTITY,
            [800, 600],
            MotionPreference::Full,
        );
        assert_eq!(first.len(), 1);
        assert!((pool.live_ages_seconds()[0] - 0.1).abs() < f64::from(EPSILON));
        assert!((first[0].opacity - 0.5).abs() < EPSILON);

        let expired = pool.advance_and_collect_inputs(
            &mut registry,
            0.11,
            Mat4::IDENTITY,
            [800, 600],
            MotionPreference::Full,
        );
        assert!(expired.is_empty());
        assert!(pool.live_ages_seconds().is_empty());
    }

    #[test]
    fn reduced_motion_spawns_at_full_rise_and_keeps_scatter_and_fade() {
        let collect = |motion| {
            let mut registry = EntityRegistry::new();
            let mut pool = PresentationPool::new(1);
            let mut hit = spawn("number", Vec3::new(0.0, 0.0, -2.0), 1.0);
            hit.scatter_radius = 20.0;
            registry.push_presentation_spawn(hit);
            let first = pool.advance_and_collect_inputs(
                &mut registry,
                0.0,
                camera_view_projection(),
                [800, 600],
                motion,
            );
            let late = pool.advance_and_collect_inputs(
                &mut registry,
                0.75,
                camera_view_projection(),
                [800, 600],
                motion,
            );
            (first[0].anchor, late[0].anchor, late[0].opacity)
        };
        let (full_first, full_late, full_opacity) = collect(MotionPreference::Full);
        let (reduced_first, reduced_late, reduced_opacity) = collect(MotionPreference::Reduced);

        // Full motion rises over the lifetime; reduced sits at the full 12 px
        // rise from its first frame and never moves.
        assert!((full_first[1] - (full_late[1] + 9.0)).abs() < EPSILON);
        assert!((reduced_first[1] - (full_first[1] - 12.0)).abs() < EPSILON);
        assert_eq!(reduced_first, reduced_late);
        // Scatter is the same fixed offset either way; fade is opacity, not motion.
        assert!((reduced_first[0] - full_first[0]).abs() < EPSILON);
        assert!((reduced_opacity - full_opacity).abs() < EPSILON);
        assert!(reduced_opacity < 1.0, "the fade still runs");
    }

    #[test]
    fn pool_marks_behind_and_offscreen_spawns_culled_but_live() {
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
            MotionPreference::Full,
        );

        assert_eq!(inputs.len(), 3);
        assert!(inputs[0].visible);
        assert!((inputs[0].anchor[0] - 400.0).abs() < EPSILON);
        assert!((inputs[0].anchor[1] - 300.0).abs() < EPSILON);
        assert!(!inputs[1].visible);
        assert!(!inputs[2].visible);
    }

    #[test]
    fn keyed_overlays_have_a_disjoint_budget_and_evict_fifo_when_damage_times_tie() {
        let mut registry = EntityRegistry::new();
        let first = registry.spawn(Transform::default());
        let second = registry.spawn(Transform::default());
        let third = registry.spawn(Transform::default());
        let mut pool = PresentationPool::new(1);

        pool.refresh_overlay(
            OverlayKind::DamagedEnemies,
            first,
            PresentationTemplateHandle::from("status"),
            1.0,
            2,
            1,
        );
        pool.refresh_overlay(
            OverlayKind::DamagedEnemies,
            second,
            PresentationTemplateHandle::from("status"),
            1.0,
            2,
            2,
        );
        pool.refresh_overlay(
            OverlayKind::DamagedEnemies,
            third,
            PresentationTemplateHandle::from("status"),
            1.0,
            2,
            3,
        );

        assert_eq!(
            pool.overlay_ids(OverlayKind::DamagedEnemies),
            [second, third]
        );
        registry.push_presentation_spawn(spawn("damage-number", Vec3::ZERO, 1.0));
        let _ = pool.advance_and_collect_inputs(
            &mut registry,
            0.0,
            Mat4::IDENTITY,
            [800, 600],
            MotionPreference::Full,
        );

        assert_eq!(pool.live_template_names(), ["damage-number"]);
        assert_eq!(
            pool.overlay_ids(OverlayKind::DamagedEnemies),
            [second, third]
        );
    }

    #[test]
    fn keyed_overlay_linger_expires_without_affecting_spawn_ring() {
        let mut registry = EntityRegistry::new();
        let target = registry.spawn(Transform::default());
        let mut pool = PresentationPool::new(1);
        pool.refresh_overlay(
            OverlayKind::DamagedEnemies,
            target,
            PresentationTemplateHandle::from("status"),
            0.1,
            1,
            1,
        );
        registry.push_presentation_spawn(spawn("damage-number", Vec3::ZERO, 1.0));

        let _ = pool.advance_and_collect_inputs(
            &mut registry,
            0.11,
            Mat4::IDENTITY,
            [800, 600],
            MotionPreference::Full,
        );

        assert!(pool.overlay_ids(OverlayKind::DamagedEnemies).is_empty());
        assert_eq!(pool.live_template_names(), ["damage-number"]);
    }

    #[test]
    fn authoritative_overlay_survives_draw_stalls_and_preserves_instance_facts() {
        let mut registry = EntityRegistry::new();
        let target = registry.spawn(Transform::default());
        let mut pool = PresentationPool::new(1);
        assert!(pool.upsert_authoritative_overlay(
            OverlayKind::DamagedEnemies,
            target,
            PresentationTemplateHandle::from("status"),
            1,
        ));
        pool.stamp_damaged_enemy_overlay(target, 0.5, 0.25, true, Vec3::new(0.0, 0.0, -2.0), false);
        let first = pool.advance_and_collect_inputs(
            &mut registry,
            0.0,
            camera_view_projection(),
            [800, 600],
            MotionPreference::Full,
        );
        assert_eq!(first.len(), 1);
        assert!(first[0].visible);
        let instance_id = first[0].instance_id;
        pool.recycle_draw_inputs(first);

        // No draw calls occur while the external host roster advances. Its
        // next draw may advance far beyond every local linger duration.
        assert!(pool.upsert_authoritative_overlay(
            OverlayKind::DamagedEnemies,
            target,
            PresentationTemplateHandle::from("updated-status"),
            1,
        ));
        let resumed = pool.advance_and_collect_inputs(
            &mut registry,
            120.0,
            camera_view_projection(),
            [800, 600],
            MotionPreference::Reduced,
        );
        assert_eq!(resumed.len(), 1);
        assert_eq!(resumed[0].instance_id, instance_id);
        assert_eq!(
            resumed[0].template,
            PresentationTemplateHandle::from("updated-status")
        );
        for (name, expected) in [("healthFraction", 0.5), ("shieldFraction", 0.25)] {
            let Some(postretro_entities::PresentationFact::Number(value)) =
                resumed[0].facts.get(name)
            else {
                panic!("missing numeric overlay fact {name}");
            };
            assert!((value - expected).abs() < EPSILON);
        }
        assert_eq!(
            resumed[0].facts.get("hasShield"),
            Some(&postretro_entities::PresentationFact::Bool(true))
        );
        assert!(resumed[0].visible);

        // External removal applies immediately, without waiting for a draw.
        pool.retain_overlays(OverlayKind::DamagedEnemies, |_| false);
        assert!(!pool.has_overlay(OverlayKind::DamagedEnemies, target));
        assert!(
            pool.advance_and_collect_inputs(
                &mut registry,
                0.0,
                camera_view_projection(),
                [800, 600],
                MotionPreference::Full,
            )
            .is_empty()
        );
    }

    #[test]
    fn authoritative_overlay_requires_roster_removal_before_replacement_at_cap() {
        let mut registry = EntityRegistry::new();
        let first = registry.spawn(Transform::default());
        let second = registry.spawn(Transform::default());
        let third = registry.spawn(Transform::default());
        let mut pool = PresentationPool::new(0);
        let template = PresentationTemplateHandle::from("status");
        assert!(!pool.upsert_authoritative_overlay(
            OverlayKind::DamagedEnemies,
            first,
            template.clone(),
            0
        ));
        assert!(pool.upsert_authoritative_overlay(
            OverlayKind::DamagedEnemies,
            first,
            template.clone(),
            2
        ));
        assert!(pool.upsert_authoritative_overlay(
            OverlayKind::DamagedEnemies,
            second,
            template.clone(),
            2
        ));
        assert!(!pool.upsert_authoritative_overlay(
            OverlayKind::DamagedEnemies,
            third,
            template.clone(),
            2
        ));
        assert_eq!(
            pool.overlay_ids(OverlayKind::DamagedEnemies),
            [first, second]
        );
        pool.retain_overlays(OverlayKind::DamagedEnemies, |entity| entity == second);
        assert!(pool.upsert_authoritative_overlay(
            OverlayKind::DamagedEnemies,
            third,
            template.clone(),
            2
        ));
        assert_eq!(
            pool.overlay_ids(OverlayKind::DamagedEnemies),
            [second, third]
        );

        // Returning an existing projection to the single-player path restores
        // its ordinary local linger ownership.
        pool.refresh_overlay(OverlayKind::DamagedEnemies, second, template, 0.1, 2, 2);
        let _ = pool.advance_and_collect_inputs(
            &mut registry,
            0.11,
            Mat4::IDENTITY,
            [800, 600],
            MotionPreference::Full,
        );
        assert_eq!(pool.overlay_ids(OverlayKind::DamagedEnemies), [third]);
        pool.evict_overlay(OverlayKind::DamagedEnemies, third);
        assert!(pool.overlay_ids(OverlayKind::DamagedEnemies).is_empty());
    }

    // Regression: returning to the frontend retained live world anchors from
    // the unloaded map in both independently-budgeted pools.
    #[test]
    fn level_unload_clears_spawn_and_overlay_budgets() {
        let mut registry = EntityRegistry::new();
        let target = registry.spawn(Transform::default());
        let mut pool = PresentationPool::new(1);
        registry.push_presentation_spawn(spawn("old-number", Vec3::ZERO, 1.0));
        let _ = pool.advance_and_collect_inputs(
            &mut registry,
            0.0,
            Mat4::IDENTITY,
            [800, 600],
            MotionPreference::Full,
        );
        pool.refresh_overlay(
            OverlayKind::DamagedEnemies,
            target,
            PresentationTemplateHandle::from("old-bar"),
            1.0,
            1,
            1,
        );

        pool.clear_world_instances();

        assert!(pool.live_template_names().is_empty());
        assert!(pool.overlay_ids(OverlayKind::DamagedEnemies).is_empty());
    }

    // Equal-time local selection is stable even if damage enumeration order
    // differs. The authoritative roster has its own external selection policy.
    #[test]
    fn keyed_overlay_equal_time_cap_uses_stable_order_not_arrival_order() {
        let mut registry = EntityRegistry::new();
        let first = registry.spawn(Transform::default());
        let second = registry.spawn(Transform::default());
        let third = registry.spawn(Transform::default());
        let mut forward = PresentationPool::new(0);
        let mut reverse = PresentationPool::new(0);

        for (entity, order) in [(first, 1), (second, 2), (third, 3)] {
            forward.refresh_overlay(
                OverlayKind::DamagedEnemies,
                entity,
                PresentationTemplateHandle::from("status"),
                1.0,
                2,
                order,
            );
        }
        for (entity, order) in [(third, 3), (second, 2), (first, 1)] {
            reverse.refresh_overlay(
                OverlayKind::DamagedEnemies,
                entity,
                PresentationTemplateHandle::from("status"),
                1.0,
                2,
                order,
            );
        }

        assert_eq!(
            forward.overlay_ids(OverlayKind::DamagedEnemies),
            [second, third]
        );
        assert_eq!(
            reverse.overlay_ids(OverlayKind::DamagedEnemies),
            [second, third]
        );
    }
}
