// What attacks draw: the one chokepoint that resolves an attack's `draws` to
// token ids, and the live descriptor a reload resolves them from.
// See: context/lib/entity_model.md §7c (Attack budget)

use std::collections::HashSet;

use postretro_entities::components::brain::{
    AttackTokenId, MAX_ATTACK_DRAWS, ResolvedDraw, ResolvedDraws,
};
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{EntityId, EntityRegistry, EntityTypeDescriptor};
use postretro_foundation::attack_budget::AttackBudgetVocabulary;
use postretro_foundation::{AttackParams, BehaviorGraphDescriptor};

/// The behavior of the live descriptor `entity` was materialized from, found
/// by its provenance's canonical name. `None` for an entity with no
/// descriptor provenance, or one whose descriptor no longer declares a
/// behavior.
pub(crate) fn live_behavior<'a>(
    registry: &'a EntityRegistry,
    entity: EntityId,
    descriptors: &'a [EntityTypeDescriptor],
) -> Option<&'a BehaviorGraphDescriptor> {
    let provenance = registry
        .get_component::<DescriptorProvenance>(entity)
        .ok()?;
    crate::scripting::builtins::data_archetype::find_descriptor(
        descriptors,
        &provenance.canonical_name,
    )?
    .behavior
    .as_ref()
}

/// Each attack's draws, in `graph`'s attack-name order: the turn identity.
/// An attack the live behavior still declares resolves from it; otherwise
/// from `graph`, the brain's spawn-time copy.
pub(crate) fn resolve_graph_draws(
    graph: &BehaviorGraphDescriptor,
    live: Option<&BehaviorGraphDescriptor>,
    attack_budgets: &AttackBudgetVocabulary,
    warned: &mut HashSet<String>,
) -> Vec<ResolvedDraws> {
    graph
        .attacks
        .iter()
        .map(|(name, attack)| {
            let attack = live
                .and_then(|live| live.attacks.get(name))
                .unwrap_or(attack);
            resolve_attack_draws(attack, attack_budgets, warned)
        })
        .collect()
}

/// THE draw-resolution chokepoint: what one attack draws, as token ids in the
/// registered vocabulary. An attack without `draws` draws one `defaultToken`.
/// Runs at bind time and on a reload, never per tick. A reference naming no
/// registered token (stale content load validation never saw) warns once and
/// is left out.
fn resolve_attack_draws(
    attack: &AttackParams,
    vocabulary: &AttackBudgetVocabulary,
    warned: &mut HashSet<String>,
) -> ResolvedDraws {
    let resolved: Vec<ResolvedDraw> = vocabulary
        .resolve_draws(attack.draws.as_deref(), |token| {
            if warned.insert(format!("attack-draw:{token}")) {
                log::warn!(
                    "[AI] an attack draws unregistered attack token `{token}`; the draw is ignored"
                );
            }
        })
        .into_iter()
        .take(MAX_ATTACK_DRAWS)
        .map(|(token, cost)| ResolvedDraw {
            token: AttackTokenId(token),
            cost,
        })
        .collect();
    ResolvedDraws::from_slice(&resolved).expect("draw lists are capped at MAX_ATTACK_DRAWS")
}
