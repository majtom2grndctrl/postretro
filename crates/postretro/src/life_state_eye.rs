//! Render-only local eye-height easing across authoritative posture corrections.
//! Timing retains gameplay eyes; this scalar replaces only their height term.
use glam::Vec3;
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::{Capability, PlayerMovementComponent, player_capability};

#[derive(Default)]
pub(crate) struct LifeStateEye {
    identity: Option<(EntityId, u8)>,
    previous_raw: f32,
    current_raw: f32,
    presented: f32,
    life: Option<(u8, u8)>,
    correcting: bool,
}

impl LifeStateEye {
    fn observe(&mut self, pawn: EntityId, movement: &PlayerMovementComponent) {
        let identity = (pawn, movement.teleport.generation);
        if self.identity != Some(identity) {
            self.identity = Some(identity);
            self.previous_raw = movement.capsule.eye_height;
            self.current_raw = movement.capsule.eye_height;
            self.presented = movement.capsule.eye_height;
            self.life = Some((movement.life_state, movement.life_state_table_revision));
            self.correcting = false;
        }
        let life = (movement.life_state, movement.life_state_table_revision);
        if self.life != Some(life) {
            self.life = Some(life);
            self.correcting = true;
        }
    }

    pub(crate) fn record_tick(&mut self, registry: &EntityRegistry) {
        let Some((pawn, movement)) = followed(registry) else {
            return;
        };
        self.observe(pawn, movement);
        self.previous_raw = self.current_raw;
        self.current_raw = movement.capsule.eye_height;
    }

    fn height_offset(&mut self, target: f32, rate: f32, alpha: f32, dt: f32) -> f32 {
        self.presented += (target - self.presented) * (1.0 - (-rate * dt.max(0.0)).exp());
        let raw = self.previous_raw + (self.current_raw - self.previous_raw) * alpha;
        self.presented - raw
    }

    pub(crate) fn render(
        &mut self,
        registry: &EntityRegistry,
        mut eye: Vec3,
        alpha: f32,
        dt: f32,
    ) -> Vec3 {
        let Some((pawn, movement)) = followed(registry) else {
            self.identity = None;
            return eye;
        };
        self.observe(pawn, movement);
        let raw = self.previous_raw + (self.current_raw - self.previous_raw) * alpha;
        if !self.correcting {
            self.presented = raw;
            return eye;
        }
        let Some(crouch) = movement.crouch.as_ref() else {
            return eye;
        };
        eye.y += self.height_offset(
            movement.capsule.eye_height,
            crouch.transition_rate,
            alpha,
            dt,
        );
        if (self.presented - movement.capsule.eye_height).abs() < 1.0e-5
            && (raw - movement.capsule.eye_height).abs() < 1.0e-5
        {
            self.correcting = false;
        }
        eye
    }
}

fn followed(registry: &EntityRegistry) -> Option<(EntityId, &PlayerMovementComponent)> {
    let pawn = registry.local_player_movement_pawn()?;
    Some((
        pawn,
        registry
            .get_component::<PlayerMovementComponent>(pawn)
            .ok()?,
    ))
}

fn look_allowed(registry: &EntityRegistry) -> bool {
    followed(registry)
        .and_then(|(_, movement)| player_capability(movement, Capability::Look))
        .unwrap_or(true)
}

pub(crate) fn apply_look(
    registry: &EntityRegistry,
    camera: &mut crate::camera::Camera,
    yaw: f32,
    pitch: f32,
) {
    if look_allowed(registry) {
        camera.rotate(yaw, pitch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movement_mut(
        registry: &mut postretro_entities::EntityRegistry,
        pawn: postretro_entities::EntityId,
    ) -> &mut PlayerMovementComponent {
        let postretro_entities::ComponentValue::PlayerMovement(movement) = registry
            .get_component_value_mut(pawn, postretro_entities::ComponentKind::PlayerMovement)
            .unwrap()
        else {
            panic!("fixture pawn has movement");
        };
        movement.as_mut()
    }

    fn fixture() -> (EntityRegistry, EntityId) {
        let mut descriptor = crate::tests::minimal_player_descriptor();
        descriptor.crouch = Some(postretro_foundation::CrouchParams {
            half_height: 0.4,
            eye_height: 0.2,
            transition_rate: 15.0,
        });
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(postretro_entities::Transform::default());
        registry
            .set_component(pawn, PlayerMovementComponent::from_descriptor(&descriptor))
            .unwrap();
        registry.mark_local_player_pawn(pawn).unwrap();
        (registry, pawn)
    }

    #[test]
    fn life_eye_replayed_height_eases_at_render_rate_and_real_teleport_resets() {
        let (mut registry, pawn) = fixture();
        let mut eye = LifeStateEye::default();
        eye.record_tick(&registry);
        assert_eq!(
            eye.render(&registry, Vec3::Y * 0.5, 1.0, 1.0 / 144.0).y,
            0.5
        );
        {
            let movement = movement_mut(&mut registry, pawn);
            movement.life_state = 1;
            movement.life_state_table_revision = 7;
            movement.capsule.eye_height = 0.2;
        }
        // A reconcile changes gameplay immediately; render retains its own prior height.
        eye.record_tick(&registry);
        let first = eye.render(&registry, Vec3::Y * 0.2, 1.0, 1.0 / 144.0).y;
        assert!(first > 0.45 && first < 0.5);
        let mut last = first;
        for _ in 0..144 {
            let next = eye.render(&registry, Vec3::Y * 0.2, 1.0, 1.0 / 144.0).y;
            assert!(next <= last && next >= 0.19999);
            last = next;
        }
        assert!((last - 0.2).abs() < 1.0e-5);
        assert_eq!(
            registry
                .get_component::<PlayerMovementComponent>(pawn)
                .unwrap()
                .capsule
                .eye_height,
            0.2
        );
        let component = movement_mut(&mut registry, pawn);
        component.teleport.generation += 1;
        component.capsule.eye_height = 0.5;
        assert_eq!(
            eye.render(&registry, Vec3::Y * 0.5, 1.0, 1.0 / 144.0).y,
            0.5
        );
    }

    #[test]
    fn life_look_input_freezes_yaw_pitch_and_undeclared_view_still_turns() {
        let (mut registry, pawn) = fixture();
        let mut camera = crate::camera::Camera::new(Vec3::ZERO, 0.0, 0.0);
        apply_look(&registry, &mut camera, 0.1, 0.2);
        let before = (camera.yaw, camera.pitch);
        let movement = movement_mut(&mut registry, pawn);
        postretro_foundation::enter_life_state(
            movement,
            1,
            &postretro_foundation::LifeStateProfile {
                look: false,
                ..Default::default()
            },
            7,
        );
        apply_look(&registry, &mut camera, 0.3, 0.4);
        assert_eq!((camera.yaw, camera.pitch), before);
        movement_mut(&mut registry, pawn).life_state = postretro_foundation::UNDECLARED_LIFE_STATE;
        apply_look(&registry, &mut camera, 0.3, 0.4);
        assert_ne!((camera.yaw, camera.pitch), before);
    }
    #[test]
    fn life_eye_arrival_frame_steps_fit_normal_key_crouch_and_stand_envelopes() {
        use postretro_foundation::{LifeStatePosture, LifeStateProfile, enter_life_state};
        let world = crate::collision::CollisionWorld::from_triangles_for_test(
            vec![
                Vec3::new(-50.0, 0.0, -50.0),
                Vec3::new(50.0, 0.0, -50.0),
                Vec3::new(50.0, 0.0, 50.0),
                Vec3::new(-50.0, 0.0, 50.0),
            ],
            vec![[0, 2, 1], [0, 3, 2]],
        );
        let (mut registry, pawn) = fixture();
        let mut component = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .clone();
        component.set_grounded(true);
        let mut position = Vec3::new(0.0, 1.21, 0.0);
        let mut input = crate::movement::MovementInput {
            wish_dir: glam::Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            facing_pitch: 0.0,
            use_pressed: false,
            drop_pressed: false,
        };
        for down in [true, false] {
            registry.set_component(pawn, component.clone()).unwrap();
            let mut key_eye = LifeStateEye::default();
            key_eye.record_tick(&registry);
            input.crouch_intent = down;
            let start = position + Vec3::Y * component.capsule.eye_height;
            let mut previous = start;
            let mut max_key_step = 0.0f32;
            for _ in 0..45 {
                let prior = position + Vec3::Y * component.capsule.eye_height;
                (position, _, _) = crate::movement::tick_with_immersion_and_armed(
                    &mut component,
                    &input,
                    &world,
                    postretro_collision::EnvironmentView::level_only(-20.0),
                    1.0 / 60.0,
                    position,
                    |_| true,
                );
                registry.set_component(pawn, component.clone()).unwrap();
                key_eye.record_tick(&registry);
                let current = position + Vec3::Y * component.capsule.eye_height;
                for alpha in [0.25, 0.5, 0.75, 1.0] {
                    let raw = prior.lerp(current, alpha);
                    let rendered = key_eye.render(&registry, raw, alpha, 1.0 / 240.0);
                    assert!(
                        (rendered - raw).length() < 1.0e-5,
                        "normal key crouch is not double-eased"
                    );
                    max_key_step = max_key_step.max((rendered.y - previous.y).abs());
                    previous = rendered;
                }
            }
            // Arrival replays to the settled capsule but body correction already
            // preserves its previously presented centre. Only height is replaced.
            let target = component.capsule.eye_height;
            let mut arrival = LifeStateEye::default();
            let mut before = component.clone();
            // Pin the actual pre-transition eye-height, independent of the body's resize.
            before.capsule.eye_height = if down { 0.5 } else { 0.2 };
            before.life_state = 0;
            registry.set_component(pawn, before.clone()).unwrap();
            arrival.record_tick(&registry);
            let origin = Vec3::Y * before.capsule.eye_height;
            arrival.render(&registry, origin, 1.0, 1.0 / 240.0);
            let mut profile = LifeStateProfile::default();
            profile.movement.posture = if down {
                LifeStatePosture::Crouch
            } else {
                LifeStatePosture::Stand
            };
            enter_life_state(&mut before, 1, &profile, 7);
            before.capsule.eye_height = target;
            registry.set_component(pawn, before).unwrap();
            arrival.record_tick(&registry);
            let mut previous = origin.y;
            for _ in 0..180 {
                let rendered = arrival
                    .render(&registry, Vec3::Y * target, 1.0, 1.0 / 240.0)
                    .y;
                assert!(
                    (rendered - previous).abs() <= max_key_step + 1.0e-4,
                    "arrival step exceeds normal key envelope: down={down}, step={}, key={max_key_step}",
                    (rendered - previous).abs()
                );
                previous = rendered;
            }
        }
    }
}
