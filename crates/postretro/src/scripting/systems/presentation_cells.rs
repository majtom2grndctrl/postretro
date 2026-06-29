// App-side presentation-cell store backing `ui.createLocalState()`.
// A presentation-only map keyed by `(scopeId, cellName)`, seeded from
// a tree's declared `localState` initials when its scope is first composed,
// written by the `CellWrite` system reaction at the game-logic stage, and
// reconciled (cleared) against the composed-scope-id set each frame. Published
// onto the UI read snapshot as `cell_values` so a `{ local }` bind resolves
// against the live cell value without the descriptor (compared by the retained
// reuse gate) ever changing.
//
// This is NEVER the authoritative store (`ui.md` §3/§6): no schema, no
// persistence, no dotted-name namespace, no readonly gating. It holds one engine
// frame of presentation state, addressed by an explicit scope id.
// See: context/lib/ui.md §3/§6 · context/lib/scripting.md §11

use std::collections::{HashMap, HashSet};

use crate::render::ui::descriptor::{AnchoredTree, CellInit, LocalState, Widget};
use crate::render::ui::tree::CellValues;
use postretro_entities::slot_table::SlotValue;

/// The app-side presentation-cell store. Keyed by `(scopeId, cellName)`; values
/// are the same `SlotValue` shapes a `{ local }` bind resolves. Presentation-only
/// and engine-frame-scoped — it is seeded from declared initials, written by the
/// `CellWrite` reaction, and pruned when a scope is no longer composed.
#[derive(Debug, Default)]
pub(crate) struct PresentationCellStore {
    cells: HashMap<(String, String), SlotValue>,
    /// Scope ids that have already been seeded this engine run. Seeding from the
    /// declared initials happens exactly once per scope id: re-composing the same
    /// scope (a structurally-identical retained-diff reuse) must NOT reset a cell a
    /// `.set()` already changed. A scope dropping out of the composed set clears
    /// this so a later re-introduction re-seeds from initials.
    seeded: HashSet<String>,
}

impl PresentationCellStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Write `value` into the cell `(scope, cell)`. The `CellWrite` reaction drains
    /// here at the game-logic stage. A write to a not-yet-seeded scope still lands
    /// (the value survives the same-frame seed because seeding never overwrites an
    /// existing cell — see `seed_scope`).
    pub(crate) fn write(&mut self, scope: String, cell: String, value: SlotValue) {
        self.cells.insert((scope, cell), value);
    }

    pub(crate) fn clear(&mut self) {
        self.cells.clear();
        self.seeded.clear();
    }

    /// Seed a scope's declared cells from `local_state` the FIRST time the scope is
    /// composed. Idempotent per scope id (tracked in `seeded`): a second compose of
    /// the same scope is a no-op, so a `.set()` value is never clobbered by a
    /// re-seed. An individual cell is only seeded when absent, so a write that
    /// landed before the first seed (same-frame ordering) also survives.
    fn seed_scope(&mut self, local_state: &LocalState) {
        if !self.seeded.insert(local_state.scope.clone()) {
            return;
        }
        for (name, init) in &local_state.cells {
            let key = (local_state.scope.clone(), name.clone());
            self.cells
                .entry(key)
                .or_insert_with(|| cell_init_to_slot_value(init));
        }
    }

    /// Reconcile the store against the set of scope ids composed this frame
    /// (gathered at the `main.rs` compose step): seed any newly-composed scope from
    /// its declared initials, and DROP every cell whose scope id is no longer
    /// present. A scope's disappearance clears both its cells and its seeded mark,
    /// so a later re-introduction re-seeds fresh.
    ///
    /// `trees` are the frame's composed descriptor trees (every modal-stack +
    /// always-on layer); the walk collects each `localState` scope declaration.
    pub(crate) fn reconcile(&mut self, trees: &[&AnchoredTree]) {
        let mut declarations: HashMap<String, &LocalState> = HashMap::new();
        for tree in trees {
            // Walks the DESCRIPTOR (`tree.root`), never the visible/taffy tree, so a
            // `visibleWhen` flip (M13 G2, Task 2b — applied only in the layout/draw/
            // focus walks) never drops a hidden subtree's `localState` cells.
            collect_local_states(&tree.root, &mut declarations);
        }

        // Seed newly-composed scopes (idempotent per scope id).
        // Sort for deterministic seed-warn ordering and stable behavior.
        let mut scopes: Vec<&String> = declarations.keys().collect();
        scopes.sort();
        for scope in scopes {
            self.seed_scope(declarations[scope]);
        }

        // Drop cells whose declaring scope is no longer composed.
        let present: HashSet<&str> = declarations.keys().map(String::as_str).collect();
        self.cells
            .retain(|(scope, _), _| present.contains(scope.as_str()));
        self.seeded.retain(|scope| present.contains(scope.as_str()));
    }

    /// Snapshot the store for the frame's read snapshot. A plain clone of the
    /// `(scope, cell) -> value` map, the way `slot_values` is cloned out of the
    /// live slot table — so the renderer reads cells without borrowing the store.
    pub(crate) fn snapshot(&self) -> CellValues {
        self.cells.clone()
    }
}

/// Coerce a `cellWrite` reaction's raw JSON value into a presentation-cell
/// `SlotValue`. Numbers/booleans/strings map directly; a
/// length-4 numeric array maps to the panel-fill `Array` shape. Any other shape
/// (object, ragged array, null) is rejected — the drain skips the write with a
/// warn rather than storing an unusable value. NEVER touches the slot table.
pub(crate) fn json_to_cell_value(value: &serde_json::Value) -> Option<SlotValue> {
    match value {
        serde_json::Value::Number(n) => n.as_f64().map(|f| SlotValue::Number(f as f32)),
        serde_json::Value::Bool(b) => Some(SlotValue::Boolean(*b)),
        serde_json::Value::String(s) => Some(SlotValue::String(s.clone())),
        serde_json::Value::Array(items) => {
            let nums: Option<Vec<f32>> =
                items.iter().map(|v| v.as_f64().map(|f| f as f32)).collect();
            nums.map(SlotValue::Array)
        }
        _ => None,
    }
}

/// Convert a declared `CellInit` to the runtime `SlotValue` a bind resolves. The
/// shapes line up one-to-one: number/boolean/string/length-4 array.
fn cell_init_to_slot_value(init: &CellInit) -> SlotValue {
    match init {
        // `SlotValue::Number` is `f32`; the declared initial parses as the JSON
        // natural `f64` and narrows here (cells carry presentation values, not
        // precision-critical store state).
        CellInit::Number(n) => SlotValue::Number(*n as f32),
        CellInit::Boolean(b) => SlotValue::Boolean(*b),
        CellInit::String(s) => SlotValue::String(s.clone()),
        CellInit::Array(a) => SlotValue::Array(a.to_vec()),
    }
}

/// Depth-first collect every container's `localState` declaration under `widget`,
/// keyed by scope id. A duplicate scope id across the composed trees keeps the
/// first seen (deterministic via the caller's iteration order); duplicate scope
/// ids are an authoring concern, not an engine error.
fn collect_local_states<'a>(widget: &'a Widget, out: &mut HashMap<String, &'a LocalState>) {
    if let Some(local_state) = widget_local_state(widget) {
        out.entry(local_state.scope.clone()).or_insert(local_state);
    }
    if let Some(children) = widget_children(widget) {
        for child in children {
            collect_local_states(child, out);
        }
    }
}

/// The `localState` declaration on a container widget, if any. Only stack
/// containers carry `localState` (the field lives on `ContainerWidget`); every
/// other kind returns `None`.
fn widget_local_state(widget: &Widget) -> Option<&LocalState> {
    match widget {
        Widget::VStack(c) | Widget::HStack(c) => c.local_state.as_ref(),
        _ => None,
    }
}

/// The child widgets of a container kind, for the recursive scope walk.
fn widget_children(widget: &Widget) -> Option<&[Widget]> {
    match widget {
        Widget::VStack(c) | Widget::HStack(c) => Some(&c.children),
        Widget::Grid(g) => Some(&g.children),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::ui::descriptor::{
        Align, AnchoredTree, BindSource, ColorValue, ContainerWidget, SpacingValue, TextBind,
        TextWidget,
    };
    use crate::render::ui::layout::Anchor;
    use std::collections::BTreeMap;

    fn cells(pairs: &[(&str, CellInit)]) -> BTreeMap<String, CellInit> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    /// A vstack declaring a `localState` scope wrapping one bound text child.
    fn scoped_tree(scope: &str, init: &[(&str, CellInit)]) -> AnchoredTree {
        AnchoredTree::passthrough(
            Anchor::Center,
            [0.0, 0.0],
            Widget::VStack(ContainerWidget {
                gap: SpacingValue::Literal(0.0),
                padding: SpacingValue::Literal(0.0),
                align: Align::Start,
                fill: None,
                border: None,
                id: None,
                focus_neighbors: Default::default(),
                focus: None,
                restore_on_return: false,
                local_state: Some(LocalState {
                    scope: scope.to_string(),
                    cells: cells(init),
                }),
                visible_when: None,
                role: None,
                children: vec![Widget::Text(TextWidget {
                    content: "fallback".into(),
                    font_size: 12.0,
                    color: ColorValue::Literal([1.0; 4]),
                    font: None,
                    bind: Some(TextBind {
                        source: BindSource::Local {
                            local: "count".into(),
                        },
                        format: None,
                        tween: None,
                    }),
                    style_ranges: None,
                    id: None,
                    focus_neighbors: Default::default(),
                    visible_when: None,
                    role: None,
                })],
            }),
        )
    }

    #[test]
    fn reconcile_seeds_declared_initials_on_first_compose() {
        let mut store = PresentationCellStore::new();
        let tree = scoped_tree("counter", &[("count", CellInit::Number(7.0))]);
        store.reconcile(&[&tree]);
        let snap = store.snapshot();
        assert_eq!(
            snap.get(&("counter".to_string(), "count".to_string())),
            Some(&SlotValue::Number(7.0)),
        );
    }

    #[test]
    fn write_updates_cell_and_survives_reseed_on_recompose() {
        let mut store = PresentationCellStore::new();
        let tree = scoped_tree("counter", &[("count", CellInit::Number(0.0))]);
        store.reconcile(&[&tree]);
        store.write("counter".into(), "count".into(), SlotValue::Number(42.0));
        // Recomposing the same scope (a structurally-identical reuse) must NOT
        // reset the written cell back to its declared initial.
        store.reconcile(&[&tree]);
        assert_eq!(
            store
                .snapshot()
                .get(&("counter".to_string(), "count".to_string())),
            Some(&SlotValue::Number(42.0)),
        );
    }

    #[test]
    fn cell_is_discarded_when_scope_no_longer_composed() {
        let mut store = PresentationCellStore::new();
        let tree = scoped_tree("counter", &[("count", CellInit::Number(3.0))]);
        store.reconcile(&[&tree]);
        store.write("counter".into(), "count".into(), SlotValue::Number(9.0));
        // The scope drops out of the composed set: its cells clear.
        store.reconcile(&[]);
        assert!(store.snapshot().is_empty());
        // Re-introducing the scope re-seeds from initials (not the stale write).
        store.reconcile(&[&tree]);
        assert_eq!(
            store
                .snapshot()
                .get(&("counter".to_string(), "count".to_string())),
            Some(&SlotValue::Number(3.0)),
        );
    }

    #[test]
    fn cell_survives_hide_show_because_reconcile_walks_the_descriptor() {
        // M13 G2 Task 2b: a `visibleWhen` flip hides a subtree via `Display::None`
        // in the render walks ONLY — the descriptor stays composed, so `reconcile`
        // (which walks `tree.root`, the descriptor, NOT the visible/taffy tree)
        // keeps the scope present and never tears down its `localState` cells. A
        // value written before a hide survives the hidden frames and is intact when
        // the subtree shows again.
        let mut store = PresentationCellStore::new();
        let tree = scoped_tree("panel", &[("count", CellInit::Number(0.0))]);
        store.reconcile(&[&tree]);
        store.write("panel".into(), "count".into(), SlotValue::Number(99.0));

        // "Hidden" frames: the descriptor is still composed (visibility is a render
        // concern the store never sees), so reconcile keeps the cell intact.
        store.reconcile(&[&tree]);
        store.reconcile(&[&tree]);

        // "Shown" again: the round-tripped value is unchanged (no re-seed to 0.0).
        store.reconcile(&[&tree]);
        assert_eq!(
            store
                .snapshot()
                .get(&("panel".to_string(), "count".to_string())),
            Some(&SlotValue::Number(99.0)),
            "a hidden subtree's localState cell survives hide/show",
        );
    }

    #[test]
    fn write_before_first_seed_survives_same_frame_seed() {
        // A `.set()` that lands before the scope's first compose/seed must not be
        // clobbered by the seed (seeding only fills absent cells).
        let mut store = PresentationCellStore::new();
        store.write("counter".into(), "count".into(), SlotValue::Number(5.0));
        let tree = scoped_tree("counter", &[("count", CellInit::Number(0.0))]);
        store.reconcile(&[&tree]);
        assert_eq!(
            store
                .snapshot()
                .get(&("counter".to_string(), "count".to_string())),
            Some(&SlotValue::Number(5.0)),
        );
    }

    #[test]
    fn clear_drops_cells_and_seed_marks_for_level_unload() {
        let mut store = PresentationCellStore::new();
        let tree = scoped_tree("counter", &[("count", CellInit::Number(0.0))]);
        store.reconcile(&[&tree]);
        store.write("counter".into(), "count".into(), SlotValue::Number(8.0));

        store.clear();

        assert!(store.snapshot().is_empty());
        store.reconcile(&[&tree]);
        assert_eq!(
            store
                .snapshot()
                .get(&("counter".to_string(), "count".to_string())),
            Some(&SlotValue::Number(0.0)),
        );
    }
}
