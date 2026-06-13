// App-side UI focus engine: consumes queued nav intents + tracked cursor, moves
// focus through containers by policy, resolves pointer hits, runs the dt-clocked
// hold-to-repeat timer, and reports the focused node id back for the focus ring.
// Pure CPU, no GPU, no taffy — it operates on the renderer's exported focus rect
// list (the reverse twin of the app→renderer snapshot).
// See: context/lib/ui.md §4 · context/lib/input.md §7

//! The focus engine runs in the input/game-logic stage (frame-order rule: it
//! consumes intents, moves focus, and the focused id rides the next snapshot — the
//! renderer only displays). State is keyed per stack tree, so a frozen lower tree
//! keeps its focus while the top tree navigates; only the TOP tree takes focus and
//! activation.
//!
//! It consumes the [`FocusRectList`](crate::render::ui::tree::FocusRectList) the
//! renderer published the PREVIOUS frame (the N→N+1 contract applied in reverse),
//! so the focus ring may trail a focus change by one frame — the same latency every
//! UI event carries.

use crate::input::ui_dispatch::PointerPos;
use crate::input::ui_nav::NavIntent;
use crate::render::ui::tree::{FocusKind, FocusRect, FocusRectList, NodeInteraction};

/// Resolve a captured-nav value step for a focused `slider` (M13 Goal F, Task 4).
/// Partitions `nav_intents` into captured — removed in place — and uncaptured —
/// retained for the focus engine. Each captured directional intent steps the value
/// by the slider's `step`: `nav.right`/`nav.up` increase, `nav.left`/`nav.down`
/// decrease; any other captured name is swallowed without moving the value. Returns
/// `Some(next)` — the value stepped from `current` and clamped to `[min, max]` —
/// when a net step occurred, else `None`. A non-`Slider` interaction returns `None`
/// and leaves `nav_intents` untouched. The app emits a `setState { slot, next }`
/// for a `Some` result, applied at the game-logic command drain (the bound slot
/// changes on the N+1 frame — the system's defining N→N+1 ordering).
pub fn capture_slider_step(
    interaction: &NodeInteraction,
    current: f32,
    nav_intents: &mut Vec<NavIntent>,
) -> Option<f32> {
    let NodeInteraction::Slider {
        min,
        max,
        step,
        captures_nav,
        ..
    } = interaction
    else {
        return None;
    };
    if captures_nav.is_empty() {
        return None;
    }

    let mut retained = Vec::with_capacity(nav_intents.len());
    let mut delta_steps = 0i32;
    for &nav in nav_intents.iter() {
        if captures_nav.iter().any(|name| name == nav.wire_name()) {
            match nav {
                NavIntent::Right | NavIntent::Up => delta_steps += 1,
                NavIntent::Left | NavIntent::Down => delta_steps -= 1,
                _ => {}
            }
        } else {
            retained.push(nav);
        }
    }
    *nav_intents = retained;

    if delta_steps == 0 {
        return None;
    }
    Some((current + step * delta_steps as f32).clamp(*min, *max))
}

/// Pointer-vs-focus interaction mode, taken as an input (the `input.mode` slot
/// write is Task 5's concern). In `Pointer` mode, cursor motion moves focus
/// (hover-focus); in `Focus` mode, the cursor never moves focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    /// Pointer drives focus: hover moves focus, clicks hit-test by topmost z.
    Pointer,
    /// Directional nav drives focus: hover is ignored.
    #[default]
    Focus,
}

/// A directional nav step the focus engine resolves against the rect list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    /// Map a directional `NavIntent` to a `Dir`, or `None` for non-directional
    /// intents (confirm/cancel/menu/options/next/prev — handled separately).
    fn from_nav(nav: NavIntent) -> Option<Dir> {
        match nav {
            NavIntent::Up => Some(Dir::Up),
            NavIntent::Down => Some(Dir::Down),
            NavIntent::Left => Some(Dir::Left),
            NavIntent::Right => Some(Dir::Right),
            _ => None,
        }
    }
}

/// Hold-to-repeat clock for a held directional nav. Confirm/cancel never repeat,
/// so only a directional press starts one. The timer is dt-clocked: each engine
/// tick adds `dt` to `elapsed`, and once the elapsed time passes the initial delay
/// (then each interval after) it yields one repeated step. Releasing the direction
/// (a return to no held direction) clears it.
#[derive(Debug, Clone)]
struct RepeatClock {
    dir: Dir,
    initial_delay: f32,
    interval: f32,
    /// Seconds since the press (or since the last emitted repeat after the delay).
    elapsed: f32,
    /// Whether the initial delay has elapsed (we are in the steady interval phase).
    repeating: bool,
}

/// One stack tree's focus state, keyed by tree identity (the stack-position key
/// the engine is driven with). Holds the focused node id and whether this tree has
/// been initialized (so re-activation can choose restore-vs-initial).
#[derive(Debug, Clone, Default)]
struct TreeFocus {
    /// The focused node id, or `None` before initialization / when the tree has no
    /// focusable nodes.
    focused: Option<String>,
    /// True once this tree has selected an initial focus at least once.
    initialized: bool,
}

/// The app-side focus engine. One per app; tracks per-tree focus state and the
/// active (top) tree's repeat clock. Driven each game-logic phase with the top
/// tree's key + the focus rect list the renderer published last frame.
#[derive(Debug, Default)]
pub struct UiFocusEngine {
    /// Per-tree focus state keyed by the stable per-tree key (the modal stack uses
    /// the registry name; the HUD uses `"hud"`). A frozen lower tree keeps its
    /// entry so a returning pop can restore it.
    trees: std::collections::HashMap<String, TreeFocus>,
    /// The key of the tree that was active (top) last engine tick, to detect a
    /// stack change (push/pop) and run restore/initial selection.
    active_key: Option<String>,
    /// Hold-to-repeat clock for the active tree's currently held direction.
    repeat: Option<RepeatClock>,
}

/// Result of one focus-engine tick: the focused node id to send back on the next
/// snapshot (drives the focus ring) and any confirm/cancel activation intent that
/// fired this tick (Task 4 wires button activation onto `confirm`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FocusTickResult {
    /// The id of the focused node in the active tree, or `None` when nothing is
    /// focusable. Rides the next `UiReadSnapshot` so the UI pass draws the ring.
    pub focused: Option<String>,
    /// True when a `confirm` (activate) intent landed on the focused node this
    /// tick. Task 4 fires the focused widget's reaction on this.
    pub confirmed: bool,
    /// True when a `cancel` intent fired this tick. Wired to back-out by the app.
    pub cancelled: bool,
}

impl UiFocusEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drive one focus-engine tick for the active (top) tree.
    ///
    /// - `active_key`: the stable per-tree key of the top stack tree, or `None`
    ///   when no gameplay UI tree is active (the engine then focuses nothing).
    /// - `rects`: the focus rect list the renderer exported for the active tree
    ///   LAST frame (reverse N→N+1). `None` when none was published.
    /// - `intents`: the directional/confirm/cancel nav intents delivered this
    ///   frame (already filtered to the active tree by the capture seam).
    /// - `cursor`: the tracked cursor position (device px), or `None`.
    /// - `clicks`: pointer-click positions delivered this frame (topmost-z hit).
    /// - `mode`: pointer-vs-focus interaction mode (Task 5 owns writing it).
    /// - `dt`: seconds since the last tick, for the hold-to-repeat clock.
    // Wide by necessity: active key + rect list + intents + cursor + clicks + mode
    // + dt are all distinct per-tick inputs from different subsystems; bundling
    // them into a struct would only obscure the single call site in `main.rs`.
    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        active_key: Option<&str>,
        rects: Option<&FocusRectList>,
        intents: &[NavIntent],
        cursor: Option<PointerPos>,
        clicks: &[PointerPos],
        mode: InputMode,
        dt: f32,
    ) -> FocusTickResult {
        let Some(active_key) = active_key else {
            // No active tree: clear the active marker and the repeat clock, focus
            // nothing. Lower trees' saved focus is retained for a future return.
            self.active_key = None;
            self.repeat = None;
            return FocusTickResult::default();
        };
        let Some(rects) = rects else {
            // The active tree published no rect list yet (first frame after a push,
            // before the renderer exported one). Focus nothing this tick.
            self.active_key = Some(active_key.to_string());
            return FocusTickResult::default();
        };

        // Stack change detection: the active tree differs from last tick (a push or
        // a returning pop). Drop the repeat clock (a new tree must re-arm) and
        // ensure the now-active tree's focus is selected: restore its saved focus
        // when it opted into `restoreOnReturn`, else (re)select its initial focus.
        let stack_changed = self.active_key.as_deref() != Some(active_key);
        if stack_changed {
            self.repeat = None;
        }
        self.ensure_initialized(active_key, rects, stack_changed);
        self.active_key = Some(active_key.to_string());

        // Drop a focused id that no longer exists in the current rect list (a
        // structural rebuild removed the node); fall back to the initial focus.
        if let Some(focused) = self.focused_id(active_key) {
            if !rects.rects.iter().any(|r| r.id == focused) {
                let initial = initial_focus_id(rects);
                self.set_focused(active_key, initial);
            }
        }

        let mut result = FocusTickResult::default();

        // Pointer mode: hover moves focus, then a click hit-tests by topmost z.
        // Hover never enqueues an intent (it is tracked cursor state), so it is
        // applied here directly from the tracked position.
        if mode == InputMode::Pointer {
            if let Some(cursor) = cursor {
                if let Some(hit) = hit_test_topmost(rects, cursor) {
                    self.set_focused(active_key, Some(hit.to_string()));
                }
            }
        }
        for click in clicks {
            if let Some(hit) = hit_test_topmost(rects, *click) {
                self.set_focused(active_key, Some(hit.to_string()));
                // A click also activates the hit node (confirm-equivalent).
                result.confirmed = true;
            }
        }

        // Directional / activation nav. A directional press moves focus and arms
        // the hold-to-repeat clock; confirm/cancel fire once and never repeat.
        let mut held_dir: Option<Dir> = None;
        for &nav in intents {
            match nav {
                NavIntent::Confirm => result.confirmed = true,
                NavIntent::Cancel => result.cancelled = true,
                NavIntent::Next => self.step_linear(active_key, rects, 1),
                NavIntent::Prev => self.step_linear(active_key, rects, -1),
                other => {
                    if let Some(dir) = Dir::from_nav(other) {
                        self.move_focus(active_key, rects, dir);
                        held_dir = Some(dir);
                    }
                }
            }
        }

        // Hold-to-repeat: a fresh directional press (this tick) arms the clock; an
        // absence of a directional intent this tick is treated as release ONLY when
        // no clock is armed — the clock itself is advanced by `dt` below and a
        // repeat fires a synthetic directional move. The intent stream carries one
        // edge per physical press (held repeats are suppressed at the input edge),
        // so a held direction shows up as: one intent on press, then none while
        // held — exactly the signal the dt clock turns into repeats.
        self.advance_repeat(active_key, rects, held_dir, dt);

        result.focused = self.focused_id(active_key).map(str::to_string);
        result
    }

    /// Ensure the active tree has a selected focus. On a stack change, restore the
    /// saved focus when the tree opted into `restoreOnReturn` (and it still
    /// exists), else select the tree's `initialFocus` (or the first focusable node).
    /// On a non-stack-change tick, initialize only if not yet initialized.
    fn ensure_initialized(&mut self, key: &str, rects: &FocusRectList, stack_changed: bool) {
        let entry = self.trees.entry(key.to_string()).or_default();
        let restore_valid = rects.restore_on_return
            && entry
                .focused
                .as_ref()
                .is_some_and(|f| rects.rects.iter().any(|r| &r.id == f));

        if stack_changed {
            if restore_valid {
                // Keep the saved focus (restoreOnReturn) — nothing to do.
                entry.initialized = true;
            } else {
                let initial = initial_focus_id(rects);
                entry.focused = initial;
                entry.initialized = true;
            }
        } else if !entry.initialized {
            entry.focused = initial_focus_id(rects);
            entry.initialized = true;
        }
    }

    /// The focused node id for `key`, if any.
    fn focused_id(&self, key: &str) -> Option<&str> {
        self.trees.get(key).and_then(|t| t.focused.as_deref())
    }

    /// Set the focused node id for `key`.
    fn set_focused(&mut self, key: &str, id: Option<String>) {
        let entry = self.trees.entry(key.to_string()).or_default();
        entry.focused = id;
        entry.initialized = true;
    }

    /// Move focus one step in `dir` from the current node: a `focusNeighbors`
    /// override wins; else the governing group's policy resolves the target
    /// (`linear` by tree order with optional wrap, `spatial` by nearest center in
    /// the directional half-plane).
    fn move_focus(&mut self, key: &str, rects: &FocusRectList, dir: Dir) {
        let Some(current_id) = self.focused_id(key).map(str::to_string) else {
            // No focus yet: a direction selects the first focusable node.
            self.set_focused(key, initial_focus_id(rects));
            return;
        };
        let Some(current) = rects.rects.iter().find(|r| r.id == current_id) else {
            return;
        };

        // 1) Neighbor override wins.
        if let Some(target) = neighbor_override(current, dir) {
            if rects.rects.iter().any(|r| r.id == target) {
                self.set_focused(key, Some(target.to_string()));
                return;
            }
        }

        // 2) Governing group policy.
        let Some(group_idx) = current.group else {
            return;
        };
        let group = &rects.groups[group_idx];
        let next = match group.kind {
            FocusKind::Linear => linear_step(rects, group_idx, &current_id, dir, group.wrap),
            FocusKind::Spatial => spatial_step(rects, group_idx, current, dir),
        };
        if let Some(next_id) = next {
            self.set_focused(key, Some(next_id));
        }
    }

    /// Next/prev linear step within the current node's group (tree order, wrap per
    /// the group's policy). Ignores neighbor overrides — next/prev is always the
    /// sequential traversal.
    fn step_linear(&mut self, key: &str, rects: &FocusRectList, delta: i32) {
        let Some(current_id) = self.focused_id(key).map(str::to_string) else {
            self.set_focused(key, initial_focus_id(rects));
            return;
        };
        let Some(current) = rects.rects.iter().find(|r| r.id == current_id) else {
            return;
        };
        let Some(group_idx) = current.group else {
            return;
        };
        let group = &rects.groups[group_idx];
        if let Some(next) = linear_index_step(rects, group, &current_id, delta, group.wrap) {
            self.set_focused(key, Some(next));
        }
    }

    /// Advance the hold-to-repeat clock. `pressed_dir` is the direction pressed
    /// THIS tick (a fresh edge), if any. A press (re)arms the clock; with no press
    /// this tick the clock is advanced by `dt` and fires a synthetic directional
    /// move once the initial delay then each interval elapses. Confirm/cancel
    /// never reach here, so they never repeat.
    fn advance_repeat(
        &mut self,
        key: &str,
        rects: &FocusRectList,
        pressed_dir: Option<Dir>,
        dt: f32,
    ) {
        // Determine the repeat cadence for the focused node's group; a group with
        // no repeat policy disables hold-to-repeat entirely.
        let repeat_cfg = self
            .focused_id(key)
            .and_then(|id| rects.rects.iter().find(|r| r.id == id))
            .and_then(|r| r.group)
            .and_then(|g| rects.groups[g].repeat);

        if let Some(dir) = pressed_dir {
            // Fresh press: arm (or re-arm to the new direction) the clock if the
            // group declares a repeat policy.
            match repeat_cfg {
                Some(cfg) => {
                    self.repeat = Some(RepeatClock {
                        dir,
                        initial_delay: cfg.initial_delay_ms / 1000.0,
                        interval: cfg.interval_ms / 1000.0,
                        elapsed: 0.0,
                        repeating: false,
                    });
                }
                None => self.repeat = None,
            }
            return;
        }

        // No press this tick. If a clock is armed, advance it and fire repeats.
        let Some(clock) = self.repeat.as_mut() else {
            return;
        };
        clock.elapsed += dt;
        // Emit as many repeats as the accumulated time covers (robust to a large
        // dt spanning multiple intervals), advancing focus each time.
        let mut fires = 0u32;
        loop {
            let threshold = if clock.repeating {
                clock.interval
            } else {
                clock.initial_delay
            };
            if threshold <= 0.0 || clock.elapsed < threshold {
                break;
            }
            clock.elapsed -= threshold;
            clock.repeating = true;
            fires += 1;
            if fires > 64 {
                // Safety clamp against a pathological dt/interval ratio.
                clock.elapsed = 0.0;
                break;
            }
        }
        let dir = clock.dir;
        for _ in 0..fires {
            self.move_focus(key, rects, dir);
        }
    }

    /// Clear the hold-to-repeat clock — called when the held direction releases
    /// (the app observes the directional key/stick return and drives release).
    pub fn release_repeat(&mut self) {
        self.repeat = None;
    }
}

/// The id a node's `focusNeighbors` names for `dir`, if any.
fn neighbor_override(rect: &FocusRect, dir: Dir) -> Option<&str> {
    match dir {
        Dir::Up => rect.neighbors.up.as_deref(),
        Dir::Down => rect.neighbors.down.as_deref(),
        Dir::Left => rect.neighbors.left.as_deref(),
        Dir::Right => rect.neighbors.right.as_deref(),
    }
}

/// The id focus should start on: the tree's `initialFocus` when it names an
/// existing focusable node, else the first focusable node in tree order.
fn initial_focus_id(rects: &FocusRectList) -> Option<String> {
    if let Some(initial) = &rects.initial_focus {
        if rects.rects.iter().any(|r| &r.id == initial) {
            return Some(initial.clone());
        }
    }
    rects.rects.first().map(|r| r.id.clone())
}

/// Linear traversal: step the current node to the previous/next member of its
/// group in tree order, wrapping per `wrap`. `dir` maps Up/Left → previous,
/// Down/Right → next (a vstack navigates with Up/Down, an hstack with Left/Right;
/// either pair walks the same sequential member order).
fn linear_step(
    rects: &FocusRectList,
    group_idx: usize,
    current_id: &str,
    dir: Dir,
    wrap: bool,
) -> Option<String> {
    let delta = match dir {
        Dir::Up | Dir::Left => -1,
        Dir::Down | Dir::Right => 1,
    };
    linear_index_step(rects, &rects.groups[group_idx], current_id, delta, wrap)
}

/// Shared index walk for linear `move_focus` and next/prev: find `current_id` in
/// the group's member list and step by `delta`, wrapping or clamping per `wrap`.
fn linear_index_step(
    rects: &FocusRectList,
    group: &crate::render::ui::tree::FocusGroup,
    current_id: &str,
    delta: i32,
    wrap: bool,
) -> Option<String> {
    let pos = group
        .members
        .iter()
        .position(|&m| rects.rects[m].id == current_id)?;
    let len = group.members.len() as i32;
    if len == 0 {
        return None;
    }
    let raw = pos as i32 + delta;
    let next = if wrap {
        ((raw % len) + len) % len
    } else if raw < 0 || raw >= len {
        return None;
    } else {
        raw
    };
    Some(rects.rects[group.members[next as usize]].id.clone())
}

/// Spatial traversal: among the group's members lying in `dir`'s half-plane
/// relative to `current`, pick the one whose center is nearest (Euclidean on
/// device-pixel centers, with a perpendicular-offset penalty so a straight-ahead
/// neighbor beats a diagonal). Returns `None` when no member lies that way.
fn spatial_step(
    rects: &FocusRectList,
    group_idx: usize,
    current: &FocusRect,
    dir: Dir,
) -> Option<String> {
    let group = &rects.groups[group_idx];
    let (cx, cy) = center(current.rect);

    let mut best: Option<(f32, &str)> = None;
    for &m in &group.members {
        let cand = &rects.rects[m];
        if cand.id == current.id {
            continue;
        }
        let (tx, ty) = center(cand.rect);
        let dx = tx - cx;
        let dy = ty - cy;
        // Primary axis must move the right way past a small epsilon; the
        // perpendicular offset is penalized so the most aligned neighbor wins.
        let (along, perp) = match dir {
            Dir::Up => (-dy, dx),
            Dir::Down => (dy, dx),
            Dir::Left => (-dx, dy),
            Dir::Right => (dx, dy),
        };
        if along <= 0.5 {
            continue;
        }
        // Weight the perpendicular offset heavily so straight-ahead wins over a
        // diagonal at similar primary distance.
        let cost = along + perp.abs() * 2.0;
        if best.map(|(bc, _)| cost < bc).unwrap_or(true) {
            best = Some((cost, cand.id.as_str()));
        }
    }
    best.map(|(_, id)| id.to_string())
}

/// Center `[cx, cy]` of a device-pixel rect `[x, y, w, h]`.
fn center(rect: [f32; 4]) -> (f32, f32) {
    (rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5)
}

/// Resolve a pointer position to the topmost focusable node under it: among all
/// rects containing the point, the one with the highest z (later in tree order
/// draws on top). `None` when the point is over no focusable node.
fn hit_test_topmost(rects: &FocusRectList, pos: PointerPos) -> Option<&str> {
    let px = pos.x as f32;
    let py = pos.y as f32;
    rects
        .rects
        .iter()
        .filter(|r| contains(r.rect, px, py))
        .max_by_key(|r| r.z)
        .map(|r| r.id.as_str())
}

/// Whether a device-pixel rect `[x, y, w, h]` contains the point `(px, py)`.
fn contains(rect: [f32; 4], px: f32, py: f32) -> bool {
    px >= rect[0] && px < rect[0] + rect[2] && py >= rect[1] && py < rect[1] + rect[3]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::ui::tree::{FocusGroup, FocusNeighbors, FocusRect, RepeatPolicy};

    fn rect(id: &str, r: [f32; 4], z: u32, group: Option<usize>) -> FocusRect {
        FocusRect {
            id: id.to_string(),
            rect: r,
            z,
            group,
            neighbors: FocusNeighbors::default(),
            interaction: None,
        }
    }

    /// A vstack-style linear group of three stacked rects.
    fn linear_list(wrap: bool, repeat: Option<RepeatPolicy>) -> FocusRectList {
        FocusRectList {
            rects: vec![
                rect("a", [0.0, 0.0, 100.0, 20.0], 0, Some(0)),
                rect("b", [0.0, 30.0, 100.0, 20.0], 1, Some(0)),
                rect("c", [0.0, 60.0, 100.0, 20.0], 2, Some(0)),
            ],
            groups: vec![FocusGroup {
                kind: FocusKind::Linear,
                wrap,
                repeat,
                members: vec![0, 1, 2],
            }],
            initial_focus: None,
            restore_on_return: false,
        }
    }

    /// A 2x2 spatial grid: a b / c d.
    fn grid_list() -> FocusRectList {
        FocusRectList {
            rects: vec![
                rect("a", [0.0, 0.0, 40.0, 40.0], 0, Some(0)),
                rect("b", [50.0, 0.0, 40.0, 40.0], 1, Some(0)),
                rect("c", [0.0, 50.0, 40.0, 40.0], 2, Some(0)),
                rect("d", [50.0, 50.0, 40.0, 40.0], 3, Some(0)),
            ],
            groups: vec![FocusGroup {
                kind: FocusKind::Spatial,
                wrap: false,
                repeat: None,
                members: vec![0, 1, 2, 3],
            }],
            initial_focus: None,
            restore_on_return: false,
        }
    }

    // --- M13 Goal F, Task 4: slider nav-capture value step ---

    fn slider_interaction(captures: &[&str]) -> NodeInteraction {
        NodeInteraction::Slider {
            slot: "audio.master".to_string(),
            min: 0.0,
            max: 1.0,
            step: 0.1,
            captures_nav: captures.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn slider_captures_nav_steps_value_and_removes_captured_intents() {
        // nav.right increases by step; the captured intent is removed so the focus
        // engine never sees it (focus stays put). An uncaptured nav.down is retained.
        let interaction = slider_interaction(&["nav.left", "nav.right"]);
        let mut intents = vec![NavIntent::Right, NavIntent::Down];
        let next = capture_slider_step(&interaction, 0.5, &mut intents);
        assert!(close(next.unwrap(), 0.6), "right steps +step from 0.5");
        assert_eq!(intents, vec![NavIntent::Down], "uncaptured intent retained");
    }

    #[test]
    fn slider_step_clamps_within_min_max() {
        // Stepping down at the floor stays at min; stepping up at the ceiling stays
        // at max — the value never escapes [min, max].
        let interaction = slider_interaction(&["nav.left", "nav.right"]);
        let mut down = vec![NavIntent::Left];
        assert!(close(
            capture_slider_step(&interaction, 0.0, &mut down).unwrap(),
            0.0
        ));
        let mut up = vec![NavIntent::Right];
        assert!(close(
            capture_slider_step(&interaction, 1.0, &mut up).unwrap(),
            1.0
        ));
    }

    #[test]
    fn slider_with_no_captured_intents_returns_none_and_leaves_intents() {
        // A nav intent not in capturesNav is left for the focus engine and no step
        // happens (returns None — no setState write).
        let interaction = slider_interaction(&["nav.left", "nav.right"]);
        let mut intents = vec![NavIntent::Down];
        assert_eq!(capture_slider_step(&interaction, 0.5, &mut intents), None);
        assert_eq!(intents, vec![NavIntent::Down]);
    }

    #[test]
    fn confirm_and_click_both_report_activation_on_the_same_focused_node() {
        // A gamepad confirm and a pointer click both surface as `confirmed=true`
        // with the same focused node — the app fires that node's button `onPress`
        // off this single flag, so confirm and click have an identical effect.
        let list = linear_list(false, None);

        // Confirm path: focus a, confirm.
        let mut fe = UiFocusEngine::new();
        fe.tick(
            Some("t"),
            Some(&list),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        let confirm = fe.tick(
            Some("t"),
            Some(&list),
            &[NavIntent::Confirm],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert!(confirm.confirmed);
        assert_eq!(confirm.focused.as_deref(), Some("a"));

        // Click path: a click on a's rect (pointer mode) focuses + activates it.
        let mut fe = UiFocusEngine::new();
        let click = fe.tick(
            Some("t"),
            Some(&list),
            &[],
            None,
            &[PointerPos { x: 10.0, y: 10.0 }],
            InputMode::Pointer,
            0.0,
        );
        assert!(click.confirmed, "a click also activates");
        assert_eq!(
            click.focused.as_deref(),
            confirm.focused.as_deref(),
            "click and confirm activate the same focused node"
        );
    }

    #[test]
    fn button_interaction_never_captures_a_slider_step() {
        // A non-Slider interaction returns None and never touches the intent list.
        let interaction = NodeInteraction::Button {
            on_press: "fire".to_string(),
        };
        let mut intents = vec![NavIntent::Right];
        assert_eq!(capture_slider_step(&interaction, 0.5, &mut intents), None);
        assert_eq!(intents, vec![NavIntent::Right]);
    }

    #[test]
    fn linear_down_moves_through_tree_order() {
        let mut fe = UiFocusEngine::new();
        let list = linear_list(false, None);
        // First tick initializes to the first focusable node.
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert_eq!(r.focused.as_deref(), Some("a"));
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[NavIntent::Down],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert_eq!(r.focused.as_deref(), Some("b"));
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[NavIntent::Down],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert_eq!(r.focused.as_deref(), Some("c"));
    }

    #[test]
    fn linear_wrap_wraps_past_the_end_when_enabled() {
        let mut fe = UiFocusEngine::new();
        let list = linear_list(true, None);
        fe.tick(
            Some("t"),
            Some(&list),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        // a -> up wraps to c.
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[NavIntent::Up],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert_eq!(
            r.focused.as_deref(),
            Some("c"),
            "up from first wraps to last"
        );
    }

    #[test]
    fn linear_no_wrap_clamps_at_the_end() {
        let mut fe = UiFocusEngine::new();
        let list = linear_list(false, None);
        fe.tick(
            Some("t"),
            Some(&list),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        // a -> up with no wrap stays on a.
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[NavIntent::Up],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert_eq!(
            r.focused.as_deref(),
            Some("a"),
            "no-wrap up clamps at first"
        );
    }

    #[test]
    fn spatial_picks_nearest_neighbor_by_direction() {
        let mut fe = UiFocusEngine::new();
        let list = grid_list();
        fe.tick(
            Some("t"),
            Some(&list),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        // a -> right -> b, a -> down -> c.
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[NavIntent::Right],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert_eq!(r.focused.as_deref(), Some("b"));
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[NavIntent::Down],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert_eq!(r.focused.as_deref(), Some("d"), "right then down reaches d");
    }

    #[test]
    fn focus_neighbors_override_wins_over_policy() {
        let mut fe = UiFocusEngine::new();
        let mut list = grid_list();
        // Override a's "right" to jump straight to d (bypassing the spatial b).
        list.rects[0].neighbors.right = Some("d".to_string());
        fe.tick(
            Some("t"),
            Some(&list),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[NavIntent::Right],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert_eq!(
            r.focused.as_deref(),
            Some("d"),
            "focusNeighbors override beats the spatial nearest-neighbor",
        );
    }

    #[test]
    fn initial_focus_selects_the_named_starting_node() {
        let mut fe = UiFocusEngine::new();
        let mut list = linear_list(false, None);
        list.initial_focus = Some("b".to_string());
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert_eq!(
            r.focused.as_deref(),
            Some("b"),
            "initialFocus picks b, not a"
        );
    }

    #[test]
    fn hold_to_repeat_fires_at_declared_delay_and_interval() {
        let mut fe = UiFocusEngine::new();
        let list = linear_list(
            false,
            Some(RepeatPolicy {
                initial_delay_ms: 300.0,
                interval_ms: 100.0,
            }),
        );
        // Init + press down (a -> b) and arm the clock.
        fe.tick(
            Some("t"),
            Some(&list),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[NavIntent::Down],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert_eq!(r.focused.as_deref(), Some("b"), "press moves once");
        // Hold for 0.2s — under the 0.3s initial delay: no repeat yet.
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.2,
        );
        assert_eq!(
            r.focused.as_deref(),
            Some("b"),
            "no repeat before initial delay"
        );
        // Another 0.15s -> total 0.35s, past the 0.3s delay: one repeat (b -> c).
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.15,
        );
        assert_eq!(
            r.focused.as_deref(),
            Some("c"),
            "first repeat after the delay"
        );
    }

    #[test]
    fn confirm_and_cancel_fire_once_and_never_repeat() {
        let mut fe = UiFocusEngine::new();
        let list = linear_list(
            false,
            Some(RepeatPolicy {
                initial_delay_ms: 100.0,
                interval_ms: 50.0,
            }),
        );
        fe.tick(
            Some("t"),
            Some(&list),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[NavIntent::Confirm],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert!(r.confirmed, "confirm fires");
        // Holding past the delay must NOT repeat confirm (no clock armed by it).
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[],
            None,
            &[],
            InputMode::Focus,
            1.0,
        );
        assert!(!r.confirmed, "confirm never repeats");
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[NavIntent::Cancel],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert!(r.cancelled, "cancel fires once");
    }

    #[test]
    fn pointer_hover_moves_focus_in_pointer_mode() {
        let mut fe = UiFocusEngine::new();
        let list = linear_list(false, None);
        // Cursor over "b" (y in [30,50)). Pointer mode -> hover focuses b.
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[],
            Some(PointerPos { x: 10.0, y: 40.0 }),
            &[],
            InputMode::Pointer,
            0.0,
        );
        assert_eq!(
            r.focused.as_deref(),
            Some("b"),
            "hover focuses the node under the cursor"
        );
    }

    #[test]
    fn pointer_hover_ignored_in_focus_mode() {
        let mut fe = UiFocusEngine::new();
        let list = linear_list(false, None);
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[],
            Some(PointerPos { x: 10.0, y: 40.0 }),
            &[],
            InputMode::Focus,
            0.0,
        );
        // Focus mode ignores hover: stays on the initial node a.
        assert_eq!(r.focused.as_deref(), Some("a"));
    }

    #[test]
    fn click_resolves_to_topmost_z() {
        let mut fe = UiFocusEngine::new();
        // Two overlapping rects; the higher z must win the hit.
        let list = FocusRectList {
            rects: vec![
                rect("under", [0.0, 0.0, 100.0, 100.0], 0, None),
                rect("over", [0.0, 0.0, 100.0, 100.0], 5, None),
            ],
            groups: vec![],
            initial_focus: None,
            restore_on_return: false,
        };
        let r = fe.tick(
            Some("t"),
            Some(&list),
            &[],
            None,
            &[PointerPos { x: 50.0, y: 50.0 }],
            InputMode::Pointer,
            0.0,
        );
        assert_eq!(r.focused.as_deref(), Some("over"), "click hits topmost z");
        assert!(r.confirmed, "a click also activates the hit node");
    }

    #[test]
    fn restore_on_return_restores_lower_tree_focus_on_pop() {
        let mut fe = UiFocusEngine::new();
        let mut lower = linear_list(false, None);
        lower.restore_on_return = true;
        // Lower tree ("hud") active; move focus to c.
        fe.tick(
            Some("hud"),
            Some(&lower),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        fe.tick(
            Some("hud"),
            Some(&lower),
            &[NavIntent::Down, NavIntent::Down],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert_eq!(fe.focused_id("hud"), Some("c"));

        // Push a modal ("pause") on top; the lower tree freezes.
        let modal = linear_list(false, None);
        let r = fe.tick(
            Some("pause"),
            Some(&modal),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert_eq!(
            r.focused.as_deref(),
            Some("a"),
            "modal starts at its own initial"
        );

        // Pop back to the lower tree: restoreOnReturn restores c, not a.
        let r = fe.tick(
            Some("hud"),
            Some(&lower),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        assert_eq!(
            r.focused.as_deref(),
            Some("c"),
            "restoreOnReturn restores the lower tree's saved focus on pop",
        );
    }

    #[test]
    fn lower_tree_freezes_while_a_modal_is_on_top() {
        let mut fe = UiFocusEngine::new();
        let lower = linear_list(false, None);
        fe.tick(
            Some("hud"),
            Some(&lower),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        // A modal is on top; nav intents drive ONLY the modal, never the frozen hud.
        let modal = linear_list(false, None);
        fe.tick(
            Some("pause"),
            Some(&modal),
            &[NavIntent::Down],
            None,
            &[],
            InputMode::Focus,
            0.0,
        );
        // The lower tree's focus is unchanged (still a) — it never saw the intent.
        assert_eq!(
            fe.focused_id("hud"),
            Some("a"),
            "frozen lower tree keeps its focus"
        );
    }
}
