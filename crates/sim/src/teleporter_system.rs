//! Authoritative teleporter event bindings and frame-end presentation residuals.
//! See: context/lib/scripting.md §12 (Teleport events)

use std::cell::RefCell;
use std::collections::HashSet;

use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, FireOrigin, FireSubject, ReactionDescriptor,
    ScriptCtx, SequenceTarget, SystemCommandFireContext, TeleporterComponent,
};
use postretro_scripting_core::group_resolution::group_commands_apply_here;
use postretro_scripting_core::ir_scopes::DispatchScope;
use postretro_scripting_core::player_event_scope::{
    reaction_uses_event_player, reads_dispatch_input,
};
use postretro_scripting_core::reaction_dispatch::{
    PrepartitionedReactionStep, ResidualOrigin, fire_prepartitioned_reactions_with_sequences,
};

use crate::mover_commands::MoverCommandDiagnostics;
use crate::residual_drain::ResidualRegistries;
use crate::scripting_systems::reaction_scheduler::ReactionScheduler;
use crate::spawner::SpawnContext;
use crate::trigger_bindings::{BindSite, partition_direct_reaction};
use crate::trigger_commands::{BoundTriggerCommand, TriggerFireContext};
use crate::trigger_system::PlayerId;

#[derive(Default)]
struct Binding {
    entity: Option<EntityId>,
    commands: Vec<BoundTriggerCommand>,
    residual: Vec<PrepartitionedReactionStep>,
}

/// Indexed by the baked teleporter index. Built at install and recomposition.
#[derive(Default)]
pub struct TeleporterBindingTable {
    bindings: Vec<Binding>,
    scope: Option<RefCell<DispatchScope>>,
    diagnostics: MoverCommandDiagnostics,
    spawn_context: SpawnContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeleportResidual {
    pub index: u16,
    pub teleporter: EntityId,
    pub pawn: EntityId,
}

pub(crate) fn reaction_uses_teleporter(descriptor: &ReactionDescriptor) -> bool {
    match descriptor {
        ReactionDescriptor::Primitive(p) => p.target.as_deref() == Some("@teleporter"),
        ReactionDescriptor::Sequence(steps) => steps
            .iter()
            .any(|s| matches!(s.id, SequenceTarget::FiredTeleporter)),
        ReactionDescriptor::Progress(_) => false,
    }
}

fn invalid_scope(descriptor: &ReactionDescriptor) -> bool {
    fn invalid_args(args: &serde_json::Value) -> bool {
        ["@occupancy", "@rising", "@dt"]
            .iter()
            .any(|name| reads_dispatch_input(args, name))
            || args.get("at").is_some()
            || matches!(
                args.get("audience").and_then(serde_json::Value::as_str),
                Some("@trigger" | "@player")
            )
    }
    if reaction_uses_event_player(descriptor) {
        return true;
    }
    match descriptor {
        ReactionDescriptor::Primitive(p) => {
            p.target.as_deref() == Some("@trigger") || invalid_args(&p.args)
        }
        ReactionDescriptor::Sequence(steps) => {
            let mut waited = false;
            steps.iter().any(|step| {
                if matches!(step.id, SequenceTarget::Wait) {
                    waited = true;
                    return step
                        .args
                        .get("interruptible")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true);
                }
                invalid_args(&step.args)
                    || matches!(
                        step.id,
                        SequenceTarget::FiredTrigger | SequenceTarget::EventPlayer
                    )
                    || (waited
                        && matches!(
                            step.id,
                            SequenceTarget::Activators | SequenceTarget::FiredTeleporter
                        ))
            })
        }
        ReactionDescriptor::Progress(_) => false,
    }
}

impl TeleporterBindingTable {
    pub fn build(
        ctx: &ScriptCtx,
        diagnostics: MoverCommandDiagnostics,
        spawn_context: SpawnContext,
    ) -> Self {
        let mut table = Self {
            diagnostics,
            spawn_context,
            ..Self::default()
        };
        if !group_commands_apply_here(ctx) {
            return table;
        }
        let registry = ctx.registry.borrow();
        let data = ctx.data_registry.borrow();
        let slots = ctx.slot_table.borrow();
        let mut manifest_seen = HashSet::new();
        for (id, value) in registry.iter_with_kind(ComponentKind::Teleporter) {
            let ComponentValue::Teleporter(teleporter) = value else {
                continue;
            };
            let index = teleporter.index as usize;
            table
                .bindings
                .resize_with(table.bindings.len().max(index + 1), Binding::default);
            table.bindings[index].entity = Some(id);
            let mut names: Vec<&str> = Vec::new();
            if !teleporter.on_teleport.is_empty() {
                names.push(&teleporter.on_teleport);
            }
            for event in data
                .teleport_events()
                .iter()
                .filter(|event| event.teleporter == id)
            {
                if event.event != "teleport" {
                    continue;
                }
                for name in &event.fire {
                    if manifest_seen.insert((id, name.as_str())) {
                        names.push(name);
                    } else {
                        log::warn!(
                            "[Teleport] duplicate member binding for {id:?} reaction `{name}`; binding once"
                        );
                    }
                }
            }
            for name in names {
                let mut found = false;
                for (ordinal, reaction) in
                    data.reactions.iter().filter(|r| r.name == name).enumerate()
                {
                    found = true;
                    if invalid_scope(&reaction.descriptor) {
                        log::warn!(
                            "[Teleport] reaction `{name}` reads an input this teleport event does not publish or retains a token after wait; not binding"
                        );
                        continue;
                    }
                    let binding = &mut table.bindings[index];
                    partition_direct_reaction(
                        reaction,
                        ordinal,
                        BindSite::Teleport,
                        &data,
                        &slots,
                        Some(ctx),
                        &mut binding.commands,
                        &mut binding.residual,
                    );
                }
                if !found {
                    log::warn!(
                        "[Teleport] event on {id:?} names missing reaction `{name}`; skipping"
                    );
                }
            }
        }
        for event in data.teleport_events() {
            if registry
                .get_component::<TeleporterComponent>(event.teleporter)
                .is_err()
            {
                log::warn!(
                    "[Teleport] member binding target {:?} is not a live teleporter; skipping",
                    event.teleporter
                );
            }
        }
        table.scope = Some(RefCell::new(DispatchScope::script(ctx.clone(), &[])));
        table
    }

    /// Call once per successful authoritative teleport, after every pawn moved.
    /// Commands only enqueue arm state for the next tick. Prediction never calls it.
    pub fn execute(&self, ctx: &ScriptCtx, index: u16, pawn: EntityId) -> Option<TeleportResidual> {
        if !group_commands_apply_here(ctx) {
            return None;
        }
        let binding = self.bindings.get(index as usize)?;
        let entity = binding.entity?;
        let scope = self.scope.as_ref()?;
        let mut registry = ctx.registry.borrow_mut();
        if !registry.exists(pawn)
            || registry
                .get_component::<TeleporterComponent>(entity)
                .is_err()
        {
            return None;
        }
        let fire = TriggerFireContext {
            fired_teleporter: Some(entity),
            activator: Some(pawn),
            ..TriggerFireContext::default()
        };
        let mut scope = scope.borrow_mut();
        for command in &binding.commands {
            command.execute_with_script_ctx(
                &mut registry,
                ctx,
                &mut scope,
                &self.diagnostics,
                &self.spawn_context,
                &fire,
            );
        }
        (!binding.residual.is_empty()).then_some(TeleportResidual {
            index,
            teleporter: entity,
            pawn,
        })
    }
}

/// Drain the saved presentation/wait work at frame end. The direct commands
/// already ran in-tick, so their next-tick writes cannot be delayed by catch-up.
pub fn drain_teleport_residuals(
    residuals: &[TeleportResidual],
    bindings: &TeleporterBindingTable,
    scheduler: &ReactionScheduler,
    registries: ResidualRegistries<'_>,
    ctx: &ScriptCtx,
    follow_ups: &mut Vec<(String, FireOrigin)>,
) {
    if !group_commands_apply_here(ctx) {
        return;
    }
    for residual in residuals {
        let Some(binding) = bindings.bindings.get(residual.index as usize) else {
            continue;
        };
        if binding.entity != Some(residual.teleporter) {
            continue;
        }
        let _origin =
            scheduler.begin_origin(residual.teleporter, PlayerId::Local(residual.pawn), false);
        let player = ctx.registry.borrow().seat_for_pawn(residual.pawn);
        let previous = ctx
            .system_commands
            .replace_fire_context(SystemCommandFireContext {
                source: format!("teleport:{:?}", residual.teleporter),
                origin: FireOrigin::HostOnly(FireSubject::Teleport {
                    player,
                    teleporter: residual.teleporter,
                }),
                ..SystemCommandFireContext::default()
            });
        follow_ups.extend(fire_prepartitioned_reactions_with_sequences(
            &binding.residual,
            registries.sequence,
            registries.reaction,
            registries.system,
            ctx,
            ResidualOrigin::TriggerBinding,
        ));
        ctx.system_commands.replace_fire_context(previous);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teleporter_commands::apply_pending_teleporter_mutations;
    use postretro_entities::{
        NamedReaction, PrimitiveDescriptor, TeleportEventDescriptor, Transform,
    };
    #[test]
    fn teleport_event_disarms_before_next_catchup_tick_and_rejects_occupancy() {
        let ctx = ScriptCtx::new();
        let (gate, pawn) = {
            let mut registry = ctx.registry.borrow_mut();
            let gate = registry.spawn(Transform::default());
            registry
                .set_component(
                    gate,
                    TeleporterComponent {
                        index: 0,
                        source_entity: 1,
                        armed: true,
                        arrival_sound: String::new(),
                        on_teleport: String::new(),
                    },
                )
                .unwrap();
            (gate, registry.spawn(Transform::default()))
        };
        let used = NamedReaction {
            name: "used".into(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "disarmTeleporter".into(),
                target: Some("@teleporter".into()),
                args: serde_json::json!({}),
                kind: None,
                tag: None,
                on_complete: None,
            }),
        };
        ctx.data_registry
            .borrow_mut()
            .populate_level(vec![used], vec![], &[]);
        ctx.data_registry
            .borrow_mut()
            .set_level_teleport_events(vec![TeleportEventDescriptor {
                teleporter: gate,
                event: "teleport".into(),
                fire: vec!["used".into()],
            }]);
        let bindings = TeleporterBindingTable::build(&ctx, Default::default(), Default::default());
        bindings.execute(&ctx, 0, pawn);
        assert!(
            ctx.registry
                .borrow()
                .get_component::<TeleporterComponent>(gate)
                .unwrap()
                .armed
        );
        apply_pending_teleporter_mutations(&ctx);
        assert!(
            !ctx.registry
                .borrow()
                .get_component::<TeleporterComponent>(gate)
                .unwrap()
                .armed
        );
        let bad = ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: "setState".into(),
            target: None,
            args: serde_json::json!({ "value": { "op": "input", "name": "@occupancy" } }),
            kind: None,
            tag: None,
            on_complete: None,
        });
        assert!(invalid_scope(&bad));
        ctx.owner_slot_writes_enabled.set(false);
        bindings.execute(&ctx, 0, pawn);
        assert!(ctx.pending_teleporter_mutations.borrow().is_empty());
    }
}
