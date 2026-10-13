use super::*;

fn request(index: usize) -> ImpactParticleRequest {
    ImpactParticleRequest {
        point: Vec3::new(index as f32, 2.0, 3.0),
        normal: Vec3::Y,
    }
}

fn overflow(registry: &mut EntityRegistry) {
    for index in 0..=MAX_PENDING_IMPACT_PARTICLE_REQUESTS {
        registry.push_impact_particle_request(request(index));
    }
}

#[test]
fn impact_intake_is_bounded_ordered_and_reuses_capacity() {
    let mut registry = EntityRegistry::new();
    let registry_capacity = registry.impact_particle_requests.capacity();
    let mut drained = Vec::with_capacity(MAX_PENDING_IMPACT_PARTICLE_REQUESTS);
    let drain_capacity = drained.capacity();
    overflow(&mut registry);
    assert_eq!(registry.dropped_impact_particle_requests, 1);
    registry.drain_impact_particle_requests_into(&mut drained);
    assert_eq!(drained.len(), MAX_PENDING_IMPACT_PARTICLE_REQUESTS);
    for (index, actual) in drained.iter().enumerate() {
        assert_eq!(*actual, request(index));
    }
    registry.push_impact_particle_request(request(999));
    registry.drain_impact_particle_requests_into(&mut drained);
    assert_eq!(drained, [request(999)]);
    registry.drain_impact_particle_requests_into(&mut drained);
    assert!(drained.is_empty());
    assert_eq!(
        registry.impact_particle_requests.capacity(),
        registry_capacity
    );
    assert_eq!(drained.capacity(), drain_capacity);
    assert!(registry.slots.is_empty(), "intake consumes no entity slots");
}

#[test]
fn impact_overflow_warning_throttles_and_resets_on_level_clear() {
    let logs = postretro_test_log_capture::LogCapture::start();
    let warnings = || {
        logs.records()
            .into_iter()
            .filter(|record| {
                record.level == log::Level::Warn
                    && record
                        .message
                        .contains("impact particle request queue full; dropped 1 bursts")
            })
            .count()
    };
    let mut registry = EntityRegistry::new();
    let mut drained = Vec::new();
    let start = Instant::now();
    overflow(&mut registry);
    registry.drain_impact_particle_requests_at(&mut drained, start);
    assert_eq!(registry.last_impact_particle_warning, Some(start));
    assert_eq!(warnings(), 1);
    assert_eq!(registry.dropped_impact_particle_requests, 0);
    overflow(&mut registry);
    registry.drain_impact_particle_requests_at(&mut drained, start + Duration::from_millis(999));
    assert_eq!(registry.last_impact_particle_warning, Some(start));
    assert_eq!(registry.dropped_impact_particle_requests, 1);
    assert_eq!(warnings(), 1);
    registry.drain_impact_particle_requests_at(&mut drained, start + Duration::from_secs(1));
    assert_eq!(
        registry.last_impact_particle_warning,
        Some(start + Duration::from_secs(1))
    );
    assert_eq!(warnings(), 2);
    assert_eq!(registry.dropped_impact_particle_requests, 0);
    registry.clear_for_level_unload();
    overflow(&mut registry);
    let early_next_level = start + Duration::from_millis(1001);
    registry.drain_impact_particle_requests_at(&mut drained, early_next_level);
    assert_eq!(
        registry.last_impact_particle_warning,
        Some(early_next_level)
    );
    assert_eq!(warnings(), 3);
    assert_eq!(registry.dropped_impact_particle_requests, 0);
}

#[test]
fn level_clear_discards_impact_intake_and_pending_overflow() {
    let mut registry = EntityRegistry::new();
    overflow(&mut registry);
    let capacity = registry.impact_particle_requests.capacity();
    registry.clear_for_level_unload();
    let mut drained = vec![request(999)];
    registry.drain_impact_particle_requests_into(&mut drained);
    assert!(drained.is_empty());
    assert_eq!(registry.dropped_impact_particle_requests, 0);
    assert_eq!(registry.last_impact_particle_warning, None);
    assert_eq!(registry.impact_particle_requests.capacity(), capacity);
}

#[test]
fn presentation_transport_visit_never_observes_impact_intake() {
    let mut registry = EntityRegistry::new();
    registry.push_impact_particle_request(request(1));
    registry.visit_unrouted_presentation_spawns(|_| panic!("local impact reached transport"));
    assert!(registry.take_presentation_spawns().is_empty());
    let mut drained = Vec::new();
    registry.drain_impact_particle_requests_into(&mut drained);
    assert_eq!(drained, [request(1)]);
}
