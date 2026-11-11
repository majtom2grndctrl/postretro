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

/// Prefix of the warn-once keys for draws naming no registered token. A newly
/// committed vocabulary drops them, so each reload warns afresh.
pub(crate) const ATTACK_DRAW_WARNING: &str = "attack-draw:";

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
            resolve_attack_draws(name, attack, attack_budgets, warned)
        })
        .collect()
}

/// THE draw-resolution chokepoint: what one attack draws, as token ids in the
/// registered vocabulary. An attack without `draws` draws one `defaultToken`.
/// Runs at bind time and on a reload, never per tick. A reference naming no
/// registered token (stale content load validation never saw, such as a reload
/// that removed the token) is left out, so that token no longer limits the
/// attack. It warns once per attack and token for each committed vocabulary
/// (keys under [`ATTACK_DRAW_WARNING`]).
fn resolve_attack_draws(
    attack_name: &str,
    attack: &AttackParams,
    vocabulary: &AttackBudgetVocabulary,
    warned: &mut HashSet<String>,
) -> ResolvedDraws {
    let resolved: Vec<ResolvedDraw> = vocabulary
        .resolve_draws(attack.draws.as_deref(), |token| {
            if warned.insert(format!("{ATTACK_DRAW_WARNING}{attack_name}:{token}")) {
                log::warn!(
                    "[AI] attack `{attack_name}` draws `{token}`, which is not a registered attack token; the draw is ignored, so `{token}` no longer limits this attack"
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
