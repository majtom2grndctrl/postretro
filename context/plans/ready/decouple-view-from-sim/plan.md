# Decouple View From Sim

> **Status:** ready
> **Depends on:** the `input-perf` changes to `InputSystem` (cached `unique_actions`, pre-sized `prev_button_states`). These already landed on `main` in commits `b4c399a` and `0dc2cff`; this plan is written against that post-input-perf state of `InputSystem`. All code changes are in the `postretro` crate.
> **Related:** `postretro/src/input/mod.rs` · `postretro/src/input/bindings.rs` · `postretro/src/main.rs` · `postretro/src/camera.rs` · `postretro/src/frame_timing.rs` · `context/lib/input.md` · `context/lib/rendering_pipeline.md`
>
> **Note on `input.md`:** the input context doc already describes the target architecture (`drain_look_inputs()`, `LookInputs`, render-rate look, gamepad `frame_dt` integration). Doc landed ahead of code. No further context-lib changes required from this plan.

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
- `InterpolableState` rationalization: drop yaw/pitch fields; `InterpolableState` interpolates position only. `view_projection` takes yaw and pitch as arguments, supplied from `self.camera` at render time
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

Currently `Camera::yaw` and `Camera::pitch` are updated inside the tick loop, then pushed into `InterpolableState` for interpolation. Rendering reads angles from `InterpolableState::view_projection`, which means on zero-tick frames the rendered angles are whatever `push_state` wrote during the last tick — stale by up to a tick's worth of frames at high render rates. Simply updating `self.camera.yaw` at render rate is not enough: rendering does not read from `self.camera`.

After the split:

- `Camera::yaw` and `Camera::pitch` are the authoritative view-angle state, updated at render rate by applying the full frame's mouse delta and gamepad look velocity before the tick loop runs.
- Movement direction (forward/right) is derived from `camera.forward()` and `camera.right()` inside the tick loop as before, reading the already-updated yaw from `Camera`. If the player moved the mouse and then a tick fires within the same frame, movement uses the freshest view direction.
- **Rendering reads angles from `self.camera` directly, not from `InterpolableState`.** `view_projection` is restructured to take yaw and pitch as arguments alongside aspect; position still comes from the interpolated state. The call site passes `self.camera.yaw` and `self.camera.pitch`. This is the only way to make the view lossless on zero-tick frames.
- `InterpolableState` no longer carries yaw/pitch. `push_state` accepts position only; `lerp` interpolates position only. Removing the fields is smaller than it sounds — two struct fields, one lerp branch, one `InterpolableState::new` signature, one test helper.

On a zero-tick frame: position is interpolated from the previous two tick states (correct, smooth), and yaw/pitch come from `self.camera` (updated this frame from the mouse, also correct). The view is lossless and current.

### Behavior during stall/catch-up (ticks > 1)

When `ticks == 5` (frame after a stall), the view angles are updated once at render rate (using the accumulated frame delta), and then the tick loop runs five times, each reading the same `camera.forward()` / `camera.right()` from the freshest view angles. This is exactly right. The player looks somewhere; five ticks of movement fire in that direction. The view angles are not divided or re-applied per tick — they are already updated before the loop. This was always how the displacement branch worked (dividing evenly across ticks) but now becomes the uniform model: angles update once per frame, ticks drive position.

### Snapshot model change

The current `snapshot()` is a single call that drains everything. After this change, the frame loop makes two distinct reads from the input system:

1. **`drain_look_inputs()`** — called once per frame, before the tick loop. Returns the accumulated mouse displacement and current gamepad look axis values. Drains `mouse_delta` and `mouse_axes` for look actions. Does not touch button states or movement axes.
2. **`snapshot()`** — called once per frame (as today), before the tick loop. Returns the full action snapshot for movement and buttons. This is unchanged for movement/gameplay consumers.

Both calls happen before the tick loop. The tick loop reads from `snapshot` for movement/buttons (as today). The render-rate look update reads from `drain_look_inputs()` immediately before the tick loop. The order inside `RedrawRequested`:

```
// Proposed design — not final code
1.  let now = Instant::now();
2.  let frame_result = self.frame_timing.begin_frame(now);  // frame_dt now in result
3.  let ticks = frame_result.ticks;
4.  let frame_dt = frame_result.frame_dt;

5.  if let Some(gp) = &mut self.gamepad_system { gp.update(&mut self.input_system); }

6.  let look = self.input_system.drain_look_inputs();   // consumes mouse + gamepad look
7.  let snapshot = self.input_system.snapshot();        // movement + buttons

8.  // Apply look at render rate — runs every frame, ticks == 0 or not.
9.  self.camera.rotate(look.yaw_delta(frame_dt), look.pitch_delta(frame_dt));

10. // Tick loop — position only.
11. for _ in 0..ticks {
12.     // movement reads snapshot; camera.forward()/right() read freshest angles
13.     ...
14.     self.frame_timing.push_state(InterpolableState::new(self.camera.position));
15. }

16. // Rendering: position interpolated, angles from self.camera directly.
17. let interp = self.frame_timing.interpolated_state();
18. let view_proj = interp.view_projection(
19.     self.camera.aspect(),
20.     self.camera.yaw,
21.     self.camera.pitch,
22. );
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

**Description:** Add a new method `drain_look_inputs(&mut self) -> LookInputs` to `InputSystem`. `LookInputs` is a plain struct with four `f32` fields: `yaw_displacement`, `pitch_displacement`, `yaw_velocity`, `pitch_velocity`. Extract `LookInputs` into a new module `postretro/src/input/look.rs` re-exported from `input/mod.rs`. `input/mod.rs` is already past 500 lines before this plan lands, so per the development guide the extraction happens up front, not conditionally.

**Gamepad velocity extraction.** Raw gamepad values live in `gamepad_axes: HashMap<GilrsAxis, f32>`, keyed by physical axis — not by `Action`. The velocity fields of `LookInputs` therefore cannot be read by direct map lookup; they must be resolved through the binding table. `bindings::resolve_axis_values` already performs this resolution (see `postretro/src/input/bindings.rs`): for a given action it walks `self.bindings`, matches gamepad-axis bindings, looks up raw values, applies `binding.scale`, and returns an `AxisValue` tagged `Velocity`. `drain_look_inputs()` calls `resolve_axis_values` for `Action::LookYaw` and `Action::LookPitch`, splits the returned slice by `AxisSource::Displacement` / `AxisSource::Velocity`, and copies the results into `LookInputs` fields. Sharing the existing resolver avoids a parallel code path.

**Mouse displacement.** The displacement branch reads the already-scaled value out of `mouse_axes` (populated by `resolve_mouse_axes`, which applies sensitivity, invert-Y, and `binding.scale`). `drain_look_inputs()` calls `resolve_mouse_axes()` first to refresh `mouse_axes`, then copies the look-action entries into `LookInputs`, then zeros `mouse_delta` and removes the look-action entries from `mouse_axes` so a subsequent `snapshot()` in the same frame does not re-emit them. Non-look mouse-axis bindings (none today, but the structure permits them) are untouched.

**Delta methods.** `LookInputs::yaw_delta(frame_dt: f32) -> f32` returns `self.yaw_displacement + self.yaw_velocity * GAMEPAD_LOOK_SENSITIVITY * frame_dt`. `pitch_delta` is symmetric. These are pure math on `LookInputs`, not methods on `InputSystem`.

**Interaction with `snapshot()`.** After `drain_look_inputs()` has zeroed `mouse_delta` and cleared the look entries from `mouse_axes`, a later `snapshot()` in the same frame will not emit any `Displacement`-sourced `LookYaw` / `LookPitch` entries — `resolve_axis_values` drops the zero results (`bindings.rs` lines 104–110).

`Velocity`-sourced entries are a different story. `drain_look_inputs()` deliberately does not clear `gamepad_axes` — gamepad stick state is persistent, not evanescent, and clearing it would break the next frame's drain. So when `snapshot()` re-runs `resolve_axis_values(LookYaw, …)` with a deflected stick, it still emits a `Velocity` entry. This is harmless: Task 3 removes every consumer of `snapshot.axis(LookYaw)` / `snapshot.axis(LookPitch)` from `main.rs`, and no other crate reads them. The velocity entry exists in the returned `ActionSnapshot` but has no reader. Plan consumers must not regress this — if a new consumer of `snapshot.axis(LookYaw/LookPitch)` is added later, it would double-apply gamepad look against `drain_look_inputs()`. Movement and button resolution are unaffected throughout.

**Acceptance criteria:**
- [ ] `LookInputs` struct lives in `postretro/src/input/look.rs` with four `f32` fields and `yaw_delta` / `pitch_delta` methods; re-exported from `input/mod.rs`
- [ ] `drain_look_inputs()` returns correct yaw/pitch displacement from mouse delta accumulated since last call
- [ ] `drain_look_inputs()` returns correct yaw/pitch velocity from current gamepad axes, resolved through `bindings::resolve_axis_values` (not direct `gamepad_axes` lookup)
- [ ] After `drain_look_inputs()`, calling `snapshot()` in the same frame returns no `Displacement`-sourced `LookYaw` / `LookPitch` entries; movement and button states still resolve correctly. Persistent `Velocity`-sourced entries (from a deflected gamepad stick) may still appear and are harmless because Task 3 removes all consumers
- [ ] `LookInputs::yaw_delta(frame_dt)` and `pitch_delta(frame_dt)` produce correct combined deltas (displacement + velocity * sensitivity * frame_dt)
- [ ] No per-frame heap allocation in `drain_look_inputs()` beyond what `resolve_axis_values` already does; `LookInputs` is a stack value
- [ ] No `unsafe`
- [ ] Unit tests (in `input/mod.rs` tests block or `input/look.rs`):
  - `drain_look_inputs_returns_mouse_displacement` — accumulate a known delta, call drain, assert displacement matches expected radians
  - `drain_look_inputs_returns_gamepad_velocity` — set a gamepad look axis, call drain, assert velocity matches the bound scaled value
  - `drain_look_inputs_clears_mouse_displacement_for_subsequent_snapshot` — accumulate mouse delta only (no gamepad), drain, then snapshot; snapshot has no `Displacement`-sourced `LookYaw` / `LookPitch` entries
  - `drain_look_inputs_leaves_gamepad_velocity_in_subsequent_snapshot` — set a gamepad stick, drain, then snapshot; snapshot still contains a `Velocity`-sourced `LookYaw` entry (pins the intentional non-clearing of `gamepad_axes` so a future change that decides to clear it surfaces here)
  - `drain_look_inputs_does_not_clear_movement_axes` — W key held, drain look, snapshot still shows `MoveForward` active
  - `look_inputs_yaw_delta_combines_displacement_and_velocity` — known displacement + known velocity + frame_dt, assert combined delta

**Depends on:** none (Task 1 and Task 2 are independent)

---

### Task 3: Move look rotation to render rate, and route rendering angles through `self.camera`

**Description:** Restructure the `RedrawRequested` handler in `main.rs` so look rotation runs once per frame before the tick loop. Rewrite `InterpolableState` and `InterpolableState::view_projection` so the rendering path reads yaw/pitch from `self.camera` directly — without this, `InterpolableState` continues to carry stale angles on zero-tick frames and the rendered view is still lossy. This task bundles both changes because they share call sites and must ship together.

Concrete changes to `frame_timing.rs`:

1. Drop `yaw` and `pitch` fields from `InterpolableState`. Update the constructor to `InterpolableState::new(position: Vec3)`.
2. Update `InterpolableState::lerp` to interpolate position only.
3. Change `InterpolableState::view_projection` signature to `fn view_projection(&self, aspect: f32, yaw: f32, pitch: f32) -> Mat4`. The body uses the passed-in yaw/pitch to build `look_dir`; position still comes from `self.position`.
4. Update `FrameTiming::new` and `FrameTiming::push_state` to pass only position through.
5. Update existing `frame_timing.rs` unit tests: any test that constructs `InterpolableState::new(pos, yaw, pitch)` collapses to `InterpolableState::new(pos)`, and any `view_projection` test passes explicit yaw/pitch arguments. `lerp_angle` and its tests stay (the function remains useful elsewhere and is not deleted).

Concrete changes to `main.rs`:

1. Update the top-level `run()` construction: `let initial_state = InterpolableState::new(initial_camera_pos, 0.0, 0.0);` (around line 161, outside `RedrawRequested`) collapses to `InterpolableState::new(initial_camera_pos)`. The sibling `Camera::new(initial_camera_pos, 0.0, 0.0)` a few lines below is unchanged — it's a `Camera`, not an `InterpolableState`.
2. After `begin_frame` / gamepad poll: call `self.input_system.drain_look_inputs()` to get `look`.
3. Call `self.input_system.snapshot()` as today for movement/button state.
4. Before the tick loop, apply: `self.camera.rotate(look.yaw_delta(frame_dt), look.pitch_delta(frame_dt));`
5. Remove from inside `for _ in 0..ticks`: the `yaw_delta` / `pitch_delta` accumulators, the `AxisSource::Displacement` and `AxisSource::Velocity` match arms for look axes, and `self.camera.rotate(...)`.
6. Remove the `look_yaw_values` and `look_pitch_values` locals.
7. `push_state` inside the tick loop stores only position: `InterpolableState::new(self.camera.position)`.
8. The rendering call becomes `interp.view_projection(self.camera.aspect(), self.camera.yaw, self.camera.pitch)`.

After these changes the tick loop body contains only: movement direction computation, `self.camera.position += ...`, and `self.frame_timing.push_state(...)`.

**Acceptance criteria:**
- [ ] `InterpolableState` has no `yaw` or `pitch` fields; its constructor takes `position` only
- [ ] `InterpolableState::view_projection` takes `aspect`, `yaw`, `pitch` as arguments
- [ ] `self.camera.rotate` is called exactly once per frame, immediately before the tick loop
- [ ] On a zero-tick frame, a non-zero mouse delta rotates the camera AND the rendered view-projection reflects the new angle (the core regression test — verifying the camera alone is not sufficient because rendering reads via `view_projection`)
- [ ] WASD movement still works correctly at tick rate (unchanged logic in tick loop)
- [ ] `look_yaw_values` and `look_pitch_values` locals are gone from `RedrawRequested`
- [ ] No per-frame allocation introduced
- [ ] No `unsafe`
- [ ] `GAMEPAD_LOOK_SENSITIVITY` is consumed inside `LookInputs::*_delta()`; its import in `main.rs` is removed if no other call site references it
- [ ] `cargo test -p postretro` passes (including the updated `frame_timing.rs` tests)

**Depends on:** Task 1 (needs `frame_dt` from `FrameTickResult`), Task 2 (needs `drain_look_inputs()` and `LookInputs`)

---

### Task 4: Regression tests — mouse delta not lost on zero-tick frame

**Description:** Add tests that pin the failure mode at the level that would have caught the bug. A frame with accumulated mouse delta and `ticks == 0` must both rotate `self.camera` AND produce a rendered view-projection matrix that reflects the new angle. Checking only `camera.yaw` is not sufficient — the original bug was that the rendered view stayed stale even when the camera moved, because rendering reads through `InterpolableState::view_projection`. Tests operate on `InputSystem`, `FrameTiming`, and `Camera` directly; no window, renderer, or GPU context.

Tests live in `postretro/src/main.rs` under `#[cfg(test)] mod tests` (main is a binary crate; keeping tests in the binary avoids making internal types `pub` for the sake of a `tests/` integration file).

**Test 1: `mouse_delta_applied_on_zero_tick_frame`.**
1. Create an `InputSystem` with default bindings and a `Camera` at origin, yaw 0.
2. Accumulate a known mouse delta: `sys.handle_mouse_delta(100.0, 0.0)`.
3. Call `sys.drain_look_inputs()`.
4. Call `FrameTiming::accumulate(Duration::from_millis(5))`. Assert `ticks == 0` and `frame_dt > 0`.
5. Apply `camera.rotate(look.yaw_delta(frame_dt), look.pitch_delta(frame_dt))`.
6. Assert `camera.yaw != 0.0`.
7. Build a baseline `view_projection(aspect, 0.0, 0.0)` and the post-rotation `view_projection(aspect, camera.yaw, camera.pitch)` from an `InterpolableState` at a fixed position. Assert the two matrices differ — this is the test that would have caught the original bug.

**Test 2: `mouse_delta_not_multiplied_on_multi_tick_frame`.**
1. Same setup, same mouse delta.
2. Call `FrameTiming::accumulate(TICK_DURATION * 3)` to force `ticks == 3`.
3. Apply look rotation once before simulating the tick loop.
4. Assert `camera.yaw` equals the single-application delta within epsilon — not 3× that delta.

**Acceptance criteria:**
- [ ] `mouse_delta_applied_on_zero_tick_frame` exists, passes, and asserts on both `camera.yaw` and the rendered `view_projection` matrix
- [ ] `mouse_delta_not_multiplied_on_multi_tick_frame` exists and passes
- [ ] Tests use `FrameTiming::accumulate` with a deterministic `Duration`, not `Instant::now()` (see `testing_guide.md` §3 "Deterministic time")
- [ ] Float comparisons use an explicit epsilon (see `testing_guide.md` §3 "Floating-point comparison")
- [ ] No GPU context required

**Depends on:** Task 2, Task 3 (tests exercise the integrated behavior)

---

## Sequencing

**Phase 1 (concurrent — no shared files):**
- Task 1 — `frame_timing.rs`: adds `frame_dt` to `FrameTickResult`
- Task 2 — `input/mod.rs` and new `input/look.rs`: adds `drain_look_inputs()` and `LookInputs`

**Phase 2 (sequential — depends on Phase 1):**
- Task 3 — `frame_timing.rs` and `main.rs`: reshapes `InterpolableState` and `view_projection`, restructures the `RedrawRequested` handler

**Phase 3 (sequential — depends on Phase 2):**
- Task 4 — regression tests in `main.rs`: exercises the full integrated behavior from Tasks 1–3

Tasks 1 and 2 can be implemented concurrently by two agents or as two sequential commits — they share no files. Task 3 requires both because it references `frame_dt` and `drain_look_inputs()`, and it also re-touches `frame_timing.rs` (signature of `InterpolableState::view_projection`) — so Task 3 must land after Task 1, not in parallel. Task 4 requires Task 3 because it tests the integrated behavior in `main.rs`.

---

## Notes

### Research sources

- **Glenn Fiedler, "Fix Your Timestep!" (gafferongames.com):** The foundational accumulator pattern Postretro already implements. The article establishes state interpolation for rendering but is deliberately silent on input — it assumes the reader handles evanescent inputs outside the tick loop. The interpolated `state = currentState * alpha + previousState * (1 - alpha)` model used by Postretro's `interpolated_state()` is directly from this article.

- **id Tech 3, `code/client/cl_input.c` (github.com/id-Software/Quake-III-Arena):** `CL_MouseMove` updates `cl.viewangles[YAW/PITCH]` directly from accumulated `cl.mouseDx/mouseDy` once per rendered frame, scaled by sensitivity. View angles are client-side, render-rate state — not server-tick state. One `usercmd_t` per rendered frame, with the freshest angles baked in. `frame_msec` is clamped to 200ms to prevent spiral-of-death. This is the canonical "view at render rate, sim at tick rate" split.

- **Unity Character Controller docs (docs.unity3d.com/Packages/com.unity.charactercontroller@1.4):** Explicit architectural guidance that evanescent inputs (button edges, mouse delta) must be handled in `Update()` (render rate), not `FixedUpdate()` (physics rate). The `FixedInputEvent` pattern tracks whether an event occurred since the last fixed update. Validates the same distinction: displacement = render rate, persistent state = tick rate.

- **Jakub Tomsu, "Reliable fixed timestep & inputs" (jakubtomsu.github.io):** Identifies the exact bug: "Inputs are lost when there are no ticks to run for few frames." Proposes dividing mouse delta across ticks (`tick_input.cursor_delta /= f32(num_ticks)`) as an alternative to render-rate look. This plan rejects division-across-ticks in favor of render-rate look (see Alternatives below) but the article correctly diagnoses the problem and confirms it is well-known.

### Alternatives considered

**Sub-tick accumulation / divide across ticks:** The approach currently in the bug state: `yaw_delta += av.value / ticks as f32`. This is wrong when `ticks == 0` (division produces NaN or the branch is skipped). Jakub Tomsu's article suggests dividing the delta across ticks, which requires guarding `ticks == 0` specially. This is a band-aid: it correctly distributes the delta across ticks but does not give the player render-rate responsiveness. With vsync off at 300Hz render / 60Hz tick, look updates only happen 60 times per second. Compare id Tech 3: viewangles update 300 times per second in this scenario. The division-across-ticks approach is simpler to implement but inferior in feel. Rejected.

**Keep yaw/pitch in `InterpolableState` and rely on `push_state` writing fresh values:** An earlier draft of this plan kept the fields, on the theory that `self.camera.yaw` would be updated before `push_state` so both state slots would hold the same angle and interpolation would be a no-op. This is only true at `push_state` time. On a zero-tick frame `push_state` never runs, so `previous_state.yaw` and `current_state.yaw` retain whatever angle was written by the last tick — potentially many render frames ago at 240 Hz / 60 Hz tick. Since rendering reads `InterpolableState` via `view_projection`, the rendered view would stay stale even though `self.camera.yaw` was current. Updating only `self.camera` does not fix the symptom. Rejected.

**Update `InterpolableState.current_state.yaw/pitch` every render frame alongside `self.camera`:** This makes `InterpolableState` render-rate-current without changing `view_projection`'s signature. It works, but it muddles the contract — `InterpolableState` becomes "mostly tick-rate, except these two fields." `push_state` would be the exclusive writer for position and a non-exclusive writer for angles. Error-prone. Rejected in favor of the cleaner split: rendering takes yaw/pitch from `self.camera` as explicit arguments, `InterpolableState` interpolates position only.

**Moving gamepad look to render rate via a separate "velocity integrator":** Some engines maintain a dedicated "view integrator" struct that consumes velocity inputs and produces angles. Postretro doesn't need this abstraction — `LookInputs::*_delta(frame_dt)` does the integration inline, which is sufficient with only one velocity-source device (gamepad). Three similar lines beat a premature helper (development guide §1.4).

### Open questions

1. **`drain_look_inputs()` + `snapshot()` call order.** The plan calls `drain_look_inputs()` before `snapshot()`. Both internally call `resolve_mouse_axes()`. Calling `drain_look_inputs()` first means `mouse_delta` is zeroed before `snapshot()` runs, so `snapshot()` sees no mouse displacement — intentional, since look axes are consumed by `drain_look_inputs()`. Document this ordering with a one-line comment in `main.rs` so a future reader does not swap the calls.
