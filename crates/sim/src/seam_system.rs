//! Authoritative level-seam `arrive` bindings and frame-end presentation residuals.
//! See: context/lib/entity_model.md §7e (Level Seams) · context/lib/scripting.md §12

use std::cell::RefCell;
use std::collections::HashSet;

use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, FireOrigin, FireSubject, ReactionDescriptor,
    ScriptCtx, SeamComponent, SequenceTarget, SystemCommandFireContext,
};
use postretro_foundation::Seat;
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

/// The one event a seam publishes.
const ARRIVE: &str = "arrive";

#[derive(Default)]
struct Binding {
    entity: Option<EntityId>,
    commands: Vec<BoundTriggerCommand>,
    residual: Vec<PrepartitionedReactionStep>,
}

/// Indexed by the baked seam index. Built at install and recomposition.
#[derive(Default)]
pub struct SeamBindingTable {
    bindings: Vec<Binding>,
    scope: Option<RefCell<DispatchScope>>,
    diagnostics: MoverCommandDiagnostics,
    spawn_context: SpawnContext,
}

/// Presentation/wait work saved at fire time for the frame-end drain. The
/// arriving player's identity is captured while the pawn is known to be
/// theirs, as a teleport's is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeamArriveResidual {
    pub index: u16,
    pub seam: EntityId,
    pub pawn: EntityId,
    pub player: PlayerId,
    /// `None` for an unseated pawn (single player's local pawn).
    pub seat: Option<Seat>,
}

/// Whether a reaction reads `on.seam`, which only a seam's `arrive` publishes.
pub(crate) fn reaction_uses_seam(descriptor: &ReactionDescriptor) -> bool {
    match descriptor {
        ReactionDescriptor::Primitive(p) => p.target.as_deref() == Some("@seam"),
        ReactionDescriptor::Sequence(steps) => steps
            .iter()
            .any(|s| matches!(s.id, SequenceTarget::FiredSeam)),
        ReactionDescriptor::Progress(_) => false,
    }
}

/// What an `arrive` binding rejects: exactly what a teleport event rejects,
/// with the teleporter token in place of the seam's.
fn invalid_scope(descriptor: &ReactionDescriptor) -> bool {
    fn invalid_args(args: &serde_json::Value) -> bool {
        ["@occupancy", "@rising", "@dt"]
            .iter()
            .any(|name| reads_dispatch_input(args, name))
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
            matches!(p.target.as_deref(), Some("@trigger" | "@teleporter")) || invalid_args(&p.args)
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
                        SequenceTarget::FiredTrigger
                            | SequenceTarget::FiredTeleporter
                            | SequenceTarget::EventPlayer
                    )
                    || (waited
                        && matches!(
                            step.id,
                            SequenceTarget::Activators | SequenceTarget::FiredSeam
                        ))
            })
        }
        ReactionDescriptor::Progress(_) => false,
    }
}

impl SeamBindingTable {
    /// No seam is bound: the default table a level unload leaves.
    #[cfg(any(test, feature = "test-support"))]
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

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
        for (id, value) in registry.iter_with_kind(ComponentKind::Seam) {
            let ComponentValue::Seam(seam) = value else {
                continue;
            };
            let index = seam.index as usize;
            table
                .bindings
                .resize_with(table.bindings.len().max(index + 1), Binding::default);
            table.bindings[index].entity = Some(id);
            let mut names: Vec<&str> = Vec::new();
            if !seam.on_arrive.is_empty() {
                names.push(&seam.on_arrive);
            }
            for event in data.seam_events().iter().filter(|event| event.seam == id) {
                if event.event != ARRIVE {
                    continue;
                }
                for name in &event.fire {
                    if manifest_seen.insert((id, name.as_str())) {
                        names.push(name);
                    } else {
                        log::warn!(
                            "[Seam] duplicate member binding for {id:?} reaction `{name}`; binding once"
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
                            "[Seam] reaction `{name}` reads an input this arrive event does not publish or retains a token after wait; not binding"
                        );
                        continue;
                    }
                    let binding = &mut table.bindings[index];
                    partition_direct_reaction(
                        reaction,
                        ordinal,
                        BindSite::Seam,
                        &data,
                        &slots,
                        Some(ctx),
                        &mut binding.commands,
                        &mut binding.residual,
                    );
                }
                if !found {
                    log::warn!(
                        "[Seam] arrive event on {id:?} names missing reaction `{name}`; skipping"
                    );
                }
            }
        }
        for event in data.seam_events() {
            if registry.get_component::<SeamComponent>(event.seam).is_err() {
                log::warn!(
                    "[Seam] member binding target {:?} is not a live level seam; skipping",
                    event.seam
                );
            }
        }
        table.scope = Some(RefCell::new(DispatchScope::script(ctx.clone(), &[])));
        table
    }

    /// Call once per arriving pawn, inside the first authoritative tick after
    /// its spawn. Bound commands run now, but a seam `arm`/`disarm` they issue
    /// only enqueues a mutation that applies at the start of the next tick;
    /// every other command applies in-tick. The destination seam's armed state
    /// does not gate its `arrive`. Host and single player only.
    pub fn execute(
        &self,
        ctx: &ScriptCtx,
        index: u16,
        pawn: EntityId,
        player: PlayerId,
    ) -> Option<SeamArriveResidual> {
        if !group_commands_apply_here(ctx) {
            return None;
        }
        let binding = self.bindings.get(index as usize)?;
        let entity = binding.entity?;
        let scope = self.scope.as_ref()?;
        let mut registry = ctx.registry.borrow_mut();
        if !registry.exists(pawn) || registry.get_component::<SeamComponent>(entity).is_err() {
            return None;
        }
        let fire = TriggerFireContext {
            fired_seam: Some(entity),
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
        let seat = registry.seat_for_pawn(pawn);
        (!binding.residual.is_empty()).then_some(SeamArriveResidual {
            index,
            seam: entity,
            pawn,
            player,
            seat,
        })
    }
}

/// Drain the saved presentation/wait work at frame end. The direct commands
/// already ran in-tick, so their next-tick writes cannot be delayed by catch-up.
pub fn drain_seam_residuals(
    residuals: &[SeamArriveResidual],
    bindings: &SeamBindingTable,
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
        if binding.entity != Some(residual.seam) {
            continue;
        }
        // A seated player who left before the drain has nobody to present to;
        // skip rather than let the default recipient route to the host screen.
        if residual.seat.is_some()
            && ctx.registry.borrow().seat_for_pawn(residual.pawn) != residual.seat
        {
            continue;
        }
        let _origin = scheduler.begin_origin(residual.seam, residual.player, false);
        let previous = ctx
            .system_commands
            .replace_fire_context(SystemCommandFireContext {
                source: format!("seam:{:?}", residual.seam),
                origin: FireOrigin::HostOnly(FireSubject::Seam {
                    player: residual.seat,
                    seam: residual.seam,
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
mod tests;
