// Emitter admission, frame ordering and end-to-end allocation proofs.
// See: context/lib/entity_model.md §8

use super::*;
use crate::alloc_probe::AllocSnapshot;
use postretro_foundation::ImpactParticleRequest;
use postretro_render_cpu::smoke::SPRITE_INSTANCE_SIZE;
use postretro_sim::particle_pool::{IMPACT_PARTICLE_COUNT, MAX_LIVE_PARTICLES};
use postretro_visibility::VisibleCells;

struct FrameHarness {
    registry: EntityRegistry,
    bridge: EmitterBridge,
    pool: ParticlePool,
    collector: ParticleRenderCollector,
    requests: Vec<ImpactParticleRequest>,
}

impl FrameHarness {
    fn new(lifetime: f32) -> Self {
        let mut collector = ParticleRenderCollector::new();
        collector.register_sprite(&derive_collection_id(
            "smoke",
            Some(lifetime),
            None,
            0.0,
            None,
            None,
        ));
        let impact = collector.register_sprite(crate::weapon::impact_collection_id());
        let mut pool = ParticlePool::new();
        pool.configure_impact_collection(impact);
        Self {
            registry: EntityRegistry::new(),
            bridge: EmitterBridge::new(),
            pool,
            collector,
            requests: Vec::new(),
        }
    }

    fn spawn(&mut self, component: BillboardEmitterComponent) -> EntityId {
        let id = self.registry.spawn(Transform::default());
        self.registry.set_component(id, component).unwrap();
        id
    }

    fn update(&mut self, delta: f32, time: f32) {
        self.bridge.update(
            &mut self.registry,
            delta,
            delta,
            time,
            &mut self.pool,
            &mut self.collector,
        );
    }

    fn drain(&mut self) {
        crate::weapon::drain_impact_particle_requests(
            &mut self.registry,
            &mut self.pool,
            &mut self.requests,
        );
    }

    fn step_and_collect(&mut self, delta: f32) {
        self.step_and_collect_in(delta, postretro_collision::EnvironmentView::level_only(0.0));
    }

    fn step_and_collect_in(
        &mut self,
        delta: f32,
        environment: postretro_collision::EnvironmentView<'_>,
    ) {
        postretro_sim::scripting_systems::particle_sim::tick_batched(
            &mut self.pool,
            &self.registry,
            delta,
            |positions, accelerations| {
                postretro_collision::resolve_particle_accelerations(
                    environment,
                    positions,
                    accelerations,
                )
            },
        );
        self.collector.collect_at_tick(
            &self.registry,
            &self.pool,
            None,
            &VisibleCells::DrawAll,
            0.0,
            None,
        );
    }

    fn fill(&mut self, count: usize, lifetime: f32) -> EntityId {
        let filler = EntityId::from_raw(u32::MAX);
        let parameters = ParticleParameters {
            lifetime,
            buoyancy: 0.0,
            drag: 0.0,
            size_curve: [1.0].into(),
            opacity_curve: [1.0].into(),
            collection: self
                .collector
                .resolve_collection(crate::weapon::impact_collection_id())
                .unwrap(),
            tint: [1.0; 3],
        };
        for _ in 0..count {
            assert!(
                self.pool
                    .spawn_emitter(filler, &parameters, Vec3::ZERO, Vec3::ZERO, 0.0)
            );
        }
        filler
    }

    fn bytes_len(&self) -> usize {
        self.collector
            .iter_collections()
            .map(|(_, bytes)| bytes.len())
            .sum()
    }
}

fn component(lifetime: f32) -> BillboardEmitterComponent {
    BillboardEmitterComponent {
        rate: 0.0,
        burst: None,
        spread: 0.0,
        lifetime,
        velocity: [0.0, 2.0, 0.0],
        buoyancy: 0.0,
        drag: 0.0,
        size_over_lifetime: [1.0].into(),
        opacity_over_lifetime: [1.0].into(),
        color: [1.0; 3],
        sprite: "smoke".into(),
        spin_rate: 0.0,
        spin_animation: None,
    }
}

fn rewrite(
    harness: &mut FrameHarness,
    id: EntityId,
    edit: impl FnOnce(&mut BillboardEmitterComponent),
) {
    let mut emitter = harness
        .registry
        .get_component::<BillboardEmitterComponent>(id)
        .unwrap()
        .clone();
    edit(&mut emitter);
    harness.registry.set_component(id, emitter).unwrap();
}

#[test]
fn emitter_cap_counts_old_and_new_parameter_blocks_together() {
    let mut harness = FrameHarness::new(5.0);
    let mut emitter = component(5.0);
    emitter.burst = Some((MAX_SPRITES - 2) as u32);
    let id = harness.spawn(emitter);
    harness.update(0.01, 0.01);
    rewrite(&mut harness, id, |emitter| {
        emitter.size_over_lifetime = [3.0].into();
        emitter.color = [0.25; 3];
        emitter.burst = Some(5);
    });
    harness.update(0.01, 0.02);
    assert_eq!(harness.pool.live_count(id), MAX_SPRITES);
    assert_eq!(
        harness.pool.parameter_block_count(),
        IMPACT_PARTICLE_COUNT + 2
    );
    assert_eq!(
        harness
            .pool
            .iter()
            .filter(|particle| (particle.size - 1.0).abs() < 1e-6)
            .count(),
        MAX_SPRITES - 2
    );
    assert_eq!(
        harness
            .pool
            .iter()
            .filter(|particle| (particle.size - 3.0).abs() < 1e-6)
            .count(),
        2
    );
    assert!(
        harness
            .registry
            .get_component::<BillboardEmitterComponent>(id)
            .unwrap()
            .burst
            .is_none()
    );
    assert_eq!(
        harness
            .registry
            .iter_with_kind(ComponentKind::Transform)
            .count(),
        1
    );
}

#[test]
fn emitter_burst_fills_only_remaining_global_budget_without_evicting_survivors() {
    let mut harness = FrameHarness::new(5.0);
    let filler = harness.fill(MAX_LIVE_PARTICLES - 3, 100.0);
    let mut emitter = component(5.0);
    emitter.burst = Some(8);
    let id = harness.spawn(emitter);
    harness.update(0.01, 0.01);
    assert_eq!(harness.pool.len(), MAX_LIVE_PARTICLES);
    assert_eq!(harness.pool.live_count(filler), MAX_LIVE_PARTICLES - 3);
    assert_eq!(harness.pool.live_count(id), 3);
    assert!(
        harness
            .registry
            .get_component::<BillboardEmitterComponent>(id)
            .unwrap()
            .burst
            .is_none()
    );
}

#[test]
fn impact_intake_precedes_several_emitters_at_shared_global_budget() {
    let mut harness = FrameHarness::new(5.0);
    let filler = harness.fill(MAX_LIVE_PARTICLES - IMPACT_PARTICLE_COUNT - 4, 100.0);
    let ids: Vec<_> = (0..3)
        .map(|_| {
            let mut emitter = component(5.0);
            emitter.burst = Some(4);
            harness.spawn(emitter)
        })
        .collect();
    crate::weapon::spawn_impact_effect_at(
        &mut harness.registry,
        Vec3::new(10.0, 0.0, 0.0),
        Vec3::Y,
    );
    harness.drain();
    assert_eq!(harness.pool.len(), MAX_LIVE_PARTICLES - 4);
    assert_eq!(
        harness
            .pool
            .iter()
            .filter(|particle| particle.emitter.is_none())
            .count(),
        IMPACT_PARTICLE_COUNT
    );
    harness.update(0.01, 0.01);
    assert_eq!(harness.pool.len(), MAX_LIVE_PARTICLES);
    assert_eq!(
        harness.pool.live_count(filler),
        MAX_LIVE_PARTICLES - IMPACT_PARTICLE_COUNT - 4
    );
    assert_eq!(harness.pool.live_count(ids[0]), 4);
    assert_eq!(harness.pool.live_count(ids[1]), 0);
    assert_eq!(harness.pool.live_count(ids[2]), 0);
    harness.drain();
    assert_eq!(
        harness.pool.len(),
        MAX_LIVE_PARTICLES,
        "drained requests never replay"
    );
}

#[test]
fn blocked_rate_drops_backlog_and_retains_fraction_when_budget_room_frees() {
    let mut harness = FrameHarness::new(5.0);
    harness.fill(MAX_LIVE_PARTICLES, 0.2);
    let mut emitter = component(5.0);
    emitter.rate = 10.0;
    let id = harness.spawn(emitter);
    for frame in 1..=3 {
        harness.update(0.25, frame as f32 * 0.25);
        assert_eq!(harness.pool.live_count(id), 0);
    }
    // Three blocked 2.5-particle frames leave only a half-particle fraction.
    harness
        .pool
        .step(&harness.registry, 0.3, |_| (0.0, Vec3::ZERO));
    assert!(harness.pool.is_empty());
    harness.update(0.025, 0.775);
    assert_eq!(harness.pool.live_count(id), 0);
    harness.update(0.025, 0.8);
    assert_eq!(
        harness.pool.live_count(id),
        1,
        "only the authored-rate crossing may resume"
    );
}

#[test]
fn intake_and_emitter_spawns_step_once_before_same_frame_collection() {
    let mut harness = FrameHarness::new(5.0);
    let mut emitter = component(5.0);
    emitter.burst = Some(1);
    let id = harness.spawn(emitter);
    harness
        .registry
        .set_component(
            id,
            Transform {
                position: Vec3::new(20.0, 0.0, 0.0),
                ..Transform::default()
            },
        )
        .unwrap();
    crate::weapon::spawn_impact_effect_at(
        &mut harness.registry,
        Vec3::new(10.0, 0.0, 0.0),
        Vec3::Y,
    );
    harness.drain();
    harness.update(0.05, 0.05);
    let before: Vec<_> = harness.pool.iter().collect();
    assert_eq!(before.len(), IMPACT_PARTICLE_COUNT + 1);
    assert!(before.iter().all(|particle| particle.age.abs() < 1e-6));
    harness.step_and_collect(0.05);
    for (old, current) in before.iter().zip(harness.pool.iter()) {
        assert!((current.age - 0.05).abs() < 1e-6);
        assert!((current.position - old.position - old.velocity * 0.05).length() < 1e-5);
    }
    assert_eq!(
        harness.bytes_len(),
        (IMPACT_PARTICLE_COUNT + 1) * SPRITE_INSTANCE_SIZE
    );
    for (_, bytes) in harness.collector.iter_collections() {
        for instance in bytes.as_chunks::<SPRITE_INSTANCE_SIZE>().0 {
            let age = f32::from_ne_bytes(instance[12..16].try_into().unwrap());
            assert!((age - 0.05).abs() < 1e-6);
        }
    }
    assert_eq!(
        harness
            .registry
            .iter_with_kind(ComponentKind::Transform)
            .count(),
        1
    );
}

#[test]
fn same_frame_emitter_rewrite_changes_only_later_spawn_snapshot() {
    let mut harness = FrameHarness::new(5.0);
    let mut emitter = component(5.0);
    emitter.burst = Some(1);
    let id = harness.spawn(emitter);
    harness.update(0.01, 0.01);
    let dust_collection = harness.collector.register_sprite(&derive_collection_id(
        "dust",
        Some(10.0),
        None,
        0.0,
        None,
        None,
    ));
    rewrite(&mut harness, id, |emitter| {
        emitter.lifetime = 10.0;
        emitter.sprite = "dust".into();
        emitter.size_over_lifetime = [2.0].into();
        emitter.opacity_over_lifetime = [0.4].into();
        emitter.buoyancy = 2.0;
        emitter.drag = 3.0;
        emitter.color = [0.25; 3];
        emitter.burst = Some(1);
    });
    harness.update(0.01, 0.01);
    harness
        .pool
        .step(&harness.registry, 0.1, |_| (10.0, Vec3::ZERO));
    let particles: Vec<_> = harness.pool.iter().collect();
    assert_eq!(particles.len(), 2);
    let old = particles
        .iter()
        .find(|particle| (particle.lifetime - 5.0).abs() < 1e-6)
        .unwrap();
    let new = particles
        .iter()
        .find(|particle| (particle.lifetime - 10.0).abs() < 1e-6)
        .unwrap();
    assert!((old.size - 1.0).abs() < 1e-6 && (old.opacity - 1.0).abs() < 1e-6);
    assert!((old.velocity.y - 2.0).abs() < 1e-6);
    assert!(old.tint.iter().all(|value| (*value - 1.0).abs() < 1e-6));
    assert!((new.size - 2.0).abs() < 1e-6 && (new.opacity - 0.4).abs() < 1e-6);
    assert!(new.velocity.length() < 1e-6);
    assert!(new.tint.iter().all(|value| (*value - 0.25).abs() < 1e-6));
    assert_eq!(new.collection, dust_collection);
    assert_ne!(old.collection, new.collection);
}

/// Both frames use the real producer → sim → collector path. The first spawns
/// and draws the requested population; the second expires it. Three emitters
/// remain fixed at both scales so per-emitter work is allowed to allocate.
fn allocation_cycle(
    harness: &mut FrameHarness,
    ids: &[EntityId; 3],
    live: usize,
    rewrite_each_frame: bool,
    environment: postretro_collision::EnvironmentView<'_>,
) -> usize {
    let probe = AllocSnapshot::arm();
    for frame in 0..2 {
        for (index, &id) in ids.iter().enumerate() {
            rewrite(harness, id, |emitter| {
                emitter.burst = if frame == 0 {
                    Some((live / 3 + usize::from(index < live % 3)) as u32)
                } else {
                    None
                };
                if rewrite_each_frame {
                    emitter.size_over_lifetime = [1.0 + frame as f32].into();
                    emitter.opacity_over_lifetime = [0.5 + frame as f32 * 0.1].into();
                    emitter.drag = frame as f32;
                }
            });
        }
        harness.drain();
        harness.update(0.01, frame as f32 * 0.01);
        harness.step_and_collect_in(0.01, environment);
        std::hint::black_box(harness.bytes_len());
        if frame == 0 {
            assert_eq!(harness.pool.len(), live);
            assert_eq!(harness.bytes_len(), live * SPRITE_INSTANCE_SIZE);
        } else {
            assert!(harness.pool.is_empty());
            assert_eq!(harness.bytes_len(), 0);
        }
    }
    probe.allocs_since()
}

#[test]
fn full_particle_frame_allocations_are_independent_of_live_count_with_rewrites() {
    let control = AllocSnapshot::arm();
    std::hint::black_box(Vec::<u8>::with_capacity(32));
    assert!(
        control.allocs_since() > 0,
        "the binary allocation probe must be active"
    );
    let mut harness = FrameHarness::new(0.02);
    let volumes = postretro_collision::EnvironmentVolumes::build(vec![
        allocation_test_volume(
            postretro_collision::ContentsMask::GRAVITY | postretro_collision::ContentsMask::PUSH,
            None,
        ),
        allocation_test_volume(postretro_collision::ContentsMask::FLUID, Some(1)),
        allocation_test_volume(
            postretro_collision::ContentsMask::GRAVITY
                | postretro_collision::ContentsMask::PUSH
                | postretro_collision::ContentsMask::FLUID,
            Some(1),
        ),
    ]);
    let timer = postretro_collision::EnvironmentQueryTimer::new();
    let untimed_environment = postretro_collision::EnvironmentView::new(0.0, &volumes, &[]);
    let timed_environment = untimed_environment.timed(&timer);
    let ids = [
        harness.spawn(component(0.02)),
        harness.spawn(component(0.02)),
        harness.spawn(component(0.02)),
    ];
    for environment in [untimed_environment, timed_environment] {
        for rewriting in [false, true] {
            allocation_cycle(&mut harness, &ids, 10_000, rewriting, environment);
            allocation_cycle(&mut harness, &ids, 10_000, rewriting, environment);
            let small = allocation_cycle(&mut harness, &ids, 100, rewriting, environment);
            let large = allocation_cycle(&mut harness, &ids, 10_000, rewriting, environment);
            assert_eq!(
                small,
                large,
                "full frame allocations must depend on emitter count, not particles; timed={} rewriting={rewriting}",
                environment.timer.is_some(),
            );
        }
    }
}

fn allocation_test_volume(
    contents: postretro_collision::ContentsMask,
    fluid: Option<u16>,
) -> postretro_collision::EnvironmentVolume {
    use glam::Vec3;
    use postretro_collision::{ConvexLeaf, LeafPlane};

    let min = Vec3::splat(-100.0);
    let max = Vec3::splat(100.0);
    let plane = |normal: Vec3, distance: f32| LeafPlane { normal, distance };
    let leaf = ConvexLeaf {
        planes: vec![
            plane(Vec3::X, max.x),
            plane(Vec3::NEG_X, -min.x),
            plane(Vec3::Y, max.y),
            plane(Vec3::NEG_Y, -min.y),
            plane(Vec3::Z, max.z),
            plane(Vec3::NEG_Z, -min.z),
        ],
        aabb_min: min,
        aabb_max: max,
    };
    let has_gravity = contents.contains(postretro_collision::ContentsMask::GRAVITY);
    let has_push = contents.contains(postretro_collision::ContentsMask::PUSH);
    postretro_collision::EnvironmentVolume {
        source_entity: 0,
        priority: 0,
        gravity: has_gravity.then_some(0.0),
        push: has_push.then_some(Vec3::ZERO),
        force: 1.0,
        fluid,
        brushes: vec![leaf],
    }
}
