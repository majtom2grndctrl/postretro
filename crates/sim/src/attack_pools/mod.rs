//! Host-authoritative attack pools: issuing starts and stops, the drain before the AI tick, membership.
//! See: context/lib/entity_model.md §7c (Attack budget, Attack pools) · context/lib/scripting.md §12

pub mod commands;
pub mod install;

use postretro_entities::components::attack_pool::{AttackPoolComponent, AttackPoolRequest};
use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry};
use postretro_foundation::{START_ATTACK_POOL, STOP_ATTACK_POOL};
use postretro_scripting_core::group_resolution::is_player_pawn;

/// A pool member verb, from a trigger `command`, a bound step or a script step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackPoolVerb {
    Start,
    Stop,
}

impl AttackPoolVerb {
    /// The verb a member primitive names, or `None` for any other primitive.
    pub fn from_primitive(primitive: &str) -> Option<Self> {
        match primitive {
            START_ATTACK_POOL => Some(Self::Start),
            STOP_ATTACK_POOL => Some(Self::Stop),
            _ => None,
        }
    }

    fn request(self, pool: EntityId) -> AttackPoolRequest {
        match self {
            Self::Start => AttackPoolRequest::Start(pool),
            Self::Stop => AttackPoolRequest::Stop(pool),
        }
    }
}

/// Queue `verb` on `pool` for the drain before the next AI tick. The caller
/// owns the host-only gate; this never runs on a connected client. A target
/// that is not an attack pool warns and skips.
pub fn issue_attack_pool_request(
    registry: &mut EntityRegistry,
    pool: EntityId,
    verb: AttackPoolVerb,
) {
    if !matches!(
        registry.has_component_kind(pool, ComponentKind::AttackPool),
        Ok(true)
    ) {
        log::warn!("[AttackPool] command target {pool:?} is not an attack pool; skipping");
        return;
    }
    registry.queue_attack_pool_request(verb.request(pool));
}

/// Queue a spawned enemy's join. It applies in issue order beside the pool
/// requests around it, so a spawn after a stop on one tick never joins the
/// stopped pool. Untagged spawns can match no pool and queue nothing.
pub fn queue_spawn_join(registry: &mut EntityRegistry, enemy: EntityId) {
    if registry.get_tags(enemy).is_ok_and(|tags| !tags.is_empty()) {
        registry.queue_attack_pool_request(AttackPoolRequest::Join(enemy));
    }
}

/// Apply every queued request in issue order. Runs immediately before the AI
/// tick and after the tick's triggers fire, so a trigger-bound request acts
/// on its own tick and every other one, queued after the last drain, acts
/// first on the next. Allocates only when a start assigns members.
pub fn apply_attack_pool_requests(registry: &mut EntityRegistry) {
    let requests = registry.take_attack_pool_requests();
    for &request in &requests {
        match request {
            AttackPoolRequest::Start(pool) => start_pool(registry, pool),
            AttackPoolRequest::Stop(pool) => stop_pool(registry, pool),
            AttackPoolRequest::Join(enemy) => join_latest_pool(registry, enemy),
        }
    }
    registry.return_attack_pool_requests(requests);
}

fn pool_mut(registry: &mut EntityRegistry, pool: EntityId) -> Option<&mut AttackPoolComponent> {
    match registry.get_component_value_mut(pool, ComponentKind::AttackPool) {
        Ok(ComponentValue::AttackPool(component)) => Some(component),
        _ => None,
    }
}

/// One past every pool's start rank, so the latest start ranks highest.
fn next_start_rank(registry: &EntityRegistry) -> u64 {
    registry
        .iter_with_kind(ComponentKind::AttackPool)
        .filter_map(|(_, value)| match value {
            ComponentValue::AttackPool(pool) => Some(pool.start_rank),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        + 1
}

/// Start `pool`, or re-start it: a new run only from stopped, a rank past
/// every other pool's, and every current holder of its member tag assigned.
/// A re-start leaves the run, so turns, waits and refreshing counts stand.
fn start_pool(registry: &mut EntityRegistry, pool: EntityId) {
    let rank = next_start_rank(registry);
    let Some(component) = pool_mut(registry, pool) else {
        log::warn!("[AttackPool] start target {pool:?} is no longer an attack pool; skipping");
        return;
    };
    if !component.running {
        component.running = true;
        component.run += 1;
    }
    component.start_rank = rank;
    if component.member_tag.is_empty() {
        return;
    }
    let member_tag = component.member_tag.clone();
    let members: Vec<EntityId> = registry
        .query_by_component_and_tag(ComponentKind::Brain, Some(&member_tag))
        .map(|(id, _)| id)
        .filter(|&id| !is_player_pawn(registry, id))
        .collect();
    for enemy in members {
        assign(registry, enemy, Some(pool));
    }
}

/// Stop `pool`: its members return to the default pool. A stopped pool is
/// left as it is.
fn stop_pool(registry: &mut EntityRegistry, pool: EntityId) {
    let Some(component) = pool_mut(registry, pool) else {
        log::warn!("[AttackPool] stop target {pool:?} is no longer an attack pool; skipping");
        return;
    };
    if !component.running {
        return;
    }
    component.running = false;
    registry.for_each_with_kind_mut(ComponentKind::Brain, |_, value| {
        if let ComponentValue::Brain(brain) = value
            && brain.attack_pool == Some(pool)
        {
            brain.attack_pool = None;
        }
    });
}

/// A spawn joins the latest-started running pool whose member tag it
/// carries. With none it keeps the default pool.
fn join_latest_pool(registry: &mut EntityRegistry, enemy: EntityId) {
    if !matches!(
        registry.has_component_kind(enemy, ComponentKind::Brain),
        Ok(true)
    ) || is_player_pawn(registry, enemy)
    {
        return;
    }
    let Ok(tags) = registry.get_tags(enemy) else {
        return;
    };
    let latest = registry
        .iter_with_kind(ComponentKind::AttackPool)
        .filter_map(|(pool, value)| match value {
            ComponentValue::AttackPool(component)
                if component.running && tags.contains(&component.member_tag) =>
            {
                Some((component.start_rank, pool))
            }
            _ => None,
        })
        .max();
    if let Some((_, pool)) = latest {
        assign(registry, enemy, Some(pool));
    }
}

fn assign(registry: &mut EntityRegistry, enemy: EntityId, pool: Option<EntityId>) {
    if let Ok(ComponentValue::Brain(brain)) =
        registry.get_component_value_mut(enemy, ComponentKind::Brain)
    {
        brain.attack_pool = pool;
    }
}

#[cfg(test)]
mod tests;
