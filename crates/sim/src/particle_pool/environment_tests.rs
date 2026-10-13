// Particle integration through the collision volume resolver.
// See: context/lib/entity_model.md §7 (Environment Volumes), §8.

use super::tests::{close, close_vec, parameters};
use super::*;
use postretro_collision::{
    ConvexLeaf, EnvironmentView, EnvironmentVolume, EnvironmentVolumes, LeafPlane,
    resolve_particle_acceleration, resolve_particle_accelerations,
};

fn volume(gravity: Option<f32>, push: Option<Vec3>, priority: i32) -> EnvironmentVolume {
    let min = Vec3::new(-1.0, -10.0, -1.0);
    let max = Vec3::new(1.0, 10.0, 1.0);
    EnvironmentVolume {
        source_entity: 0,
        priority,
        gravity,
        push,
        fluid: Some(0),
        brushes: vec![ConvexLeaf {
            planes: vec![
                LeafPlane {
                    normal: Vec3::X,
                    distance: max.x,
                },
                LeafPlane {
                    normal: Vec3::NEG_X,
                    distance: -min.x,
                },
                LeafPlane {
                    normal: Vec3::Y,
                    distance: max.y,
                },
                LeafPlane {
                    normal: Vec3::NEG_Y,
                    distance: -min.y,
                },
                LeafPlane {
                    normal: Vec3::Z,
                    distance: max.z,
                },
                LeafPlane {
                    normal: Vec3::NEG_Z,
                    distance: -min.z,
                },
            ],
            aabb_min: min,
            aabb_max: max,
        }],
    }
}

#[test]
fn regional_push_applies_without_buoyancy_scaling_including_zero_buoyancy() {
    let volumes =
        EnvironmentVolumes::build(vec![volume(Some(-2.0), Some(Vec3::new(3.0, 4.0, 5.0)), 0)]);
    let view = EnvironmentView::new(-10.0, &volumes, &[]);
    let registry = EntityRegistry::new();
    let mut pool = ParticlePool::new();
    for (index, buoyancy) in [-1.0, 0.0, 2.0].into_iter().enumerate() {
        let mut params = parameters();
        params.buoyancy = buoyancy;
        params.drag = 0.0;
        assert!(pool.spawn_emitter(
            EntityId::from_raw(index as u32),
            &params,
            Vec3::ZERO,
            Vec3::ZERO,
            0.0
        ));
    }
    pool.step(&registry, 0.25, |position| {
        resolve_particle_acceleration(view, position)
    });
    let particles: Vec<_> = pool.iter().collect();
    close_vec(particles[0].velocity, Vec3::new(0.75, 0.5, 1.25));
    close_vec(particles[1].velocity, Vec3::new(0.75, 1.0, 1.25));
    close_vec(particles[2].velocity, Vec3::new(0.75, 2.0, 1.25));
}

#[test]
fn low_gravity_applies_inside_and_exit_step_uses_level_gravity() {
    let volumes = EnvironmentVolumes::build(vec![volume(Some(-2.0), None, 0)]);
    let view = EnvironmentView::new(-10.0, &volumes, &[]);
    let registry = EntityRegistry::new();
    let mut pool = ParticlePool::new();
    let mut params = parameters();
    params.drag = 0.0;
    assert!(pool.spawn_emitter(EntityId::from_raw(0), &params, Vec3::ZERO, Vec3::X, 0.0));
    pool.step(&registry, 0.5, |position| {
        resolve_particle_acceleration(view, position)
    });
    let inside = pool.iter().next().unwrap();
    close(inside.position.x, 0.5);
    close(inside.velocity.y, -1.0);
    pool.step(&registry, 0.6, |position| {
        resolve_particle_acceleration(view, position)
    });
    let outside = pool.iter().next().unwrap();
    close(outside.position.x, 1.1);
    close(outside.velocity.y, -7.0);
}

#[test]
fn zero_buoyancy_floats_inside_and_outside_regional_gravity() {
    let volumes = EnvironmentVolumes::build(vec![volume(Some(-100.0), None, 0)]);
    let view = EnvironmentView::new(-10.0, &volumes, &[]);
    let registry = EntityRegistry::new();
    let mut pool = ParticlePool::new();
    let mut params = parameters();
    params.buoyancy = 0.0;
    params.drag = 0.0;
    for (index, position) in [Vec3::ZERO, Vec3::X * 2.0].into_iter().enumerate() {
        assert!(pool.spawn_emitter(
            EntityId::from_raw(index as u32),
            &params,
            position,
            Vec3::ZERO,
            0.0
        ));
    }
    pool.step(&registry, 0.5, |position| {
        resolve_particle_acceleration(view, position)
    });
    for particle in pool.iter() {
        close_vec(particle.velocity, Vec3::ZERO);
    }
}

#[test]
fn overlapping_gravity_and_push_choose_their_own_priority_winners() {
    let volumes = EnvironmentVolumes::build(vec![
        volume(Some(-2.0), Some(Vec3::X), 0),
        volume(Some(-4.0), None, 5),
        volume(None, Some(Vec3::Z * 6.0), 10),
    ]);
    let view = EnvironmentView::new(-10.0, &volumes, &[]);
    let registry = EntityRegistry::new();
    let mut pool = ParticlePool::new();
    let mut params = parameters();
    params.drag = 0.0;
    assert!(pool.spawn_emitter(EntityId::from_raw(0), &params, Vec3::ZERO, Vec3::ZERO, 0.0));
    pool.step(&registry, 0.5, |position| {
        resolve_particle_acceleration(view, position)
    });
    close_vec(
        pool.iter().next().unwrap().velocity,
        Vec3::new(0.0, -2.0, 3.0),
    );
}

#[test]
fn batches_sample_advanced_positions_once_before_cross_chunk_expiry() {
    let volumes = EnvironmentVolumes::build(vec![volume(Some(-2.0), Some(Vec3::Z), 0)]);
    let view = EnvironmentView::new(-10.0, &volumes, &[]);
    let registry = EntityRegistry::new();
    let id = EntityId::from_raw(0);
    let mut pool = ParticlePool::new();
    for index in 0..131 {
        let mut params = parameters();
        params.drag = 0.0;
        params.lifetime = if index % 3 == 0 { 0.5 } else { 2.0 };
        // All start inside; alternate crossing out on this step.
        let velocity = if index % 2 == 0 {
            Vec3::X * 4.0
        } else {
            Vec3::ZERO
        };
        assert!(pool.spawn_emitter(id, &params, Vec3::ZERO, velocity, 0.0));
    }
    let mut sampled = 0;
    let mut batches = 0;
    pool.step_batched(&registry, 0.5, |positions, accelerations| {
        sampled += positions.len();
        batches += 1;
        assert_eq!(positions.len(), accelerations.len());
        resolve_particle_accelerations(view, positions, accelerations);
    });
    assert_eq!(sampled, 131);
    assert_eq!(batches, 3);
    assert_eq!(pool.len(), 87);
    assert_eq!(pool.live_count(id), 87);
    for particle in pool.iter() {
        close(particle.age, 0.5);
        if particle.position.x > 1.0 {
            close_vec(particle.velocity, Vec3::new(4.0, -5.0, 0.0));
        } else {
            close_vec(particle.velocity, Vec3::new(0.0, -1.0, 0.5));
        }
    }
    pool.step_batched(&registry, 2.0, |positions, accelerations| {
        resolve_particle_accelerations(view, positions, accelerations);
    });
    assert!(pool.is_empty());
    assert_eq!(pool.live_count(id), 0);
    assert_eq!(pool.parameter_block_count(), IMPACT_PARTICLE_COUNT);
    pool.step_batched(&registry, 0.5, |_, _| panic!("empty pool queried"));
}
