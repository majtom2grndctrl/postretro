# Input and Game Loop Micro-Allocations

> **Status:** ready
> **Depends on:** none. All changes are in the `postretro` crate.
> **Related:** `postretro/src/input/mod.rs` · `postretro/src/main.rs` · `context/plans/drafts/render-perf/`

---

## Context

Companion to `render-perf`. Three per-frame allocation sites sit outside the render hot path — in input polling and the window title diagnostic — small enough that mixing them into the renderer plan would muddy its scope, but real heap traffic on every `RedrawRequested` event.

1. **`input/mod.rs:171–177`** — `InputSystem::snapshot()` deduplicates action bindings via a three-step `collect::<HashSet<_>>().into_iter().collect()` every frame:

   ```rust
   let actions: Vec<Action> = self.bindings.iter()
       .map(|b| b.action)
       .collect::<std::collections::HashSet<_>>()
       .into_iter()
       .collect();
   ```

   Three intermediate allocations (the `Vec` before `collect`, the `HashSet`, the final `Vec`) to derive a list that is fixed after binding setup. The binding list only changes on rebind — not every frame.

2. **`input/mod.rs:206`** — `self.prev_button_states = button_states.clone()` deep-copies the HashMap every frame to seed the next frame's state-transition logic.

3. **`main.rs:534–544`** — `format!()` allocates a ~180-byte String every frame for the debug window title (camera leaf, face counts, world position). The title changes every frame because position is float-formatted, so the OS window title API sees a new string on every redraw.

Individually small; together they're avoidable busy work in the per-frame path, and the fixes are short enough that a focused plan is the right shape.

**What this plan does not fix — and why.** `snapshot()` also allocates fresh `button_states` and `axis_values` maps via `HashMap::new()` at `input/mod.rs:180`, and `resolve_axis_values` at `input/bindings.rs:104–111` returns a fresh `Vec<AxisValue>` on every call. Eliminating those requires changing `ActionSnapshot`'s ownership model: today it owns its maps by value and the caller takes them by move; reusing them across frames means either (a) handing out a borrowed snapshot keyed to pooled state on `InputSystem` (adds a lifetime parameter, ripples through every `snapshot()` consumer) or (b) reusing a single long-lived `ActionSnapshot` the caller observes without ownership (changes the API shape materially). The `resolve_axis_values` return type is coupled to the same ownership model because `axis_values: HashMap<Action, Vec<AxisValue>>` at `input/mod.rs:32` — changing `resolve_axis_values` to return `[Option<AxisValue>; 2]` alone doesn't eliminate the allocation; the call site at line 192 would just construct a fresh `Vec` to insert into the HashMap. Both belong to a larger API refactor; this plan takes the narrower win.

---

## Goal

Remove three per-frame allocations without changing observable input behavior or the window title content. `cargo test -p postretro` passes unchanged.

---

## Approach

Two file-isolated tasks, fully parallel. Task A covers the two input-subsystem allocations in `snapshot()`; Task B covers the window title. They share no files and can ship in either order.

```
A (input/mod.rs)    ───── merge
B (main.rs title)   ───── merge
```

---

### Task A — Input snapshot: cache unique actions, reuse prev_button_states allocation

**Crate:** `postretro` · **File:** `src/input/mod.rs`

**Fix 1: cache unique action list.** Add a `unique_actions: Vec<Action>` field to `InputSystem`. Populate it whenever bindings change (constructor and any rebind entry point). `snapshot()` iterates the cached list instead of rebuilding it:

```rust
// Replace lines 171–177 with:
for &action in &self.unique_actions {
    ...
}
```

If bindings are currently immutable after construction, the field is built once in the constructor and never touched again. If there's a rebind API, update the cache there. Grep for write sites to `self.bindings` inside `impl InputSystem` to find the refresh points.

**Fix 2: reuse prev_button_states allocation.** Replace the clone at line 206:

```rust
// Before:
self.prev_button_states = button_states.clone();

// After:
self.prev_button_states.clear();
self.prev_button_states.extend(button_states.iter().map(|(&k, &v)| (k, v)));
```

`clear()` retains the HashMap's backing capacity. To make this allocation-free from the first frame onward, pre-size `prev_button_states` in whichever `InputSystem` initializer currently constructs it — call `HashMap::with_capacity(N)` where N is the count of button-type actions in the binding set (or a comfortable upper bound, since the backing store can grow but never needs to shrink). `Action` and `ButtonState` are `Copy`, so the iterator map itself is free.

After the first few frames, this pattern performs zero heap allocation for `prev_button_states`, assuming the action set is stable (which it is — see Fix 1). On the first frame, the map may rehash as it's filled to capacity; subsequent frames hit the stabilized backing store.

**Note.** `std::mem::swap` is tempting but doesn't work: after the swap, the local `button_states` would hold stale prev-state data, and `ActionSnapshot` needs the fresh frame's map to return to the caller.

---

### Task B — Window title: reuse a String buffer

**Crate:** `postretro` · **File:** `src/main.rs`

**Problem.** `format!()` at lines 534–544 allocates a new String every frame for a diagnostic window title:

```rust
ws.window.set_title(&format!(
    "Postretro | {region_label}:{} | faces: {}/{}/{}/{} (total/raw_pvs/pvs/frustum) | pos: ({:.0}, {:.0}, {:.0})",
    stats.camera_leaf, stats.total_faces, stats.raw_pvs_faces, stats.pvs_faces,
    stats.frustum_faces, pos.x, pos.y, pos.z,
));
```

**Fix: reuse a `String` buffer on `App`.** Add a `title_buf: String` field. Each frame:

```rust
use std::fmt::Write as _;

self.title_buf.clear();
write!(
    self.title_buf,
    "Postretro | {region_label}:{} | faces: {}/{}/{}/{} (total/raw_pvs/pvs/frustum) | pos: ({:.0}, {:.0}, {:.0})",
    stats.camera_leaf, stats.total_faces, stats.raw_pvs_faces, stats.pvs_faces,
    stats.frustum_faces, pos.x, pos.y, pos.z,
).unwrap();
ws.window.set_title(&self.title_buf);
```

`String::clear()` retains the backing allocation so `write!` reuses it after the first frame. Zero heap traffic once steady state is reached.

**Follow-up alternative: throttle.** If later profiling shows that the OS-level `set_title` call itself is expensive (it may do real work inside winit or the windowing backend), switching to "update the title every Nth frame" is a cheap change on top of the buffer reuse. Not done here — a human cannot read a title that updates 60 times a second anyway, so throttling is a useful follow-up but not a correctness requirement.

---

## Files to modify

| File | Task | Change |
|------|------|--------|
| `postretro/src/input/mod.rs` | A | Add `unique_actions` cache; pre-size `prev_button_states` with `with_capacity` in constructor; replace `.clone()` with `clear() + extend()` |
| `postretro/src/main.rs` | B | Add `title_buf: String` field on `App`; reuse via `clear() + write!()` in the title update path |

---

## Acceptance Criteria

All ACs are stated behaviorally (what must be true at the per-frame boundary) rather than as grep patterns. An implementer verifies by reading the final function body and reasoning about whether the per-frame path allocates, not by matching surface syntax.

1. `cargo test -p postretro` passes after each task lands. No test count regression. No new tests required.
2. No new `unsafe` blocks.
3. **Task A Fix 1:** After steady state, `InputSystem::snapshot()` does not rebuild the unique-actions list per frame — the cached `unique_actions` field is read directly. Verification: read the final `snapshot()` body; confirm the three-step `collect / HashSet / collect` chain is gone; confirm that whatever write path `self.bindings` has (constructor, rebind, or none) also refreshes `unique_actions`.
4. **Task A Fix 2:** After the first frame, the `prev_button_states` update path performs no heap allocation. The constructor (or wherever `InputSystem` is initialized) uses `HashMap::with_capacity` sized to the action count so that the first-frame `extend` also stays within the pre-sized capacity. Verification: read the final function body and the initializer; the `.clone()` at line 206 is replaced; the initializer calls `with_capacity` with a value >= the action count.
5. **Task A behavior:** Input behavior is unchanged. All existing input tests pass. Manual spot-check: WASD movement, mouse look, jump, crouch, and any other bound actions trigger at the correct edges (press/release transitions).
6. **Task B:** Window title displays the same diagnostic information, updated every frame. The title update path no longer uses `format!` — it uses `write!` into `self.title_buf`. After the first frame, no heap allocation occurs for the title string; the buffer's capacity stabilizes and `clear()` does not release it.
7. No framerate regression on the test map.

---

## Out of scope

- **The two `HashMap::new()` calls in `snapshot()` at `input/mod.rs:180`.** `button_states` and `axis_values` are allocated fresh every frame because `ActionSnapshot` currently owns them by value. Eliminating the per-frame allocation requires changing `ActionSnapshot` to either hold references to pooled state on `InputSystem` (adds a lifetime parameter, ripples through every caller of `snapshot()`) or reusing a single long-lived `ActionSnapshot` the caller observes without ownership (changes the API shape materially). Both are larger refactors than this plan is taking.
- **`resolve_axis_values` returning `Vec::new()` at `input/bindings.rs:104–111`.** Changing the return to `[Option<AxisValue>; 2]` alone does not eliminate the allocation — the call site at `input/mod.rs:192` still has to construct a `Vec<AxisValue>` to insert into `HashMap<Action, Vec<AxisValue>>`. Eliminating the allocation requires also changing the HashMap's value type, which ripples into `ActionSnapshot::axis_value` and every downstream consumer. Coupled to the `HashMap::new()` item above — both are symptoms of the same ownership-model concern.
- **Replacing `HashMap` with a smaller map type** (`FxHashMap`, `IndexMap`, `SmallMap`). Not motivated without measurement; the existing `HashMap` is fine for action sets in the low tens.
- **Redesigning the input binding data model** so bindings are stored grouped by action, which would eliminate the dedup step entirely. Larger refactor; this plan takes the narrower win.
- **Renderer hot path** — see `context/plans/drafts/render-perf/`.

---

## Open Questions

None. Promote to ready when scheduled.
