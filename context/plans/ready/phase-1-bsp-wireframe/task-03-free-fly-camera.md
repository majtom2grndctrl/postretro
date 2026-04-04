# Task 03: Free-Fly Camera

> **Phase:** 1 — BSP Loading and Wireframe
> **Dependencies:** task-02 (renderer — provides the uniform buffer and per-frame update path for the view-projection matrix).
> **Produces:** navigable wireframe. Camera position also consumed by task-04 (PVS leaf lookup).

---

## Goal

Implement a free-fly camera with raw winit keyboard/mouse input. Compute view and projection matrices and upload the combined view-projection uniform each frame. This is a temporary navigation camera — Phase 2 replaces the input handling with an action-mapped system, but the projection parameters carry forward. Keep projection (FOV, clip planes, aspect ratio) separable from input handling so Phase 2 can replace input without rewriting projection.

---

## Implementation Guidance

### Projection

| Parameter | Value |
|-----------|-------|
| Horizontal FOV | 100 degrees |
| Near clip | 0.1 units |
| Far clip | 4096.0 units |
| Aspect ratio | Derived from window dimensions, updated on resize |
| Vertical FOV | Derived: `2 * atan(tan(hfov/2) * height/width)` |

### Movement

| Parameter | Value |
|-----------|-------|
| Speed | 320 units/sec (Quake player speed) |
| Sprint modifier | 2x speed while holding Shift |
| Vertical | Q to descend, E to ascend (world-relative, not view-relative) |
| Frame-rate independence | Multiply speed by delta time (wall-clock dt — Phase 1 has no fixed timestep) |

Movement direction: forward is derived from yaw only (no pitch component in movement vector). Looking down doesn't slow forward movement. Strafe is perpendicular to forward in the XZ plane.

### Rotation

| Parameter | Value |
|-----------|-------|
| Input | Raw mouse delta from winit `DeviceEvent::MouseMotion` |
| Conversion | `delta_pixels * sensitivity` produces rotation in radians |
| Sensitivity | 0.002 rad/pixel (approximate — Phase 2 tunes the canonical value) |
| Yaw | Horizontal mouse delta rotates around world Y axis |
| Pitch | Vertical mouse delta rotates around camera's local X axis |
| Pitch clamp | +/- 89 degrees from horizontal (prevents gimbal lock at poles) |
| Roll | Always zero. No roll input. |

### Orientation storage

Store orientation as yaw + pitch angles (two `f32` values), not a quaternion. Rebuild the view matrix each frame from angles. This avoids accumulated floating-point drift and makes the pitch clamp trivial.

### Mouse capture

- Use `window.set_cursor_grab(CursorGrabMode::Confined)` (or `Locked` if the platform supports it).
- Use `window.set_cursor_visible(false)`.
- Uncapture on Escape (before closing) so the cursor returns to normal.

### View-projection upload

Each frame:
1. Compute the view matrix from camera position and yaw/pitch angles.
2. Compute the perspective projection matrix from FOV and window aspect ratio.
3. Multiply: `projection * view`.
4. Write the result to the uniform buffer created in task-02.

---

## Key Decisions

| Item | Resolution |
|------|------------|
| Orientation representation | Yaw + pitch angles. Not quaternion. Rebuild view matrix each frame. |
| Mouse input source | `DeviceEvent::MouseMotion` for raw deltas. Not `WindowEvent::CursorMoved`. |
| Frame timing | Wall-clock delta time between frames. No fixed timestep in Phase 1. |
| Cursor grab mode | `Confined` with `Locked` as preferred alternative if supported. |

---

## Acceptance Criteria

1. WASD moves the camera through BSP geometry. Movement is smooth and frame-rate independent.
2. Mouse rotates the camera. Yaw is unconstrained; pitch clamps at +/- 89 degrees.
3. Q descends, E ascends (world-relative).
4. Shift doubles movement speed.
5. Mouse is captured on focus; Escape releases the cursor before exit.
6. View-projection matrix updates each frame — wireframe perspective changes as the camera moves.
7. Window resize updates the aspect ratio without distortion.
