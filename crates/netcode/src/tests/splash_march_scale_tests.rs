//! S2 (`E17--movers-block-shots-and-sight`): host march cost per tick with
//! four clients firing a splash weapon continuously through cycling doors,
//! against the host player's own rockets at the same rate. A synthetic room
//! shaped like `mover-setpiece` (a 32 m room, three sliding doors across the
//! firing lines); it proves nothing and only prints.
//!
//! Run release, single-threaded, nothing heavy beside it:
//!   cargo test --release -p postretro-netcode --lib splash_march_scale_row -- --ignored --nocapture --test-threads=1
//! See: context/lib/testing_guide.md §Resource bounds

use super::*;
use crate::collision::moving::CombinedCollisionWorld;
use crate::collision::test_movers::{DoorPoses, door_slab};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

const WARM_TICKS: u32 = 120;
const SAMPLED_TICKS: u32 = 1_800;
/// The reference rocket launcher: 30 m/s, radius 0.25, 4 s, 750 ms recovery.
const SPEED: f32 = 30.0;
const RADIUS: f32 = 0.25;
const LIFETIME: f32 = 4.0;
const FIRE_EVERY_TICKS: u32 = 45;
const CLIENTS: u32 = 4;
const DT: f32 = 1.0 / 60.0;

fn room() -> CollisionWorld {
    // A 32 m box: floor, ceiling and four walls.
    let (lo, hi) = (Vec3::new(-16.0, -1.0, -16.0), Vec3::new(16.0, 7.0, 16.0));
    let corners = [
        Vec3::new(lo.x, lo.y, lo.z),
        Vec3::new(hi.x, lo.y, lo.z),
        Vec3::new(hi.x, hi.y, lo.z),
        Vec3::new(lo.x, hi.y, lo.z),
        Vec3::new(lo.x, lo.y, hi.z),
        Vec3::new(hi.x, lo.y, hi.z),
        Vec3::new(hi.x, hi.y, hi.z),
        Vec3::new(lo.x, hi.y, hi.z),
    ];
    let faces = [
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [3, 6, 2],
        [3, 7, 6],
        [0, 4, 7],
        [0, 7, 3],
        [1, 2, 6],
        [1, 6, 5],
    ];
    CollisionWorld::from_triangles_for_test(corners.to_vec(), faces.to_vec())
}

/// Three doors sliding across the room's x axis, each on its own cycle.
fn door_poses(tick: u32) -> DoorPoses {
    let mut poses = DoorPoses::default();
    for (k, x) in [-6.0_f32, 2.0, 10.0].into_iter().enumerate() {
        let phase = (tick as f32 * DT * 0.5 + k as f32 * 0.7).sin();
        poses.set(
            20 + k as u32,
            Vec3::new(x, -1.0, phase * 6.0),
            glam::Quat::IDENTITY,
        );
    }
    poses
}

fn shooter_origin(k: u32) -> Vec3 {
    Vec3::new(-14.0, 1.5, -6.0 + 4.0 * k as f32)
}

fn shot(k: u32, n: u32, fire_tick: u32) -> AuthorizedShot {
    let pawn = EntityId::from_raw(1 + k);
    let mut shot = authorized_test_shot(
        ShotId::from_parts(
            10 + k,
            n,
            postretro_foundation::ActivationLane::Primary,
            0,
        ),
        pawn,
        EntityId::from_raw(100 + k),
        fire_tick,
        36.0,
        128.0,
    );
    shot.is_projectile = true;
    shot.splash = Some(SplashDescriptor {
        knockback: None,
        radius: 5.0,
        min_fraction: 0.0,
        self_damage: true,
    });
    shot.projectile_radius = Some(RADIUS);
    shot.projectile_direction = Some(Vec3::X);
    shot.projectile_speed = Some(SPEED);
    shot.projectile_lifetime_seconds = Some(LIFETIME);
    shot.projectile_tick_seconds = Some(DT);
    shot.splash_on_path_tolerance = Some(RADIUS + 0.84 + 0.25);
    shot.fire_origin = shooter_origin(k);
    shot.timeout_budget_ticks = 600;
    shot
}

fn host_rocket(registry: &mut EntityRegistry, owner: EntityId, origin: Vec3) {
    let projectile = registry.spawn(Transform {
        position: origin,
        ..Transform::default()
    });
    registry
        .set_component(
            projectile,
            postretro_entities::components::projectile::ProjectileComponent {
                source_sounds: None,
                predicted_visible: true,
                source_action: None,
                source_shot: None,
                knockback_impulse: [0.0; 3],
                direction: Vec3::X.to_array(),
                speed: SPEED,
                radius: RADIUS,
                remaining_range: 128.0,
                remaining_lifetime: LIFETIME,
                damage: 36.0,
                credit_source: "weapon.test.scale".to_string(),
                owner_pawn: owner,
                owner_life: 0,
                owner_weapon: owner,
                attackerless: false,
                spawned: false,
                predicted_shot_id: None,
                elapsed_flight_age: 0.0,
                flipbook_active: false,
                impact_light: None,
                splash: Some(SplashDescriptor {
                    knockback: None,
                    radius: 5.0,
                    min_fraction: 0.0,
                    self_damage: true,
                }),
                source_weapon: None,
                activation: None,
                damage_type: None,
            },
        )
        .unwrap();
}

fn percentile(samples: &mut [Duration], at: f64) -> f64 {
    samples.sort();
    let index = ((samples.len() as f64 - 1.0) * at).round() as usize;
    samples[index].as_secs_f64() * 1e3
}

#[test]
#[ignore = "measurement: run release with --ignored --nocapture --test-threads=1"]
fn splash_march_scale_row() {
    let world = room();
    let movers: Vec<_> = (0..3).map(|k| door_slab(20 + k, 0.1, 6.0, 3.0)).collect();
    let hit_zones = crate::scripting_systems::hit_zones::HitZoneStore::new();
    let allocator = NetworkIdAllocator::new();

    // Four clients' marches.
    let mut registry = EntityRegistry::new();
    let mut open_shots = OpenAuthorizedShots::new();
    let mut march_samples = Vec::new();
    let mut open_marches = 0usize;
    let mut fired = [0u32; CLIENTS as usize];
    for tick in 1..=WARM_TICKS + SAMPLED_TICKS {
        for k in 0..CLIENTS {
            if (tick + k * 11) % FIRE_EVERY_TICKS == 0 {
                fired[k as usize] += 1;
                let shot = shot(k, fired[k as usize], tick);
                let id = shot.shot_id;
                open_shots.record(shot, 7);
                // The client declares its wall contact at once; cost is the march.
                open_shots.bind_splash_declaration(
                    id,
                    &wire::HitDeclaration {
                        shot_id: crate::wire_convert::shot_id_to_wire(id),
                        records: vec![world_contact(
                            Vec3::new(16.0, 1.5, shooter_origin(k).z),
                            Vec3::NEG_X,
                        )],
                    },
                );
                open_shots.retire(id);
            }
        }
        let poses = door_poses(tick);
        let combined = CombinedCollisionWorld::new(&world, &movers, &poses);
        let started = Instant::now();
        let released = crate::splash_march::step_and_release(
            &mut registry,
            &combined,
            &hit_zones,
            &allocator,
            &mut open_shots,
            tick,
            &mut |_| {},
        );
        let elapsed = started.elapsed();
        std::hint::black_box(released);
        if tick > WARM_TICKS {
            march_samples.push(elapsed);
            open_marches += open_shots.marches.len();
        }
    }

    // The host player firing the same weapon at the same rate.
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let owner = registry.borrow_mut().spawn(Transform::default());
    let mut host_samples = Vec::new();
    let mut in_flight = 0usize;
    for tick in 1..=WARM_TICKS + SAMPLED_TICKS {
        if tick % FIRE_EVERY_TICKS == 0 {
            host_rocket(&mut registry.borrow_mut(), owner, shooter_origin(0));
        }
        let poses = door_poses(tick);
        let combined = CombinedCollisionWorld::new(&world, &movers, &poses);
        let started = Instant::now();
        let contacts =
            crate::sim::advance(&registry, &combined, &hit_zones, 0.0, DT, &mut |_| {});
        let elapsed = started.elapsed();
        std::hint::black_box(contacts);
        if tick > WARM_TICKS {
            host_samples.push(elapsed);
            in_flight += registry
                .borrow()
                .iter_with_kind(ComponentKind::Projectile)
                .count();
        }
    }

    let ticks = SAMPLED_TICKS as f64;
    let mean = |samples: &[Duration]| {
        samples.iter().map(Duration::as_secs_f64).sum::<f64>() * 1e3 / samples.len() as f64
    };
    let march_mean = mean(&march_samples);
    let host_mean = mean(&host_samples);
    let marches_per_tick = open_marches as f64 / ticks;
    let rockets_per_tick = in_flight as f64 / ticks;
    eprintln!(
        "[SplashMarch] {SAMPLED_TICKS} sampled ticks; 3 cycling doors; reference rocket 30 m/s, radius 0.25, one shot per {FIRE_EVERY_TICKS} ticks per shooter",
    );
    eprintln!(
        "[SplashMarch] 4 clients' marches   | open {marches_per_tick:>5.2}/tick | p50 {:.4} p99 {:.4} worst {:.4} mean {march_mean:.4} ms/tick | {:.2} µs per march per tick",
        percentile(&mut march_samples.clone(), 0.5),
        percentile(&mut march_samples.clone(), 0.99),
        percentile(&mut march_samples, 1.0),
        march_mean * 1e3 / marches_per_tick.max(1e-9),
    );
    eprintln!(
        "[SplashMarch] host player's rockets | live {rockets_per_tick:>5.2}/tick | p50 {:.4} p99 {:.4} worst {:.4} mean {host_mean:.4} ms/tick | {:.2} µs per rocket per tick",
        percentile(&mut host_samples.clone(), 0.5),
        percentile(&mut host_samples.clone(), 0.99),
        percentile(&mut host_samples, 1.0),
        host_mean * 1e3 / rockets_per_tick.max(1e-9),
    );
}
