# Phase 2: Input and Frame Timing

> **Status:** draft
> **Depends on:** Phase 1 (BSP wireframe exists, free-fly camera works with raw winit input)
> **Related:** `context/lib/input.md` (action mapping contract) · `context/lib/rendering_pipeline.md` §1 (frame structure) · `context/lib/entity_model.md` §5 (update model) · `context/lib/development_guide.md` (stack, conventions)

---

## Goal

Phase 2 establishes two foundational pieces of infrastructure: the fixed-timestep frame loop and the input action-mapping system. Together they decouple game logic from render rate and decouple game logic from physical input hardware. Every subsequent phase depends on these two systems -- entity updates, audio triggers, player movement, and rendering interpolation all build on the frame loop and action snapshot introduced here.

---

## Scope

### In scope

- **Fixed-timestep loop** -- accumulator pattern, interpolation factor, delta-time clamping.
- **Input subsystem** -- action mapping for keyboard and mouse (winit), gamepad (gilrs).
- **Action types** -- button (binary on/off with pressed/held/released) and axis (scalar -1 to 1).
- **Binding resolution** -- OR for buttons, highest-magnitude for axes.
- **Mouse handling** -- raw motion, cursor capture/release, sensitivity scalar, invert-Y.
- **Mouse delta accumulation** -- accumulate raw deltas between frames, apply sensitivity once per snapshot.
- **Gamepad support** -- gilrs polling, analog sticks with radial dead zones, trigger axes.
- **Action-driven camera** -- replace the Phase 1 raw winit free-fly camera with a camera that reads from the action-state snapshot.
- **Default bindings** -- complete default binding set for keyboard/mouse and gamepad.

### Out of scope

- Textures, lighting, materials
- Audio
- Entities, collision, gravity
- Config file loading or runtime rebinding UI (bindings are hardcoded defaults)
- Input recording/replay
- Networked input
- Motion controls, touch input, VR/AR input

---

## Task List

| ID | Task | File | Dependencies | Description |
|----|------|------|-------------|-------------|
| 01 | Fixed-timestep loop | `task-01-fixed-timestep.md` | none | Accumulator-based game loop, 60 Hz tick rate, interpolation factor, edge case handling. |
| 02 | Input core types | `task-02-input-core.md` | none | Action ID enum, button state machine, binding resolution, action snapshot, winit keyboard processing. |
| 03 | Mouse handling | `task-03-mouse-handling.md` | 02 | Raw motion deltas, accumulation, cursor capture/release, sensitivity, invert-Y. |
| 04 | Gamepad | `task-04-gamepad.md` | 02 | gilrs polling, radial dead zones, trigger mapping, active gamepad tracking. |
| 05 | Default bindings | `task-05-default-bindings.md` | 02 | Complete keyboard/mouse and gamepad binding tables as hardcoded constants. |
| 06 | Action-driven camera | `task-06-action-camera.md` | 01, 02, 03, 04, 05 | Replace Phase 1 raw camera with snapshot-driven camera inside the fixed-timestep loop. |
| 07 | Integration and cleanup | `task-07-integration.md` | 06 | Remove Phase 1 raw input, verify all inputs work together, confirm acceptance criteria. |

---

## Execution Order

```
        ┌──────────────────┐     ┌──────────────────┐
        │ 01 Fixed-timestep│     │ 02 Input core     │
        └────────┬─────────┘     └──┬────┬────┬──────┘
                 │                   │    │    │
                 │            ┌──────┘    │    └──────┐
                 │            │           │           │
                 │     ┌──────▼───┐ ┌─────▼────┐ ┌───▼──────────┐
                 │     │ 03 Mouse │ │ 04 Gamepd│ │ 05 Bindings  │
                 │     └──────┬───┘ └─────┬────┘ └───┬──────────┘
                 │            │           │           │
                 │            └─────┴─────┴───────────┘
                 │                  │
                 └──────────┬───────┘
                            │
                     ┌──────▼──────┐
                     │ 06 Camera   │
                     └──────┬──────┘
                            │
                     ┌──────▼──────┐
                     │ 07 Integrate│
                     └─────────────┘
```

### Concurrency rules

| Phase | Tasks | Notes |
|-------|-------|-------|
| Wave 1 | 01, 02 | Independent systems. Build in parallel. |
| Wave 2 | 03, 04, 05 | All depend on 02. Independent of each other. Can run in parallel once 02 completes. |
| Wave 3 | 06 | Depends on all prior tasks. Sequential. |
| Wave 4 | 07 | Final integration. Sequential after 06. |

Task 01 has no dependents until Task 06, so it can run alongside Wave 1 and Wave 2 work. The only hard constraint is that 01 must complete before 06 starts.

---

## Acceptance Criteria

1. Camera navigates the wireframe BSP using keyboard and mouse through the action-mapping layer. Raw winit input is no longer directly consumed by the camera.
2. Camera navigates using a gamepad (analog sticks for movement and look). Dead zones prevent drift.
3. Keyboard, mouse, and gamepad can be used simultaneously. Binding resolution produces correct results (OR for buttons, highest-magnitude for axes).
4. Frame timing is stable: a 60 Hz fixed timestep runs regardless of render rate. Camera motion is smooth at both low (30 Hz) and high (144+ Hz) render rates.
5. Long stalls (simulate with a sleep or breakpoint) do not cause the camera to teleport -- accumulator clamp limits catch-up ticks.
6. Mouse capture locks and hides the cursor during navigation. Focus loss releases the cursor. Focus regain re-captures.
7. Invert-Y flag negates the pitch axis for both mouse and gamepad.
8. Interpolation produces smooth camera motion between ticks. No visible stuttering at render rates that are not multiples of the tick rate.

---

## What Carries Forward

The fixed-timestep loop and input subsystem are foundational infrastructure. Every subsequent phase inherits them.

| Infrastructure | Consumers |
|---------------|-----------|
| Fixed-timestep loop (accumulator, tick, interpolation) | Phase 7 (player movement at fixed rate), Phase 8 (entity updates at fixed rate), all phases with game logic |
| Action snapshot | Phase 7 (movement reads move/jump actions), Phase 8 (entity interactions read use/shoot), all gameplay systems |
| Mouse handling (raw motion, capture, sensitivity) | All phases -- mouse input is the primary input for an FPS |
| Gamepad support | All phases -- gamepad bindings extend naturally as actions are added |
| Interpolation state pattern (previous + current + alpha) | Phase 8 (entity position interpolation for rendering), any system that needs smooth visuals from fixed-rate updates |
| Button state machine (pressed/held/released) | Phase 7 (jump on pressed, not held), Phase 8 (shoot on pressed, use on pressed), all input-consuming systems |
