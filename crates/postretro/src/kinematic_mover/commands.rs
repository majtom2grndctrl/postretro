//! Declarative mover command application and scripting registrations.

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
            mover.current_linear_velocity = Vec3::ZERO;
        }
        MoverCommand::Reverse => {
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
            vec![Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0)],
            vec!["start".to_string(), "finish".to_string()],
            1.0,
            wait_ms,
            mode,
            true,
            Vec3::ZERO,
            0.0,
            0.0,
            false,
        )
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
        assert_eq!(mover.current_linear_velocity, Vec3::ZERO);
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
    fn set_spin_rate_converts_degrees_and_mutates_only_target_rate() {
        let mut mover = sample_mover(KinematicMoverMode::PingPong, 250.0);
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
                sample_mover(KinematicMoverMode::PingPong, 0.0),
            )
            .unwrap();

        let mut kvp_registry = EntityRegistry::new();
        let kvp_target = kvp_registry.spawn(transform_at(Vec3::ZERO));
        assert_eq!(reaction_target, kvp_target, "fixture registries must align");
        kvp_registry
            .set_component(kvp_target, sample_mover(KinematicMoverMode::PingPong, 0.0))
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
                sample_mover(KinematicMoverMode::PingPong, 0.0),
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
