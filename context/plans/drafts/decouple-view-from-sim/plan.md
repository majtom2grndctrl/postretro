# Decouple View From Sim

> **Status:** draft
> **Depends on:** none. All changes are in the `postretro` crate.
> **Related:** `postretro/src/input/mod.rs` · `postretro/src/main.rs` · `postretro/src/camera.rs` · `postretro/src/frame_timing.rs` · `context/lib/input.md` · `context/lib/rendering_pipeline.md`

---

## Goal

Fix silent mouse input loss on zero-tick frames. View rotation (yaw/pitch) updates every render frame using the full accumulated mouse delta and the frame's elapsed time; movement and all other game logic continue running at the fixed tick rate. Mouse look is lossless and smooth at any render rate.

---

## Scope

### In scope

- Mouse displacement handling: consume the full accumulated mouse delta every render frame, not inside the fixed-tick loop
- Gamepad velocity-source look: move to render-rate integration using frame elapsed time (`frame_dt`) instead of `tick_dt`
- `InputSystem::snapshot()` split: separate evanescent render-rate inputs (mouse displacement, gamepad look velocity) from tick-rate inputs (movement, buttons)
- `Camera` yaw/pitch update path: update directly from render-rate look, before `push_state` and before rendering
- `InterpolableState` rationalization: yaw/pitch no longer need to be interpolated between tick states, because they are now at render rate and always "current"
- Tests that pin the "no input lost when ticks == 0" invariant at a level that would have caught this bug

### Out of scope

- Movement/collision: stays at tick rate, no changes
- Networking: non-goal for this project per `context/lib/index.md §4`
- New input devices beyond mouse/keyboard/gamepad
- Any unrelated refactors (including the input-perf allocation work in `context/plans/ready/input-perf/`)
- HUD or on-screen diagnostic changes
- The `FrameRateMeter` and vsync toggle — already complete, not touched here

---

## Shared Context

### The root cause

`InputSystem::snapshot()` drains and zeros `mouse_delta` unconditionally. When `ticks == 0`, the fixed-tick `for` loop never runs, but the delta was already consumed. The motion is permanently lost. At high render rates (vsync off), the majority of frames have `ticks == 0`, so most mouse input is silently discarded.

### The canonical split: render-rate view, tick-rate sim

Every serious fixed-timestep engine since id Tech 3 (1999) makes this architectural distinction:

- **Evanescent inputs** (mouse displacement, scroll wheel, click-edges) — rate-based and one-shot. If the consumer doesn't read them this frame, they are gone. These must be consumed at render rate, every frame, regardless of how many ticks fired.
- **Persistent inputs** (held keys, gamepad movement sticks) — state-based. "W is still held" is equally true next frame. These are safely read inside the tick loop.

Quake 3's `cl_input.c` reflects this: `CL_MouseMove` updates `cl.viewangles` directly from `cl.mouseDx/Dy` once per rendered frame, before the usercmd is submitted to the server. View angles are client-side, render-rate state. Server ticks consume the resulting `usercmd_t`, but the angles themselves updated before each rendered frame. Unity's official guidance on character controller input reinforces the same principle: handle evanescent events (mouse delta, button edges) in `Update()` — the render-rate callback — not in `FixedUpdate()`. Their `FixedInputEvent` pattern exists precisely because input loss in FixedUpdate is a known, documented failure mode.

The Gaffer on Games "Fix Your Timestep!" accumulator pattern (the model Postretro already implements correctly) handles rendering but is silent on input; it assumes the reader knows to sample evanescent inputs outside the tick loop.

### Gamepad look velocity: also render-rate

The current code applies gamepad look velocity inside the tick loop using `tick_dt`. This is conceptually correct at steady-state 60Hz (one tick per frame), but breaks at non-integer multiples of the tick rate. At 200Hz render rate with a 60Hz tick, most frames have zero ticks. When a tick frame fires, gamepad look jumps by a full tick's worth of rotation. Between tick frames it doesn't move at all. The result is visible jitter.

The fix is the same as for mouse: move gamepad look velocity integration to render rate, using frame elapsed time (`frame_dt`) instead of `tick_dt`. `GAMEPAD_LOOK_SENSITIVITY` stays in radians-per-second, multiplied by `frame_dt` in seconds. The result is smooth rotation proportional to real elapsed time.

### Where `frame_dt` comes from

`FrameTiming::begin_frame(now)` already computes `elapsed = now.duration_since(self.last_frame)`. It stores this as an internal duration to feed the accumulator. The plan adds `frame_dt: f32` to `FrameTickResult` (returned by `begin_frame`/`accumulate`), exposing the elapsed frame time in seconds. No new Instant calls, no new state, no allocation.

### Where yaw/pitch live after this change

Currently `Camera::yaw` and `Camera::pitch` are updated inside the tick loop, then pushed into `InterpolableState` for interpolation. After the split:

- `Camera::yaw` and `Camera::pitch` are authoritative view-angle state, updated at render rate by applying the full frame's mouse delta and gamepad look velocity before the tick loop runs.
- Movement direction (forward/right) is derived from `camera.forward()` and `camera.right()` inside the tick loop as before, reading the already-updated yaw from `Camera`. This is correct: if the player moved the mouse and then the tick fires within the same frame, movement correctly uses the freshest view direction.
- `InterpolableState` carries yaw/pitch for backward compatibility with the `view_projection` call that follows the tick loop. After this change, `push_state` still stores yaw/pitch in `InterpolableState`, but they are identical on both state slots because view angles update before `push_state` is called. Interpolating between identical values is correct — the position interpolation still benefits from having two states. No `InterpolableState` fields are removed; the rendering path is unchanged.

This means: on a zero-tick frame, position is interpolated from the previous two tick states (correct, smooth), and yaw/pitch come from the freshest camera state (updated this frame from the mouse, also correct). The view is lossless and current.

### Behavior during stall/catch-up (ticks > 1)

When `ticks == 5` (frame after a stall), the view angles are updated once at render rate (using the accumulated frame delta), and then the tick loop runs five times, each reading the same `camera.forward()` / `camera.right()` from the freshest view angles. This is exactly right. The player looks somewhere; five ticks of movement fire in that direction. The view angles are not divided or re-applied per tick — they are already updated before the loop. This was always how the displacement branch worked (dividing evenly across ticks) but now becomes the uniform model: angles update once per frame, ticks drive position.

### Snapshot model change

The current `snapshot()` is a single call that drains everything. After this change, the frame loop makes two distinct reads from the input system:

1. **`drain_look_inputs()`** — called once per frame, before the tick loop. Returns the accumulated mouse displacement and current gamepad look axis values. Drains `mouse_delta` and `mouse_axes` for look actions. Does not touch button states or movement axes.
2. **`snapshot()`** — called once per frame (as today), before the tick loop. Returns the full action snapshot for movement and buttons. This is unchanged for movement/gameplay consumers.

Both calls happen before the tick loop. The tick loop reads from `snapshot` for movement/buttons (as today). The render-rate look update reads from `drain_look_inputs()` immediately before the tick loop. The order inside `RedrawRequested`:

```
// Proposed design — not final code
1. let now = Instant::now();
2. let frame_result = self.frame_timing.begin_frame(now);  // frame_dt now in result
3. let ticks = frame_result.ticks;
4. let frame_dt = frame_result.frame_dt;

5. if let Some(gp) = &mut self.gamepad_system { gp.update(&mut self.input_system); }

6. let look = self.input_system.drain_look_inputs();   // NEW: consumes mouse+gamepad look
7. let snapshot = self.input_system.snapshot();        // unchanged: movement + buttons

8. // Apply look at render rate — runs every frame, ticks == 0 or not
9. self.camera.rotate(look.yaw_delta(frame_dt), look.pitch_delta(frame_dt));

10. // Tick loop — unchanged for movement
11. for _ in 0..ticks {
12.     // movement reads snapshot, camera.forward()/right() from already-updated angles
13.     ...
14.     self.frame_timing.push_state(InterpolableState::new(
15.         self.camera.position,
16.         self.camera.yaw,    // already render-rate current
17.         self.camera.pitch,  // already render-rate current
18.     ));
19. }
```

`LookInputs` is a plain struct: `yaw_displacement: f32`, `pitch_displacement: f32`, `yaw_velocity: f32`, `pitch_velocity: f32`. Its `yaw_delta(frame_dt)` method returns `yaw_displacement + yaw_velocity * GAMEPAD_LOOK_SENSITIVITY * frame_dt`. No heap allocation.

### Determinism note

The fixed tick loop is deterministic: given the same sequence of `snapshot()` results and the same starting state, it produces the same position/velocity output. View angles are no longer part of the tick loop's determinism domain — they are render-rate state. For any future networking where the authoritative server needs to tick at a fixed rate, view angles would be sent as part of `usercmd_t` (as in id Tech 3), not derived inside the server's tick loop. This is the networking-friendly architecture; the change makes future networking easier, not harder.

---

## Tasks

### Task 1: Add `frame_dt` to `FrameTickResult`

**Description:** `FrameTickResult` currently returns `ticks` and `alpha`. Add `frame_dt: f32` — the elapsed frame time in seconds. `FrameTiming::accumulate` already computes `elapsed` from the `Duration` passed in; capture it as `frame_dt = elapsed.as_secs_f32()`. On a zero-time frame (`elapsed.is_zero()`), `frame_dt` is `0.0`. No new state on `FrameTiming` — it is computed and returned, not stored.

**Acceptance criteria:**
- [ ] `FrameTickResult` has a `frame_dt: f32` field
- [ ] `frame_dt` equals the elapsed duration passed to `accumulate()`, in seconds
- [ ] `frame_dt` is `0.0` when `elapsed.is_zero()`
- [ ] Existing `FrameTiming` tests pass unchanged
- [ ] One new unit test: `accumulate_returns_correct_frame_dt` — pass a known `Duration`, assert `frame_dt` matches `Duration::as_secs_f32()` within epsilon
- [ ] No `unsafe`, no per-frame allocation

**Depends on:** none

---

### Task 2: Add `drain_look_inputs()` to `InputSystem`

**Description:** Add a new method `drain_look_inputs(&mut self) -> LookInputs` to `InputSystem`. `LookInputs` is a plain struct with four `f32` fields: `yaw_displacement`, `pitch_displacement`, `yaw_velocity`, `pitch_velocity`. The method calls `resolve_mouse_axes()` (already exists), copies the resolved look-action values out of `mouse_axes`, copies current gamepad look axis values from `gamepad_axes`, then clears `mouse_delta` and the look-action entries from `mouse_axes`. It does NOT clear button states, movement axes, or any non-look axis.

`LookInputs` provides a method `yaw_delta(frame_dt: f32) -> f32` returning `self.yaw_displacement + self.yaw_velocity * GAMEPAD_LOOK_SENSITIVITY * frame_dt`, and similarly `pitch_delta(frame_dt: f32) -> f32`. These methods live on `LookInputs`, not on `InputSystem` — they are pure math with no side effects.

The existing `snapshot()` method must continue to work correctly after `drain_look_inputs()` is called in the same frame: it should find `mouse_delta` already zero (no duplicate application of mouse displacement) and `mouse_axes` already cleared for look actions (no duplication). Since `resolve_mouse_axes()` runs inside `snapshot()` too, and both `drain_look_inputs()` and `snapshot()` call it, verify that double-calling `resolve_mouse_axes()` on a cleared `mouse_delta` is a no-op (it is: delta is (0,0), so no new values enter `mouse_axes`).

**Acceptance criteria:**
- [ ] `LookInputs` struct defined with the four fields and the two delta methods; placed in `input/mod.rs` or extracted to a small sibling if it grows (it won't)
- [ ] `drain_look_inputs()` returns correct yaw/pitch displacement from mouse delta accumulated since last call
- [ ] `drain_look_inputs()` returns correct yaw/pitch velocity from current gamepad axes
- [ ] After `drain_look_inputs()`, calling `snapshot()` in the same frame returns no look-axis displacement values (mouse was drained) but still returns movement and button states correctly
- [ ] `LookInputs::yaw_delta(frame_dt)` and `pitch_delta(frame_dt)` produce correct combined deltas (displacement + velocity * sensitivity * frame_dt)
- [ ] No per-frame heap allocation in `drain_look_inputs()` — `LookInputs` is a stack-allocated struct
- [ ] No `unsafe`
- [ ] Unit tests (in `input/mod.rs` tests block):
  - `drain_look_inputs_returns_mouse_displacement` — accumulate a known delta, call drain, assert displacement matches expected radians
  - `drain_look_inputs_clears_delta_for_subsequent_snapshot` — drain then snapshot; snapshot has no look displacement
  - `drain_look_inputs_does_not_clear_movement_axes` — W key held, drain look, snapshot still shows MoveForward active
  - `look_inputs_yaw_delta_combines_displacement_and_velocity` — known displacement + known velocity + frame_dt, assert combined delta

**Depends on:** none (Task 1 and Task 2 are independent)

---

### Task 3: Move look rotation to render rate in `main.rs`

**Description:** Restructure the `RedrawRequested` handler to apply look rotation once per frame before the tick loop, using `drain_look_inputs()` and `frame_dt`. Remove the look-rotation logic from inside the `for _ in 0..ticks` loop. The tick loop continues to read `snapshot` for movement and buttons.

Concrete changes to `main.rs`:

1. After `begin_frame` / gamepad poll: call `self.input_system.drain_look_inputs()` to get `look`.
2. Call `self.input_system.snapshot()` as today for movement/button state.
3. Before the tick loop, apply: `self.camera.rotate(look.yaw_delta(frame_dt), look.pitch_delta(frame_dt));`
4. Remove from inside the `for _ in 0..ticks` loop: the `yaw_delta` / `pitch_delta` accumulation, the `AxisSource::Displacement` and `AxisSource::Velocity` match arms for look axes, and `self.camera.rotate(...)`.
5. Remove `look_yaw_values` and `look_pitch_values` locals (the pre-computed slices from the snapshot that were used for the per-tick look).
6. `push_state` inside the tick loop continues to store `self.camera.yaw` and `self.camera.pitch` — these are now render-rate current, but that is correct.

The tick loop after this change contains only: movement direction computation, `self.camera.position += ...`, and `self.frame_timing.push_state(...)`.

**Acceptance criteria:**
- [ ] Mouse look is applied exactly once per render frame, regardless of `ticks`
- [ ] On a zero-tick frame, a non-zero mouse delta is still applied to `self.camera` (the core regression test)
- [ ] On a multi-tick frame (`ticks > 1`, simulated via `accumulate(TICK_DURATION * 3)`), look is applied once (not three times)
- [ ] WASD movement still works correctly at tick rate (unchanged logic in tick loop)
- [ ] `look_yaw_values` and `look_pitch_values` locals are gone from `RedrawRequested`
- [ ] No per-frame allocation introduced
- [ ] No `unsafe`
- [ ] The `GAMEPAD_LOOK_SENSITIVITY` constant remains in `input/mod.rs` — it is now consumed by `LookInputs::*_delta()`, so the import in `main.rs` is no longer needed; remove the import if it becomes unused
- [ ] `cargo test -p postretro` passes

**Depends on:** Task 1 (needs `frame_dt` from `FrameTickResult`), Task 2 (needs `drain_look_inputs()` and `LookInputs`)

---

### Task 4: Integration test — mouse delta not lost on zero-tick frame

**Description:** Add an integration-level test that directly exercises the failure mode: a frame with accumulated mouse delta but `ticks == 0` must still rotate the camera. This test operates on `InputSystem`, `FrameTiming`, and `Camera` directly — it does not need a window, renderer, or GPU context.

The test simulates one frame:
1. Create an `InputSystem` with default bindings.
2. Accumulate a known mouse delta: `sys.handle_mouse_delta(100.0, 0.0)` (large enough to measure).
3. Call `sys.drain_look_inputs()` to get `look`.
4. Use `FrameTiming::accumulate(Duration::from_millis(5))` to get a `FrameTickResult` with `ticks == 0` and `frame_dt > 0`.
5. Assert `ticks == 0` (verifies this is actually a zero-tick scenario).
6. Apply `camera.rotate(look.yaw_delta(result.frame_dt), look.pitch_delta(result.frame_dt))`.
7. Assert `camera.yaw != 0.0` — the rotation was applied despite zero ticks.

A second test covers the multi-tick case: `ticks == 3`, look applied once before the loop, verify rotation equals the full delta (not 3× the delta).

These tests live in `postretro/src/main.rs` under `#[cfg(test)] mod tests`, or as a crate-level integration test in `postretro/tests/` if the types involved are `pub`. Choose whichever the borrow checker permits without making types unnecessarily public.

**Acceptance criteria:**
- [ ] `mouse_delta_applied_on_zero_tick_frame` test exists and passes
- [ ] `mouse_delta_not_multiplied_on_multi_tick_frame` test exists and passes
- [ ] Tests use `FrameTiming::accumulate` with a deterministic duration, not `Instant::now()` (see testing_guide.md §3 "Deterministic time")
- [ ] Tests use approximate float comparison with epsilon (see testing_guide.md §3 "Floating-point comparison")
- [ ] No GPU context required; these are pure logic tests

**Depends on:** Task 2, Task 3 (tests exercise the integrated behavior)

---

## Sequencing

**Phase 1 (concurrent — no shared files):**
- Task 1 — `frame_timing.rs` only
- Task 2 — `input/mod.rs` only

**Phase 2 (sequential — depends on Phase 1):**
- Task 3 — `main.rs`: consumes `frame_dt` from Task 1 and `drain_look_inputs()` from Task 2

**Phase 3 (sequential — depends on Phase 2):**
- Task 4 — integration tests: exercises the full integrated behavior from Tasks 1–3

Tasks 1 and 2 can be implemented concurrently by two agents or shipped as two sequential commits — they share no files and have no data dependency. Task 3 requires both because it references `frame_dt` and `drain_look_inputs()`. Task 4 requires Task 3 because it tests the integrated behavior in `main.rs`.

---

## Notes

### Research sources

- **Glenn Fiedler, "Fix Your Timestep!" (gafferongames.com):** The foundational accumulator pattern Postretro already implements. The article establishes state interpolation for rendering but is deliberately silent on input — it assumes the reader handles evanescent inputs outside the tick loop. The interpolated `state = currentState * alpha + previousState * (1 - alpha)` model used by Postretro's `interpolated_state()` is directly from this article.

- **id Tech 3, `code/client/cl_input.c` (github.com/id-Software/Quake-III-Arena):** `CL_MouseMove` updates `cl.viewangles[YAW/PITCH]` directly from accumulated `cl.mouseDx/mouseDy` once per rendered frame, scaled by sensitivity. View angles are client-side, render-rate state — not server-tick state. One `usercmd_t` per rendered frame, with the freshest angles baked in. `frame_msec` is clamped to 200ms to prevent spiral-of-death. This is the canonical "view at render rate, sim at tick rate" split.

- **Unity Character Controller docs (docs.unity3d.com/Packages/com.unity.charactercontroller@1.4):** Explicit architectural guidance that evanescent inputs (button edges, mouse delta) must be handled in `Update()` (render rate), not `FixedUpdate()` (physics rate). The `FixedInputEvent` pattern tracks whether an event occurred since the last fixed update. Validates the same distinction: displacement = render rate, persistent state = tick rate.

- **Jakub Tomsu, "Reliable fixed timestep & inputs" (jakubtomsu.github.io):** Identifies the exact bug: "Inputs are lost when there are no ticks to run for few frames." Proposes dividing mouse delta across ticks (`tick_input.cursor_delta /= f32(num_ticks)`) as an alternative to render-rate look. This plan rejects division-across-ticks in favor of render-rate look (see Alternatives below) but the article correctly diagnoses the problem and confirms it is well-known.

### Alternatives considered

**Sub-tick accumulation / divide across ticks:** The approach currently in the bug state: `yaw_delta += av.value / ticks as f32`. This is wrong when `ticks == 0` (division produces NaN or the branch is skipped). Jakub Tomsu's article suggests dividing the delta across ticks, which requires guarding `ticks == 0` specially. This is a band-aid: it correctly distributes the delta across ticks but does not give the player render-rate responsiveness. With vsync off at 300Hz render / 60Hz tick, look updates only happen 60 times per second. Compare id Tech 3: viewangles update 300 times per second in this scenario. The division-across-ticks approach is simpler to implement but inferior in feel. Rejected.

**Remove `yaw`/`pitch` from `InterpolableState`:** If view angles update at render rate and are always "current", they never need interpolation between tick states. Removing them from `InterpolableState` would simplify `push_state` and `interpolated_state`. However, the rendering path calls `interp.view_projection(aspect)`, which currently combines position (interpolated) and angles (from the state) into one matrix. Decoupling would require `view_projection` to take a separate `(yaw, pitch)` argument. This is architecturally cleaner but a larger change that touches more call sites. More importantly, keeping yaw/pitch in `InterpolableState` with render-rate values means both state slots hold the same angles, and interpolating between identical values is a no-op — correct behavior with zero code change to the rendering path. The simpler approach is taken; the cleaner refactor is a follow-up if desired.

**Moving gamepad look to render rate via a separate "velocity integrator":** Some engines maintain a dedicated "view integrator" struct that consumes velocity inputs and produces angles. Postretro doesn't need this abstraction — `LookInputs::*_delta(frame_dt)` does the integration inline, which is sufficient with only one velocity-source device (gamepad). Three similar lines beat a premature helper (development guide §1.4).

### Open questions

1. **`LookInputs` placement:** The struct is simple enough to live in `input/mod.rs`. If `input/mod.rs` exceeds ~500 lines after this addition, consider extracting it (development guide §2.1). Implementer should check line count before deciding.

2. **`drain_look_inputs()` + `snapshot()` call order:** The plan calls `drain_look_inputs()` before `snapshot()`. Both internally call `resolve_mouse_axes()`. Calling `drain_look_inputs()` first means `mouse_delta` is zeroed before `snapshot()` runs, so `snapshot()` sees no mouse displacement. This is intentional — look axes are consumed by `drain_look_inputs()`. Verify this ordering is correct and document it with a comment in `main.rs` so a future reader doesn't swap the calls.

3. **The `input-perf` plan and `snapshot()` interaction:** `context/plans/ready/input-perf/` modifies `InputSystem::snapshot()` to eliminate per-frame allocations. If that plan ships before this one, its changes to the binding resolution path must be compatible with `drain_look_inputs()`. The two plans do not conflict architecturally (input-perf only changes the deduplicated-actions cache and the `prev_button_states` clone; this plan adds a new method). Confirm no merge conflict before implementing.

4. **`GAMEPAD_LOOK_SENSITIVITY` constant:** Currently lives in `input/mod.rs` and is imported in `main.rs`. After Task 3, it is consumed by `LookInputs::*_delta()` (in `input/mod.rs`) and the import in `main.rs` can be removed. Implementer should verify the import is actually unused and remove it — Rust will warn if it isn't.
