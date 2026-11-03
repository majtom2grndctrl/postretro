//! Remote direct-hit validation uses the same spatial life capability as local fire.

use super::*;
use postretro_foundation::{LifeStateProfile, enter_life_state};

#[test]
fn life_state_remote_projectile_target_capability_overrides_hp_but_hitscan_keeps_impact_policy() {
    for projectile in [false, true] {
        for hp in [0.0, 100.0] {
            for damageable in [false, true] {
                let mut fixture = HitIngestFixture::new(CollisionWorld::new());
                let mut health = fixture.target_health();
                health.current = hp;
                fixture
                    .registry
                    .set_component(fixture.target, health)
                    .unwrap();
                let mut movement = movement_component_with_eye_height(0.5);
                enter_life_state(
                    &mut movement,
                    0,
                    &LifeStateProfile {
                        damageable,
                        ..Default::default()
                    },
                    0,
                );
                fixture
                    .registry
                    .set_component(fixture.target, movement)
                    .unwrap();
                fixture.shot_mut().is_projectile = projectile;
                fixture.shot_mut().fire_origin = Vec3::new(0.0, 0.5, 0.0);
                let declaration =
                    fixture.declaration(vec![fixture.record(Vec3::new(4.0, 0.5, 0.0), None)]);
                let result = fixture.ingest_result(7, &declaration);
                assert!(result.fire_accepted);
                assert_eq!(
                    result.hit_accepted,
                    !projectile || damageable,
                    "projectile={projectile}, hp={hp}, damageable={damageable}"
                );
                let impacts = fixture.registry.take_impact_dispatches();
                assert_eq!(!impacts.is_empty(), !projectile || damageable);
            }
        }
    }
}
