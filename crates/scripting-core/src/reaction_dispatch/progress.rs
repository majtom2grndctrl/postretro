// Kill-progress subscriptions: per-tag thresholds over the members captured at install.
// See: context/lib/scripting.md §12 (Reaction Dispatch Model — Entity addressing)

use std::collections::{HashMap, HashSet};

use crate::data_descriptors::ReactionDescriptor;
use crate::data_registry::DataRegistry;
use crate::registry::{EntityId, EntityRegistry};

/// The entities carrying one progress tag when the level installed. Only
/// their kills count, so an NPC a spawner releases later — which inherits its
/// spawner's tags — neither raises `total` nor advances `killed`.
#[derive(Debug, Clone, PartialEq)]
struct TagMembership {
    /// Install-time members not yet killed. A credited kill removes its id, so
    /// a repeated report for one entity never counts twice. Ids are
    /// generation-checked: a later spawn reusing a freed slot is not a member.
    remaining: HashSet<EntityId>,
    total: u32,
    killed: u32,
}

/// Threshold compare: `killed/total >= at` (`at: 1.0` means "all dead").
#[derive(Debug, Clone, PartialEq)]
struct ProgressState {
    tag: String,
    at: f32,
    fire: String,
    /// One-shot guard: fires exactly once even if more members die after the threshold is crossed.
    fired: bool,
}

/// Active progress subscriptions for the current level. Membership is keyed by
/// tag and shared by every subscription on that tag; an entity carrying several
/// subscribed tags at install counts toward each independently.
pub struct ProgressTracker {
    memberships: HashMap<String, TagMembership>,
    /// Data-registry order, so one kill fires its targets in a fixed order.
    subscriptions: Vec<ProgressState>,
}

impl ProgressTracker {
    pub fn new() -> Self {
        Self {
            memberships: HashMap::new(),
            subscriptions: Vec::new(),
        }
    }

    /// Level install: capture each subscribed tag's members from `entity_registry`.
    /// Callers should `clear()` first so a previous level's ids never linger.
    pub fn initialize(&mut self, data_registry: &DataRegistry, entity_registry: &EntityRegistry) {
        self.subscribe(data_registry, entity_registry, Vec::new());
    }

    /// Recompose (mod hot reload): rebuild subscriptions from the recomposed
    /// reaction set while keeping each tag's install-time membership and kill
    /// tally, so entities spawned since install never join the set. A tag new to
    /// this recompose captures its members now. A subscription identical to one
    /// already fired stays fired.
    pub fn recompose(&mut self, data_registry: &DataRegistry, entity_registry: &EntityRegistry) {
        let previous = std::mem::take(&mut self.subscriptions);
        let subscribed: HashSet<String> = data_registry
            .reactions
            .iter()
            .filter_map(|named| match &named.descriptor {
                ReactionDescriptor::Progress(p) => Some(p.tag.clone()),
                _ => None,
            })
            .collect();
        self.memberships.retain(|tag, _| subscribed.contains(tag));
        self.subscribe(data_registry, entity_registry, previous);
    }

    fn subscribe(
        &mut self,
        data_registry: &DataRegistry,
        entity_registry: &EntityRegistry,
        previous: Vec<ProgressState>,
    ) {
        for named in &data_registry.reactions {
            let ReactionDescriptor::Progress(p) = &named.descriptor else {
                continue;
            };
            self.memberships
                .entry(p.tag.clone())
                .or_insert_with(|| capture_members(entity_registry, &p.tag));
            let fired = previous.iter().any(|state| {
                state.fired && state.tag == p.tag && state.at == p.at && state.fire == p.fire
            });
            self.subscriptions.push(ProgressState {
                tag: p.tag.clone(),
                at: p.at,
                fire: p.fire.clone(),
                fired,
            });
        }
    }

    /// Credit the kill of `entity`. Returns event names to fire; caller passes
    /// each name to [`super::fire_named_event_with_sequences`].
    pub fn on_entity_killed(&mut self, entity: EntityId) -> Vec<String> {
        let mut credited = false;
        for membership in self.memberships.values_mut() {
            if membership.remaining.remove(&entity) {
                membership.killed = membership.killed.saturating_add(1);
                credited = true;
            }
        }
        if !credited {
            return Vec::new();
        }
        let mut to_fire = Vec::new();
        for state in &mut self.subscriptions {
            if state.fired {
                continue;
            }
            let Some(membership) = self.memberships.get(&state.tag) else {
                continue;
            };
            if membership.total == 0 {
                continue;
            }
            let ratio = membership.killed as f32 / membership.total as f32;
            if ratio >= state.at {
                state.fired = true;
                to_fire.push(state.fire.clone());
            }
        }
        to_fire
    }

    pub fn clear(&mut self) {
        self.memberships.clear();
        self.subscriptions.clear();
    }

    #[cfg(test)]
    pub(super) fn subscription_count(&self, tag: &str) -> usize {
        self.subscriptions
            .iter()
            .filter(|state| state.tag == tag)
            .count()
    }
}

impl Default for ProgressTracker {
    fn default() -> Self {
        Self::new()
    }
}

fn capture_members(entity_registry: &EntityRegistry, tag: &str) -> TagMembership {
    use crate::registry::ComponentKind;

    // INVARIANT: every spawned entity carries a Transform component — `EntityRegistry::spawn`
    // writes it unconditionally. Walking only the Transform column visits each tagged
    // entity once, however many components it carries.
    let remaining: HashSet<EntityId> = entity_registry
        .query_by_component_and_tag(ComponentKind::Transform, Some(tag))
        .map(|(id, _)| id)
        .collect();
    TagMembership {
        total: remaining.len() as u32,
        remaining,
        killed: 0,
    }
}
