// CPU particle simulation: integrates ParticleState entities each game-logic tick.
// See: context/plans/in-progress/scripting-foundation/plan-3-emitter-entity.md §Sub-plan 2

use glam::Vec3;

use crate::scripting::components::billboard_emitter::BillboardEmitterComponent;
use crate::scripting::components::particle::ParticleState;
use crate::scripting::components::sprite_visual::SpriteVisual;
use crate::scripting::registry::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform,
};

/// World gravity in m/s². Negative is "down" by convention. Combined with
/// `BillboardEmitterComponent::buoyancy` per the plan's sign convention:
/// `vertical_accel = WORLD_GRAVITY * -buoyancy`. So `buoyancy = -1` falls at
/// `-9.81 m/s²`, `buoyancy = 0` floats, `buoyancy > 0` rises.
pub(crate) const WORLD_GRAVITY: f32 = -9.81;

/// Linear-interpolated curve evaluation over `[0, 1]`. Empty curve defaults to
/// `1.0` (unreachable from script — reserved for Rust-side defaulting).
fn eval_curve(curve: &[f32], t: f32) -> f32 {
    if curve.is_empty() {
        return 1.0;
    }
    if curve.len() == 1 {
        return curve[0];
    }
    let s = t * (curve.len() - 1) as f32;
    let i = s.floor() as usize;
    let frac = s - i as f32;
    let a = curve[i];
    let b = curve[(i + 1).min(curve.len() - 1)];
    a * (1.0 - frac) + b * frac
}

/// Advance every `ParticleState` entity by `delta` seconds. Two-pass: collect
/// snapshots, mutate, then despawn expired particles after the iteration so
/// the registry is never mutated mid-walk.
///
/// Frame ordering: this runs after the emitter bridge (Sub-plan 3) and before
/// the light bridge, so newly spawned particles tick once before render.
pub(crate) fn tick(registry: &mut EntityRegistry, delta: f32) {
    // Pass 1: gather (id, snapshot) so we drop the immutable iterator borrow
    // before issuing the mutating writes below. ParticleState clones are
    // cheap at particle scale (curves are short Vec<f32>).
    let mut snapshots: Vec<(EntityId, ParticleState)> = Vec::new();
    for (id, value) in registry.iter_with_kind(ComponentKind::ParticleState) {
        let ComponentValue::ParticleState(state) = value else {
            continue;
        };
        snapshots.push((id, state.clone()));
    }

    let mut to_despawn: Vec<EntityId> = Vec::new();

    for (id, mut state) in snapshots {
        state.age += delta;

        // Position integration: read the live Transform, advance, write back.
        let mut position = match registry.get_component::<Transform>(id) {
            Ok(t) => t.position,
            Err(_) => continue,
        };
        let velocity_vec = Vec3::from_array(state.velocity);
        position += velocity_vec * delta;

        // Velocity integration: gravity + buoyancy on Y, then drag damping.
        let mut velocity = velocity_vec;
        velocity.y += WORLD_GRAVITY * -state.buoyancy * delta;
        let damping = (1.0 - state.drag * delta).max(0.0);
        velocity *= damping;
        state.velocity = velocity.to_array();

        // Curve evaluation at normalized age. lifetime > 0 is enforced at the
        // FFI boundary; a zero/negative slipping through would yield t = ∞ or
        // NaN, so guard with max(0).
        let lifetime = state.lifetime.max(f32::MIN_POSITIVE);
        let t = (state.age / lifetime).clamp(0.0, 1.0);
        let size = eval_curve(&state.size_curve, t);
        let opacity = eval_curve(&state.opacity_curve, t);

        // Spin: read live emitter spin_rate every tick (so reactions and
        // tweens take effect immediately). Orphaned particles tick at 0.
        let spin_rate = match state.emitter {
            Some(parent) => match registry.get_component::<BillboardEmitterComponent>(parent) {
                Ok(emitter) => emitter.spin_rate,
                Err(_) => 0.0,
            },
            None => 0.0,
        };

        // Update the visual. Read-modify-write keeps any future fields the sim
        // does not own (sprite, tint) intact.
        let mut visual = match registry.get_component::<SpriteVisual>(id) {
            Ok(v) => v.clone(),
            Err(_) => continue,
        };
        visual.size = size;
        visual.opacity = opacity;
        visual.rotation += spin_rate * delta;

        // Update Transform.position.
        let mut transform = *registry
            .get_component::<Transform>(id)
            .expect("Transform existed at the top of this iteration");
        transform.position = position;
        let _ = registry.set_component(id, transform);
        let _ = registry.set_component(id, visual);

        let expired = state.age >= state.lifetime;
        let _ = registry.set_component(id, state);

        if expired {
            to_despawn.push(id);
        }
    }

    // Pass 2: despawn after iteration. Stale-id errors are ignored — the
    // entity may have been removed by another system between passes.
    for id in to_despawn {
        let _ = registry.despawn(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::components::billboard_emitter::BillboardEmitterComponent;

    fn default_emitter_component() -> BillboardEmitterComponent {
        BillboardEmitterComponent {
            rate: 0.0,
            burst: None,
            spread: 0.0,
            lifetime: 1.0,
            initial_velocity: [0.0, 0.0, 0.0],
            buoyancy: 0.0,
            drag: 0.0,
            size_over_lifetime: vec![1.0],
            opacity_over_lifetime: vec![1.0],
            color: [1.0, 1.0, 1.0],
            sprite: "smoke".into(),
            spin_rate: 0.0,
            spin_animation: None,
        }
    }

    fn spawn_particle(
        registry: &mut EntityRegistry,
        velocity: [f32; 3],
        lifetime: f32,
        buoyancy: f32,
        drag: f32,
        size_curve: Vec<f32>,
        opacity_curve: Vec<f32>,
        emitter: Option<EntityId>,
    ) -> EntityId {
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                ParticleState {
                    velocity,
                    age: 0.0,
                    lifetime,
                    buoyancy,
                    drag,
                    size_curve,
                    opacity_curve,
                    emitter,
                },
            )
            .unwrap();
        registry
            .set_component(
                id,
                SpriteVisual {
                    sprite: "smoke".into(),
                    size: 0.0,
                    opacity: 0.0,
                    rotation: 0.0,
                    tint: [1.0, 1.0, 1.0],
                },
            )
            .unwrap();
        id
    }

    #[test]
    fn eval_curve_handles_empty_single_and_endpoints() {
        assert_eq!(eval_curve(&[], 0.5), 1.0);
        assert_eq!(eval_curve(&[0.7], 0.0), 0.7);
        assert_eq!(eval_curve(&[0.7], 1.0), 0.7);
        let curve = [0.5_f32, 1.0, 0.5];
        assert!((eval_curve(&curve, 0.0) - 0.5).abs() < 1e-6);
        assert!((eval_curve(&curve, 0.5) - 1.0).abs() < 1e-6);
        assert!((eval_curve(&curve, 1.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn parabolic_trajectory_under_normal_gravity() {
        // velocity = (0, 5, 0), buoyancy = -1 → vertical_accel = WORLD_GRAVITY.
        // y(t) = 5t + 0.5 * WORLD_GRAVITY * t^2 (semi-implicit Euler at fixed
        // step matches the analytic curve up to O(dt) integration error; we
        // use a small step and a generous tolerance).
        let mut reg = EntityRegistry::new();
        let id = spawn_particle(
            &mut reg,
            [0.0, 5.0, 0.0],
            10.0,
            -1.0,
            0.0,
            vec![1.0],
            vec![1.0],
            None,
        );
        let dt = 1.0_f32 / 240.0;
        let mut elapsed = 0.0_f32;
        let samples = [0.25_f32, 0.5, 1.0];
        let mut next_sample = 0;
        while next_sample < samples.len() {
            tick(&mut reg, dt);
            elapsed += dt;
            if elapsed + 0.5 * dt >= samples[next_sample] {
                let t = elapsed;
                let analytic_y = 5.0 * t + 0.5 * WORLD_GRAVITY * t * t;
                let pos_y = reg.get_component::<Transform>(id).unwrap().position.y;
                assert!(
                    (pos_y - analytic_y).abs() < 0.05,
                    "at t={t}: got {pos_y}, expected ~{analytic_y}"
                );
                next_sample += 1;
            }
        }
    }

    #[test]
    fn drag_decays_velocity_to_near_zero_within_four_lifetimes() {
        let mut reg = EntityRegistry::new();
        // Long lifetime so the particle does not despawn during the test.
        let id = spawn_particle(
            &mut reg,
            [10.0, 0.0, 0.0],
            1000.0,
            0.0,
            1.0,
            vec![1.0],
            vec![1.0],
            None,
        );
        // Simulate 4 lifetimes (= 4 seconds) of drag-relevant time. With
        // drag = 1.0 the continuous decay is exp(-t); discrete-Euler with
        // small dt converges to that. After 4 seconds, |v| ≈ 10 * exp(-4)
        // ≈ 0.18 — small relative to the initial 10 m/s.
        let dt = 1.0_f32 / 2000.0;
        for _ in 0..8000 {
            tick(&mut reg, dt);
        }
        let vx = reg.get_component::<ParticleState>(id).unwrap().velocity[0];
        // 5% of initial magnitude; matches the analytic bound above with
        // headroom for first-order Euler error (1 - drag*dt) vs exp(-drag*dt).
        assert!(vx.abs() < 0.5, "drag failed to damp velocity: vx = {vx}");
    }

    #[test]
    fn size_curve_endpoints_and_midpoint() {
        // size_over_lifetime = [0.5, 1.0, 0.5] should produce 0.5 at t=0,
        // 1.0 at t=0.5, 0.5 at t=1.0.
        let mut reg = EntityRegistry::new();
        let id = spawn_particle(
            &mut reg,
            [0.0, 0.0, 0.0],
            1.0,
            0.0,
            0.0,
            vec![0.5, 1.0, 0.5],
            vec![1.0],
            None,
        );

        // After a tiny tick, age ≈ 0 → size ≈ 0.5.
        tick(&mut reg, 1e-4);
        let v = reg.get_component::<SpriteVisual>(id).unwrap();
        assert!((v.size - 0.5).abs() < 1e-3, "size at t=0: {}", v.size);

        // Advance to t = 0.5 (lifetime = 1.0).
        for _ in 0..4999 {
            tick(&mut reg, 1e-4);
        }
        let v = reg.get_component::<SpriteVisual>(id).unwrap();
        assert!((v.size - 1.0).abs() < 1e-2, "size at t=0.5: {}", v.size);
    }

    #[test]
    fn particle_at_lifetime_is_despawned_in_same_tick() {
        let mut reg = EntityRegistry::new();
        let id = spawn_particle(
            &mut reg,
            [0.0, 0.0, 0.0],
            0.05,
            0.0,
            0.0,
            vec![1.0],
            vec![1.0],
            None,
        );
        tick(&mut reg, 0.1);
        assert!(!reg.exists(id), "particle past lifetime must be despawned");
    }

    #[test]
    fn spin_rate_two_pi_completes_one_full_rotation_per_second() {
        let mut reg = EntityRegistry::new();
        let emitter_id = reg.spawn(Transform::default());
        let mut emitter = default_emitter_component();
        emitter.spin_rate = std::f32::consts::TAU;
        reg.set_component(emitter_id, emitter).unwrap();
        let id = spawn_particle(
            &mut reg,
            [0.0, 0.0, 0.0],
            10.0,
            0.0,
            0.0,
            vec![1.0],
            vec![1.0],
            Some(emitter_id),
        );

        let dt = 1.0_f32 / 240.0;
        // 0.5 seconds → 120 steps.
        for _ in 0..120 {
            tick(&mut reg, dt);
        }
        let rotation = reg.get_component::<SpriteVisual>(id).unwrap().rotation;
        assert!(
            (rotation - std::f32::consts::PI).abs() < 1e-2,
            "rotation at t=0.5s: {rotation}, expected ~π"
        );
    }

    #[test]
    fn orphaned_particle_retains_rotation_without_panicking() {
        let mut reg = EntityRegistry::new();
        let emitter_id = reg.spawn(Transform::default());
        let mut emitter = default_emitter_component();
        emitter.spin_rate = std::f32::consts::TAU;
        reg.set_component(emitter_id, emitter).unwrap();
        let id = spawn_particle(
            &mut reg,
            [0.0, 0.0, 0.0],
            10.0,
            0.0,
            0.0,
            vec![1.0],
            vec![1.0],
            Some(emitter_id),
        );

        // Tick once with the emitter live so rotation accumulates.
        tick(&mut reg, 0.1);
        let rotation_before = reg.get_component::<SpriteVisual>(id).unwrap().rotation;
        assert!(rotation_before > 0.0, "rotation must have advanced");

        // Despawn the parent emitter — particle is now orphaned.
        reg.despawn(emitter_id).unwrap();

        // Further ticks must not panic and rotation must not advance.
        tick(&mut reg, 0.1);
        tick(&mut reg, 0.1);
        let rotation_after = reg.get_component::<SpriteVisual>(id).unwrap().rotation;
        assert!(
            (rotation_after - rotation_before).abs() < 1e-6,
            "orphaned particle rotation should not advance; before {rotation_before}, after {rotation_after}"
        );
    }

    #[test]
    #[ignore = "release-mode bench; run with `cargo test --release -- --ignored`"]
    fn bench_500_particles_one_frame_under_half_a_millisecond() {
        let mut reg = EntityRegistry::new();
        let emitter_id = reg.spawn(Transform::default());
        reg.set_component(emitter_id, default_emitter_component())
            .unwrap();
        for _ in 0..500 {
            spawn_particle(
                &mut reg,
                [0.0, 1.0, 0.0],
                10.0,
                -1.0,
                0.5,
                vec![0.3, 1.0, 0.5],
                vec![0.0, 0.8, 0.0],
                Some(emitter_id),
            );
        }
        let dt = 1.0_f32 / 60.0;
        let start = std::time::Instant::now();
        tick(&mut reg, dt);
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_micros() < 500,
            "500-particle tick took {:?}; expected < 500µs",
            elapsed
        );
    }
}
