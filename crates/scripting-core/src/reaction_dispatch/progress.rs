// Kill-progress subscriptions: per-tag thresholds captured at level load.
// See: context/lib/scripting.md §12 (Reaction Dispatch Model)

use std::collections::HashMap;

use crate::data_descriptors::ReactionDescriptor;
use crate::data_registry::DataRegistry;
use crate::registry::EntityRegistry;

/// `total` is captured at level load; subsequent spawns do NOT raise it.
/// Threshold compare: `killed/total >= at` (`at: 1.0` means "all dead").
#[derive(Debug, Clone, PartialEq)]
struct ProgressState {
    total: u32,
    killed: u32,
    at: f32,
    fire: String,
    /// One-shot guard: fires exactly once even if more entities die after the threshold is crossed.
    fired: bool,
}

/// Active progress subscriptions for the current level, keyed by spawn tag.
/// An entity tagged with multiple values decrements each bucket independently when it dies.
pub struct ProgressTracker {
    subscriptions: HashMap<String, Vec<ProgressState>>,
}

impl ProgressTracker {
    pub fn new() -> Self {
        Self {
            subscriptions: HashMap::new(),
        }
    }

    /// Callers should `clear()` first to avoid duplicate subscriptions.
    pub fn initialize(&mut self, data_registry: &DataRegistry, entity_registry: &EntityRegistry) {
        for named in &data_registry.reactions {
            if let ReactionDescriptor::Progress(p) = &named.descriptor {
                let total = count_entities_with_tag(entity_registry, &p.tag);
                let bucket = self.subscriptions.entry(p.tag.clone()).or_default();
                bucket.push(ProgressState {
                    total,
                    killed: 0,
                    at: p.at,
                    fire: p.fire.clone(),
                    fired: false,
                });
            }
        }
    }

    /// Returns event names to fire; caller passes each name to [`super::fire_named_event_with_sequences`].
    pub fn on_entity_killed(&mut self, tags: &[String]) -> Vec<String> {
        let mut to_fire = Vec::new();
        for tag in tags {
            let Some(subs) = self.subscriptions.get_mut(tag) else {
                continue;
            };
            for state in subs.iter_mut() {
                if state.fired || state.total == 0 {
                    continue;
                }
                state.killed = state.killed.saturating_add(1);
                let ratio = state.killed as f32 / state.total as f32;
                if ratio >= state.at {
                    state.fired = true;
                    to_fire.push(state.fire.clone());
                }
            }
        }
        to_fire
    }

    pub fn clear(&mut self) {
        self.subscriptions.clear();
    }

    #[cfg(test)]
    pub(super) fn subscription_count(&self, tag: &str) -> usize {
        self.subscriptions.get(tag).map(|v| v.len()).unwrap_or(0)
    }
}

impl Default for ProgressTracker {
    fn default() -> Self {
        Self::new()
    }
}

fn count_entities_with_tag(entity_registry: &EntityRegistry, tag: &str) -> u32 {
    use crate::registry::ComponentKind;

    // INVARIANT: every spawned entity carries a Transform component — `EntityRegistry::spawn`
    // writes it unconditionally. A spawn path that skips Transform causes silent underreporting
    // here, which corrupts progress-tracker thresholds. Walking only the Transform column also
    // avoids double-counting entities that carry multiple components.
    entity_registry
        .query_by_component_and_tag(ComponentKind::Transform, Some(tag))
        .count() as u32
}
