//! Resolve composed manifest trigger events to `(volume, edge, reaction)`
//! bindings and install them after the brush-authored ones.
//!
//! Two forms reach here: mod-global tag-keyed rules (`defineTriggerEvent`,
//! `DataRegistry::trigger_events`, already filtered by their `levels`
//! selector at compose) and level member events (`t.on`,
//! `DataRegistry::volume_trigger_events`), keyed by one volume id. The order on
//! a shared edge is brush `on_fire`/`on_exit` first (built by
//! `TriggerBindingTable::build_*`), then mod-global rules, then level member
//! events, each in authored order. A resolved `(volume, edge, reaction)` binds
//! once: a second source resolving to it is skipped with one warning naming
//! both, because running the reaction twice would double its effects and
//! restart any `wait` in it.
//! See: context/lib/scripting.md §12 (Per-member sources)

use std::collections::HashMap;
use std::fmt;

use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, ScriptCtx, TriggerVolumeComponent,
};
use postretro_scripting_core::data_registry::DataRegistry;

use super::TriggerBindingTable;
use crate::trigger_system::TriggerEventEdge;

/// Where a resolved manifest trigger-event binding was declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerEventSource {
    /// A mod-global `defineTriggerEvent` rule in the mod manifest, keyed by tag.
    ModManifest { tag: String },
    /// A level member event (`t.on`) returned from the level script.
    LevelScript { trigger: EntityId },
}

impl fmt::Display for TriggerEventSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ModManifest { tag } => write!(
                formatter,
                "the mod manifest's `defineTriggerEvent` for tag `{tag}`"
            ),
            Self::LevelScript { trigger } => {
                write!(formatter, "the level script's `on` for trigger {trigger}")
            }
        }
    }
}

/// One `(volume, edge, reaction)` binding resolved from a manifest trigger
/// event, before it is partitioned into tick commands and residual steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTriggerEvent {
    pub trigger: EntityId,
    pub edge: TriggerEventEdge,
    pub reaction: String,
    pub source: TriggerEventSource,
}

/// Whether resolution logs its authoring diagnostics. Install reports them
/// once; the install-time validation passes resolve the same set silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionDiagnostics {
    Report,
    Silent,
}

fn edge_for(event: &str) -> Option<TriggerEventEdge> {
    match event {
        "enter" => Some(TriggerEventEdge::Enter),
        "exit" => Some(TriggerEventEdge::Exit),
        _ => None,
    }
}

/// Resolve the composed manifest trigger events against the installed
/// registry: mod-global rules expand to every volume carrying their tag
/// (sorted by id), level member events bind their one volume when its id is
/// still a live trigger volume (generation-checked; stale ids warn-skip). The
/// result is in binding order and holds each `(volume, edge, reaction)` once.
pub fn resolve_manifest_trigger_events(
    registry: &EntityRegistry,
    data_registry: &DataRegistry,
    diagnostics: ResolutionDiagnostics,
) -> Vec<ResolvedTriggerEvent> {
    let report = diagnostics == ResolutionDiagnostics::Report;
    let mut resolved = Vec::new();

    for descriptor in &data_registry.trigger_events {
        let Some(edge) = edge_for(&descriptor.event) else {
            if report {
                log::warn!(
                    "[Trigger] unknown trigger-event `{}` on tag `{}`; descriptor is inert",
                    descriptor.event,
                    descriptor.tag
                );
            }
            continue;
        };
        let mut triggers: Vec<EntityId> = registry
            .query_by_component_and_tag(ComponentKind::TriggerVolume, Some(&descriptor.tag))
            .map(|(id, _)| id)
            .collect();
        triggers.sort_unstable();
        if triggers.is_empty() {
            if report {
                log::warn!(
                    "[Trigger] trigger-event tag `{}` matched no trigger volumes; descriptor is inert",
                    descriptor.tag
                );
            }
            continue;
        }
        for reaction in &descriptor.fire {
            for &trigger in &triggers {
                resolved.push(ResolvedTriggerEvent {
                    trigger,
                    edge,
                    reaction: reaction.clone(),
                    source: TriggerEventSource::ModManifest {
                        tag: descriptor.tag.clone(),
                    },
                });
            }
        }
    }

    for descriptor in data_registry.volume_trigger_events() {
        let Some(edge) = edge_for(&descriptor.event) else {
            if report {
                log::warn!(
                    "[Trigger] unknown trigger-event `{}` on trigger {}; level event is inert",
                    descriptor.event,
                    descriptor.trigger
                );
            }
            continue;
        };
        if registry
            .get_component::<TriggerVolumeComponent>(descriptor.trigger)
            .is_err()
        {
            if report {
                log::warn!(
                    "[Trigger] level script trigger event names {}, which is not a live trigger volume (stale or wrong kind); not binding",
                    descriptor.trigger
                );
            }
            continue;
        }
        for reaction in &descriptor.fire {
            resolved.push(ResolvedTriggerEvent {
                trigger: descriptor.trigger,
                edge,
                reaction: reaction.clone(),
                source: TriggerEventSource::LevelScript {
                    trigger: descriptor.trigger,
                },
            });
        }
    }

    dedupe_resolved(resolved, report)
}

/// Keep the first binding of each `(volume, edge, reaction)`; a later source
/// resolving to the same triple is dropped with one warning naming both.
fn dedupe_resolved(resolved: Vec<ResolvedTriggerEvent>, report: bool) -> Vec<ResolvedTriggerEvent> {
    let mut first_source: HashMap<(EntityId, TriggerEventEdge, String), TriggerEventSource> =
        HashMap::with_capacity(resolved.len());
    let mut kept = Vec::with_capacity(resolved.len());
    for binding in resolved {
        let key = (binding.trigger, binding.edge, binding.reaction.clone());
        if let Some(first) = first_source.get(&key) {
            if report {
                log::warn!(
                    "[Trigger] reaction `{}` is bound to {:?} on trigger {} by both {first} and {}; binding it once",
                    binding.reaction,
                    binding.edge,
                    binding.trigger,
                    binding.source,
                );
            }
            continue;
        }
        first_source.insert(key, binding.source.clone());
        kept.push(binding);
    }
    kept
}

impl TriggerBindingTable {
    /// Bind the composed manifest trigger events after brush-authored bindings
    /// are built. They append to any existing binding for the same edge, so
    /// brush KVP work always runs first, then mod-global rules, then level
    /// member events (see [`resolve_manifest_trigger_events`]).
    pub fn install_manifest_events(
        &mut self,
        registry: &EntityRegistry,
        data_registry: &DataRegistry,
        script_ctx: &ScriptCtx,
    ) {
        let resolved =
            resolve_manifest_trigger_events(registry, data_registry, ResolutionDiagnostics::Report);
        let slots = script_ctx.slot_table.borrow();
        for binding in resolved {
            self.bind_event(
                binding.trigger,
                binding.edge,
                &binding.reaction,
                data_registry,
                &slots,
                Some(script_ctx),
            );
        }
    }
}
