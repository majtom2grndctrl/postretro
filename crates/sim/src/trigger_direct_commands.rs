//! A trigger's no-script `command`: per-kind acceptance, Enter application, install check.
//! See: context/lib/build_pipeline.md §Custom FGD (trigger_volume) · context/lib/scripting.md §12

use postretro_entities::components::projectile_trap::TrapRequest;
use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, TriggerCommand, TriggerVolumeComponent,
};

use crate::kinematic_mover::MoverAutoCloseTimers;
use crate::mover_commands::apply_mover_command_to_known_movers;
use crate::spawner::{SpawnContext, spawn_from_spawner_member};

/// The member kinds a trigger command can reach. A member carrying a brain is
/// never one, whatever its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandKind {
    Mover,
    Trap,
    Spawner,
}

const COMMAND_KINDS: [(CommandKind, ComponentKind); 3] = [
    (CommandKind::Mover, ComponentKind::KinematicMover),
    (CommandKind::Trap, ComponentKind::ProjectileTrap),
    (CommandKind::Spawner, ComponentKind::Spawner),
];

/// The acceptance table. Movers take their motion commands, traps `fire`,
/// `start` and `stop`, spawners `fire` (one batch per firing).
fn accepts(kind: CommandKind, command: &TriggerCommand) -> bool {
    match kind {
        CommandKind::Mover => !matches!(command, TriggerCommand::Fire),
        CommandKind::Trap => matches!(
            command,
            TriggerCommand::Fire | TriggerCommand::Start | TriggerCommand::Stop
        ),
        CommandKind::Spawner => matches!(command, TriggerCommand::Fire),
    }
}

fn trap_request(command: &TriggerCommand) -> Option<TrapRequest> {
    match command {
        TriggerCommand::Fire => Some(TrapRequest::Fire),
        TriggerCommand::Start => Some(TrapRequest::Start {
            interval_ms: None,
            phase_ms: None,
        }),
        TriggerCommand::Stop => Some(TrapRequest::Stop),
        TriggerCommand::Reverse | TriggerCommand::GoToPathNode(_) => None,
    }
}

fn has_brain(registry: &EntityRegistry, id: EntityId) -> bool {
    matches!(
        registry.has_component_kind(id, ComponentKind::Brain),
        Ok(true)
    )
}

/// Brainless members of `kind`'s column carrying `tag`, in registry order.
fn tagged_members(registry: &EntityRegistry, column: ComponentKind, tag: &str) -> Vec<EntityId> {
    let mut members: Vec<EntityId> = registry
        .query_by_component_and_tag(column, Some(tag))
        .map(|(id, _)| id)
        .filter(|&id| !has_brain(registry, id))
        .collect();
    members.sort_unstable();
    members
}

/// Shared handles a trigger's direct command reaches beyond the registry.
#[derive(Debug, Clone, Default)]
pub struct DirectCommandHandles {
    pub auto_close_timers: Option<MoverAutoCloseTimers>,
    /// Absent in fixtures that author no spawner; a spawner `fire` then skips.
    pub spawn_context: Option<SpawnContext>,
}

/// Apply a trigger's command on Enter to every brainless member carrying
/// `tag` whose kind accepts it. Runs only on the authoritative trigger tick,
/// so a trap request here acts on this tick's trap stage.
pub(crate) fn apply_direct_trigger_command(
    registry: &mut EntityRegistry,
    tag: &str,
    command: &TriggerCommand,
    handles: &DirectCommandHandles,
) {
    if tag.is_empty() {
        return;
    }
    for (kind, column) in COMMAND_KINDS {
        if !accepts(kind, command) {
            continue;
        }
        let members = tagged_members(registry, column, tag);
        if members.is_empty() {
            continue;
        }
        match kind {
            CommandKind::Mover => {
                let mover = command
                    .mover_command()
                    .expect("movers accept only commands with a mover form");
                apply_mover_command_to_known_movers(
                    registry,
                    &members,
                    &mover,
                    handles.auto_close_timers.as_ref(),
                );
            }
            CommandKind::Trap => {
                let request = trap_request(command).expect("traps accept only trap requests");
                for trap in members {
                    crate::traps::issue_trap_request(registry, trap, request);
                }
            }
            CommandKind::Spawner => {
                let Some(spawn_context) = handles.spawn_context.as_ref() else {
                    log::debug!("[Trigger] spawner `fire` without a spawn context; skipping");
                    continue;
                };
                for spawner in members {
                    spawn_from_spawner_member(registry, spawner, spawn_context);
                }
            }
        }
    }
}

/// Warn once per trigger whose non-empty `target_tag` some entity carries but
/// no member accepts the trigger's command. An empty or unmatched tag stays
/// silent. Runs at level install, after every map placement materializes.
pub fn warn_unaccepted_trigger_commands(registry: &EntityRegistry) {
    let mut triggers: Vec<(EntityId, TriggerVolumeComponent)> = registry
        .iter_with_kind(ComponentKind::TriggerVolume)
        .filter_map(|(id, _)| {
            registry
                .get_component::<TriggerVolumeComponent>(id)
                .ok()
                .map(|trigger| (id, trigger.clone()))
        })
        .collect();
    triggers.sort_unstable_by_key(|(id, _)| *id);
    for (id, trigger) in triggers {
        let tag = trigger.target_tag.as_str();
        if tag.is_empty() {
            continue;
        }
        let carried = registry
            .query_by_component_and_tag(ComponentKind::Transform, Some(tag))
            .next()
            .is_some();
        let accepted = COMMAND_KINDS.iter().any(|&(kind, column)| {
            accepts(kind, &trigger.command) && !tagged_members(registry, column, tag).is_empty()
        });
        if carried && !accepted {
            log::warn!(
                "[Trigger] trigger {id}: command `{}` reaches no member tagged `{tag}`; no brainless member carrying the tag accepts it",
                command_name(&trigger.command)
            );
        }
    }
}

fn command_name(command: &TriggerCommand) -> &'static str {
    match command {
        TriggerCommand::Start => "start",
        TriggerCommand::Stop => "stop",
        TriggerCommand::Reverse => "reverse",
        TriggerCommand::GoToPathNode(_) => "go_to_path_node",
        TriggerCommand::Fire => "fire",
    }
}

#[cfg(test)]
mod tests;
