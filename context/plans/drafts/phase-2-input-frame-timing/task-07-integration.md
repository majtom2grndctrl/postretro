# Task 07: Integration and Cleanup

> **Dependencies:** Task 06 (action-driven camera is working)
> **Related:** `context/lib/input.md` · `context/lib/rendering_pipeline.md` §1

---

## Goal

Remove Phase 1 raw input handling, verify all systems work together, and confirm the phase acceptance criteria are met. This is the final integration pass.

---

## Implementation Guidance

### Remove Phase 1 raw input

All input now flows through the action snapshot. Remove:

- Direct winit key event handling in the camera module.
- Any raw `KeyCode` checks outside the input subsystem.
- Any cursor grab/release logic that duplicates what the input subsystem now owns.

The camera module should have zero dependency on winit types. It reads the action snapshot and nothing else.

### Verification matrix

| Test | What to verify |
|------|---------------|
| Keyboard + mouse | WASD movement and mouse look drive camera through action layer. |
| Gamepad only | Analog sticks drive movement and look. Dead zones prevent drift at rest. |
| Simultaneous input | Keyboard + gamepad active in the same frame. Binding resolution produces correct results (OR for buttons, highest-magnitude for axes). |
| Frame timing stability | Camera motion is smooth at 30 Hz, 60 Hz, and 144+ Hz render rates. No stuttering at non-multiple rates. |
| Long stall recovery | Simulate with a sleep or breakpoint. Camera does not teleport. Accumulator clamp limits catch-up ticks. |
| Cursor capture | Cursor locks during gameplay. Alt-tab releases. Re-focus re-captures. |
| Invert-Y | Toggle invert-Y for mouse and gamepad. Pitch axis negates correctly. |
| First frame | No blending artifact on startup -- initial state renders correctly. |

### Cleanup scope

- Remove dead code from Phase 1 input handling.
- Ensure no `// TODO` markers are left without follow-up tasks.
- Verify logging follows conventions: `[Input]` subsystem tag, no per-frame spam.

---

## Key Decisions

| Topic | Decision |
|-------|----------|
| Cleanup scope | Phase 1 raw input only. Do not refactor unrelated code. |
| Camera winit dependency | Zero. Camera reads action snapshot only. |

---

## Acceptance Criteria

All Phase 2 acceptance criteria must pass:

1. Camera navigates the wireframe BSP using keyboard and mouse through the action-mapping layer. Raw winit input is no longer directly consumed by the camera.
2. Camera navigates using a gamepad (analog sticks for movement and look). Dead zones prevent drift.
3. Keyboard, mouse, and gamepad can be used simultaneously. Binding resolution produces correct results.
4. Frame timing is stable: 60 Hz fixed timestep runs regardless of render rate. Camera motion is smooth at both low (30 Hz) and high (144+ Hz) render rates.
5. Long stalls do not cause camera teleportation.
6. Mouse capture locks and hides cursor during navigation. Focus loss releases. Focus regain re-captures.
7. Invert-Y flag negates pitch axis for both mouse and gamepad.
8. Interpolation produces smooth camera motion between ticks. No visible stuttering at render rates that are not multiples of the tick rate.
9. No Phase 1 raw input code remains in the camera module.
