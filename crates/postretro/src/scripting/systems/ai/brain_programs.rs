// Evaluator-owned, bind-time-flattened behavior-statechart programs.
//
// The retained descriptor is recursive; this side table is not. Every
// envelope receives one stable vector index during `sync`, so the tick walks
// only existing slices and fixed brain-path entries.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry};
use postretro_foundation::{
    BakedIr, BehaviorGraphDescriptor, BehaviorGraphEnvelope, BehaviorLayerDescriptor,
    BehaviorSelectorEntry, BoundProgram, CURRENT_IR_VERSION, GuardedRow, IrType, bind,
};

use super::brain_scope::BrainScope;
use super::candidate_scope::CandidateScope;

/// Guards for one descriptor envelope. `wildcard` and every activity's `rows`
/// remain in their authored declaration order. `None` is a disabled edge.
pub(crate) struct BoundEnvelope {
    pub(crate) wildcard: Vec<Option<BoundProgram<BrainScope>>>,
    pub(crate) activities: Vec<BoundActivity>,
}

pub(crate) struct BoundActivity {
    pub(crate) rows: Vec<Option<BoundProgram<BrainScope>>>,
    /// Parallel to the descriptor activity's `layers` BTreeMap. Selector guards
    /// bind here for the AI-owned `move`/`offense` names; other selectors keep
    /// `None` slots for alignment. A nested graph stores its flattened
    /// child-envelope index.
    pub(crate) layers: Vec<BoundLayer>,
}

pub(crate) enum BoundLayer {
    Selector(Vec<Option<BoundProgram<BrainScope>>>),
    Graph(usize),
}

pub(crate) struct BrainEntityPrograms {
    graph: Arc<BehaviorGraphDescriptor>,
    envelopes: Vec<BoundEnvelope>,
    candidate_filter: Option<BoundProgram<CandidateScope>>,
}

impl BrainEntityPrograms {
    pub(crate) fn envelope(&self, index: usize) -> Option<&BoundEnvelope> {
        self.envelopes.get(index)
    }

    pub(crate) fn candidate_filter(&self) -> Option<&BoundProgram<CandidateScope>> {
        self.candidate_filter.as_ref()
    }
}

/// The evaluator's programs and reusable live binding scopes.
pub(crate) struct BrainPrograms {
    scope: BrainScope,
    candidate_scope: CandidateScope,
    entries: HashMap<EntityId, BrainEntityPrograms>,
    /// Graph replacement always restarts at the new root initial descent. An
    /// old numeric state would smuggle forbidden flat history into statecharts.
    pending_reseats: HashSet<EntityId>,
    live: HashSet<EntityId>,
}

impl BrainPrograms {
    pub(crate) fn new() -> Self {
        Self {
            scope: BrainScope::for_validation(),
            candidate_scope: CandidateScope::for_validation(),
            entries: HashMap::new(),
            pending_reseats: HashSet::new(),
            live: HashSet::new(),
        }
    }

    pub(crate) fn scope_mut(&mut self) -> &mut BrainScope {
        &mut self.scope
    }

    pub(crate) fn get(&self, entity: EntityId) -> Option<&BrainEntityPrograms> {
        self.entries.get(&entity)
    }

    /// Borrow the immutable bound table and the reusable mutable scope at once
    /// without cloning either. Every path evaluator uses this seam so selector
    /// and transition evaluation share the same refreshed facts.
    pub(crate) fn with_entry_scope<R>(
        &mut self,
        entity: EntityId,
        evaluate: impl FnOnce(&BrainEntityPrograms, &mut BrainScope) -> R,
    ) -> Option<R> {
        let (entries, scope) = (&self.entries, &mut self.scope);
        entries.get(&entity).map(|entry| evaluate(entry, scope))
    }

    pub(crate) fn take_reseat(&mut self, entity: EntityId) -> bool {
        self.pending_reseats.remove(&entity)
    }

    pub(crate) fn candidate_filter_context(
        &mut self,
        entity: EntityId,
    ) -> (Option<&BoundProgram<CandidateScope>>, &mut CandidateScope) {
        let (entries, candidate_scope) = (&self.entries, &mut self.candidate_scope);
        let filter = entries
            .get(&entity)
            .and_then(BrainEntityPrograms::candidate_filter);
        (filter, candidate_scope)
    }

    /// Bind changed graphs and release dead entries. All growth happens here,
    /// outside the hot evaluator and its zero-allocation probe window.
    pub(crate) fn sync(&mut self, registry: &EntityRegistry, warned: &mut HashSet<String>) {
        self.live.clear();
        for (entity, value) in registry.iter_with_kind(ComponentKind::Brain) {
            let ComponentValue::Brain(brain) = value else {
                continue;
            };
            self.live.insert(entity);
            let unchanged = self
                .entries
                .get(&entity)
                .is_some_and(|entry| Arc::ptr_eq(&entry.graph, &brain.graph));
            if !unchanged {
                if self.entries.contains_key(&entity) {
                    self.pending_reseats.insert(entity);
                }
                self.entries.insert(
                    entity,
                    bind_graph(
                        &self.scope,
                        &self.candidate_scope,
                        Arc::clone(&brain.graph),
                        warned,
                    ),
                );
            }
        }
        self.entries.retain(|entity, _| self.live.contains(entity));
        self.pending_reseats
            .retain(|entity| self.live.contains(entity));
    }
}

pub(super) fn bind_graph(
    scope: &BrainScope,
    candidate_scope: &CandidateScope,
    graph: Arc<BehaviorGraphDescriptor>,
    warned: &mut HashSet<String>,
) -> BrainEntityPrograms {
    let mut envelopes = Vec::new();
    bind_envelope(scope, &graph.envelope, &mut envelopes, warned);
    let candidate_filter = graph
        .candidate_filter
        .as_ref()
        .and_then(|filter| bind_candidate_filter(scope, candidate_scope, filter, warned));
    BrainEntityPrograms {
        graph,
        envelopes,
        candidate_filter,
    }
}

fn bind_envelope(
    scope: &BrainScope,
    envelope: &BehaviorGraphEnvelope,
    envelopes: &mut Vec<BoundEnvelope>,
    warned: &mut HashSet<String>,
) -> usize {
    let index = envelopes.len();
    envelopes.push(BoundEnvelope {
        wildcard: bind_rows(
            scope,
            envelope
                .transitions
                .get("*")
                .map(Vec::as_slice)
                .unwrap_or_default(),
            warned,
        ),
        activities: Vec::new(),
    });

    let mut activities = Vec::with_capacity(envelope.activities.len());
    for (name, activity) in &envelope.activities {
        let rows = bind_rows(
            scope,
            envelope
                .transitions
                .get(name)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            warned,
        );
        let mut layers = Vec::with_capacity(activity.layers.len());
        for (layer_name, layer) in &activity.layers {
            match layer {
                BehaviorLayerDescriptor::Selector(entries) => {
                    let programs = if matches!(layer_name.as_str(), "move" | "offense") {
                        entries
                            .iter()
                            .map(|entry| match entry {
                                BehaviorSelectorEntry::Row(row) => row
                                    .when
                                    .as_ref()
                                    .and_then(|when| bind_guard(scope, when, warned)),
                                BehaviorSelectorEntry::Motion(_) => None,
                            })
                            .collect()
                    } else {
                        let mut programs = Vec::with_capacity(entries.len());
                        programs.resize_with(entries.len(), || None);
                        programs
                    };
                    layers.push(BoundLayer::Selector(programs));
                }
                BehaviorLayerDescriptor::Graph(child) => {
                    layers.push(BoundLayer::Graph(bind_envelope(
                        scope, child, envelopes, warned,
                    )));
                }
            }
        }
        activities.push(BoundActivity { rows, layers });
    }
    envelopes[index].activities = activities;
    index
}

fn bind_rows(
    scope: &BrainScope,
    rows: &[GuardedRow],
    warned: &mut HashSet<String>,
) -> Vec<Option<BoundProgram<BrainScope>>> {
    rows.iter()
        .map(|row| bind_guard(scope, &row.when, warned))
        .collect()
}

fn bind_guard(
    scope: &BrainScope,
    node: &postretro_foundation::IrNode,
    warned: &mut HashSet<String>,
) -> Option<BoundProgram<BrainScope>> {
    let baked = BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root: node.clone(),
    };
    match bind(&baked, scope) {
        Ok(program) if program.root_type == IrType::Bool => Some(program),
        Ok(program) => {
            let reason = format!("guard root is {:?}, not boolean", program.root_type);
            if warned.insert(format!("brain-guard:{reason}")) {
                log::warn!("[AI] behavior {reason}; the affected edge is disabled");
            }
            None
        }
        Err(error) => {
            let reason = error.to_string();
            if warned.insert(format!("brain-guard:{reason}")) {
                log::warn!(
                    "[AI] behavior guard could not be bound ({reason}); the affected edge is disabled"
                );
            }
            None
        }
    }
}

fn bind_candidate_filter(
    _brain_scope: &BrainScope,
    scope: &CandidateScope,
    filter: &postretro_foundation::IrNode,
    warned: &mut HashSet<String>,
) -> Option<BoundProgram<CandidateScope>> {
    let baked = BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root: filter.clone(),
    };
    match bind(&baked, scope) {
        Ok(program) if program.root_type == IrType::Bool => Some(program),
        Ok(program) => {
            let reason = format!(
                "candidate filter root is {:?}, not boolean",
                program.root_type
            );
            if warned.insert(format!("candidate-filter:{reason}")) {
                log::warn!("[AI] behavior {reason}; candidate filtering is disabled");
            }
            None
        }
        Err(error) => {
            let reason = error.to_string();
            if warned.insert(format!("candidate-filter:{reason}")) {
                log::warn!("[AI] candidate filter could not be bound ({reason}); it is disabled");
            }
            None
        }
    }
}
