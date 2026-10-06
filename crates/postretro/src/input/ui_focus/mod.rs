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
//! It consumes the [`FocusRectList`](postretro_ui::tree::FocusRectList) the
//! renderer published the PREVIOUS frame (the N→N+1 contract applied in reverse),
//! so the focus ring may trail a focus change by one frame — the same latency every
//! UI event carries.

use crate::input::ui_dispatch::PointerPos;
use crate::input::ui_nav::NavIntent;
use postretro_ui::tree::{FocusRectList, NodeInteraction, RepeatPolicy};

mod nesting;
mod repeat;
mod slider;
#[cfg(test)]
mod tests;
mod traversal;

use nesting::{Candidate, first_enabled_stop, group_contains, step_from};
use repeat::{ConfirmRepeatClock, ENGINE_DEFAULT_REPEAT, RepeatClock, RepeatTimer};
pub use slider::{capture_slider_step, slider_value};
use traversal::{Dir, hit_test_topmost, initial_focus_id, linear_index_step, neighbor_override};

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

impl InputMode {
    /// The stable wire name for the engine-owned `input.mode` enum slot
    /// (`"pointer"` / `"focus"`). The App-side input-mode writer maps a mode
    /// transition to this string; a `text` widget bound to `input.mode` displays
    /// it. Matches the enum values declared on the slot in `slot_table.rs`.
    pub fn wire_name(self) -> &'static str {
        match self {
            InputMode::Pointer => "pointer",
            InputMode::Focus => "focus",
        }
    }

    /// Whether the OS cursor should be VISIBLE for this mode while a capturing UI
    /// tree is on the stack (M13 Goal F, Task 5). `Pointer` shows the cursor (the
    /// user is pointing); `Focus` hides it (the user is navigating with stick /
    /// D-pad / keys, so the cursor would be a distraction). Inert when no
    /// capturing tree is up — the caller gates on that. See: context/lib/input.md §5.
    pub fn cursor_visible(self) -> bool {
        matches!(self, InputMode::Pointer)
    }

    /// Whether the focus RING should be shown for this mode while a capturing UI
    /// tree is on the stack. The complement of [`cursor_visible`]: `Focus` shows
    /// the ring (directional nav needs it), `Pointer` hides it (the cursor is the
    /// indicator). Inert when no capturing tree is up — the caller gates on that.
    pub fn ring_visible(self) -> bool {
        matches!(self, InputMode::Focus)
    }
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
    /// Activation-repeat clock for a held confirm on a `repeatOnHold` button (M13
    /// Text-Entry, Task 2). `Some` only while such a button's confirm is held.
    confirm_repeat: Option<ConfirmRepeatClock>,
    /// The member last focused in each nested group, keyed by tree and group
    /// index; entering the group lands there. Validated on use, since a
    /// rebuild may renumber groups or remove the member.
    group_memory: std::collections::HashMap<(String, usize), String>,
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
    /// Signed value steps a held slider repeat produced this tick, already
    /// multiplied by its acceleration. The app applies them to the focused
    /// slider's slot.
    pub slider_steps: i32,
    /// Tabs a tab intent activated this tick, in order; the app fires each
    /// one's `onPress` as a confirm would.
    pub activations: Vec<String>,
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
            // The active tree published no rect list yet (first frame after a
            // push, or the export still describes the tree a pop removed this
            // frame). Focus nothing, and leave the active key unchanged so the
            // next tick with this tree's own export still sees the change (P11).
            return FocusTickResult::default();
        };

        // Stack change detection: the active tree differs from last tick (a push or
        // a returning pop). Drop the repeat clock (a new tree must re-arm) and
        // ensure the now-active tree's focus is selected: restore its saved focus
        // when it opted into `restoreOnReturn`, else (re)select its initial focus.
        let stack_changed = self.active_key.as_deref() != Some(active_key);
        if stack_changed {
            self.repeat = None;
            self.confirm_repeat = None;
        }
        self.ensure_initialized(active_key, rects, stack_changed);
        self.active_key = Some(active_key.to_string());

        // Drop a focused id that no longer exists in the current rect list (a
        // structural rebuild removed the node); fall back to the initial focus.
        if let Some(focused) = self.focused_id(active_key)
            && !rects.rects.iter().any(|r| r.id == focused)
        {
            let initial = initial_focus_id(rects);
            self.set_focused(active_key, initial);
        }

        let mut result = FocusTickResult::default();

        // Clicks always hit-test by topmost z (in either mode); hover-to-focus is
        // the pointer-mode-only part. In Pointer mode cursor motion moves focus
        // (hover), then any click below hit-tests and activates. Hover never
        // enqueues an intent (it is tracked cursor state), so it is applied here
        // directly from the tracked position; the click loop runs regardless of mode.
        if mode == InputMode::Pointer
            && let Some(cursor) = cursor
            && let Some(hit) = hit_test_topmost(rects, cursor)
        {
            self.set_focused(active_key, Some(hit.to_string()));
        }
        for click in clicks {
            if let Some(hit) = hit_test_topmost(rects, *click) {
                self.set_focused(active_key, Some(hit.to_string()));
                // A click also activates the hit node (confirm-equivalent).
                result.confirmed = true;
            }
        }

        // Directional / activation nav. A directional press moves focus and arms
        // the hold-to-repeat clock; cancel fires once and never repeats. Confirm
        // fires once AND — on a `repeatOnHold` button — arms the activation-repeat
        // clock so a held confirm re-fires below (the one activation-repeat
        // exception; nav `confirm` is otherwise single-fire like F).
        let mut held_dir: Option<Dir> = None;
        let mut confirm_pressed = false;
        // The tab a tab intent selected earlier this tick: two intents on one
        // frame advance two tabs, though the export still shows the old one.
        let mut pending_tab: Option<usize> = None;
        for &nav in intents {
            match nav {
                NavIntent::Confirm => {
                    result.confirmed = true;
                    confirm_pressed = true;
                }
                NavIntent::Cancel => result.cancelled = true,
                NavIntent::Next => self.step_linear(active_key, rects, 1),
                NavIntent::Prev => self.step_linear(active_key, rects, -1),
                NavIntent::TabNext => {
                    self.step_tab(active_key, rects, 1, &mut pending_tab, &mut result);
                }
                NavIntent::TabPrev => {
                    self.step_tab(active_key, rects, -1, &mut pending_tab, &mut result);
                }
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
        result.slider_steps = self.advance_repeat(active_key, rects, held_dir, dt);

        // Activation-repeat (M13 Text-Entry, Task 2): a held confirm on a focused
        // `repeatOnHold` button re-fires its activation on the SAME repeat timer.
        // The confirm intent stream carries one edge per press (held confirms are
        // suppressed at the input edge, exactly like directional nav), so a held
        // confirm shows up as one `Confirm` on press then none while held — the dt
        // clock turns that absence into repeats. Each repeat sets `confirmed` so the
        // app fires `on_press` through the same activation path the initial press
        // uses. Released externally (the app drives `release_confirm_repeat`).
        if self.advance_confirm_repeat(active_key, rects, confirm_pressed, dt) {
            result.confirmed = true;
        }

        self.remember_group_focus(active_key, rects);
        result.focused = self.focused_id(active_key).map(str::to_string);
        result
    }

    /// Record the focused node as the last-focused member of every group that
    /// holds it, so re-entering any of them lands back on it.
    fn remember_group_focus(&mut self, key: &str, rects: &FocusRectList) {
        let Some(focused) = self.focused_id(key) else {
            return;
        };
        let Some(rect) = rects.rects.iter().position(|r| r.id == focused) else {
            return;
        };
        let focused = focused.to_string();
        let mut group = rects.rects[rect].group;
        while let Some(g) = group {
            self.group_memory
                .insert((key.to_string(), g), focused.clone());
            group = rects.groups[g].parent;
        }
    }

    /// The node focus lands on when a move enters `group`: its last-focused
    /// member while that still exists inside it and is enabled, else its first
    /// enabled member (P13).
    fn enter_group(&self, key: &str, rects: &FocusRectList, group: usize) -> Option<String> {
        let remembered = self
            .group_memory
            .get(&(key.to_string(), group))
            .and_then(|id| rects.rects.iter().position(|r| &r.id == id))
            .filter(|&r| !rects.rects[r].disabled && group_contains(rects, group, r));
        remembered
            .or_else(|| first_enabled_stop(rects, group))
            .map(|r| rects.rects[r].id.clone())
    }

    /// Ensure the active tree has a selected focus. On a stack change, restore the
    /// saved focus when the tree opted into `restoreOnReturn` (and it still
    /// exists), else select the tree's `initialFocus` (or the first focusable node).
    /// On a non-stack-change tick, initialize only if not yet initialized.
    fn ensure_initialized(&mut self, key: &str, rects: &FocusRectList, stack_changed: bool) {
        let entry = self.trees.entry(key.to_string()).or_default();
        // Restore applies to a key seen before: the app keys each pushed modal
        // instance separately, so a fresh push (even of a tree closed and
        // reopened this frame) is a new key with no saved focus (O14, P12).
        let restore_valid = rects.restore_on_return
            && entry
                .focused
                .as_ref()
                .is_some_and(|f| rects.rects.iter().any(|r| &r.id == f && !r.disabled));

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

    /// Forget saved focus for trees no longer on the stack, so per-instance
    /// keys do not accumulate.
    pub fn retain_trees(&mut self, alive: impl Fn(&str) -> bool) {
        self.trees.retain(|key, _| alive(key));
        self.group_memory.retain(|(key, _), _| alive(key));
    }

    /// How many trees hold saved focus.
    pub fn tree_count(&self) -> usize {
        self.trees.len()
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

        // 1) Neighbor override wins — but never onto a disabled node (M13 G2-T3):
        // a disabled target is unreachable, so the override is ignored and the
        // governing group policy resolves the move instead (which also skips it).
        if let Some(target) = neighbor_override(current, dir)
            && rects.rects.iter().any(|r| r.id == target && !r.disabled)
        {
            self.set_focused(key, Some(target.to_string()));
            return;
        }

        // 2) Governing group policy.
        let Some(group_idx) = current.group else {
            return;
        };
        // Each nested group is one candidate in its group, by its bounds. A
        // move the group cannot answer continues in the enclosing group,
        // escaping outward until a group answers or the root is reached.
        let Some(current_index) = rects.rects.iter().position(|r| r.id == current_id) else {
            return;
        };
        let mut origin = Candidate::Rect(current_index);
        let mut group = group_idx;
        loop {
            match step_from(rects, group, origin, dir) {
                Some(Candidate::Rect(r)) => {
                    self.set_focused(key, Some(rects.rects[r].id.clone()));
                    return;
                }
                Some(Candidate::Group(g)) => {
                    if let Some(id) = self.enter_group(key, rects, g) {
                        self.set_focused(key, Some(id));
                    }
                    return;
                }
                None => match rects.groups[group].parent {
                    Some(parent) => {
                        origin = Candidate::Group(group);
                        group = parent;
                    }
                    None => return,
                },
            }
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

    /// Activate the adjacent tab in the top tree's tablist and move focus to it,
    /// wrapping and skipping disabled tabs (P20). The tablist holding focus
    /// wins, else the first. With no tab selected, forward picks the first tab
    /// and back the last; a lone enabled tab takes focus without activating. A
    /// tree with no tablist steps Next/Prev instead.
    fn step_tab(
        &mut self,
        key: &str,
        rects: &FocusRectList,
        delta: i32,
        pending: &mut Option<usize>,
        result: &mut FocusTickResult,
    ) {
        let focused_tablist = self
            .focused_id(key)
            .and_then(|id| rects.rects.iter().find(|r| r.id == id))
            .and_then(|r| r.tablist);
        let Some(tablist) =
            focused_tablist.or_else(|| rects.rects.iter().filter_map(|r| r.tablist).min())
        else {
            self.step_linear(key, rects, delta);
            return;
        };
        let tabs: Vec<usize> = (0..rects.rects.len())
            .filter(|&i| rects.rects[i].tablist == Some(tablist))
            .collect();
        let enabled = |i: usize| !rects.rects[i].disabled;
        let enabled_count = tabs.iter().filter(|&&i| enabled(i)).count();
        if enabled_count == 0 {
            return;
        }
        if enabled_count == 1 {
            let only = tabs
                .iter()
                .copied()
                .find(|&i| enabled(i))
                .unwrap_or(tabs[0]);
            self.set_focused(key, Some(rects.rects[only].id.clone()));
            return;
        }
        let current = pending.or_else(|| {
            tabs.iter()
                .copied()
                .find(|&i| rects.rects[i].selected.is_some_and(|s| s > 0.5))
        });
        let next = match current.and_then(|cur| tabs.iter().position(|&i| i == cur)) {
            None => {
                let mut order = tabs.iter().copied().filter(|&i| enabled(i));
                if delta > 0 {
                    order.next()
                } else {
                    order.next_back()
                }
            }
            Some(position) => {
                let len = tabs.len() as i32;
                let mut raw = position as i32;
                let mut found = None;
                for _ in 0..len {
                    raw = (raw + delta).rem_euclid(len);
                    if enabled(tabs[raw as usize]) {
                        found = Some(tabs[raw as usize]);
                        break;
                    }
                }
                found
            }
        };
        let Some(next) = next else {
            return;
        };
        *pending = Some(next);
        let id = rects.rects[next].id.clone();
        self.set_focused(key, Some(id.clone()));
        result.activations.push(id);
    }

    /// Advance the hold-to-repeat clock. `pressed_dir` is the direction pressed
    /// THIS tick (a fresh edge), if any. A press (re)arms the clock; with no press
    /// this tick the clock is advanced by `dt` and fires a synthetic directional
    /// move once the initial delay then each interval elapses. Confirm/cancel never
    /// reach this function; confirm repeat for `repeatOnHold` buttons is handled by
    /// [`advance_confirm_repeat`].
    ///
    /// Returns the signed slider steps a repeat produced: a repeat on a focused
    /// slider that captures the held direction steps its value instead of
    /// moving focus.
    fn advance_repeat(
        &mut self,
        key: &str,
        rects: &FocusRectList,
        pressed_dir: Option<Dir>,
        dt: f32,
    ) -> i32 {
        if let Some(dir) = pressed_dir {
            // Fresh press: arm (or re-arm to the new direction) the clock.
            self.repeat = Some(RepeatClock::armed(dir, self.repeat_policy(key, rects)));
            return 0;
        }

        // No press this tick. If a clock is armed, advance it and fire a repeat.
        let Some(clock) = self.repeat.as_mut() else {
            return 0;
        };
        clock.held += dt;
        if !clock.timer.advance(dt) {
            return 0;
        }
        let dir = clock.dir;
        let multiplier = clock.slider_multiplier();
        match self.focused_slider_sign(key, rects, dir) {
            Some(sign) => sign * multiplier,
            None => {
                self.move_focus(key, rects, dir);
                0
            }
        }
    }

    /// The focused node's group repeat cadence; the engine default where the
    /// group authors none. An authored zero delay still never repeats.
    fn repeat_policy(&self, key: &str, rects: &FocusRectList) -> RepeatPolicy {
        self.focused_id(key)
            .and_then(|id| rects.rects.iter().find(|r| r.id == id))
            .and_then(|r| r.group)
            .and_then(|g| rects.groups[g].repeat)
            .unwrap_or(ENGINE_DEFAULT_REPEAT)
    }

    /// `+1`/`-1` when the focused node is a slider that captures `dir`'s nav,
    /// else `None`.
    fn focused_slider_sign(&self, key: &str, rects: &FocusRectList, dir: Dir) -> Option<i32> {
        let focused = self.focused_id(key)?;
        let rect = rects.rects.iter().find(|r| r.id == focused)?;
        let Some(NodeInteraction::Slider { captures_nav, .. }) = &rect.interaction else {
            return None;
        };
        let nav = dir.to_nav();
        captures_nav
            .iter()
            .any(|name| name == nav.wire_name())
            .then_some(match dir {
                Dir::Right | Dir::Up => 1,
                Dir::Left | Dir::Down => -1,
            })
    }

    /// Arm hold-to-repeat for a directional press a focused slider captured
    /// before the tick (the press stepped the value once; holding repeats it).
    /// The press is captured before this frame's tick, so the policy comes
    /// from the tree active last tick; a stack change this frame clears it.
    pub fn arm_slider_repeat(&mut self, rects: &FocusRectList, nav: NavIntent) {
        let (Some(dir), Some(key)) = (Dir::from_nav(nav), self.active_key.clone()) else {
            return;
        };
        self.repeat = Some(RepeatClock::armed(dir, self.repeat_policy(&key, rects)));
    }

    /// Clear the hold-to-repeat clock — called when the held direction releases
    /// (the app observes the directional key/stick return and drives release).
    pub fn release_repeat(&mut self) {
        self.repeat = None;
    }

    /// Advance the activation (confirm) repeat clock (M13 Text-Entry, Task 2),
    /// returning `true` when a repeated activation fired this tick. A fresh confirm
    /// press arms the clock ONLY when the focused node is a `button` carrying a
    /// `repeat_on_hold` policy (the on-screen keyboard backspace); otherwise it
    /// clears any armed clock, preserving F's single-fire rule for flag-less
    /// confirms. With no press this tick the armed clock advances by `dt` on the
    /// shared [`RepeatTimer`], firing once per elapsed delay/interval. The fired
    /// activation re-targets the CURRENTLY focused button each tick (the same
    /// node — confirm does not move focus), so the app fires its `on_press` again.
    fn advance_confirm_repeat(
        &mut self,
        key: &str,
        rects: &FocusRectList,
        confirm_pressed: bool,
        dt: f32,
    ) -> bool {
        if confirm_pressed {
            // Fresh confirm: arm the clock only for a `repeatOnHold` button; any
            // other focused node leaves activation single-fire (clear the clock).
            match self.focused_button_repeat(key, rects) {
                Some(policy) => {
                    self.confirm_repeat = Some(ConfirmRepeatClock {
                        timer: RepeatTimer::armed(policy.initial_delay_ms, policy.interval_ms),
                    });
                }
                None => self.confirm_repeat = None,
            }
            return false;
        }

        // No confirm this tick. If a clock is armed, advance it; any fire is a
        // repeated activation (a held confirm yields one edge then silence — the
        // dt clock turns that silence into repeats, mirroring the nav clock).
        let Some(clock) = self.confirm_repeat.as_mut() else {
            return false;
        };
        clock.timer.advance(dt)
    }

    /// Clear the activation-repeat clock — called when the held confirm releases
    /// (the app observes the confirm key/button return and drives release, mirroring
    /// [`release_repeat`] for directional nav).
    pub fn release_confirm_repeat(&mut self) {
        self.confirm_repeat = None;
    }

    /// The `repeat_on_hold` policy of the focused node when it is a `button`
    /// carrying one, else `None`. Drives whether a held confirm arms the
    /// activation-repeat clock.
    fn focused_button_repeat(&self, key: &str, rects: &FocusRectList) -> Option<RepeatPolicy> {
        let focused = self.focused_id(key)?;
        let rect = rects.rects.iter().find(|r| r.id == focused)?;
        match &rect.interaction {
            Some(NodeInteraction::Button { repeat_on_hold, .. }) => *repeat_on_hold,
            _ => None,
        }
    }
}
