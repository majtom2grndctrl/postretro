//! O14: payload ordering matters when snapshots replay a real predicted tail.
use super::*;
use crate::predict_reconcile_harness_test_fixtures::{
    DT, GRAVITY, NO_CORRECTION_M, START, floor_world, forward_command, player_descriptor,
};
use crate::prediction::{CorrectionClass, TELEPORT_CORRECTION_MIN_M, classify_correction};
use postretro_foundation::{
    LifeStateProfile, PlayerMovementComponent, enter_life_state, restamp_life_state,
};

const LEAD: u32 = 8;

struct Backlog {
    endpoint: NetEndpoint,
    client: EntityRegistry,
    pawn: EntityId,
    host: EntityRegistry,
    host_pawn: EntityId,
    world: crate::collision::CollisionWorld,
    next_client: u32,
    next_host: u32,
}

impl Backlog {
    fn new(speed: f32) -> (Self, TuningPayload) {
        let mut movement = player_descriptor();
        movement.ground.speed.run = speed;
        let mut profile = LifeStateProfile::default();
        profile.movement.speed_scale = 0.3;
        let mut payload = TuningPayload::new(
            Some(movement.clone()),
            std::array::from_fn(|_| None),
            EnvironmentTuning {
                level_gravity: GRAVITY,
                fluids: Default::default(),
            },
        );
        payload.life_state_names = vec!["alive".into(), "down".into()];
        payload.life_state_profiles = vec![(1, profile.clone())];
        payload.life_state_table_revision = 9;
        let mut endpoint = client_endpoint();
        let mut client = EntityRegistry::new();
        install(&mut endpoint, &mut client, &bytes(&payload));
        let pawn = arm(&mut endpoint, &mut client);
        let NetEndpoint::Client { prediction, .. } = &mut endpoint else {
            unreachable!()
        };
        prediction.arm(PAWN, pawn);
        prediction.mark_first_record_adopted();
        let mut component = PlayerMovementComponent::from_descriptor(&movement);
        enter_life_state(&mut component, 1, &profile, 9);
        component.life_state_elapsed_ticks = 600;
        let pose = Transform {
            position: START,
            ..Default::default()
        };
        client.set_component(pawn, pose).unwrap();
        client.set_component(pawn, component.clone()).unwrap();
        let mut host = EntityRegistry::new();
        let host_pawn = host.spawn(pose);
        host.set_component(host_pawn, component).unwrap();
        (
            Self {
                endpoint,
                client,
                pawn,
                host,
                host_pawn,
                world: floor_world(),
                next_client: 0,
                next_host: 0,
            },
            payload,
        )
    }

    fn predict(&mut self) {
        let command = wire_convert::sim_command_to_input(&forward_command(false), self.next_client);
        self.next_client += 1;
        let current = (
            *self.client.get_component::<Transform>(self.pawn).unwrap(),
            self.client
                .get_component::<PlayerMovementComponent>(self.pawn)
                .unwrap()
                .clone(),
        );
        let NetEndpoint::Client { prediction, .. } = &mut self.endpoint else {
            unreachable!()
        };
        let next = prediction
            .predict_tick(
                command,
                current,
                &self.world,
                postretro_collision::EnvironmentView::level_only(GRAVITY),
                DT,
            )
            .unwrap();
        prediction.decay_presentation_offset();
        self.client.set_component(self.pawn, next.0).unwrap();
        self.client.set_component(self.pawn, next.1).unwrap();
    }

    fn tick(&mut self) {
        self.predict();
        self.host.snapshot_transforms();
        let _ = crate::sim::run_host_movement_tick(
            &mut self.host,
            &self.world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            &[(self.host_pawn, forward_command(false).movement)],
            DT,
        );
        self.next_host += 1;
    }

    fn merge(&mut self) -> (f32, CorrectionClass) {
        let before = self
            .client
            .get_component::<Transform>(self.pawn)
            .unwrap()
            .position;
        let movement = movement_state::movement_state_to_wire(
            self.host
                .get_component::<PlayerMovementComponent>(self.host_pawn)
                .unwrap(),
            0.0,
        );
        let pose = *self
            .host
            .get_component::<Transform>(self.host_pawn)
            .unwrap();
        let NetEndpoint::Client {
            prediction,
            life_state_tables,
            ..
        } = &mut self.endpoint
        else {
            unreachable!()
        };
        assert!(
            !prediction.history().is_empty(),
            "a snapshot must replay unacknowledged commands"
        );
        let class = reconcile::reconcile_local_pawn_with_mover_history(
            &mut self.client,
            prediction,
            self.pawn,
            pose,
            Some(&movement),
            Some(self.next_host - 1),
            self.next_host + 1,
            None,
            &self.world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            DT,
            life_state_tables,
        )
        .unwrap();
        assert_eq!(
            prediction.history().len(),
            LEAD as usize,
            "every merge retains and replays the fixed command lead"
        );
        let after = self
            .client
            .get_component::<Transform>(self.pawn)
            .unwrap()
            .position;
        (before.distance(after), class)
    }

    fn install(&mut self, payload: &TuningPayload) {
        let before = self
            .client
            .get_component::<PlayerMovementComponent>(self.pawn)
            .unwrap()
            .life_state_elapsed_ticks;
        install(&mut self.endpoint, &mut self.client, &bytes(payload));
        assert_eq!(
            self.client
                .get_component::<PlayerMovementComponent>(self.pawn)
                .unwrap()
                .life_state_elapsed_ticks,
            before,
            "a tuning rebuild preserves elapsed before replay"
        );
    }

    fn edit_host_profile(&mut self, payload: &TuningPayload) {
        let (id, profile) = &payload.life_state_profiles[0];
        let mut movement = self
            .host
            .get_component::<PlayerMovementComponent>(self.host_pawn)
            .unwrap()
            .clone();
        let elapsed = movement.life_state_elapsed_ticks;
        movement.life_state = *id;
        movement.life_state_table_revision = payload.life_state_table_revision;
        restamp_life_state(&mut movement, Some(profile));
        assert_eq!(movement.life_state_elapsed_ticks, elapsed);
        self.host.set_component(self.host_pawn, movement).unwrap();
    }
}

#[test]
fn life_state_o14_profile_reload_with_backlog_corrects_once_after_payload_and_each_old_profile_merge()
 {
    for (speed, snapshots_first, bump_revision) in [
        (11.0, false, false),
        (11.0, false, true),
        (11.0, true, false),
        (11.0, true, true),
        (800.0, true, true),
    ] {
        let (mut h, initial) = Backlog::new(speed);
        for _ in 0..LEAD {
            h.predict();
        }
        let mut baseline = 0.0_f32;
        for _ in 0..20 {
            h.tick();
            h.tick();
            let (magnitude, class) = h.merge();
            baseline = baseline.max(magnitude);
            assert_eq!(class, CorrectionClass::Ordinary);
        }
        assert!(baseline <= NO_CORRECTION_M);
        let bound = baseline + NO_CORRECTION_M;
        let mut changed = initial.clone();
        changed.life_state_profiles[0].1.movement.speed_scale = 0.6;
        if bump_revision {
            // Another archetype adds a sorted name during this profile edit.
            changed.life_state_names.insert(0, "aaa".into());
            changed.life_state_profiles[0].0 = 2;
            changed.life_state_table_revision += 1;
        }
        assert_eq!(
            changed.movement, initial.movement,
            "only life-state data changed"
        );
        h.edit_host_profile(&changed);
        if snapshots_first {
            for _ in 0..3 {
                h.tick();
                h.tick();
                let (magnitude, class) = h.merge();
                assert!(
                    magnitude > bound,
                    "each old-profile replay needs a correction: {speed}/{bump_revision}: {magnitude}"
                );
                let expected = if bump_revision {
                    // Unknown revision must smooth at any magnitude.
                    if magnitude <= crate::prediction::ORDINARY_CORRECTION_MAX_M {
                        CorrectionClass::Ordinary
                    } else {
                        CorrectionClass::OversizedSmoothed
                    }
                } else {
                    classify_correction(magnitude, false)
                };
                assert_eq!(
                    class, expected,
                    "held versus bumped revision classification"
                );
                if speed == 800.0 {
                    assert!(
                        magnitude >= TELEPORT_CORRECTION_MIN_M,
                        "exercise the classification split above the teleport floor"
                    );
                }
                let movement = h
                    .client
                    .get_component::<PlayerMovementComponent>(h.pawn)
                    .unwrap();
                assert_eq!(movement.life_speed_scale, 0.3);
            }
        }
        h.install(&changed);
        let mut corrections = Vec::new();
        for _ in 0..6 {
            h.tick();
            h.tick();
            corrections.push(h.merge());
            let movement = h
                .client
                .get_component::<PlayerMovementComponent>(h.pawn)
                .unwrap();
            assert_eq!(movement.life_state_elapsed_ticks, 600 + h.next_host + LEAD);
            assert_eq!(movement.life_speed_scale, 0.6);
            assert_eq!(movement.life_state, changed.life_state_profiles[0].0);
        }
        assert!(
            corrections[0].0 > bound,
            "the first matching-profile replay repairs the outstanding divergence"
        );
        if bump_revision {
            assert_ne!(
                corrections[0].1,
                CorrectionClass::Teleport,
                "adopting the new revision forces a glide even above the floor"
            );
        } else {
            assert_eq!(
                corrections[0].1,
                classify_correction(corrections[0].0, false),
                "a same-revision profile edit retains magnitude classification"
            );
        }
        assert!(
            corrections
                .iter()
                .skip(1)
                .all(|&(magnitude, _)| magnitude <= bound),
            "no repeated correction once both peers use the profile: {corrections:?}"
        );
        assert_eq!(
            corrections
                .iter()
                .filter(|&&(magnitude, _)| magnitude > bound)
                .count(),
            1
        );
    }
}
