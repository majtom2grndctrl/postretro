// Pool lifecycle, parameter ownership and allocation contracts.
// See: context/plans/in-progress/perf-particle-sim-cost/index.md.

use super::*;
use crate::alloc_probe::AllocSnapshot;
use postretro_entities::Transform;
use postretro_test_log_capture::LogCapture;

pub(super) fn parameters() -> ParticleParameters {
    ParticleParameters {
        lifetime: 2.0,
        buoyancy: -1.0,
        drag: 0.2,
        size_curve: Arc::from([1.0, 3.0]),
        opacity_curve: Arc::from([1.0, 0.5]),
        collection: 4,
        tint: [0.3, 0.5, 0.7],
    }
}

fn emitter_component(spin: f32) -> BillboardEmitterComponent {
    BillboardEmitterComponent {
        rate: 10.0,
        burst: None,
        spread: 0.0,
        lifetime: 2.0,
        velocity: [0.0; 3],
        buoyancy: -1.0,
        drag: 0.2,
        size_over_lifetime: Arc::from([1.0, 3.0]),
        opacity_over_lifetime: Arc::from([1.0, 0.5]),
        color: [0.3, 0.5, 0.7],
        sprite: "smoke".into(),
        spin_rate: spin,
        spin_animation: None,
    }
}

fn emitter(registry: &mut EntityRegistry, spin: f32) -> EntityId {
    let id = registry.spawn(Transform::default());
    registry.set_component(id, emitter_component(spin)).unwrap();
    id
}

fn spawn(pool: &mut ParticlePool, id: EntityId, parameters: &ParticleParameters) {
    assert!(pool.spawn_emitter(id, parameters, Vec3::ZERO, Vec3::ZERO, 0.0));
}

pub(super) fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-5,
        "got {actual}, expected {expected}"
    );
}

pub(super) fn close_vec(actual: Vec3, expected: Vec3) {
    assert!(
        (actual - expected).abs().max_element() < 1e-5,
        "got {actual}, expected {expected}"
    );
}

#[test]
fn integration_matches_original_euler_curves_drag_and_live_then_orphan_spin() {
    let mut registry = EntityRegistry::new();
    let id = emitter(&mut registry, 1.25);
    let mut pool = ParticlePool::new();
    let mut params = parameters();
    params.lifetime = 5.0;
    params.size_curve = Arc::from([0.5, 1.25, 0.25]);
    params.opacity_curve = Arc::from([1.0, 0.75, 0.0]);
    let mut position = Vec3::new(1.0, 2.0, -3.0);
    let mut velocity = Vec3::new(2.0, 3.0, -1.0);
    assert!(pool.spawn_emitter(id, &params, position, velocity, 1.25));
    let dt = 1.0 / 120.0;
    let mut age = 0.0;
    let mut rotation = 0.0;
    let mut spin = 1.25;
    for frame in 0..480 {
        if frame == 60 {
            spin = -2.0;
            registry.set_component(id, emitter_component(spin)).unwrap();
        }
        if frame == 120 {
            registry.despawn(id).unwrap();
            let replacement = emitter(&mut registry, 100.0);
            assert_eq!(replacement.to_raw() & 0xffff, id.to_raw() & 0xffff);
            assert_ne!(replacement, id);
        }
        age += dt;
        position += velocity * dt;
        velocity.y += -9.81 * -params.buoyancy * dt;
        velocity *= (1.0 - params.drag * dt).max(0.0);
        rotation += spin * dt;
        pool.step(&registry, dt, |_| (-9.81, Vec3::ZERO));
        let particle = pool.iter().next().unwrap();
        close_vec(particle.position, position);
        close_vec(particle.velocity, velocity);
        close(particle.age, age);
        close(particle.rotation, rotation);
        // Independent three-knot interpolation, including the segment switch.
        let t = age / params.lifetime;
        let (a, b, fraction) = if t < 0.5 {
            (0, 1, t * 2.0)
        } else {
            (1, 2, (t - 0.5) * 2.0)
        };
        close(
            particle.size,
            params.size_curve[a] * (1.0 - fraction) + params.size_curve[b] * fraction,
        );
        close(
            particle.opacity,
            params.opacity_curve[a] * (1.0 - fraction) + params.opacity_curve[b] * fraction,
        );
    }
    assert_eq!(pool.live_count(id), 1);
}

#[test]
fn each_particle_samples_its_advanced_position_and_own_acceleration() {
    let registry = EntityRegistry::new();
    let mut pool = ParticlePool::new();
    let id = EntityId::from_raw(0);
    let mut params = parameters();
    params.drag = 0.0;
    assert!(pool.spawn_emitter(id, &params, Vec3::new(-1.0, 0.0, 0.0), Vec3::X * 2.0, 0.0));
    assert!(pool.spawn_emitter(id, &params, Vec3::new(-3.0, 0.0, 0.0), Vec3::X, 0.0));
    let mut calls = 0;
    pool.step(&registry, 1.0, |position| {
        calls += 1;
        if position.x > 0.0 {
            (-2.0, Vec3::Z * 3.0)
        } else {
            (-8.0, Vec3::Z * -4.0)
        }
    });
    assert_eq!(calls, 2);
    let particles: Vec<_> = pool.iter().collect();
    close_vec(particles[0].position, Vec3::X);
    close_vec(particles[0].velocity, Vec3::new(2.0, -2.0, 3.0));
    close_vec(particles[1].position, Vec3::X * -2.0);
    close_vec(particles[1].velocity, Vec3::new(1.0, -8.0, -4.0));
}

#[test]
fn spawn_time_parameters_survive_every_block_field_change() {
    let registry = EntityRegistry::new();
    let mut pool = ParticlePool::new();
    let id = EntityId::from_raw(0);
    let old = parameters();
    spawn(&mut pool, id, &old);
    let new = ParticleParameters {
        lifetime: 4.0,
        buoyancy: 2.0,
        drag: 0.8,
        size_curve: Arc::from([4.0, 8.0]),
        opacity_curve: Arc::from([0.8, 0.2]),
        collection: 9,
        tint: [0.9, 0.2, 0.1],
    };
    spawn(&mut pool, id, &new);
    assert_eq!(pool.live_count(id), 2);
    assert_eq!(pool.parameter_block_count(), IMPACT_PARTICLE_COUNT + 2);
    pool.step(&registry, 0.5, |_| (-10.0, Vec3::ZERO));
    let particles: Vec<_> = pool.iter().collect();
    let a = particles[0];
    let b = particles[1];
    close(a.lifetime, 2.0);
    close(a.velocity.y, -4.5);
    close(a.size, 1.5);
    close(a.opacity, 0.875);
    assert_eq!(a.collection, old.collection);
    for (actual, expected) in a.tint.into_iter().zip(old.tint) {
        close(actual, expected);
    }
    close(b.lifetime, 4.0);
    close(b.velocity.y, 6.0);
    close(b.size, 4.5);
    close(b.opacity, 0.725);
    assert_eq!(b.collection, new.collection);
    for (actual, expected) in b.tint.into_iter().zip(new.tint) {
        close(actual, expected);
    }
    pool.step(&registry, 1.5, |_| (-10.0, Vec3::ZERO));
    assert_eq!(pool.len(), 1);
    assert_eq!(pool.iter().next().unwrap().collection, new.collection);
    assert_eq!(pool.live_count(id), 1);
}

#[test]
fn blocks_are_lazy_content_compared_and_ignore_non_block_rewrites() {
    let mut registry = EntityRegistry::new();
    let id = emitter(&mut registry, 1.0);
    let mut pool = ParticlePool::new();
    let mut params = parameters();
    for index in 0..100 {
        params.drag = index as f32;
    }
    assert_eq!(pool.parameter_block_count(), IMPACT_PARTICLE_COUNT);
    spawn(&mut pool, id, &params);
    for index in 0..100 {
        params.drag = index as f32 + 0.5;
    }
    assert_eq!(pool.parameter_block_count(), IMPACT_PARTICLE_COUNT + 1);
    spawn(&mut pool, id, &params);
    assert_eq!(pool.parameter_block_count(), IMPACT_PARTICLE_COUNT + 2);
    // A fresh curve allocation with equal contents is the same block value.
    params.size_curve = Arc::from(params.size_curve.as_ref());
    let mut component = emitter_component(4.0);
    component.rate = 100.0;
    component.burst = Some(500);
    registry.set_component(id, component).unwrap();
    assert!(pool.spawn_emitter(id, &params, Vec3::ZERO, Vec3::ZERO, 4.0));
    pool.step(&registry, 0.0, |_| (0.0, Vec3::ZERO));
    assert_eq!(pool.parameter_block_count(), IMPACT_PARTICLE_COUNT + 2);
    assert_eq!(pool.live_count(id), 3);
    assert_eq!(pool.spin_slot_count(), 1);
    pool.step(&registry, 2.0, |_| (0.0, Vec3::ZERO));
    assert!(pool.is_empty());
    assert_eq!(pool.parameter_block_count(), IMPACT_PARTICLE_COUNT);
    assert_eq!(pool.spin_slot_count(), 0);
}

#[test]
fn recycled_current_block_never_aliases_another_emitter() {
    let registry = EntityRegistry::new();
    let a = EntityId::from_raw(0);
    let b = EntityId::from_raw(1);
    let mut pool = ParticlePool::new();
    let mut long = parameters();
    long.lifetime = 10.0;
    spawn(&mut pool, a, &long);
    let mut short = parameters();
    short.lifetime = 0.1;
    short.collection = 6;
    spawn(&mut pool, a, &short);
    pool.step(&registry, 0.2, |_| (0.0, Vec3::ZERO));
    assert_eq!(pool.live_count(a), 1);
    let mut other = parameters();
    other.collection = 99;
    spawn(&mut pool, b, &other);
    spawn(&mut pool, a, &short);
    let particles: Vec<_> = pool.iter().collect();
    assert_eq!(particles[2].collection, short.collection);
    close(particles[2].lifetime, short.lifetime);
    assert_eq!(pool.live_count(a), 2);
    assert_eq!(pool.live_count(b), 1);
}

#[test]
fn expiry_swap_removes_both_edges_without_skipping_or_repeating_survivors() {
    let registry = EntityRegistry::new();
    let id = EntityId::from_raw(0);
    let mut pool = ParticlePool::new();
    for (index, lifetime) in [0.1, 1.0, 0.1, 2.0, 0.1].into_iter().enumerate() {
        let mut params = parameters();
        params.lifetime = lifetime;
        params.drag = 0.0;
        assert!(pool.spawn_emitter(id, &params, Vec3::X * index as f32, Vec3::X, 0.0));
    }
    let mut calls = 0;
    pool.step(&registry, 0.1, |_| {
        calls += 1;
        (0.0, Vec3::ZERO)
    });
    assert_eq!(calls, 5);
    assert_eq!(pool.len(), 2);
    assert_eq!(pool.live_count(id), 2);
    for particle in pool.iter() {
        close(particle.age, 0.1);
        assert!(
            (particle.position.x - 1.1).abs() < 1e-5 || (particle.position.x - 3.1).abs() < 1e-5
        );
    }
    pool.step(&registry, 2.0, |_| (0.0, Vec3::ZERO));
    assert!(pool.is_empty());
    assert_eq!(pool.live_count(id), 0);
    assert_eq!(pool.parameter_block_count(), IMPACT_PARTICLE_COUNT);
}

#[test]
fn dense_spin_slots_recycle_without_reaching_orphans_or_other_emitters() {
    let mut registry = EntityRegistry::new();
    let a = emitter(&mut registry, 1.0);
    let b = emitter(&mut registry, 2.0);
    let c = emitter(&mut registry, 3.0);
    let mut pool = ParticlePool::new();
    let mut short = parameters();
    short.lifetime = 0.1;
    spawn(&mut pool, a, &short);
    let long = parameters();
    spawn(&mut pool, b, &long);
    spawn(&mut pool, c, &long);
    pool.step(&registry, 0.1, |_| (0.0, Vec3::ZERO));
    registry.despawn(b).unwrap();
    let replacement = emitter(&mut registry, 100.0);
    spawn(&mut pool, replacement, &long);
    pool.step(&registry, 0.1, |_| (0.0, Vec3::ZERO));
    let orphan = pool
        .iter()
        .find(|particle| particle.emitter == Some(b))
        .unwrap();
    close(orphan.rotation, 0.4);
    let c_particle = pool
        .iter()
        .find(|particle| particle.emitter == Some(c))
        .unwrap();
    close(c_particle.rotation, 0.6);
    pool.step(&registry, 3.0, |_| (0.0, Vec3::ZERO));
    assert_eq!(pool.spin_slot_count(), 0);
    assert_eq!(pool.parameter_block_count(), IMPACT_PARTICLE_COUNT);
}

#[test]
fn global_budget_mixes_impacts_and_emitters_drops_without_mint_or_eviction() {
    let registry = EntityRegistry::new();
    let mut pool = ParticlePool::new();
    let id = EntityId::from_raw(0);
    let params = parameters();
    for _ in 0..MAX_LIVE_PARTICLES - 5 {
        spawn(&mut pool, id, &params);
    }
    for index in 0..IMPACT_PARTICLE_COUNT {
        assert_eq!(pool.spawn_impact(Vec3::X, Vec3::ZERO, index), index < 5);
    }
    let original_blocks = pool.parameter_block_count();
    let mut changed = params.clone();
    changed.collection = 99;
    assert!(!pool.spawn_emitter(id, &changed, Vec3::ZERO, Vec3::ZERO, 0.0));
    assert!(!pool.spawn_emitter(EntityId::from_raw(2), &changed, Vec3::ZERO, Vec3::ZERO, 0.0));
    assert_eq!(pool.parameter_block_count(), original_blocks);
    assert_eq!(pool.spin_slot_count(), 1);
    assert_eq!(pool.len(), MAX_LIVE_PARTICLES);
    assert_eq!(pool.live_count(id), MAX_LIVE_PARTICLES - 5);
    pool.step(&registry, 0.3, |_| (0.0, Vec3::ZERO));
    assert_eq!(pool.len(), MAX_LIVE_PARTICLES - 5);
    for _ in 0..5 {
        spawn(&mut pool, id, &changed);
    }
    assert_eq!(pool.len(), MAX_LIVE_PARTICLES);
    assert_eq!(pool.live_count(id), MAX_LIVE_PARTICLES);
}

#[test]
fn overflow_warning_throttles_and_clear_resets_clock_and_warning() {
    let capture = LogCapture::start();
    let mut pool = ParticlePool::new();
    let id = EntityId::from_raw(0);
    let params = parameters();
    for _ in 0..MAX_LIVE_PARTICLES {
        spawn(&mut pool, id, &params);
    }
    pool.set_current_time(50.0);
    for _ in 0..5 {
        assert!(!pool.spawn_impact(Vec3::ZERO, Vec3::ZERO, 0));
    }
    pool.set_current_time(50.9);
    assert!(!pool.spawn_impact(Vec3::ZERO, Vec3::ZERO, 0));
    pool.set_current_time(51.0);
    assert!(!pool.spawn_impact(Vec3::ZERO, Vec3::ZERO, 0));
    pool.clear();
    for _ in 0..MAX_LIVE_PARTICLES {
        spawn(&mut pool, id, &params);
    }
    pool.set_current_time(0.1);
    assert!(!pool.spawn_impact(Vec3::ZERO, Vec3::ZERO, 0));
    let warnings = capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level == log::Level::Warn && record.message.contains("[Particles] live budget")
        })
        .count();
    assert_eq!(warnings, 3);
}

#[test]
fn impact_blocks_are_static_reusable_and_keep_exact_parameters_after_clear() {
    let registry = EntityRegistry::new();
    let mut pool = ParticlePool::new();
    pool.configure_impact_collection(77);
    for index in 0..IMPACT_PARTICLE_COUNT {
        assert!(pool.spawn_impact(Vec3::ZERO, Vec3::X, index));
    }
    // Repeated frame configuration is allowed when the resolved index agrees.
    pool.configure_impact_collection(77);
    assert_eq!(pool.parameter_block_count(), IMPACT_PARTICLE_COUNT);
    for (index, particle) in pool.iter().enumerate() {
        close(particle.lifetime, 0.18 * (0.8 + index as f32 * 0.025));
        close(particle.rotation, index as f32 * 0.73);
        assert_eq!(particle.collection, 77);
        assert_eq!(particle.emitter, None);
    }
    pool.step(&registry, 0.01, |_| (-100.0, Vec3::ZERO));
    close(pool.iter().next().unwrap().velocity.x, 0.96);
    close(pool.iter().next().unwrap().velocity.y, 0.0);
    let columns_capacity = pool.positions.capacity();
    let block_capacity = pool.blocks.capacity();
    let spin_capacity = pool.spins.capacity();
    pool.clear();
    assert!(pool.is_empty());
    assert_eq!(pool.parameter_block_count(), IMPACT_PARTICLE_COUNT);
    assert_eq!(pool.positions.capacity(), columns_capacity);
    assert_eq!(pool.blocks.capacity(), block_capacity);
    assert_eq!(pool.spins.capacity(), spin_capacity);
    pool.configure_impact_collection(88);
    assert!(pool.spawn_impact(Vec3::ZERO, Vec3::ZERO, 0));
    assert_eq!(pool.iter().next().unwrap().collection, 88);
    assert_eq!(pool.parameter_block_count(), IMPACT_PARTICLE_COUNT);
}

fn frame_allocation_count(count: usize, rewrite_curves: bool) -> usize {
    let registry = EntityRegistry::new();
    let id = EntityId::from_raw(0);
    let mut pool = ParticlePool::new();
    let mut params = parameters();
    params.lifetime = 0.02;
    // Warm both parameter slots with one cohort surviving a subsequent frame.
    for _ in 0..count {
        spawn(&mut pool, id, &params);
    }
    pool.step(&registry, 0.01, |_| (-9.81, Vec3::ZERO));
    let probe = AllocSnapshot::arm();
    if rewrite_curves {
        params.size_curve = Arc::from([0.2, 0.8, 0.1]);
        params.opacity_curve = Arc::from([1.0, 0.0]);
        params.drag = 1.0;
    }
    for _ in 0..count {
        spawn(&mut pool, id, &params);
    }
    pool.step(&registry, 0.01, |_| (-9.81, Vec3::ZERO));
    pool.step(&registry, 0.02, |_| (-9.81, Vec3::ZERO));
    let allocations = probe.allocs_since();
    assert!(pool.is_empty());
    assert_eq!(pool.parameter_block_count(), IMPACT_PARTICLE_COUNT);
    allocations
}

#[test]
fn spawn_integrate_expire_allocations_do_not_scale_with_particle_count() {
    let small = frame_allocation_count(100, false);
    let large = frame_allocation_count(10_000, false);
    assert_eq!(small, 0);
    assert_eq!(large, 0);
    let small_rewrite = frame_allocation_count(100, true);
    let large_rewrite = frame_allocation_count(10_000, true);
    assert_eq!(
        small_rewrite, 2,
        "only the emitter's two authored curve allocations"
    );
    assert_eq!(large_rewrite, small_rewrite);
}
