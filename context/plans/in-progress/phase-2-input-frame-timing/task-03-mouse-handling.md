# Task 03: Mouse Handling

> **Dependencies:** Task 02 (input core types exist)
> **Related:** `context/lib/input.md` §4 (mouse handling contract)

---

## Goal

Implement mouse-specific input: raw motion via winit `DeviceEvent::MouseMotion`, delta accumulation between frames, cursor capture/release, sensitivity scalar, and invert-Y. Mouse input feeds into the action snapshot built in Task 02.

---

## Implementation Guidance

### Raw motion

Use winit's `DeviceEvent::MouseMotion` for raw deltas, bypassing OS acceleration. This is essential for consistent aiming -- OS pointer acceleration varies across platforms and user settings.

Request raw input via:
- `window.set_cursor_grab(CursorGrabMode::Locked)` -- locks cursor to window center, provides raw deltas.
- `window.set_cursor_visible(false)` -- hides cursor during gameplay.

### Delta accumulation

Mouse motion events arrive between frames at OS-determined rates. Sum all mouse motion deltas between frames into a single (dx, dy) pair. Apply sensitivity scalar and invert-Y flag once to produce the final look-axis values in the action snapshot.

This prevents lost motion at low framerates and jitter at high framerates.

### Cursor capture

| Event | Action |
|-------|--------|
| Gameplay active | Lock and hide cursor. |
| `WindowEvent::Focused(false)` | Release cursor, make visible. |
| Focus regained during gameplay | Re-capture and hide cursor. |

### Platform fallback

`CursorGrabMode::Locked` is preferred but falls back to `Confined` on platforms that don't support it (some Linux window managers). Both prevent cursor escape; only Locked provides consistent raw deltas. Attempt Locked first; if it fails, fall back to Confined and log a warning.

### Defaults

| Parameter | Default | Notes |
|-----------|---------|-------|
| Sensitivity | 0.002 | Radians per raw mouse unit. Tuned for 800 DPI mice at comfortable speed. Adjust during testing. |
| Invert Y | false | Standard FPS convention. |

### Mouse buttons

Mouse buttons (Left, Right, Middle) are handled as button inputs through the binding system from Task 02. They arrive as `WindowEvent::MouseInput` events. Process them the same as keyboard keys -- update per-button pressed/released tracking, resolve through bindings.

---

## Key Decisions

| Topic | Decision |
|-------|----------|
| Motion source | `DeviceEvent::MouseMotion` raw deltas, not cursor position. |
| Sensitivity default | 0.002 rad/raw unit. Needs testing, may adjust. |
| Cursor grab mode | Locked preferred, Confined fallback. |

---

## Acceptance Criteria

1. Mouse look uses raw deltas, not cursor position. OS acceleration does not affect look speed.
2. Mouse deltas accumulate correctly between frames -- no lost motion at low framerates.
3. Sensitivity scalar converts raw deltas to radians. Changing the value visibly affects look speed.
4. Invert-Y flag negates the pitch axis when enabled.
5. Cursor locks and hides during gameplay. Focus loss releases the cursor. Focus regain re-captures.
6. Mouse buttons (left click, right click) produce correct button action state through the binding system.
