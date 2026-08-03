//! Declarative mover command application and scripting registrations.
//! See: context/lib/scripting.md §10.6

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use glam::Vec3;
use postretro_entities::{EntityId, EntityRegistry, KinematicMoverComponent, MoverCommand};
use postretro_scripting_core::reaction_registry::{ReactionError, ReactionPrimitiveRegistry};
use postretro_scripting_core::sequence::{SequenceError, SequencedPrimitiveRegistry};
use serde::Deserialize;

use super::{mover_is_at_waypoint, path_coordinate, reanchor_direction};

/// Per-level warning deduplication shared by mover command routes whose
/// registries survive level reloads.
#[derive(Debug, Clone, Default)]
pub(crate) struct MoverCommandDiagnostics {
    warned_non_mover_targets: Rc<RefCell<HashSet<EntityId>>>,
    warned_non_trigger_targets: Rc<RefCell<HashSet<EntityId>>>,
}

impl MoverCommandDiagnostics {
    pub(crate) fn clear(&self) {
        self.warned_non_mover_targets.borrow_mut().clear();
        self.warned_non_trigger_targets.borrow_mut().clear();
    }

    fn warn_non_mover_target_once(&self, entity: EntityId) {
        if self.warned_non_mover_targets.borrow_mut().insert(entity) {
            log::warn!("[Mover] command target {entity} has no KinematicMoverComponent; skipping");
        }
    }

    pub(crate) fn warn_non_trigger_target_once(&self, entity: EntityId) {
        if self.warned_non_trigger_targets.borrow_mut().insert(entity) {
            log::warn!(
                "[Trigger] arm/disarm target {entity} has no TriggerVolumeComponent; skipping"
            );
        }
    }
}

/// Apply a declarative command by mutating only deterministic mover phase.
///
/// This deliberately has no registry, clock, RNG, or host-role dependency so
/// the same command produces the same next phase for every simulation peer.
pub(crate) fn apply_mover_command(mover: &mut KinematicMoverComponent, command: &MoverCommand) {
    match command {
        MoverCommand::Start => {
            mover.blocked = false;
            if mover.completed || (mover.started && mover.wait_remaining_ms <= 0.0) {
                return;
            }
            mover.started = true;
            mover.wait_remaining_ms = 0.0;
        }
        MoverCommand::Stop => {
            if !mover.started {
                return;
            }
            mover.started = false;
        }
        MoverCommand::Reverse => {
            mover.blocked = false;
            if mover.waypoints.len() < 2 {
                return;
            }
            reanchor_direction(mover, if mover.direction_sign >= 0 { -1 } else { 1 });
            mover.started = true;
            mover.completed = false;
            mover.wait_remaining_ms = 0.0;
        }
        MoverCommand::GoToPathNode(name) => {
            let mut matches = mover
                .waypoint_names
                .iter()
                .enumerate()
                .filter_map(|(index, waypoint_name)| (waypoint_name == name).then_some(index));
            let Some(target) = matches.next() else {
                log::warn!(
                    "[Mover] go_to_path_node for mover {} references unknown waypoint `{name}`; skipping",
                    mover.mover_id
                );
                return;
            };
            if matches.next().is_some() || target > usize::from(u16::MAX) {
                log::warn!(
                    "[Mover] go_to_path_node for mover {} cannot uniquely resolve waypoint `{name}`; skipping",
                    mover.mover_id
                );
                return;
            }
            let target = target as u16;
            mover.blocked = false;
            if mover_is_at_waypoint(mover, target) {
                return;
            }

            let direction = if path_coordinate(mover)
                .map(|coordinate| f32::from(target) > coordinate)
                .unwrap_or(target > mover.segment_index)
            {
                1
            } else {
                -1
            };
            reanchor_direction(mover, direction);
            mover.target_segment = Some(target);
            mover.started = true;
            mover.completed = false;
            mover.wait_remaining_ms = 0.0;
        }
        MoverCommand::SetSpinRate(rate_deg_s) => {
            if !rate_deg_s.is_finite() {
                log::warn!(
                    "[Mover] set_spin_rate for mover {} has non-finite rate; skipping",
                    mover.mover_id
                );
                return;
            }
            if *rate_deg_s != 0.0
                && (!mover.spin_axis.is_finite()
                    || mover.spin_axis.normalize_or_zero() == Vec3::ZERO)
            {
                log::warn!(
                    "[Mover] set_spin_rate for mover {} requires a non-zero spin axis; skipping",
                    mover.mover_id
                );
                return;
            }
            mover.spin_target_rate_rad_s = rate_deg_s.to_radians();
        }
    }
}

/// Apply one command to an already-resolved tag target set. Non-movers remain
/// untouched so a mixed tag cannot accidentally gain a mover component.
pub(crate) fn apply_mover_command_to_targets(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    command: &MoverCommand,
    diagnostics: &MoverCommandDiagnostics,
) {
    apply_mover_command_to_targets_inner(registry, targets, command, Some(diagnostics));
}

/// Apply a command to targets produced by a `KinematicMover` component query.
/// Unlike authored mixed-tag routes, this path cannot encounter a non-mover.
pub(crate) fn apply_mover_command_to_known_movers(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    command: &MoverCommand,
) {
    apply_mover_command_to_targets_inner(registry, targets, command, None);
}

fn apply_mover_command_to_targets_inner(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    command: &MoverCommand,
    diagnostics: Option<&MoverCommandDiagnostics>,
) {
    for &entity in targets {
        let Ok(mut mover) = registry
            .get_component::<KinematicMoverComponent>(entity)
            .cloned()
        else {
            if let Some(diagnostics) = diagnostics {
                diagnostics.warn_non_mover_target_once(entity);
            }
            continue;
        };
        apply_mover_command(&mut mover, command);
        let _ = registry.set_component(entity, mover);
    }
}

/// Register the closed mover command vocabulary for named, tag-targeted
/// reactions. Each route intentionally converges on the shared command applier
/// used by KVP trigger dispatch.
pub(crate) fn register_mover_reaction_primitives(
    registry: &mut ReactionPrimitiveRegistry,
    diagnostics: MoverCommandDiagnostics,
) {
    let start_diagnostics = diagnostics.clone();
    registry.register("moverStart", move |registry, targets, _args| {
        apply_mover_command_to_targets(registry, targets, &MoverCommand::Start, &start_diagnostics);
        Ok(())
    });
    let stop_diagnostics = diagnostics.clone();
    registry.register("moverStop", move |registry, targets, _args| {
        apply_mover_command_to_targets(registry, targets, &MoverCommand::Stop, &stop_diagnostics);
        Ok(())
    });
    let reverse_diagnostics = diagnostics.clone();
    registry.register("moverReverse", move |registry, targets, _args| {
        apply_mover_command_to_targets(
            registry,
            targets,
            &MoverCommand::Reverse,
            &reverse_diagnostics,
        );
        Ok(())
    });
    let go_to_path_node_diagnostics = diagnostics.clone();
    registry.register("moverGoToPathNode", move |registry, targets, args| {
        let args: MoverGoToPathNodeArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("moverGoToPathNode: failed to deserialize args: {e}"),
            })?;
        apply_mover_command_to_targets(
            registry,
            targets,
            &MoverCommand::GoToPathNode(args.node),
            &go_to_path_node_diagnostics,
        );
        Ok(())
    });
    let set_spin_rate_diagnostics = diagnostics.clone();
    registry.register("moverSetSpinRate", move |registry, targets, args| {
        let args: MoverSetSpinRateArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("moverSetSpinRate: failed to deserialize args: {e}"),
            })?;
        apply_mover_command_to_targets(
            registry,
            targets,
            &MoverCommand::SetSpinRate(args.rate),
            &set_spin_rate_diagnostics,
        );
        Ok(())
    });
}

/// Register the same command vocabulary on the per-entity sequence path.
/// SDK mover handles return sequence-step arrays, while direct primitive
/// reactions use the tag-targeted registry above.
pub(crate) fn register_sequenced_mover_primitives(
    registry: &mut SequencedPrimitiveRegistry,
    ctx: postretro_entities::ScriptCtx,
    diagnostics: MoverCommandDiagnostics,
) {
    register_sequenced_mover_command(
        registry,
        ctx.clone(),
        diagnostics.clone(),
        "moverStart",
        MoverCommand::Start,
    );
    register_sequenced_mover_command(
        registry,
        ctx.clone(),
        diagnostics.clone(),
        "moverStop",
        MoverCommand::Stop,
    );
    register_sequenced_mover_command(
        registry,
        ctx.clone(),
        diagnostics.clone(),
        "moverReverse",
        MoverCommand::Reverse,
    );
    let go_to_path_node_ctx = ctx.clone();
    let go_to_path_node_diagnostics = diagnostics.clone();
    registry.register("moverGoToPathNode", move |id, args| {
        let args: MoverGoToPathNodeArgs =
            serde_json::from_value(args.clone()).map_err(|e| SequenceError::InvalidArgument {
                reason: format!("moverGoToPathNode: failed to deserialize args: {e}"),
            })?;
        let mut entities = go_to_path_node_ctx.registry.borrow_mut();
        apply_mover_command_to_targets(
            &mut entities,
            &[id],
            &MoverCommand::GoToPathNode(args.node),
            &go_to_path_node_diagnostics,
        );
        Ok(())
    });
    registry.register("moverSetSpinRate", move |id, args| {
        let args: MoverSetSpinRateArgs =
            serde_json::from_value(args.clone()).map_err(|e| SequenceError::InvalidArgument {
                reason: format!("moverSetSpinRate: failed to deserialize args: {e}"),
            })?;
        let mut entities = ctx.registry.borrow_mut();
        apply_mover_command_to_targets(
            &mut entities,
            &[id],
            &MoverCommand::SetSpinRate(args.rate),
            &diagnostics,
        );
        Ok(())
    });
}

fn register_sequenced_mover_command(
    registry: &mut SequencedPrimitiveRegistry,
    ctx: postretro_entities::ScriptCtx,
    diagnostics: MoverCommandDiagnostics,
    name: &'static str,
    command: MoverCommand,
) {
    registry.register(name, move |id, _args| {
        let mut entities = ctx.registry.borrow_mut();
        apply_mover_command_to_targets(&mut entities, &[id], &command, &diagnostics);
        Ok(())
    });
}

#[derive(Debug, Deserialize)]
struct MoverGoToPathNodeArgs {
    node: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MoverSetSpinRateArgs {
    pub(crate) rate: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Quat, Vec3};
    use postretro_entities::{KinematicMoverMode, ScriptCtx};
    use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

    fn sample_mover(mode: KinematicMoverMode, wait_ms: f32) -> KinematicMoverComponent {
        KinematicMoverComponent::new(
            7,
            postretro_entities::KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0)],
                waypoint_names: vec!["start".to_string(), "finish".to_string()],
                speed_mps: 1.0,
                wait_ms,
                mode,
                started: true,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        )
    }

    fn spin_capable_mover(mode: KinematicMoverMode, wait_ms: f32) -> KinematicMoverComponent {
        let mut mover = sample_mover(mode, wait_ms);
        mover.spin_axis = Vec3::Y;
        mover
    }

    fn transform_at(position: Vec3) -> postretro_entities::Transform {
        postretro_entities::Transform {
            position,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }

    #[test]
    fn command_diagnostics_deduplicate_within_a_level_and_reset_on_clear() {
        let diagnostics = MoverCommandDiagnostics::default();
        let shared_route = diagnostics.clone();
        let entity = EntityId::from_raw(41);

        diagnostics.warn_non_mover_target_once(entity);
        shared_route.warn_non_mover_target_once(entity);
        diagnostics.warn_non_trigger_target_once(entity);
        shared_route.warn_non_trigger_target_once(entity);

        assert_eq!(diagnostics.warned_non_mover_targets.borrow().len(), 1);
        assert_eq!(diagnostics.warned_non_trigger_targets.borrow().len(), 1);

        diagnostics.clear();
        assert!(diagnostics.warned_non_mover_targets.borrow().is_empty());
        assert!(diagnostics.warned_non_trigger_targets.borrow().is_empty());

        shared_route.warn_non_mover_target_once(entity);
        shared_route.warn_non_trigger_target_once(entity);
        assert_eq!(diagnostics.warned_non_mover_targets.borrow().len(), 1);
        assert_eq!(diagnostics.warned_non_trigger_targets.borrow().len(), 1);
    }

    #[test]
    fn mover_commands_mutate_phase_and_preserve_their_idempotent_edges() {
        let mut mover = sample_mover(KinematicMoverMode::Once, 250.0);
        mover.segment_elapsed_ms = 750.0;
        mover.wait_remaining_ms = 100.0;
        mover.current_linear_velocity = Vec3::X;

        apply_mover_command(&mut mover, &MoverCommand::Stop);
        assert!(!mover.started);
        assert_eq!(mover.segment_elapsed_ms, 750.0);
        assert_eq!(
            mover.current_linear_velocity,
            Vec3::X,
            "post-command phase retains the motion of the tick that just ran"
        );
        let stopped = mover.clone();
        apply_mover_command(&mut mover, &MoverCommand::Stop);
        assert_eq!(mover, stopped);

        apply_mover_command(&mut mover, &MoverCommand::Start);
        assert!(mover.started);
        assert_eq!(mover.wait_remaining_ms, 0.0);
        assert_eq!(mover.segment_elapsed_ms, 750.0);
        let started = mover.clone();
        apply_mover_command(&mut mover, &MoverCommand::Start);
        assert_eq!(mover, started);

        mover.completed = true;
        let completed = mover.clone();
        apply_mover_command(&mut mover, &MoverCommand::Start);
        assert_eq!(mover, completed);
    }

    #[test]
    fn restart_commands_clear_a_reconciled_stop_hold() {
        let mut start = sample_mover(KinematicMoverMode::PingPong, 0.0);
        start.blocked = true;
        apply_mover_command(&mut start, &MoverCommand::Start);
        assert!(!start.blocked);

        let mut reverse = sample_mover(KinematicMoverMode::PingPong, 0.0);
        reverse.blocked = true;
        apply_mover_command(&mut reverse, &MoverCommand::Reverse);
        assert!(!reverse.blocked);

        let mut go_to = sample_mover(KinematicMoverMode::PingPong, 0.0);
        go_to.blocked = true;
        apply_mover_command(
            &mut go_to,
            &MoverCommand::GoToPathNode("finish".to_string()),
        );
        assert!(!go_to.blocked);
    }

    #[test]
    fn set_spin_rate_converts_degrees_and_mutates_only_target_rate() {
        let mut mover = spin_capable_mover(KinematicMoverMode::PingPong, 250.0);
        mover.direction_sign = -1;
        mover.segment_elapsed_ms = 750.0;
        mover.wait_remaining_ms = 100.0;
        mover.current_linear_velocity = Vec3::X;
        mover.completed = true;
        mover.spin_angle_rad = 0.75;
        mover.spin_rate_rad_s = 1.25;
        mover.spin_target_rate_rad_s = -0.5;

        let mut expected = mover.clone();
        expected.spin_target_rate_rad_s = 180.0_f32.to_radians();

        apply_mover_command(&mut mover, &MoverCommand::SetSpinRate(180.0));

        assert_eq!(mover, expected);
    }

    #[test]
    fn set_spin_rate_skips_non_finite_targets_without_mutating_phase() {
        let mover = sample_mover(KinematicMoverMode::PingPong, 250.0);

        for rate in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut candidate = mover.clone();
            apply_mover_command(&mut candidate, &MoverCommand::SetSpinRate(rate));
            assert_eq!(candidate, mover);
        }
    }

    // Regression: a zero-axis v1/default mover accepted a nonzero target and
    // reached axis-angle construction with an invalid angular phase.
    #[test]
    fn set_spin_rate_with_non_normalizable_axis_is_a_no_op_through_tick() {
        for spin_axis in [Vec3::ZERO, Vec3::splat(f32::MIN_POSITIVE)] {
            let mut registry = EntityRegistry::new();
            let entity = registry.spawn(transform_at(Vec3::ZERO));
            let mut mover = sample_mover(KinematicMoverMode::PingPong, 0.0);
            mover.spin_axis = spin_axis;
            registry.set_component(entity, mover.clone()).unwrap();

            apply_mover_command_to_known_movers(
                &mut registry,
                &[entity],
                &MoverCommand::SetSpinRate(180.0),
            );
            let mut tick_states = super::super::MoverTickStateTable::default();
            super::super::run_kinematic_mover_tick(&mut registry, &mut tick_states, 0.25);

            let after = registry
                .get_component::<KinematicMoverComponent>(entity)
                .unwrap();
            let transform = registry
                .get_component::<postretro_entities::Transform>(entity)
                .unwrap();
            let tick = tick_states.get(after.mover_id).unwrap();
            assert_eq!(after.spin_target_rate_rad_s, 0.0);
            assert_eq!(after.spin_rate_rad_s, 0.0);
            assert_eq!(after.spin_angle_rad, 0.0);
            assert!((transform.position - Vec3::new(0.25, 0.0, 0.0)).length() < 1.0e-6);
            assert!(transform.rotation.is_finite());
            assert!(transform.rotation.abs_diff_eq(Quat::IDENTITY, 1.0e-6));
            assert_eq!(tick.angular_velocity, Vec3::ZERO);
            assert_eq!(tick.tick_rotation_delta, Quat::IDENTITY);
        }
    }

    #[test]
    fn set_spin_rate_zero_remains_valid_for_a_zero_axis_mover() {
        let mut mover = sample_mover(KinematicMoverMode::PingPong, 0.0);
        mover.spin_target_rate_rad_s = 1.0;

        apply_mover_command(&mut mover, &MoverCommand::SetSpinRate(0.0));

        assert_eq!(mover.spin_target_rate_rad_s, 0.0);
    }

    #[test]
    fn set_spin_rate_spins_up_zero_rate_mover_with_authored_axis() {
        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(transform_at(Vec3::ZERO));
        registry
            .set_component(
                entity,
                spin_capable_mover(KinematicMoverMode::PingPong, 0.0),
            )
            .unwrap();

        apply_mover_command_to_known_movers(
            &mut registry,
            &[entity],
            &MoverCommand::SetSpinRate(180.0),
        );
        let mut tick_states = super::super::MoverTickStateTable::default();
        super::super::run_kinematic_mover_tick(&mut registry, &mut tick_states, 0.25);

        let mover = registry
            .get_component::<KinematicMoverComponent>(entity)
            .unwrap();
        let transform = registry
            .get_component::<postretro_entities::Transform>(entity)
            .unwrap();
        assert!((mover.spin_rate_rad_s - std::f32::consts::PI).abs() < 1.0e-6);
        assert!((mover.spin_target_rate_rad_s - std::f32::consts::PI).abs() < 1.0e-6);
        assert!((mover.spin_angle_rad - std::f32::consts::FRAC_PI_4).abs() < 1.0e-6);
        assert!(
            transform
                .rotation
                .abs_diff_eq(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4), 1.0e-6)
        );
    }

    // Regression: reverse is linear-path control and must not restart a
    // stopped one-waypoint pure rotator.
    #[test]
    fn reverse_does_not_resume_stopped_pure_rotator() {
        let mut mover = KinematicMoverComponent::new(
            7,
            postretro_entities::KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO],
                waypoint_names: vec!["origin".to_string()],
                speed_mps: 0.0,
                wait_ms: 0.0,
                mode: KinematicMoverMode::Once,
                started: true,
                spin_axis: Vec3::Y,
                initial_spin_rate_rad_s: 1.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        );
        apply_mover_command(&mut mover, &MoverCommand::Stop);
        let stopped = mover.clone();

        apply_mover_command(&mut mover, &MoverCommand::Reverse);

        assert_eq!(mover, stopped);
        let mut transform = transform_at(Vec3::ZERO);
        let pose = super::super::advance_mover_phase_one_tick(&mut mover, &mut transform, 0.25);
        assert_eq!(mover.spin_angle_rad, 0.0);
        assert_eq!(pose.angular_velocity, Vec3::ZERO);
        assert_eq!(pose.tick_rotation_delta, Quat::IDENTITY);
    }

    #[test]
    fn reverse_still_resumes_stopped_translating_mover() {
        let mut mover = sample_mover(KinematicMoverMode::PingPong, 0.0);
        apply_mover_command(&mut mover, &MoverCommand::Stop);

        apply_mover_command(&mut mover, &MoverCommand::Reverse);

        assert!(mover.started);
        assert!(!mover.completed);
        assert_eq!(mover.direction_sign, -1);
    }

    #[test]
    fn command_target_applier_skips_non_movers() {
        let mut registry = EntityRegistry::new();
        let mover_entity = registry.spawn(transform_at(Vec3::ZERO));
        registry
            .set_component(
                mover_entity,
                sample_mover(KinematicMoverMode::PingPong, 0.0),
            )
            .unwrap();
        let non_mover = registry.spawn(transform_at(Vec3::ONE));
        registry
            .set_tags(mover_entity, vec!["lift_group".to_string()])
            .unwrap();
        registry
            .set_tags(non_mover, vec!["lift_group".to_string()])
            .unwrap();

        apply_mover_command_to_targets(
            &mut registry,
            &[mover_entity, non_mover],
            &MoverCommand::Stop,
            &MoverCommandDiagnostics::default(),
        );

        assert!(
            registry
                .get_component::<KinematicMoverComponent>(mover_entity)
                .is_ok()
        );
        assert!(
            !registry
                .get_component::<KinematicMoverComponent>(mover_entity)
                .unwrap()
                .started
        );
        assert!(
            registry
                .get_component::<KinematicMoverComponent>(non_mover)
                .is_err()
        );
    }

    #[test]
    fn set_spin_rate_reaction_and_sequence_routes_match_shared_kvp_command_path() {
        let mut reaction_registry_entities = EntityRegistry::new();
        let reaction_target = reaction_registry_entities.spawn(transform_at(Vec3::ZERO));
        reaction_registry_entities
            .set_component(
                reaction_target,
                spin_capable_mover(KinematicMoverMode::PingPong, 0.0),
            )
            .unwrap();

        let mut kvp_registry = EntityRegistry::new();
        let kvp_target = kvp_registry.spawn(transform_at(Vec3::ZERO));
        assert_eq!(reaction_target, kvp_target, "fixture registries must align");
        kvp_registry
            .set_component(
                kvp_target,
                spin_capable_mover(KinematicMoverMode::PingPong, 0.0),
            )
            .unwrap();

        let sequence_ctx = ScriptCtx::new();
        let sequence_target = sequence_ctx
            .registry
            .borrow_mut()
            .spawn(transform_at(Vec3::ZERO));
        sequence_ctx
            .registry
            .borrow_mut()
            .set_component(
                sequence_target,
                spin_capable_mover(KinematicMoverMode::PingPong, 0.0),
            )
            .unwrap();

        let mut reactions = ReactionPrimitiveRegistry::new();
        register_mover_reaction_primitives(&mut reactions, Default::default());
        assert!(
            reactions
                .dispatch(
                    "moverSetSpinRate",
                    &mut reaction_registry_entities,
                    &[reaction_target],
                    &serde_json::json!({ "rate": -90.0 }),
                )
                .unwrap()
        );

        let mut sequences = SequencedPrimitiveRegistry::new();
        register_sequenced_mover_primitives(
            &mut sequences,
            sequence_ctx.clone(),
            Default::default(),
        );
        sequences
            .get("moverSetSpinRate")
            .expect("set-spin-rate sequence primitive should register")(
            sequence_target,
            &serde_json::json!({ "rate": -90.0 }),
        )
        .unwrap();

        apply_mover_command_to_targets(
            &mut kvp_registry,
            &[kvp_target],
            &MoverCommand::SetSpinRate(-90.0),
            &MoverCommandDiagnostics::default(),
        );

        let kvp_mover = kvp_registry
            .get_component::<KinematicMoverComponent>(kvp_target)
            .unwrap();
        assert_eq!(
            reaction_registry_entities
                .get_component::<KinematicMoverComponent>(reaction_target)
                .unwrap(),
            kvp_mover,
            "the reaction primitive must use the same mover-phase applier as KVP commands"
        );
        let sequence_mover = sequence_ctx
            .registry
            .borrow()
            .get_component::<KinematicMoverComponent>(sequence_target)
            .unwrap()
            .clone();
        assert_eq!(
            sequence_mover, *kvp_mover,
            "the sequence primitive must use the same mover-phase applier as KVP commands"
        );
        assert_eq!(
            kvp_mover.spin_target_rate_rad_s,
            (-90.0_f32).to_radians(),
            "the shared applier owns degrees-to-radians conversion"
        );
    }
}
