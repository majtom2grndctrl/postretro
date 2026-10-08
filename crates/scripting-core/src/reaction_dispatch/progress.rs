// Kill-progress subscriptions: per-tag thresholds over the members captured at install.
// See: context/lib/scripting.md §12 (Reaction Dispatch Model — Entity addressing)

use std::collections::{HashMap, HashSet};

use crate::data_descriptors::ReactionDescriptor;
use crate::data_registry::DataRegistry;
use crate::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use crate::registry::{EntityId, EntityRegistry};

/// One tag's install-time member count and how many of them have died.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TagMembership {
    total: u32,
    killed: u32,
}

/// Identity of a subscription's fired latch: the `(tag, fire)` pair, never the
/// threshold or the subscription's position. A `progress` fires at most once per
/// level per pair, so editing `at`, reordering the composed set (mod-global
/// reactions compose before level ones), or dropping and re-adding a
/// subscription never re-fires it — and two thresholds naming one pair fire once.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Latch {
    tag: String,
    fire: String,
}

/// Threshold compare: `killed/total >= at` (`at: 1.0` means "all dead").
#[derive(Debug, Clone, PartialEq)]
struct ProgressState {
    latch: Latch,
    at: f32,
}

/// Active progress subscriptions for the current level.
///
/// Membership rule: a tag's members are the map-placed entities carrying it at
/// level install — never the live registry. Install snapshots every tag on every
/// map-placed entity once every map placement has materialized (after the
/// data-archetype and spawner sweeps, before `levelLoad`), so a tag a recompose
/// subscribes for the first time (or drops and later re-adds) resolves against
/// that same install-time set, and an NPC a spawner releases later — which
/// carries its spawner's `spawned_tags` — never joins any tag's set. Every
/// credited kill of an install-time entity is recorded whether or not a
/// subscription watched it then, so a late-subscribed tag counts members already
/// dead as already killed. A member removed without kill credit (a script
/// despawn above zero HP) leaves every set it was in.
///
/// An entity carrying several subscribed tags counts toward each independently;
/// a tag repeated on one entity counts once.
pub struct ProgressTracker {
    /// Map-placed entities with at least one tag at install → their distinct
    /// tags. Ids are generation-checked: a later spawn reusing a freed slot is
    /// not here.
    install_tags: HashMap<EntityId, Vec<String>>,
    /// Install-time entities whose kill has been credited this level. A repeated
    /// report for one entity never counts twice.
    killed: HashSet<EntityId>,
    /// Per subscribed tag, derived from `install_tags` and `killed`.
    memberships: HashMap<String, TagMembership>,
    /// Data-registry order, so one kill fires its targets in a fixed order.
    subscriptions: Vec<ProgressState>,
    /// Every `(tag, fire)` pair that fired this level. One-shot: survives
    /// recompose, including a subscription dropped and later re-added.
    fired: HashSet<Latch>,
    /// Whether this level's install captured membership. A recompose before (or
    /// without) that capture subscribes nothing: a connected client never
    /// captures, since `progress` is host-authoritative.
    captured: bool,
}

impl ProgressTracker {
    pub fn new() -> Self {
        Self {
            install_tags: HashMap::new(),
            killed: HashSet::new(),
            memberships: HashMap::new(),
            subscriptions: Vec::new(),
            fired: HashSet::new(),
            captured: false,
        }
    }

    /// Level install, once every map placement exists: drop any previous level's
    /// state, snapshot the install-time tag index from `entity_registry`, and
    /// subscribe. Warns once per subscribed tag that captures no members, since
    /// such a `progress` can never fire, and once per subscription over a
    /// non-empty set whose `at` is not in `(0, 1]`. Returns event names whose
    /// threshold the set already meets (`at: 0`), latched exactly as a
    /// recompose would; the caller dispatches them like kill-driven fires.
    pub fn initialize(
        &mut self,
        data_registry: &DataRegistry,
        entity_registry: &EntityRegistry,
    ) -> Vec<String> {
        self.clear();
        self.install_tags = snapshot_install_tags(entity_registry);
        self.captured = true;
        self.subscribe(data_registry);
        self.warn_install_thresholds();
        self.evaluate()
    }

    /// Recompose (mod hot reload): rebuild subscriptions from the recomposed
    /// reaction set against the install-time membership and kill record — the
    /// live registry is never read, so entities spawned since install never join.
    /// Fired latches survive. Returns event names whose threshold the recomposed
    /// set already meets (e.g. a lowered `at`), each `(tag, fire)` pair firing at
    /// most once per level; the caller dispatches them like kill-driven fires.
    pub fn recompose(&mut self, data_registry: &DataRegistry) -> Vec<String> {
        if !self.captured {
            return Vec::new();
        }
        let subscribed: HashSet<&str> = data_registry
            .reactions
            .iter()
            .filter_map(|named| match &named.descriptor {
                ReactionDescriptor::Progress(p) => Some(p.tag.as_str()),
                _ => None,
            })
            .collect();
        // Kept tags stay cached so a zero-member tag warns only when first
        // subscribed; a dropped tag rebuilds identically from the install record.
        self.memberships
            .retain(|tag, _| subscribed.contains(tag.as_str()));
        self.subscribe(data_registry);
        self.evaluate()
    }

    fn subscribe(&mut self, data_registry: &DataRegistry) {
        self.subscriptions.clear();
        for named in &data_registry.reactions {
            let ReactionDescriptor::Progress(p) = &named.descriptor else {
                continue;
            };
            if !self.memberships.contains_key(&p.tag) {
                let membership = self.membership_for(&p.tag);
                if membership.total == 0 {
                    log::warn!(
                        "[Scripting] progress on tag `{}` has no members: no map-placed entity \
                         carried the tag at level install, so it can never fire. NPCs a spawner \
                         releases later (its `spawned_tags`) never count toward a progress",
                        p.tag
                    );
                }
                self.memberships.insert(p.tag.clone(), membership);
            }
            self.subscriptions.push(ProgressState {
                latch: Latch {
                    tag: p.tag.clone(),
                    fire: p.fire.clone(),
                },
                at: p.at,
            });
        }
    }

    fn membership_for(&self, tag: &str) -> TagMembership {
        let mut membership = TagMembership {
            total: 0,
            killed: 0,
        };
        for (id, tags) in &self.install_tags {
            if tags.iter().any(|t| t == tag) {
                membership.total += 1;
                if self.killed.contains(id) {
                    membership.killed += 1;
                }
            }
        }
        membership
    }

    /// Credit the kill of `entity`. Returns event names to fire; caller passes
    /// each name to [`super::fire_named_event_with_sequences`].
    pub fn on_entity_killed(&mut self, entity: EntityId) -> Vec<String> {
        let Some(tags) = self.install_tags.get(&entity) else {
            return Vec::new();
        };
        if !self.killed.insert(entity) {
            return Vec::new();
        }
        for tag in tags {
            if let Some(membership) = self.memberships.get_mut(tag) {
                membership.killed = membership.killed.saturating_add(1);
            }
        }
        self.evaluate()
    }

    /// `entity` left the world without kill credit (a despawn above zero HP).
    /// An install-time member drops out of every set it was in, so the rest can
    /// still meet `at: 1.0`; the threshold is re-evaluated against the smaller
    /// total. Returns event names to fire, like [`Self::on_entity_killed`].
    pub fn on_entity_removed(&mut self, entity: EntityId) -> Vec<String> {
        if self.killed.contains(&entity) {
            return Vec::new();
        }
        let Some(tags) = self.install_tags.remove(&entity) else {
            return Vec::new();
        };
        for tag in &tags {
            if let Some(membership) = self.memberships.get_mut(tag) {
                membership.total = membership.total.saturating_sub(1);
            }
        }
        self.evaluate()
    }

    /// Fire every unlatched subscription whose threshold is met, latching it.
    fn evaluate(&mut self) -> Vec<String> {
        let mut to_fire = Vec::new();
        for state in &self.subscriptions {
            if self.fired.contains(&state.latch) {
                continue;
            }
            let Some(membership) = self.memberships.get(&state.latch.tag) else {
                continue;
            };
            if membership.total == 0 {
                continue;
            }
            let ratio = membership.killed as f32 / membership.total as f32;
            if state.at.is_finite() && ratio >= state.at {
                self.fired.insert(state.latch.clone());
                to_fire.push(state.latch.fire.clone());
            }
        }
        to_fire
    }

    /// Parsing rejects `at` outside `[0, 1]` (NaN and infinities included), so
    /// authored content reaches here in range, where `at: 0` fires at install;
    /// install warns once for it. Hand-built descriptors skip parsing, so the
    /// other branches are defensive: a finite `at` below zero fires at install,
    /// and one above one or non-finite never fires. A tag with no members has
    /// already warned that it can never fire, so it gets no threshold warning.
    fn warn_install_thresholds(&self) {
        for state in &self.subscriptions {
            let has_members = self
                .memberships
                .get(&state.latch.tag)
                .is_some_and(|membership| membership.total > 0);
            if !has_members || (state.at.is_finite() && state.at > 0.0 && state.at <= 1.0) {
                continue;
            }
            let consequence = if state.at.is_finite() && state.at <= 0.0 {
                "it fires as soon as the level loads"
            } else {
                "it can never fire"
            };
            log::warn!(
                "[Scripting] progress on tag `{}` firing `{}` has `at` {}: {consequence}",
                state.latch.tag,
                state.latch.fire,
                state.at
            );
        }
    }

    pub fn clear(&mut self) {
        self.install_tags.clear();
        self.killed.clear();
        self.memberships.clear();
        self.subscriptions.clear();
        self.fired.clear();
        self.captured = false;
    }

    #[cfg(test)]
    pub(super) fn subscription_count(&self, tag: &str) -> usize {
        self.subscriptions
            .iter()
            .filter(|state| state.latch.tag == tag)
            .count()
    }
}

impl Default for ProgressTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Every distinct tag on every map-placed entity in `entity_registry`, by entity.
fn snapshot_install_tags(entity_registry: &EntityRegistry) -> HashMap<EntityId, Vec<String>> {
    use crate::registry::ComponentKind;

    // INVARIANT: every spawned entity carries a Transform component — `EntityRegistry::spawn`
    // writes it unconditionally. Walking only the Transform column visits each entity once.
    entity_registry
        .iter_with_kind(ComponentKind::Transform)
        .filter(|(id, _)| is_map_placed(entity_registry, *id))
        .filter_map(|(id, _)| {
            let tags = entity_registry.get_tags(id).ok()?;
            // `_tags "wave1 wave1"` names one membership, not two: a duplicate
            // would credit one kill twice.
            let mut distinct: Vec<String> = Vec::with_capacity(tags.len());
            for tag in tags {
                if !distinct.contains(tag) {
                    distinct.push(tag.clone());
                }
            }
            (!distinct.is_empty()).then_some((id, distinct))
        })
        .collect()
}

/// Map placement, or no descriptor provenance at all (built-in map kinds such
/// as trigger volumes carry none). A runtime spawn or a player pawn is never a
/// progress member. Mirrors `is_map_placed` behind `worldQuery` in
/// `crates/sim/src/scripting/entity_world_primitives.rs`.
fn is_map_placed(entity_registry: &EntityRegistry, id: EntityId) -> bool {
    entity_registry
        .get_component::<DescriptorProvenance>(id)
        .map_or(true, |provenance| {
            provenance.spawn_path == DescriptorSpawnPath::MapPlacement
        })
}
