//! App-side lifetime, projection, and temporary draw assembly for presentation spawns.

use glam::Mat4;
use postretro_entities::{EntityRegistry, PresentationSpawn};
use postretro_ui::{UiInstance, tree::UiDrawData};

use crate::presentation_projection::project_world_to_screen;

/// Production budget for transient spawn presentation. Keyed overlays receive
/// their own independently bounded pool when that archetype lands.
pub(crate) const DEFAULT_PRESENTATION_SPAWN_CAPACITY: usize = 32;

/// Fixed-capacity, app-side lifetime owner for transient world-anchored
/// presentation. It accepts registry intake only; producers never access this
/// pool or renderer state directly.
pub(crate) struct PresentationPool {
    capacity: usize,
    frame_time_seconds: f64,
    next_intake_sequence: u64,
    spawns: Vec<LivePresentationSpawn>,
}

#[derive(Debug, Clone)]
struct LivePresentationSpawn {
    spawn: PresentationSpawn,
    spawn_time_seconds: f64,
    intake_sequence: u64,
}

impl PresentationPool {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            frame_time_seconds: 0.0,
            next_intake_sequence: 0,
            spawns: Vec::with_capacity(capacity),
        }
    }

    /// Drain registry intake, advance the frame-time clock, and emit this
    /// frame's passive draw data. Intake occurs before the clock advances so a
    /// new spawn starts at age zero, then advances by this same render frame's
    /// delta alongside all older instances.
    pub(crate) fn advance_and_build_draw_data(
        &mut self,
        registry: &mut EntityRegistry,
        frame_dt_seconds: f32,
        view_projection: Mat4,
        viewport_size: [u32; 2],
    ) -> UiDrawData {
        for spawn in registry.take_presentation_spawns() {
            self.intake(spawn);
        }

        self.frame_time_seconds += valid_frame_delta(frame_dt_seconds);
        let frame_time_seconds = self.frame_time_seconds;
        self.spawns.retain(|live| {
            age_seconds_at(frame_time_seconds, live) < lifetime_seconds(&live.spawn)
        });

        self.build_temporary_draw_data(view_projection, viewport_size)
    }

    fn intake(&mut self, spawn: PresentationSpawn) {
        if self.capacity == 0 {
            return;
        }
        if self.spawns.len() == self.capacity {
            let oldest = self.oldest_spawn_index();
            self.spawns.remove(oldest);
        }

        self.spawns.push(LivePresentationSpawn {
            spawn,
            spawn_time_seconds: self.frame_time_seconds,
            intake_sequence: self.next_intake_sequence,
        });
        self.next_intake_sequence = self.next_intake_sequence.wrapping_add(1);
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

    fn build_temporary_draw_data(
        &self,
        view_projection: Mat4,
        viewport_size: [u32; 2],
    ) -> UiDrawData {
        let mut draw = UiDrawData::default();
        for live in &self.spawns {
            let Some(anchor) =
                project_world_to_screen(live.spawn.world_anchor, view_projection, viewport_size)
            else {
                continue;
            };

            let lifetime = lifetime_seconds(&live.spawn);
            let age = self.age_seconds(live);
            let progress = (age / lifetime).clamp(0.0, 1.0) as f32;
            let rise = finite_or_zero(live.spawn.motion.rise_pixels) * progress;
            let alpha = fade_alpha(&live.spawn, age, lifetime);

            // Temporary Task 2 seam: Task 3 replaces this one quad with the
            // renderer-side one-shot template lowering. The pool intentionally
            // retains the template and facts even though this seam cannot read
            // them yet.
            draw.push_quad(UiInstance::panel(
                [anchor.x - 8.0, anchor.y - 8.0 - rise, 16.0, 16.0],
                [1.0, 0.35, 0.1, alpha],
                [0.0; 4],
            ));
        }
        draw
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
    value.is_finite().then_some(value).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use glam::{Mat4, Vec3};
    use postretro_entities::{
        PresentationFade, PresentationMotion, PresentationSpawn, PresentationTemplateHandle,
    };

    use super::*;

    const EPSILON: f32 = 1.0e-5;

    fn spawn(template: &str, anchor: Vec3, lifetime_seconds: f32) -> PresentationSpawn {
        PresentationSpawn {
            world_anchor: anchor,
            template: PresentationTemplateHandle::from(template),
            facts: BTreeMap::new(),
            lifetime_seconds,
            motion: PresentationMotion { rise_pixels: 12.0 },
            fade: PresentationFade {
                duration_seconds: 0.5,
            },
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
        let _ = pool.advance_and_build_draw_data(&mut registry, 0.0, Mat4::IDENTITY, [800, 600]);

        registry.push_presentation_spawn(spawn("third", Vec3::ZERO, 1.0));
        let _ = pool.advance_and_build_draw_data(&mut registry, 0.0, Mat4::IDENTITY, [800, 600]);

        assert_eq!(pool.live_template_names(), ["second", "third"]);
    }

    #[test]
    fn pool_uses_frame_time_for_new_spawn_age_and_expiry() {
        let mut registry = EntityRegistry::new();
        let mut pool = PresentationPool::new(1);
        registry.push_presentation_spawn(spawn("impact", Vec3::ZERO, 0.2));

        let first =
            pool.advance_and_build_draw_data(&mut registry, 0.1, Mat4::IDENTITY, [800, 600]);
        assert_eq!(first.quads.len(), 1);
        assert!((pool.live_ages_seconds()[0] - 0.1).abs() < f64::from(EPSILON));
        assert!((first.quads.instances[0].color[3] - 0.5).abs() < EPSILON);

        let expired =
            pool.advance_and_build_draw_data(&mut registry, 0.11, Mat4::IDENTITY, [800, 600]);
        assert!(expired.quads.is_empty());
        assert!(pool.live_ages_seconds().is_empty());
    }

    #[test]
    fn pool_skips_behind_and_offscreen_spawns() {
        let mut registry = EntityRegistry::new();
        let mut pool = PresentationPool::new(3);
        registry.push_presentation_spawn(spawn("visible", Vec3::new(0.0, 0.0, -2.0), 1.0));
        registry.push_presentation_spawn(spawn("behind", Vec3::new(0.0, 0.0, 2.0), 1.0));
        registry.push_presentation_spawn(spawn("offscreen", Vec3::new(100.0, 0.0, -2.0), 1.0));

        let draw = pool.advance_and_build_draw_data(
            &mut registry,
            0.0,
            camera_view_projection(),
            [800, 600],
        );

        assert_eq!(draw.quads.len(), 1);
        assert!((draw.quads.instances[0].rect[0] - 392.0).abs() < EPSILON);
        assert!((draw.quads.instances[0].rect[1] - 292.0).abs() < EPSILON);
    }
}
