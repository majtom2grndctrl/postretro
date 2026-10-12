// Particle integration through the collision volume resolver.
// See: context/lib/entity_model.md §7 (Environment Volumes), §8.

use super::tests::{close, close_vec, parameters};
use super::*;
use postretro_collision::{
    ConvexLeaf, EnvironmentView, EnvironmentVolume, EnvironmentVolumes, LeafPlane,
    resolve_particle_acceleration,
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
